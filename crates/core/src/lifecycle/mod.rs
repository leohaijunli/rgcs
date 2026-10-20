//! Teardown supervisor (plan §3 A1): one ordered, deadline-bounded shutdown
//! for every path that stops the app or the link.
//!
//! Before this module three paths tore the app down differently — `disconnect`
//! stopped RTK/logs/inspector, `shutdown_app` only closed the link, and the
//! sub-windows each cleaned up whatever they owned, while closing the main
//! window did not run any of it. The order is a safety property, not a
//! preference:
//!
//! ```text
//! MotorTest → Rtk → Logs → Params → MissionAndCommand → Recorder → Link → Terrain
//! ```
//!
//! - props stop (and the release frames are on the wire) **while the link is
//!   still up**, so a Stop can still reach the flight controller;
//! - the link-scoped services release their handles next;
//! - the recorder flushes after the services that feed it (A4);
//! - only then does the connection close, so a reconnect cannot race a
//!   half-closed socket;
//! - the terrain server is app-scoped: a reconnect keeps the 3D view.
//!
//! Pure logic: the caller supplies the vehicle state and the services. The
//! clock is only read to enforce deadlines, so ordering and refusal are
//! unit-testable with fake services.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Per-stage budget. A stage that ignores it is cut off and the shutdown
/// continues — a hung service must not hang the exit.
pub const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_secs(2);

/// Whole-shutdown budget for best-effort stages. Mandatory stages
/// ([`Stage::is_mandatory`]) still run once it is gone.
pub const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// One stage of the teardown, in the order they are stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Motor test (own link): stop the session, release the actuators.
    MotorTest,
    /// RTK correction injection.
    Rtk,
    /// FC log download.
    Logs,
    /// FC parameter protocol.
    Params,
    /// Mission upload/download + command sessions (drop = stop).
    MissionAndCommand,
    /// Flight recorder: flush tlog + event log (A4).
    Recorder,
    /// The MAVLink connection itself.
    Link,
    /// Local 3D tileset server (app-scoped).
    Terrain,
}

impl Stage {
    /// Every stage, in stop order.
    pub const ALL: [Stage; 8] = [
        Stage::MotorTest,
        Stage::Rtk,
        Stage::Logs,
        Stage::Params,
        Stage::MissionAndCommand,
        Stage::Recorder,
        Stage::Link,
        Stage::Terrain,
    ];

    /// Stable name for logs and the shutdown report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MotorTest => "motor_test",
            Self::Rtk => "rtk",
            Self::Logs => "logs",
            Self::Params => "params",
            Self::MissionAndCommand => "mission_command",
            Self::Recorder => "recorder",
            Self::Link => "link",
            Self::Terrain => "terrain",
        }
    }

    /// Stopped on `disconnect`/reconnect as well as on exit. The terrain
    /// server is not link-scoped: stopping it would blank the 3D view for a
    /// reconnect that is about to succeed.
    pub fn is_link_scoped(self) -> bool {
        !matches!(self, Self::Terrain)
    }

    /// Never skipped by the total budget: props must stop and the port must be
    /// released, otherwise a reconnect fails while the vehicle may still be
    /// turning.
    pub fn is_mandatory(self) -> bool {
        matches!(self, Self::MotorTest | Self::Link)
    }
}

/// Why the app or the link is being torn down.
///
/// Exported to the frontend (L4): the confirm dialog asks the shell what a
/// teardown would do (`teardown_guard`) instead of re-implementing the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The operator disconnected.
    Disconnect,
    /// In-app exit (Settings ▸ Quit, `shutdown_app`).
    ShutdownApp,
    /// The main window's close button / Alt-F4 / session end.
    WindowClose,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disconnect => "disconnect",
            Self::ShutdownApp => "shutdown_app",
            Self::WindowClose => "window_close",
        }
    }

    /// Link-scoped reasons leave app-scoped services (the terrain server)
    /// alone. An in-place *link swap* (`connect` replacing a link) is not a
    /// teardown — it stops the old link's services only once the new link is
    /// up, so it deliberately does not come through here.
    pub fn is_link_scope(self) -> bool {
        matches!(self, Self::Disconnect)
    }

    /// The word the operator types to confirm this teardown once the shell has
    /// said it is allowed but lossy (L4). Uppercase: the dialog compares it
    /// verbatim so a stray Enter cannot confirm.
    pub fn confirm_keyword(self) -> &'static str {
        match self {
            Self::Disconnect => "DISCONNECT",
            Self::ShutdownApp | Self::WindowClose => "QUIT",
        }
    }
}

