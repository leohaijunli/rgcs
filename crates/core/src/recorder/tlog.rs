//! QGC-aligned telemetry log (`tlog`).
//!
//! Each record is an 8-byte timestamp followed by one raw MAVLink packet
//! (including its start sign):
//!
//! ```text
//! ┌──────────────────┬────────────────────────────────────────────┐
//! │ u64 µs since epoch │ MAVLink frame (STX … checksum [/signature]) │
//! └──────────────────┴────────────────────────────────────────────┘
//! ```
//!
//! QGC writes the timestamp by `memcpy`-ing a `uint64_t`, so it is in host byte
//! order — little-endian on every platform MagGCS and QGC target
//! (`*verify on a big-endian host*`, plan §9 item 6). The packet is
//! self-delimiting, so [`read_records`] re-frames it without a length prefix,
//! which is what makes a QGC-written file readable here and vice versa.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use super::RecorderError;

/// Bytes of the timestamp that prefixes every record.
pub const TIMESTAMP_LEN: usize = 8;

/// MAVLink start-of-frame markers.
const STX_V1: u8 = 0xFE;
const STX_V2: u8 = 0xFD;
/// Header bytes after the STX (v1: len, seq, sys, comp, msgid).
const V1_HEADER: usize = 5;
/// Header bytes after the STX (v2: len, incompat, compat, seq, sys, comp, msgid[3]).
const V2_HEADER: usize = 9;
const CHECKSUM: usize = 2;
const SIGNATURE: usize = 13;
const IFLAG_SIGNED: u8 = 0x01;

/// One recorded frame, as read back from a tlog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlogRecord {
    /// Microseconds since the Unix epoch.
    pub utc_us: u64,
    /// Exact MAVLink frame bytes.
    pub bytes: Vec<u8>,
}

/// Append one record: the timestamp, then the frame bytes verbatim.
pub fn write_frame<W: Write>(w: &mut W, utc_us: u64, bytes: &[u8]) -> io::Result<()> {
    w.write_all(&utc_us.to_le_bytes())?;
    w.write_all(bytes)
}

/// Total length of the MAVLink frame that starts with `first` (`0` when the
/// start sign is not a MAVLink version).
fn frame_len(first: u8, payload_len: u8, incompat_flags: u8) -> Option<usize> {
    match first {
        STX_V2 => {
            let signed = incompat_flags & IFLAG_SIGNED != 0;
            Some(
                1 + V2_HEADER
                    + payload_len as usize
                    + CHECKSUM
                    + if signed { SIGNATURE } else { 0 },
            )
        }
        STX_V1 => Some(1 + V1_HEADER + payload_len as usize + CHECKSUM),
        _ => None,
    }
}

/// Read every complete record. A truncated final record (a crash mid-write) is
/// ignored rather than reported, so the file always parses up to the last
/// flush.
pub fn read_records(path: &Path) -> Result<Vec<TlogRecord>, RecorderError> {
    let data = fs::read(path).map_err(|e| RecorderError::Open {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(read_records_from(&data))
}

/// Parse records from an in-memory tlog image (used by [`read_records`] and the
/// tests).
pub fn read_records_from(data: &[u8]) -> Vec<TlogRecord> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + TIMESTAMP_LEN + 2 <= data.len() {
        let first = data[i + TIMESTAMP_LEN];
        // A v2 frame needs its incompatibility flags byte before we can size it.
        if first == STX_V2 && i + TIMESTAMP_LEN + 3 > data.len() {
            break;
        }
        let payload_len = data[i + TIMESTAMP_LEN + 1];
        let incompat_flags = if first == STX_V2 {
            data[i + TIMESTAMP_LEN + 2]
        } else {
            0
        };
        let Some(frame_len) = frame_len(first, payload_len, incompat_flags) else {
            break;
        };
        let end = i + TIMESTAMP_LEN + frame_len;
        if end > data.len() {
            break; // truncated tail
        }
        let mut ts = [0u8; TIMESTAMP_LEN];
        ts.copy_from_slice(&data[i..i + TIMESTAMP_LEN]);
        out.push(TlogRecord {
            utc_us: u64::from_le_bytes(ts),
            bytes: data[i + TIMESTAMP_LEN..end].to_vec(),
        });
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, hand-built MAVLink v2 frame (CRC contents are irrelevant to
    /// framing). Layout: STX, len, incompat, compat, seq, sys, comp, msgid[3],
    /// payload, checksum[, signature].
    fn v2_frame(payload_len: u8, signed: bool) -> Vec<u8> {
        let mut frame = vec![
            STX_V2,
            payload_len,
            if signed { 0x01 } else { 0 },
            0,
            0,
            1,
            1,
            9,
            0,
            0,
        ];
        frame.extend(std::iter::repeat_n(0u8, payload_len as usize));
        frame.extend([0, 0]); // checksum
        if signed {
            frame.extend([0u8; SIGNATURE]);
        }
        frame
    }

    /// A minimal MAVLink v1 frame.
    fn v1_frame(payload_len: u8) -> Vec<u8> {
        let mut frame = vec![STX_V1, payload_len, 0, 1, 1, 0];
        frame.extend(std::iter::repeat_n(0u8, payload_len as usize));
        frame.extend([0, 0]); // checksum
        frame
    }

    #[test]
    fn round_trips_frames_byte_for_byte() {
        let frames = [v2_frame(0, false), v2_frame(7, true), v1_frame(3)];
        let mut buf = Vec::new();
        for (n, frame) in frames.iter().enumerate() {
            write_frame(&mut buf, 1_000 + n as u64, frame).expect("write");
        }
        let records = read_records_from(&buf);
        assert_eq!(records.len(), 3);
        for (n, record) in records.iter().enumerate() {
            assert_eq!(record.utc_us, 1_000 + n as u64);
            assert_eq!(
                &record.bytes, &frames[n],
                "frame {n} must be byte-identical"
            );
        }
    }

    #[test]
    fn a_truncated_final_record_is_ignored() {
        let frames = [v2_frame(4, false), v1_frame(2)];
        let mut buf = Vec::new();
        for (n, frame) in frames.iter().enumerate() {
            write_frame(&mut buf, n as u64, frame).expect("write");
        }
        // Simulate a crash that wrote only the first two bytes of a third frame.
        buf.extend([0u8; TIMESTAMP_LEN]);
        buf.extend([STX_V2, 40]); // claims a long frame that never arrives
        let records = read_records_from(&buf);
        assert_eq!(records.len(), 2, "the complete frames survive");
        assert_eq!(&records[1].bytes, &frames[1]);
    }

    #[test]
    fn a_corrupt_prefix_stops_cleanly() {
        // A byte that is neither STX is treated as the end of the log.
        let mut buf = Vec::new();
        write_frame(&mut buf, 7, &v2_frame(0, false)).expect("write");
        buf.extend([0u8; TIMESTAMP_LEN]);
        buf.push(0x00);
        assert_eq!(read_records_from(&buf).len(), 1);
    }
}
