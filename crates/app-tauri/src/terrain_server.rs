//! Local quantized-mesh tileset server (plan B): streams a `dem-prep
//! terrain` output folder (`layer.json` + `{z}/{x}/{y}.terrain`) from a
//! loopback-only HTTP server so `Cesium.CesiumTerrainProvider.fromUrl`
//! can load 3D terrain without a cloud dependency (ADR-004 addendum).
//!
//! Deliberately hand-rolled (keep the dependency set lean — same call as
//! `terrain_service::base64_encode`): GET/HEAD/OPTIONS only, read-only,
//! bound to `127.0.0.1` only. The origin is pinned in the desktop CSP
//! (`tauri.conf.json` `connect-src` + `tests/desktop_csp.rs`), so moving
//! [`TERRAIN_PORT`] requires updating both together.
//!
//! Failure/risk analysis: the only attack surface is a local process
//! speaking HTTP to a bound loopback port. Mitigations: no method other
//! than GET/HEAD/OPTIONS, no percent-decoded paths (we only ever emit
//! plain `[0-9/]`-paths, so `%`/`\\` are rejected outright), `..`
//! components rejected, canonicalized paths must stay under the tileset
//! root (blocks symlink escapes), only `layer.json`/`*.terrain` files
//! served, bounded request-line read with a timeout, `Connection: close`
//! per request (no keep-alive state to manage). Port conflicts surface
//! as a command error instead of a silent failure.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Loopback port pinned by the desktop CSP `connect-src`. Changing it
/// means updating `tauri.conf.json` and `tests/desktop_csp.rs` together.
pub const TERRAIN_PORT: u16 = 18823;

/// Bound on the request line (`METHOD target HTTP/1.x`). Real clients
/// send < 100 bytes; a runaway or hostile client cannot make us allocate
/// unbounded memory before we answer 400.
const MAX_REQUEST_LINE: u64 = 4096;

