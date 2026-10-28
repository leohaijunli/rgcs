//! MAVLink → typed instrument sample decoding (W1).
//!
//! Magnetometer fields are read with the generic [`crate::signals::extract`]
//! walker and normalized to nanotesla with
//! [`crate::signals::normalize_mag_fields`]; GNSS is read with the existing
//! [`crate::telemetry::GpsRawInt`] normalizer. No per-vendor frame parsing.

use mavlink::Message;
use thiserror::Error;

use crate::mavlink::{MavMessage, MessageEnvelope};
use crate::signals::extract::{extract, ExtractError, Extraction};
use crate::signals::{
    normalize_mag_fields, HIGHRES_IMU_ID, RAW_IMU_ID, SCALED_IMU2_ID, SCALED_IMU3_ID, SCALED_IMU_ID,
};
use crate::telemetry::GpsRawInt;

use super::sample::{GnssSample, InstrumentKind, MagInstance, MagSample};

/// MAVLink message ids carrying a magnetometer triple whose wire unit MagGCS
/// knows how to convert (`HIGHRES_IMU` gauss, all raw/scaled variants
/// milligauss).
pub const MAG_MESSAGE_IDS: [u32; 5] = [
    HIGHRES_IMU_ID,
    RAW_IMU_ID,
    SCALED_IMU_ID,
    SCALED_IMU2_ID,
    SCALED_IMU3_ID,
];

/// Whether a message id carries a magnetometer triple with a known unit.
pub fn is_mag_message(message_id: u32) -> bool {
    MAG_MESSAGE_IDS.contains(&message_id)
}

