//! RTK base-station support: RTCM3 framing and forwarding to the flight
//! controller (Phase 2, `docs/ARCHITECTURE.md` (RTK/RTCM)).
//!
//! Data flow: base receiver → [`rtcm`] parses RTCM3 frames and validates their
//! CRC → [`forward`] fragments them into MAVLink `GPS_RTCM_DATA`.
//!
//! [`base`] owns independent receiver sessions and F9P Survey-In configuration;
//! [`service`] forwards fresh corrections to one vehicle transport and reports
//! statistics. [`source`] also supports preconfigured serial receivers.

pub mod base;
pub mod forward;
pub mod multi;
pub mod rtcm;
mod ubx;

pub use forward::{fragment, packetize_px4, RtcmForwardError, RtcmFragment, RtcmPacketBatch};
pub use multi::{ForeignInjector, ForeignState, FOREIGN_WINDOW};
pub use rtcm::{encode_frame, parse_frame, RtcmError, RtcmFrame, RtcmFramer};
pub mod service;
pub mod source;
