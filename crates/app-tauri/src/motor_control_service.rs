//! Explicit, non-persisted permission to control Motor Test outputs (MR-22a).
//! The default is monitoring only. The short gate lock linearizes OFF with
//! motor-start enqueue and tone dispatch; releases remain available after OFF.

use std::fmt;

use maggcs_core::mavlink::LinkState;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::State;

use crate::motor_test_state::MotorTestTargetRef;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MotorControlStatus {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MotorControlError {
    Disabled,
    NoTarget,
    TargetChanged,
    Busy,
    ReleaseFailed { message: String },
}

impl fmt::Display for MotorControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "Motor control is OFF — enable it before starting a test"),
            Self::NoTarget => write!(f, "connect the PX4 target before enabling motor control"),
            Self::TargetChanged => write!(
                f,
                "motor-control target changed — enable control again for the current link"
            ),
            Self::Busy => write!(
                f,
                "finish or reset the current motor operation before enabling control"
            ),
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
        inner.revision = inner.revision.wrapping_add(1);
    }

    fn enable(&self, binding: Binding, expected_revision: u64) -> Result<(), MotorControlError> {
        let mut inner = self.inner.lock();
        if inner.revision != expected_revision {
            return Err(MotorControlError::Busy);
        }
        inner.binding = Some(binding);
        inner.revision = inner.revision.wrapping_add(1);
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
    let binding = state.motor_test.control.inner.lock().binding;
    MotorControlStatus {
        enabled: binding.is_some() && binding == current,
    }
}

pub fn ensure_enabled(state: &AppState) -> Result<(), MotorControlError> {
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
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<MotorControlStatus, String> {
    set_control(&state, enabled)
        .await
        .map_err(|error| error.to_string())
}
