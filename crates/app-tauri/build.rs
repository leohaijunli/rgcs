fn main() {
    // The frontend is built by `scripts/run-desktop.sh` (`npm --prefix
    // frontend run build`) because `beforeBuildCommand` is empty — plain
    // `cargo build` does not run it. Vite clears `dist/` before emitting, so
    // a cargo build that overlaps a (failed or missing) frontend build used
    // to embed nothing and fail only at runtime with "asset not found:
    // index.html". Fail here instead, with the fix in the message.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let index = std::path::Path::new(&manifest_dir)
        .join("../../frontend/dist/index.html");
    if !index.exists() {
        panic!(
            "frontend/dist/index.html is missing — build the frontend first: \
             npm --prefix frontend run build"
        );
    }
    // Re-embed whenever the built frontend changes.
    println!("cargo:rerun-if-changed={}", index.display());
    tauri_build::build()
}