/// What one stage did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopOutcome {
    /// The service stopped inside its budget.
    Stopped { elapsed_ms: u64 },
    /// Registered, but there was nothing to stop.
    NotRunning,
    /// No service is registered for this stage yet (the stage is reserved).
    NotRegistered,
    /// The stage exceeded its budget; the shutdown moved on.
    TimedOut { elapsed_ms: u64 },
    /// The service reported a failure; the shutdown moved on.
    Failed { message: String },
    /// The total budget was gone before this stage started (best-effort only).
    Skipped,
}

impl StopOutcome {
    /// Whether this stage is worth mentioning to the operator.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::TimedOut { .. } | Self::Failed { .. })
    }
}

/// One stoppable part of the app.
///
/// `dyn` cannot carry a native `async fn`, so the future is boxed; the
/// supervisor additionally wraps every call in `tokio::time::timeout`, so an
/// implementation that ignores `deadline` cannot hang the shutdown.
pub trait ManagedService: Send + Sync {
    /// Stable name for the report (`motor_test`, `rtk`, …).
    fn name(&self) -> &'static str;
    /// Stop, given this stage's budget.
    fn stop<'a>(
        &'a self,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>>;
}

/// What the refusal gate looks at. `None` means "cannot tell" (no link, stale
/// telemetry) and never refuses: a GCS that will not quit on a dead link is
/// worse than the risk it guards against. L3/L6 tighten freshness later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VehicleState {
    /// `HEARTBEAT.base_mode & MAV_MODE_FLAG_SAFETY_ARMED`.
    pub armed: Option<bool>,
    /// `EXTENDED_SYS_STATE.landed_state` — `None` when unknown or stale.
    pub on_ground: Option<bool>,
}

impl VehicleState {
    /// No link / nothing decoded yet.
    pub fn unknown() -> Self {
        Self {
            armed: None,
            on_ground: None,
        }
    }
}

/// Tunables (A6 moves them into the app config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecyclePolicy {
    /// Refuse a teardown while the vehicle is armed.
    pub refuse_when_armed: bool,
    /// Refuse a teardown while the vehicle is airborne.
    pub refuse_when_flying: bool,
    pub stage_timeout: Duration,
    pub total_timeout: Duration,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            refuse_when_armed: true,
            refuse_when_flying: true,
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
        }
    }
}

impl LifecyclePolicy {
    /// Whether this policy would refuse a teardown right now, and why.
    ///
    /// Pure and registry-free on purpose: the UI asks for this *before* the
    /// operator commits (L4's confirm bar), and the app-level guard command
    /// answers from the same code the supervisor enforces.
    pub fn refusal(&self, vehicle: VehicleState) -> Option<String> {
        if self.refuse_when_armed && vehicle.armed == Some(true) {
            return Some("the vehicle is armed".to_string());
        }
        if self.refuse_when_flying && vehicle.on_ground == Some(false) {
            return Some("the vehicle is airborne".to_string());
        }
        None
    }
}

/// What one teardown would do right now, answered *before* the operator
/// commits (L4).
///
/// The UI renders this instead of re-deriving the policy: the refusal and
/// word to type come from the same code the supervisor enforces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TeardownGuard {
    pub reason: StopReason,
    /// Why a teardown would be refused; `None` when it would proceed.
    pub refusal: Option<String>,
    /// The word the operator types to confirm an allowed-but-lossy teardown;
    /// `None` when a plain confirm is enough.
    pub keyword: Option<String>,
    /// Always false: no teardown reason bypasses armed/airborne protection.
    /// Kept in the serialized contract for compatibility with existing clients.
    pub forced: bool,
    pub armed: Option<bool>,
    pub on_ground: Option<bool>,
}

