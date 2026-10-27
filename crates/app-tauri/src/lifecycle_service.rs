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
//! 3D view survives a link swap). Every exit reason is refused while armed or
//! airborne; window-close refusal is sent to the main webview for explanation.
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
use tauri::{AppHandle, Manager, Runtime};

use crate::state::AppState;

/// Milliseconds since `start`, saturating into the report's `u64`.
fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// The vehicle state the refusal policy needs. Freshness follows the safety
/// checklist's 3-second heartbeat threshold. Missing/stale samples stay
/// unknown rather than blocking an exit on a dead or disconnected link.
pub fn vehicle_state(state: &AppState) -> VehicleState {
    let snapshot = state.hub().map(|hub| hub.snapshot());
    let now_ms = epoch_ms();
    let armed = snapshot
        .as_ref()
        .filter(|snapshot| is_fresh(snapshot.field_ages.heartbeat_at_ms, now_ms))
        .and_then(|snapshot| snapshot.heartbeat.as_ref())
        .map(|hb| hb.base_mode.safety_armed);
    let on_ground = snapshot.as_ref().and_then(|snapshot| {
        if !is_fresh(snapshot.field_ages.extended_sys_state_at_ms, now_ms) {
            return None;
        }
        snapshot
            .extended_sys_state
            .as_ref()
            .and_then(|extended| match extended.landed_state {
                maggcs_core::telemetry::LandedState::OnGround => Some(true),
                maggcs_core::telemetry::LandedState::InAir
                | maggcs_core::telemetry::LandedState::Takeoff
                | maggcs_core::telemetry::LandedState::Landing => Some(false),
                maggcs_core::telemetry::LandedState::Unknown => None,
            })
    });
    VehicleState { armed, on_ground }
}

/// Safety checklist telemetry age threshold (3 seconds).
const SAFETY_TELEMETRY_STALE_AFTER_MS: u64 = maggcs_core::runtime::TARGET_HEARTBEAT_FRESH_MS;

fn is_fresh(at_ms: Option<u64>, now_ms: u64) -> bool {
    at_ms.is_some_and(|at| now_ms.saturating_sub(at) <= SAFETY_TELEMETRY_STALE_AFTER_MS)
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Stop the motor test **while its link is still up** — the whole point of
/// the order.
///
/// L4: wait for the `Stop` to be accepted instead of the old `try_send`, which
/// could drop the command on a momentarily full channel and leave the props
/// spinning until PX4's own 3 s timeout. Then wait for the session to leave
/// the sending state: the release frames are written in the same pass, so once
/// the state settles they are on the wire.
struct MotorTestStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for MotorTestStage<R> {
    fn name(&self) -> &'static str {
        Stage::MotorTest.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            let state = self.app.state::<AppState>();
            let had_aux = state.motor_test.capture_operations.is_active()
                || state.motor_test.log_operations.is_active();
            let motor = stop_running_motor_test(&self.app, &state, deadline).await;
            if matches!(
                motor,
                StopOutcome::Failed { .. } | StopOutcome::TimedOut { .. }
            ) {
                return motor;
            }
            if let Err(message) = crate::motor_capture_operations::stop_all(
                &self.app,
                deadline.saturating_sub(started.elapsed()),
            )
            .await
            {
                return StopOutcome::Failed { message };
            }
            if let Err(message) = crate::motor_log_operations::stop_all(
                &self.app,
                deadline.saturating_sub(started.elapsed()),
            )
            .await
            {
                return StopOutcome::Failed { message };
            }
            if matches!(motor, StopOutcome::NotRunning) && !had_aux {
                StopOutcome::NotRunning
            } else {
                StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                }
            }
        })
    }
}

