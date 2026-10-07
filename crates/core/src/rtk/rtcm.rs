//! RTCM3 framing: the `0xD3` preamble, the 10-bit length word, CRC-24Q
//! validation and a streaming framer that resynchronises on garbage
//! (Phase 2, `docs/DEVELOPMENT_PLAN.md` §8).
//!
//! RTCM3 frame layout:
//!
//! ```text
//! 0xD3 | 6 reserved bits (=0) + 10-bit payload length (big-endian) | payload | CRC-24Q (3 bytes)
//! ```
//!
//! The CRC covers the preamble, the length word and the payload, and is
//! transmitted most-significant byte first.

/// RTCM3 preamble byte.
pub const PREAMBLE: u8 = 0xD3;
/// Bytes before the payload (preamble + 2-byte length word).
pub const HEADER_LEN: usize = 3;
/// Trailing CRC-24Q length.
pub const CRC_LEN: usize = 3;
/// Largest payload the 10-bit length word can express.
pub const MAX_PAYLOAD_LEN: usize = 1023;
/// Reserved bits of the length word; a valid frame leaves them zero.
const RESERVED_MASK: u8 = 0xFC;
/// CRC-24Q polynomial (0x1864CFB); the implicit bit 24 is dropped.
const CRC24Q_POLY: u32 = 0x0086_4CFB;
/// CRC-24Q width mask.
const CRC24Q_MASK: u32 = 0x00FF_FFFF;

/// Reasons a byte stream or buffer is not a valid RTCM3 frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtcmError {
    /// The buffer does not start with the `0xD3` preamble.
    #[error("not an RTCM3 frame: expected preamble 0x{PREAMBLE:02X}")]
    NotPreamble,
    /// The length word's reserved bits were set, so this is a false preamble.
    #[error("RTCM3 length word has reserved bits set")]
    ReservedBitsSet,
    /// Fewer bytes than the frame needs; feed more and try again.
    #[error("RTCM3 frame is incomplete")]
    Incomplete,
    /// The trailing checksum does not match the frame contents.
    #[error("RTCM3 CRC mismatch: frame says {expected:#08X}, computed {computed:#08X}")]
    CrcMismatch { expected: u32, computed: u32 },
    /// A payload longer than the 10-bit length word allows.
    #[error("RTCM3 payload of {len} bytes exceeds the maximum of {MAX_PAYLOAD_LEN}")]
    PayloadTooLong { len: usize },
}

/// A validated RTCM3 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcmFrame {
    /// 12-bit RTCM message number (0 when the payload is too short to hold one).
    pub message_type: u16,
    /// Payload bytes between the length word and the CRC.
    pub payload: Vec<u8>,
    /// The frame's CRC-24Q, as transmitted.
    pub crc: u32,
}

impl RtcmFrame {
    /// Re-encode the frame (preamble, length, payload, CRC).
    pub fn to_bytes(&self) -> Vec<u8> {
        // `payload` came from a parsed frame or `encode_frame`, so it always
        // fits; fall back to a truncated write rather than panicking.
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len() + CRC_LEN);
        let len = self.payload.len().min(MAX_PAYLOAD_LEN);
        out.push(PREAMBLE);
        out.push(((len >> 8) as u8) & 0x03);
        out.push((len & 0xFF) as u8);
        out.extend_from_slice(&self.payload[..len]);
        out.push((self.crc >> 16) as u8);
        out.push((self.crc >> 8) as u8);
        out.push(self.crc as u8);
        out
    }
}

/// CRC-24Q over `bytes` (the RTCM3 checksum), MSB-first, init 0.
pub fn crc24q(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in bytes {
        crc ^= (byte as u32) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x0100_0000 != 0 {
                crc ^= CRC24Q_POLY;
            }
        }
        crc &= CRC24Q_MASK;
    }
    crc
}

/// 12-bit RTCM message number from a payload.
pub fn message_type(payload: &[u8]) -> u16 {
    if payload.len() < 2 {
        return 0;
    }
    u16::from_be_bytes([payload[0], payload[1]]) >> 4
}

