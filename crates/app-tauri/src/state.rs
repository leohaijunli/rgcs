//! Process-wide state: active vehicle links and the latest link status.
//!
//! Links are keyed by [`LinkId`] and stored in a map with a `primary` pointer
//! so adding a second vehicle later is an insert rather than a rewrite
//! (issues.md #21). `connect`/`disconnect` serialize on [`AppState::ops`] so
//! two concurrent commands cannot race the swap.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use maggcs_core::calib::mag::MagCalibration;
use maggcs_core::config::AppConfig;
use maggcs_core::mavlink::connection::LinkStatus;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::mission::service::MissionService;
use maggcs_core::project::ProjectStore;
use maggcs_core::safety::SafetyConfig;
use maggcs_core::telemetry::hub::TelemetryHub;
use maggcs_core::CommandService;
use parking_lot::Mutex;

use crate::alert_service::AlertState;
use crate::inspector_service::InspectorState;
use crate::instruments_service::InstrumentsState;
use crate::logs_service::{LogsRuntime, LogsState};
use crate::motor_test_state::MotorTestState;
use crate::params_service::{ParamsRuntime, ParamsState};
use crate::recorder_service::RecorderState;
use crate::rtk_service::{RtkRuntime, RtkState};
use crate::terrain_server::TerrainServer;

pub use maggcs_core::runtime::LinkId;

/// Process-wide magnetometer calibration: one loaded JSON drives the Signal
/// Inspector's mag traces (A·(m−b)) AND the motor-test alarm's total field.
/// Survives link swaps; the motor-test tap re-reads it on every attach.
#[derive(Default, Clone)]
pub struct CalibrationStore(Arc<Mutex<Option<MagCalibration>>>);

impl CalibrationStore {
    /// The currently loaded calibration, if any.
    pub fn get(&self) -> Option<MagCalibration> {
        self.0.lock().clone()
    }

    /// Replace the loaded calibration (`None` = clear).
    pub fn set(&self, cal: Option<MagCalibration>) {
        *self.0.lock() = cal;
    }
}

/// Everything bound to a single connection.
pub struct ActiveLink {
    /// Stable id, also the map key.
    pub id: LinkId,
    /// The connection worker handle.
    pub connection: ConnectionHandle,
    /// Telemetry aggregation hub for this link.
    pub hub: TelemetryHub,
    /// Mission service for this link.
    pub mission: MissionService,
    /// Command service for this link.
    pub command: CommandService,
    /// FC log protocol and forwarding task for this link.
    pub logs: Option<LogsRuntime>,
    /// FC parameter protocol and event-forwarding tasks for this link.
    pub params: Option<ParamsRuntime>,
    /// RTK injection service, stats task and observer for this link.
    pub rtk: Option<RtkRuntime>,
}

#[derive(Default)]
struct AppInner {
    links: BTreeMap<LinkId, ActiveLink>,
    primary: Option<LinkId>,
    link: Option<LinkStatus>,
}

