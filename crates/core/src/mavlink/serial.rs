//! Passive FC discovery and same-open-transport handoff (AUTO-03).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::connection::{spawn_with_serial, Conn, SerialBootstrap, SpawnedConnection};
use super::message::{MavHeader, MavMessage, MessageEnvelope};
use super::{ConnectionConfig, Endpoint, MavlinkError};
use crate::devices::{
    BaudSelection, LeasePhase, SerialCancellation, SerialDiscoveryError, SerialLease, SerialOwner,
};
use crate::telemetry::{Autopilot, VehicleType};

/// How long one baud rate is observed before moving to the next.
pub const FC_PROBE_WINDOW: Duration = Duration::from_secs(3);
/// Overall budget for one passive probe run.
pub const FC_PROBE_TIMEOUT: Duration = Duration::from_secs(25);
pub const FC_BAUD_RATES: [u32; 7] = [115_200, 57_600, 38_400, 9_600, 230_400, 460_800, 921_600];
const AUTOPILOT_COMPONENT: u8 = 1;
const REQUIRED_HEARTBEATS: usize = 2;
const MAX_PROBE_TARGETS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FcIdentity {
    pub system_id: u8,
    pub component_id: u8,
    pub autopilot: Autopilot,
    pub vehicle: VehicleType,
}

#[derive(Debug, Clone)]
pub struct FcProbeConfig {
    pub baud: BaudSelection,
    /// None for passive discovery before choosing a distinct GCS identity.
    /// A configured ID is checked for conflicts before handoff.
    pub local_system_id: Option<u8>,
    pub expected_target: Option<FcIdentity>,
}

/// Classify role independently of the automatic-link supported-platform policy.
pub fn classify_autopilot_heartbeat(env: &MessageEnvelope) -> Option<FcIdentity> {
    let MavMessage::HEARTBEAT(heartbeat) = &env.message else {
        return None;
    };
    if env.header.system_id == 0
        || env.header.component_id != AUTOPILOT_COMPONENT
        || heartbeat.autopilot == ::mavlink::common::MavAutopilot::MAV_AUTOPILOT_INVALID
    {
        return None;
    }
    let vehicle = VehicleType::from(heartbeat.mavtype);
    if matches!(
        vehicle,
        VehicleType::Gcs
            | VehicleType::AntennaTracker
            | VehicleType::OnboardController
            | VehicleType::Gimbal
            | VehicleType::Camera
            | VehicleType::Other
            | VehicleType::Unknown
    ) {
        return None;
    }
    Some(FcIdentity {
        system_id: env.header.system_id,
        component_id: env.header.component_id,
        autopilot: Autopilot::from(heartbeat.autopilot),
        vehicle,
    })
}

pub fn automatic_fc_supported(identity: &FcIdentity) -> bool {
    identity.autopilot == Autopilot::Px4
        && matches!(
            identity.vehicle,
            VehicleType::Quadrotor
                | VehicleType::Coaxial
                | VehicleType::Tricopter
                | VehicleType::Hexarotor
                | VehicleType::Octorotor
                | VehicleType::Dodecarotor
                | VehicleType::GenericMultirotor
        )
}

pub struct ProbedFcSerial {
    bootstrap: SerialBootstrap,
    target: FcIdentity,
    endpoint: Endpoint,
}

impl ProbedFcSerial {
    pub fn target(&self) -> &FcIdentity {
        &self.target
    }
    pub fn port(&self) -> &crate::devices::SerialPortRef {
        self.bootstrap.lease.port()
    }
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    pub async fn connect(
        self,
        config: ConnectionConfig,
    ) -> Result<SpawnedConnection, MavlinkError> {
        if config.endpoint != self.endpoint
            || config.target_system_id != self.target.system_id
            || config.target_component_id != self.target.component_id
            || config.auto_discover_target
            || config.system_id == self.target.system_id
        {
            return Err(MavlinkError::Protocol(
                "verified serial handoff identity does not match connection config".into(),
            ));
        }
        spawn_with_serial(config, Some(self.bootstrap)).await
    }
}

/// Probe reads MAVLink only; no heartbeat, stream request or configuration write
/// is sent. The final connection and lease are transferred without reopening.
pub async fn probe_fc_serial(
    lease: SerialLease,
    config: FcProbeConfig,
    cancel: SerialCancellation,
) -> Result<ProbedFcSerial, SerialDiscoveryError> {
    probe_with_opener(
        lease,
        config,
        cancel,
        FC_PROBE_WINDOW,
        FC_PROBE_TIMEOUT,
        |endpoint| async move {
            ::mavlink::connect_async::<MavMessage>(&endpoint.to_address_string()).await
        },
    )
    .await
}

