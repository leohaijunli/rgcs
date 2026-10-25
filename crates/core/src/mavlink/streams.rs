//! Standard-message catalog and exact stream selection (MR-18a).
//!
//! Metadata comes from the same locked `mavlink` dependency as the decoder.
//! Search results are suggestions; resolving a selection requires an exact
//! name or ID. PX4 metadata describes source availability, not observed traffic.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[path = "standard_stream_catalog.rs"]
mod catalog;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamField {
    pub name: String,
    /// MAVLink primitive type, including an array size where applicable.
    pub wire_type: String,
    /// Wire units, not the converted engineering units used by plot adapters.
    pub units: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Px4StreamSupport {
    /// Present in the PX4 v1.17 stream registry. Board/build configuration and
    /// actual stream availability/rate still require live confirmation.
    DocumentedV1_17,
    /// Support has not been established by the catalog's PX4 source metadata.
    Unknown,
}

/// Source-reviewed PX4 v1.17 inputs to a standard stream. This is metadata,
/// not a generic uORB transport or a promise to expose every source field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Px4StreamMapping {
    pub uorb_sources: Vec<String>,
    pub source_url: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandardStream {
    pub id: u32,
    pub name: String,
    pub fields: Vec<StreamField>,
    pub px4_support: Px4StreamSupport,
    pub px4_mapping: Option<Px4StreamMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamLookupError {
    #[error("no verified standard MAVLink mapping for uORB topic {topic}; the installed firmware cannot expose arbitrary uORB topics through an interval request; select a documented standard message")]
    NoStandardMapping { topic: String },
    #[error("unknown exact MAVLink message name or ID: {selection}")]
    UnknownMessage { selection: String },
    #[error(
        "ambiguous MAVLink selection {selection}; choose an exact name or one of {candidates:?}"
    )]
    AmbiguousSelection {
        selection: String,
        candidates: Vec<u32>,
    },
}

struct FieldSpec {
    name: &'static str,
    wire_type: &'static str,
    units: Option<&'static str>,
}

struct StreamSpec {
    id: u32,
    name: &'static str,
    #[allow(dead_code)] // Checked against the compiled decoder by the catalog test.
    extra_crc: u8,
    fields: &'static [FieldSpec],
}

struct MappingSpec {
    id: u32,
    sources: &'static [&'static str],
    note: &'static str,
}

const PX4_SOURCE_BASE: &str =
    "https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/streams";
const PX4_MAPPINGS: &[MappingSpec] = &[
    MappingSpec {
        id: 24,
        sources: &["sensor_gps"],
        note: "GPS instance 0; selected position, fix and accuracy fields with wire scaling. Sends a no-GPS fallback after an established source stops updating.",
    },
    MappingSpec {
        id: 26,
        sources: &["vehicle_imu", "vehicle_imu_status", "sensor_mag"],
        note: "Instance 0; integrated accel/gyro converted to mG and mrad/s, sensor-mag values scaled to mGauss, and available sensor temperature. This is not the bias-corrected HIGHRES_IMU path.",
    },
    MappingSpec {
        id: 36,
        sources: &["actuator_outputs"],
        note: "Instances 0/1 use PX4 stream names SERVO_OUTPUT_RAW_0/_1 and port 0/1. Both share message ID 36; requesting this ID does not select a uORB instance. Output interpretation depends on the configured protocol.",
    },
    MappingSpec {
        id: 105,
        sources: &["vehicle_imu", "sensor_selection", "vehicle_magnetometer", "vehicle_air_data", "differential_pressure", "estimator_sensor_bias", "estimator_selector_status"],
        note: "Selected IMU with matching estimated accel/gyro bias removed; combines selected magnetic, air-data and differential-pressure fields. Magnetic bias is also removed when available. fields_updated identifies changed groups.",
    },
    MappingSpec {
        id: 147,
        sources: &["battery_status"],
        note: "Each updated battery instance produces selected fields, not the complete topic. Current A becomes cA, voltage V becomes mV, and disconnected/unknown fields use MAVLink sentinels. Aggregate frame rate may include multiple batteries.",
    },
    MappingSpec {
        id: 291,
        sources: &["esc_status"],
        note: "RPM, voltage and current are emitted in batches of up to four ESCs. index is the first zero-based ESC index (0, 4, ...), not a batch number; frame rate may include several batches.",
    },
];

