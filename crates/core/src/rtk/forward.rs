//! Fragment RTCM3 frames into MAVLink `GPS_RTCM_DATA` payloads (Phase 2,
//! `docs/DEVELOPMENT_PLAN.md` §8).
//!
//! A `GPS_RTCM_DATA` packet carries at most 180 bytes, so a longer RTCM frame
//! is split into up to four numbered fragments. The FC reassembles them and
//! only hands the RTCM to the GPS once the whole message is present.
//!
//! `flags` (MAVLink spec):
//!
//! ```text
//! bit 0     : 1 = the message is fragmented
//! bits 1..2 : fragment id (0..3)
//! bits 3..7 : sequence id, incremented per RTCM message so the FC can tell
//!             fragments of different messages apart
//! ```

use ::mavlink::common::GPS_RTCM_DATA_DATA;

/// Bytes carried by one `GPS_RTCM_DATA` packet.
pub const FRAGMENT_LEN: usize = 180;
/// Maximum fragments per RTCM message (2-bit fragment id).
pub const MAX_FRAGMENTS: usize = 4;
/// `flags` bit 0: the message is fragmented.
const FLAG_FRAGMENTED: u8 = 0x01;
const FRAGMENT_ID_SHIFT: u8 = 1;
const FRAGMENT_ID_MASK: u8 = 0x03;
const SEQUENCE_SHIFT: u8 = 3;
const SEQUENCE_MASK: u8 = 0x1F;

/// Reasons an RTCM frame cannot be forwarded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtcmForwardError {
    /// The frame is longer than [`MAX_FRAGMENTS`] packets can carry.
    #[error(
        "RTCM frame of {len} bytes needs {fragments} fragments; the MAVLink maximum is {MAX_FRAGMENTS}"
    )]
    TooLong { len: usize, fragments: usize },
}

/// One `GPS_RTCM_DATA` payload plus its `flags` byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcmFragment {
    pub flags: u8,
    pub data: Vec<u8>,
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

/// Split an RTCM frame into `GPS_RTCM_DATA` packets.
///
/// A frame that fits in one packet is sent unfragmented (`flags = 0`), per the
/// MAVLink spec. `sequence_id` is the caller's per-message counter; only its
/// low 5 bits are transmitted.
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
}