impl TeardownGuard {
    /// `at_stake` is the caller's own state — local work that would be lost
    /// (an unuploaded mission), which core cannot see. Whether that warrants a
    /// keyword is a rule, and stays here.
    pub fn of(
        policy: LifecyclePolicy,
        reason: StopReason,
        vehicle: VehicleState,
        at_stake: bool,
    ) -> Self {
        let refusal = policy.refusal(vehicle);
        Self {
            reason,
            keyword: if refusal.is_none() && at_stake {
                Some(reason.confirm_keyword().to_string())
            } else {
                None
            },
            refusal,
            forced: false,
            armed: vehicle.armed,
            on_ground: vehicle.on_ground,
        }
    }
}

/// Why a teardown did not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShutdownError {
    /// The vehicle state forbids it.
    #[error("refused: {reason}")]
    Refused { reason: String },
    /// Another shutdown is in flight (single-flight).
    #[error("a shutdown is already in progress")]
    AlreadyRunning,
    /// The motor-test release was not confirmed, so the link must stay open.
    #[error("motor-test safety stop failed at {stage:?}: {outcome:?}; link kept open")]
    SafetyStopFailed { stage: Stage, outcome: StopOutcome },
}

/// What happened, stage by stage, in stop order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    entries: Vec<(Stage, &'static str, StopOutcome)>,
}

impl ShutdownReport {
    fn push(&mut self, stage: Stage, name: &'static str, outcome: StopOutcome) {
        self.entries.push((stage, name, outcome));
    }

    /// One entry per attempted stage, in stop order.
    pub fn entries(&self) -> &[(Stage, &'static str, StopOutcome)] {
        &self.entries
    }

    /// The outcome for `stage`; `None` = the stage was out of scope for this
    /// reason (link scope skips the app-scoped stages).
    pub fn outcome(&self, stage: Stage) -> Option<&StopOutcome> {
        self.entries
            .iter()
            .find(|(s, _, _)| *s == stage)
            .map(|(_, _, o)| o)
    }

    /// Stages that timed out or failed — what L4 shows the operator.
    pub fn incomplete(&self) -> Vec<(&'static str, &StopOutcome)> {
        self.entries
            .iter()
            .filter(|(_, _, outcome)| outcome.needs_attention())
            .map(|(_, name, outcome)| (*name, outcome))
            .collect()
    }

    /// Every attempted stage stopped cleanly (or had nothing to do).
    pub fn is_clean(&self) -> bool {
        self.incomplete().is_empty()
    }
}

/// The supervisor: a registry of stages plus the refusal policy.
pub struct Lifecycle {
    policy: LifecyclePolicy,
    services: Mutex<BTreeMap<Stage, Arc<dyn ManagedService>>>,
    /// Single-flight: two windows closing at once (or a close racing an
    /// in-app quit) must not run the teardown twice.
    running: tokio::sync::Mutex<()>,
}

impl Default for Lifecycle {
    /// The conservative policy ([`LifecyclePolicy::default`]) and no stages.
    fn default() -> Self {
        Self::new(LifecyclePolicy::default())
    }
}

impl Lifecycle {
    pub fn new(policy: LifecyclePolicy) -> Self {
        Self {
            policy,
            services: Mutex::new(BTreeMap::new()),
            running: tokio::sync::Mutex::new(()),
        }
    }

    pub fn policy(&self) -> LifecyclePolicy {
        self.policy
    }

    /// Register (or replace) the service for a stage. Stages are stopped in
    /// [`Stage::ALL`] order whatever the registration order.
    pub fn register(&self, stage: Stage, service: Arc<dyn ManagedService>) {
        self.services
            .lock()
            .expect("lifecycle registry lock poisoned")
            .insert(stage, service);
    }

