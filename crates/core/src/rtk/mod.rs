//! RTK base-station support: RTCM3 framing and forwarding to the flight
//! controller (Phase 2, `docs/DEVELOPMENT_PLAN.md` §8).
//!
//! Data flow: base receiver → [`rtcm`] parses RTCM3 frames and validates their
//! CRC → [`forward`] fragments them into MAVLink `GPS_RTCM_DATA`.
//!
//! Source abstraction (`RtcmSource` for serial/NTRIP/TCP) and the injection
//! service land in follow-up slices.

pub mod forward;
pub mod rtcm;

pub use forward::{fragment, RtcmForwardError, RtcmFragment};
pub use rtcm::{encode_frame, parse_frame, RtcmError, RtcmFrame, RtcmFramer};
