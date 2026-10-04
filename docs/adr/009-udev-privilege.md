# ADR-009: udev privilege strategy — polkit + dedicated helper

- Status: Draft
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

- Wrong device: rules are bound to VID/PID and serial (or physical port path
  when no serial); generation is deterministic and previewed first.
- Wrong config: the helper validates the rule text against a strict grammar
  before writing.
- Other-path writes: rejected by path canonicalization + allow-list.

## Consequences

- Linux native is the only platform with udev; Windows/macOS only get device
  identification and port mapping. WSL2 needs usbipd-win and systemd udev
  verified separately.
- Headless mode exposes install/uninstall as CLI subcommands.