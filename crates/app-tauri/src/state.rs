//! Process-wide state: the active connection, its telemetry hub, and the
//! latest link status broadcast to the UI.

use maggcs_core::mavlink::connection::LinkStatus;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::telemetry::hub::TelemetryHub;
use parking_lot::Mutex;

use crate::mission_service::MissionService;

#[derive(Default)]
struct AppInner {
    connection: Option<ConnectionHandle>,
    hub: Option<TelemetryHub>,
    mission: Option<MissionService>,
    link: Option<LinkStatus>,
}

/// Shared application state managed by Tauri.
#[derive(Default)]
pub struct AppState {
    inner: Mutex<AppInner>,
}

impl AppState {
    /// Store the active connection handle.
    pub fn set_connection(&self, handle: ConnectionHandle) {
        self.inner.lock().connection = Some(handle);
    }

    /// Take the active connection (used by `disconnect`).
    pub fn take_connection(&self) -> Option<ConnectionHandle> {
        self.inner.lock().connection.take()
    }

    /// Clone of the active connection handle, if any.
    pub fn connection(&self) -> Option<ConnectionHandle> {
        self.inner.lock().connection.clone()
    }

    /// Store the telemetry hub (replaces any previous one).
    pub fn set_hub(&self, hub: TelemetryHub) {
        self.inner.lock().hub = Some(hub);
    }

    /// Clone of the current hub, if any.
    pub fn hub(&self) -> Option<TelemetryHub> {
        self.inner.lock().hub.clone()
    }

    /// Take and shut down the current hub.
    pub fn take_hub(&self) -> Option<TelemetryHub> {
        let hub = self.inner.lock().hub.take();
        if let Some(h) = &hub {
            h.shutdown();
        }
        hub
    }

    /// Store the mission service (replaces any previous one).
    pub fn set_mission(&self, service: MissionService) {
        self.inner.lock().mission = Some(service);
    }

    /// Take the mission service.
    pub fn take_mission(&self) -> Option<MissionService> {
        self.inner.lock().mission.take()
    }

    /// Clone of the current mission service, if any.
    pub fn mission(&self) -> Option<MissionService> {
        self.inner.lock().mission.clone()
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
