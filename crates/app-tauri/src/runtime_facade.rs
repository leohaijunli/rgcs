//! AF-01: narrow, explicit target context over the existing native owners.
//!
//! This facade creates identities and copied observations only. It never opens a
//! transport, starts a parameter worker, owns a cache, or shuts down borrowed I/O.
//! Operation/effect APIs are added with AF-02's owner-bound coordination.

use maggcs_core::mavlink::serial::automatic_fc_supported;
use maggcs_core::mavlink::{ConnectionHandle, LinkState};
use maggcs_core::params::service::{validate_name, ParamObservation, ParamService};
use maggcs_core::runtime::{
    LinkId, MavlinkTarget, RuntimeError, TargetAvailability, TargetRef, TargetRole, TargetSnapshot,
    TransportGeneration, TransportOwner, VehicleId, VehicleSessionId, TARGET_HEARTBEAT_FRESH_MS,
};
use maggcs_core::telemetry::{hub::TelemetryHub, Heartbeat, TelemetrySnapshot};
use parking_lot::Mutex;
use std::collections::{BTreeMap, BTreeSet};
use tauri::{AppHandle, Manager, Runtime};

use crate::motor_test_state::TargetLinkOrigin;
use crate::state::AppState;

/// Supported operational flight context plus the existing independent bench.
/// A borrowed bench aliases the primary entry. This is not a fleet feature.
const MAX_TARGET_CONTEXTS: usize = 2;

#[derive(Clone)]
struct SourceView {
    handle_id: usize,
    link_id: Option<LinkId>,
    generation: u64,
    node: Option<MavlinkTarget>,
    availability: TargetAvailability,
    heartbeat_age_ms: Option<u64>,
    parameters: bool,
    roles: Vec<TargetRole>,
    owner: TransportOwner,
    endpoint: String,
}

pub(crate) struct TargetSource {
    connection: ConnectionHandle,
    hub: Option<TelemetryHub>,
    params: Option<ParamService>,
    command: Option<maggcs_core::CommandService>,
    mission: Option<maggcs_core::mission::MissionService>,
    view: SourceView,
}

/// Native composition only: reuse the verified target's existing parameter
/// worker/transport. Tool consumers still receive restricted TargetHandle APIs.
pub(crate) struct ParameterBinding {
    pub connection: ConnectionHandle,
    pub service: ParamService,
}

pub(crate) struct CommandBinding {
    pub connection: ConnectionHandle,
    pub service: maggcs_core::CommandService,
}

pub(crate) struct MissionBinding {
    pub connection: ConnectionHandle,
    pub service: maggcs_core::mission::MissionService,
}

struct SourceRole {
    parameters: bool,
    link_id: Option<LinkId>,
    owner: TransportOwner,
}

impl TargetSource {
    pub(crate) fn primary(
        id: LinkId,
        connection: ConnectionHandle,
        hub: TelemetryHub,
        params: Option<ParamService>,
        command: maggcs_core::CommandService,
        mission: maggcs_core::mission::MissionService,
    ) -> Self {
        let snapshot = hub.snapshot();
        let link = hub.subscribe_link();
        let connected = link.borrow().link_state == LinkState::Connected && link.borrow().fc_alive;
        let age = snapshot
            .field_ages
            .heartbeat_at_ms
            .and_then(|at| epoch_ms().and_then(|now| now.checked_sub(at)));
        let view = view_for_connection(
            &connection,
            snapshot.heartbeat.as_ref(),
            age,
            connected,
            SourceRole {
                parameters: params.is_some(),
                link_id: Some(id),
                owner: TransportOwner::Primary,
            },
        );
        Self {
            connection,
            hub: Some(hub),
            params,
            command: Some(command),
            mission: Some(mission),
            view,
        }
    }
}

fn epoch_ms() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

