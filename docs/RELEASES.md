# Releases and installers

## Release workflow

`.github/workflows/ci.yml` runs when a `v*` tag is pushed. A branch push alone
does not build new installers. Each Windows/Ubuntu installer job depends on
all three check jobs succeeding:

- Rust formatting, Clippy, workspace tests, and committed ts-rs binding sync.
- `cargo deny`: dependency advisories, licenses, bans, and sources.
- Frontend production dependency audit, typecheck, UI/pure-module checks,
  and production build.

The installer jobs append their files to the same draft GitHub release.
Actions artifacts contain only the finished installers. Windows builds
explicitly select `nsis,msi`; Ubuntu builds select `appimage,deb`.

When preparing a release tag, keep the Cargo workspace version, Tauri config
version, and frontend package/lockfile versions equal to the tag without its
`v` prefix. The fixes below ship as `v1.0.2`, with installer version `1.0.2`.

## Check failures diagnosed on 2026-10-23

The [v1.0.0 run](https://github.com/leohaijunli/rgcs/actions/runs/39978640605)
failed at these steps:

- **Rust:** `cargo fmt --check`, in `actuator_service.rs` and
  `core/src/logs/protocol.rs`. Clippy and tests were skipped in that run.
- **Frontend:** the production dependency audit, before typechecking ran.
  Cesium's transitive DOMPurify `3.4.16` had advisories; the lockfile now
  resolves `3.4.17`.
- **Dependency check:** local reproduction with the pinned cargo-deny
  `0.20.2` reported Tokio `RUSTSEC-2026-0386` and ZIP
  `RUSTSEC-2026-0367`/`0368`/`0369`. Licenses, bans, and sources passed.
  Tokio is updated to `1.53.3` and ZIP to `9.0.5`. ZIP's fallible entry-name
  decoding is propagated through the existing archive error type.

The `v1.0.1` follow-up passed formatting, Clippy, dependency checks, and the
frontend job, then exposed the terrain helper discovery test's missing
prerequisite. Before running the workspace tests, CI now builds the regular
helper with `cargo build -p dem-prep --bin dem-prep`. A test harness alone does
not provide the executable that discovery expects in the profile directory.

To reproduce the Rust job locally, build the frontend, then run:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build -p dem-prep --bin dem-prep
cargo test --all-targets
```

## Choosing an installer

| Platform | Format | Runtime packaging |
| --- | --- | --- |
| Windows | NSIS `setup.exe` or `.msi` | Uses the system WebView2 runtime, or downloads its bootstrapper if needed |
| Ubuntu | `.deb` | Installs the application and uses distribution-managed runtime dependencies |
| Linux portable | `.AppImage` | Bundles WebKitGTK/GTK and many runtime libraries, making it larger |

The v1.0.0 Actions artifact archives were **39,503,543 bytes** for Windows
and **302,876,194 bytes** for Ubuntu. Those are the sizes of the entire
artifact ZIPs, not individual installer sizes. The old upload pattern,
`target/release/bundle/**`, collected whole packaging directories, including
intermediate files such as AppImage staging trees. Uploads now select only
`.exe`/`.msi` or `.deb`/`.AppImage` files. New artifact sizes can be compared
after the next tagged build.

For Ubuntu, download the `.deb` asset directly when portability is not
needed. The larger AppImage is expected because its runtime libraries travel
with the application; see [Tauri's AppImage guide](https://v2.tauri.app/distribute/appimage/).

## Windows download warnings

Windows installers currently have no Authenticode code-signing setup.
SmartScreen and browsers can show an unknown-publisher, unrecognized-app,
or download-reputation warning for unsigned/new executables. That warning
alone does not identify a malware detection, and passing CI does not remove
publisher/reputation warnings.

For public distribution, sign both the application executable and its
installers with a trusted Windows code-signing certificate/service and a
timestamp. A newly signed file can still need time to build SmartScreen
reputation. See [Tauri's Windows signing guide](https://v2.tauri.app/distribute/sign/windows/).

If the warning names a specific malware threat, record its exact text and
the downloaded filename so that detection can be investigated directly.

## Windows Motor test black/unresponsive window

The window-opening commands were synchronous. Tauri documents a Windows
WebView2 deadlock when creating a webview from a synchronous command: the
new window can remain black and the UI thread cannot process its close
button. `actuator_open`, `inspector_open`, `params_open`, and `map_open` now
run as asynchronous Tauri commands.

Install a build containing this change and follow the
[Windows verification steps in README](../README.md#windows-tool-window-verification):
render within 5 seconds, close within 2 seconds, repeat open/close three
times, and confirm that the main window stays responsive. This native
Windows check is separate from the Rust tests and browser-based UI checks.
