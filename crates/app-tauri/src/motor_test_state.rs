//! Motor-test process state (A3): the running service handle, kept in
//! [`crate::state::AppState`] so it survives window focus changes. The
//! service itself is bound to a link: it stops when the link dies and is
//! re-spawned on the next start command.

use parking_lot::Mutex as PLMutex;
use std::time::Duration;

use maggcs_core::calib::mag::MagCalibration;
use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::mag_alarm::MagAlarm;
use maggcs_core::motor_test::scope::ScopeBus;
use maggcs_core::motor_test::source::ForwardSlot;
use maggcs_core::motor_test::{MotorTestService, SafetyLimits, SessionState};
use maggcs_core::signals::derived::EscAggregator;

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
}

/// The motor test's own target link: the **real PX4** (serial), separate from
/// the main GCS's primary link (which may be a UDP SITL). Every `ACTUATOR_TEST`
/// frame the motor test sends goes out through this link.
pub struct MotorTestTarget {
    pub handle: ConnectionHandle,
    pub endpoint: String,
    /// Expected MAVLink system ID of this target; this is routing only and
    /// does not change the vehicle's configured identity.
    pub target_system_id: u8,
    /// `COM_MOT_TEST_EN` read from the PX4 (`None` = not read yet / unknown).
    /// PX4 denies `ACTUATOR_TEST` while this parameter is not 1.
    pub com_mot_test_en: std::sync::Arc<std::sync::Mutex<Option<f32>>>,
}

/// Owns a motor-test auxiliary link and its link-scoped tasks. Stop is
/// idempotent; the caller must complete any active motor-test release before
/// invoking it.
pub struct AuxiliaryLinkRuntime {
    handle: Option<ConnectionHandle>,
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
            tasks: tasks.into_iter().collect(),
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
            handle.shutdown().await;
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
                handle.shutdown().await;
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

/// The SITL source link plus its SERVO_OUTPUT_RAW forwarder runtime.
pub struct SitlLinkRuntime {
    pub slot: ForwardSlot,
    pub endpoint: String,
    pub runtime: AuxiliaryLinkRuntime,
}

impl TargetLinkRuntime {
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
}

/// Process-wide holder; `Default` = nothing running. `last_state` mirrors the
/// newest event so the status poll stays cheap (no service round-trip).
#[derive(Default)]
pub struct MotorTestState {
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

/// Conservative default envelope per declared prop load (plan §2.2-2: the
/// load sets the output cap and confirmation strength). Numbers are starting
/// points — M0 bench data will tighten them.
pub fn limits_for_load(load: &str, target_system_id: u8) -> SafetyLimits {
    let (max_value, max_slew_per_s) = match load {
        // No props: safest — full range, brisk ramp.
        "none" => (1.0, 8.0),
        // Flat plates: some thrust, still benign.
        "plate" => (0.6, 4.0),
        // Full props: low cap, slow ramp, until M0 says otherwise.
        _ => (0.3, 2.0),
    };
    SafetyLimits {
        max_value,
        max_slew_per_s,
        heartbeat_timeout: Duration::from_millis(1500),
        target_sys: target_system_id,
        target_comp: 1,
    }
}
