//! Minimal ULog (PX4) reader for the inspector's replay source (plan decision B).
//!
//! Only what replay needs is implemented: the file header, the `F` (format) and
//! `A` (add-logged) records that give a topic its field layout, and the `D`
//! (data) records. Info/parameter/logged-string/sync records are skipped
//! byte-exactly, so a stream interleaving them still parses.
//!
//! ULog stores uORB topics rather than MAVLink messages, so a topic has no
//! MAVLink id; [`replay`](super::ulog::replay) synthesises a stable one from the
//! topic name and keeps the real name in the catalog.
//!
//! Three format quirks are handled (all verified against a real PX4 log):
//!
//! - A record's `msg_size` counts the payload only; the 3-byte record header is
//!   extra (`payload = size`, not `size - 3`).
//! - A field spec is `type name` with an optional array suffix on the *type*:
//!   `float[3] vel`, `char[8] name`. The type may name another format (a nested
//!   struct), which is sized recursively but not expanded into signals.
//! - PX4 declares trailing `_padding0` fields but does **not** log them, so a
//!   topic's data length is the struct size minus its trailing padding. An
//!   embedded nested struct is sized with its padding *included* (it is no
//!   longer trailing there).
//!
//! Topic instances come from `A` records (`multi_id`, `msg_id`, name); the newer
//! `M` record is not used by PX4 for topics and is ignored.

pub mod replay;

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

/// The 7 fixed magic bytes at the start of every ULog file.
pub const ULOG_MAGIC: [u8; 7] = [0x55, 0x4c, 0x6f, 0x67, 0x01, 0x12, 0x35];
/// File header: magic(7) + version(1) + timestamp(8).
const HEADER_LEN: u64 = 16;
/// Record header: `u16 payload_len` + `u8 msg_type`.
const RECORD_HEADER_LEN: u64 = 3;
/// Guard against a corrupt size field making us allocate unboundedly.
const MAX_PAYLOAD: usize = 1 << 20;
/// Nested-type recursion limit.
const MAX_NESTING: usize = 16;

/// Everything that can go wrong reading a ULog.
#[derive(Debug, thiserror::Error)]
pub enum UlogError {
    #[error("not a ULog file (bad magic)")]
    BadMagic,
    #[error("unsupported ULog version {0}")]
    UnsupportedVersion(u8),
    #[error("truncated ULog file")]
    Truncated,
    #[error("malformed ULog record: {0}")]
    BadFormat(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A ULog field type. `Char` is sized but never emitted as a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTy {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    Char,
}

impl FieldTy {
    /// Map a format type name to a field type (`bool` is one unsigned byte).
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "int8_t" => Self::I8,
            "uint8_t" | "bool" => Self::U8,
            "int16_t" => Self::I16,
            "uint16_t" => Self::U16,
            "int32_t" => Self::I32,
            "uint32_t" => Self::U32,
            "int64_t" => Self::I64,
            "uint64_t" => Self::U64,
            "float" => Self::F32,
            "double" => Self::F64,
            "char" => Self::Char,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 | Self::Char => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }

    /// Read one little-endian scalar from the start of `b` (short slices yield
    /// `NaN` rather than an error, so a truncated record still plots).
    fn read(self, b: &[u8]) -> f64 {
        if b.len() < self.size() {
            return f64::NAN;
        }
        match self {
            Self::I8 => b[0] as i8 as f64,
            Self::U8 | Self::Char => b[0] as f64,
            Self::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
            Self::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
            Self::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Self::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Self::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            Self::I64 => {
                i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f64
            }
            Self::U64 => {
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f64
            }
            Self::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        }
    }
}

/// One scalar component of a topic, with its byte offset inside a data body.
#[derive(Debug, Clone, PartialEq)]
pub struct UlogField {
    /// `name` for scalars, `name[i]` for array elements (the same convention
    /// `signals::extract` uses for MAVLink arrays).
    pub name: String,
    pub offset: usize,
    pub ty: FieldTy,
}

/// A resolved topic: its layout plus the numeric signals it produces.
#[derive(Debug, Clone, PartialEq)]
pub struct UlogTopic {
    /// uORB topic name, e.g. `vehicle_attitude`.
    pub name: String,
    /// Instance number from the `A` record.
    pub multi_id: u8,
    /// Bytes of one `D` body, i.e. after the 2-byte topic id.
    pub payload_len: usize,
    /// Numeric components; array fields expanded, `char`/nested/`_padding` left
    /// out.
    pub fields: Vec<UlogField>,
}

impl UlogTopic {
    /// Decode every numeric component of `body` into `out`, in `fields` order.
    pub fn decode(&self, body: &[u8], out: &mut Vec<f64>) {
        out.clear();
        for f in &self.fields {
            let end = f.offset + f.ty.size();
            out.push(match body.get(f.offset..end) {
                Some(slice) => f.ty.read(slice),
                None => f64::NAN,
            });
        }
    }

