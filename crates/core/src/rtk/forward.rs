//! Fragment RTCM3 frames into MAVLink `GPS_RTCM_DATA` payloads (Phase 2,
//! `docs/ARCHITECTURE.md` (RTK/RTCM)).
//!
//! A `GPS_RTCM_DATA` packet carries at most 180 bytes. MAVLink defines groups
//! of up to four numbered fragments for receivers that reassemble a buffer.
//! PX4 v1.17 instead forwards each packet's data to the GNSS in arrival order
//! (ADR-008). [`packetize_px4`] carries a complete RTCM frame across multiple
//! groups when needed; [`fragment`] handles just one group.
//!
//! `flags` (MAVLink spec):
//!
//! ```text
//! bit 0     : 1 = the message is fragmented
//! bits 1..2 : fragment id (0..3)
//! bits 3..7 : sequence id, incremented per transport group so receivers can
//!             distinguish reassembly buffers
//! ```

use ::mavlink::common::GPS_RTCM_DATA_DATA;

use super::rtcm::{CRC_LEN, HEADER_LEN, MAX_FRAME_LEN};

/// Bytes carried by one `GPS_RTCM_DATA` packet.
pub const FRAGMENT_LEN: usize = 180;
/// Maximum fragments per transport group (2-bit fragment id).
pub const MAX_FRAGMENTS: usize = 4;
/// Maximum bytes per transport group, not the RTCM3 frame size limit.
pub const MAX_GROUP_LEN: usize = FRAGMENT_LEN * MAX_FRAGMENTS;
const MIN_FRAME_LEN: usize = HEADER_LEN + CRC_LEN;
/// `flags` bit 0: the message is fragmented.
const FLAG_FRAGMENTED: u8 = 0x01;
const FRAGMENT_ID_SHIFT: u8 = 1;
const FRAGMENT_ID_MASK: u8 = 0x03;
const SEQUENCE_SHIFT: u8 = 3;
const SEQUENCE_MASK: u8 = 0x1F;

/// Reasons an RTCM frame cannot be forwarded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtcmForwardError {
    /// One transport group is longer than [`MAX_FRAGMENTS`] packets can carry.
    #[error(
        "RTCM transport group of {len} bytes needs {fragments} fragments; the MAVLink maximum is {MAX_FRAGMENTS}"
    )]
    TooLong { len: usize, fragments: usize },
    /// A complete frame must fit the RTCM3 header, payload and CRC bounds.
    #[error("RTCM frame length {len} is outside {MIN_FRAME_LEN}..={MAX_FRAME_LEN} bytes")]
    InvalidFrameLength { len: usize },
}

/// One `GPS_RTCM_DATA` payload plus its `flags` byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcmFragment {
    pub flags: u8,
    pub data: Vec<u8>,
}

/// Ordered packets for one complete RTCM frame on the PX4 byte-stream path.
/// All packets must be sent before beginning the next frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcmPacketBatch {
    pub packets: Vec<RtcmFragment>,
    /// Next 5-bit transport-group sequence, including any unfragmented group.
    pub next_sequence_id: u8,
}

impl RtcmFragment {
    /// Build the MAVLink message payload (zero-padding `data` to 180 bytes).
    pub fn to_message(&self) -> GPS_RTCM_DATA_DATA {
        let mut data = [0u8; FRAGMENT_LEN];
        let len = self.data.len().min(FRAGMENT_LEN);
        data[..len].copy_from_slice(&self.data[..len]);
        GPS_RTCM_DATA_DATA {
            flags: self.flags,
            len: len as u8,
            data,
        }
    }
}