async fn probe_with_opener<F, Fut>(
    lease: SerialLease,
    config: FcProbeConfig,
    cancel: SerialCancellation,
    window: Duration,
    timeout: Duration,
    mut open: F,
) -> Result<ProbedFcSerial, SerialDiscoveryError>
where
    F: FnMut(Endpoint) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Conn>>,
{
    config.baud.validate()?;
    cancel.check()?;
    if lease.owner() == SerialOwner::RtkBase {
        return Err(SerialDiscoveryError::PortBusy {
            owner: Some(SerialOwner::RtkBase),
        });
    }
    lease.set_phase(LeasePhase::Probing)?;
    // A fixed selection is tried first, then the remaining ladder: the user's
    // guess may name a rate the FC is not using (e.g. a saved TELEM port), and
    // refusing would make every manual serial connect baud-fragile.
    let rates = match config.baud {
        BaudSelection::Auto => FC_BAUD_RATES.to_vec(),
        BaudSelection::Fixed(rate) => {
            let mut rates = vec![rate];
            rates.extend(FC_BAUD_RATES.iter().copied().filter(|&r| r != rate));
            rates
        }
    };
    let mut bootstrap = SerialBootstrap {
        transport: None,
        lease,
        heartbeat: None,
        cancel,
        single_transport: true,
    };
    let result = tokio::time::timeout(timeout, async {
        let mut unrecognized_bytes: u64 = 0;
        for rate in rates {
            bootstrap.cancel.check()?;
            drop(bootstrap.transport.take());
            tokio::select! {
                biased;
                _ = bootstrap.cancel.cancelled() => return Err(SerialDiscoveryError::Cancelled),
                result = bootstrap.lease.revalidate() => result?,
            }
            let endpoint = Endpoint::Serial {
                port: bootstrap.lease.port().device.port_name.clone().into(),
                baudrate: rate,
            };
            let mut conn = match open(endpoint.clone()).await {
                Ok(conn) => conn,
                // Transient conditions that apply to every rate: stop the walk.
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(SerialDiscoveryError::PermissionDenied {
                        port: bootstrap.lease.port().device.port_name.clone(),
                    });
                }
                // A rate the OS/driver refuses (EINVAL, unsupported): try the
                // next one instead of failing the whole probe.
                Err(_) if !cancelled_now(&bootstrap) => continue,
                Err(_) => return Err(SerialDiscoveryError::Cancelled),
            };
            conn.set_allow_recv_any_version(true);
            bootstrap.transport = Some(conn);
            let Some(conn) = bootstrap.transport.as_ref() else {
                return Err(SerialDiscoveryError::WorkerGone);
            };
            let result = tokio::select! {
                biased;
                _ = bootstrap.cancel.cancelled() => return Err(SerialDiscoveryError::Cancelled),
                _ = bootstrap.lease.cancelled() => return Err(SerialDiscoveryError::Cancelled),
                result = observe_fc(conn, &config, window) => result,
            };
            match result {
                Ok((target, heartbeat)) => {
                    bootstrap.heartbeat = Some(heartbeat);
                    return Ok((target, endpoint));
                }
                Err(SerialDiscoveryError::NoAutopilot) => {}
                // The rate was right for framing but no acceptable FC showed
                // up: further rates cannot improve on a decoded stream.
                Err(error @ (SerialDiscoveryError::UnsupportedMavlinkVersion
                | SerialDiscoveryError::LocalSystemIdConflict { .. }
                | SerialDiscoveryError::UnsupportedAutopilot { .. }
                | SerialDiscoveryError::AmbiguousTargets { .. }
                | SerialDiscoveryError::UnexpectedTarget { .. })) => {
                    return Err(error);
                }
                Err(SerialDiscoveryError::UnrecognizedStream { bytes }) => {
                    unrecognized_bytes += bytes;
                }
                Err(error) => return Err(error),
            }
        }
        if unrecognized_bytes > 0 {
            return Err(SerialDiscoveryError::UnrecognizedStream {
                bytes: unrecognized_bytes,
            });
        }
        Err(SerialDiscoveryError::NoAutopilot)
    })
    .await
    .map_err(|_| SerialDiscoveryError::ProbeTimeout {
        stage: "FC discovery".into(),
    })??;
    Ok(ProbedFcSerial {
        bootstrap,
        target: result.0,
        endpoint: result.1,
    })
}

struct Evidence {
    identity: FcIdentity,
    sequences: BTreeSet<u8>,
    last: MessageEnvelope,
}