pub fn px4_stream_mapping(message_id: u32) -> Option<Px4StreamMapping> {
    let mapping = PX4_MAPPINGS
        .iter()
        .find(|mapping| mapping.id == message_id)?;
    let stream = catalog::STREAMS
        .iter()
        .find(|stream| stream.id == message_id)?;
    Some(Px4StreamMapping {
        uorb_sources: mapping
            .sources
            .iter()
            .map(|source| (*source).into())
            .collect(),
        source_url: format!("{PX4_SOURCE_BASE}/{}.hpp", stream.name),
        note: mapping.note.into(),
    })
}

fn source_matches(message_id: u32, query: &str) -> bool {
    PX4_MAPPINGS.iter().any(|mapping| {
        mapping.id == message_id
            && mapping
                .sources
                .iter()
                .any(|source| source.eq_ignore_ascii_case(query))
    })
}

/// Resolve only exact, verified source aliases. A shared topic never silently
/// chooses a stream; unreviewed topics have no standard mapping in this catalog.
pub fn resolve_uorb_stream(topic: &str) -> Result<StandardStream, StreamLookupError> {
    let topic = topic.trim();
    let candidates: Vec<_> = catalog::STREAMS
        .iter()
        .filter(|stream| source_matches(stream.id, topic))
        .collect();
    match candidates.as_slice() {
        [stream] => Ok(stream.descriptor()),
        [] => Err(StreamLookupError::NoStandardMapping {
            topic: topic.into(),
        }),
        _ => Err(StreamLookupError::AmbiguousSelection {
            selection: topic.into(),
            candidates: candidates.iter().map(|stream| stream.id).collect(),
        }),
    }
}

// Source: PX4/PX4-Autopilot v1.17.0,
// src/modules/mavlink/mavlink_messages.cpp (included/registered stream headers).
// Some entries are conditional on dialect/build/CONSTRAINED_FLASH. This list
// never adds messages to common: a name must also exist in the compiled catalog.
const PX4_V1_17_STREAMS: &[&str] = &[
    "ACTUATOR_OUTPUT_STATUS",
    "ADSB_VEHICLE",
    "ALTITUDE",
    "ATTITUDE",
    "ATTITUDE_QUATERNION",
    "ATTITUDE_TARGET",
    "AUTOPILOT_STATE_FOR_GIMBAL_DEVICE",
    "AUTOPILOT_VERSION",
    "AVAILABLE_MODES",
    "BATTERY_INFO",
    "BATTERY_STATUS",
    "CAMERA_IMAGE_CAPTURED",
    "CAMERA_TRIGGER",
    "COMMAND_LONG",
    "COMPONENT_INFORMATION",
    "COMPONENT_METADATA",
    "CURRENT_MODE",
    "DEBUG",
    "DEBUG_FLOAT_ARRAY",
    "DEBUG_VECT",
    "DISTANCE_SENSOR",
    "EFI_STATUS",
    "ESC_INFO",
    "ESC_STATUS",
    "ESTIMATOR_STATUS",
    "EXTENDED_SYS_STATE",
    "FIGURE_EIGHT_EXECUTION_STATUS",
    "FLIGHT_INFORMATION",
    "FUEL_STATUS",
    "GIMBAL_DEVICE_ATTITUDE_STATUS",
    "GIMBAL_DEVICE_INFORMATION",
    "GIMBAL_DEVICE_SET_ATTITUDE",
    "GIMBAL_MANAGER_INFORMATION",
    "GIMBAL_MANAGER_STATUS",
    "GLOBAL_POSITION",
    "GLOBAL_POSITION_INT",
    "GNSS_INTEGRITY",
    "GPS2_RAW",
    "GPS_GLOBAL_ORIGIN",
    "GPS_RAW_INT",
    "GPS_RTCM_DATA",
    "GPS_STATUS",
    "HEARTBEAT",
    "HIGH_LATENCY2",
    "HIGHRES_IMU",
    "HIL_ACTUATOR_CONTROLS",
    "HIL_STATE_QUATERNION",
    "HOME_POSITION",
    "HYGROMETER_SENSOR",
    "LANDING_TARGET",
    "LINK_NODE_STATUS",
    "LOCAL_POSITION_NED",
    "MAG_CAL_REPORT",
    "MANUAL_CONTROL",
    "MOUNT_ORIENTATION",
    "NAMED_VALUE_FLOAT",
    "NAV_CONTROLLER_OUTPUT",
    "OBSTACLE_DISTANCE",
    "ODOMETRY",
    "OPEN_DRONE_ID_ARM_STATUS",
    "OPEN_DRONE_ID_BASIC_ID",
    "OPEN_DRONE_ID_LOCATION",
    "OPEN_DRONE_ID_SYSTEM",
    "OPTICAL_FLOW_RAD",
    "ORBIT_EXECUTION_STATUS",
    "PING",
    "POSITION_TARGET_GLOBAL_INT",
    "POSITION_TARGET_LOCAL_NED",
    "PROTOCOL_VERSION",
    "RAW_RPM",
    "RC_CHANNELS",
    "SCALED_IMU",
    "SCALED_IMU2",
    "SCALED_IMU3",
    "SCALED_PRESSURE",
    "SCALED_PRESSURE2",
    "SCALED_PRESSURE3",
    "SERVO_OUTPUT_RAW",
    "STATUSTEXT",
    "STORAGE_INFORMATION",
    "SYS_STATUS",
    "SYSTEM_TIME",
    "TIME_ESTIMATE_TO_TARGET",
    "TIMESYNC",
    "VFR_HUD",
    "VIBRATION",
    "WIND_COV",
];

