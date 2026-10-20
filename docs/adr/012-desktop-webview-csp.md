# ADR-012: Desktop webview CSP must allow WebAssembly and eval for CesiumJS

- Status: Accepted (implemented)
- Date: 2026-10-07

## Context

The desktop shell (`crates/app-tauri`) loads `frontend/dist` through the Tauri
asset protocol with a strict CSP (`script-src 'self'`), and the flight view
renders a CesiumJS `Viewer` (ADR-010).

With `script-src 'self'` the window came up showing only the flat `--mg-bg`
colour. The bundle threw during module evaluation:

- `Refused to create a WebAssembly object because 'unsafe-eval' or
  'wasm-unsafe-eval' is not an allowed source of script` — Cesium's bundled
  Emscripten glue instantiates WebAssembly while the module is loaded.
- `EvalError: Refused to evaluate a string as JavaScript because 'unsafe-eval'
  or 'trusted-types-eval' is not an allowed source of script` — knockout, which
  `Cesium.Viewer` uses for its DOM, compiles `data-bind` expressions with
  `new Function`.

Because both failures happen while the module evaluates, React never mounts
(`#root` stays empty) and the operator sees what looks like a black window even
though the webview and the GPU path are healthy. The same `frontend/dist` served
over plain HTTP (no CSP) rendered correctly, which is why the bug was first
misattributed to WebKitGTK DMA-BUF/NVIDIA compositing.

## Decision

- `app.security.csp` in `crates/app-tauri/tauri.conf.json` keeps
  `script-src 'self' 'wasm-unsafe-eval' 'unsafe-eval'`. No other source is added
  to `script-src`, and inline script stays forbidden.
- The requirement is pinned twice: `crates/app-tauri/tests/desktop_csp.rs`
  (`cargo test -p maggcs-app`) fails if the policy is tightened, and
  `scripts/desktop-smoke.sh` launches the real binary and asserts the window
  actually paints.

## Rationale

- CesiumJS 1.120 needs both permissions. There is no supported flag that removes
  the WebAssembly requirement, and knockout's binding parser is exercised by
  `Viewer`'s own DOM, so `CesiumWidget` alone would not remove the second one.
- The webview only loads first-party assets from the asset protocol: no remote
  script origin, no user-supplied HTML, so `'unsafe-eval'` does not expose
  third-party code execution.
- The alternative — dropping `Cesium.Viewer`'s UI layer or the wasm helpers — is
  a much larger change for a desktop-only shell.

## Consequences

- `'unsafe-eval'` is a deliberate, scoped exception. It must not be copied to
  `crates/server` or any web-facing surface.
- A CesiumJS or Vite/dependency bump may change which directives are needed;
  after such bumps run `cargo test -p maggcs-app` and `scripts/desktop-smoke.sh`.
- Diagnostic rule: a window that shows only the CSS background never executed
  its JavaScript; it is not a compositing fault.