fn view_for_connection(
    connection: &ConnectionHandle,
    heartbeat: Option<&Heartbeat>,
    heartbeat_age_ms: Option<u64>,
    connected: bool,
    role: SourceRole,
) -> SourceView {
    let SourceRole {
        parameters,
        link_id,
        owner,
    } = role;
    let generation = connection.generation();
    let identity = connection.target_identity();
    let node = identity
        .as_ref()
        .map(|identity| MavlinkTarget::new(identity.system_id, identity.component_id));
    let availability = if connection.is_shutdown_requested() {
        TargetAvailability::Disconnected
    } else if generation == 0 || identity.is_none() {
        TargetAvailability::Unverified
    } else if identity
        .as_ref()
        .is_some_and(|identity| !automatic_fc_supported(identity))
    {
        TargetAvailability::Unsupported
    } else if !connected {
        TargetAvailability::Disconnected
    } else if heartbeat_age_ms.is_some_and(|age| age > TARGET_HEARTBEAT_FRESH_MS) {
        TargetAvailability::Stale
    } else if !heartbeat
        .zip(identity.as_ref())
        .is_some_and(|(heartbeat, identity)| {
            heartbeat.system_id == identity.system_id
                && heartbeat.component_id == identity.component_id
                && heartbeat.autopilot == identity.autopilot
                && heartbeat.vehicle_type == identity.vehicle
                && heartbeat_age_ms.is_some()
        })
    {
        TargetAvailability::Unverified
    } else {
        TargetAvailability::Available
    };
    SourceView {
        handle_id: connection.handle_id(),
        link_id,
        generation,
        node,
        availability,
        heartbeat_age_ms,
        parameters,
        roles: vec![match owner {
            TransportOwner::Primary => TargetRole::Primary,
            TransportOwner::DedicatedMotorTest => TargetRole::MotorTest,
        }],
        owner,
        endpoint: connection.config().endpoint.to_address_string(),
    }
}

fn sources(state: &AppState) -> Vec<TargetSource> {
    let mut sources: Vec<_> = state.primary_runtime_source().into_iter().collect();
    // Do not hold the target lock while acquiring the telemetry lock; the
    // telemetry producer and release paths must never wait on an inverted pair.
    let target = state.motor_test.target.lock().as_ref().map(|target| {
        (
            target.origin(),
            target.generation_is_current(),
            target.target.handle.clone(),
            target.target.params.clone(),
        )
    });
    let Some((origin, generation_current, connection, params)) = target else {
        return sources;
    };
    if origin == TargetLinkOrigin::MainSerial {
        // A borrowed role cannot extend a retired primary transport's context,
        // and cannot introduce a second cache or I/O-owner entry.
        if let Some(primary) = sources.iter_mut().find(|source| {
            source.connection.handle_id() == connection.handle_id() && generation_current
        }) {
            primary.view.roles.push(TargetRole::MotorTest);
        }
        return sources;
    }
    let telemetry = state.motor_test.telemetry.lock();
    let opened_at = connection.connected_since_snapshot();
    let current_heartbeat = telemetry.as_ref().filter(|telemetry| {
        telemetry
            .last_heartbeat_at
            .zip(opened_at)
            .is_some_and(|(heartbeat, opened)| heartbeat >= opened)
    });
    let heartbeat = current_heartbeat.and_then(|telemetry| telemetry.hb.as_ref());
    let age = current_heartbeat
        .and_then(|telemetry| telemetry.last_heartbeat_at)
        .map(|at| at.elapsed().as_millis().min(u64::MAX as u128) as u64);
    let view = view_for_connection(
        &connection,
        heartbeat,
        age,
        generation_current && heartbeat.is_some(),
        SourceRole {
            parameters: true,
            link_id: None,
            owner: TransportOwner::DedicatedMotorTest,
        },
    );
    sources.push(TargetSource {
        connection,
        hub: None,
        params: Some(params),
        command: None,
        mission: None,
        view,
    });
    sources
}

