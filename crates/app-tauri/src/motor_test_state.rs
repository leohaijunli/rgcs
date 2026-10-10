//! Motor-test process state (A3): the running service handle, kept in
//! [`crate::state::AppState`] so it survives window focus changes. The
//! service itself is bound to a link: it stops when the link dies and is
//! re-spawned on the next start command.

use parking_lot::Mutex as PLMutex;
use std::time::Duration;

use maggcs_core::mavlink::ConnectionHandle;
use maggcs_core::motor_test::source::ForwardSlot;
use maggcs_core::motor_test::{MotorTestService, SafetyLimits, SessionState};

/// The live service handle (plus the event-forwarder task handle, so the
/// forwarder dies with the entry).
pub struct RunningMotorTest {
    pub svc: MotorTestService,
    /// Held only so the forwarder aborts when the entry is replaced; never
    /// read directly.
    #[allow(dead_code)]
    pub forwarder: tauri::async_runtime::JoinHandle<()>,
}

/// The secondary link: a PX4 SITL whose `SERVO_OUTPUT_RAW` feeds the
/// realtime-forward slot (revised D3).
pub struct SitlLink {
    pub handle: ConnectionHandle,
    /// Tap task: normalizes the SITL's main outputs 1–4 into the slot.
    pub tap: tauri::async_runtime::JoinHandle<()>,
    pub slot: ForwardSlot,
    pub endpoint: String,
}

/// Process-wide holder; `Default` = nothing running. `last_state` mirrors the
/// newest event so the status poll stays cheap (no service round-trip).
#[derive(Default)]
pub struct MotorTestState {
    pub running: PLMutex<Option<RunningMotorTest>>,
    pub last_state: PLMutex<Option<SessionState>>,
    pub sitl: PLMutex<Option<SitlLink>>,
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
pub fn limits_for_load(load: &str) -> SafetyLimits {
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
        target_sys: 1,
        target_comp: 1,
    }
}
