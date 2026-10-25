//! Shared PX4 NuttShell exchange over MAVLink `SERIAL_CONTROL`.
//!
//! The motor-test capture service (dedicated serial target) and the primary
//! link's log deletion share the same wire mechanics: frame a complete shell
//! line, require a unique marker line to delimit the response, and parse the
//! known PX4 outputs (`logger status`, the SD log index). The transport is
//! link-agnostic — callers pass the connection, the target ids and their own
//! route subscription.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use maggcs_core::logs::capture::SdCaptureError;
use maggcs_core::logs::LogEntryInfo;
use maggcs_core::mavlink::connection::{ConnectionEvent, ConnectionHandle};
use maggcs_core::mavlink::router::RoutedEvents;
use maggcs_core::mavlink::MavMessage;

/// `SERIAL_CONTROL.data` capacity (MAVLink message definition).
pub(crate) const SHELL_PAYLOAD: usize = 70;
/// PX4 SD-card log directory.
pub(crate) const LOG_DIRECTORY: &str = "/fs/microsd/log";
const SHELL_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SHELL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const SHELL_MAX_OUTPUT: usize = 256 * 1024;
/// Long enough for one prompt round trip, short enough to keep the
/// pre-delete shell check snappy.
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// PX4 starts the shell on the first command and drops it, so the probe sends
/// a throwaway line and waits this long before the command that is read back.
const SHELL_WARMUP: Duration = Duration::from_millis(2000);
/// A NuttShell comment: both shells ignore it, but the reply still carries the
/// prompt, which is all the probe needs.
const SHELL_PROBE_COMMAND: &str = "# maggcs-shell-probe";

/// Which shell PX4 exposes over the MAVLink shell on a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellKind {
    /// NuttShell (`nsh`), the NuttX flight-controller shell: has `cat`, `rm`,
    /// `ls` and `echo`, so the marker exchange and SD-log deletion work.
    NuttShell,
    /// PX4's POSIX shell (`pxh`), used by SITL: it has neither `echo` (the
    /// exchange marker) nor the NuttShell file tools.
    Pxh,
    /// No prompt observed within [`SHELL_PROBE_TIMEOUT`].
    Unknown,
}

fn shell_flags() -> ::mavlink::common::SerialControlFlag {
    ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
        | ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
}

/// Frame a complete NuttShell line without truncating long file paths.
pub(crate) fn shell_frames(
    line: &str,
    target_system_id: u8,
    target_component_id: u8,
) -> Vec<MavMessage> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    bytes
        .chunks(SHELL_PAYLOAD)
        .map(|chunk| {
            let mut data = [0u8; SHELL_PAYLOAD];
            data[..chunk.len()].copy_from_slice(chunk);
            MavMessage::SERIAL_CONTROL(::mavlink::common::SERIAL_CONTROL_DATA {
                device: ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
                flags: shell_flags(),
                timeout: 0,
                baudrate: 0,
                count: chunk.len() as u8,
                data,
                target_system: target_system_id,
                target_component: target_component_id,
            })
        })
        .collect()
}