struct Context {
    link_id: LinkId,
    vehicle_id: VehicleId,
    session_id: VehicleSessionId,
    generation: u64,
    node: Option<MavlinkTarget>,
}

impl Context {
    fn new(source: &SourceView) -> Result<Self, RuntimeError> {
        Ok(Self {
            link_id: match source.link_id {
                Some(id) => id,
                None => LinkId::next()?,
            },
            vehicle_id: VehicleId::fresh()?,
            session_id: VehicleSessionId::fresh()?,
            generation: source.generation,
            node: source.node,
        })
    }

    fn matches(&self, expected: &TargetRef) -> bool {
        self.link_id == expected.link_id
            && self.vehicle_id == expected.vehicle_id
            && self.session_id == expected.session_id
            && self.generation == expected.generation.get()
            && self.node == Some(expected.mavlink_target)
    }

    fn snapshot(&self, source: &SourceView) -> Result<TargetSnapshot, RuntimeError> {
        let target = if source.availability == TargetAvailability::Available {
            Some(TargetRef {
                vehicle_id: self.vehicle_id.clone(),
                session_id: self.session_id.clone(),
                link_id: self.link_id,
                generation: TransportGeneration::new(self.generation)?,
                mavlink_target: self.node.ok_or(RuntimeError::TargetUnavailable)?,
            })
        } else {
            None
        };
        Ok(TargetSnapshot {
            link_id: self.link_id,
            target,
            availability: source.availability,
            heartbeat_age_ms: source.heartbeat_age_ms,
            parameter_service_available: source.parameters,
            roles: source.roles.clone(),
            transport_owner: source.owner,
            endpoint: source.endpoint.clone(),
        })
    }
}

/// Metadata only, initialized without transports or tasks. Retained contexts are
/// bounded by the actual supported primary/dedicated sources, not a UI selection.
#[derive(Default)]
pub struct RuntimeState {
    /// Serialize metadata capture/publication so an older concurrent lookup
    /// cannot retire the contexts installed by a newer lookup. No I/O uses it.
    snapshots: Mutex<()>,
    contexts: Mutex<BTreeMap<usize, Context>>,
    /// A successfully accepted reboot makes this generation unavailable before
    /// the transport necessarily drops. A later generation clears the barrier.
    retired_generations: Mutex<BTreeMap<usize, u64>>,
}

impl RuntimeState {
    fn refresh(&self, views: &[SourceView]) -> Result<Vec<TargetSnapshot>, RuntimeError> {
        if views.len() > MAX_TARGET_CONTEXTS {
            return Err(RuntimeError::InvalidRequest {
                message: "too many supported target contexts".into(),
            });
        }
        let active: BTreeSet<_> = views.iter().map(|source| source.handle_id).collect();
        let mut retired = self.retired_generations.lock();
        retired.retain(|handle, generation| {
            views
                .iter()
                .any(|source| source.handle_id == *handle && source.generation == *generation)
        });
        let mut contexts = self.contexts.lock();
        contexts.retain(|handle, _| active.contains(handle));
        let mut snapshots = Vec::with_capacity(views.len());
        for source in views {
            let replace = contexts
                .get(&source.handle_id)
                .is_none_or(|context| source.link_id.is_some_and(|id| context.link_id != id));
            if replace {
                contexts.insert(source.handle_id, Context::new(source)?);
            }
            let context = contexts
                .get_mut(&source.handle_id)
                .ok_or(RuntimeError::TargetUnavailable)?;
            if context.generation > source.generation {
                return Err(RuntimeError::TargetChanged);
            }
            if source.node.is_some() && context.node.is_some() && source.node != context.node {
                // Equal endpoint/IDs cannot prove physical identity continuity.
                // A changed verified node starts a new logical registration.
                context.vehicle_id = VehicleId::fresh()?;
                context.session_id = VehicleSessionId::fresh()?;
            } else if context.generation != source.generation {
                context.session_id = VehicleSessionId::fresh()?;
            }
            context.generation = source.generation;
            if source.node.is_some() {
                context.node = source.node;
            }
            if retired.get(&source.handle_id) == Some(&source.generation) {
                let mut unavailable = source.clone();
                unavailable.availability = TargetAvailability::Unverified;
                snapshots.push(context.snapshot(&unavailable)?);
            } else {
                snapshots.push(context.snapshot(source)?);
            }
        }
        Ok(snapshots)
    }

