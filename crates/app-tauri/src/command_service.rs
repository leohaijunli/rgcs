//! Command service adapter: drives `core::commands::CommandService` on the
//! live MAVLink link and forwards its outcomes to the React UI.
//!
//! Events (`"command"`): `{ command, kind, result?, message? }` with
//! `kind = sent | completed | failed`. `sent` is re-emitted for every
//! retransmission so the UI can show that the ack is still outstanding; a
//! terminal `completed`/`failed` event ends the operation.

use ::mavlink::common::MavCmd;
use maggcs_core::commands::{CommandEvent, CommandResult, CommandService};
use maggcs_core::mavlink::connection::ConnectionHandle;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Payload forwarded to the webview on every `"command"` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CommandEventPayload {
    pub command: &'static str,
    pub kind: &'static str,
    pub result: Option<&'static str>,
    pub message: Option<String>,
}

/// Map a `MAV_CMD` onto the stable name shared with the UI.
pub fn command_name(command: MavCmd) -> &'static str {
    match command {
        MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH => "rtl",
        _ => "unknown",
    }
}

/// Map a `CommandResult` onto the UI vocabulary.
pub fn result_name(result: CommandResult) -> &'static str {
    match result {
        CommandResult::Accepted => "accepted",
        CommandResult::TemporarilyRejected => "temporarily_rejected",
        CommandResult::Denied => "denied",
        CommandResult::Unsupported => "unsupported",
        CommandResult::Failed => "failed",
        CommandResult::InProgress => "in_progress",
        CommandResult::Cancelled => "cancelled",
        CommandResult::CommandLongOnly => "command_long_only",
        CommandResult::CommandIntOnly => "command_int_only",
        CommandResult::CommandUnsupportedMavFrame => "unsupported_mav_frame",
    }
}

/// Serialize a core event into the UI payload.
fn payload(e: &CommandEvent) -> CommandEventPayload {
    match e {
        CommandEvent::Sent {
            command,
            confirmation,
        } => CommandEventPayload {
            command: command_name(*command),
            kind: "sent",
            result: None,
            message: (*confirmation > 0).then(|| format!("retry {confirmation}")),
        },
        CommandEvent::Completed { command, result } => CommandEventPayload {
            command: command_name(*command),
            kind: "completed",
            result: Some(result_name(*result)),
            message: None,
        },
        CommandEvent::Failed { command, error } => CommandEventPayload {
            command: command_name(*command),
            kind: "failed",
            result: None,
            message: Some(error.to_string()),
        },
    }
}

/// Start the core command service on `handle` and forward its events to the
/// `"command"` Tauri event. The returned handle is the caller's way to
/// enqueue commands; dropping it stops the task.
pub fn spawn(
    app: AppHandle,
    handle: ConnectionHandle,
    target_sys: u8,
    target_comp: u8,
) -> CommandService {
    let (service, mut events_rx) = CommandService::spawn(handle, target_sys, target_comp);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events_rx.recv().await {
            let _ = app.emit("command", payload(&event));
        }
    });
    service
}

#[cfg(test)]
mod tests {
    use super::*;
    use maggcs_core::commands::CommandError;

    #[test]
    fn maps_known_commands_and_falls_back_to_unknown() {
        assert_eq!(command_name(MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH), "rtl");
        assert_eq!(command_name(MavCmd::MAV_CMD_NAV_TAKEOFF), "unknown");
    }

    #[test]
    fn sent_payload_marks_retransmissions() {
        let first = payload(&CommandEvent::Sent {
            command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
            confirmation: 0,
        });
        assert_eq!(first.kind, "sent");
        assert_eq!(first.command, "rtl");
        assert!(first.message.is_none());

        let retry = payload(&CommandEvent::Sent {
            command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
            confirmation: 2,
        });
        assert_eq!(retry.message.as_deref(), Some("retry 2"));
    }

    #[test]
    fn completed_payload_reports_the_result() {
        let accepted = payload(&CommandEvent::Completed {
            command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
            result: CommandResult::Accepted,
        });
        assert_eq!(accepted.kind, "completed");
        assert_eq!(accepted.result, Some("accepted"));

        let denied = payload(&CommandEvent::Completed {
            command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
            result: CommandResult::Denied,
        });
        assert_eq!(denied.result, Some("denied"));
    }

    #[test]
    fn failed_payload_carries_the_reason() {
        let failed = payload(&CommandEvent::Failed {
            command: MavCmd::MAV_CMD_NAV_RETURN_TO_LAUNCH,
            error: CommandError::NoAck { attempts: 4 },
        });
        assert_eq!(failed.kind, "failed");
        assert_eq!(failed.command, "rtl");
        assert!(failed.result.is_none());
        assert!(failed.message.unwrap().contains("4"));
    }
}