/// Split one transport group into `GPS_RTCM_DATA` packets (at most 720 bytes).
///
/// A frame that fits in one packet is sent unfragmented (`flags = 0`), per the
/// MAVLink spec. `sequence_id` is the caller's per-group counter; only its low
/// 5 bits are transmitted. Exact 360/540-byte groups omit zero-length terminators
/// under the PX4 policy in ADR-008.
pub fn fragment(frame: &[u8], sequence_id: u8) -> Result<Vec<RtcmFragment>, RtcmForwardError> {
    if frame.len() <= FRAGMENT_LEN {
        return Ok(vec![RtcmFragment {
            flags: 0,
            data: frame.to_vec(),
        }]);
    }
    let count = frame.len().div_ceil(FRAGMENT_LEN);
    if count > MAX_FRAGMENTS {
        return Err(RtcmForwardError::TooLong {
            len: frame.len(),
            fragments: count,
        });
    }
    let sequence = (sequence_id & SEQUENCE_MASK) << SEQUENCE_SHIFT;
    Ok((0..count)
        .map(|index| {
            let start = index * FRAGMENT_LEN;
            let end = (start + FRAGMENT_LEN).min(frame.len());
            RtcmFragment {
                flags: FLAG_FRAGMENTED
                    | ((index as u8) & FRAGMENT_ID_MASK) << FRAGMENT_ID_SHIFT
                    | sequence,
                data: frame[start..end].to_vec(),
            }
        })
        .collect())
}