    /// Whether a service is registered for this stage.
    pub fn registered(&self, stage: Stage) -> bool {
        self.services
            .lock()
            .expect("lifecycle registry lock poisoned")
            .contains_key(&stage)
    }

    fn service(&self, stage: Stage) -> Option<Arc<dyn ManagedService>> {
        self.services
            .lock()
            .expect("lifecycle registry lock poisoned")
            .get(&stage)
            .cloned()
    }

    /// Whether a teardown would be refused right now. Pure, so the UI can ask
    /// before the operator commits (L4's confirm bar).
    pub fn refusal(&self, vehicle: VehicleState) -> Option<String> {
        self.policy.refusal(vehicle)
    }

    /// Stop every stage in scope, in order, each inside its own budget.
    ///
    /// Every reason refuses while the vehicle is armed or airborne.
    pub async fn shutdown(
        &self,
        reason: StopReason,
        vehicle: VehicleState,
    ) -> Result<ShutdownReport, ShutdownError> {
        if let Some(reason) = self.refusal(vehicle) {
            return Err(ShutdownError::Refused { reason });
        }

        let _flight = self
            .running
            .try_lock()
            .map_err(|_| ShutdownError::AlreadyRunning)?;

        let started = Instant::now();
        let mut report = ShutdownReport::default();
        for stage in Stage::ALL {
            if reason.is_link_scope() && !stage.is_link_scoped() {
                continue;
            }
            let Some(service) = self.service(stage) else {
                report.push(stage, "—", StopOutcome::NotRegistered);
                continue;
            };

            let remaining = self.policy.total_timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() && !stage.is_mandatory() {
                report.push(stage, service.name(), StopOutcome::Skipped);
                continue;
            }

            // Mandatory stages keep a full budget once the total is spent;
            // best-effort stages get what is left.
            let budget = if remaining.is_zero() {
                self.policy.stage_timeout
            } else {
                self.policy.stage_timeout.min(remaining)
            };

            let elapsed = Instant::now();
            let outcome = match tokio::time::timeout(budget, service.stop(budget)).await {
                Ok(outcome) => outcome,
                Err(_) => StopOutcome::TimedOut {
                    elapsed_ms: millis(elapsed.elapsed()),
                },
            };
            report.push(stage, service.name(), outcome.clone());
            if stage == Stage::MotorTest && outcome.needs_attention() {
                return Err(ShutdownError::SafetyStopFailed { stage, outcome });
            }
        }
        Ok(report)
    }
}

fn millis(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Configurable fake service: records that it ran, and how it behaves.
    #[derive(Clone)]
    enum Behaviour {
        Instant,
        Delay(Duration),
        /// Never returns (an implementation that ignores its deadline).
        Hang,
        Fail(String),
    }

    struct Fake {
        name: &'static str,
        behaviour: Behaviour,
        log: Arc<StdMutex<Vec<&'static str>>>,
    }

    impl ManagedService for Fake {
        fn name(&self) -> &'static str {
            self.name
        }

        fn stop<'a>(
            &'a self,
            _deadline: Duration,
        ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
            let name = self.name;
            let behaviour = self.behaviour.clone();
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().expect("test log").push(name);
                match behaviour {
                    Behaviour::Instant => StopOutcome::Stopped { elapsed_ms: 0 },
                    Behaviour::Delay(d) => {
                        tokio::time::sleep(d).await;
                        StopOutcome::Stopped {
                            elapsed_ms: millis(d),
                        }
                    }
                    Behaviour::Hang => {
                        std::future::pending::<()>().await;
                        unreachable!("pending never resolves")
                    }
                    Behaviour::Fail(message) => StopOutcome::Failed { message },
                }
            })
        }
    }

    fn fake(
        stage: Stage,
        name: &'static str,
        behaviour: Behaviour,
        log: &Arc<StdMutex<Vec<&'static str>>>,
        lifecycle: &Lifecycle,
    ) {
        lifecycle.register(
            stage,
            Arc::new(Fake {
                name,
                behaviour,
                log: log.clone(),
            }),
        );
    }

    fn log() -> Arc<StdMutex<Vec<&'static str>>> {
        Arc::new(StdMutex::new(Vec::new()))
    }

    #[tokio::test]
    async fn stages_run_in_the_fixed_order() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        // Registered out of order: the stop order comes from Stage::ALL.
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);
        fake(
            Stage::MotorTest,
            "motor",
            Behaviour::Instant,
            &log,
            &lifecycle,
        );
        fake(Stage::Rtk, "rtk", Behaviour::Instant, &log, &lifecycle);
        fake(
            Stage::Terrain,
            "terrain",
            Behaviour::Instant,
            &log,
            &lifecycle,
        );

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");

        assert_eq!(
            log.lock().expect("test log").as_slice(),
            ["motor", "rtk", "link", "terrain"]
        );
        assert!(report.is_clean());
        assert_eq!(
            report.outcome(Stage::Recorder),
            Some(&StopOutcome::NotRegistered)
        );
    }

    #[tokio::test]
    async fn a_stage_that_ignores_its_deadline_is_cut_off_and_the_rest_still_run() {
        let lifecycle = Lifecycle::new(LifecyclePolicy {
            stage_timeout: Duration::from_millis(50),
            ..LifecyclePolicy::default()
        });
        let log = log();
        fake(Stage::Rtk, "hung", Behaviour::Hang, &log, &lifecycle);
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");

        assert!(matches!(
            report.outcome(Stage::Rtk),
            Some(StopOutcome::TimedOut { .. })
        ));
        assert_eq!(
            report.outcome(Stage::Link),
            Some(&StopOutcome::Stopped { elapsed_ms: 0 })
        );
        assert_eq!(log.lock().expect("test log").as_slice(), ["hung", "link"]);
        assert!(!report.is_clean());
    }

    #[tokio::test]
    async fn a_failed_motor_test_stop_aborts_before_closing_the_link() {
        for behaviour in [
            Behaviour::Fail("release write failed".to_string()),
            Behaviour::Hang,
        ] {
            let lifecycle = Lifecycle::new(LifecyclePolicy {
                stage_timeout: Duration::from_millis(20),
                ..LifecyclePolicy::default()
            });
            let log = log();
            fake(Stage::MotorTest, "motor", behaviour, &log, &lifecycle);
            fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

            let error = lifecycle
                .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
                .await
                .expect_err("uncertain motor-test release must abort teardown");

            assert!(matches!(
                error,
                ShutdownError::SafetyStopFailed {
                    stage: Stage::MotorTest,
                    outcome: StopOutcome::Failed { .. } | StopOutcome::TimedOut { .. },
                }
            ));
            assert_eq!(
                log.lock().expect("test log").as_slice(),
                ["motor"],
                "link close must not run after release failure"
            );
        }
    }

    #[tokio::test]
    async fn a_failed_stage_is_reported_and_the_shutdown_continues() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        fake(
            Stage::Logs,
            "logs",
            Behaviour::Fail("link gone".to_string()),
            &log,
            &lifecycle,
        );
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");

        assert_eq!(
            report.outcome(Stage::Logs),
            Some(&StopOutcome::Failed {
                message: "link gone".to_string()
            })
        );
        assert_eq!(report.incomplete().len(), 1);
        assert_eq!(report.incomplete()[0].0, "logs");
        assert_eq!(log.lock().expect("test log").as_slice(), ["logs", "link"]);
    }

    #[tokio::test]
    async fn a_clean_run_registers_not_running_services() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();

        struct Idle;
        impl ManagedService for Idle {
            fn name(&self) -> &'static str {
                "idle"
            }
            fn stop<'a>(
                &'a self,
                _deadline: Duration,
            ) -> Pin<Box<dyn Future<Output = StopOutcome> + Send + 'a>> {
                Box::pin(async { StopOutcome::NotRunning })
            }
        }
        lifecycle.register(Stage::Rtk, Arc::new(Idle));
        let _ = &log;

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");
        assert_eq!(report.outcome(Stage::Rtk), Some(&StopOutcome::NotRunning));
    }

    #[tokio::test]
    async fn armed_vehicles_refuse_disconnect_and_shutdown() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        let armed = VehicleState {
            armed: Some(true),
            on_ground: Some(true),
        };
        assert_eq!(
            lifecycle.refusal(armed).as_deref(),
            Some("the vehicle is armed")
        );

        for reason in [
            StopReason::Disconnect,
            StopReason::ShutdownApp,
            StopReason::WindowClose,
        ] {
            let err = lifecycle
                .shutdown(reason, armed)
                .await
                .expect_err("all teardown paths must be refused while armed");
            assert!(matches!(err, ShutdownError::Refused { .. }));
        }
        assert!(
            log.lock().expect("test log").is_empty(),
            "nothing may stop while refused"
        );
    }

    #[tokio::test]
    async fn airborne_vehicles_refuse_the_teardown() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let airborne = VehicleState {
            armed: Some(true),
            on_ground: Some(false),
        };
        assert_eq!(
            lifecycle.refusal(airborne).as_deref(),
            Some("the vehicle is armed"),
            "armed is the more specific reason"
        );

        // Disarmed but airborne (a landing vehicle): still refused.
        let landing = VehicleState {
            armed: Some(false),
            on_ground: Some(false),
        };
        assert_eq!(
            lifecycle.refusal(landing).as_deref(),
            Some("the vehicle is airborne")
        );

        for reason in [
            StopReason::Disconnect,
            StopReason::ShutdownApp,
            StopReason::WindowClose,
        ] {
            let err = lifecycle
                .shutdown(reason, landing)
                .await
                .expect_err("all exit paths must refuse while airborne");
            assert!(matches!(err, ShutdownError::Refused { .. }));
        }
    }

    #[tokio::test]
    async fn unknown_vehicle_state_never_refuses() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        assert!(lifecycle.refusal(VehicleState::unknown()).is_none());
        assert!(lifecycle
            .refusal(VehicleState {
                armed: None,
                on_ground: Some(true)
            })
            .is_none());
        assert!(lifecycle
            .refusal(VehicleState {
                armed: Some(false),
                on_ground: Some(true)
            })
            .is_none());
    }

    #[tokio::test]
    async fn policy_can_allow_a_teardown_while_armed() {
        let lifecycle = Lifecycle::new(LifecyclePolicy {
            refuse_when_armed: false,
            refuse_when_flying: false,
            ..LifecyclePolicy::default()
        });
        let log = log();
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        lifecycle
            .shutdown(
                StopReason::Disconnect,
                VehicleState {
                    armed: Some(true),
                    on_ground: Some(false),
                },
            )
            .await
            .expect("policy allows it");
        assert_eq!(log.lock().expect("test log").as_slice(), ["link"]);
    }

    #[tokio::test]
    async fn window_close_refuses_armed_and_airborne_vehicles_but_allows_safe_exit() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);
        for vehicle in [
            VehicleState {
                armed: Some(true),
                on_ground: Some(true),
            },
            VehicleState {
                armed: Some(false),
                on_ground: Some(false),
            },
        ] {
            let err = lifecycle
                .shutdown(StopReason::WindowClose, vehicle)
                .await
                .expect_err("window close must respect the vehicle safety gate");
            assert!(matches!(err, ShutdownError::Refused { .. }));
        }
        assert!(
            log.lock().expect("test log").is_empty(),
            "refused closes stop no services"
        );

        lifecycle
            .shutdown(
                StopReason::WindowClose,
                VehicleState {
                    armed: Some(false),
                    on_ground: Some(true),
                },
            )
            .await
            .expect("a safe vehicle may exit");
        assert_eq!(log.lock().expect("test log").as_slice(), ["link"]);
    }

    #[tokio::test]
    async fn link_scope_skips_app_scoped_stages() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        fake(
            Stage::Params,
            "params",
            Behaviour::Instant,
            &log,
            &lifecycle,
        );
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);
        fake(
            Stage::Terrain,
            "terrain",
            Behaviour::Instant,
            &log,
            &lifecycle,
        );

        let report = lifecycle
            .shutdown(StopReason::Disconnect, VehicleState::unknown())
            .await
            .expect("not refused");
        assert_eq!(log.lock().expect("test log").as_slice(), ["params", "link"]);
        assert_eq!(
            report.outcome(Stage::Terrain),
            None,
            "the 3D server survives a disconnect"
        );

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");
        assert_eq!(
            report.outcome(Stage::Terrain),
            Some(&StopOutcome::Stopped { elapsed_ms: 0 })
        );
    }

    #[tokio::test]
    async fn a_second_shutdown_is_refused_while_one_runs() {
        let lifecycle = Arc::new(Lifecycle::new(LifecyclePolicy::default()));
        let log = log();
        fake(
            Stage::Rtk,
            "slow",
            Behaviour::Delay(Duration::from_millis(200)),
            &log,
            &lifecycle,
        );

        let first = {
            let lifecycle = lifecycle.clone();
            tokio::spawn(async move {
                lifecycle
                    .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
                    .await
            })
        };
        // Let the first shutdown take the flight lock.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let second = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await;
        assert_eq!(
            second.expect_err("single-flight"),
            ShutdownError::AlreadyRunning
        );

        first
            .await
            .expect("task joined")
            .expect("first shutdown finished");
    }

    #[tokio::test]
    async fn a_second_shutdown_runs_once_the_first_is_done() {
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        let log = log();
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        lifecycle
            .shutdown(StopReason::Disconnect, VehicleState::unknown())
            .await
            .expect("first");
        lifecycle
            .shutdown(StopReason::Disconnect, VehicleState::unknown())
            .await
            .expect("second");
        assert_eq!(
            log.lock().expect("test log").as_slice(),
            ["link", "link"],
            "both runs happened"
        );
    }

    #[tokio::test]
    async fn the_total_budget_skips_best_effort_stages_but_never_mandatory_ones() {
        // RTK burns the whole budget after a successful mandatory motor-test
        // stop. Later best-effort stages are skipped; the mandatory link stage
        // still gets its own budget.
        let lifecycle = Lifecycle::new(LifecyclePolicy {
            stage_timeout: Duration::from_millis(30),
            total_timeout: Duration::from_millis(30),
            ..LifecyclePolicy::default()
        });
        let log = log();
        fake(
            Stage::MotorTest,
            "motor",
            Behaviour::Instant,
            &log,
            &lifecycle,
        );
        fake(Stage::Rtk, "rtk", Behaviour::Hang, &log, &lifecycle);
        fake(Stage::Logs, "logs", Behaviour::Instant, &log, &lifecycle);
        fake(Stage::Link, "link", Behaviour::Instant, &log, &lifecycle);

        let report = lifecycle
            .shutdown(StopReason::ShutdownApp, VehicleState::unknown())
            .await
            .expect("not refused");

        let ran = log.lock().expect("test log").clone();
        assert_eq!(
            ran,
            ["motor", "rtk", "link"],
            "timed-out RTK and later best-effort stages were skipped"
        );
        assert_eq!(
            report.outcome(Stage::MotorTest),
            Some(&StopOutcome::Stopped { elapsed_ms: 0 })
        );
        assert!(matches!(
            report.outcome(Stage::Rtk),
            Some(StopOutcome::TimedOut { .. })
        ));
        assert_eq!(
            report.outcome(Stage::Link),
            Some(&StopOutcome::Stopped { elapsed_ms: 0 }),
            "a mandatory stage keeps a full budget once the total is gone"
        );
        assert_eq!(report.outcome(Stage::Logs), Some(&StopOutcome::Skipped));
    }

    #[test]
    fn stage_order_and_scopes_are_the_contract() {
        assert_eq!(Stage::ALL.len(), 8);
        for (i, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(
                Stage::ALL.iter().position(|s| s == stage),
                Some(i),
                "no duplicates"
            );
        }
        // The safety properties the order encodes.
        let position = |stage: Stage| Stage::ALL.iter().position(|s| *s == stage);
        assert!(position(Stage::MotorTest) < position(Stage::Link));
        assert!(position(Stage::MissionAndCommand) < position(Stage::Recorder));
        assert!(position(Stage::Recorder) < position(Stage::Link));
        assert!(position(Stage::Link) < position(Stage::Terrain));
        assert!(!Stage::Terrain.is_link_scoped());
        assert!(Stage::MotorTest.is_mandatory() && Stage::Link.is_mandatory());
        assert!(!Stage::Rtk.is_mandatory());
    }

    #[test]
    fn the_confirm_keyword_names_the_action() {
        assert_eq!(StopReason::Disconnect.confirm_keyword(), "DISCONNECT");
        assert_eq!(StopReason::ShutdownApp.confirm_keyword(), "QUIT");
        assert_eq!(StopReason::WindowClose.confirm_keyword(), "QUIT");
        // Uppercase and single-word by contract: the dialog compares the typed
        // text verbatim, so a stray Enter or a lowercase name must not confirm.
        for reason in [
            StopReason::Disconnect,
            StopReason::ShutdownApp,
            StopReason::WindowClose,
        ] {
            let keyword = reason.confirm_keyword();
            assert!(keyword.chars().all(|c| c.is_ascii_uppercase()), "{keyword}");
            assert_eq!(keyword, keyword.trim());
        }
    }

    #[test]
    fn the_guard_asks_for_a_keyword_only_when_something_is_at_stake() {
        let policy = LifecyclePolicy::default();
        let grounded = VehicleState {
            armed: Some(false),
            on_ground: Some(true),
        };
        let clean = TeardownGuard::of(policy, StopReason::Disconnect, grounded, false);
        assert_eq!(clean.refusal, None);
        assert_eq!(
            clean.keyword, None,
            "nothing to lose: a plain confirm is enough"
        );
        assert!(!clean.forced);

        let lossy = TeardownGuard::of(policy, StopReason::Disconnect, grounded, true);
        assert_eq!(lossy.keyword.as_deref(), Some("DISCONNECT"));
        assert_eq!(lossy.armed, Some(false));
        assert_eq!(
            TeardownGuard::of(policy, StopReason::ShutdownApp, grounded, true)
                .keyword
                .as_deref(),
            Some("QUIT")
        );
    }

    #[test]
    fn a_refused_teardown_never_offers_a_keyword() {
        // The typo-proof interlock must not look like the way through: while
        // the vehicle is armed the only answer is the refusal.
        let policy = LifecyclePolicy::default();
        let armed = VehicleState {
            armed: Some(true),
            on_ground: Some(true),
        };
        let guard = TeardownGuard::of(policy, StopReason::Disconnect, armed, true);
        assert_eq!(guard.refusal.as_deref(), Some("the vehicle is armed"));
        assert_eq!(guard.keyword, None);

        let window_close = TeardownGuard::of(policy, StopReason::WindowClose, armed, true);
        assert!(!window_close.forced);
        assert_eq!(
            window_close.refusal.as_deref(),
            Some("the vehicle is armed"),
            "window close must not bypass the refusal"
        );
    }

    #[test]
    fn the_registry_free_refusal_is_the_one_the_supervisor_enforces() {
        // L4: the UI asks the policy directly, so the two answers must be the
        // same function, not two copies of the rule.
        let policy = LifecyclePolicy::default();
        let lifecycle = Lifecycle::new(LifecyclePolicy::default());
        for vehicle in [
            VehicleState::unknown(),
            VehicleState {
                armed: Some(true),
                on_ground: Some(true),
            },
            VehicleState {
                armed: Some(false),
                on_ground: Some(false),
            },
            VehicleState {
                armed: Some(false),
                on_ground: Some(true),
            },
        ] {
            assert_eq!(policy.refusal(vehicle), lifecycle.refusal(vehicle));
        }
        assert_eq!(
            policy
                .refusal(VehicleState {
                    armed: Some(true),
                    on_ground: Some(false)
                })
                .as_deref(),
            Some("the vehicle is armed"),
            "armed is reported first: it is the reason the operator must fix"
        );
    }
}