    fn retire(&self, target: &TargetRef) {
        let handle = self
            .contexts
            .lock()
            .iter()
            .find(|(_, context)| context.matches(target))
            .map(|(handle, _)| *handle);
        if let Some(handle) = handle {
            self.retired_generations
                .lock()
                .insert(handle, target.generation.get());
        }
    }

    fn handle_for(&self, target: &TargetRef) -> Result<usize, RuntimeError> {
        self.contexts
            .lock()
            .iter()
            .find(|(_, context)| context.matches(target))
            .map(|(handle, _)| *handle)
            .ok_or(RuntimeError::TargetChanged)
    }
}

/// Trusted native composition is private to this facade. Consumers receive only
/// target snapshots/handles, not this AppHandle or a mutable application store.
pub struct RuntimeFacade<R: Runtime> {
    app: AppHandle<R>,
}

pub trait TargetRuntime<R: Runtime> {
    fn snapshot(&self, link: LinkId) -> Result<TargetSnapshot, RuntimeError>;
    fn bind(&self, expected: &TargetRef) -> Result<TargetHandle<R>, RuntimeError>;
}

impl<R: Runtime> Clone for RuntimeFacade<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
        }
    }
}

impl<R: Runtime> RuntimeFacade<R> {
    pub fn new(app: &AppHandle<R>) -> Self {
        Self { app: app.clone() }
    }