/// Packetize a complete, already CRC-validated RTCM frame for PX4 v1.17's
/// ordered-byte forwarding path. Content validation belongs to [`super::rtcm`];
/// this function checks frame length and splits at transport-group boundaries.
/// It is not a claim that other autopilots accept partial RTCM frames per group.
pub fn packetize_px4(frame: &[u8], sequence_id: u8) -> Result<RtcmPacketBatch, RtcmForwardError> {
    if !(MIN_FRAME_LEN..=MAX_FRAME_LEN).contains(&frame.len()) {
        return Err(RtcmForwardError::InvalidFrameLength { len: frame.len() });
    }
    let mut packets = Vec::with_capacity(frame.len().div_ceil(FRAGMENT_LEN));
    let mut next_sequence_id = sequence_id & SEQUENCE_MASK;
    for group in frame.chunks(MAX_GROUP_LEN) {
        packets.extend(fragment(group, next_sequence_id)?);
        next_sequence_id = next_sequence_id.wrapping_add(1) & SEQUENCE_MASK;
    }
    Ok(RtcmPacketBatch {
        packets,
        next_sequence_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn short_frame_is_sent_unfragmented() {
        let frame = payload(100);
        let fragments = fragment(&frame, 0).expect("fragment");
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].flags, 0, "a single packet is not fragmented");
        assert_eq!(fragments[0].data, frame);

        let message = fragments[0].to_message();
        assert_eq!(message.flags, 0);
        assert_eq!(message.len, 100);
        assert_eq!(&message.data[..100], &frame[..]);
        assert!(message.data[100..].iter().all(|&b| b == 0), "zero padded");
    }

    #[test]
    fn exactly_one_packet_stays_unfragmented() {
        let fragments = fragment(&payload(FRAGMENT_LEN), 7).expect("fragment");
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].flags, 0);
        assert_eq!(fragments[0].data.len(), FRAGMENT_LEN);
    }

    #[test]
    fn one_byte_over_splits_into_two() {
        let frame = payload(FRAGMENT_LEN + 1);
        let fragments = fragment(&frame, 3).expect("fragment");
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].data.len(), FRAGMENT_LEN);
        assert_eq!(fragments[1].data.len(), 1);
        for (i, f) in fragments.iter().enumerate() {
            assert_eq!(f.flags & FLAG_FRAGMENTED, FLAG_FRAGMENTED);
            assert_eq!((f.flags >> FRAGMENT_ID_SHIFT) & FRAGMENT_ID_MASK, i as u8);
        }
    }

    #[test]
    fn sequence_id_rides_bits_three_to_seven() {
        let fragments = fragment(&payload(FRAGMENT_LEN + 1), 0x25).expect("fragment");
        // 0x25 & 0x1F == 0x05
        assert_eq!(
            fragments[0].flags,
            FLAG_FRAGMENTED | (0x05 << SEQUENCE_SHIFT)
        );
        assert_eq!(fragments[0].flags >> SEQUENCE_SHIFT, 0x05);
    }

    #[test]
    fn four_fragments_is_the_limit() {
        let fragments = fragment(&payload(MAX_FRAGMENTS * FRAGMENT_LEN), 0).expect("fragment");
        assert_eq!(fragments.len(), MAX_FRAGMENTS);
        assert_eq!(
            (fragments[3].flags >> FRAGMENT_ID_SHIFT) & FRAGMENT_ID_MASK,
            3
        );
    }

    #[test]
    fn too_long_is_rejected() {
        let err = fragment(&payload(MAX_FRAGMENTS * FRAGMENT_LEN + 1), 0).unwrap_err();
        assert_eq!(
            err,
            RtcmForwardError::TooLong {
                len: MAX_FRAGMENTS * FRAGMENT_LEN + 1,
                fragments: MAX_FRAGMENTS + 1,
            }
        );
    }

    #[test]
    fn fragments_reassemble_the_original_frame() {
        let frame = payload(MAX_FRAGMENTS * FRAGMENT_LEN);
        let fragments = fragment(&frame, 1).expect("fragment");
        let rebuilt: Vec<u8> = fragments.iter().flat_map(|f| f.data.clone()).collect();
        assert_eq!(rebuilt, frame);
    }

    fn rtcm_frame(total_len: usize) -> Vec<u8> {
        super::super::rtcm::encode_frame(&vec![0x55; total_len - MIN_FRAME_LEN])
            .expect("valid RTCM fixture")
    }

    #[test]
    fn px4_packetization_covers_rtcm_size_boundaries() {
        for (len, lengths, groups) in [
            (6, vec![6], 1),
            (180, vec![180], 1),
            (181, vec![180, 1], 1),
            (360, vec![180, 180], 1),
            (540, vec![180, 180, 180], 1),
            (720, vec![180, 180, 180, 180], 1),
            (721, vec![180, 180, 180, 180, 1], 2),
            (900, vec![180, 180, 180, 180, 180], 2),
            (901, vec![180, 180, 180, 180, 180, 1], 2),
            (1029, vec![180, 180, 180, 180, 180, 129], 2),
        ] {
            let frame = rtcm_frame(len);
            let batch = packetize_px4(&frame, 5).expect("packetize full RTCM frame");
            assert_eq!(
                batch
                    .packets
                    .iter()
                    .map(|packet| packet.data.len())
                    .collect::<Vec<_>>(),
                lengths,
                "{len}-byte frame"
            );
            assert_eq!(batch.next_sequence_id, 5 + groups);
            let rebuilt = batch
                .packets
                .iter()
                .flat_map(|packet| {
                    let message = packet.to_message();
                    assert!(usize::from(message.len) <= FRAGMENT_LEN);
                    assert!(message.data[usize::from(message.len)..]
                        .iter()
                        .all(|byte| *byte == 0));
                    message.data[..usize::from(message.len)].to_vec()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                rebuilt, frame,
                "no dropped, duplicated or padded bytes at {len}"
            );
            assert_eq!(
                super::super::rtcm::parse_frame(&rebuilt)
                    .expect("CRC preserved")
                    .1,
                len
            );
        }
    }

    #[test]
    fn px4_packetization_wraps_between_groups_and_reserves_the_next_sequence() {
        let batch = packetize_px4(&rtcm_frame(MAX_FRAME_LEN), 31).expect("maximum frame");
        // First group: sequence 31, fragment IDs 0..3. Tail: sequence 0, IDs 0..1.
        assert_eq!(
            batch
                .packets
                .iter()
                .map(|packet| packet.flags)
                .collect::<Vec<_>>(),
            vec![249, 251, 253, 255, 1, 3]
        );
        assert_eq!(batch.next_sequence_id, 1);
        assert_eq!(
            packetize_px4(&rtcm_frame(MAX_FRAME_LEN), u8::MAX).expect("mask sequence"),
            batch
        );
        let next =
            packetize_px4(&rtcm_frame(181), batch.next_sequence_id).expect("following frame");
        assert_eq!(
            next.packets
                .iter()
                .map(|packet| packet.flags)
                .collect::<Vec<_>>(),
            vec![9, 11]
        );
        assert_eq!(next.next_sequence_id, 2);
    }

    #[test]
    fn px4_packetization_rejects_invalid_frame_lengths() {
        for len in [0, 1, MIN_FRAME_LEN - 1, MAX_FRAME_LEN + 1, 2048] {
            assert_eq!(
                packetize_px4(&vec![0; len], 0),
                Err(RtcmForwardError::InvalidFrameLength { len })
            );
        }
    }
}