/// Shared application state managed by Tauri.
#[derive(Default)]
pub struct AppState {
    pub serial: tokio::sync::OnceCell<Arc<crate::serial_service::SerialService>>,
    /// Lightweight target identities for the narrow Runtime facade. No I/O,
    /// protocol worker or optional tool is created by its default state.
    pub runtime: crate::runtime_facade::RuntimeState,
    /// Native-issued module/client scopes and the inert operation ledger.
    pub runtime_clients: crate::runtime_clients::RuntimeClients,
    pub(crate) missions: crate::mission_operations::MissionState,
    inner: Mutex<AppInner>,
    /// Signal Inspector tap/catalog/batcher (survives link swaps, ADR-016).
    pub inspector: InspectorState,
    /// Walk-instrument runner + window channel (W1, survives link swaps).
    pub instruments: InstrumentsState,
    /// Flight recorder for the primary link (A4): tlog + event log.
    pub recorder: RecorderState,
    /// Shared magnetometer calibration (inspector + motor-test, one file).
    pub calibration: CalibrationStore,
    /// Process-wide parameter cache; its protocol runtime belongs to
    /// `ActiveLink` and is torn down with that link.
    pub params: Arc<ParamsState>,
    /// Motor-test service handle + event forwarder (survives link swaps; the
    /// service itself stops with its link and is re-spawned on the next
    /// start).
    pub motor_test: MotorTestState,
    /// Process-wide alert center (plan §3 A3): dedup, latching and the badge
    /// count for every window.
    pub alerts: Arc<AlertState>,
    /// Process-wide RTK status snapshots; service runtime belongs to
    /// `ActiveLink` and is torn down with that link.
    pub rtk: Arc<RtkState>,
    /// Process-wide log download state/events; the protocol runtime belongs
    /// to `ActiveLink` and is torn down with that link.
    pub logs: Arc<LogsState>,
    /// Local 3D terrain tileset server (plan B) — `terrain3d_start` /
    /// `terrain3d_stop`; `None` while no tileset is served.
    pub terrain3d: Mutex<Option<TerrainServer>>,
    /// Opened DEM (drape + per-point sampling). Cached by tile path: a
    /// CogDem::open reads the full rasters, so re-opening per call would
    /// dominate the cost of an interactive edit.
    pub dem: Mutex<Option<(String, std::sync::Arc<maggcs_core::terrain::cog::CogDem>)>>,
    /// Held during connect/disconnect and window-close teardown so lifecycle
    /// operations cannot race or stop a newly swapped link (issues.md #21).
    ops: tokio::sync::Mutex<()>,
    /// Single-flight guard for the window-close teardown. Reset after a
    /// safety refusal so the operator can retry after landing/disarming.
    closing: AtomicBool,
    /// Latest native close refusal, retained until the main webview consumes
    /// it so startup/rehydration cannot lose the explanation event.
    close_refusal: Mutex<Option<String>>,
    /// Persisted operator config, loaded once at startup and updated by
    /// `config_save`. The arm gate reads safety thresholds from here (A6).
    config: Mutex<AppConfig>,
    /// The open project (one per process, advisory-locked; ADR-019, PJ1).
    pub project: Mutex<Option<ProjectStore>>,
    /// The live drape monitor (DR6): one per flight, its baseline the
    /// uploaded/compiled drape result; `None` while not monitoring.
    pub drape_monitor: Mutex<Option<crate::terrain_service::MonitorSession>>,
    /// Cross-window single source of truth (A11 phase 1): versioned UI state
    /// (layer toggles first), written through the `shared_state_*` commands
    /// and broadcast to every window.
    pub shared: maggcs_core::shared_state::SharedState,
}

impl AppState {
    /// Serialization lock for connection lifecycle commands.
    pub fn ops(&self) -> &tokio::sync::Mutex<()> {
        &self.ops
    }

    /// Claim the window-close teardown. `false` = one is already in flight.
    pub fn begin_close(&self) -> bool {
        !self.closing.swap(true, Ordering::SeqCst)
    }

    /// Allow a later close attempt after the current close was refused/failed.
    pub fn cancel_close(&self) {
        self.closing.store(false, Ordering::SeqCst);
    }

    /// Store the latest native close refusal for event delivery and recovery.
    pub fn set_close_refusal(&self, reason: String) {
        *self.close_refusal.lock() = Some(reason);
    }

    /// Take the retained refusal, if any, after a webview subscribes.
    pub fn take_close_refusal(&self) -> Option<String> {
        self.close_refusal.lock().take()
    }

    /// The current persisted config (defaults before the file is loaded).
    pub fn config(&self) -> AppConfig {
        self.config.lock().clone()
    }

    /// Safety thresholds only; used by the per-snapshot health monitor.
    pub fn safety_config(&self) -> SafetyConfig {
        self.config.lock().safety.clone()
    }

    /// Replace the cached config after a successful load or save.
    pub fn set_config(&self, config: AppConfig) {
        *self.config.lock() = config.normalized();
    }

    fn with_primary<T>(&self, f: impl FnOnce(&ActiveLink) -> T) -> Option<T> {
        let inner = self.inner.lock();
        let id = inner.primary?;
        inner.links.get(&id).map(f)
    }

    /// Insert `link`, make it primary, and return the previous primary.
    pub fn set_primary_link(&self, link: ActiveLink) -> Option<ActiveLink> {
        let mut inner = self.inner.lock();
        let old = inner.primary.and_then(|id| inner.links.remove(&id));
        inner.primary = Some(link.id);
        inner.links.insert(link.id, link);
        old
    }

    /// Command sender for the primary link's log runtime.
    pub(crate) fn primary_logs_sender(
        &self,
    ) -> Option<tokio::sync::mpsc::Sender<crate::logs_service::LogCommand>> {
        self.with_primary(|link| link.logs.as_ref().and_then(LogsRuntime::sender))
            .flatten()
    }

    /// Attach the RTK runtime to the primary link.
    pub fn set_primary_rtk(&self, rtk: RtkRuntime) {
        let mut inner = self.inner.lock();
        if let Some(id) = inner.primary {
            if let Some(link) = inner.links.get_mut(&id) {
                link.rtk = Some(rtk);
            }
        }
    }

    /// Take the primary RTK runtime for ordered lifecycle teardown.
    pub fn take_primary_rtk(&self) -> Option<RtkRuntime> {
        let mut inner = self.inner.lock();
        let id = inner.primary?;
        inner.links.get_mut(&id)?.rtk.take()
    }

