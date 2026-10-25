//! Motor-test process state (A3): the running service handle, kept in
//! [`crate::state::AppState`] so it survives window focus changes. The
//! service itself is bound to a link: it stops when the link dies and is
//! re-spawned on the next start command.

use parking_lot::Mutex as PLMutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use maggcs_core::calib::mag::MagCalibration;
use maggcs_core::logs::capture::{SdCaptureError, SdCaptureStatus};
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::mag_alarm::MagAlarm;
use maggcs_core::motor_test::mapping::{OutputMap, ServoPort, SERVO_CHANNELS_PER_PORT};
use maggcs_core::motor_test::protocol::OutputProtocols;
use maggcs_core::motor_test::pwm::PwmModel;
use maggcs_core::motor_test::scope::{ScopeBus, ScopeOutputMeta, MAX_ESC};
use maggcs_core::motor_test::source::ForwardSlot;
use maggcs_core::motor_test::{MotorTestService, SafetyLimits, SessionState};
use maggcs_core::signals::derived::EscAggregator;

/// Component ID of the PX4 autopilot (default; the target is route-scoped).
pub const TARGET_COMPONENT_ID: u8 = 1;

/// Stable identity of the link a motor-test session is bound to. A reconnect
/// through `actuator_connect_target` creates a new connection ID; a stale
/// session must never be reused for the replacement target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MotorTestTargetRef {
    pub connection_id: usize,
    pub target_system_id: u8,
    pub target_component_id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLinkOrigin {
    Dedicated,
    MainSerial,
}

/// An explicit offer to borrow the currently connected primary serial node.
/// IPC callers echo the identity and generation; an obsolete offer is refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MainSerialTarget {
    pub target: MotorTestTargetRef,
    pub transport_generation: u64,
    pub endpoint: String,
}

/// The live service handle (plus the event-forwarder task handle, so the
/// forwarder dies with the entry).
pub struct RunningMotorTest {
    pub svc: MotorTestService,
    /// Held only so the forwarder aborts when the entry is replaced; never
    /// read directly.
    #[allow(dead_code)]
    pub forwarder: tauri::async_runtime::JoinHandle<()>,
    /// Latest values actually sent to the FC (post clamp/slew) — the
    /// window's actuator-output readout.
    pub output: tokio::sync::watch::Receiver<Vec<f32>>,
    /// Link this service was started with. A start for a different target
    /// must replace the session, never reuse it.
    pub target: MotorTestTargetRef,
    /// Output units, mapping and revision captured for this service's lifetime.
    pub scope_outputs: Vec<ScopeOutputMeta>,
    pub tone_generation: u64,
}

impl RunningMotorTest {
    pub fn matches(&self, target: MotorTestTargetRef) -> bool {
        self.target == target
    }
}

/// One validated, atomically published view of the target's output
/// configuration. Readers must take one clone under one lock so they cannot
/// mix fields from two refreshes.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputConfigSnapshot {
    /// Monotonic revision per target link, starting at 1.
    pub revision: u64,
    /// Link identity this snapshot was read from.
    pub target: MotorTestTargetRef,
    pub map: OutputMap,
    pub models: Vec<PwmModel>,
    pub protocols: OutputProtocols,
    /// Every existing timer parameter was readable.
    pub protocols_known: bool,
    pub dshot_3d_enable: Option<i32>,
    pub dshot_bidir_en: Option<i32>,
}

impl OutputConfigSnapshot {
    /// True only when a start may use this snapshot as-is.
    pub fn is_startable(&self) -> bool {
        self.map.state == maggcs_core::motor_test::mapping::OutputMapState::Complete
            && self.protocols_known
    }
}