async fn observe_fc(
    conn: &Conn,
    config: &FcProbeConfig,
    window: Duration,
) -> Result<(FcIdentity, MessageEnvelope), SerialDiscoveryError> {
    let deadline = tokio::time::Instant::now() + window;
    let mut seen: BTreeMap<(u8, u8), Evidence> = BTreeMap::new();
    // Bytes that arrived without decoding into an acceptable autopilot
    // heartbeat: CRC-valid frames of other messages, frames of unknown
    // message ids, and raw garbage. Reported so the caller can tell a
    // silent/wrong-baud link from one carrying traffic we did not recognise.
    let mut unrecognized_bytes: u64 = 0;
    loop {
        let raw = match tokio::time::timeout_at(deadline, conn.recv_raw()).await {
            Err(_) => break,
            Ok(Ok(raw)) => raw,
            Ok(Err(::mavlink::error::MessageReadError::Parse(_))) => continue,
            Ok(Err(error)) => {
                return Err(SerialDiscoveryError::OpenFailed {
                    port: "active FC probe".into(),
                    message: error.to_string(),
                })
            }
        };
        let Ok(message) = <MavMessage as ::mavlink::Message>::parse(
            raw.version(),
            raw.message_id(),
            raw.payload(),
        ) else {
            unrecognized_bytes += raw_frame_len(&raw);
            continue;
        };
        let env = MessageEnvelope {
            header: MavHeader {
                system_id: raw.system_id(),
                component_id: raw.component_id(),
                sequence: raw.sequence(),
            },
            message,
            received_at: Instant::now(),
        };
        let Some(identity) = classify_autopilot_heartbeat(&env) else {
            unrecognized_bytes += raw_frame_len(&raw);
            continue;
        };
        if !matches!(raw.version(), ::mavlink::MavlinkVersion::V2) {
            return Err(SerialDiscoveryError::UnsupportedMavlinkVersion);
        }
        if config.local_system_id == Some(identity.system_id) {
            return Err(SerialDiscoveryError::LocalSystemIdConflict {
                system_id: identity.system_id,
            });
        }
        let key = (identity.system_id, identity.component_id);
        if let Some(previous) = seen.get(&key) {
            if previous.identity != identity {
                return Err(SerialDiscoveryError::UnexpectedTarget {
                    expected: previous.identity.clone(),
                    observed: identity,
                });
            }
        }
        if seen.len() >= MAX_PROBE_TARGETS && !seen.contains_key(&key) {
            return Err(SerialDiscoveryError::AmbiguousTargets {
                targets: seen.into_values().map(|e| e.identity).collect(),
            });
        }
        let evidence = seen.entry(key).or_insert_with(|| Evidence {
            identity,
            sequences: BTreeSet::new(),
            last: env.clone(),
        });
        evidence.sequences.insert(env.header.sequence);
        evidence.last = env;
    }
    choose_evidence(seen, config.expected_target.as_ref())
        .map_err(|error| {
            // Surface the observed traffic volume with the quiet-stream
            // failure: "nothing decoded" alone hides a live but foreign
            // link, which is a wiring/baud hint, not a dead port.
            if matches!(error, SerialDiscoveryError::NoAutopilot) && unrecognized_bytes > 0 {
                SerialDiscoveryError::UnrecognizedStream {
                    bytes: unrecognized_bytes,
                }
            } else {
                error
            }
        })
}

/// Encoded frame size of a raw MAVLink message (header + payload + checksum
/// [+ signature]).
fn raw_frame_len(raw: &::mavlink::MAVLinkMessageRaw) -> u64 {
    match raw {
        ::mavlink::MAVLinkMessageRaw::V1(msg) => msg.raw_bytes().len() as u64,
        ::mavlink::MAVLinkMessageRaw::V2(msg) => msg.raw_bytes().len() as u64,
    }
}

fn choose_evidence(
    mut seen: BTreeMap<(u8, u8), Evidence>,
    expected: Option<&FcIdentity>,
) -> Result<(FcIdentity, MessageEnvelope), SerialDiscoveryError> {
    if let Some(expected) = expected {
        if let Some(evidence) = seen.remove(&(expected.system_id, expected.component_id)) {
            if &evidence.identity != expected {
                return Err(SerialDiscoveryError::UnexpectedTarget {
                    expected: expected.clone(),
                    observed: evidence.identity,
                });
            }
            return verified(evidence);
        }
        if let Some(observed) = seen.into_values().next() {
            return Err(SerialDiscoveryError::UnexpectedTarget {
                expected: expected.clone(),
                observed: observed.identity,
            });
        }
        return Err(SerialDiscoveryError::NoAutopilot);
    }
    if seen.len() > 1 {
        return Err(SerialDiscoveryError::AmbiguousTargets {
            targets: seen.into_values().map(|e| e.identity).collect(),
        });
    }
    verified(
        seen.into_values()
            .next()
            .ok_or(SerialDiscoveryError::NoAutopilot)?,
    )
}

fn verified(evidence: Evidence) -> Result<(FcIdentity, MessageEnvelope), SerialDiscoveryError> {
    if !automatic_fc_supported(&evidence.identity) {
        return Err(SerialDiscoveryError::UnsupportedAutopilot {
            observed: evidence.identity,
        });
    }
    if evidence.sequences.len() < REQUIRED_HEARTBEATS {
        return Err(SerialDiscoveryError::NoAutopilot);
    }
    Ok((evidence.identity, evidence.last))
}

/// Whether the probe's cancellation fired (used to stop the baud ladder).
fn cancelled_now(bootstrap: &SerialBootstrap) -> bool {
    bootstrap.cancel.is_cancelled()
}

#[cfg(test)]
mod tests;
