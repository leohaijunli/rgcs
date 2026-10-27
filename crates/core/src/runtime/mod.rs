//! Small, transport-independent Runtime contracts (AF-00/01).
//!
//! Logical aircraft identities are separate from MAVLink routing addresses.
//! Services remain in their existing modules; this module does not open I/O or
//! create protocol workers. The desktop facade composes existing services.

mod ids;
pub mod operations;

pub use ids::{
    ClientId, LinkId, ModuleInstanceId, OperationId, RequestId, Revision, SubscriptionId,
    TransportGeneration, VehicleId, VehicleSessionId,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Existing route identity, not the application logical aircraft ID.
pub type MavlinkTarget = crate::telemetry::VehicleId;

/// Shared native target-availability freshness threshold, matching the current
/// primary-serial offer and lifecycle telemetry policy.
pub const TARGET_HEARTBEAT_FRESH_MS: u64 = 3_000;

/// An exact Runtime context, echoed by callers and checked before use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RuntimeTargetRef")]
pub struct TargetRef {
    pub vehicle_id: VehicleId,
    pub session_id: VehicleSessionId,
    pub link_id: LinkId,
    pub generation: TransportGeneration,
    pub mavlink_target: MavlinkTarget,
}

impl TargetRef {
    /// Reject broadcast addresses even when a caller deserialized the existing
    /// telemetry node type, whose constructor intentionally accepts any bytes.
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if self.mavlink_target.system_id == 0 || self.mavlink_target.component_id == 0 {
            return Err(RuntimeError::InvalidIdentity {
                kind: "MAVLink target".into(),
                reason: "a Runtime target must use nonzero unicast IDs".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RuntimeTargetAvailability")]
#[serde(rename_all = "snake_case")]
pub enum TargetAvailability {
    Unverified,
    Available,
    Disconnected,
    Stale,
    Unsupported,
}

/// Current supported client roles, not a fleet registry/product feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, rename = "RuntimeTargetRole")]
#[serde(rename_all = "snake_case")]
pub enum TargetRole {
    Primary,
    MotorTest,
}

/// The actual transport owner. A Motor Test role can appear on a primary-owned
/// target without becoming its I/O or parameter-cache owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RuntimeTransportOwner")]
#[serde(rename_all = "snake_case")]
pub enum TransportOwner {
    Primary,
    DedicatedMotorTest,
}

/// Lightweight state that does not grant operation or motor permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename = "RuntimeTargetSnapshot")]
pub struct TargetSnapshot {
    pub link_id: LinkId,
    pub target: Option<TargetRef>,
    pub availability: TargetAvailability,
    #[ts(type = "number | null")]
    pub heartbeat_age_ms: Option<u64>,
    pub parameter_service_available: bool,
    pub roles: Vec<TargetRole>,
    pub transport_owner: TransportOwner,
    pub endpoint: String,
}

/// Errors are structured across IPC; none implies successful physical cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, TS)]
#[ts(export)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RuntimeError {
    #[error("the requested target is unavailable or unverified")]
    TargetUnavailable,
    #[error("the target session, route or transport generation changed")]
    TargetChanged,
    #[error("invalid {kind}: {reason}")]
    InvalidIdentity { kind: String, reason: String },
    #[error("cannot allocate Runtime identity: {message}")]
    IdentityUnavailable { message: String },
    #[error("{kind} identity space is exhausted")]
    IdentityExhausted { kind: String },
    #[error("the {service} service is unavailable")]
    ServiceUnavailable { service: String },
    #[error("invalid Runtime request: {message}")]
    InvalidRequest { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> TargetRef {
        TargetRef {
            vehicle_id: VehicleId::new("logical-aircraft-a").expect("vehicle ID"),
            session_id: VehicleSessionId::new("session-a").expect("session ID"),
            link_id: LinkId::new(7).expect("link ID"),
            generation: TransportGeneration::new(1).expect("generation"),
            mavlink_target: MavlinkTarget::new(42, 1),
        }
    }

    #[test]
    fn logical_aircraft_and_mavlink_route_are_separate() {
        let first = reference();
        let mut another_aircraft = first.clone();
        another_aircraft.vehicle_id = VehicleId::new("logical-aircraft-b").expect("vehicle ID");
        assert_ne!(first, another_aircraft);
        assert_eq!(first.mavlink_target, another_aircraft.mavlink_target);
        assert!(first.validate().is_ok());
    }

    #[test]
    fn exact_reference_includes_session_link_generation_and_route() {
        let original = reference();
        let mut replacements = [
            original.clone(),
            original.clone(),
            original.clone(),
            original.clone(),
        ];
        replacements[0].session_id = VehicleSessionId::new("session-b").expect("session ID");
        replacements[1].link_id = LinkId::new(8).expect("link ID");
        replacements[2].generation = TransportGeneration::new(2).expect("generation");
        replacements[3].mavlink_target = MavlinkTarget::new(43, 1);
        for replacement in replacements {
            assert_ne!(original, replacement);
        }
    }

    #[test]
    fn broadcast_addresses_are_not_control_target_references() {
        for address in [MavlinkTarget::new(0, 1), MavlinkTarget::new(42, 0)] {
            let mut target = reference();
            target.mavlink_target = address;
            assert!(matches!(
                target.validate(),
                Err(RuntimeError::InvalidIdentity { .. })
            ));
        }
    }

    #[test]
    fn target_reference_round_trip_retains_large_generation_and_route() {
        let mut target = reference();
        target.generation = TransportGeneration::new(u64::MAX).expect("generation");
        let json = serde_json::to_string(&target).expect("serialize");
        assert!(json.contains("\"generation\":\"18446744073709551615\""));
        assert_eq!(
            serde_json::from_str::<TargetRef>(&json).expect("deserialize"),
            target
        );
    }
}