/// Idle read timeout for the request line — a client that connects and
/// goes silent must not pin a connection thread forever.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A running tileset server. Dropping (or [`TerrainServer::stop`]-ing)
/// it stops the accept loop and joins the threads.
#[derive(Debug)]
pub struct TerrainServer {
    /// Resolved bind address (loopback + actual port) — also used to
    /// wake the accept loop on shutdown.
    addr: SocketAddr,
    /// Base URL of `layer.json` — hand straight to
    /// `CesiumTerrainProvider.fromUrl`.
    layer_url: String,
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TerrainServer {
    /// Bind [`TERRAIN_PORT`] and serve `dir` until stopped.
    ///
    /// # Errors
    ///
    /// A stringified error when `dir` is not a folder, carries no
    /// `layer.json` (so it is not a `dem-prep terrain` output), or the
    /// port cannot be bound (already in use).
    pub fn start(dir: PathBuf) -> Result<Self, String> {
        Self::bind(dir, TERRAIN_PORT)
    }

    /// Same as [`TerrainServer::start`] with an explicit port
    /// (`0` = OS-assigned, used by the tests).
    fn bind(dir: PathBuf, port: u16) -> Result<Self, String> {
        if !dir.is_dir() {
            return Err(format!("{}: not a directory", dir.display()));
        }
        let root = dir
            .canonicalize()
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        if !root.join("layer.json").is_file() {
            return Err(format!(
                "{}: no layer.json — run `dem-prep terrain` first",
                root.display()
            ));
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .map_err(|e| format!("cannot bind 127.0.0.1:{port}: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?;
        let running = Arc::new(AtomicBool::new(true));
        let thread_root = root.clone();
        let thread_flag = Arc::clone(&running);
        let handle = std::thread::Builder::new()
            .name("terrain3d-accept".into())
            .spawn(move || accept_loop(listener, thread_root, thread_flag))
            .map_err(|e| format!("spawn terrain server thread: {e}"))?;
        Ok(Self {
            layer_url: format!("http://127.0.0.1:{}/layer.json", addr.port()),
            addr,
            running,
            handle: Some(handle),
        })
    }

    /// Base URL of the served `layer.json` (what the frontend passes to
    /// `CesiumTerrainProvider.fromUrl`).
    pub fn url(&self) -> String {
        self.layer_url.clone()
    }

    /// Actual bound port (tests bind `0`).
    #[cfg(test)]
    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Stop the server and join its threads.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Wake the blocking accept() so the loop observes the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TerrainServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Accept loop: one short-lived thread per connection, because Cesium
/// fetches several tiles in parallel.
fn accept_loop(listener: TcpListener, root: PathBuf, running: Arc<AtomicBool>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                let root = root.clone();
                let spawned = std::thread::Builder::new()
                    .name("terrain3d-conn".into())
                    .spawn(move || {
                        let _ = handle_connection(stream, &root);
                    });
                if spawned.is_err() {
                    return;
                }
            }
            Err(_) => {
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                // Transient accept error (e.g. EMFILE): back off rather
                // than spin, keep serving.
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Result of reading the (single) request line we care about.
enum RequestLine {
    /// Client closed without sending anything (our shutdown probe).
    Closed,
    /// No newline within [`MAX_REQUEST_LINE`] (or unreadable).
    Oversized,
    /// A complete request line (still unparsed).
    Line(String),
}

fn read_request_line(stream: &mut TcpStream) -> RequestLine {
    if stream.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return RequestLine::Closed;
    }
    let reader = BufReader::new(&mut *stream);
    let mut limited = reader.take(MAX_REQUEST_LINE);
    let mut line = String::new();
    match limited.read_line(&mut line) {
        Ok(0) => RequestLine::Closed,
        Ok(_) if line.ends_with('\n') => RequestLine::Line(line),
        Ok(_) => RequestLine::Oversized,
        Err(_) => RequestLine::Closed,
    }
}

fn handle_connection(mut stream: TcpStream, root: &Path) -> std::io::Result<()> {
    let line = match read_request_line(&mut stream) {
        RequestLine::Closed => return Ok(()),
        RequestLine::Oversized => {
            return respond(
                &mut stream,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                "",
                b"400 request line too long\n",
                false,
            );
        }
        RequestLine::Line(line) => line,
    };
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    match method {
        "OPTIONS" => respond_preflight(&mut stream),
        "GET" | "HEAD" => {
            let head_only = method == "HEAD";
            let path = target.split(['?', '#']).next().unwrap_or("");
            match resolve_path(root, path) {
                Ok(file) => serve_file(&mut stream, &file, head_only),
                Err(status) => respond_status(&mut stream, status, head_only),
            }
        }
        _ => respond(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            "Allow: GET, HEAD, OPTIONS\r\n",
            b"405 method not allowed\n",
            false,
        ),
    }
}

/// Map a request path to a file under `root`, or the HTTP status to
/// answer with.
///
/// `root` must already be canonicalized; the candidate is canonicalized
/// too, so a symlink pointing outside the tileset is refused.
fn resolve_path(root: &Path, path: &str) -> Result<PathBuf, u16> {
    if !path.starts_with('/') {
        return Err(400);
    }
    // We only ever generate plain `{z}/{x}/{y}` paths — reject anything
    // percent-encoded (double-decode tricks) or Windows-flavoured.
    if path.contains('%') || path.contains('\\') || path.contains('\0') {
        return Err(400);
    }
    let rel = &path[1..];
    if rel.is_empty() {
        return canonical_under(root, &root.join("layer.json"));
    }
    let mut clean = PathBuf::new();
    for component in rel.split('/') {
        match component {
            "" | "." => continue,
            ".." => return Err(403),
            other => clean.push(other),
        }
    }
    canonical_under(root, &root.join(clean))
}

fn canonical_under(root: &Path, candidate: &Path) -> Result<PathBuf, u16> {
    let canonical = candidate.canonicalize().map_err(|_| 404u16)?;
    if !canonical.starts_with(root) {
        return Err(403); // symlink escape
    }
    let allowed = match canonical.file_name().and_then(|n| n.to_str()) {
        Some(name) => name == "layer.json" || canonical.extension().is_some_and(|e| e == "terrain"),
        None => false,
    };
    if !allowed {
        return Err(404); // not part of the tileset surface
    }
    Ok(canonical)
}

fn serve_file(stream: &mut TcpStream, file: &Path, head_only: bool) -> std::io::Result<()> {
    let is_mesh = file.extension().is_some_and(|e| e == "terrain");
    let (content_type, extra) = if is_mesh {
        // Tiles are stored gzip-compressed (dem-prep `generate_tileset`);
        // the browser decompresses transparently.
        (
            "application/vnd.quantized-mesh",
            "Content-Encoding: gzip\r\n",
        )
    } else {
        ("application/json", "")
    };
    match fs::read(file) {
        Ok(body) => respond(stream, 200, "OK", content_type, extra, &body, head_only),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            respond_status(stream, 404, head_only)
        }
        Err(_) => respond_status(stream, 500, head_only),
    }
}

/// CORS preflight answer (`Access-Control-Allow-*` mirrors what
/// `CesiumTerrainProvider` sends: safelisted `Accept` at most).
fn respond_preflight(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 204 No Content\r\n\
          Access-Control-Allow-Origin: *\r\n\
          Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n\
          Access-Control-Allow-Headers: Accept\r\n\
          Access-Control-Max-Age: 86400\r\n\
          Connection: close\r\n\r\n",
    )?;
    stream.flush()
}

fn respond_status(stream: &mut TcpStream, status: u16, head_only: bool) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let body = format!("{status} {reason}\n").into_bytes();
    respond(
        stream,
        status,
        reason,
        "text/plain; charset=utf-8",
        "",
        &body,
        head_only,
    )
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    extra: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\
         {extra}\r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// Fresh empty temp folder (pid-suffixed so parallel tests never
    /// collide).
    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("maggcs-terrain3d-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("tempdir");
        dir
    }

    fn write_layer_json(dir: &Path) {
        fs::write(
            dir.join("layer.json"),
            br#"{"format":"quantized-mesh-1.0"}"#,
        )
        .expect("layer.json");
    }

    /// Raw HTTP exchange over a TcpStream; returns (head, body).
    fn http(port: u16, request: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        stream.write_all(request.as_bytes()).expect("write");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).expect("read");
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("header terminator");
        (
            String::from_utf8_lossy(&raw[..split]).into_owned(),
            raw[split + 4..].to_vec(),
        )
    }

    fn status_of(head: &str) -> u16 {
        head.split_whitespace()
            .nth(1)
            .expect("status")
            .parse()
            .expect("status code")
    }

    #[test]
    fn serves_layer_json_from_root() {
        let dir = tempdir("layer");
        write_layer_json(&dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        let (head, body) = http(server.port(), "GET /layer.json HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&head), 200);
        assert!(head.contains("Content-Type: application/json"));
        assert!(head.contains("Access-Control-Allow-Origin: *"));
        assert_eq!(body, br#"{"format":"quantized-mesh-1.0"}"#);
        // "/" is an alias for layer.json (Cesium's default fetch).
        let (head, body) = http(server.port(), "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&head), 200);
        assert_eq!(body, br#"{"format":"quantized-mesh-1.0"}"#);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn serves_gzip_terrain_tile() {
        let dir = tempdir("tile");
        write_layer_json(&dir);
        let tile = dir.join("19").join("164903");
        fs::create_dir_all(&tile).expect("z dir");
        let payload = [0x1fu8, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef];
        fs::write(tile.join("403858.terrain"), payload).expect("tile");
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        let (head, body) = http(
            server.port(),
            "GET /19/164903/403858.terrain HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 200);
        assert!(head.contains("Content-Type: application/vnd.quantized-mesh"));
        assert!(head.contains("Content-Encoding: gzip"));
        assert_eq!(body, payload);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_traversal_and_encoded_paths() {
        let dir = tempdir("traversal");
        write_layer_json(&dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        // Raw `..` components (browsers normalize these away before
        // sending — our own client never emits them either).
        let (head, _) = http(
            server.port(),
            "GET /../../etc/passwd HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 403);
        // Percent-encoding is rejected outright (no decoding pass exists).
        let (head, _) = http(
            server.port(),
            "GET /%2e%2e/secret HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 400);
        // Files outside the tileset surface are 404, not served.
        fs::write(dir.join("notes.txt"), b"hi").expect("decoy");
        let (head, _) = http(server.port(), "GET /notes.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&head), 404);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn answers_preflight_and_head() {
        let dir = tempdir("cors");
        write_layer_json(&dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        let (head, body) = http(
            server.port(),
            "OPTIONS /layer.json HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 204);
        assert!(head.contains("Access-Control-Allow-Origin: *"));
        assert!(head.contains("Access-Control-Allow-Methods: GET, HEAD, OPTIONS"));
        assert!(body.is_empty());
        // HEAD: headers (incl. Content-Length) but no body.
        let (head, body) = http(
            server.port(),
            "HEAD /layer.json HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 200);
        assert!(head.contains("Content-Length: "));
        assert!(body.is_empty());
        // Unsupported methods → 405.
        let (head, _) = http(
            server.port(),
            "POST /layer.json HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert_eq!(status_of(&head), 405);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_rejects_folder_without_layer_json() {
        let dir = tempdir("nomanifest");
        let err = TerrainServer::bind(dir.clone(), 0).expect_err("must fail");
        assert!(err.contains("layer.json"), "got: {err}");
        let err = TerrainServer::bind(dir.join("missing"), 0).expect_err("must fail");
        assert!(err.contains("not a directory"), "got: {err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_joins_and_releases_the_port() {
        let dir = tempdir("stop");
        write_layer_json(&dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        let port = server.port();
        server.stop();
        // The port must be free again (wake-up connect raced the shutdown
        // — either way a fresh bind proves the listener is gone).
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("port released");
        drop(listener);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_points_at_layer_json_on_the_bound_port() {
        let dir = tempdir("url");
        write_layer_json(&dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        assert_eq!(
            server.url(),
            format!("http://127.0.0.1:{}/layer.json", server.port())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Manual smoke against a real `dem-prep terrain` output:
    /// `TERRAIN_SMOKE_DIR=/path/to/tileset cargo test -p maggcs-app --lib -- --ignored`.
    #[test]
    #[ignore = "needs TERRAIN_SMOKE_DIR pointing at a generated tileset"]
    fn serves_a_real_tileset_layer_json_and_a_tile() {
        let Ok(dir) = std::env::var("TERRAIN_SMOKE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let server = TerrainServer::bind(dir.clone(), 0).expect("bind");
        let (head, body) = http(server.port(), "GET /layer.json HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(status_of(&head), 200);
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("layer.json is valid JSON");
        assert_eq!(json["format"], "quantized-mesh-1.0");
        assert_eq!(json["scheme"], "tms");
        assert_eq!(json["projection"], "EPSG:4326");
        let maxzoom = json["maxzoom"].as_u64().expect("maxzoom") as u8;
        assert!(maxzoom > 0);
        // First available tile at maxzoom (TMS: x west, y south). The
        // descriptor must be an object — Cesium reads `range.startX`.
        let rect = &json["available"][maxzoom as usize][0];
        assert!(
            rect.is_object(),
            "availability descriptor must be an object"
        );
        let x = rect["startX"].as_u64().expect("startX");
        let y = rect["startY"].as_u64().expect("startY");
        let (head, body) = http(
            server.port(),
            &format!("GET /{maxzoom}/{x}/{y}.terrain HTTP/1.1\r\nHost: x\r\n\r\n"),
        );
        assert_eq!(status_of(&head), 200);
        assert!(head.contains("Content-Encoding: gzip"));
        assert_eq!(&body[..2], &[0x1f, 0x8b], "gzip magic");
        // The server streams the file byte-for-byte (mesh-level content
        // is covered by dem-prep's encode/decode tests).
        let on_disk =
            fs::read(dir.join(format!("{maxzoom}/{x}/{y}.terrain"))).expect("tile on disk");
        assert_eq!(body, on_disk, "served bytes match the tileset file");
    }
}
