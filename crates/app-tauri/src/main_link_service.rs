//! Explicit Motor Test borrowing of a primary serial link (MR-22b).
//! The primary runtime owns the transport and ParamService. Borrowing owns
//! only Motor Test tasks and expires at the first transport/target boundary.

use std::time::Duration;

use maggcs_core::mavlink::{ConnectionEvent, Endpoint, LinkState, MessageRoute};
use maggcs_core::telemetry::Autopilot;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::motor_test_state::{
    AuxiliaryLinkRuntime, MainSerialTarget, MotorTestTargetRef, TargetLinkOrigin,
    TARGET_COMPONENT_ID,
};
use crate::state::AppState;

const HEARTBEAT_FRESH_MS: u64 = maggcs_core::runtime::TARGET_HEARTBEAT_FRESH_MS;
const LIFECYCLE_POLL: Duration = Duration::from_millis(100);
type Attachment =
    std::sync::Arc<parking_lot::Mutex<Option<crate::motor_test_state::OutputConfigSnapshot>>>;

pub fn candidate(state: &AppState) -> Option<MainSerialTarget> {
    let handle = state.connection()?;
    let config = handle.config();
    if !matches!(config.endpoint, Endpoint::Serial { .. }) || handle.generation() == 0 {
        return None;
    }
    let link = state.link_status()?;
    if link.link_state != LinkState::Connected || !link.fc_alive {
        return None;
    }
    let snapshot = state.hub()?.snapshot();
    let heartbeat = snapshot.heartbeat?;
    let (system_id, component_id) = handle
        .target_identity()
        .map(|id| (id.system_id, id.component_id))
        .unwrap_or((config.target_system_id, config.target_component_id));
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let at = snapshot.field_ages.heartbeat_at_ms?;
    if now_ms.saturating_sub(u128::from(at)) > u128::from(HEARTBEAT_FRESH_MS)
        || heartbeat.autopilot != Autopilot::Px4
        || heartbeat.system_id == 0
        || heartbeat.component_id != TARGET_COMPONENT_ID
        || heartbeat.system_id != system_id
        || heartbeat.component_id != component_id
    {
        return None;
    }
    // An incompatible/missing primary parameter runtime cannot be duplicated
    // on the same link. Retain the dedicated-target workflow instead.
    state.primary_params()?;
    Some(MainSerialTarget {
        target: MotorTestTargetRef {
            connection_id: handle.handle_id(),
            target_system_id: heartbeat.system_id,
            target_component_id: heartbeat.component_id,
        },
        transport_generation: handle.generation(),
        endpoint: config.endpoint.to_address_string(),
    })
}