/// Failures while turning a MAVLink frame into an instrument sample.
#[derive(Debug, Error)]
pub enum InstrumentError {
    /// A magnetometer frame lacked one of `xmag`/`ymag`/`zmag`.
    #[error("magnetometer frame {message_id} has no complete xmag/ymag/zmag triple")]
    IncompleteMagField { message_id: u32 },
    /// A decoded value was NaN or infinite.
    #[error("non-finite {field} in message {message_id}")]
    NonFiniteField {
        message_id: u32,
        field: &'static str,
    },
    /// A frame was routed to the wrong decoder.
    #[error("expected a {expected:?} frame, got message {message_id}")]
    UnexpectedKind {
        expected: InstrumentKind,
        message_id: u32,
    },
    /// The generic field extractor failed.
    #[error("field extraction failed: {0}")]
    Extract(#[from] ExtractError),
    /// The telemetry GNSS normalizer rejected the frame.
    #[error("GNSS decode failed: {0}")]
    Gnss(#[from] crate::telemetry::TelemetryParseError),
}

/// Decode a magnetometer frame; `Ok(None)` when the frame is not the requested
/// mag message.
pub fn decode_mag(
    env: &MessageEnvelope,
    instance: MagInstance,
) -> Result<Option<MagSample>, InstrumentError> {
    let message_id = env.message.message_id();
    if message_id != instance.message_id || !is_mag_message(message_id) {
        return Ok(None);
    }
    let extraction = extract(&env.message)?;
    let value = |name: &str| {
        extraction
            .fields
            .iter()
            .find(|f| f.field == name)
            .map(|f| f.value)
    };
    let (Some(x), Some(y), Some(z)) = (value("xmag"), value("ymag"), value("zmag")) else {
        return Err(InstrumentError::IncompleteMagField { message_id });
    };
    let x_nt = normalize_mag_fields(message_id, "xmag", x);
    let y_nt = normalize_mag_fields(message_id, "ymag", y);
    let z_nt = normalize_mag_fields(message_id, "zmag", z);
    for (field, v) in [("xmag", x_nt), ("ymag", y_nt), ("zmag", z_nt)] {
        if !v.is_finite() {
            return Err(InstrumentError::NonFiniteField { message_id, field });
        }
    }
    Ok(Some(MagSample {
        t_ms: fc_time_ms(&extraction).unwrap_or(0.0),
        instance,
        x_nt,
        y_nt,
        z_nt,
        total_nt: (x_nt * x_nt + y_nt * y_nt + z_nt * z_nt).sqrt(),
    }))
}

/// Decode a GNSS frame (`GPS_RAW_INT`); `Ok(None)` for any other message.
pub fn decode_gnss(env: &MessageEnvelope) -> Result<Option<GnssSample>, InstrumentError> {
    let MavMessage::GPS_RAW_INT(raw) = &env.message else {
        return Ok(None);
    };
    let gps = GpsRawInt::try_from(&env.message)?;
    Ok(Some(GnssSample {
        t_ms: raw.time_usec as f64 / 1000.0,
        fix: gps.fix_type,
        latitude_deg: gps.latitude_deg,
        longitude_deg: gps.longitude_deg,
        altitude: gps.altitude,
        hdop: gps.hdop,
        satellites_visible: gps.satellites_visible,
        velocity_m_s: gps.velocity_m_s,
        course_over_ground_deg: gps.course_over_ground_deg,
    }))
}

/// The FC sample time in milliseconds, from `time_usec` (µs) or `time_boot_ms`.
fn fc_time_ms(extraction: &Extraction) -> Option<f64> {
    for field in extraction.fields.iter() {
        match field.field.as_str() {
            "time_usec" => return Some(field.value / 1000.0),
            "time_boot_ms" => return Some(field.value),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::MavHeader;
    use crate::signals::extract::ExtractError;
    use crate::telemetry::GpsFixType;
    use mavlink::common::{
        GpsFixType as MavGpsFixType, GPS_RAW_INT_DATA, HIGHRES_IMU_DATA, RAW_IMU_DATA,
        SCALED_IMU2_DATA, SCALED_IMU3_DATA, SCALED_IMU_DATA,
    };
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

    fn highres(xmag: f32, ymag: f32, zmag: f32, time_usec: u64) -> MavMessage {
        MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
            time_usec,
            xmag,
            ymag,
            zmag,
            ..Default::default()
        })
    }

    #[test]
    fn highres_mag_is_converted_to_nanotesla() {
        let env = envelope(highres(0.3, 0.4, 0.0, 1_500_000));
        let sample = decode_mag(&env, MagInstance::primary())
            .expect("decode")
            .expect("is mag");
        // `f32` wire values lose precision at 1e5 scaling; allow 0.01 nT.
        assert!((sample.x_nt - 30_000.0).abs() < 1e-2);
        assert!((sample.y_nt - 40_000.0).abs() < 1e-2);
        assert!((sample.z_nt).abs() < 1e-2);
        assert!((sample.total_nt - 50_000.0).abs() < 1e-2);
        assert!((sample.t_ms - 1_500.0).abs() < 1e-9);
    }

    #[test]
    fn raw_imu_milligauss_is_converted() {
        let env = envelope(MavMessage::RAW_IMU(RAW_IMU_DATA {
            time_usec: 0,
            xmag: 250,
            ymag: 0,
            zmag: 0,
            ..Default::default()
        }));
        let instance = MagInstance::new(RAW_IMU_ID, 1);
        let sample = decode_mag(&env, instance).expect("decode").expect("is mag");
        assert!((sample.x_nt - 25_000.0).abs() < 1e-3);
    }

    #[test]
    fn scaled_imu_instances_decode_with_distinct_message_identity() {
        let fixtures = [
            (
                MavMessage::SCALED_IMU(SCALED_IMU_DATA {
                    xmag: 100,
                    ..Default::default()
                }),
                SCALED_IMU_ID,
                1,
                10_000.0,
            ),
            (
                MavMessage::SCALED_IMU2(SCALED_IMU2_DATA {
                    xmag: 200,
                    ..Default::default()
                }),
                SCALED_IMU2_ID,
                2,
                20_000.0,
            ),
            (
                MavMessage::SCALED_IMU3(SCALED_IMU3_DATA {
                    xmag: 300,
                    ..Default::default()
                }),
                SCALED_IMU3_ID,
                3,
                30_000.0,
            ),
        ];
        for (message, message_id, sensor, expected_x) in fixtures {
            let env = envelope(message);
            let instance = MagInstance::new(message_id, sensor);
            let sample = decode_mag(&env, instance).expect("decode").expect("is mag");
            assert_eq!(sample.instance, instance);
            assert!((sample.x_nt - expected_x).abs() < 1e-3);
        }
    }

    #[test]
    fn non_mag_frame_is_ignored() {
        let env = envelope(MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA::default()));
        assert!(decode_mag(&env, MagInstance::primary())
            .expect("decode")
            .is_none());
    }

    #[test]
    fn wrong_message_is_ignored_not_errored() {
        // A RAW_IMU frame with a HIGHRES_IMU instance is simply not ours.
        let env = envelope(MavMessage::RAW_IMU(RAW_IMU_DATA::default()));
        assert!(decode_mag(&env, MagInstance::primary())
            .expect("decode")
            .is_none());
    }

    #[test]
    fn gnss_frame_decodes_fix_and_position() {
        let env = envelope(MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA {
            time_usec: 2_000_000,
            lat: 485_560_600,
            lon: -1_233_813_600,
            alt: 20_000,
            eph: 80,
            epv: 120,
            vel: 120,
            cog: 9_000,
            fix_type: MavGpsFixType::GPS_FIX_TYPE_RTK_FIXED,
            satellites_visible: 22,
            ..Default::default()
        }));
        let sample = decode_gnss(&env).expect("decode").expect("is gnss");
        assert_eq!(sample.fix, GpsFixType::RtkFixed);
        assert!((sample.latitude_deg - 48.55606).abs() < 1e-9);
        assert!((sample.longitude_deg + 123.38136).abs() < 1e-9);
        assert!((sample.altitude.meters() - 20.0).abs() < 1e-9);
        assert!((sample.hdop - 0.8).abs() < 1e-9);
        assert_eq!(sample.satellites_visible, 22);
        assert!((sample.velocity_m_s - 1.2).abs() < 1e-9);
        assert!((sample.course_over_ground_deg - 90.0).abs() < 1e-9);
        assert!((sample.t_ms - 2_000.0).abs() < 1e-9);
    }

    #[test]
    fn non_gnss_frame_is_ignored() {
        let env = envelope(highres(0.0, 0.0, 0.0, 0));
        assert!(decode_gnss(&env).expect("decode").is_none());
    }

    #[test]
    fn extract_error_is_convertible() {
        let err: InstrumentError = ExtractError("boom".into()).into();
        assert!(matches!(err, InstrumentError::Extract(_)));
    }
}