/// The explicitly selected PX4 target, on a dedicated connection or the
/// borrowed primary serial link. Every motor command uses this exact handle.
pub struct MotorTestTarget {
    pub handle: ConnectionHandle,
    pub endpoint: String,
    /// Expected MAVLink system ID of this target; this is routing only and
    /// does not change the vehicle's configured identity.
    pub target_system_id: u8,
    /// `COM_MOT_TEST_EN` read from the PX4 (`None` = not read yet / unknown).
    /// PX4 denies `ACTUATOR_TEST` while this parameter is not 1.
    pub params: maggcs_core::params::ParamService,
    /// A confirmed persistent parameter change needs activation after reboot.
    pub restart_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Validated map/models/protocol/flags published as one revision.
    /// `None` until the first read; readers never mix fields.
    pub config: std::sync::Arc<PLMutex<Option<OutputConfigSnapshot>>>,
    /// A refresh hit a read failure and kept the previous snapshot for
    /// inspection. START stays blocked until a refresh succeeds.
    pub config_stale: std::sync::Arc<AtomicBool>,
    /// The FC's complete parameter-name inventory from a full pull, when one
    /// completed. Lets the reader distinguish absent channels from failures.
    pub param_names: std::sync::Arc<PLMutex<Option<std::collections::HashSet<String>>>>,
}

impl MotorTestTarget {
    /// Identity of this connection + node. `connection_id` changes whenever
    /// the target link is replaced, so pending/started work can detect it.
    pub fn target_ref(&self) -> MotorTestTargetRef {
        MotorTestTargetRef {
            connection_id: self.handle.handle_id(),
            target_system_id: self.target_system_id,
            target_component_id: TARGET_COMPONENT_ID,
        }
    }
}

/// Owns a motor-test auxiliary link and its link-scoped tasks. Stop is
/// idempotent; the caller must complete any active motor-test release before
/// invoking it.
pub struct AuxiliaryLinkRuntime {
    handle: Option<ConnectionHandle>,
    borrowed_generation: Option<u64>,
    tasks: Vec<tauri::async_runtime::JoinHandle<()>>,
}

impl AuxiliaryLinkRuntime {
    /// Construct an owner for the connection and its observer/tap task.
    pub fn new(
        handle: ConnectionHandle,
        tasks: impl IntoIterator<Item = tauri::async_runtime::JoinHandle<()>>,
    ) -> Self {
        Self {
            handle: Some(handle),
            borrowed_generation: None,
            tasks: tasks.into_iter().collect(),
        }
    }

    /// Own only tool-local tasks; the primary runtime retains the transport.
    pub fn borrowed(handle: &ConnectionHandle) -> Self {
        Self {
            handle: None,
            borrowed_generation: Some(handle.generation()),
            tasks: Vec::new(),
        }
    }

    /// Register another link-scoped observer before publishing the runtime.
    pub fn add_task(&mut self, task: tauri::async_runtime::JoinHandle<()>) {
        self.tasks.push(task);
    }

    /// Register multiple tasks belonging to the same link.
    pub fn add_tasks(
        &mut self,
        tasks: impl IntoIterator<Item = tauri::async_runtime::JoinHandle<()>>,
    ) {
        self.tasks.extend(tasks);
    }

    /// Take tasks for a window-only detach while keeping the link connected.
    pub fn take_tasks(&mut self) -> Vec<tauri::async_runtime::JoinHandle<()>> {
        std::mem::take(&mut self.tasks)
    }

    /// Stop observers and then the connection. Repeated calls are harmless
    /// and never hold a state lock across an await.
    pub async fn stop(&mut self) {
        stop_auxiliary_tasks(self.take_tasks()).await;
        if let Some(handle) = self.handle.take() {
            let _ = handle
                .shutdown_and_wait(maggcs_core::lifecycle::DEFAULT_STAGE_TIMEOUT)
                .await;
        }
    }
}

impl Drop for AuxiliaryLinkRuntime {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
        if let Some(handle) = self.handle.take() {
            tauri::async_runtime::spawn(async move {
                let _ = handle
                    .shutdown_and_wait(maggcs_core::lifecycle::DEFAULT_STAGE_TIMEOUT)
                    .await;
            });
        }
    }
}