pub(crate) async fn send_lines(
    handle: &ConnectionHandle,
    generation: u64,
    target_system_id: u8,
    target_component_id: u8,
    lines: Vec<String>,
) -> Result<(), SdCaptureError> {
    let deadline = tokio::time::Instant::now() + SHELL_WRITE_TIMEOUT;
    tokio::time::timeout(SHELL_WRITE_TIMEOUT, async {
        for line in lines {
            for frame in shell_frames(&line, target_system_id, target_component_id) {
                handle
                    .send_in_generation(frame, generation, deadline)
                    .await
                    .map_err(|error| SdCaptureError::Transport {
                        message: error.to_string(),
                    })?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| SdCaptureError::Transport {
        message: "timed out confirming target shell writes".into(),
    })?
}

static SHELL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Execute one command and return its output. Completion requires the marker
/// output line, so a lost reply is a timeout rather than a silent success.
pub(crate) async fn exchange(
    handle: &ConnectionHandle,
    target_system_id: u8,
    target_component_id: u8,
    route: &mut RoutedEvents,
    command: &str,
) -> Result<String, SdCaptureError> {
    let marker = format!(
        "__MAGDEL_{:08x}_{}__",
        std::process::id(),
        SHELL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    send_lines(
        handle,
        handle.generation(),
        target_system_id,
        target_component_id,
        vec![String::new(), command.to_string(), format!("echo {marker}")],
    )
    .await?;

    let deadline = Instant::now() + SHELL_RESPONSE_TIMEOUT;
    let mut output = String::new();
    let mut line_buffer = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(SdCaptureError::ShellTimeout {
                command: command.into(),
            });
        }
        let event = tokio::time::timeout(remaining, route.recv())
            .await
            .map_err(|_| SdCaptureError::ShellTimeout {
                command: command.into(),
            })?;
        match event {
            Ok(ConnectionEvent::Message(envelope)) => {
                let MavMessage::SERIAL_CONTROL(message) = envelope.message else {
                    continue;
                };
                let Some(chunk) = reply(
                    &message,
                    handle.config().system_id,
                    handle.config().component_id,
                ) else {
                    continue;
                };
                line_buffer.push_str(&chunk);
                if output.len() + line_buffer.len() > SHELL_MAX_OUTPUT {
                    return Err(SdCaptureError::UnexpectedResponse {
                        output: "target shell output exceeded the limit; no complete response"
                            .into(),
                    });
                }
                while let Some(pos) = line_buffer.find('\n') {
                    let line: String = line_buffer.drain(..=pos).collect();
                    let line = clean_line(&line);
                    if line.trim() == marker {
                        return Ok(output);
                    }
                    output.push_str(&line);
                    output.push('\n');
                }
            }
            Ok(ConnectionEvent::LinkError(_))
            | Ok(ConnectionEvent::Failed(_))
            | Ok(ConnectionEvent::HeartbeatLost { .. }) => {
                return Err(SdCaptureError::TargetUnavailable);
            }
            Ok(ConnectionEvent::Connected { .. }) => return Err(SdCaptureError::TargetChanged),
            Ok(_) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                return Err(SdCaptureError::UnexpectedResponse {
                    output: "target shell response lost frames; result is unconfirmed".into(),
                });
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(SdCaptureError::TargetUnavailable);
            }
        }
    }
}

/// Classify the shell from its prompt text. `clean_line` has already stripped
/// the newlines, so the prompt token and its echoes concatenate safely.
pub(crate) fn classify_shell_text(text: &str) -> ShellKind {
    if text.contains("pxh>") {
        ShellKind::Pxh
    } else if text.contains("nsh>") {
        ShellKind::NuttShell
    } else {
        ShellKind::Unknown
    }
}

/// Ask the shell to emit a prompt and classify it. Deletion drives NuttShell
/// file tools, so a POSIX `pxh` link (SITL) is refused here instead of letting
/// every [`exchange`] time out waiting for a marker `echo` that `pxh` rejects.
pub(crate) async fn probe_shell(
    handle: &ConnectionHandle,
    target_system_id: u8,
    target_component_id: u8,
    route: &mut RoutedEvents,
) -> Result<ShellKind, SdCaptureError> {
    let generation = handle.generation();
    send_lines(
        handle,
        generation,
        target_system_id,
        target_component_id,
        vec![String::new()],
    )
    .await?;
    // Discard the shell's first (dropped-command) reply, then read the reply to
    // a harmless command, whose prompt identifies the shell.
    read_shell_text(route, handle, SHELL_WARMUP, false).await?;
    send_lines(
        handle,
        generation,
        target_system_id,
        target_component_id,
        vec![SHELL_PROBE_COMMAND.to_string()],
    )
    .await?;
    let text = read_shell_text(route, handle, SHELL_PROBE_TIMEOUT, true).await?;
    Ok(classify_shell_text(&text))
}

/// Read reply chunks into one cleaned string until `window` elapses, or a
/// shell kind is recognised when `stop_on_kind` is set.
async fn read_shell_text(
    route: &mut RoutedEvents,
    handle: &ConnectionHandle,
    window: Duration,
    stop_on_kind: bool,
) -> Result<String, SdCaptureError> {
    let deadline = Instant::now() + window;
    let mut text = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(text);
        }
        match tokio::time::timeout(remaining, route.recv()).await {
            Ok(Ok(ConnectionEvent::Message(envelope))) => {
                let MavMessage::SERIAL_CONTROL(message) = envelope.message else {
                    continue;
                };
                let Some(chunk) = reply(
                    &message,
                    handle.config().system_id,
                    handle.config().component_id,
                ) else {
                    continue;
                };
                text.push_str(&clean_line(&chunk));
                if stop_on_kind && classify_shell_text(&text) != ShellKind::Unknown {
                    return Ok(text);
                }
            }
            Ok(Ok(ConnectionEvent::LinkError(_)))
            | Ok(Ok(ConnectionEvent::Failed(_)))
            | Ok(Ok(ConnectionEvent::HeartbeatLost { .. }))
            | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                return Err(SdCaptureError::TargetUnavailable);
            }
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Err(_) => return Ok(text),
        }
    }
}

/// Drop ANSI escape sequences and control characters from one shell line.
pub(crate) fn clean_line(raw: &str) -> String {
    let mut chars = raw.chars().peekable();
    let mut line = String::new();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for character in chars.by_ref() {
                if ('@'..='~').contains(&character) {
                    break;
                }
            }
        } else if !character.is_control() || character == '\t' {
            line.push(character);
        }
    }
    line
}

