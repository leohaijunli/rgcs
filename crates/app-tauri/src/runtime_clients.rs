//! AF-02b foundation: native-issued module/client identity and captured expiry.
//! Window labels identify the actual IPC caller; opaque owner IDs alone do not
//! authenticate a client. This layer never opens I/O or restores motor authority.

use std::collections::BTreeMap;

use maggcs_core::runtime::operations::{
    OperationCoordinator, OperationError, OperationOwner, RuntimeClientSession,
};
use maggcs_core::runtime::{ClientId, ModuleInstanceId, TargetRef};
use parking_lot::Mutex;
use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};

use crate::runtime_facade::{RuntimeFacade, TargetRuntime};
use crate::state::AppState;

const MAX_WINDOW_CLIENTS: usize = 64;
const MAX_MODULE_INSTANCES: usize = 128;
const MAX_MODULE_ID_BYTES: usize = 64;

struct WindowClient {
    id: ClientId,
    modules: BTreeMap<ModuleInstanceId, RuntimeClientSession>,
}

#[derive(Default)]
pub struct RuntimeClients {
    windows: Mutex<BTreeMap<String, WindowClient>>,
    pub(crate) operations: OperationCoordinator,
}

impl RuntimeClients {
    pub(crate) fn owner_is_attached(&self, owner: &OperationOwner) -> bool {
        self.windows.lock().values().any(|window| {
            window.id == owner.client_id && window.modules.contains_key(&owner.instance_id)
        })
    }
    pub(crate) fn attach(
        &self,
        window: &str,
        module_id: String,
        target: TargetRef,
    ) -> Result<(RuntimeClientSession, bool), OperationError> {
        if module_id.is_empty()
            || module_id.len() > MAX_MODULE_ID_BYTES
            || !module_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        {
            return Err(OperationError::InvalidRequest {
                message: "invalid module descriptor ID".into(),
            });
        }
        target
            .validate()
            .map_err(|error| OperationError::Runtime { error })?;
        // Random issuance is outside the short registry/expiry lock.
        let client_id = ClientId::fresh().map_err(|error| OperationError::Runtime { error })?;
        let instance_id =
            ModuleInstanceId::fresh().map_err(|error| OperationError::Runtime { error })?;
        let mut windows = self.windows.lock();
        if let Some(existing) = windows.get(window).and_then(|client| {
            client
                .modules
                .values()
                .find(|session| session.module_id == module_id && session.target == target)
        }) {
            return Ok((existing.clone(), false));
        }
        if windows
            .values()
            .map(|client| client.modules.len())
            .sum::<usize>()
            >= MAX_MODULE_INSTANCES
            || (!windows.contains_key(window) && windows.len() >= MAX_WINDOW_CLIENTS)
        {
            return Err(OperationError::CapacityExceeded);
        }
        let first_window = !windows.contains_key(window);
        let client = windows
            .entry(window.into())
            .or_insert_with(|| WindowClient {
                id: client_id,
                modules: BTreeMap::new(),
            });
        let session = RuntimeClientSession {
            owner: OperationOwner {
                client_id: client.id.clone(),
                instance_id,
            },
            target,
            module_id,
        };
        client
            .modules
            .insert(session.owner.instance_id.clone(), session.clone());
        Ok((session, first_window))
    }

    pub(crate) fn authorize(
        &self,
        window: &str,
        session: &RuntimeClientSession,
    ) -> Result<(), OperationError> {
        let windows = self.windows.lock();
        let client = windows.get(window).ok_or(OperationError::NotOwner)?;
        if client.id != session.owner.client_id {
            return Err(OperationError::NotOwner);
        }
        let current = client
            .modules
            .get(&session.owner.instance_id)
            .ok_or(OperationError::NotOwner)?;
        if current != session {
            return Err(OperationError::ContextMismatch);
        }
        Ok(())
    }

    pub(crate) fn detach(
        &self,
        window: &str,
        session: &RuntimeClientSession,
    ) -> Result<(), OperationError> {
        // Validate/remove atomically, then signal owned work outside this lock.
        {
            let mut windows = self.windows.lock();
            let client = windows.get_mut(window).ok_or(OperationError::NotOwner)?;
            if client.id != session.owner.client_id {
                return Err(OperationError::NotOwner);
            }
            if client.modules.get(&session.owner.instance_id) != Some(session) {
                return Err(OperationError::ContextMismatch);
            }
            client.modules.remove(&session.owner.instance_id);
            // Keep the window's client ID until actual native destruction so a
            // new module does not accidentally inherit a stale close callback.
        }
        self.operations.cancel_owner(&session.owner)?;
        Ok(())
    }

    pub(crate) fn expire_window(&self, client_id: &ClientId) -> Result<(), OperationError> {
        let owners = {
            let mut windows = self.windows.lock();
            let label = windows
                .iter()
                .find(|(_, client)| &client.id == client_id)
                .map(|(label, _)| label.clone());
            let Some(label) = label else { return Ok(()) };
            let Some(client) = windows.remove(&label) else {
                return Ok(());
            };
            client
                .modules
                .into_values()
                .map(|session| session.owner)
                .collect::<Vec<_>>()
        };
        for owner in owners {
            self.operations.cancel_owner(&owner)?;
        }
        Ok(())
    }
}