    /// The `timestamp` field (µs), if the topic carries one. PX4 logs it first.
    pub fn timestamp_us(&self, body: &[u8]) -> Option<u64> {
        let f = self
            .fields
            .iter()
            .find(|f| f.name == "timestamp" && f.ty == FieldTy::U64)?;
        let v = f.ty.read(body.get(f.offset..f.offset + 8)?);
        (v.is_finite() && v >= 0.0).then_some(v as u64)
    }
}

/// The file header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UlogHeader {
    pub version: u8,
    /// Log start time, µs (PX4 boot clock).
    pub timestamp_us: u64,
}

/// What `next_record` found.
#[derive(Debug, Clone, PartialEq)]
pub enum UlogRecord {
    /// A `D` record; its body is available from [`UlogReader::data_body`] and
    /// its layout from [`UlogReader::topics`].
    Data { topic_id: u16 },
    /// A record replay does not use (info, parameter, logged string, …),
    /// already skipped; the byte is the ULog message type.
    Skipped(u8),
}

/// One field of a `F` record before nested types are resolved.
#[derive(Debug, Clone, PartialEq)]
struct RawField {
    ty: String,
    arr: usize,
    name: String,
}

/// A `F` record: the raw field list, keyed by topic name.
#[derive(Debug, Clone, PartialEq, Default)]
struct FormatDef {
    fields: Vec<RawField>,
}

/// A fully sized layout.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    /// Bytes written for a top-level topic (trailing padding omitted).
    logged_len: usize,
    /// Bytes of the full struct, padding included (used when embedded).
    full_len: usize,
    /// Numeric, non-padding scalar components with their offsets.
    signals: Vec<UlogField>,
}

/// Streaming ULog reader over any seekable source. Keeps the header and the
/// topic table across seeks, so replay can jump around the file.
pub struct UlogReader<S: Read + Seek> {
    src: S,
    pub header: UlogHeader,
    /// `msg_id` → (instance, topic name) from the `A` records.
    instances: HashMap<u16, (u8, String)>,
    /// Topic name → raw field list from the `F` records.
    formats: HashMap<String, FormatDef>,
    /// Fully resolved topics, keyed by `msg_id`.
    pub topics: HashMap<u16, UlogTopic>,
    body: Vec<u8>,
    /// Byte offset just past the record last returned.
    pub position: u64,
}