/// Confirm a motor-test stop before releasing the application-owned runtime.
/// On timeout or transport failure, `on_confirmed` is not called, keeping the
/// service and its links available for an operator retry.
async fn stop_motor_test_before_cleanup<F, C>(
    stop: F,
    deadline: Duration,
    on_confirmed: C,
) -> StopOutcome
where
    F: Future<Output = Result<bool, String>>,
    C: FnOnce(),
{
    let started = Instant::now();
    let stopped_sending = match tokio::time::timeout(deadline, stop).await {
        Err(_) => {
            return StopOutcome::TimedOut {
                elapsed_ms: elapsed_ms(started),
            }
        }
        Ok(Err(error)) => {
            return StopOutcome::Failed {
                message: format!("motor-test release failed: {error}"),
            }
        }
        Ok(Ok(stopped_sending)) => stopped_sending,
    };
    on_confirmed();
    if stopped_sending {
        StopOutcome::Stopped {
            elapsed_ms: elapsed_ms(started),
        }
    } else {
        StopOutcome::NotRunning
    }
}

async fn stop_running_motor_test<R: Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    deadline: Duration,
) -> StopOutcome {
    if state.motor_test.operations.is_active() {
        let started = Instant::now();
        return match crate::motor_operations::stop_all(app, deadline).await {
            Ok(()) => {
                crate::motor_control_service::revoke(state);
                state.motor_test.running.lock().take();
                state
                    .motor_test
                    .note_state(maggcs_core::motor_test::SessionState::Idle);
                StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                }
            }
            Err(message) => StopOutcome::Failed { message },
        };
    }
    crate::motor_control_service::revoke(state);
    let service = match state.motor_test.running.lock().as_ref() {
        Some(running) => running.svc.clone(),
        None => return StopOutcome::NotRunning,
    };
    stop_motor_test_before_cleanup(
        async {
            service
                .stop_and_wait()
                .await
                .map(|receipt| receipt.released_motors > 0)
                .map_err(|error| error.to_string())
        },
        deadline,
        || {
            state.motor_test.running.lock().take();
            state
                .motor_test
                .note_state(maggcs_core::motor_test::SessionState::Idle);
        },
    )
    .await
}

/// Stop the RTK source and link-owned service/observer tasks.
struct RtkStage<R: Runtime> {
    app: AppHandle<R>,
    stop_base: bool,
}

impl<R: Runtime> ManagedService for RtkStage<R> {
    fn name(&self) -> &'static str {
        Stage::Rtk.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let started = Instant::now();
            if self.stop_base {
                if let Some(id) = state.rtk.base.snapshot().session_id {
                    if let Err(error) = state.rtk.base.stop(id).await {
                        return StopOutcome::Failed {
                            message: error.to_string(),
                        };
                    }
                }
            }
            match state.take_primary_rtk() {
                Some(runtime) => {
                    runtime
                        .stop(deadline.saturating_sub(started.elapsed()))
                        .await
                }
                None if self.stop_base => StopOutcome::Stopped {
                    elapsed_ms: elapsed_ms(started),
                },
                None => StopOutcome::NotRunning,
            }
        })
    }
}

/// Stop the FC log-download runtime owned by the primary link (O1/A2).
struct LogsStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for LogsStage<R> {
    fn name(&self) -> &'static str {
        Stage::Logs.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            let shell = crate::console_service::stop(&self.app, deadline).await;
            let state = self.app.state::<AppState>();
            let actors = state
                .logs
                .stop_operations(&self.app, deadline.saturating_sub(started.elapsed()))
                .await;
            let worker = match state.take_primary_logs() {
                Some(runtime) => {
                    runtime
                        .stop(deadline.saturating_sub(started.elapsed()))
                        .await
                }
                None => StopOutcome::NotRunning,
            };
            for outcome in [&shell, &actors, &worker] {
                if matches!(
                    outcome,
                    StopOutcome::Failed { .. } | StopOutcome::TimedOut { .. }
                ) {
                    return outcome.clone();
                }
            }
            if matches!(worker, StopOutcome::NotRunning) {
                if matches!(actors, StopOutcome::NotRunning) {
                    shell
                } else {
                    actors
                }
            } else {
                worker
            }
        })
    }
}

/// Stop the link-owned parameter protocol and event-forwarding tasks.
struct ParamsStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for ParamsStage<R> {
    fn name(&self) -> &'static str {
        Stage::Params.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            match state.take_primary_params() {
                Some(runtime) => runtime.stop(deadline).await,
                None => StopOutcome::NotRunning,
            }
        })
    }
}