    /// Parameter service handle for the primary link.
    pub(crate) fn primary_params(&self) -> Option<maggcs_core::params::service::ParamService> {
        self.with_primary(|link| self.params.service(link))
            .flatten()
    }

    /// Cached parameter values retained across a link swap.
    pub(crate) fn params_cache(&self) -> Option<maggcs_core::params::service::ParamCache> {
        self.params.cache()
    }

    /// Attach the freshly created parameter runtime to the primary link.
    pub fn set_primary_params(&self, params: ParamsRuntime) {
        let mut inner = self.inner.lock();
        if let Some(id) = inner.primary {
            if let Some(link) = inner.links.get_mut(&id) {
                link.params = Some(params);
            }
        }
    }

    /// Take the primary link's parameter runtime for ordered teardown.
    pub fn take_primary_params(&self) -> Option<ParamsRuntime> {
        let mut inner = self.inner.lock();
        let id = inner.primary?;
        inner.links.get_mut(&id)?.params.take()
    }

    /// Attach the freshly created runtime to the primary link.
    pub fn set_primary_logs(&self, logs: LogsRuntime) {
        let mut inner = self.inner.lock();
        if let Some(id) = inner.primary {
            if let Some(link) = inner.links.get_mut(&id) {
                link.logs = Some(logs);
            }
        }
    }

    /// Take the primary link's log runtime for ordered lifecycle teardown.
    pub fn take_primary_logs(&self) -> Option<LogsRuntime> {
        let mut inner = self.inner.lock();
        let id = inner.primary?;
        inner.links.get_mut(&id)?.logs.take()
    }

    /// Remove and return the primary link, if any.
    pub fn take_primary_link(&self) -> Option<ActiveLink> {
        let mut inner = self.inner.lock();
        let id = inner.primary.take()?;
        inner.links.remove(&id)
    }

    /// Number of active links (multi-vehicle readiness).
    #[cfg(test)]
    pub fn link_count(&self) -> usize {
        self.inner.lock().links.len()
    }

    /// Cloned hub of the primary link, if any.
    pub fn hub(&self) -> Option<TelemetryHub> {
        self.with_primary(|l| l.hub.clone())
    }

    /// Cloned command service of the primary link, if any.
    pub fn command(&self) -> Option<CommandService> {
        self.with_primary(|l| l.command.clone())
    }

    /// The connection handle of the primary link, if any.
    pub fn connection(&self) -> Option<ConnectionHandle> {
        self.with_primary(|l| l.connection.clone())
    }

    pub(crate) fn primary_runtime_source(&self) -> Option<crate::runtime_facade::TargetSource> {
        self.with_primary(|link| {
            crate::runtime_facade::TargetSource::primary(
                link.id,
                link.connection.clone(),
                link.hub.clone(),
                self.params.service(link),
                link.command.clone(),
                link.mission.clone(),
            )
        })
    }

    /// The process-wide alert center.
    pub fn alerts(&self) -> Arc<AlertState> {
        self.alerts.clone()
    }

    /// Latest link status published by the pump.
    pub fn link_status(&self) -> Option<LinkStatus> {
        self.inner.lock().link.clone()
    }

    /// Publish a new link status.
    pub fn publish_link(&self, status: LinkStatus) {
        self.inner.lock().link = Some(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_ids_are_unique_and_monotonic() {
        let a = LinkId::next().expect("link ID");
        let b = LinkId::next().expect("link ID");
        assert_ne!(a, b);
        assert!(a < b);
    }

    #[test]
    fn begin_close_is_one_shot() {
        // The close button can fire again while the teardown runs; only the
        // first event may start it (A1).
        let state = AppState::default();
        assert!(state.begin_close());
        assert!(!state.begin_close());
    }

    #[test]
    fn a_refused_close_can_be_retried() {
        let state = AppState::default();
        assert!(state.begin_close());
        state.cancel_close();
        assert!(state.begin_close());
    }

    #[test]
    fn close_refusal_survives_until_the_webview_consumes_it() {
        let state = AppState::default();
        state.set_close_refusal("the vehicle is airborne".to_string());
        assert_eq!(
            state.take_close_refusal().as_deref(),
            Some("the vehicle is airborne")
        );
        assert_eq!(state.take_close_refusal(), None);
    }

    #[test]
    fn link_status_round_trips() {
        let state = AppState::default();
        assert!(state.link_status().is_none());
        assert!(state.take_primary_link().is_none());
        assert_eq!(state.link_count(), 0);

        state.publish_link(LinkStatus::disconnected());
        assert!(state.link_status().is_some());
    }
}