impl<S: Read + Seek> UlogReader<S> {
    /// Read the file header and leave the source at the first record.
    pub fn open(mut src: S) -> Result<Self, UlogError> {
        let mut head = [0u8; HEADER_LEN as usize];
        src.read_exact(&mut head).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                UlogError::Truncated
            } else {
                UlogError::Io(e)
            }
        })?;
        if head[..7] != ULOG_MAGIC {
            return Err(UlogError::BadMagic);
        }
        let version = head[7];
        if version > 1 {
            return Err(UlogError::UnsupportedVersion(version));
        }
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&head[8..16]);
        Ok(Self {
            src,
            header: UlogHeader {
                version,
                timestamp_us: u64::from_le_bytes(ts),
            },
            instances: HashMap::new(),
            formats: HashMap::new(),
            topics: HashMap::new(),
            body: Vec::new(),
            position: HEADER_LEN,
        })
    }

    /// The body of the last `D` record (after the 2-byte topic id), or empty if
    /// the last record was not data.
    pub fn data_body(&self) -> &[u8] {
        self.body.get(2..).unwrap_or_default()
    }

    /// Seek to an absolute byte offset (used by replay seeking).
    pub fn seek_to(&mut self, offset: u64) -> Result<(), UlogError> {
        self.src.seek(SeekFrom::Start(offset))?;
        self.position = offset;
        Ok(())
    }

    /// Read the next record. `Ok(None)` means a clean end of file; a record cut
    /// short by a crash is treated as the end of file too.
    pub fn next_record(&mut self) -> Result<Option<UlogRecord>, UlogError> {
        let mut hdr = [0u8; RECORD_HEADER_LEN as usize];
        if !read_full(&mut self.src, &mut hdr)? {
            return Ok(None);
        }
        let size = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
        let ty = hdr[2];
        if size > MAX_PAYLOAD {
            return Err(UlogError::BadFormat(format!("payload size {size}")));
        }
        self.body.clear();
        self.body.resize(size, 0);
        if !read_full(&mut self.src, &mut self.body)? {
            return Ok(None);
        }
        self.position += RECORD_HEADER_LEN + size as u64;

        match ty {
            b'F' => {
                let text = String::from_utf8_lossy(&self.body).into_owned();
                let (name, def) = parse_format(&text)?;
                // Resolve any topic instance that was waiting for this format.
                let waiting: Vec<u16> = self
                    .instances
                    .iter()
                    .filter(|(_, (_, n))| n == &name)
                    .map(|(id, _)| *id)
                    .collect();
                self.formats.insert(name, def);
                for id in waiting {
                    self.try_bind(id);
                }
                Ok(Some(UlogRecord::Skipped(ty)))
            }
            b'A' => {
                if size >= 3 {
                    let multi_id = self.body[0];
                    let id = u16::from_le_bytes([self.body[1], self.body[2]]);
                    let name = String::from_utf8_lossy(&self.body[3..]).into_owned();
                    self.instances.insert(id, (multi_id, name));
                    self.try_bind(id);
                }
                Ok(Some(UlogRecord::Skipped(ty)))
            }
            b'D' => {
                if size < 2 {
                    return Err(UlogError::BadFormat("short data record".into()));
                }
                let topic_id = u16::from_le_bytes([self.body[0], self.body[1]]);
                self.try_bind(topic_id);
                Ok(Some(UlogRecord::Data { topic_id }))
            }
            // 'I' info, 'M' multi-text, 'P'/'Q' parameters, 'L' logged string,
            // 'S' sync, 'O' dropout, 'B' flag bits: nothing replay needs.
            _ => Ok(Some(UlogRecord::Skipped(ty))),
        }
    }

    /// Resolve a waiting topic instance once its format is known.
    fn try_bind(&mut self, id: u16) {
        if self.topics.contains_key(&id) {
            return;
        }
        let Some((multi_id, name)) = self.instances.get(&id).cloned() else {
            return;
        };
        let Ok(layout) = resolve_layout(&name, &self.formats, &mut HashMap::new(), 0) else {
            return;
        };
        self.topics.insert(
            id,
            UlogTopic {
                name,
                multi_id,
                payload_len: layout.logged_len,
                fields: layout.signals,
            },
        );
    }
}

/// Parse a `F` record: `name:type field;type[N] field;…;`.
fn parse_format(text: &str) -> Result<(String, FormatDef), UlogError> {
    let (name, rest) = text
        .split_once(':')
        .ok_or_else(|| UlogError::BadFormat(format!("no ':' in {text:?}")))?;
    let mut fields = Vec::new();
    for part in rest.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (spec, field_name) = part.split_once(' ').ok_or_else(|| {
            UlogError::BadFormat(format!("{name}: field {part:?} has no name"))
        })?;
        let (ty, arr) = split_array(spec)?;
        fields.push(RawField {
            ty: ty.to_string(),
            arr,
            name: field_name.trim().to_string(),
        });
    }
    if fields.is_empty() {
        return Err(UlogError::BadFormat(format!("{name}: no fields")));
    }
    Ok((name.to_string(), FormatDef { fields }))
}

/// Split a type spec into its base name and array length (`float[3]` → 3).
fn split_array(spec: &str) -> Result<(&str, usize), UlogError> {
    match spec.split_once('[') {
        None => Ok((spec, 1)),
        Some((base, tail)) => {
            let len: usize = tail
                .strip_suffix(']')
                .unwrap_or(tail)
                .parse()
                .map_err(|_| UlogError::BadFormat(format!("bad array spec {spec:?}")))?;
            Ok((base, len.max(1)))
        }
    }
}

