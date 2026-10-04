# ADR-001: Architecture — core library + Tauri desktop + headless server

- Status: Draft
- Date: 2026-10-03

## Decision

Split the codebase into:

- `crates/core` — a pure Rust library crate containing all domain logic
  (MAVLink, telemetry, mission, terrain, survey, RTK, devices, height).
- `crates/app-tauri` — desktop entry. Tauri is itself a Rust application, so
  the desktop app embeds `core` directly and does **not** run a separate
  backend process.
- `crates/server` — headless mode exposing REST + WebSocket over `core`.

## Rationale

- One implementation of protocol/domain logic shared by desktop and headless.
- No network round-trip between UI and logic in the desktop app.
- Frontend (React) talks to the Tauri command layer (IPC) or to the server's
  REST/WebSocket API, both backed by the same `core` crate.

## Consequences

- Everything in `core` must be backend-agnostic (no Tauri types).
- Command/REST layers are thin adapters over `core`.
- The udev helper and dem-prep tool are separate binaries, not part of `core`.