/// Abort and join link-scoped observer tasks after releasing state locks.
pub async fn stop_auxiliary_tasks(tasks: Vec<tauri::async_runtime::JoinHandle<()>>) {
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

/// The motor test target link plus its telemetry observer runtime.
pub struct TargetLinkRuntime {
    pub target: MotorTestTarget,
    pub runtime: AuxiliaryLinkRuntime,
}

/// Which SITL node and physical output bank feeds the forward slot. The
/// dedicated connection is the first identity component; these fields pick the
/// node and port so another system on the same bus cannot overwrite the feed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SitlSourceConfig {
    pub endpoint: String,
    pub system_id: u8,
    pub component_id: u8,
    pub port: ServoPort,
}

/// The SITL source link plus its SERVO_OUTPUT_RAW forwarder runtime.
pub struct SitlLinkRuntime {
    pub slot: ForwardSlot,
    pub source: SitlSourceConfig,
    pub runtime: AuxiliaryLinkRuntime,
}

impl TargetLinkRuntime {
    pub fn origin(&self) -> TargetLinkOrigin {
        if self.runtime.borrowed_generation.is_some() {
            TargetLinkOrigin::MainSerial
        } else {
            TargetLinkOrigin::Dedicated
        }
    }

    pub fn generation_is_current(&self) -> bool {
        self.runtime
            .borrowed_generation
            .is_none_or(|generation| generation == self.target.handle.generation())
    }

    /// Stop observers and the link owned by this target.
    pub async fn stop(mut self) {
        self.runtime.stop().await;
    }
}

impl SitlLinkRuntime {
    /// Stop the actuator tap and the link owned by this SITL source.
    pub async fn stop(mut self) {
        self.runtime.stop().await;
    }
}

#[cfg(test)]
mod auxiliary_runtime_tests {
    use super::*;

    #[tokio::test]
    async fn auxiliary_runtime_stop_is_idempotent() {
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .expect("bind ephemeral UDP port")
            .local_addr()
            .expect("socket address");
        let config = maggcs_core::mavlink::ConnectionConfig {
            endpoint: maggcs_core::mavlink::Endpoint::UdpListener { addr: port },
            ..Default::default()
        };
        let (handle, _events, first) = maggcs_core::mavlink::connection::spawn_connection(config)
            .await
            .expect("spawn auxiliary link");
        first
            .await
            .expect("first connection result")
            .expect("link bound");
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_dropped = dropped.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);
            impl Drop for DropFlag {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _flag = DropFlag(task_dropped);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx
            .await
            .expect("auxiliary task started before it was aborted");
        let mut runtime = AuxiliaryLinkRuntime::new(handle.clone(), [task]);

        runtime.stop().await;
        runtime.stop().await;
        assert!(runtime.handle.is_none());
        assert!(runtime.tasks.is_empty());
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn auxiliary_runtime_stop_after_transport_failure_releases_tasks() {
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .expect("bind ephemeral UDP port")
            .local_addr()
            .expect("socket address");
        let config = maggcs_core::mavlink::ConnectionConfig {
            endpoint: maggcs_core::mavlink::Endpoint::UdpListener { addr: port },
            ..Default::default()
        };
        let (handle, _events, first) = maggcs_core::mavlink::connection::spawn_connection(config)
            .await
            .expect("spawn auxiliary link");
        first
            .await
            .expect("first connection result")
            .expect("link bound");

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_dropped = dropped.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);
            impl Drop for DropFlag {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _flag = DropFlag(task_dropped);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx
            .await
            .expect("auxiliary task started before transport failure");
        let mut runtime = AuxiliaryLinkRuntime::new(handle.clone(), [task]);

        // The connection worker stops (e.g. its transport failed). The runtime
        // still owns a handle clone and the observer; stop must not hang.
        tokio::time::timeout(std::time::Duration::from_secs(2), handle.shutdown())
            .await
            .expect("transport shutdown timed out");
        tokio::time::timeout(std::time::Duration::from_secs(2), runtime.stop())
            .await
            .expect("auxiliary stop hung after transport failure");

        assert!(runtime.handle.is_none());
        assert!(runtime.tasks.is_empty());
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "observer must be dropped before stop returns"
        );
    }
}