/// Stop the telemetry hub, then let the mission/command handles drop with the
/// link. The hub must go before the link: its pump feeds the mission/command
/// sessions and the tlog, and closing the socket first would tear them down
/// mid-upload.
struct MissionStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for MissionStage<R> {
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
struct RecorderStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for RecorderStage<R> {
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
struct LinkStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for LinkStage<R> {
    fn name(&self) -> &'static str {
        Stage::Link.as_str()
    }

    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let started = Instant::now();
            crate::main_link_service::cleanup(&self.app, &state).await;
            let Some(link) = state.take_primary_link() else {
                return StopOutcome::NotRunning;
            };
            if let Err(error) = link.connection.shutdown_and_wait(deadline).await {
                return StopOutcome::Failed {
                    message: error.to_string(),
                };
            }
            StopOutcome::Stopped {
                elapsed_ms: elapsed_ms(started),
            }
        })
    }
}

/// Stop the local 3D tileset server (app-scoped: skipped on a disconnect).
struct TerrainStage<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> ManagedService for TerrainStage<R> {
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
fn build<R: Runtime>(app: &AppHandle<R>, reason: StopReason) -> Lifecycle {
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
    lifecycle.register(
        Stage::Rtk,
        Arc::new(RtkStage {
            app: app.clone(),
            stop_base: !reason.is_link_scope(),
        }),
    );
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
pub async fn run<R: Runtime>(
    app: &AppHandle<R>,
    reason: StopReason,
) -> Result<ShutdownReport, ShutdownError> {
    let lifecycle = build(app, reason);
    let state = app.state::<AppState>();
    let vehicle = vehicle_state(&state);
    // Use the same captured preflight state as the supervisor. A refused
    // teardown leaves automatic policy intact; an approved teardown first
    // prevents a late probe from installing a replacement link.
    if lifecycle.refusal(vehicle).is_none() {
        if let Some(serial) = state.serial.get() {
            if reason.is_link_scope() {
                let _ = serial.pause_memory(
                    maggcs_core::config::serial::AutoConnectRole::PrimaryFc,
                    None,
                );
            } else {
                serial.quiesce();
            }
        }
    }
    let report = match lifecycle.shutdown(reason, vehicle).await {
        Ok(report) => report,
        Err(err) => {
            if !reason.is_link_scope() {
                if let Some(serial) = state.serial.get() {
                    serial.resume_after_refusal();
                }
            }
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
    // U15: a detached map window never outlives the app — the main window's
    // teardown takes it down (closing the map alone only re-embeds).
    crate::map_service::close_map_on_shutdown(app);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};

    use maggcs_core::commands::CommandService;
    use maggcs_core::mavlink::connection::spawn_connection;
    use maggcs_core::mavlink::{ConnectionConfig, Endpoint};
    use maggcs_core::mission::service::{MissionIds, MissionService};
    use maggcs_core::telemetry::hub::{TelemetryHub, DEFAULT_PUSH_HZ};
    use maggcs_core::telemetry::VehicleId;
    use tauri::test::mock_app;

    use crate::rtk_service::RtkRuntime;
    use crate::state::{ActiveLink, LinkId};

    #[tokio::test]
    async fn failed_motor_test_stop_keeps_runtime_cleanup_unrun() {
        let cleaned = AtomicBool::new(false);
        let outcome = stop_motor_test_before_cleanup(
            async { Err("release write failed".to_string()) },
            Duration::from_secs(1),
            || cleaned.store(true, Ordering::SeqCst),
        )
        .await;

        assert!(matches!(outcome, StopOutcome::Failed { .. }));
        assert!(!cleaned.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn confirmed_motor_test_stop_runs_runtime_cleanup() {
        let cleaned = AtomicBool::new(false);
        let outcome =
            stop_motor_test_before_cleanup(async { Ok(true) }, Duration::from_secs(1), || {
                cleaned.store(true, Ordering::SeqCst)
            })
            .await;

        assert!(matches!(outcome, StopOutcome::Stopped { .. }));
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn timed_out_motor_test_stop_keeps_runtime_cleanup_unrun() {
        let cleaned = AtomicBool::new(false);
        let outcome = stop_motor_test_before_cleanup(
            std::future::pending::<Result<bool, String>>(),
            Duration::ZERO,
            || cleaned.store(true, Ordering::SeqCst),
        )
        .await;

        assert!(matches!(outcome, StopOutcome::TimedOut { .. }));
        assert!(!cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn vehicle_state_is_unknown_without_a_link() {
        // No link means no heartbeat: the refusal policy must see `None`, not
        // a fabricated `false` (A1/L4 — never block an exit on blind data).
        let state = AppState::default();
        assert_eq!(vehicle_state(&state), VehicleState::unknown());
        assert_eq!(SAFETY_TELEMETRY_STALE_AFTER_MS, 3_000);
    }

    #[test]
    fn stale_vehicle_samples_do_not_create_a_false_refusal() {
        let now = epoch_ms();
        assert!(!is_fresh(
            Some(now.saturating_sub(SAFETY_TELEMETRY_STALE_AFTER_MS + 1)),
            now
        ));
        assert!(is_fresh(
            Some(now.saturating_sub(SAFETY_TELEMETRY_STALE_AFTER_MS)),
            now
        ));
        assert!(!is_fresh(None, now));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn disconnect_releases_logs_params_and_rtk_with_the_primary_link() {
        let probe = UdpSocket::bind("127.0.0.1:0").expect("reserve UDP port");
        let port = probe.local_addr().expect("probe address").port();
        drop(probe);
        let config = ConnectionConfig {
            endpoint: Endpoint::UdpListener {
                addr: format!("127.0.0.1:{port}")
                    .parse()
                    .expect("listener address"),
            },
            ..Default::default()
        };
        let (handle, events, first) = spawn_connection(config).await.expect("spawn primary link");
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .expect("listener startup timed out")
            .expect("first result channel")
            .expect("listener bound");

        let app = mock_app();
        app.manage(AppState::default());
        let state = app.state::<AppState>();
        let hub = TelemetryHub::spawn(events, DEFAULT_PUSH_HZ, VehicleId::new(1, 1));
        let (mission, _mission_events) = MissionService::spawn(
            handle.clone(),
            MissionIds {
                self_system: 255,
                self_component: 190,
                target_system: 1,
                target_component: 1,
            },
        );
        let (command, _command_events) = CommandService::spawn(handle.clone(), 1, 1);
        state.set_primary_link(ActiveLink {
            id: LinkId::next().expect("link ID"),
            connection: handle.clone(),
            hub,
            mission,
            command,
            logs: None,
            params: None,
            rtk: None,
        });

        let logs = state.logs.spawn(app.handle().clone(), handle.clone());
        state.set_primary_logs(logs);
        let params = state.params.spawn(app.handle(), handle.clone());
        state.set_primary_params(params);
        let rtk = RtkRuntime::spawn(Arc::clone(&state.rtk), app.handle().clone(), handle.clone());
        state.set_primary_rtk(rtk);

        let report = run(app.handle(), StopReason::Disconnect)
            .await
            .expect("disconnect is allowed without a heartbeat");

        for stage in [Stage::Rtk, Stage::Logs, Stage::Params, Stage::Link] {
            assert!(
                matches!(report.outcome(stage), Some(StopOutcome::Stopped { .. })),
                "{stage:?} did not stop cleanly: {:?}",
                report.outcome(stage)
            );
        }
        assert!(report.is_clean());
        assert_eq!(state.link_count(), 0);
        assert!(state.take_primary_logs().is_none());
        assert!(state.take_primary_params().is_none());
        assert!(state.take_primary_rtk().is_none());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match UdpSocket::bind(("127.0.0.1", port)) {
                Ok(socket) => {
                    drop(socket);
                    break;
                }
                Err(error) => assert!(
                    Instant::now() < deadline,
                    "link socket was not released: {error}"
                ),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