pub(crate) fn ensure_port_not_primary(state: &AppState, endpoint: &Endpoint) -> Result<(), String> {
    let Some(handle) = state.connection() else {
        return Ok(());
    };
    if let (Endpoint::Serial { port, .. }, Endpoint::Serial { port: main, .. }) =
        (endpoint, &handle.config().endpoint)
    {
        let same = port == main
            || std::fs::canonicalize(port)
                .ok()
                .zip(std::fs::canonicalize(main).ok())
                .is_some_and(|(a, b)| a == b);
        if same {
            return Err(
                "this serial port is already the main connection; choose Use main serial link"
                    .into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn is_attached(state: &AppState) -> bool {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .is_some_and(|runtime| runtime.origin() == TargetLinkOrigin::MainSerial)
}

/// The primary shell/log tools and Motor Test must not run independent
/// request/response protocols against the same FC at the same time.
pub(crate) fn ensure_primary_tools_available(state: &AppState) -> Result<(), String> {
    if is_attached(state) {
        Err("Motor Test owns shell/log operations on the main serial link; use its Capture/Logs controls or detach its target".into())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub async fn actuator_use_main_target<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    expected: MainSerialTarget,
) -> Result<(), String> {
    let _ops = state.ops().lock().await;
    // Finish an in-flight primary shell exchange before claiming its protocol.
    let console = app.try_state::<std::sync::Arc<crate::console_service::ConsoleState>>();
    let _shell = match console.as_ref() {
        Some(console) => Some(console.lock_shell().await),
        None => None,
    };
    if console.as_ref().is_some_and(|console| console.is_active()) {
        return Err(
            "close the primary MAVLink console before borrowing its shell for Motor Test".into(),
        );
    }
    if candidate(&state).as_ref() != Some(&expected) {
        return Err(
            "main serial target changed or is unavailable; refresh the target offer".into(),
        );
    }
    let handle = state
        .connection()
        .ok_or("main serial link is unavailable")?;
    if handle.state().await != LinkState::Connected {
        return Err("main serial link is disconnected".into());
    }
    {
        let target = state.motor_test.target.lock();
        if let Some(target) = target.as_ref() {
            if target.origin() == TargetLinkOrigin::MainSerial
                && target.target.target_ref() == expected.target
                && target.generation_is_current()
            {
                return Ok(());
            }
            return Err(
                "detach the current Motor Test target before using the main serial link".into(),
            );
        }
    }
    if state.logs.is_busy() {
        return Err(
            "finish or cancel the main-window log transfer before attaching Motor Test".into(),
        );
    }
    let params = state
        .primary_params()
        .ok_or("main parameter service is unavailable")?;
    crate::motor_control_service::revoke(&state);
    crate::actuator_service::stop_active_session(&state).await?;
    state.motor_test.running.lock().take();
    let events = handle.subscribe_route(MessageRoute::messages(&[0]));
    let runtime = AuxiliaryLinkRuntime::borrowed(&handle);
    crate::actuator_service::install_target(
        &app,
        &state,
        handle.clone(),
        expected.endpoint.clone(),
        expected.target.target_system_id,
        params,
        runtime,
    );
    let attachment = state
        .motor_test
        .target
        .lock()
        .as_ref()
        .map(|runtime| runtime.target.config.clone())
        .ok_or("Motor Test attachment ended")?;
    let watcher_app = app.clone();
    let watcher = tauri::async_runtime::spawn(async move {
        watch_attachment(watcher_app, handle, expected, attachment, events).await;
    });
    if let Some(target) = state.motor_test.target.lock().as_mut() {
        target.runtime.add_task(watcher);
    } else {
        watcher.abort();
    }
    Ok(())
}

async fn watch_attachment<R: Runtime>(
    app: AppHandle<R>,
    handle: maggcs_core::mavlink::ConnectionHandle,
    expected: MainSerialTarget,
    attachment: Attachment,
    mut events: maggcs_core::mavlink::router::RoutedEvents,
) {
    let mut tick = tokio::time::interval(LIFECYCLE_POLL);
    loop {
        let lost = tokio::select! {
            event = events.recv() => match event {
                Ok(ConnectionEvent::Connected { generation, .. }) => generation != expected.transport_generation,
                Ok(ConnectionEvent::LinkError(_)) | Ok(ConnectionEvent::Failed(_))
                    | Ok(ConnectionEvent::HeartbeatLost { .. }) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => true,
                _ => false,
            },
            _ = tick.tick() => false,
        };
        let state = app.state::<AppState>();
        let replaced = state
            .connection()
            .is_none_or(|current| current.handle_id() != expected.target.connection_id);
        if lost
            || replaced
            || handle.generation() != expected.transport_generation
            || handle.state().await != LinkState::Connected
        {
            if !matches_attachment(&state, &expected, &attachment) {
                return;
            }
            crate::motor_control_service::revoke(&state);
            // Cleanup must not abort/join its own watcher task.
            let cleanup_app = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = cleanup_app.state::<AppState>();
                let _ops = state.ops().lock().await;
                if !matches_attachment(&state, &expected, &attachment) {
                    return;
                }
                if let Err(error) = crate::actuator_service::stop_active_session(&state).await {
                    let _ = cleanup_app.emit("actuator_stop_warning", &error);
                    // Keep the failed session for its explicit STOP diagnostics;
                    // the invalid attachment and tool tasks still expire.
                } else {
                    state.motor_test.running.lock().take();
                }
                cleanup(&cleanup_app, &state).await;
            });
            return;
        }
    }
}

fn matches_attachment(
    state: &AppState,
    expected: &MainSerialTarget,
    attachment: &Attachment,
) -> bool {
    state
        .motor_test
        .target
        .lock()
        .as_ref()
        .is_some_and(|runtime| {
            runtime.origin() == TargetLinkOrigin::MainSerial
                && runtime.target.target_ref() == expected.target
                && std::sync::Arc::ptr_eq(&runtime.target.config, attachment)
        })
}

/// Caller holds AppState::ops and has already confirmed motor release, or is
/// handling a failed transport whose release result was explicitly reported.
pub(crate) async fn cleanup<R: Runtime>(app: &AppHandle<R>, state: &AppState) {
    if !is_attached(state) {
        return;
    }
    crate::motor_control_service::revoke(state);
    crate::stream_service::stop(state);
    crate::run_service::interrupt_active(app, state, "main serial attachment ended");
    crate::actuator_service::stop_actuator_logs(state).await;
    crate::capture_service::stop_actuator_capture(state).await;
    crate::actuator_service::detach_motor_telemetry(state).await;
    let old = state.motor_test.target.lock().take();
    if let Some(old) = old {
        old.stop().await;
    }
}

/// Preserve an existing main connection if its tool-owned releases fail.
pub(crate) async fn before_primary_replace<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
) -> Result<(), String> {
    if is_attached(state) {
        crate::motor_control_service::revoke(state);
        crate::actuator_service::stop_active_session(state).await?;
        state.motor_test.running.lock().take();
        cleanup(app, state).await;
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "main_link_service_tests.rs"]
pub(crate) mod tests;