/// Size a format, resolving nested types recursively. Memoised per call tree.
fn resolve_layout(
    name: &str,
    formats: &HashMap<String, FormatDef>,
    cache: &mut HashMap<String, Layout>,
    depth: usize,
) -> Result<Layout, UlogError> {
    if let Some(l) = cache.get(name) {
        return Ok(l.clone());
    }
    if depth > MAX_NESTING {
        return Err(UlogError::BadFormat(format!("nested type depth at {name}")));
    }
    let def = formats
        .get(name)
        .ok_or_else(|| UlogError::BadFormat(format!("unknown type {name}")))?;

    let mut offset = 0usize;
    // Bytes up to the end of the last non-padding field: PX4 pads structs to
    // 8 bytes but logs only the meaningful part.
    let mut logged_len = 0usize;
    let mut signals = Vec::new();
    for f in &def.fields {
        let (elem_size, ty) = match FieldTy::from_name(&f.ty) {
            Some(ty) => (ty.size(), Some(ty)),
            None => {
                // An embedded struct: size it fully (its padding is interior).
                let nested = resolve_layout(&f.ty, formats, cache, depth + 1)?;
                (nested.full_len, None)
            }
        };
        if let Some(ty) = ty {
            if ty != FieldTy::Char && !f.name.starts_with('_') {
                for i in 0..f.arr {
                    signals.push(UlogField {
                        name: if f.arr == 1 {
                            f.name.clone()
                        } else {
                            format!("{}[{i}]", f.name)
                        },
                        offset: offset + i * elem_size,
                        ty,
                    });
                }
            }
        }
        offset += elem_size * f.arr;
        if !f.name.starts_with('_') {
            logged_len = offset;
        }
    }
    let layout = Layout {
        logged_len,
        full_len: offset,
        signals,
    };
    cache.insert(name.to_string(), layout.clone());
    Ok(layout)
}