    pub fn targets(&self) -> Result<Vec<TargetSnapshot>, RuntimeError> {
        let state = self.app.state::<AppState>();
        let _snapshot = state.runtime.snapshots.lock();
        let sources = sources(&state);
        state.runtime.refresh(
            &sources
                .iter()
                .map(|source| source.view.clone())
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn parameter_binding(
        &self,
        expected: &TargetRef,
    ) -> Result<ParameterBinding, RuntimeError> {
        let source = self.resolve(expected)?;
        let service = source
            .params
            .ok_or_else(|| RuntimeError::ServiceUnavailable {
                service: "parameters".into(),
            })?;
        Ok(ParameterBinding {
            connection: source.connection,
            service,
        })
    }

    pub(crate) fn command_binding(
        &self,
        expected: &TargetRef,
    ) -> Result<CommandBinding, RuntimeError> {
        let source = self.resolve(expected)?;
        let service = source
            .command
            .ok_or_else(|| RuntimeError::ServiceUnavailable {
                service: "commands".into(),
            })?;
        Ok(CommandBinding {
            connection: source.connection,
            service,
        })
    }

    pub(crate) fn mission_binding(
        &self,
        expected: &TargetRef,
    ) -> Result<MissionBinding, RuntimeError> {
        let source = self.resolve(expected)?;
        let service = source
            .mission
            .ok_or_else(|| RuntimeError::ServiceUnavailable {
                service: "mission".into(),
            })?;
        Ok(MissionBinding {
            connection: source.connection,
            service,
        })
    }

    /// Reboot acceptance retires the exact verified generation immediately;
    /// reconnect creates a fresh session and clears this metadata barrier.
    pub(crate) fn retire_target(&self, expected: &TargetRef) -> Result<(), RuntimeError> {
        expected.validate()?;
        let state = self.app.state::<AppState>();
        let _snapshot = state.runtime.snapshots.lock();
        let sources = sources(&state);
        state.runtime.refresh(
            &sources
                .iter()
                .map(|source| source.view.clone())
                .collect::<Vec<_>>(),
        )?;
        state.runtime.retire(expected);
        Ok(())
    }

    fn resolve(&self, expected: &TargetRef) -> Result<TargetSource, RuntimeError> {
        expected.validate()?;
        let state = self.app.state::<AppState>();
        let _snapshot = state.runtime.snapshots.lock();
        let sources = sources(&state);
        let snapshots = state.runtime.refresh(
            &sources
                .iter()
                .map(|source| source.view.clone())
                .collect::<Vec<_>>(),
        )?;
        let handle = state.runtime.handle_for(expected)?;
        if !snapshots
            .iter()
            .any(|snapshot| snapshot.target.as_ref() == Some(expected))
        {
            return Err(RuntimeError::TargetUnavailable);
        }
        let source = sources
            .into_iter()
            .find(|source| source.view.handle_id == handle)
            .ok_or(RuntimeError::TargetChanged)?;
        if source.connection.generation() != expected.generation.get()
            || source.connection.is_shutdown_requested()
        {
            return Err(RuntimeError::TargetChanged);
        }
        Ok(source)
    }
}

impl<R: Runtime> TargetRuntime<R> for RuntimeFacade<R> {
    fn snapshot(&self, link: LinkId) -> Result<TargetSnapshot, RuntimeError> {
        self.targets()?
            .into_iter()
            .find(|snapshot| snapshot.link_id == link)
            .ok_or(RuntimeError::TargetUnavailable)
    }

    fn bind(&self, expected: &TargetRef) -> Result<TargetHandle<R>, RuntimeError> {
        self.resolve(expected)?;
        Ok(TargetHandle {
            runtime: self.clone(),
            target: expected.clone(),
        })
    }
}

/// A target-bound read capability. There is deliberately no connection, mutable
/// cache, global cancellation, transport shutdown or motor-permission accessor.
pub struct TargetHandle<R: Runtime> {
    runtime: RuntimeFacade<R>,
    target: TargetRef,
}

impl<R: Runtime> TargetHandle<R> {
    pub fn target(&self) -> &TargetRef {
        &self.target
    }

    pub fn telemetry_snapshot(&self) -> Result<Option<TelemetrySnapshot>, RuntimeError> {
        let source = self.runtime.resolve(self.target())?;
        let snapshot = source.hub.map(|hub| hub.snapshot());
        self.runtime.resolve(self.target())?;
        Ok(snapshot)
    }

    pub fn parameter_observation(
        &self,
        name: &str,
    ) -> Result<Option<ParamObservation>, RuntimeError> {
        validate_name(name).map_err(|error| RuntimeError::InvalidRequest {
            message: error.to_string(),
        })?;
        let source = self.runtime.resolve(self.target())?;
        let opened = source
            .connection
            .connected_since_snapshot()
            .ok_or(RuntimeError::TargetUnavailable)?;
        let params = source
            .params
            .ok_or_else(|| RuntimeError::ServiceUnavailable {
                service: "parameters".into(),
            })?;
        let observation = params.cache().observation_since(name, opened);
        self.runtime.resolve(self.target())?;
        Ok(observation)
    }
}

#[tauri::command]
pub fn runtime_targets(app: AppHandle) -> Result<Vec<TargetSnapshot>, RuntimeError> {
    RuntimeFacade::new(&app).targets()
}

#[tauri::command]
pub fn runtime_target_snapshot(
    app: AppHandle,
    link: LinkId,
) -> Result<TargetSnapshot, RuntimeError> {
    RuntimeFacade::new(&app).snapshot(link)
}

#[tauri::command]
pub fn runtime_telemetry_snapshot(
    app: AppHandle,
    target: TargetRef,
) -> Result<Option<TelemetrySnapshot>, RuntimeError> {
    RuntimeFacade::new(&app).bind(&target)?.telemetry_snapshot()
}

#[tauri::command]
pub fn runtime_parameter_observation(
    app: AppHandle,
    target: TargetRef,
    name: String,
) -> Result<Option<ParamObservation>, RuntimeError> {
    RuntimeFacade::new(&app)
        .bind(&target)?
        .parameter_observation(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(handle: usize, generation: u64) -> SourceView {
        SourceView {
            handle_id: handle,
            link_id: Some(LinkId::new(handle as u64).expect("link ID")),
            generation,
            node: Some(MavlinkTarget::new(42, 1)),
            availability: TargetAvailability::Available,
            heartbeat_age_ms: Some(10),
            parameters: true,
            roles: vec![TargetRole::Primary],
            owner: TransportOwner::Primary,
            endpoint: "isolated fixture".into(),
        }
    }

    fn target(state: &RuntimeState, views: &[SourceView]) -> TargetRef {
        state.refresh(views).expect("snapshots")[0]
            .target
            .clone()
            .expect("verified target")
    }

    #[test]
    fn same_target_is_stable_and_borrowed_role_keeps_identity() {
        let state = RuntimeState::default();
        let mut source = view(1, 1);
        let before = target(&state, &[source.clone()]);
        source.roles.push(TargetRole::MotorTest);
        assert_eq!(before, target(&state, &[source.clone()]));
        source.roles.pop();
        assert_eq!(before, target(&state, &[source]));
        assert_eq!(state.contexts.lock().len(), 1);
    }

    #[test]
    fn replacement_and_retirement_reject_old_references() {
        let state = RuntimeState::default();
        let old = target(&state, &[view(1, 1)]);
        let new = target(&state, &[view(2, 1)]);
        assert_ne!(old.vehicle_id, new.vehicle_id);
        assert_eq!(state.handle_for(&old), Err(RuntimeError::TargetChanged));
        state.refresh(&[]).expect("retire");
        assert_eq!(state.handle_for(&new), Err(RuntimeError::TargetChanged));
    }

    #[test]
    fn reopened_generation_gets_a_new_session_without_inheriting_old_ref() {
        let state = RuntimeState::default();
        let old = target(&state, &[view(1, 1)]);
        let new = target(&state, &[view(1, 2)]);
        assert_eq!(old.vehicle_id, new.vehicle_id);
        assert_eq!(old.link_id, new.link_id);
        assert_ne!(old.session_id, new.session_id);
        assert_eq!(state.handle_for(&old), Err(RuntimeError::TargetChanged));
    }

    #[test]
    fn retired_reboot_generation_is_unavailable_until_transport_reopens() {
        let state = RuntimeState::default();
        let old = target(&state, &[view(1, 1)]);
        state.retire(&old);
        let retired = state.refresh(&[view(1, 1)]).expect("retired snapshot");
        assert_eq!(retired[0].availability, TargetAvailability::Unverified);
        assert!(retired[0].target.is_none());
        let reopened = target(&state, &[view(1, 2)]);
        assert_ne!(reopened.session_id, old.session_id);
        assert_eq!(reopened.generation.get(), 2);
    }

    #[test]
    fn late_lower_generation_does_not_restore_an_old_context() {
        let state = RuntimeState::default();
        let current = target(&state, &[view(1, 2)]);
        assert_eq!(
            state.refresh(&[view(1, 1)]),
            Err(RuntimeError::TargetChanged)
        );
        assert_eq!(state.handle_for(&current), Ok(1));
        assert_eq!(target(&state, &[view(1, 2)]), current);
    }

    #[test]
    fn changed_verified_route_does_not_merge_aircraft() {
        let state = RuntimeState::default();
        let old = target(&state, &[view(1, 1)]);
        let mut changed = view(1, 1);
        changed.node = Some(MavlinkTarget::new(43, 1));
        let new = target(&state, &[changed]);
        assert_ne!(old.vehicle_id, new.vehicle_id);
        assert_ne!(old.session_id, new.session_id);
        assert_eq!(state.handle_for(&old), Err(RuntimeError::TargetChanged));
    }

    #[test]
    fn unsupported_or_unverified_source_has_no_available_reference() {
        for availability in [
            TargetAvailability::Unverified,
            TargetAvailability::Disconnected,
            TargetAvailability::Stale,
            TargetAvailability::Unsupported,
        ] {
            let state = RuntimeState::default();
            let mut source = view(1, 1);
            source.availability = availability;
            let snapshot = state.refresh(&[source]).expect("snapshot").remove(0);
            assert_eq!(snapshot.availability, availability);
            assert!(snapshot.target.is_none());
        }
    }

    #[test]
    fn dedicated_bench_has_independent_identity_and_bounded_retirement() {
        let state = RuntimeState::default();
        let primary = view(1, 1);
        let mut bench = view(2, 1);
        bench.link_id = None;
        bench.roles = vec![TargetRole::MotorTest];
        bench.owner = TransportOwner::DedicatedMotorTest;
        let snapshots = state
            .refresh(&[primary.clone(), bench.clone()])
            .expect("sources");
        assert_ne!(
            snapshots[0].target.as_ref().expect("primary").vehicle_id,
            snapshots[1].target.as_ref().expect("bench").vehicle_id
        );
        let primary_ref = snapshots[0].target.clone().expect("primary");
        let bench_ref = snapshots[1].target.clone().expect("bench");
        state.refresh(&[bench]).expect("primary retired");
        assert_eq!(
            state.handle_for(&primary_ref),
            Err(RuntimeError::TargetChanged)
        );
        assert_eq!(state.handle_for(&bench_ref), Ok(2));
        assert_eq!(state.contexts.lock().len(), 1);
        let over_capacity = [view(1, 1), view(2, 1), view(3, 1)];
        assert!(matches!(
            state.refresh(&over_capacity),
            Err(RuntimeError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn facade_startup_creates_no_target_or_transport() {
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        let runtime = RuntimeFacade::new(app.handle());
        assert!(runtime.targets().expect("snapshot").is_empty());
        assert_eq!(
            runtime.snapshot(LinkId::new(1).expect("link ID")),
            Err(RuntimeError::TargetUnavailable)
        );
    }

    #[tokio::test]
    async fn an_open_transport_and_configured_address_are_not_verified_authority() {
        use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
        let (handle, events, first) =
            maggcs_core::mavlink::connection::spawn_connection(ConnectionConfig {
                endpoint: Endpoint::UdpListener {
                    addr: "127.0.0.1:0".parse().expect("endpoint"),
                },
                target_system_id: 42,
                target_component_id: 1,
                ..Default::default()
            })
            .await
            .expect("transport");
        first.await.expect("worker result").expect("opened");
        let app = tauri::test::mock_app();
        app.manage(AppState::default());
        let state = app.state::<AppState>();
        let hub = TelemetryHub::spawn(events, 20, MavlinkTarget::new(42, 1));
        let (mission, _) = maggcs_core::MissionService::spawn(
            handle.clone(),
            maggcs_core::MissionIds {
                self_system: 255,
                self_component: 190,
                target_system: 42,
                target_component: 1,
            },
        );
        let (command, _) = maggcs_core::CommandService::spawn(handle.clone(), 42, 1);
        state.set_primary_link(crate::state::ActiveLink {
            id: LinkId::next().expect("link ID"),
            connection: handle.clone(),
            hub: hub.clone(),
            mission,
            command,
            params: None,
            logs: None,
            rtk: None,
        });
        let runtime = RuntimeFacade::new(app.handle());
        let snapshot = runtime.targets().expect("transport snapshot").remove(0);
        assert_eq!(snapshot.availability, TargetAvailability::Unverified);
        assert!(snapshot.target.is_none());
        assert!(!snapshot.parameter_service_available);
        assert!(state.motor_test.target.lock().is_none());
        assert!(!crate::motor_control_service::status(&state).enabled);
        state.take_primary_link();
        hub.shutdown();
        handle
            .shutdown_and_wait(std::time::Duration::from_secs(1))
            .await
            .expect("shutdown");
    }
}