/// Total length of the frame at the front of `bytes`, or `None` if more bytes
/// are needed. Validates the preamble and the reserved bits only.
pub fn frame_len(bytes: &[u8]) -> Result<Option<usize>, RtcmError> {
    if bytes.is_empty() || bytes[0] != PREAMBLE {
        return Err(RtcmError::NotPreamble);
    }
    if bytes.len() < HEADER_LEN {
        return Ok(None);
    }
    if bytes[1] & RESERVED_MASK != 0 {
        return Err(RtcmError::ReservedBitsSet);
    }
    let len = (((bytes[1] & 0x03) as usize) << 8) | bytes[2] as usize;
    Ok(Some(HEADER_LEN + len + CRC_LEN))
}

/// Parse and CRC-check the complete frame at the front of `bytes`, returning it
/// and the number of bytes it consumed.
pub fn parse_frame(bytes: &[u8]) -> Result<(RtcmFrame, usize), RtcmError> {
    let total = frame_len(bytes)?.ok_or(RtcmError::Incomplete)?;
    if bytes.len() < total {
        return Err(RtcmError::Incomplete);
    }
    let payload = &bytes[HEADER_LEN..HEADER_LEN + (total - HEADER_LEN - CRC_LEN)];
    let expected = ((bytes[total - 3] as u32) << 16)
        | ((bytes[total - 2] as u32) << 8)
        | bytes[total - 1] as u32;
    let computed = crc24q(&bytes[..total - CRC_LEN]);
    if expected != computed {
        return Err(RtcmError::CrcMismatch { expected, computed });
    }
    Ok((
        RtcmFrame {
            message_type: message_type(payload),
            payload: payload.to_vec(),
            crc: expected,
        },
        total,
    ))
}

/// Encode a payload as an RTCM3 frame (computing the CRC).
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, RtcmError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(RtcmError::PayloadTooLong { len: payload.len() });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + CRC_LEN);
    out.push(PREAMBLE);
    out.push(((payload.len() >> 8) as u8) & 0x03);
    out.push((payload.len() & 0xFF) as u8);
    out.extend_from_slice(payload);
    let crc = crc24q(&out);
    out.push((crc >> 16) as u8);
    out.push((crc >> 8) as u8);
    out.push(crc as u8);
    Ok(out)
}

/// Incremental RTCM3 framer: feed arbitrary byte chunks, get whole frames back.
///
/// A byte stream carries no framing, so garbage, false preambles and corrupted
/// frames are expected. On a bad header or CRC the scan resumes one byte after
/// the candidate preamble, so a real frame that starts inside a corrupt one is
/// still recovered.
#[derive(Debug, Default)]
pub struct RtcmFramer {
    buf: Vec<u8>,
}