impl StreamSpec {
    fn descriptor(&self) -> StandardStream {
        StandardStream {
            id: self.id,
            name: self.name.into(),
            fields: self
                .fields
                .iter()
                .map(|field| StreamField {
                    name: field.name.into(),
                    wire_type: field.wire_type.into(),
                    units: field.units.map(Into::into),
                })
                .collect(),
            px4_support: if PX4_V1_17_STREAMS.contains(&self.name) {
                Px4StreamSupport::DocumentedV1_17
            } else {
                Px4StreamSupport::Unknown
            },
            px4_mapping: px4_stream_mapping(self.id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Exact,
    UorbSourceExact,
    NamePrefix,
    NamePart,
    FieldExact,
    FieldPart,
}

fn contains_ascii(haystack: &str, needle: &str) -> bool {
    needle.is_empty()
        || haystack
            .as_bytes()
            .windows(needle.len())
            .any(|part| part.eq_ignore_ascii_case(needle.as_bytes()))
}

fn matches(query: &str) -> Vec<(Rank, &'static StreamSpec)> {
    let name = query.to_ascii_uppercase();
    let id = query.parse::<u32>().ok();
    let mut matches: Vec<_> = catalog::STREAMS
        .iter()
        .filter_map(|stream| {
            let rank = if query.is_empty() || stream.name == name || id == Some(stream.id) {
                Rank::Exact
            } else if source_matches(stream.id, query) {
                Rank::UorbSourceExact
            } else if stream.name.starts_with(&name) {
                Rank::NamePrefix
            } else if stream.name.contains(&name) {
                Rank::NamePart
            } else if stream
                .fields
                .iter()
                .any(|field| field.name.eq_ignore_ascii_case(query))
            {
                Rank::FieldExact
            } else if stream
                .fields
                .iter()
                .any(|field| contains_ascii(field.name, query))
            {
                Rank::FieldPart
            } else {
                return None;
            };
            Some((rank, stream))
        })
        .collect();
    matches.sort_unstable_by_key(|(rank, stream)| (*rank, stream.id));
    matches
}

/// Case-insensitive name/field search. Exact name and numeric-ID matches rank
/// first, then exact verified uORB sources. Blank returns stable numeric order.
pub fn search_standard_streams(query: &str) -> Vec<StandardStream> {
    matches(query.trim())
        .into_iter()
        .map(|(_, stream)| stream.descriptor())
        .collect()
}

/// Resolve an exact name, numeric ID or unique verified uORB source. A partial result is
/// still not permission to select that message; multiple partial matches return
/// candidate IDs so the operator can make an explicit choice.
pub fn resolve_standard_stream(selection: &str) -> Result<StandardStream, StreamLookupError> {
    let selection = selection.trim();
    let found = matches(selection);
    if !selection.is_empty() {
        if let Some((Rank::Exact, stream)) = found.first() {
            return Ok(stream.descriptor());
        }
        if matches!(found.first(), Some((Rank::UorbSourceExact, _))) {
            return resolve_uorb_stream(selection);
        }
        if found.len() > 1 {
            return Err(StreamLookupError::AmbiguousSelection {
                selection: selection.into(),
                candidates: found.iter().map(|(_, stream)| stream.id).collect(),
            });
        }
    }
    Err(StreamLookupError::UnknownMessage {
        selection: selection.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mavlink::MavMessage;
    use ::mavlink::Message;

    #[test]
    fn every_catalog_entry_matches_the_compiled_common_decoder() {
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for entry in catalog::STREAMS {
            assert!(ids.insert(entry.id), "duplicate ID {}", entry.id);
            assert!(names.insert(entry.name), "duplicate name {}", entry.name);
            let message = MavMessage::default_message_from_id(entry.id).expect("compiled message");
            assert_eq!(message.message_name(), entry.name);
            assert_eq!(MavMessage::message_id_from_name(entry.name), Some(entry.id));
            assert_eq!(
                MavMessage::extra_crc(entry.id),
                entry.extra_crc,
                "{} base wire fields",
                entry.name
            );
        }
    }

    #[test]
    fn required_streams_resolve_and_rank_first_by_exact_name_and_id() {
        for (name, id) in [
            ("BATTERY_STATUS", 147),
            ("SCALED_IMU", 26),
            ("HIGHRES_IMU", 105),
        ] {
            for query in [name.into(), name.to_ascii_lowercase(), format!(" {id} ")] {
                let resolved = resolve_standard_stream(&query).expect("exact stream");
                assert_eq!((resolved.name.as_str(), resolved.id), (name, id));
                assert_eq!(resolved.px4_support, Px4StreamSupport::DocumentedV1_17);
                assert_eq!(search_standard_streams(&query).first(), Some(&resolved));
            }
        }
    }

    #[test]
    fn field_metadata_keeps_wire_units_arrays_and_extensions() {
        let battery = resolve_standard_stream("147").expect("battery");
        for (name, wire_type, units) in [
            ("current_battery", "int16_t", "cA"),
            ("voltages", "uint16_t[10]", "mV"),
            ("voltages_ext", "uint16_t[4]", "mV"),
        ] {
            let field = battery
                .fields
                .iter()
                .find(|field| field.name == name)
                .expect("field");
            assert_eq!(field.wire_type, wire_type);
            assert_eq!(field.units.as_deref(), Some(units));
        }
        let imu = resolve_standard_stream("HIGHRES_IMU").expect("IMU");
        assert_eq!(
            imu.fields
                .iter()
                .find(|field| field.name == "xacc")
                .expect("xacc")
                .wire_type,
            "float"
        );
        let heartbeat = resolve_standard_stream("0").expect("inherited minimal message");
        assert_eq!(heartbeat.name, "HEARTBEAT");
        assert!(heartbeat
            .fields
            .iter()
            .any(|field| field.name == "mavlink_version"
                && field.wire_type == "uint8_t"
                && field.units.is_none()));
    }

    #[test]
    fn field_search_is_stable_but_does_not_silently_select_a_message() {
        let results = search_standard_streams("CURRENT_BATTERY");
        assert!(results.iter().any(|stream| stream.id == 147));
        assert!(results.iter().any(|stream| stream.id == 1));
        assert_eq!(results, search_standard_streams("current_battery"));
        assert!(
            matches!(resolve_standard_stream("current_battery"), Err(StreamLookupError::AmbiguousSelection { candidates, .. }) if candidates.contains(&147) && candidates.contains(&1))
        );
        assert!(matches!(
            resolve_standard_stream("SCALED_IM"),
            Err(StreamLookupError::AmbiguousSelection { .. })
        ));
        assert!(
            matches!(
                resolve_standard_stream("HIGHRES_IM"),
                Err(StreamLookupError::UnknownMessage { .. })
            ),
            "unique partial names still require exact selection"
        );
    }

    #[test]
    fn unknown_queries_never_invent_messages_or_firmware_support() {
        for selection in [
            "",
            "-1",
            "16777216",
            "9999999999999999999999",
            "mag_sensor_custom_topic",
            "🌐",
        ] {
            assert!(matches!(
                resolve_standard_stream(selection),
                Err(StreamLookupError::UnknownMessage { .. })
            ));
        }
        assert!(search_standard_streams("mag_sensor_custom_topic").is_empty());
        assert_eq!(
            resolve_standard_stream("MISSION_REQUEST_LIST")
                .expect("common protocol message")
                .px4_support,
            Px4StreamSupport::Unknown
        );
        let full = search_standard_streams("");
        assert_eq!(full.len(), catalog::STREAMS.len());
        assert!(full.windows(2).all(|pair| pair[0].id < pair[1].id));
    }

    #[test]
    fn verified_sources_resolve_with_evidence_and_preserve_exact_name_precedence() {
        for (id, name, alias) in [
            (24, "GPS_RAW_INT", "sensor_gps"),
            (26, "SCALED_IMU", "sensor_mag"),
            (36, "SERVO_OUTPUT_RAW", "actuator_outputs"),
            (105, "HIGHRES_IMU", "vehicle_magnetometer"),
            (147, "BATTERY_STATUS", "battery_status"),
            (291, "ESC_STATUS", "esc_status"),
        ] {
            let resolved = resolve_uorb_stream(alias).expect("verified source");
            assert_eq!(resolved.id, id);
            assert_eq!(resolved.name, name);
            let mapping = resolved.px4_mapping.as_ref().expect("source evidence");
            assert!(mapping.uorb_sources.iter().any(|source| source == alias));
            assert_eq!(mapping.source_url, format!("{PX4_SOURCE_BASE}/{name}.hpp"));
            assert!(!mapping.note.is_empty());
            for query in [
                name.to_string(),
                id.to_string(),
                name.to_ascii_lowercase(),
                format!(" {} ", alias.to_ascii_uppercase()),
            ] {
                assert_eq!(
                    resolve_standard_stream(&query).expect("exact selection"),
                    resolved
                );
                assert_eq!(search_standard_streams(&query).first(), Some(&resolved));
            }
        }
        assert!(
            px4_stream_mapping(0).is_none(),
            "registry presence does not invent source evidence"
        );
        assert_eq!(search_standard_streams("battery_status")[0].id, 147);
    }

    #[test]
    fn shared_imu_source_is_ambiguous_and_unknown_topics_are_explicitly_unmapped() {
        let expected = StreamLookupError::AmbiguousSelection {
            selection: "vehicle_imu".into(),
            candidates: vec![26, 105],
        };
        assert_eq!(resolve_uorb_stream("vehicle_imu"), Err(expected.clone()));
        assert_eq!(resolve_standard_stream("vehicle_imu"), Err(expected));
        assert_eq!(
            search_standard_streams("vehicle_imu")
                .iter()
                .map(|stream| stream.id)
                .collect::<Vec<_>>(),
            vec![26, 105]
        );
        for topic in [
            "vehicle_local_position",
            "mag_sensor_custom_topic",
            "scaled_imu",
            "highres_imu",
            "sensor_",
            "",
        ] {
            let error = resolve_uorb_stream(topic).expect_err("not a verified topic");
            assert_eq!(
                error,
                StreamLookupError::NoStandardMapping {
                    topic: topic.into()
                }
            );
            assert!(error
                .to_string()
                .contains("select a documented standard message"));
        }
        assert!(search_standard_streams("mag_sensor_custom_topic").is_empty());
    }
}
