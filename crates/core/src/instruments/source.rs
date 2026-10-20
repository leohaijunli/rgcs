//! Instrument source abstraction (W1).
//!
//! An [`InstrumentSource`] turns an inbound MAVLink frame into a typed
//! [`InstrumentSample`]. Sources are stateless decoders; the same trait will
//! back the synthetic generator and, later, a ULog replay.
//!
//! Read-only guarantee (ADR-020): sources never write MAVLink, parameters or
//! DroneCAN configuration.

use crate::mavlink::MessageEnvelope;

use super::decode::{decode_gnss, decode_mag, InstrumentError};
use super::sample::{GnssSample, InstrumentKind, InstrumentSample, MagInstance, MagSample};

/// A source of typed instrument samples decoded from MAVLink frames.
pub trait InstrumentSource: Send + Sync {
    /// Human-readable name for logs and the manifest.
    fn name(&self) -> &str;

    /// Which instrument this source produces.
    fn kind(&self) -> InstrumentKind;

    /// Decode one frame. `Ok(None)` means the frame is not for this source;
    /// `Err` means the frame is ours but malformed.
    fn decode(&self, env: &MessageEnvelope) -> Result<Option<InstrumentSample>, InstrumentError>;
}

/// Decodes a magnetometer triple (default: PX4 primary `HIGHRES_IMU`).
#[derive(Debug, Clone, Copy)]
pub struct MagnetometerSource {
    instance: MagInstance,
}

impl MagnetometerSource {
    /// Decode the given MAVLink mag instance.
    pub const fn new(instance: MagInstance) -> Self {
        Self { instance }
    }

    /// Decode the PX4 primary magnetometer (`HIGHRES_IMU`, sensor 1).
    pub const fn primary() -> Self {
        Self::new(MagInstance::primary())
    }

    /// The instance this source reads.
    pub const fn instance(&self) -> MagInstance {
        self.instance
    }
}

impl InstrumentSource for MagnetometerSource {
    fn name(&self) -> &str {
        "magnetometer"
    }

    fn kind(&self) -> InstrumentKind {
        InstrumentKind::Magnetometer
    }

    fn decode(&self, env: &MessageEnvelope) -> Result<Option<InstrumentSample>, InstrumentError> {
        Ok(decode_mag(env, self.instance)?.map(InstrumentSample::Mag))
    }
}

/// Decodes `GPS_RAW_INT` (fix, position, RTK status, time).
#[derive(Debug, Clone, Copy, Default)]
pub struct GnssSource;

impl GnssSource {
    /// A new GNSS source.
    pub const fn new() -> Self {
        Self
    }
}

impl InstrumentSource for GnssSource {
    fn name(&self) -> &str {
        "gnss"
    }

    fn kind(&self) -> InstrumentKind {
        InstrumentKind::Gnss
    }

    fn decode(&self, env: &MessageEnvelope) -> Result<Option<InstrumentSample>, InstrumentError> {
        Ok(decode_gnss(env)?.map(InstrumentSample::Gnss))
    }
}

/// Convenience: the default survey sources (primary mag + GNSS).
pub fn default_sources() -> Vec<Box<dyn InstrumentSource>> {
    vec![
        Box::new(MagnetometerSource::primary()),
        Box::new(GnssSource::new()),
    ]
}

/// Helper for tests/UI: assert a sample is GNSS and return it.
pub fn as_gnss(sample: &InstrumentSample) -> Option<&GnssSample> {
    match sample {
        InstrumentSample::Gnss(g) => Some(g),
        _ => None,
    }
}

/// Helper for tests/UI: assert a sample is a magnetometer reading and return it.
pub fn as_mag(sample: &InstrumentSample) -> Option<&MagSample> {
    match sample {
        InstrumentSample::Mag(m) => Some(m),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::MavHeader;
    use crate::signals::HIGHRES_IMU_ID;
    use mavlink::common::{GpsFixType, MavMessage, GPS_RAW_INT_DATA, HIGHRES_IMU_DATA};
    use std::time::Instant;

    fn envelope(message: MavMessage) -> MessageEnvelope {
        MessageEnvelope {
            header: MavHeader {
                system_id: 1,
                component_id: 1,
                sequence: 0,
            },
            message,
            received_at: Instant::now(),
        }
    }

    #[test]
    fn magnetometer_source_decodes_primary() {
        let src = MagnetometerSource::primary();
        assert_eq!(src.name(), "magnetometer");
        assert_eq!(src.kind(), InstrumentKind::Magnetometer);
        assert_eq!(src.instance().message_id, HIGHRES_IMU_ID);

        let env = envelope(MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
            xmag: 0.3,
            ymag: 0.4,
            zmag: 0.0,
            ..Default::default()
        }));
        let sample = src.decode(&env).unwrap().expect("mag");
        assert!(as_mag(&sample).is_some());
        assert!(as_gnss(&sample).is_none());
    }

    #[test]
    fn gnss_source_decodes_fix() {
        let src = GnssSource::new();
        assert_eq!(src.kind(), InstrumentKind::Gnss);
        let env = envelope(MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA {
            fix_type: GpsFixType::GPS_FIX_TYPE_RTK_FLOAT,
            satellites_visible: 18,
            ..Default::default()
        }));
        let sample = src.decode(&env).unwrap().expect("gnss");
        assert_eq!(as_gnss(&sample).unwrap().satellites_visible, 18);
    }

    #[test]
    fn default_sources_cover_both_kinds() {
        let kinds: Vec<_> = default_sources().iter().map(|s| s.kind()).collect();
        assert!(kinds.contains(&InstrumentKind::Magnetometer));
        assert!(kinds.contains(&InstrumentKind::Gnss));
    }
}
