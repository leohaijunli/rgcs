//! Explicit, non-persisted permission to control Motor Test outputs (MR-22a).
//! The default is monitoring only. The short gate lock linearizes OFF with
//! motor-start enqueue and tone dispatch; releases remain available after OFF.

use std::fmt;

#[cfg(test)]
use maggcs_core::mavlink::LinkState;
use maggcs_core::motor_test::operation_types::{
    MotorOperationError, MotorPermissionRef, MotorPermissionRequest, MotorPermissionStatus,
    MotorProtocolError,
};
use maggcs_core::runtime::operations::{OperationError, RuntimeClientSession};
use maggcs_core::runtime::Revision;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use crate::motor_test_state::MotorTestTargetRef;
use crate::state::AppState;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MotorControlStatus {
    pub enabled: bool,
    pub permission: Option<MotorPermissionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MotorControlError {
    Disabled,
    #[cfg(test)]
    NoTarget,
    TargetChanged,
    #[cfg(test)]
    Busy,
    #[cfg(test)]
    ReleaseFailed {
        message: String,
    },
}

impl fmt::Display for MotorControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "Motor control is OFF — enable it before starting a test"),
            #[cfg(test)]
            Self::NoTarget => write!(f, "connect the PX4 target before enabling motor control"),
            Self::TargetChanged => write!(
                f,
                "motor-control target changed — enable control again for the current link"
            ),
            #[cfg(test)]
            Self::Busy => write!(
                f,
                "finish or reset the current motor operation before enabling control"
            ),
            #[cfg(test)]
            Self::ReleaseFailed { message } => write!(
                f,
                "Motor control is OFF, but release is incomplete; retry STOP: {message}"
            ),
        }
    }
}
impl std::error::Error for MotorControlError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Binding {
    target: MotorTestTargetRef,
    transport_generation: u64,
}

#[derive(Default)]
struct Permission {
    binding: Option<Binding>,
    revision: u64,
    owned: Option<MotorPermissionRef>,
}

#[derive(Default)]
pub struct MotorControlGate {
    inner: Mutex<Permission>,
}

impl MotorControlGate {
    fn revision(&self) -> u64 {
        self.inner.lock().revision
    }

    pub fn revoke(&self) {
        let mut inner = self.inner.lock();
        inner.binding = None;
        inner.owned = None;
        inner.revision = inner.revision.saturating_add(1);
    }

    #[cfg(test)]
    fn enable(&self, binding: Binding, expected_revision: u64) -> Result<(), MotorControlError> {
        let mut inner = self.inner.lock();
        if inner.revision != expected_revision {
            return Err(MotorControlError::Busy);
        }
        inner.binding = Some(binding);
        inner.revision = inner
            .revision
            .checked_add(1)
            .ok_or(MotorControlError::Busy)?;
        Ok(())
    }
}

fn current_binding(state: &AppState) -> Option<Binding> {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .filter(|runtime| runtime.generation_is_current())
        .map(|runtime| Binding {
            target: runtime.target.target_ref(),
            transport_generation: runtime.target.handle.generation(),
        })
}

pub fn status(state: &AppState) -> MotorControlStatus {
    let current = current_binding(state);
    let inner = state.motor_test.control.inner.lock();
    let binding = inner.binding;
    let live = inner
        .owned
        .as_ref()
        .is_none_or(|permission| state.runtime_clients.owner_is_attached(&permission.owner));
    MotorControlStatus {
        enabled: binding.is_some() && binding == current && live,
        permission: if live && binding == current {
            inner.owned.clone()
        } else {
            None
        },
    }
}

pub fn ensure_enabled(state: &AppState) -> Result<(), MotorControlError> {
    if !status(state).enabled {
        return Err(MotorControlError::Disabled);
    }
    let current = current_binding(state);
    match state.motor_test.control.inner.lock().binding {
        None => Err(MotorControlError::Disabled),
        Some(binding) if Some(binding) == current => Ok(()),
        _ => Err(MotorControlError::TargetChanged),
    }
}

pub fn with_enabled<T>(
    state: &AppState,
    send: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !status(state).enabled {
        return Err(MotorControlError::Disabled.to_string());
    }
    let current = current_binding(state);
    let permission = state.motor_test.control.inner.lock();
    match permission.binding {
        None => Err(MotorControlError::Disabled.to_string()),
        Some(binding) if Some(binding) == current => send(),
        _ => Err(MotorControlError::TargetChanged.to_string()),
    }
}

/// Revoke synchronously, before any operation lock or release wait.
pub fn revoke(state: &AppState) {
    state.motor_test.control.revoke();
    state.motor_test.tones.cancel();
}