/// The first NuttShell error line in a command's output, if any.
pub(crate) fn command_error(output: &str) -> Option<&str> {
    output.lines().find(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("nsh:")
            || lower.contains("unknown command")
            || lower.contains("command not found")
            || lower.contains("permission denied")
    })
}

/// Only replies to this GCS's shell request can advance the exchange. PX4
/// serial-passthrough and another GCS's shell output are different streams.
pub(crate) fn reply(
    message: &::mavlink::common::SERIAL_CONTROL_DATA,
    gcs_system_id: u8,
    gcs_component_id: u8,
) -> Option<String> {
    if message.device != ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL
        || !message
            .flags
            .contains(::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY)
        || (message.target_system != 0 && message.target_system != gcs_system_id)
        || (message.target_component != 0 && message.target_component != gcs_component_id)
        || message.count == 0
        || usize::from(message.count) > SHELL_PAYLOAD
    {
        return None;
    }
    Some(String::from_utf8_lossy(&message.data[..usize::from(message.count)]).into_owned())
}

/// Parse PX4's `logdata.txt` lines (`mtime size path`) into safe log paths.
/// PX4 writes these lines in the same order it assigns MAVLink log ids.
pub(crate) fn log_path_from_listing(listing: &str, entry: &LogEntryInfo) -> Result<String, String> {
    if let Some(error) = command_error(listing) {
        return Err(format!("cannot read PX4 log index: {error}"));
    }
    let mut index = 0u16;
    for line in listing.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(first) = fields.first() else {
            continue;
        };
        let Ok(time_utc) = first.parse::<u32>() else {
            continue;
        };
        if fields.len() != 3 {
            return Err("PX4 log index contains a malformed entry; refresh the list".into());
        }
        let size = fields[1]
            .parse::<u32>()
            .map_err(|_| "PX4 log index contains an invalid size".to_string())?;
        if index == entry.id {
            if time_utc != entry.time_utc || size != entry.size {
                return Err("SD log identity changed; refresh the list before deleting".into());
            }
            let path = fields[2];
            let relative = path.strip_prefix(&format!("{LOG_DIRECTORY}/"));
            let valid = relative.is_some_and(|relative| {
                relative.split('/').count() == 2
                    && relative
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..")
            }) && path.ends_with(".ulg")
                && path
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
            if !valid {
                return Err("PX4 returned an unsafe SD log path; deletion refused".into());
            }
            return Ok(path.into());
        }
        index = index.checked_add(1).ok_or("PX4 log index is too large")?;
    }
    Err(format!(
        "log #{} is no longer in the PX4 log index; refresh the list",
        entry.id
    ))
}

