//! MAVLink command protocol (issue #5).
//!
//! Sends a `COMMAND_LONG` to the flight controller and awaits its
//! `COMMAND_ACK`: lost acks are retransmitted with an incremented
//! `confirmation` until the budget is exhausted. Pure state machine — the
//! caller feeds received frames via [`handle`] and sends the returned frames
//! on the link; [`retransmit_due`] returns frames to re-send after a timeout.
//!
//! Sessions are single-slot: at most one command may be in flight per
//! instance. Starting a second command while one is pending fails with
//! [`CommandError::Busy`] — unless the newcomer is an *emergency* command
//! (§3 A7): RTL, land and arm/disarm take the slot from an in-flight normal
//! command, which is reported as [`CommandError::Superseded`] instead of
//! being dropped silently. An emergency command is never preempted.

pub mod operation_types;
pub mod service;
mod transaction;

pub use service::{CommandCommand, CommandService, RETRANSMIT_TICK};
pub use transaction::{CommandTransaction, OWNED_CANCEL_TIMEOUT};

use std::time::{Duration, Instant};

use ::mavlink::common::{MavCmd, MavMessage, MavResult, COMMAND_ACK_DATA, COMMAND_LONG_DATA};
use ::mavlink::MavHeader;

/// Timeout before an unacked command is retransmitted.
pub const COMMAND_RETRY_TIMEOUT: Duration = Duration::from_millis(1000);
/// Maximum retransmissions before giving up (total sends = retries + 1).
pub const COMMAND_MAX_RETRIES: u32 = 3;

/// Final outcome of a command as reported by the flight controller.
///
/// Maps 1:1 onto [`MavResult`]; surfaced to the UI through the app layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CommandResult {
    Accepted,
    TemporarilyRejected,
    Denied,
    Unsupported,
    Failed,
    InProgress,
    Cancelled,
    CommandLongOnly,
    CommandIntOnly,
    #[serde(rename = "unsupported_mav_frame")]
    CommandUnsupportedMavFrame,
}

impl CommandResult {
    /// Map from the MAVLink result enum.
    pub fn from_mav(r: MavResult) -> Self {
        match r {
            MavResult::MAV_RESULT_ACCEPTED => Self::Accepted,
            MavResult::MAV_RESULT_TEMPORARILY_REJECTED => Self::TemporarilyRejected,
            MavResult::MAV_RESULT_DENIED => Self::Denied,
            MavResult::MAV_RESULT_UNSUPPORTED => Self::Unsupported,
            MavResult::MAV_RESULT_FAILED => Self::Failed,
            MavResult::MAV_RESULT_IN_PROGRESS => Self::InProgress,
            MavResult::MAV_RESULT_CANCELLED => Self::Cancelled,
            MavResult::MAV_RESULT_COMMAND_LONG_ONLY => Self::CommandLongOnly,
            MavResult::MAV_RESULT_COMMAND_INT_ONLY => Self::CommandIntOnly,
            _ => Self::CommandUnsupportedMavFrame,
        }
    }

    /// Whether the FC executed the command successfully.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Accepted)
    }
}

/// Map a `MAV_CMD` onto the stable name shared with the UI.
///
/// One table for the app shell, the alert text and the event log: a command
/// the operator sees as "arm_disarm" everywhere (L1 uses it to label a
/// `STATUSTEXT` that explains a rejection).
pub fn command_label(command: MavCmd) -> &'static str {
    match command {
        MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH => "rtl",
        MavCmd::MAV_CMD_NAV_TAKEOFF => "takeoff",
        MavCmd::MAV_CMD_NAV_LAND => "land",
        MavCmd::MAV_CMD_DO_SET_MODE => "set_mode",
        MavCmd::MAV_CMD_COMPONENT_ARM_DISARM => "arm_disarm",
        MavCmd::MAV_CMD_DO_REPOSITION => "goto",
        MavCmd::MAV_CMD_DO_SET_HOME => "set_home",
        _ => "unknown",
    }
}

/// Command urgency (plan §3 A7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Waits its turn: at most one command is in flight, and this one queues
    /// behind nothing.
    #[default]
    Normal,
    /// Takes effect now: preempts an in-flight [`Priority::Normal`] command.
    Emergency,
}

