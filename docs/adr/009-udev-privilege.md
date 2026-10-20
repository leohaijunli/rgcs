# ADR-009: udev privilege strategy — polkit + dedicated helper

- Status: Implemented (2026-10-10)
- Date: 2026-10-03

## Decision

- The main MagGCS process never runs as root.
- Installing udev rules is delegated to `helpers/udev-installer`, a small
  helper binary invoked through polkit (`pkexec`).
- The helper only ever writes files matching the application's own naming
  pattern under `/etc/udev/rules.d/` (e.g. `99-maggcs-*.rules`) and reloads
  rules via `udevadm`. Any other path or file name is rejected.
- Rules are generated and previewed in the main process; the helper is
  passed the exact rule text and target file name via stdin (not argv).

## Failure/risk analysis

- Wrong device: rules are bound to VID/PID; generation is deterministic and
  previewed in the settings dialog before the pkexec prompt.
- Wrong config: the helper validates every rule line against a strict
  grammar (SUBSYSTEM=="tty", ATTRS{idVendor/idProduct} hex4, MODE="0666" —
  nothing else) and rejects the whole batch on one bad line.
- Other-path writes: the target file name is fixed
  (`/etc/udev/rules.d/99-maggcs.rules`); argv carries only the mode word.
- Malicious stdin: shell metacharacters and `RUN+=`/`OWNER=` style directives
  are rejected by the grammar; nothing is passed to a shell.
- Recovery: uninstall deletes the file and reloads (verified); re-running
  install is idempotent.
- Rollout note (2026-10-10): the helper ships as
  `helpers/udev-installer/install.sh` and is invoked with
  `pkexec bash <script> install`; rules are generated from the enumerated
  devices' VID/PID pairs in `commands.rs` (`udev_rule_text`, unit-tested).

## Consequences

- Linux native is the only platform with udev; Windows/macOS only get device
  identification and port mapping. WSL2 needs usbipd-win and systemd udev
  verified separately.
- Headless mode exposes install/uninstall as CLI subcommands.