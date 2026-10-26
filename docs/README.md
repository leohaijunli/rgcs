# MagGCS documents.

- `ARCHITECTURE.md` — system architecture: crates, processes/windows,
  MAVLink data flow, height conventions, PX4 interface facts, ADR index.
- `FEATURES.md` — feature guide per view/window.
- `RELEASES.md` — release checks, installer formats and sizes, Windows
  download warnings, and tool-window troubleshooting.
- `../issues.md` — open issues, deferred work (GoPro, M0 bench checklist),
  items awaiting verification.
- `sitl-wsl-windows.md` — running PX4 SITL on Windows + WSL2 for local dev.
- `adr/` — architecture decision records 001-017 (the implemented plans —
  ui-layout, motor-test, signal-inspector, improve_plan, DEVELOPMENT_PLAN —
  were merged into these docs and the git history in 2026-10-10 cleanup;
  the design-process notes (MapView module split, telemetry channels) were
  folded into ARCHITECTURE/FEATURES and the code in 2026-10-13 cleanup).

## Linux troubleshooting

- **`ignoring unreadable manifest … missing field range_source`** — older
  motor-test recordings predate the PWM provenance/protocol fields. The current
  loader accepts these schema-v1 snapshots with explicit unknown/fallback
  metadata and preserves their recorded ranges and flags.

- **No alert sounds / `GStreamer element autoaudiosink not found`** — the
  WebKitGTK webview plays the alert audio through GStreamer. Install the
  audio plugins:

  ```bash
  sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good
  ```

- **`mavlink: skipping undecodable frame (raw frame still recorded)`** — PX4
  v1.17 emits `EVENT` frames with an enum value the dialect cannot represent.
  The GCS consumes nothing from them; the exact wire bytes stay in the tlog
  (A4), so replays and archives are unaffected. The message prints once per
  connection as noise control.
