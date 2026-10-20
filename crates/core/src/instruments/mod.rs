//! Walk-magnetometer instrument access (plan §5B W1, ADR-020).
//!
//! The walking rig carries a PX4 used **as an instrument hub only** (it does
//! not fly): DroneCAN magnetometers and a Holybro H-RTK ZED-F9P publish on
//! MAVLink. This module is therefore a **thin adapter over the existing
//! decoders** — no NMEA parsing, no serial/TCP/Bluetooth transport, no vendor
//! frames, and it never writes MAVLink/DroneCAN/parameter configuration.
//!
//! - [`sample`] — typed [`InstrumentSample`] (mag in nT, GNSS with a
//!   datum-tagged [`height::Height`]).
//! - [`source`] — [`InstrumentSource`] and the magnetometer/GNSS decoders built
//!   on [`crate::signals`] and [`crate::telemetry`].
//! - [`monitor`] — per-instrument rate / staleness / clock-jump detection,
//!   emitting [`StreamEvent`]s that map onto the alert center (A3).
//! - [`sim`] — a deterministic MAVLink generator so CI needs no hardware.
//! - [`live`] — the tokio runner mirroring [`crate::signals::tap`].

pub mod decode;
pub mod live;
pub mod monitor;
pub mod sample;
pub mod sim;
pub mod source;

pub use decode::InstrumentError;
pub use live::{route, run_instruments, subscribe, InstrumentEvent, INSTRUMENT_CHANNEL_CAP};
pub use monitor::{MonitorConfig, StreamEvent, StreamHealth, StreamMonitor};
pub use sample::{GnssSample, InstrumentKind, InstrumentSample, MagInstance, MagSample};
pub use sim::{SimConfig, Simulator};
pub use source::{default_sources, GnssSource, InstrumentSource, MagnetometerSource};