/// Silence on LOG_REQUEST_LIST is not proof of an empty SD card. Require a
/// complete recursive directory listing with no file entries instead.
pub(crate) fn parse_empty_log_directory(output: &str) -> Result<bool, String> {
    if let Some(error) = command_error(output) {
        return Err(format!("cannot verify SD logs: {error}"));
    }
    let mut saw_root = false;
    let mut has_file = false;
    for raw in output.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("nsh>") || line.starts_with("ls -R ") {
            continue;
        }
        if line == format!("{LOG_DIRECTORY}:") {
            saw_root = true;
            continue;
        }
        if line.starts_with(&format!("{LOG_DIRECTORY}/")) && line.ends_with(':') {
            continue;
        }
        if line.starts_with("echo __MAGDEL_") || line.starts_with("NuttShell ") {
            continue;
        }
        if !saw_root {
            return Err(format!("unexpected SD-directory response: {line}"));
        }
        for entry in line.split_whitespace() {
            if !entry.ends_with('/') {
                has_file = true;
            }
        }
    }
    if !saw_root {
        return Err("target did not confirm the SD log directory".into());
    }
    Ok(!has_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logdata_listing_maps_ids_to_safe_absolute_paths() {
        let listing = "1700000000 580665 /fs/microsd/log/2026-10-22/21_02_22.ulg\n1700000100 1032480 /fs/microsd/log/2026-10-22/21_26_00.ulg\n   broken line\n1700000200 1 /etc/passwd\n1700000300 1 /fs/microsd/log/../../etc/passwd.ulg\n";
        let mut entry = LogEntryInfo {
            id: 1,
            size: 1032480,
            time_utc: 1700000100,
        };
        assert_eq!(
            log_path_from_listing(listing, &entry).expect("log 1"),
            "/fs/microsd/log/2026-10-22/21_26_00.ulg"
        );
        entry.size = 42;
        assert!(
            log_path_from_listing(listing, &entry).is_err(),
            "changed identity"
        );
        entry.id = 3;
        entry.size = 1;
        entry.time_utc = 1700000300;
        assert!(
            log_path_from_listing(listing, &entry).is_err(),
            "path traversal"
        );
    }

    #[test]
    fn shell_kind_is_classified_from_the_prompt() {
        assert_eq!(classify_shell_text("nsh> "), ShellKind::NuttShell);
        assert_eq!(classify_shell_text("pxh> "), ShellKind::Pxh);
        assert_eq!(classify_shell_text(""), ShellKind::Unknown);
        // A stray "pxh" without the prompt punctuation is not a prompt.
        assert_eq!(classify_shell_text("pxh shell"), ShellKind::Unknown);
    }

    #[test]
    fn shell_frame_terminates_the_line_and_sets_shell_flags() {
        let frames = shell_frames("logger status", 42, 1);
        let MavMessage::SERIAL_CONTROL(frame) = &frames[0] else {
            panic!("serial control frame");
        };
        assert_eq!(frame.count as usize, "logger status\n".len());
        assert_eq!(&frame.data[..frame.count as usize], b"logger status\n");
        assert_eq!(frame.target_system, 42);
        assert_eq!(frame.target_component, 1);
        assert_eq!(
            frame.flags,
            ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_RESPOND
                | ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE
        );
    }

    #[test]
    fn shell_frames_preserve_long_paths_and_one_final_newline() {
        let long = "x".repeat(100);
        let frames = shell_frames(&long, 42, 1);
        assert_eq!(frames.len(), 2);
        let mut bytes = Vec::new();
        for frame in frames {
            let MavMessage::SERIAL_CONTROL(frame) = frame else {
                panic!("serial control")
            };
            assert!(usize::from(frame.count) <= SHELL_PAYLOAD);
            bytes.extend_from_slice(&frame.data[..usize::from(frame.count)]);
        }
        assert_eq!(bytes, format!("{long}\n").as_bytes());
    }

    #[test]
    fn empty_sd_directory_requires_complete_positive_evidence() {
        assert!(parse_empty_log_directory(
            "nsh> ls -R /fs/microsd/log\n/fs/microsd/log:\nnsh> echo __MAGDEL_1__\n"
        )
        .expect("empty root"));
        assert!(parse_empty_log_directory(
            "/fs/microsd/log:\n 2026-10-22/\n/fs/microsd/log/2026-10-22:\n"
        )
        .expect("empty subdirectory"));
        assert!(!parse_empty_log_directory(
            "/fs/microsd/log:\n 2026-10-22/\n/fs/microsd/log/2026-10-22:\n 21_00_00.ulg\n"
        )
        .expect("file"));
        assert!(parse_empty_log_directory("").is_err());
        assert!(
            parse_empty_log_directory("nsh: ls: stat failed: No such file or directory\n").is_err()
        );
    }

    #[test]
    fn shell_errors_and_ansi_prompt_are_not_success() {
        assert_eq!(clean_line("\u{1b}[K__MAGDEL_1__\r\n"), "__MAGDEL_1__");
        assert!(command_error("nsh: rm: unlink failed: No such file or directory\n").is_some());
        assert!(command_error("nsh> rm /fs/microsd/log/2026-10-22/21_00_00.ulg\n").is_none());
    }

    fn reply_msg(
        target_system: u8,
        target_component: u8,
        text: &str,
    ) -> ::mavlink::common::SERIAL_CONTROL_DATA {
        let mut data = [0u8; SHELL_PAYLOAD];
        data[..text.len()].copy_from_slice(text.as_bytes());
        ::mavlink::common::SERIAL_CONTROL_DATA {
            device: ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_SHELL,
            flags: ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_REPLY,
            count: text.len() as u8,
            data,
            target_system,
            target_component,
            ..Default::default()
        }
    }

    #[test]
    fn only_this_gcs_receives_target_shell_replies() {
        let accepted = reply_msg(250, 250, "INFO [logger] Not logging\r\n");
        assert_eq!(
            reply(&accepted, 250, 250).as_deref(),
            Some("INFO [logger] Not logging\r\n")
        );
        assert!(reply(&accepted, 42, 250).is_none(), "another GCS");
        let mut foreign = accepted.clone();
        foreign.flags = ::mavlink::common::SerialControlFlag::SERIAL_CONTROL_FLAG_EXCLUSIVE;
        assert!(reply(&foreign, 250, 250).is_none(), "not a reply");
        let mut other_device = accepted;
        other_device.device = ::mavlink::common::SerialControlDev::SERIAL_CONTROL_DEV_TELEM1;
        assert!(reply(&other_device, 250, 250).is_none(), "another stream");
    }
}