impl Priority {
    pub fn is_emergency(self) -> bool {
        matches!(self, Self::Emergency)
    }
}

/// Urgency of a `MAV_CMD`.
///
/// The list is the plan's (RTL / land / disarm) with arm folded in: both
/// directions of `COMPONENT_ARM_DISARM` are safety commands nobody wants
/// queued behind a `goto`, and PX4 runs its own prearm checks on the arm
/// direction anyway.
pub fn command_priority(command: MavCmd) -> Priority {
    match command {
        MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH
        | MavCmd::MAV_CMD_NAV_LAND
        | MavCmd::MAV_CMD_COMPONENT_ARM_DISARM => Priority::Emergency,
        _ => Priority::Normal,
    }
}

/// Events emitted by the command session.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandEvent {
    /// A `COMMAND_LONG` was (re)transmitted; `confirmation` counts retries.
    Sent { command: MavCmd, confirmation: u8 },
    /// A terminal `COMMAND_ACK` was received from the target FC.
    Completed {
        command: MavCmd,
        result: CommandResult,
    },
    /// The session ended without a terminal ack.
    Failed {
        command: MavCmd,
        error: CommandError,
    },
}

/// Errors that can end a command session.
#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, thiserror::Error, ts_rs::TS,
)]
#[ts(export, rename = "CommandProtocolError")]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CommandError {
    /// A session is already in flight.
    #[error("a command is already in progress")]
    Busy,
    /// No `COMMAND_ACK` arrived within the retry budget.
    #[error("no COMMAND_ACK after {attempts} attempts")]
    NoAck { attempts: u32 },
    /// The link to the flight controller was lost mid-session.
    #[error("link to the flight controller was lost")]
    LinkLost,
    /// An emergency command took the single session slot (A7). The command was
    /// abandoned before its ack arrived: the FC may or may not have run it.
    #[error("superseded by an emergency command")]
    Superseded,
    /// The command service task terminated.
    #[error("command service stopped")]
    ServiceStopped,
    /// The caller-side await window (`send_await`) elapsed before the
    /// session reached a terminal outcome.
    #[error("no terminal COMMAND_ACK within the await timeout")]
    TimedOut,
    #[error("the owned command was cancelled")]
    Cancelled,
    #[error("the command belongs to an earlier transport generation")]
    TargetChanged,
    #[error("this command transaction was already submitted")]
    AlreadySubmitted,
    #[error("an unresolved command in this transport generation makes ACK correlation ambiguous")]
    AmbiguousAck,
}

#[derive(Debug)]
enum State {
    Idle,
    AwaitAck {
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
        confirmation: u8,
        deadline: Instant,
        retries: u32,
    },
}

/// Build a `COMMAND_LONG` addressed to the target FC.
fn command_long(
    command: MavCmd,
    params: [f32; 7],
    confirmation: u8,
    target_sys: u8,
    target_comp: u8,
) -> MavMessage {
    MavMessage::COMMAND_LONG(COMMAND_LONG_DATA {
        param1: params[0],
        param2: params[1],
        param3: params[2],
        param4: params[3],
        param5: params[4],
        param6: params[5],
        param7: params[6],
        command,
        target_system: target_sys,
        target_component: target_comp,
        confirmation,
    })
}

/// MAVLink command protocol state machine (one per FC).
#[derive(Debug)]
pub struct CommandSession {
    /// The flight controller we talk to.
    target_sys: u8,
    target_comp: u8,
    state: State,
}

impl CommandSession {
    /// New idle session for the given flight controller. `COMMAND_ACK` has no
    /// target field, so the session only accepts acks sent *by* the FC.
    pub fn new(target_sys: u8, target_comp: u8) -> Self {
        Self {
            target_sys,
            target_comp,
            state: State::Idle,
        }
    }