impl RtcmFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes buffered so far that do not yet form a complete frame.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Drop any buffered bytes (e.g. after the source reconnects).
    pub fn reset(&mut self) {
        self.buf.clear();
    }

    /// Append `chunk` and return every complete, CRC-valid frame found.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<RtcmFrame> {
        self.buf.extend_from_slice(chunk);
        let mut frames = Vec::new();
        let mut start = 0usize;
        loop {
            let Some(offset) = self.buf[start..].iter().position(|&b| b == PREAMBLE) else {
                start = self.buf.len();
                break;
            };
            start += offset;
            match frame_len(&self.buf[start..]) {
                Err(_) => start += 1,
                Ok(None) => break,
                Ok(Some(_)) => match parse_frame(&self.buf[start..]) {
                    Ok((frame, consumed)) => {
                        frames.push(frame);
                        start += consumed;
                    }
                    // Need more bytes: keep the candidate and wait.
                    Err(RtcmError::Incomplete) => break,
                    // Corrupt: resume one byte after the candidate preamble.
                    Err(_) => start += 1,
                },
            }
        }
        self.buf.drain(..start);
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Payload of `MSM`-like shape: 12-bit message number then filler.
    fn payload_of(message_type: u16) -> Vec<u8> {
        let mut payload = vec![0u8; 40];
        payload[0] = (message_type >> 4) as u8;
        payload[1] = ((message_type & 0x0F) as u8) << 4;
        for (i, b) in payload.iter_mut().enumerate().skip(2) {
            *b = (i as u8).wrapping_mul(7).wrapping_add(1);
        }
        payload
    }

    #[test]
    fn crc24q_matches_the_standard_check_value() {
        // CRC RevEng "CRC-24/OPENPGP"-style width,poly=0x1864CFB,init=0,
        // refin/refout=false, xorout=0 → check("123456789") = 0xCDE703.
        assert_eq!(crc24q(b"123456789"), 0xCD_E703);
        assert_eq!(crc24q(&[]), 0);
    }

    #[test]
    fn encode_then_parse_round_trips() {
        let payload = payload_of(1074); // MSM4 GPS
        let bytes = encode_frame(&payload).expect("encode");
        assert_eq!(bytes[0], PREAMBLE);
        assert_eq!(frame_len(&bytes).expect("len"), Some(bytes.len()));
        let (frame, consumed) = parse_frame(&bytes).expect("parse");
        assert_eq!(consumed, bytes.len());
        assert_eq!(frame.message_type, 1074);
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.to_bytes(), bytes);
    }

    #[test]
    fn parses_an_independently_computed_frame() {
        // MT1005-shaped frame generated outside this crate (a straight
        // CRC-24Q implementation; the same implementation reproduces the
        // standard check value above). Locks in the byte layout.
        const GOLDEN: [u8; 25] = [
            0xD3, 0x00, 0x13, 0x3E, 0xD0, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x45, 0x13, 0x9E,
        ];
        assert_eq!(frame_len(&GOLDEN).expect("len"), Some(GOLDEN.len()));
        let (frame, consumed) = parse_frame(&GOLDEN).expect("parse");
        assert_eq!(consumed, GOLDEN.len());
        assert_eq!(frame.crc, 0x45_139E);
        assert_eq!(frame.message_type, 1005);
        assert_eq!(frame.payload.len(), 19);
        assert_eq!(frame.payload[0], 0x3E);
        assert_eq!(frame.to_bytes(), GOLDEN.to_vec());
        // A framer fed this frame byte-by-byte agrees.
        let mut framer = RtcmFramer::new();
        let mut frames = Vec::new();
        for b in &GOLDEN {
            frames.extend(framer.push(&[*b]));
        }
        assert_eq!(frames, vec![frame]);
    }

    #[test]
    fn message_type_reads_the_12_bit_field() {
        assert_eq!(message_type(&[0x43, 0x30]), 1075);
        assert_eq!(message_type(&[0x00, 0x00]), 0);
        assert_eq!(message_type(&[0xFF, 0xF0]), 4095);
        assert_eq!(message_type(&[0x01]), 0);
    }

    #[test]
    fn rejects_a_bad_preamble() {
        assert_eq!(frame_len(&[0x00]).unwrap_err(), RtcmError::NotPreamble);
        assert_eq!(frame_len(&[]).unwrap_err(), RtcmError::NotPreamble);
    }

    #[test]
    fn rejects_reserved_bits_in_the_length_word() {
        // 0x04 in byte 1 sets a reserved bit.
        assert_eq!(
            frame_len(&[PREAMBLE, 0x04, 0x00]).unwrap_err(),
            RtcmError::ReservedBitsSet
        );
    }

    #[test]
    fn short_header_and_short_body_are_incomplete() {
        assert_eq!(frame_len(&[PREAMBLE]).expect("len"), None);
        assert_eq!(frame_len(&[PREAMBLE, 0x00]).expect("len"), None);
        let bytes = encode_frame(&payload_of(1005)).expect("encode");
        let err = parse_frame(&bytes[..bytes.len() - 1]).unwrap_err();
        assert_eq!(err, RtcmError::Incomplete);
    }

    #[test]
    fn detects_a_corrupted_payload_and_crc() {
        let mut bytes = encode_frame(&payload_of(1005)).expect("encode");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(
            parse_frame(&bytes).unwrap_err(),
            RtcmError::CrcMismatch { .. }
        ));

        let mut flipped = encode_frame(&payload_of(1005)).expect("encode");
        flipped[HEADER_LEN] ^= 0x01;
        assert!(matches!(
            parse_frame(&flipped).unwrap_err(),
            RtcmError::CrcMismatch { .. }
        ));
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let too_long = vec![0u8; MAX_PAYLOAD_LEN + 1];
        assert_eq!(
            encode_frame(&too_long).unwrap_err(),
            RtcmError::PayloadTooLong {
                len: MAX_PAYLOAD_LEN + 1
            }
        );
    }

    #[test]
    fn framer_splits_coalesced_frames() {
        let mut stream = encode_frame(&payload_of(1005)).expect("a");
        stream.extend(encode_frame(&payload_of(1074)).expect("b"));
        let mut framer = RtcmFramer::new();
        let frames = framer.push(&stream);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].message_type, 1005);
        assert_eq!(frames[1].message_type, 1074);
        assert_eq!(framer.buffered(), 0);
    }

    #[test]
    fn framer_reassembles_a_byte_by_byte_stream() {
        let bytes = encode_frame(&payload_of(1230)).expect("encode");
        let mut framer = RtcmFramer::new();
        let mut frames = Vec::new();
        for b in &bytes {
            frames.extend(framer.push(&[*b]));
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, 1230);
        assert_eq!(framer.buffered(), 0);
    }

    #[test]
    fn framer_resyncs_past_garbage_and_false_preambles() {
        // Junk, then a false preamble whose reserved bits are set.
        let mut stream = vec![0x00, 0x11, PREAMBLE, 0x04, 0x00];
        stream.extend(encode_frame(&payload_of(1005)).expect("good"));
        let mut framer = RtcmFramer::new();
        let frames = framer.push(&stream);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, 1005);
        assert_eq!(framer.buffered(), 0);
    }

    #[test]
    fn framer_recovers_once_a_false_frame_is_bounded() {
        // A false preamble with a plausible length makes the framer wait for
        // that many bytes. Once they arrive the CRC fails and the real frame
        // that started inside it is still recovered.
        let good = encode_frame(&payload_of(1074)).expect("good");
        let mut stream = vec![0x00, PREAMBLE, 0x00, 0x40]; // claims 3 + 64 + 3 bytes
        stream.extend_from_slice(&good);
        stream.resize(4 + 70, 0xAA);
        let mut framer = RtcmFramer::new();
        let frames = framer.push(&stream);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].message_type, 1074);
    }

    #[test]
    fn framer_skips_a_corrupt_frame_and_keeps_the_next() {
        let mut bad = encode_frame(&payload_of(1005)).expect("bad");
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        let mut stream = bad;
        stream.extend(encode_frame(&payload_of(1074)).expect("good"));
        let mut framer = RtcmFramer::new();
        let frames = framer.push(&stream);
        assert_eq!(frames.len(), 1, "only the intact frame survives");
        assert_eq!(frames[0].message_type, 1074);
        assert_eq!(framer.buffered(), 0);
    }

    #[test]
    fn framer_keeps_a_partial_frame_buffered() {
        let bytes = encode_frame(&payload_of(1005)).expect("encode");
        let (head, tail) = bytes.split_at(5);
        let mut framer = RtcmFramer::new();
        assert!(framer.push(head).is_empty());
        assert_eq!(framer.buffered(), 5);
        let frames = framer.push(tail);
        assert_eq!(frames.len(), 1);
        assert_eq!(framer.buffered(), 0);
    }

    #[test]
    fn framer_reset_drops_buffered_bytes() {
        let bytes = encode_frame(&payload_of(1005)).expect("encode");
        let mut framer = RtcmFramer::new();
        framer.push(&bytes[..4]);
        assert_eq!(framer.buffered(), 4);
        framer.reset();
        assert_eq!(framer.buffered(), 0);
    }
}