#[cfg(test)]
pub(crate) async fn set_control(
    state: &AppState,
    enabled: bool,
) -> Result<MotorControlStatus, MotorControlError> {
    if !enabled {
        revoke(state);
        crate::actuator_service::stop_active_session(state)
            .await
            .map_err(|message| MotorControlError::ReleaseFailed { message })?;
        return Ok(status(state));
    }
    let revision = state.motor_test.control.revision();
    let _ops = state.ops().lock().await;
    crate::actuator_service::ensure_idle_session(state).map_err(|_| MotorControlError::Busy)?;
    let handle = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.handle.clone())
        .ok_or(MotorControlError::NoTarget)?;
    if handle.state().await != LinkState::Connected {
        return Err(MotorControlError::NoTarget);
    }
    let binding = current_binding(state).ok_or(MotorControlError::NoTarget)?;
    state.motor_test.control.enable(binding, revision)?;
    state.motor_test.tones.cancel();
    if !status(state).enabled {
        revoke(state);
        return Err(MotorControlError::TargetChanged);
    }
    Ok(status(state))
}

#[tauri::command]
pub async fn actuator_set_motor_control(
    app: AppHandle,
    window: WebviewWindow,
    request: MotorPermissionRequest,
) -> Result<MotorPermissionStatus, MotorOperationError> {
    set_owned(&app, window.label(), request).await
}

pub(crate) fn ensure_owned(
    state: &AppState,
    session: &RuntimeClientSession,
    permission: &MotorPermissionRef,
) -> Result<(), MotorOperationError> {
    if permission.owner != session.owner || permission.target != session.target {
        return Err(OperationError::NotOwner.into());
    }
    if !status(state).enabled {
        return Err(MotorProtocolError::Disabled.into());
    }
    if state.motor_test.control.inner.lock().owned.as_ref() != Some(permission) {
        return Err(MotorProtocolError::PermissionChanged.into());
    }
    Ok(())
}

pub(crate) fn revoke_owned(state: &AppState, permission: &MotorPermissionRef) {
    let mut inner = state.motor_test.control.inner.lock();
    if inner.owned.as_ref() == Some(permission) {
        inner.binding = None;
        inner.owned = None;
        inner.revision = inner.revision.saturating_add(1);
    }
}

/// Retire a closed window's grant only. A late callback from an old Motor
/// window cannot revoke the grant issued to its replacement.
pub(crate) fn revoke_client(state: &AppState, client_id: &maggcs_core::runtime::ClientId) {
    let mut inner = state.motor_test.control.inner.lock();
    if inner
        .owned
        .as_ref()
        .is_some_and(|permission| &permission.owner.client_id == client_id)
    {
        inner.binding = None;
        inner.owned = None;
        inner.revision = inner.revision.saturating_add(1);
    }
}

pub(crate) async fn set_owned<R: Runtime>(
    app: &AppHandle<R>,
    window: &str,
    request: MotorPermissionRequest,
) -> Result<MotorPermissionStatus, MotorOperationError> {
    let state = app.state::<AppState>();
    state
        .runtime_clients
        .authorize(window, &request.context.session)?;
    crate::motor_operations::binding(app, &request.context.session)?;
    if !request.enabled {
        let owned = state.motor_test.control.inner.lock().owned.clone();
        match (request.permission, owned) {
            (None, None) => {}
            (Some(permission), Some(current)) => {
                if permission.owner != request.context.session.owner
                    || permission.target != request.context.session.target
                    || current != permission
                {
                    return Err(OperationError::NotOwner.into());
                }
                revoke_owned(&state, &permission);
                state.motor_test.tones.cancel();
                crate::motor_operations::cancel_permission(app, &permission)?;
            }
            _ => {
                return Err(OperationError::NotOwner.into());
            }
        }
        return Ok(MotorPermissionStatus { permission: None });
    }
    let expected = state.motor_test.control.revision();
    let _ops = state.ops().lock().await;
    state
        .runtime_clients
        .authorize(window, &request.context.session)?;
    crate::motor_operations::binding(app, &request.context.session)?;
    crate::actuator_service::ensure_idle_session(&state)
        .map_err(|message| MotorProtocolError::InvalidRequest { message })?;
    let binding = current_binding(&state).ok_or(MotorProtocolError::Disabled)?;
    let mut inner = state.motor_test.control.inner.lock();
    if inner.revision != expected {
        return Err(MotorProtocolError::PermissionChanged.into());
    }
    if inner.owned.as_ref().is_some_and(|permission| {
        permission.owner != request.context.session.owner
            && state.runtime_clients.owner_is_attached(&permission.owner)
    }) {
        return Err(OperationError::NotOwner.into());
    }
    let revision = inner
        .revision
        .checked_add(1)
        .ok_or(OperationError::CapacityExceeded)?;
    let permission = MotorPermissionRef {
        owner: request.context.session.owner,
        target: request.context.session.target,
        revision: Revision::new(revision).map_err(|error| OperationError::Runtime { error })?,
    };
    inner.binding = Some(binding);
    inner.revision = revision;
    inner.owned = Some(permission.clone());
    Ok(MotorPermissionStatus {
        permission: Some(permission),
    })
}
