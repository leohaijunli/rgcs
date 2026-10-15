//! App adapter for the core teardown supervisor (plan §3 A1).
//!
//! Every path that stops the link or the app — Settings ▸ Disconnect, the
//! in-app Shut down, and the main window's close button (X, Alt-F4, logout) —
//! runs through [`run`], so the stop order is one property instead of three
//! ad-hoc cleanups:
//!
//! ```text
//! MotorTest → Rtk → Logs → Params → MissionAndCommand → Recorder → Link → Terrain
//! ```
//!
//! The registry is rebuilt on every teardown from what is *currently*
//! attached, so a second teardown can never touch a stale handle.
//!
//! `disconnect` is link-scoped: the app-scoped terrain server stays up (the
//! 3D view survives a link swap). The window close button *forces* the
//! shutdown (the operator's escape hatch), so an armed vehicle can never trap
//! them in the app; every other reason is refused while armed or airborne
//! (L4 adds the typed confirmation on top).
//!
//! Pure policy and ordering live in `maggcs_core::lifecycle`; this module only
//! maps the live process state onto that trait.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use maggcs_core::alerts::{AlertInput, AlertSource, Severity};
use maggcs_core::lifecycle::{
    Lifecycle, LifecyclePolicy, ManagedService, ShutdownError, ShutdownReport, Stage, StopOutcome,
    StopReason, VehicleState,
};
use maggcs_core::motor_test::service::MotorTestCommand;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// How often the motor-test stage re-checks the session state while it waits
/// for the release pass to land.
const SESSION_POLL: Duration = Duration::from_millis(20);

/// Milliseconds since `start`, saturating into the report's `u64`.
fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// The vehicle state the refusal policy needs: `armed` from the primary
/// link's heartbeat. `on_ground` stays unknown until L6 decodes
/// `EXTENDED_SYS_STATE` — the supervisor only refuses on a known `false`, so
/// a missing landing state can never block a legitimate exit.
pub fn vehicle_state(state: &AppState) -> VehicleState {
    let armed = state
        .hub()
        .and_then(|hub| hub.snapshot().heartbeat)
        .map(|hb| hb.base_mode.safety_armed);
    VehicleState {
        armed,
        on_ground: None,
    }
}

/// Stop the motor test **while its link is still up** — the whole point of
/// the order.
///
/// L4: wait for the `Stop` to be accepted instead of the old `try_send`, which
/// could drop the command on a momentarily full channel and leave the props
/// spinning until PX4's own 3 s timeout. Then wait for the session to leave
/// the sending state: the release frames are written in the same pass, so once
/// the state settles they are on the wire.
struct MotorTestStage {
    app: AppHandle,
}

async fn wait_not_sending(state: &AppState) {
    loop {
        if !state.motor_test.session().is_some_and(|s| s.is_sending()) {
            return;
        }
        tokio::time::sleep(SESSION_POLL).await;
    }
}

impl ManagedService for MotorTestStage {
    fn name(&self) -> &'static str {
        Stage::MotorTest.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let started = Instant::now();
            let Some(running) = state.motor_test.running.lock().take() else {
                return StopOutcome::NotRunning;
            };
            let was_sending = state.motor_test.session().is_some_and(|s| s.is_sending());
            match tokio::time::timeout(deadline, running.svc.send(MotorTestCommand::Stop)).await {
                Err(_) => {
                    return StopOutcome::TimedOut {
                        elapsed_ms: elapsed_ms(started),
                    }
                }
                // The command channel is closed: the session already ended.
                Ok(Err(_)) => {}
                Ok(Ok(())) => {}
            }
            if !was_sending {
                return StopOutcome::NotRunning;
            }
            let remaining = deadline.saturating_sub(started.elapsed());
            if tokio::time::timeout(remaining, wait_not_sending(&state))
                .await
                .is_err()
            {
                return StopOutcome::TimedOut {
                    elapsed_ms: elapsed_ms(started),
                };
            }
            StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            }
        })
    }
}

/// Drop the RTK injection service + its foreign observer (M1).
struct RtkStage {
    app: AppHandle,
}

impl ManagedService for RtkStage {
    fn name(&self) -> &'static str {
        Stage::Rtk.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            if state.rtk.detach() {
                StopOutcome::Stopped { elapsed_ms: 0 }
            } else {
                StopOutcome::NotRunning
            }
        })
    }
}

/// Drop the FC log-download task (O1).
struct LogsStage {
    app: AppHandle,
}

impl ManagedService for LogsStage {
    fn name(&self) -> &'static str {
        Stage::Logs.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            if state.logs.detach() {
                StopOutcome::Stopped { elapsed_ms: 0 }
            } else {
                StopOutcome::NotRunning
            }
        })
    }
}

/// Stop the parameter protocol task + its event forwarder.
struct ParamsStage {
    app: AppHandle,
}

impl ManagedService for ParamsStage {
    fn name(&self) -> &'static str {
        Stage::Params.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            if state.params.release() {
                StopOutcome::Stopped { elapsed_ms: 0 }
            } else {
                StopOutcome::NotRunning
            }
        })
    }
}

/// Stop the telemetry hub, then let the mission/command handles drop with the
/// link. The hub must go before the link: its pump feeds the mission/command
/// sessions and the tlog, and closing the socket first would tear them down
/// mid-upload.
struct MissionStage {
    app: AppHandle,
}