    /// True when no command is in flight.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    pub(crate) fn target_system(&self) -> u8 {
        self.target_sys
    }

    pub(crate) fn target_component(&self) -> u8 {
        self.target_comp
    }

    /// Urgency of the command in flight, `None` when the slot is free. Lets
    /// the caller predict whether [`Self::begin_with_priority`] would preempt
    /// or refuse.
    pub fn in_flight_priority(&self) -> Option<Priority> {
        match &self.state {
            State::Idle => None,
            State::AwaitAck { priority, .. } => Some(*priority),
        }
    }

    /// Send a command of the given urgency and start awaiting its ack.
    ///
    /// A7: while something is in flight, an emergency command takes the slot
    /// and the command it displaced is reported as
    /// [`CommandError::Superseded`] *before* the new `Sent`. A normal command
    /// arriving during anything, or an emergency arriving during an emergency,
    /// is refused with [`CommandError::Busy`] — there is still only ever one
    /// command in flight, so an ack can never be matched to the wrong
    /// `COMMAND_LONG`.
    pub fn begin_with_priority(
        &mut self,
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
    ) -> Result<(Vec<CommandEvent>, Vec<MavMessage>), CommandError> {
        let in_flight = match &self.state {
            State::Idle => None,
            State::AwaitAck {
                command, priority, ..
            } => Some((*command, *priority)),
        };
        let Some((in_flight, in_flight_priority)) = in_flight else {
            return Ok(self.start(command, params, priority));
        };
        if !(priority.is_emergency() && in_flight_priority == Priority::Normal) {
            return Err(CommandError::Busy);
        }
        let (mut events, frames) = self.start(command, params, priority);
        // The displaced command is reported first: its caller learns that the
        // command ended (and why) instead of waiting out a retry budget for an
        // ack that will now be ignored.
        events.insert(
            0,
            CommandEvent::Failed {
                command: in_flight,
                error: CommandError::Superseded,
            },
        );
        Ok((events, frames))
    }

    /// Send a normal-priority command and start awaiting its ack.
    pub fn begin(
        &mut self,
        command: MavCmd,
        params: [f32; 7],
    ) -> Result<(Vec<CommandEvent>, Vec<MavMessage>), CommandError> {
        self.begin_with_priority(command, params, Priority::Normal)
    }

    /// Take the slot for `command` (the caller has already checked that it may).
    fn start(
        &mut self,
        command: MavCmd,
        params: [f32; 7],
        priority: Priority,
    ) -> (Vec<CommandEvent>, Vec<MavMessage>) {
        let msg = command_long(command, params, 0, self.target_sys, self.target_comp);
        self.state = State::AwaitAck {
            command,
            params,
            priority,
            confirmation: 0,
            deadline: Instant::now() + COMMAND_RETRY_TIMEOUT,
            retries: 0,
        };
        (
            vec![CommandEvent::Sent {
                command,
                confirmation: 0,
            }],
            vec![msg],
        )
    }

    /// Handle an incoming frame. Only `COMMAND_ACK` frames from the target FC
    /// matching the pending command are accepted; everything else is ignored
    /// (including acks addressed to other GCS sessions).
    pub fn handle(
        &mut self,
        header: &MavHeader,
        msg: &MavMessage,
    ) -> (Vec<CommandEvent>, Vec<MavMessage>) {
        if header.system_id != self.target_sys || header.component_id != self.target_comp {
            return (Vec::new(), Vec::new());
        }
        let MavMessage::COMMAND_ACK(ack) = msg else {
            return (Vec::new(), Vec::new());
        };
        let Some((command, result)) = self.on_ack(ack) else {
            return (Vec::new(), Vec::new());
        };
        (
            vec![CommandEvent::Completed { command, result }],
            Vec::new(),
        )
    }

    /// Frames due for retransmission (call periodically). Resends the pending
    /// `COMMAND_LONG` with `confirmation` incremented.
    pub fn retransmit_due(&mut self, now: Instant) -> (Vec<CommandEvent>, Vec<MavMessage>) {
        let State::AwaitAck {
            command,
            params,
            confirmation,
            deadline,
            retries,
            ..
        } = &mut self.state
        else {
            return (Vec::new(), Vec::new());
        };
        if now < *deadline {
            return (Vec::new(), Vec::new());
        }
        *retries += 1;
        if *retries > COMMAND_MAX_RETRIES {
            return (Vec::new(), Vec::new());
        }
        *confirmation = confirmation.saturating_add(1);
        *deadline = now + COMMAND_RETRY_TIMEOUT;
        let msg = command_long(
            *command,
            *params,
            *confirmation,
            self.target_sys,
            self.target_comp,
        );
        (
            vec![CommandEvent::Sent {
                command: *command,
                confirmation: *confirmation,
            }],
            vec![msg],
        )
    }

    /// Report a retransmission-exhausted failure, if the pending command gave
    /// up. Returns the command that failed and the error.
    pub fn take_timeout_failure(&mut self) -> Option<(MavCmd, CommandError)> {
        let State::AwaitAck {
            command, retries, ..
        } = &self.state
        else {
            return None;
        };
        if *retries > COMMAND_MAX_RETRIES {
            let command = *command;
            let attempts = *retries;
            self.state = State::Idle;
            Some((command, CommandError::NoAck { attempts }))
        } else {
            None
        }
    }

    /// End an in-flight session because the link went away.
    pub fn cancel(&mut self) -> Vec<CommandEvent> {
        let State::AwaitAck { command, .. } = &self.state else {
            return Vec::new();
        };
        let command = *command;
        self.state = State::Idle;
        vec![CommandEvent::Failed {
            command,
            error: CommandError::LinkLost,
        }]
    }

    fn on_ack(&mut self, ack: &COMMAND_ACK_DATA) -> Option<(MavCmd, CommandResult)> {
        let State::AwaitAck { command, .. } = &self.state else {
            return None;
        };
        let command = *command;
        if ack.command != command {
            return None;
        }
        if ack.result == MavResult::MAV_RESULT_IN_PROGRESS {
            // The FC is executing; keep waiting, refresh the retransmit
            // deadline instead of counting this as a missed ack.
            if let State::AwaitAck { deadline, .. } = &mut self.state {
                *deadline = Instant::now() + COMMAND_RETRY_TIMEOUT;
            }
            return None;
        }
        self.state = State::Idle;
        Some((command, CommandResult::from_mav(ack.result)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FC_SYS: u8 = 1;
    const FC_COMP: u8 = 1;

    fn rtl() -> MavCmd {
        MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH
    }

    fn ack(command: MavCmd, result: MavResult) -> MavMessage {
        MavMessage::COMMAND_ACK(COMMAND_ACK_DATA {
            command,
            result,
            ..Default::default()
        })
    }

    fn fc_header() -> MavHeader {
        MavHeader {
            system_id: FC_SYS,
            component_id: FC_COMP,
            sequence: 0,
        }
    }

    #[test]
    fn begin_sends_long_and_awaits() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        let (events, frames) = s.begin(rtl(), [0.0; 7]).unwrap();
        assert_eq!(
            events,
            vec![CommandEvent::Sent {
                command: rtl(),
                confirmation: 0,
            }]
        );
        assert_eq!(frames.len(), 1);
        let MavMessage::COMMAND_LONG(c) = &frames[0] else {
            panic!("expected COMMAND_LONG")
        };
        assert_eq!(c.command, rtl());
        assert_eq!(c.confirmation, 0);
        assert_eq!(c.target_system, FC_SYS);
        assert_eq!(c.target_component, FC_COMP);
        assert!(!s.is_idle());
    }

    #[test]
    fn busy_rejects_second_begin() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        assert_eq!(s.begin(rtl(), [0.0; 7]), Err(CommandError::Busy));
    }

    #[test]
    fn accepted_ack_completes() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let (events, frames) = s.handle(&fc_header(), &ack(rtl(), MavResult::MAV_RESULT_ACCEPTED));
        assert!(frames.is_empty());
        assert_eq!(
            events,
            vec![CommandEvent::Completed {
                command: rtl(),
                result: CommandResult::Accepted,
            }]
        );
        assert!(s.is_idle());
    }

    #[test]
    fn denied_ack_reports_result() {
        // Acceptance: a denied command must surface a distinct, terminal result.
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let (events, _) = s.handle(&fc_header(), &ack(rtl(), MavResult::MAV_RESULT_DENIED));
        assert_eq!(
            events,
            vec![CommandEvent::Completed {
                command: rtl(),
                result: CommandResult::Denied,
            }]
        );
        assert!(s.is_idle());
    }

    #[test]
    fn in_progress_keeps_waiting() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let (events, frames) =
            s.handle(&fc_header(), &ack(rtl(), MavResult::MAV_RESULT_IN_PROGRESS));
        assert!(events.is_empty() && frames.is_empty());
        assert!(!s.is_idle());
    }

    #[test]
    fn ack_for_other_command_ignored() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let (events, frames) = s.handle(
            &fc_header(),
            &ack(
                MavCmd::MAV_CMD_COMPONENT_ARM_DISARM,
                MavResult::MAV_RESULT_ACCEPTED,
            ),
        );
        assert!(events.is_empty() && frames.is_empty());
        assert!(!s.is_idle());
    }

    #[test]
    fn ack_from_other_node_ignored() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        // A second GCS (sysid 99) acknowledges on behalf of the FC: ignored.
        let foreign = MavHeader {
            system_id: 99,
            component_id: 1,
            sequence: 0,
        };
        let (events, frames) = s.handle(&foreign, &ack(rtl(), MavResult::MAV_RESULT_ACCEPTED));
        assert!(events.is_empty() && frames.is_empty());
        assert!(!s.is_idle());
    }

    #[test]
    fn lost_ack_retransmits_with_confirmation() {
        // Acceptance: a lost ACK must be retried with confirmation incremented.
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();

        let mut now = Instant::now() + COMMAND_RETRY_TIMEOUT + Duration::from_millis(1);
        for expected in 1..=COMMAND_MAX_RETRIES {
            let (events, frames) = s.retransmit_due(now);
            assert_eq!(
                events,
                vec![CommandEvent::Sent {
                    command: rtl(),
                    confirmation: expected as u8,
                }],
                "retry {expected} must resend with confirmation {expected}"
            );
            assert_eq!(frames.len(), 1);
            let MavMessage::COMMAND_LONG(c) = &frames[0] else {
                panic!("expected COMMAND_LONG")
            };
            assert_eq!(c.confirmation, expected as u8);
            now += COMMAND_RETRY_TIMEOUT;
        }
        assert!(!s.is_idle(), "not exhausted yet");
    }

    #[test]
    fn lost_ack_exhausts_after_budget() {
        // Acceptance: an always-lost ACK must end with a clear NoAck failure.
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();

        let mut now = Instant::now() + COMMAND_RETRY_TIMEOUT + Duration::from_millis(1);
        for _ in 0..COMMAND_MAX_RETRIES {
            let (_, _) = s.retransmit_due(now);
            now += COMMAND_RETRY_TIMEOUT;
        }
        let (events, frames) = s.retransmit_due(now);
        assert!(events.is_empty() && frames.is_empty());
        assert_eq!(
            s.take_timeout_failure(),
            Some((
                rtl(),
                CommandError::NoAck {
                    attempts: COMMAND_MAX_RETRIES + 1,
                }
            ))
        );
        assert!(s.is_idle());
    }

    #[test]
    fn early_tick_sends_nothing() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let (events, frames) = s.retransmit_due(Instant::now());
        assert!(events.is_empty() && frames.is_empty());
    }

    #[test]
    fn cancel_reports_link_lost() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin(rtl(), [0.0; 7]).unwrap();
        let events = s.cancel();
        assert_eq!(
            events,
            vec![CommandEvent::Failed {
                command: rtl(),
                error: CommandError::LinkLost,
            }]
        );
        assert!(s.is_idle());
        assert!(s.cancel().is_empty(), "cancel while idle emits nothing");
    }

    #[test]
    fn idle_session_handles_ignore_ack() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        let (events, frames) = s.handle(&fc_header(), &ack(rtl(), MavResult::MAV_RESULT_ACCEPTED));
        assert!(events.is_empty() && frames.is_empty());
    }

    /// GOTO: the normal-priority command the acceptance case preempts.
    fn goto() -> MavCmd {
        MavCmd::MAV_CMD_DO_REPOSITION
    }

    #[test]
    fn the_safety_commands_are_emergencies() {
        assert_eq!(command_priority(rtl()), Priority::Emergency);
        assert_eq!(
            command_priority(MavCmd::MAV_CMD_NAV_LAND),
            Priority::Emergency
        );
        assert_eq!(
            command_priority(MavCmd::MAV_CMD_COMPONENT_ARM_DISARM),
            Priority::Emergency
        );
        for normal in [
            goto(),
            MavCmd::MAV_CMD_NAV_TAKEOFF,
            MavCmd::MAV_CMD_DO_SET_MODE,
            MavCmd::MAV_CMD_DO_SET_HOME,
        ] {
            assert_eq!(command_priority(normal), Priority::Normal, "{normal:?}");
        }
        assert!(Priority::Emergency.is_emergency() && !Priority::Normal.is_emergency());
    }

    #[test]
    fn emergency_preempts_a_normal_command() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin_with_priority(goto(), [0.0; 7], Priority::Normal)
            .unwrap();
        assert_eq!(s.in_flight_priority(), Some(Priority::Normal));

        let (events, frames) = s
            .begin_with_priority(rtl(), [1.0; 7], Priority::Emergency)
            .unwrap();
        assert_eq!(
            events,
            vec![
                // The displaced command is reported first: its caller learns
                // it ended instead of waiting out the retry budget.
                CommandEvent::Failed {
                    command: goto(),
                    error: CommandError::Superseded,
                },
                CommandEvent::Sent {
                    command: rtl(),
                    confirmation: 0,
                },
            ]
        );
        assert_eq!(frames.len(), 1, "one frame: the new COMMAND_LONG");
        let MavMessage::COMMAND_LONG(c) = &frames[0] else {
            panic!("expected COMMAND_LONG")
        };
        assert_eq!(c.command, rtl());
        assert_eq!(c.confirmation, 0, "the preempting command starts fresh");
        assert_eq!(c.param1, 1.0);
        assert_eq!(s.in_flight_priority(), Some(Priority::Emergency));
    }

    #[test]
    fn only_an_emergency_may_preempt() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin_with_priority(goto(), [0.0; 7], Priority::Normal)
            .unwrap();
        // A normal command behind a normal one still waits.
        assert_eq!(
            s.begin_with_priority(rtl(), [0.0; 7], Priority::Normal),
            Err(CommandError::Busy)
        );
    }

    #[test]
    fn an_emergency_command_is_never_preempted() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin_with_priority(rtl(), [0.0; 7], Priority::Emergency)
            .unwrap();
        assert_eq!(
            s.begin_with_priority(MavCmd::MAV_CMD_NAV_LAND, [0.0; 7], Priority::Emergency),
            Err(CommandError::Busy),
            "two emergency commands still queue: one session, one ack"
        );
        assert_eq!(
            s.begin_with_priority(goto(), [0.0; 7], Priority::Normal),
            Err(CommandError::Busy)
        );
        assert_eq!(s.in_flight_priority(), Some(Priority::Emergency));
    }

    #[test]
    fn a_preempted_command_stops_retransmitting_and_its_late_ack_is_ignored() {
        let mut s = CommandSession::new(FC_SYS, FC_COMP);
        s.begin_with_priority(goto(), [0.0; 7], Priority::Normal)
            .unwrap();
        let (_, frames) = s
            .begin_with_priority(rtl(), [0.0; 7], Priority::Emergency)
            .unwrap();
        assert_eq!(frames.len(), 1);

        // The retransmit carries the new command, never the superseded one.
        let (events, frames) = s.retransmit_due(Instant::now() + COMMAND_RETRY_TIMEOUT);
        assert_eq!(
            events,
            vec![CommandEvent::Sent {
                command: rtl(),
                confirmation: 1,
            }]
        );
        let MavMessage::COMMAND_LONG(c) = &frames[0] else {
            panic!("expected COMMAND_LONG")
        };
        assert_eq!(c.command, rtl());
        assert_eq!(
            s.take_timeout_failure(),
            None,
            "the superseded command must not fail the *new* session"
        );

        // A late ack for the superseded goto changes nothing.
        let (events, _) = s.handle(&fc_header(), &ack(goto(), MavResult::MAV_RESULT_ACCEPTED));
        assert!(events.is_empty());
        assert_eq!(s.in_flight_priority(), Some(Priority::Emergency));

        // The new command's own ack still completes.
        let (events, _) = s.handle(&fc_header(), &ack(rtl(), MavResult::MAV_RESULT_ACCEPTED));
        assert_eq!(
            events,
            vec![CommandEvent::Completed {
                command: rtl(),
                result: CommandResult::Accepted,
            }]
        );
        assert!(s.is_idle());
    }
}
