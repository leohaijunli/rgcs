# ADR-010: Frontend stack — React + Zustand + Tailwind + Radix

- Status: Draft
- Date: 2026-10-03
- Confirmed: Rust + Tauri desktop; u-blox ZED-F9P base station; PX4 FC.

## Decision

- React + TypeScript strict mode, bundled by Vite inside Tauri.
- State: Zustand. Charts: uPlot (high-frequency realtime curves).
- Styling: Tailwind CSS + Radix primitives (unstyled), design tokens as CSS
  variables; two themes (dark tech default, high-contrast light for field
  sunlight). No hardcoded colors in components.
- 3D/2D map: Cesium.
- i18n: en + zh from day one.

## Rationale

- Tight TypeScript/`ts-rs` interop (ADR-002), fast local iteration, no online
  font/asset dependency in the field.
- Cesium is the de-facto standard for georeferenced survey display.

## Consequences

- Glass-panel effects limited to a bounded set of panels to protect Cesium
  frame rate; touch targets ≥ 44 px.
- Every view gets a Playwright screenshot baseline for visual regression.