/// One physical output bank from the target's `SERVO_OUTPUT_RAW`.
#[derive(Debug, Clone, Copy)]
pub struct FcPortOutputs {
    pub pwm: [u16; SERVO_CHANNELS_PER_PORT as usize],
    pub at: std::time::Instant,
}

/// Live mag/ESC telemetry for the motor-test window (A4 wiring): the tap
/// updates the aggregator + alarm; the window polls or receives events.
pub struct MotorTelemetry {
    pub esc: EscAggregator,
    pub alarm: MagAlarm,
    /// Loaded calibration (`None` = apply none — raw pass-through).
    pub calib: Option<MagCalibration>,
    /// Latest calibrated total field, nT (`None` = no mag sample yet).
    pub last_mag_total_nt: Option<f64>,
    /// The target FC's latest heartbeat (armed / mode) decoded from the serial
    /// link the motor test sends to — the state that gates the START button.
    pub hb: Option<maggcs_core::telemetry::Heartbeat>,
    /// The target FC's latest `SERVO_OUTPUT_RAW`, separately for MAIN and AUX.
    /// Index by [`ServoPort::wire`]; feedback is selected later by the output
    /// mapping, so AUX bank frames cannot overwrite MAIN values.
    pub fc_outputs: [Option<FcPortOutputs>; 2],
    /// When the last target-FC heartbeat arrived. Unlike `hb` (the decoded
    /// state) this lets the status poll tell "alive now" from "was alive once".
    pub last_heartbeat_at: Option<std::time::Instant>,
    /// Last `ESC_STATUS` receive time for each zero-based ESC slot.
    /// `None` = that ESC has never reported. Only slots covered by a frame
    /// are refreshed, including when its first index is not a multiple of four.
    pub esc_seen_at: [Option<std::time::Instant>; MAX_ESC],
}

/// Process-wide holder; `Default` = nothing running. `last_state` mirrors the
/// newest event so the status poll stays cheap (no service round-trip).
#[derive(Default)]
pub struct MotorTestState {
    pub control: crate::motor_control_service::MotorControlGate,
    pub running: PLMutex<Option<RunningMotorTest>>,
    pub last_state: PLMutex<Option<SessionState>>,
    /// The real-PX4 (serial) link the motor test sends to.
    pub target: PLMutex<Option<TargetLinkRuntime>>,
    pub sitl: PLMutex<Option<SitlLinkRuntime>>,
    /// Telemetry tap (mag/ESC) — `None` until a link attaches.
    pub telemetry: PLMutex<Option<MotorTelemetry>>,
    /// Producer → flusher queue for the window's signal scope (SITL servo,
    /// target-FC servo/mag/ESC, sent commands).
    pub scope: ScopeBus,
    /// Abort handle for the scope flusher (runs while the window is open).
    pub scope_flusher: PLMutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// SD-log list/download state for the serial target PX4 (feature 2); the
    /// protocol runtime is link-scoped and rebuilt with the target.
    pub logs: PLMutex<ActuatorLogs>,
    /// Target-shell SD-file capture (Task 6). Link-scoped like the log
    /// download runtime; stopped before its target connection is closed.
    pub capture: PLMutex<ActuatorCapture>,
    /// Per-test run coordinator + recovery manifest (Task 8).
    pub run: PLMutex<crate::run_service::ActuatorRun>,
    pub tones: std::sync::Arc<crate::motor_tone_service::MotorToneState>,
    pub streams: PLMutex<Option<crate::stream_service::ActuatorStreams>>,
    pub stream_profiles: std::sync::Arc<PLMutex<crate::stream_service::StreamProfiles>>,
}

/// The serial target's SD-capture service: one shell controller per target.
pub struct ActuatorCapture {
    pub runtime: Option<ActuatorCaptureRuntime>,
    pub status: std::sync::Arc<PLMutex<SdCaptureStatus>>,
}

impl Default for ActuatorCapture {
    fn default() -> Self {
        Self {
            runtime: None,
            status: std::sync::Arc::new(PLMutex::new(SdCaptureStatus::default())),
        }
    }
}