/// `read_exact`, reporting a clean end of file when nothing was read at all.
fn read_full<R: Read>(src: &mut R, buf: &mut [u8]) -> Result<bool, UlogError> {
    let mut read = 0;
    while read < buf.len() {
        match src.read(&mut buf[read..]) {
            Ok(0) => return Ok(false),
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a ULog in memory. `size` covers the payload only, so the record on
    /// disk is `3 + payload` bytes — matching a real PX4 log.
    pub(crate) fn record(ty: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = (payload.len() as u16).to_le_bytes().to_vec();
        v.push(ty);
        v.extend_from_slice(payload);
        v
    }

    pub(crate) fn header(start_us: u64) -> Vec<u8> {
        let mut v = ULOG_MAGIC.to_vec();
        v.push(1);
        v.extend_from_slice(&start_us.to_le_bytes());
        v
    }

    pub(crate) fn format_record(text: &str) -> Vec<u8> {
        record(b'F', text.as_bytes())
    }

    pub(crate) fn add_record(multi_id: u8, msg_id: u16, name: &str) -> Vec<u8> {
        let mut p = vec![multi_id];
        p.extend_from_slice(&msg_id.to_le_bytes());
        p.extend_from_slice(name.as_bytes());
        record(b'A', &p)
    }

    pub(crate) fn data_record(msg_id: u16, body: &[u8]) -> Vec<u8> {
        let mut p = msg_id.to_le_bytes().to_vec();
        p.extend_from_slice(body);
        record(b'D', &p)
    }

    /// `sensor`: timestamp + one float + a float[3] + a bool + trailing padding.
    pub(crate) fn sensor_log(start_us: u64) -> Vec<u8> {
        let mut v = header(start_us);
        v.extend(format_record(
            "sensor:uint64_t timestamp;float x;float[3] vel;bool ok;uint8_t[4] _padding0;",
        ));
        v.extend(add_record(0, 7, "sensor"));
        for i in 0..3u64 {
            let mut b = (start_us + i * 1000).to_le_bytes().to_vec();
            b.extend_from_slice(&(1.5 * i as f32).to_le_bytes());
            for j in 0..3 {
                b.extend_from_slice(&((i * 10 + j) as f32).to_le_bytes());
            }
            b.push(i as u8);
            v.extend(data_record(7, &b));
        }
        v
    }

    fn reader(bytes: Vec<u8>) -> UlogReader<Cursor<Vec<u8>>> {
        UlogReader::open(Cursor::new(bytes)).expect("open")
    }

    #[test]
    fn reads_header_and_skips_unknown_records() {
        let mut v = header(172_000);
        v.extend(record(b'B', &[1, 0, 0, 0, 0, 0, 0, 0]));
        v.extend(record(b'I', b"\x03pi=3"));
        let mut r = reader(v);
        assert_eq!(r.header.version, 1);
        assert_eq!(r.header.timestamp_us, 172_000);
        assert!(matches!(r.next_record().unwrap(), Some(UlogRecord::Skipped(b'B'))));
        assert!(matches!(r.next_record().unwrap(), Some(UlogRecord::Skipped(b'I'))));
        assert_eq!(r.next_record().unwrap(), None, "clean EOF");
    }

    #[test]
    fn rejects_a_non_ulog_file() {
        let mut v = vec![0u8; 16];
        v[..3].copy_from_slice(b"abc");
        assert!(matches!(
            UlogReader::open(Cursor::new(v)),
            Err(UlogError::BadMagic)
        ));
    }

    #[test]
    fn a_truncated_record_is_the_end_of_file() {
        let mut v = header(0);
        let rec = format_record("t:float x;");
        v.extend(&rec[..rec.len() - 1]);
        let mut r = reader(v);
        assert_eq!(r.next_record().unwrap(), None);
    }

    #[test]
    fn parses_a_format_and_decodes_data() {
        let mut r = reader(sensor_log(100_000));
        let mut data = 0;
        while let Some(rec) = r.next_record().expect("read") {
            if let UlogRecord::Data { topic_id } = rec {
                assert_eq!(topic_id, 7);
                let topic = r.topics.get(&topic_id).expect("topic bound");
                assert_eq!(topic.name, "sensor");
                assert_eq!(topic.multi_id, 0);
                // 8 + 4 + 12 + 1 = 25; the uint8_t[4] padding is not logged.
                assert_eq!(topic.payload_len, 25);
                assert_eq!(r.data_body().len(), 25);
                assert_eq!(topic.timestamp_us(r.data_body()), Some(100_000 + data * 1000));
                let mut out = Vec::new();
                topic.decode(r.data_body(), &mut out);
                assert_eq!(out.len(), 6, "timestamp, x, vel[0..2], ok");
                assert_eq!(out[0], (100_000 + data * 1000) as f64);
                assert_eq!(out[1], 1.5 * data as f64);
                assert_eq!(out[3], (data * 10 + 1) as f64, "vel[1]");
                assert_eq!(out[5], data as f64, "ok");
                data += 1;
            }
        }
        assert_eq!(data, 3);
    }

    #[test]
    fn char_and_padding_fields_become_no_signals() {
        let mut v = header(0);
        v.extend(format_record(
            "s:char[8] name;uint64_t timestamp;uint16_t a;uint16_t b;",
        ));
        v.extend(add_record(0, 1, "s"));
        v.extend(data_record(1, &[0u8; 20]));
        let mut r = reader(v);
        while let Some(rec) = r.next_record().expect("read") {
            if matches!(rec, UlogRecord::Data { .. }) {
                let topic = r.topics.get(&1).expect("topic");
                let names: Vec<&str> = topic.fields.iter().map(|f| f.name.as_str()).collect();
                assert_eq!(names, ["timestamp", "a", "b"]);
                assert_eq!(topic.payload_len, 20);
                return;
            }
        }
        panic!("no data record");
    }

    #[test]
    fn embedded_nested_types_are_sized_with_their_padding() {
        let mut v = header(0);
        // `inner` logs 4 bytes of content but its struct is 8 bytes padded; the
        // embedded copy still occupies all 8 (its padding is interior here).
        v.extend(format_record("inner:uint32_t a;uint8_t[4] _padding0;"));
        v.extend(format_record("outer:uint64_t timestamp;inner sub;uint8_t[2] _padding0;"));
        v.extend(add_record(0, 2, "outer"));
        v.extend(data_record(2, &[0u8; 16]));
        let mut r = reader(v);
        while let Some(rec) = r.next_record().expect("read") {
            if matches!(rec, UlogRecord::Data { .. }) {
                let topic = r.topics.get(&2).expect("topic");
                assert_eq!(topic.payload_len, 16, "8 + 8 (inner incl. padding), no tail pad");
                let names: Vec<&str> = topic.fields.iter().map(|f| f.name.as_str()).collect();
                assert_eq!(names, ["timestamp"], "nested structs are not expanded");
                return;
            }
        }
        panic!("no data record");
    }
}