#[tauri::command]
pub fn runtime_client_attach(
    app: AppHandle,
    window: WebviewWindow,
    module_id: String,
    target: TargetRef,
) -> Result<RuntimeClientSession, OperationError> {
    RuntimeFacade::new(&app)
        .bind(&target)
        .map_err(|error| OperationError::Runtime { error })?;
    let state = app.state::<AppState>();
    let (session, first_window) =
        state
            .runtime_clients
            .attach(window.label(), module_id, target)?;
    if first_window {
        let client_id = session.owner.client_id.clone();
        let cleanup_app = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                let state = cleanup_app.state::<AppState>();
                if let Err(error) = state.runtime_clients.expire_window(&client_id) {
                    state.alerts().raise(maggcs_core::alerts::AlertInput::new(
                        maggcs_core::alerts::AlertSource::System,
                        maggcs_core::alerts::Severity::Warn,
                        format!("Runtime client cleanup is incomplete: {error}"),
                    ));
                }
            }
        });
    }
    // Registration can race a target replacement or native window destruction.
    // Never return a late attachment as authority for a different/current target.
    state.runtime_clients.authorize(window.label(), &session)?;
    if let Err(error) = RuntimeFacade::new(&app).bind(&session.target) {
        let _ = state.runtime_clients.detach(window.label(), &session);
        return Err(OperationError::Runtime { error });
    }
    Ok(session)
}

#[tauri::command]
pub fn runtime_client_detach(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
) -> Result<(), OperationError> {
    let state = app.state::<AppState>();
    state.runtime_clients.authorize(window.label(), &session)?;
    state.runtime_clients.detach(window.label(), &session)
}

#[tauri::command]
pub fn runtime_client_validate(
    app: AppHandle,
    window: WebviewWindow,
    session: RuntimeClientSession,
) -> Result<(), OperationError> {
    app.state::<AppState>()
        .runtime_clients
        .authorize(window.label(), &session)?;
    RuntimeFacade::new(&app)
        .bind(&session.target)
        .map_err(|error| OperationError::Runtime { error })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::runtime::operations::{OperationRequest, OperationResource};
    use maggcs_core::runtime::{
        LinkId, MavlinkTarget, TransportGeneration, VehicleId, VehicleSessionId,
    };

    fn target() -> TargetRef {
        TargetRef {
            vehicle_id: VehicleId::new("vehicle-a").expect("vehicle"),
            session_id: VehicleSessionId::new("target-session-a").expect("session"),
            link_id: LinkId::new(1).expect("link"),
            generation: TransportGeneration::new(1).expect("generation"),
            mavlink_target: MavlinkTarget::new(42, 1),
        }
    }

    #[test]
    fn a_window_cannot_use_another_windows_owner_identity() {
        let clients = RuntimeClients::default();
        let (a, _) = clients
            .attach("window-a", "parameters".into(), target())
            .expect("a");
        let (b, _) = clients
            .attach("window-b", "parameters".into(), target())
            .expect("b");
        assert_eq!(
            clients.authorize("window-a", &b),
            Err(OperationError::NotOwner)
        );
        assert!(clients.authorize("window-a", &a).is_ok());
        assert_eq!(
            clients.detach("window-a", &b),
            Err(OperationError::NotOwner)
        );
        assert!(clients.authorize("window-b", &b).is_ok());
    }

    #[test]
    fn same_window_module_target_attachment_is_idempotent() {
        let clients = RuntimeClients::default();
        let (first, created) = clients
            .attach("main", "parameters".into(), target())
            .expect("first");
        assert!(created);
        for _ in 0..10 {
            let (again, created) = clients
                .attach("main", "parameters".into(), target())
                .expect("again");
            assert_eq!(again, first);
            assert!(!created);
        }
        assert_eq!(
            clients
                .windows
                .lock()
                .get("main")
                .expect("client")
                .modules
                .len(),
            1
        );
    }

    #[test]
    fn a_captured_native_close_cannot_expire_a_reopened_window() {
        let clients = RuntimeClients::default();
        let (old, _) = clients
            .attach("params", "parameters".into(), target())
            .expect("old");
        clients.expire_window(&old.owner.client_id).expect("close");
        let (new, _) = clients
            .attach("params", "parameters".into(), target())
            .expect("new");
        assert_ne!(old.owner, new.owner);
        clients
            .expire_window(&old.owner.client_id)
            .expect("late close");
        assert!(clients.authorize("params", &new).is_ok());
        assert_eq!(
            clients.authorize("params", &old),
            Err(OperationError::NotOwner)
        );
    }

    #[test]
    fn client_target_context_cannot_be_reassigned_by_a_caller() {
        let clients = RuntimeClients::default();
        let (mut session, _) = clients
            .attach("params", "parameters".into(), target())
            .expect("attach");
        session.target.generation = TransportGeneration::new(2).expect("generation");
        assert_eq!(
            clients.authorize("params", &session),
            Err(OperationError::ContextMismatch)
        );
    }

    #[test]
    fn detach_signals_only_its_operations_without_touching_other_owners() {
        let clients = RuntimeClients::default();
        let (a, _) = clients
            .attach("a", "parameters".into(), target())
            .expect("a");
        let (b, _) = clients
            .attach("b", "inspector".into(), target())
            .expect("b");
        let request = |session: &RuntimeClientSession, resource| OperationRequest {
            request_id: maggcs_core::runtime::RequestId::fresh().expect("request"),
            owner: session.owner.clone(),
            target: session.target.clone(),
            resources: vec![resource],
            budget_ms: 5000,
        };
        let a_work = clients
            .operations
            .prepare(request(&a, OperationResource::ParameterWrite))
            .expect("a work");
        let b_work = clients
            .operations
            .prepare(request(&b, OperationResource::Shell))
            .expect("b work");
        a_work.mark_running().expect("running a");
        b_work.mark_running().expect("running b");
        clients.detach("a", &a).expect("detach a");
        assert!(a_work.cancellation().is_cancelled());
        assert!(!b_work.cancellation().is_cancelled());
        assert!(b_work.check().is_ok());
    }
}