/// A running target-shell capture service.
pub struct ActuatorCaptureRuntime {
    pub(crate) target: MotorTestTargetRef,
    pub(crate) commands: Option<tokio::sync::mpsc::Sender<CaptureRequest>>,
    pub(crate) task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl ActuatorCaptureRuntime {
    /// Close the command channel and wait for the service task to exit.
    pub async fn stop(mut self, deadline: std::time::Duration) {
        self.commands.take();
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(deadline, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

impl Drop for ActuatorCaptureRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Capture requests from Tauri commands to the shell task.
pub(crate) enum CaptureRequest {
    Tone(crate::motor_tone_service::ToneJob),
    Shell {
        command: String,
        reply: tokio::sync::oneshot::Sender<Result<String, SdCaptureError>>,
    },
    Check {
        sdlog_mode: Option<i32>,
        sdlog_backend: Option<i32>,
        reply: tokio::sync::oneshot::Sender<Result<SdCaptureStatus, SdCaptureError>>,
    },
    Start {
        test_id: String,
        sdlog_mode: Option<i32>,
        sdlog_backend: Option<i32>,
        reply: tokio::sync::oneshot::Sender<Result<SdCaptureStatus, SdCaptureError>>,
    },
    Stop {
        reply: tokio::sync::oneshot::Sender<Result<SdCaptureStatus, SdCaptureError>>,
    },
}

/// The serial target's SD-log download runtime (feature 2): runs a core
/// `LogProtocol` on the target handlestant, forwarding events to the window.
pub struct ActuatorLogs {
    pub runtime: Option<ActuatorLogsRuntime>,
    pub last_list: std::sync::Arc<std::sync::Mutex<Option<Vec<maggcs_core::logs::LogEntryInfo>>>>,
    pub last_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl Default for ActuatorLogs {
    fn default() -> Self {
        Self {
            runtime: None,
            last_list: std::sync::Arc::new(std::sync::Mutex::new(None)),
            last_error: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// A running SD-log protocol on the serial target link.
pub struct ActuatorLogsRuntime {
    pub(crate) commands: Option<tokio::sync::mpsc::Sender<ActuatorLogCommand>>,
    pub(crate) task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl ActuatorLogsRuntime {
    pub(crate) fn sender(&self) -> Option<tokio::sync::mpsc::Sender<ActuatorLogCommand>> {
        self.commands.clone()
    }

    /// Close the command channel and wait for the protocol task to exit.
    pub async fn stop(mut self, deadline: std::time::Duration) {
        self.commands.take();
        if let Some(mut task) = self.task.take() {
            let _ = tokio::time::timeout(deadline, &mut task).await;
        }
    }
}

impl Drop for ActuatorLogsRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub(crate) enum ActuatorLogCommand {
    List {
        reply: tokio::sync::oneshot::Sender<Result<Vec<maggcs_core::logs::LogEntryInfo>, String>>,
    },
    Download {
        id: u16,
        size: u32,
        save_path: String,
    },
    Cancel,
}

impl MotorTestState {
    /// Record the newest session state (called by the event forwarder).
    pub fn note_state(&self, state: SessionState) {
        *self.last_state.lock() = Some(state);
    }

    /// Newest known session state, if any event arrived yet.
    pub fn session(&self) -> Option<SessionState> {
        *self.last_state.lock()
    }
}

/// The one safety envelope for every motor-test start. The UI no longer
/// lets the operator declare a lighter load: even a props-off bench must pass
/// the full-prop cap, slew limit and long-press confirmation.
pub fn motor_test_limits(target_system_id: u8) -> SafetyLimits {
    SafetyLimits {
        max_value: 0.3,
        max_slew_per_s: 2.0,
        heartbeat_timeout: Duration::from_millis(1500),
        target_sys: target_system_id,
        target_comp: TARGET_COMPONENT_ID,
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    #[test]
    fn every_motor_test_uses_the_full_prop_envelope() {
        let limits = motor_test_limits(42);
        assert_eq!((limits.max_value, limits.max_slew_per_s), (0.3, 2.0));
        assert_eq!(limits.target_sys, 42);
        assert_eq!(limits.target_comp, TARGET_COMPONENT_ID);
    }
}