impl ManagedService for MissionStage {
    fn name(&self) -> &'static str {
        Stage::MissionAndCommand.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            match state.hub() {
                Some(hub) => {
                    hub.shutdown();
                    StopOutcome::Stopped { elapsed_ms: 0 }
                }
                None => StopOutcome::NotRunning,
            }
        })
    }
}

/// Flush and close the flight recorder (A4). It sits after the services that
/// feed it (mission/command, params, logs, RTK) so nothing is appended after
/// the flush, and before the link so its tap still has the last frames.
struct RecorderStage {
    app: AppHandle,
}

impl ManagedService for RecorderStage {
    fn name(&self) -> &'static str {
        Stage::Recorder.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            state.recorder.stop(deadline).await
        })
    }
}

/// Close the primary connection. Taking the link also drops the mission and
/// command services, whose tasks end when their last handle goes.
struct LinkStage {
    app: AppHandle,
}

impl ManagedService for LinkStage {
    fn name(&self) -> &'static str {
        Stage::Link.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let started = Instant::now();
            let Some(link) = state.take_primary_link() else {
                return StopOutcome::NotRunning;
            };
            link.connection.shutdown().await;
            StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            }
        })
    }
}

/// Stop the local 3D tileset server (app-scoped: skipped on a disconnect).
struct TerrainStage {
    app: AppHandle,
}

impl ManagedService for TerrainStage {
    fn name(&self) -> &'static str {
        Stage::Terrain.as_str()
    }

    fn stop<'a>(
        &'a self,
        _deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let Some(server) = state.terrain3d.lock().take() else {
                return StopOutcome::NotRunning;
            };
            let started = Instant::now();
            // `stop()` joins the accept/serve threads: never block the async
            // runtime on it.
            match tokio::task::spawn_blocking(move || server.stop()).await {
                Ok(()) => StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                },
                Err(e) => StopOutcome::Failed {
                    message: format!("terrain server join failed: {e}"),
                },
            }
        })
    }
}

/// The policy every teardown runs under. One accessor so the guard the UI
/// asks about (L4) and the supervisor that enforces it cannot drift apart.
pub fn policy() -> LifecyclePolicy {
    LifecyclePolicy::default()
}

/// Build a fresh registry from what is attached right now. Called once per
/// teardown: a handle taken here is never reused by a later teardown.
fn build(app: &AppHandle) -> Lifecycle {
    let lifecycle = Lifecycle::new(policy());
    let state = app.state::<AppState>();
    // Only registered while a session exists: an idle motor-test window owns
    // no link state, and its scope flusher belongs to the window, not to us.
    if state.motor_test.running.lock().is_some() {
        lifecycle.register(
            Stage::MotorTest,
            Arc::new(MotorTestStage { app: app.clone() }),
        );
    }
    lifecycle.register(Stage::Rtk, Arc::new(RtkStage { app: app.clone() }));
    lifecycle.register(Stage::Logs, Arc::new(LogsStage { app: app.clone() }));
    lifecycle.register(Stage::Params, Arc::new(ParamsStage { app: app.clone() }));
    lifecycle.register(
        Stage::MissionAndCommand,
        Arc::new(MissionStage { app: app.clone() }),
    );
    lifecycle.register(
        Stage::Recorder,
        Arc::new(RecorderStage { app: app.clone() }),
    );
    lifecycle.register(Stage::Link, Arc::new(LinkStage { app: app.clone() }));
    lifecycle.register(Stage::Terrain, Arc::new(TerrainStage { app: app.clone() }));
    lifecycle
}

/// Run one teardown for `reason`.
///
/// A refusal (armed / airborne) raises a `warn` alert so the reason is
/// visible even when a call site swallows the returned error.
pub async fn run(app: &AppHandle, reason: StopReason) -> Result<ShutdownReport, ShutdownError> {
    let lifecycle = build(app);
    let state = app.state::<AppState>();
    let vehicle = vehicle_state(&state);
    // The window chrome forces (see `StopReason::forces`): an armed or airborne
    // exit still happens, but it must not happen silently — record what it
    // walked past, so the log and the alert history say so (L4).
    if reason.forces() {
        if let Some(refused) = lifecycle.refusal(vehicle) {
            state.alerts().raise(AlertInput::new(
                AlertSource::System,
                Severity::Warn,
                format!("forced shutdown ({}): {refused}", reason.as_str()),
            ));
        }
    }
    let report = match lifecycle.shutdown(reason, vehicle).await {
        Ok(report) => report,
        Err(err) => {
            state.alerts().raise(AlertInput::new(
                AlertSource::System,
                Severity::Warn,
                format!("{} refused: {err}", reason.as_str()),
            ));
            return Err(err);
        }
    };
    for (name, outcome) in report.incomplete() {
        state.alerts().raise(AlertInput::new(
            AlertSource::System,
            Severity::Warn,
            format!("shutdown stage {name}: {outcome:?}"),
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vehicle_state_is_unknown_without_a_link() {
        // No link means no heartbeat: the refusal policy must see `None`, not
        // a fabricated `false` (A1/L4 — never block an exit on blind data).
        let state = AppState::default();
        assert_eq!(vehicle_state(&state), VehicleState::unknown());
    }
}
