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
//! The timestamp is big-endian, as QGC (`qToBigEndian`), Mission Planner and
//! pymavlink (`>Q`) write and read it. Framing, CRC checks and resync past
//! damaged bytes come from [`mavlink_tlog`].

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use mavlink_tlog::{TlogSlice, TlogWriter};

use super::RecorderError;

/// Bytes of the timestamp that prefixes every record.
pub const TIMESTAMP_LEN: usize = mavlink_tlog::TIMESTAMP_LEN;

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
    TlogWriter::new(w).write_frame(utc_us, bytes)
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
    TlogSlice::new(data)
        .map(|r| TlogRecord {
            utc_us: r.timestamp_us,
            bytes: r.frame.as_bytes().to_vec(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mavlink_tlog::{encode, Header, Version};

    /// A valid frame with a real CRC.
    fn frame(version: Version, msg_id: u32, payload: &[u8]) -> Vec<u8> {
        let header = Header {
            seq: 0,
            sys_id: 1,
            comp_id: 1,
            msg_id,
        };
        let mut out = [0u8; 280];
        let n = encode(version, header, payload, None, &mut out).expect("encode");
        out[..n].to_vec()
    }

    fn v2_frame(payload_len: u8) -> Vec<u8> {
        // PARAM_VALUE (22) with a non-zero tail so nothing is trimmed.
        frame(Version::V2, 22, &vec![7u8; payload_len as usize])
    }

    fn v1_frame() -> Vec<u8> {
        frame(Version::V1, 0, &[1u8; 9]) // HEARTBEAT
    }

    #[test]
    fn round_trips_frames_byte_for_byte() {
        let frames = [v2_frame(25), v2_frame(7), v1_frame()];
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
        let frames = [v2_frame(4), v1_frame()];
        let mut buf = Vec::new();
        for (n, frame) in frames.iter().enumerate() {
            write_frame(&mut buf, n as u64, frame).expect("write");
        }
        // Simulate a crash that wrote only the first two bytes of a third frame.
        buf.extend([0u8; TIMESTAMP_LEN]);
        buf.extend([0xFD, 40]); // claims a long frame that never arrives
        let records = read_records_from(&buf);
        assert_eq!(records.len(), 2, "the complete frames survive");
        assert_eq!(&records[1].bytes, &frames[1]);
    }

    #[test]
    fn timestamp_is_big_endian_like_qgc() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 0x0102_0304_0506_0708, &v1_frame()).expect("write");
        assert_eq!(&buf[..TIMESTAMP_LEN], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn a_damaged_byte_does_not_lose_the_rest_of_the_log() {
        let mut buf = Vec::new();
        write_frame(&mut buf, 7, &v2_frame(4)).expect("write");
        buf.push(0x00); // one stray byte
        for n in 0..3 {
            write_frame(&mut buf, 8 + n, &v1_frame()).expect("write");
        }
        let records = read_records_from(&buf);
        assert_eq!(records.len(), 4, "every frame after the damage survives");
        assert_eq!(records[3].utc_us, 10);
    }

    #[test]
    fn a_frame_with_a_bad_crc_is_refused() {
        let mut bad = v1_frame();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        let mut buf = Vec::new();
        write_frame(&mut buf, 1, &bad).expect("write");
        write_frame(&mut buf, 2, &v1_frame()).expect("write");
        let records = read_records_from(&buf);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].utc_us, 2);
    }
}
