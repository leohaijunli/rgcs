# AUTO-01: shared serial discovery and automatic connection

- Date: 2026-10-25
- Status: **Implemented in software — physical acceptance pending**
- Backlog: [RTK base and air–ground TODOs](rtk-base-and-air-ground-todos.md)
- Decision record: [ADR-028](../adr/028-shared-serial-discovery.md)

This document completes AUTO-01a's source audit and presents AUTO-01b/01c's
contract, confirmed by the operator. Rust blocks are interface declarations, with private implementation
details omitted. AUTO-02 and AUTO-03 implement the confirmed contract in small
steps. Subsequent operator instructions enable automatic linking by default,
retain manual connections, and defer physical testing until software delivery.
Acceptance cases below are requirements unless an implementation result is recorded.

## 1. AUTO-01a — current paths and integration points

This table records the pre-implementation audit. The implemented API and
delivery notes below supersede the original-path observations.

| Path | Observed behavior | Shared-service integration |
| --- | --- | --- |
| [`devices/manager.rs`](../../crates/core/src/devices/manager.rs): `enumerate`, `spawn_watcher` | Enumerates Linux `ttyACM*`/`ttyUSB*` and Windows numeric `COM*`; polls every 500 ms. Diffs complete `SerialDeviceInfo` values. The handle exposes an event bus, without a retained snapshot or shutdown/join operation. A failed scan is skipped; the first successful baseline emits no Added burst. | Retain one authoritative, revisioned inventory; run blocking enumeration off the async executor; publish initial state and enumeration health; own and join the watcher. |
| [`devices/device.rs`](../../crates/core/src/devices/device.rs), [`database.rs`](../../crates/core/src/devices/database.rs) | `DeviceId` is VID/PID. `SerialDeviceInfo` adds path, transport, USB serial, manufacturer and product, but no USB-interface identifier. The database labels F9P and M8P as RTK candidates, F9R and generic bridges as Other. | Separate attachment identity, stable USB matching, role hints and protocol evidence. The RTK role alone cannot authorize F9P configuration. |
| [`app-tauri/src/lib.rs`](../../crates/app-tauri/src/lib.rs): setup | One watcher forwards `devices_changed`; each event triggers another enumeration. Broadcast lag is skipped. [`commands::enumerate_devices`](../../crates/app-tauri/src/commands.rs) and [`rtk_service::devices`](../../crates/app-tauri/src/rtk_service.rs) enumerate independently. | Initialize the shared service once after native settings load. Both selectors and compatibility events project the same snapshot; recover subscriptions from that snapshot. |
| [`commands.rs`](../../crates/app-tauri/src/commands.rs): `connect`, `resolve_target` | Opens the supplied endpoint, waits up to 5 s for opening, then up to 5 s for target IDs. Quiet links fall back to configured IDs. The configured-ID heartbeat fast path does not check the autopilot/type fields. Main link installation starts normal services and stream requests. | Reserve every serial open. Factor link installation so automatic discovery can transfer an already-open, verified transport and its IDs. Do not call the endpoint-opening IPC command again after probing. |
| [`mavlink/connection.rs`](../../crates/core/src/mavlink/connection.rs): worker | Starts GCS heartbeats on an opened transport. `candidate_autopilot_node` only excludes `MAV_AUTOPILOT_GENERIC`; `INVALID` and non-vehicle nodes are not excluded by that predicate. The configured-ID liveness path accepts any heartbeat at those IDs. Retries use the original address and a 1 s delay. | Provide a passive FC probe and one shared autopilot predicate, including configured-ID paths. Guard manual worker reopen attempts with the lease. Automatic serial retries have one supervisor and re-resolve identity on every attempt. |
| [`rtk_service.rs`](../../crates/app-tauri/src/rtk_service.rs): `rtk_start_serial`, `occupied_ports`, `ensure_port_not_base` | Re-enumerates and compares the selected full metadata; excludes known FC and occupied main/target/auxiliary ports. Entry-point checks run under `AppState::ops`. Base Stop bypasses that lock. | Replace distributed checks with atomic reservations shared by all entry points; preserve the independent, promptly cancellable base lifetime. |
| [`rtk/base.rs`](../../crates/core/src/rtk/base.rs), [`rtk/ubx.rs`](../../crates/core/src/rtk/ubx.rs), [`rtk/source.rs`](../../crates/core/src/rtk/source.rs) | One base worker opens the source. Survey-In identifies F9P, then configures RAM; preconfigured mode sends no protocol bytes. Stop has an 800 ms join budget and retains an unfinished worker. Failure ends the worker; no automatic base retry exists. | Move a lease into the actual reader worker. Keep identification and configuration on the same open stream. A supervisor starts each replacement with a new `BaseSessionId`. |
| [`actuator_service.rs`](../../crates/app-tauri/src/actuator_service.rs), [`main_link_service.rs`](../../crates/app-tauri/src/main_link_service.rs), [`state.rs`](../../crates/app-tauri/src/state.rs) | Dedicated FC and auxiliary-source commands check base ownership. Dedicated FC also checks the primary port. Main and auxiliary opens do not perform a complete symmetric ownership check against every other vehicle owner. Primary borrowing clones the existing connection/parameter service. | Register main, dedicated target, serial auxiliary source and base owners in the same registry. Borrowing a main link uses its existing lease; a second open is rejected. |
| [`lifecycle_service.rs`](../../crates/app-tauri/src/lifecycle_service.rs), `ConnectionHandle::shutdown` | Vehicle disconnect preserves the independent base. App shutdown stops it. MAVLink `shutdown()` signals cancellation but does not await worker exit; removal from `AppState` is not proof the serial handle closed. | Keep ownership in the worker until actual transport closure. Suspend new attempts before approved teardown; retain a Stopping reservation after a join timeout. |

### Persistence and frontend startup

| Existing authority | Contents / behavior | Consequence |
| --- | --- | --- |
| [`core::config::AppConfig`](../../crates/core/src/config/mod.rs), [`config_service`](../../crates/app-tauri/src/config_service.rs) | Version 1 `app_config_dir/config.json`: endpoint, launch auto-connect flag, last target sysid/type and other preferences. Default auto-connect is true for the SITL UDP endpoint. Native startup loads a cache; save writes a sibling temporary file and renames it. | The current launch flag describes one endpoint, not independent FC/RTK discovery policies. `LastTarget` is not a stable serial identity. |
| [`desktop/appConfig.ts`](../../frontend/src/desktop/appConfig.ts), [`prefs.ts`](../../frontend/src/desktop/prefs.ts) | Main webview hydrates and writes whole `AppConfig` snapshots, with localStorage bootstrap/fallback. A missing or unusable native file is represented as `None`; the owner may then save defaults. | New native-owned serial policy needs serialized, revision-checked updates and an explicit unreadable-file state. It must not be erased by an unrelated webview preference save. |
| [`desktop/bridge.ts`](../../frontend/src/desktop/bridge.ts), [`stores/autoconnect.ts`](../../frontend/src/stores/autoconnect.ts) | Startup uses a frontend 3 s cancel timer, then invokes `connect(endpoint)`. It does not search serial ports. | Native state selects the primary-link startup owner. The legacy timer applies only to an eligible nonserial endpoint; a legacy serial path is a selection/migration hint. |
| [`RtkDrawer.tsx`](../../frontend/src/components/RtkDrawer.tsx), [`stores/rtk.ts`](../../frontend/src/stores/rtk.ts) | `maggcs.rtk.survey-in` saves mode, accuracy, duration and baud locally. Selection is component state; one known candidate can be preselected. Start/Stop are explicit. | Import valid receiver settings without inventing a saved identity. Automatic eligibility follows the confirmed default-on policy; manual selection remains available. |
| [`stores/devices.ts`](../../frontend/src/stores/devices.ts) | Fetches a list and subscribes to unversioned `devices_changed`. | Subscribe before fetching the retained snapshot; reject older revisions and display current ownership from that same source. |

Native storage is **`app_config_dir/serial-connections.json`**, with
schema in `core::config` and file I/O in the desktop adapter. The application
identifier is `io.maggcs.desktop`; on Linux the usual directory is
`$XDG_CONFIG_HOME/io.maggcs.desktop` (fallback `~/.config/io.maggcs.desktop`).
Resolve it through Tauri rather than hardcoding a home directory. This sidecar
follows the existing native-owned stream-profile pattern and avoids the
whole-`AppConfig` webview writer race.

## 2. Confirmed policies and operator amendments

1. **Automatic by default, manual available:** FC and RTK automatic linking
   default to enabled for eligible, unambiguous devices. Preserve manual
   Connect/Disconnect and explicit port/baud/mode selection. A manual choice
   takes precedence. Survey-In applies the selected, identified RAM
   configuration on eligible connections, including reconnects.
2. **Saved selection wins:** wait for the saved stable identity if it is absent
   or busy. Do not fall through to another receiver/FC. With no saved selection,
   consider one unambiguous, free, known candidate for the requested role.
   F9P-capable USB candidates use a narrower allowlist than the RTK role hint.
   Bind that selected stable identity to the current intent before probing;
   retry/hotplug cannot silently select another candidate for that intent.
3. **Ambiguity:** multiple matching devices, duplicate USB serials, missing
   stable identity, or multiple eligible unsaved candidates produce
   `NeedsSelection`. Generic bridges require explicit saved selection and
   fresh protocol evidence. A path-only saved entry is a manual selection hint.
4. **Pause:** operator Disconnect/Cancel pauses that role until Connect/Resume.
   Persist the pause across app restarts. A webview reload, port event or retry
   timer cannot resume it. Disabling future auto-connect does not itself tear
   down a healthy manually established link; Disconnect is the stop operation.
5. **Single owner:** one primary FC auto intent and one base auto intent.
   Manual selection supersedes a pending automatic attempt, subject to existing
   teardown rules. An automatic attempt cannot replace a working manual link.
6. **Protocol scope:** automatic FC attachment initially accepts PX4 multirotor
   autopilots. Recognizable but unsupported autopilots are reported explicitly.
   A silent serial radio requires manual connection; passive discovery cannot
   establish its FC identity by opening alone.
7. **Recovery:** every base reconnection is a fresh session. An automatically
   discovered FC is handed to a single-transport worker; transport failure
   retires that worker and the supervisor starts a newly verified connection.
   Heartbeat loss alone does not reopen a surviving serial transport.

The operator confirmed the interfaces and subsequently changed the initial
disabled default to enabled while explicitly retaining manual connections.
Physical acceptance will be performed by the operator after code delivery.

## 3. Identity, inventory and ownership declarations

Implemented inventory/lease types live in `core::devices::{discovery, lease}`. Existing
`SerialDeviceInfo`, `DeviceId` and `DeviceRole` remain the metadata vocabulary.
Opaque IDs/revisions serialize as decimal strings, following `BaseSessionId`;
clients must not convert them to JavaScript `Number`. Runtime tokens are not
persisted.

```rust
pub struct InventoryRevision(u64);
pub struct PortInstanceId(u64);
pub struct SerialLeaseId(u64);

pub struct StableUsbId {
    pub device_id: DeviceId,
    pub serial_number: String,
}

pub struct SerialPortRef {
    pub instance_id: PortInstanceId,
    pub device: SerialDeviceInfo,
}

pub enum SerialOwner { PrimaryFc, MotorTestTarget, AuxiliarySource, RtkBase }
pub enum LeasePhase { Reserved, Probing, Active, Stopping }

pub struct SerialLeaseStatus {
    pub id: SerialLeaseId,
    pub owner: SerialOwner,
    pub phase: LeasePhase,
}

pub struct SerialPortSnapshot {
    pub port: SerialPortRef,
    pub stable_id: Option<StableUsbId>,
    pub suggested_role: DeviceRole,
    pub lease: Option<SerialLeaseStatus>,
}

pub enum InventoryHealth { Starting, Ready, Unavailable { message: String } }

pub struct SerialInventory {
    pub revision: InventoryRevision,
    pub health: InventoryHealth,
    pub ports: Vec<SerialPortSnapshot>,
}

pub struct LeaseRequest {
    pub expected: SerialPortRef,
    pub owner: SerialOwner,
}

pub trait SerialInventorySource: Send + Sync {
    fn enumerate(&self) -> Result<Vec<SerialDeviceInfo>, DeviceError>;
}

pub struct SerialDiscoveryConfig {
    pub poll_interval: Duration,
    pub enumeration_timeout: Duration,
}

pub struct SerialDiscoveryHandle { /* shared inventory and lease registry */ }
pub struct SerialLease { /* non-Clone, non-Serialize, private construction */ }

pub fn spawn_serial_discovery(
    source: Arc<dyn SerialInventorySource>,
    config: SerialDiscoveryConfig,
) -> Result<(SerialDiscoveryHandle, JoinHandle<()>), SerialDiscoveryError>;

impl SerialDiscoveryHandle {
    pub fn snapshot(&self) -> SerialInventory;
    pub fn subscribe(&self) -> watch::Receiver<SerialInventory>;
    pub async fn refresh(&self) -> Result<SerialInventory, SerialDiscoveryError>;
    pub async fn reserve(&self, request: LeaseRequest)
        -> Result<SerialLease, SerialDiscoveryError>;
    pub async fn shutdown(&self, deadline: Duration)
        -> Result<(), SerialDiscoveryError>;
}

impl SerialLease {
    pub fn id(&self) -> SerialLeaseId;
    pub fn owner(&self) -> SerialOwner;
    pub fn port(&self) -> &SerialPortRef;
}

pub fn stable_usb_id(device: &SerialDeviceInfo) -> Option<StableUsbId>;
```

### Identity and reservation rules

- Stable matching uses exact VID/PID plus a nonempty, non-whitespace-only USB
  serial string. Manufacturer/product are hints, not durable identifiers.
  The serial string remains case-sensitive. USB identity is matching evidence,
  not device authentication.
- Duplicate stable keys are ambiguous. The current enumerator has no USB
  interface discriminator, so a multi-interface collision also needs selection;
  it must not select the first port. Do not manufacture stability from a path,
  product label or VID/PID alone.
- An observed removal or identity replacement retires the attachment ID. A
  matching device returning under another path gets a new attachment ID, which
  a saved stable selector may resolve. Old selections, leases and probe results
  do not authorize the new attachment.
- A stable USB bridge identifies the bridge. F9P `MON-VER` proves model/firmware
  compatibility, not a unique receiver behind that bridge. Re-probe each new
  connection and start a new base session; do not claim physical reference
  continuity. Reference changes remain LINK-06.
- Canonicalize existing Unix path aliases and normalize Windows COM names
  before reservation. Check both the live path and stable-key claim. Retain
  the resolved alias identity while a handle is open, even if its symlink is
  removed. Only an enumerated operator port can become an automatic candidate.
- Reservation validates the selected attachment against fresh enumeration and
  atomically claims it before any serial open. Recheck on opening and before
  promotion/configuration; retire the attempt on observable identity change.
  Unrelated inventory updates do not invalidate an unchanged selection.
- Enumeration failure retains the last list as unavailable; it is not an empty
  successful scan. Suspend new automatic opens until enumeration recovers.
  Do not stop healthy sessions solely because enumeration failed.
- The actual I/O worker owns the lease. Close the handle before releasing it.
  Cancellation/timeout of a caller cannot release a lease still held by a
  blocking open, reader or writer. Late completion closes instead of publishing
  a successful session. A timed-out worker remains visibly Stopping.
- A manual MAVLink worker must validate its lease before **each** reopen. A
  different device at its old path is refused. A path-only selection cannot
  authorize automatic reopening after an observed detach. Automatic renumbering
  is resolved by the supervisor and produces a new connection configuration.

The 500 ms polling source cannot prove an unplug/replacement that occurs
entirely between scans with identical metadata. Protocol checks and new-session
rules address observable changes; native hotplug and actual hardware traces
are needed to characterize the remaining boundary.

## 4. Protocol and transport handoff declarations

Implemented additions span `core::mavlink` and `core::rtk::base`. `Duration`,
`MessageEnvelope`, `ConnectionConfig`, `ConnectionHandle`, `BaseMode`,
`BaseSessionId`, `Autopilot` and `VehicleType` refer to the existing types.

```rust
pub enum BaudSelection { Auto, Fixed(u32) }

pub struct SerialCancellation { /* cloneable shared cancellation signal */ }
impl SerialCancellation {
    pub fn new() -> Self;
    pub fn cancel(&self);
    pub async fn cancelled(&self);
}

pub struct FcIdentity {
    pub system_id: u8,
    pub component_id: u8,
    pub autopilot: Autopilot,
    pub vehicle: VehicleType,
}

pub struct FcProbeConfig {
    pub baud: BaudSelection,
    pub local_system_id: u8,
    pub expected_target: Option<FcIdentity>,
}

pub fn classify_autopilot_heartbeat(env: &MessageEnvelope) -> Option<FcIdentity>;

pub struct ProbedFcSerial { /* live transport, lease, evidence, cancellation */ }
impl ProbedFcSerial {
    pub fn target(&self) -> &FcIdentity;
    pub fn port(&self) -> &SerialPortRef;
    pub fn endpoint(&self) -> Endpoint;
    pub async fn connect(self, config: ConnectionConfig)
        -> Result<SpawnedConnection, MavlinkError>;
}

pub async fn probe_fc_serial(
    lease: SerialLease,
    config: FcProbeConfig,
    cancel: SerialCancellation,
) -> Result<ProbedFcSerial, SerialDiscoveryError>;

pub type SpawnedConnection = (
    ConnectionHandle,
    broadcast::Receiver<ConnectionEvent>,
    oneshot::Receiver<Result<(), LinkFailure>>,
);

impl ConnectionHandle {
    pub async fn shutdown_and_wait(&self, deadline: Duration)
        -> Result<(), MavlinkError>;
    pub fn target_identity(&self) -> Option<FcIdentity>;
}

pub async fn spawn_leased_serial_connection(
    config: ConnectionConfig,
    lease: SerialLease,
) -> Result<SpawnedConnection, MavlinkError>;

pub struct BaseSerialRequest {
    pub baud: BaudSelection,
    pub mode: BaseMode,
}

impl RtkBaseServiceHandle {
    pub async fn start_leased_serial(
        &self,
        lease: SerialLease,
        request: BaseSerialRequest,
        cancel: SerialCancellation,
    ) -> Result<BaseSessionId, BaseError>;
}
```

`start_leased_serial` returns the session ID after allocating the cancellable
worker, as the current base API does. Identification/configuration results
remain in `BaseStatus`. The lease and cancellation move into that worker; the
base's existing `stop(session_id)` cancels and joins it. A preconfigured source
requires an explicit saved selection and fixed baud for automatic reconnect;
its open/Ready state does not establish receiver identity or an RTK fix.

`ProbedFcSerial::connect` consumes the successful probe's **same open
transport**. It checks freshness, cancellation, attachment and matching
endpoint/IDs before enabling normal MAVLink traffic. Seed initial heartbeat
evidence into the event stream so a subscriber does not lose the probe's
selected target. Configure exact discovered IDs and disable opportunistic
retargeting for that managed session. Start telemetry, params, mission,
commands and forwarding only after the native lifecycle adapter accepts it.

The automatic serial worker exits on transport loss. Its supervisor alone
schedules the next probe/open, producing a new `handle_id`; generation starts
within that new handle. Compare **`(handle_id, generation)`**, not a bare
generation across different handles. The manual leased entry point preserves
explicit endpoint connection, with ownership/identity checks on all opens and
reopens. It is not used as the passive probe implementation.

### Autopilot evidence

- Parse complete CRC-valid MAVLink frames. Automatic attachment requires
  MAVLink 2 and two heartbeats with distinct packet sequences from the same
  system/component in the current baud's observation window. The heartbeat
  payload's `mavlink_version = 3` is not a wire-version detector.
- Require nonzero system ID, `MAV_COMP_ID_AUTOPILOT1` (component 1), an actual
  vehicle type and an autopilot other than `MAV_AUTOPILOT_INVALID`. GCS,
  onboard-controller, camera, gimbal and other peripheral types are excluded
  even if they advertise a plausible autopilot value or configured target IDs.
- Role classification and supported-platform policy are separate:
  `GENERIC` alone neither proves nor disproves an autopilot. Initial automatic
  attachment accepts PX4 with Quadrotor, Coaxial, Tricopter, Hexarotor,
  Octorotor, Dodecarotor or GenericMultirotor type. Other recognizable FCs
  report `UnsupportedAutopilot` instead of being mistaken for noise.
- Observe the full window; two eligible autopilots on one stream produce
  `AmbiguousTargets`. A saved expected target can select its matching node;
  a changed target produces `UnexpectedTarget`, without falling back to IDs
  1/1. Reject a target using the local GCS system ID (ADR-003).
- Use the common predicate in the ordinary worker's configured-ID/adoption
  paths and `resolve_target`, so an INVALID/GCS heartbeat cannot become alive
  through a fallback path. Re-probe after transport reopen; discard old parsed
  bytes, evidence and target decisions at attachment/baud boundaries.

## 5. Automatic intent, settings and native API

Schema, candidate selection, retry budgets and shared state types live in core
(`config::serial` / `devices::autoconnect`). `app-tauri::serial_service`
coordinates intent/pause transitions with the existing native link/base and
persistence lifecycles. The UI observes native
state and submits operator intent. It does not run another serial retry timer.

```rust
pub enum AutoConnectRole { PrimaryFc, RtkBase }

pub struct AutoConnectIntentRef {
    pub role: AutoConnectRole,
    pub id: String, // checked process counter, decimal string on IPC
}

pub struct SavedSerialDevice {
    pub stable_id: Option<StableUsbId>,
    pub last_device: SerialDeviceInfo, // display/selection hint, not authority
}

pub struct AutoConnectPolicy {
    pub enabled: bool,
    pub paused_by_user: bool,
    pub saved: Option<SavedSerialDevice>,
}

pub struct FcAutoConnectSettings {
    pub policy: AutoConnectPolicy,
    pub baud: BaudSelection,
    pub expected_target: Option<FcIdentity>,
}

pub struct RtkAutoConnectSettings {
    pub policy: AutoConnectPolicy,
    pub receiver: BaseSerialRequest,
}

pub struct SerialAutoConnectSettings {
    pub version: u32,
    pub fc: FcAutoConnectSettings,
    pub rtk: RtkAutoConnectSettings,
}

pub enum AutoConnectPhase {
    Disabled, Paused, WaitingForDevice, NeedsSelection, WaitingForPort,
    Probing, Starting, Running, RetryWait, Stopping, Failed,
}

pub enum SerialSessionRef {
    Base { id: BaseSessionId },
    Fc { connection_id: String, generation: String },
}

pub struct AutoConnectStatus {
    pub intent: AutoConnectIntentRef,
    pub revision: String,
    pub phase: AutoConnectPhase,
    pub attempt: Option<String>,
    pub port: Option<SerialPortRef>,
    pub session: Option<SerialSessionRef>,
    pub retry_in_ms: Option<u32>,
    pub error: Option<SerialFailure>,
}

pub struct SerialAutoConnectSnapshot {
    pub revision: String,
    pub settings_revision: String,
    pub settings: SerialAutoConnectSettings,
    pub primary_startup_owner: PrimaryStartupOwner,
    pub fc: AutoConnectStatus,
    pub rtk: AutoConnectStatus,
    pub settings_error: Option<String>,
}

pub enum PrimaryStartupOwner {
    LegacyEndpoint, SerialDiscovery, ManualSerialSelection, BlockedSettings,
}

// Desktop adapter signatures; Tauri wrappers inject AppHandle/State.
pub async fn serial_inventory() -> Result<SerialInventory, String>;
pub async fn serial_resolve_endpoint(endpoint: String) -> Result<SerialPortRef, String>;
pub async fn serial_auto_connect_status(legacy_rtk: Option<BaseSerialRequest>)
    -> Result<SerialAutoConnectSnapshot, String>;
pub async fn serial_auto_connect_save(
    expected: String,
    settings: SerialAutoConnectSettings,
) -> Result<SerialAutoConnectSnapshot, String>;
pub async fn serial_auto_connect_pause(expected: AutoConnectIntentRef)
    -> Result<AutoConnectStatus, String>;
pub async fn serial_auto_connect_resume(expected: AutoConnectIntentRef)
    -> Result<AutoConnectStatus, String>;
```

Native events: `serial_inventory` and `serial_auto_connect_status`, carrying
the corresponding complete snapshots. Legacy `enumerate_devices`,
`devices_changed` and `rtk_serial_ports` can project the shared inventory while
consumers migrate. Add generated TypeScript types and use subscribe-then-fetch
with revision checks in every window.

Manual serial IPC also carries the selected attachment: add
`expected_serial: Option<SerialPortRef>` to primary, dedicated-target and
auxiliary connection requests, required when their endpoint is serial.
RTK Start carries both the device metadata and its `port: SerialPortRef`.
Match endpoint aliases to that reference in the native adapter. A missing or
stale reference returns an actionable selection/DeviceChanged error; do not manufacture a
fresh selection from an old UI path. Update the existing manual selectors with
this foundation in AUTO-02a; later UI slices add automatic-policy controls.

The native startup owner is authoritative. `LegacyEndpoint` permits the
existing launch timer only for a nonserial endpoint when native FC discovery
is disabled. An enabled FC policy owns startup even while paused or waiting.
A legacy saved serial endpoint without a new policy is offered for explicit
selection/migration (`ManualSerialSelection`); its path does not enable an
automatic open. An unreadable serial settings file gives `BlockedSettings`
and no automatic primary connection. Manual connection remains available.

### Transitions and cancellation

| Input | Required transition / effect |
| --- | --- |
| Startup with enabled, unpaused role | Reconcile initial inventory; select saved identity or unique eligible known candidate; reserve before starting one attempt. |
| Saved device absent / lease busy | WaitingForDevice / WaitingForPort; no fallback selection and no probe of an owned stream. |
| Manual selection or settings change | Advance intent ID, cancel obsolete attempt and use the new selection only after existing lifecycle rules permit it. |
| Successful start | Running identifies the owned session; `BaseStatus` still describes identification/survey/readiness and `LinkStatus` describes FC liveness. Running is not a correction-reception claim. |
| Observable removal or retryable I/O failure | Cancel the old attempt/session; wait for actual worker closure; retry only the intended identity. A base replacement gets a new ID and empty readiness/correction history. |
| User Disconnect/Cancel | Check expected intent/session; pause scheduling immediately, cancel pending probes and run the existing session teardown. Persist pause independently of device I/O. |
| Explicit Resume/Connect | Clear pause through a revision-checked native update; create a new intent. A late prior probe, retry wakeup or Stop cannot change it. |
| FC disconnect refused by existing lifecycle guard | Preserve the current session and policy. Do not use the new pause API to bypass armed/airborne or owned-motor-release rules. |
| App shutdown | After teardown preflight succeeds, stop admitting attempts for both roles, cancel discovery work, then join session workers and inventory task. Closing the app does not itself set persistent user pause. |

Role state is native and survives webview reload. RTK Stop stays independent
of `AppState::ops`; cancellation and motor STOP/E-STOP must not wait for an
inventory scan, disk write or protocol probe. Use brief reservation/commit
sections; perform blocking enumeration and serial work outside global locks.
Recheck intent under the lifecycle commit lock before installing a probed FC.
If another owner wins during probing, close the result rather than replacing it.

The existing `rtk_stop(session_id)` adapter checks the exact current base ID
before applying role pause; an obsolete ID cannot pause a new base. The
ordinary primary `disconnect` adapter applies FC pause only after its existing
preflight guard allows teardown. A refused teardown cannot be bypassed by
calling the new pause command. Successful explicit manual Connect clears that
role's pause and invalidates any older pending automatic attempt.

Settings writes use a single native writer, validated schema, expected revision,
a unique sibling temporary file and atomic replacement. A missing file supplies
default-enabled policies. Existing explicitly saved opt-outs are preserved.
Corrupt, unreadable or newer-version files remain intact and
block automatic startup pending explicit correction. Ordinary preference saves
cannot overwrite this file. Import legacy RTK values only while creating a
missing sidecar, without inventing a saved identity.

Pause takes effect in memory before persistence. Successful pause completion
requires the pause to be saved; persistence failure reports an error while
retaining the in-process pause. Device cancellation is still delivered within
its own deadline. Resume must persist successfully before opening; late queued
settings writes cannot overwrite a newer resume/pause revision.

### Proposed timing constants

| Constant / rule | Value and measurement boundary |
| --- | --- |
| Inventory polling | 500 ms, reusing `DEFAULT_WATCH_INTERVAL`. |
| Enumeration deadline | 2 s; only one scan in flight. A late blocking scan cannot publish over a newer result or start another attempt. |
| Probe concurrency | At most one probe per role and one per leased physical port; FC and RTK can probe different ports concurrently. |
| FC baud order | Saved successful/fixed baud first; automatic order 115200, 57600, 38400, 9600, 230400, 460800, 921600, deduplicated. |
| FC evidence window / attempt | 3 s per baud, at most 25 s total from probe start, including opens and baud changes. Deadline ends the attempt even if not all rates were tried. |
| F9P baud order / identity budget | Reuse `[38400, 57600, 9600, 115200, 230400, 460800, 921600]`, at most 2 s per MON-VER and 16 s overall identification including open. Configuration retains its separate transaction/old-survey deadlines. |
| Cancellation / close | Bounded serial I/O at 100 ms; existing base join budget 800 ms; fixture cancellation/retirement observation within 1 s. A blocked OS operation retains its lease after the deadline. |
| Transient retry backoff | 1, 2, 4, 8, 16, then 30 s; cap at 30 s. Use an injected monotonic clock. Inventory churn does not reset the counter. |
| Missing device / internal busy | Wake from matching attachment / lease-release events rather than repeatedly opening an unavailable port. A new matching attachment can wake one waiting attempt. |

## 6. Errors and device-write boundary

```rust
pub enum SerialDiscoveryError {
    Enumeration(DeviceError),
    InvalidConfig { field: &'static str },
    EnumerationTimeout,
    InvalidInventory { port: String },
    IdExhausted,
    InventoryUnavailable,
    DeviceGone { instance: PortInstanceId },
    DeviceChanged { instance: PortInstanceId },
    StableIdentityRequired,
    RoleMismatch { expected: DeviceRole, observed: DeviceRole },
    AmbiguousDevices { ports: Vec<PortInstanceId> },
    PortBusy { owner: Option<SerialOwner> }, // None: external/OS owner
    PermissionDenied { port: String },
    OpenFailed { port: String, message: String },
    InvalidBaud { baud: u32 },
    ProbeTimeout { stage: String },
    NoAutopilot,
    UnsupportedMavlinkVersion,
    UnsupportedAutopilot { observed: FcIdentity },
    AmbiguousTargets { targets: Vec<FcIdentity> },
    UnexpectedTarget { expected: FcIdentity, observed: FcIdentity },
    LocalSystemIdConflict { system_id: u8 },
    Cancelled,
    StaleAttempt,
    WorkerGone,
    StopTimeout,
}

pub enum RetryDisposition { Backoff, WaitForDevice, WaitForPort, UserAction }
pub struct SerialFailure {
    pub code: String, // stable snake_case code derived from typed variants
    pub message: String,
    pub stage: String,
    pub retry: RetryDisposition,
}
```

Core errors implement `thiserror::Error`. Transport/base APIs retain
`MavlinkError`/`BaseError`; Tauri command rejection follows the existing string
error convention, while runtime status carries structured `SerialFailure`
code, stage and retry disposition. Preserve `BaseError`'s
unsupported receiver, NAK, timeout, readback mismatch and stale-session detail.
Retry classification uses the operation stage: transient open/pre-configuration
timeouts may back off. A displaced or absent saved device waits for its matching
identity, without adopting the replacement at its old path. Ambiguous/unstable
identity, unsupported protocol, unexpected FC target, permission failure and
failed configuration need user action. Do not run an endless configuration loop
after NAK/readback mismatch. Disconnect of a running eligible device can begin
the explicitly selected reconnect policy after old-worker retirement.

### Permitted effects

| Operation | Serial bytes / device effects allowed |
| --- | --- |
| Inventory, role hints, stable matching, reservation, settings save | Zero device opens and zero serial bytes; host settings writes only. |
| Automatic FC probe | Open/read, host baud changes and parser reset. Zero GCS heartbeat, stream, parameter, mission, shell, motor or GNSS configuration messages. |
| Verified FC handoff | Normal GCS heartbeat and existing link-service traffic, on the retained verified transport with selected IDs. |
| Explicit manual FC connection | Existing explicit connection traffic after reservation. Every reopen still validates ownership/selected device. |
| F9P identification | Bounded host-rate changes and UBX MON-VER polls only until supported identity is established. A read-only query is still a serial write. |
| Selected Survey-In configuration | Existing identified F9P RAM-only CFG-VALSET, matching ACK/readback, UART1 baud handoff, old-survey stop barrier and RTCM activation from ADR-027. |
| Preconfigured receiver | Read only; no MON-VER, baud-configuration command or CFG write to the receiver. Host uses the selected fixed baud. |
| Pause / release | Cancel and close owned I/O. Receiver RAM changes already applied are not rolled back; it may continue surveying/outputting independently. |

### Failure analysis before implementation

| Failure / exposure | Required handling |
| --- | --- |
| FC port mistaken for a base, including stale UI and concurrent connect | Canonical lease plus current identity checks before open; known FC excluded from RTK candidates; F9P proof before CFG. All manual/automatic entry points participate. |
| Generic adapter connected to an unexpected device | Explicit saved adapter selection; bounded protocol-specific probe on each attachment. Model evidence does not establish receiver uniqueness behind the adapter. |
| Unknown device interprets query bytes or line transitions | Never UBX-probe arbitrary free bridges automatically. Inventory is passive. Opening/baud changes may affect DTR/RTS electrically even when zero bytes are sent; avoid deliberate reset pulses and measure installed-device behavior in FIELD-01. |
| QGC/another process already uses the port | Respect OS exclusive-open/busy errors; record external busy without guessing a PID. The in-process registry cannot discover every external open or guarantee OS exclusivity. No automatic permission escalation. |
| Cancellation while a blocking worker opens/writes | Worker retains lease; no replacement opener until actual close; late success is discarded. |
| Unplug during partial receiver configuration | Report failed stage and possible applied RAM state. A replacement session begins with fresh identity and the confirmed reset/configuration sequence. |
| Pause races timer, native reload or file save | Intent/attempt guards invalidate old effects; native pause is immediate, persisted by one revisioned writer; failed persistence is visible. |
| Port disappears/reappears under another name | Match saved stable identity, new attachment/session, no stale correction replay; canonical aliases cannot create another owner. |
| Survey restarts at another reference location | Clear readiness and session-scoped data. Automatic re-survey is part of the enabled mode's write scope; mission-time reference-change behavior requires LINK-06 evidence/policy. |

## 7. Measurable acceptance matrix

Use a scripted inventory source, fake monotonic clock, open/close counters and
wire captures. Serial integration uses PTYs/injected transports without
relaxing the production operator-port filter. Each case asserts actual effects
and state, including negative open/write counts.

| ID | Layer / scenario | Required result |
| --- | --- | --- |
| AC-01 | Inventory: initial device, failed scan, missed/coalesced events | Initial snapshot contains attached devices without waiting for an Added event. Failure preserves unavailable cached data; next successful snapshot reconciles the full set. At most one scan runs concurrently. |
| AC-02 | Ordering: old refresh/event arrives last | Snapshot revisions strictly order changes; delayed results cannot replace a newer list or lease state. |
| AC-03 | Identity: A unplugged, A returns as `ttyACM2`; B takes A's old path | Exactly one new A session; zero configuration bytes to B; old selection/probe/session IDs rejected. |
| AC-04 | Missing/blank/duplicate USB serial, two interfaces with same key, multiple candidates | NeedsSelection with all matching ports shown; zero automatic opens. Manual choice may open once but cannot authorize identity-free reconnect. |
| AC-05 | Saved device absent/busy while another valid candidate exists | Wait for the saved device; zero probes/configuration on the alternate candidate. |
| AC-06 | Ownership: race main, dedicated FC, serial auxiliary and RTK; test canonical alias and COM normalization | One lease/open winner per physical port; every loser reports the owner. Borrowing main adds zero opens; reverse-order main/target acquisition is also covered. |
| AC-07 | Stop/open timeout, shutdown signal before actual worker exit | No second open while the first worker retains I/O. Stopping remains visible until close; actual close precedes lease release. |
| AC-08 | RTK: supported F9P, F9R, M8P, bridge, wrong baud and noise | Only supported F9P evidence permits Survey-In CFG. Unsupported/noise transcripts have zero CFG writes. Auto identification respects the 16 s total budget; mode defaults encode 20000 / 180 and RAM layer 1. |
| AC-09 | RTK configuration NAK/readback mismatch; passive mode | Failure names the stage and does not retry configuration without user action. Preconfigured operation has exactly zero transmitted serial bytes. |
| AC-10 | RTK reconnect with FC absent, then FC disconnect/reconnect | New base ID after receiver reopen, no old readiness/corrections. Vehicle-only disconnect preserves current base ID; forwarding resumes with fresh frames only. |
| AC-11 | Retry clock and cancellation | Attempt delays are 1/2/4/8/16/30/30 s; unrelated hotplug cannot reset them. Cancelled probe settles within 1 s in bounded-I/O fixtures; no obsolete timer starts another open. |
| AC-12 | Pause from idle wait, probe, config, running and backoff; reload/restart/resume | No further automatic opens while paused, including after reload/restart. Resume creates one current intent/attempt. An old Stop/Pause cannot affect the replacement. |
| AC-13 | FC positive: PX4 multirotor sysid 42/component 1, sequence wrap | Two fresh distinct heartbeat sequences identify 42/1; 255 → 0 is valid. Selected IDs reach connection config and normal services. Probe sends zero bytes; promotion performs zero additional opens. |
| AC-14 | FC negative: GCS/INVALID, GCS/PX4, onboard controller, wrong component, GNSS, noise, bad CRC, MAVLink 1, repeated same frame | None auto-attaches. GCS/INVALID at configured 1/1 also fails worker/resolve-target liveness paths. Recognized unsupported autopilots are reported distinctly. |
| AC-15 | FC: two autopilots, expected-target mismatch, local-ID conflict | AmbiguousTargets without saved selection; exact saved target can select its node. Mismatch/conflict sends zero normal-session bytes. |
| AC-16 | FC probe timeout, removal during promotion, managed reopen | Probe finishes by 25 s; stale result is closed. Reopen uses new identity/protocol evidence and a new handle; no second retry loop or duplicate session. |
| AC-17 | Persistence: missing/corrupt/newer file, concurrent windows, legacy bootstrap, delayed writes | Default-enabled policies only for a missing file; saved opt-outs and unusable existing bytes preserved. Stale revision rejected; unrelated AppConfig save cannot erase policies. Failed Resume save causes zero opens; failed Pause save preserves in-process pause and reports failure. |
| AC-18 | Lifecycle: refused FC Disconnect, blocked disk/scan, app close, native startup versus legacy timer | Refusal preserves link/policy. Scan/disk work does not delay cancellation delivery or motor release. Approved shutdown admits zero new attempts. Enabled native FC policy, unreadable settings and an unmigrated legacy serial path each result in zero legacy endpoint-timer connects. |
| AC-19 | Frontend state and manual verification | Browser fixtures show waiting/busy/ambiguous/probing/paused/retry/error states and stale-event rejection. Capture 1024×640, 1920×1080 and Field/light screenshots; manually verify selection, Disconnect, Resume and cross-window ownership in packaged Tauri. |

AUTO-01c's documentation acceptance: every audited path resolves, declarations
cover inventory/lease/probe/session/settings/pause/errors, each implementation
slice maps to the matrix, and proposed versus measured results are labeled.
Check local documentation links, acceptance-ID coverage and `git diff --check`.

## 8. Small implementation slices after confirmation

AUTO-02a-1 implements retained discovery, single-flight blocking enumeration,
attachment IDs and joined shutdown. AUTO-02a-2 adds worker-owned leases and
AUTO-02a-3 wires native/manual paths. AUTO-02/03 software, fixtures and the
RTK/GPS instrument panel are implemented; recorded checks and hardware work
are in the [delivery backlog](rtk-base-and-air-ground-todos.md).

| Slice | Work | Acceptance |
| --- | --- | --- |
| AUTO-02a | Shared inventory/leases, injectable enumeration, actual-worker ownership; migrate manual serial IPC/selectors, opens and reopen checks before enabling automatic RTK. | AC-01–07; existing main-serial borrowing and base lifecycle regressions. |
| AUTO-02b | Native sidecar/policy/intent state, saved selection, RTK worker lease, retry and persistent pause. | AC-03–12, AC-17–18; existing UBX/base/forwarding regressions. |
| AUTO-02c | RTK controls and native snapshot adapters, generated bindings, README/manual steps/screenshots. | AC-19 and existing RTK browser checks. |
| AUTO-03a | Shared real-autopilot predicate, passive bounded FC probe and same-transport handoff. | AC-13–16, including configured-ID negative cases and wire/open counts. |
| AUTO-03b | FC supervisor, primary-link installation, native/legacy startup arbitration and manual override. | AC-05–07, AC-11–18; generation, target-borrowing and parameter lifecycle regressions. |
| AUTO-03c | FC discovery status/selection/Resume UI and packaged manual verification. | AC-19. |

Existing regression commands to retain when the corresponding implementation
lands include `cargo test -p maggcs-core --lib devices`,
`cargo test -p maggcs-core --lib rtk`,
`cargo test -p maggcs-core --test connection_integration`,
`cargo test -p maggcs-core --test rtk_service_integration`,
`cargo test -p maggcs-app --lib rtk`, and
`cargo test -p maggcs-app --lib main_link_service`. Add focused fake-clock and
PTY coverage for the new matrix rather than treating existing passes as proof
of automatic discovery. Frontend slices require typecheck/build/i18n and
browser verification; document their actual commands and screenshots.

## 9. Physical dependencies and LINK-05 decision

Before LINK-01–04, LINK-06 and FIELD-01, record base/rover models and firmware,
installed PX4 version, radio models/firmware/rates, each serial baud, native
USB versus adapter/radio topology, and QGC/router/correction-source routing.
The existing PX4 v1.17 SITL result does not supply the installed-hardware record.

For LINK-05, collect one timestamped record per reboot/power-cycle containing
OS presence/path/USB identity, resolved endpoint, connection `handle_id`,
`generation`, transport-open time, connection events, heartbeat system/component
and available boot-time observations. Proposed repeatability target: **five
software reboots and five power cycles per installed serial topology**. Record
failures/missing signals as well as successful reconnections; user-triggered
GCS reconnect must be distinguishable from a physical transport failure.

Evaluate the full handle/generation tuple. If every physical reboot retires
the transport, verify state invalidation/recovery at that boundary. If any FC
restart leaves the serial handle/generation alive, use that trace to propose a
separate vehicle boot-epoch contract, including wrap/reordering behavior. A
heartbeat gap alone is not evidence of a new vehicle boot. Keep the independent
base session throughout vehicle-only experiments.

Hardware trace acquisition and the boot-epoch decision remain pending. The
current AUTO signatures intentionally model attachment, attempt, base session
and transport identity; they make no unmeasured claim about FC boot identity.

## 10. Submission verification — 2026-10-25

- Documentation validation covered the five submission files: 51 local links
  and two heading anchors resolved; fenced declaration blocks were balanced.
- AC-01–AC-19 are 19 unique cases, all mapped to implementation slices. All
  seven contract areas have declarations; proposal/backlog statuses agree.
- `git diff --check` passed, with additional `git diff --no-index --check`
  checks for the new/untracked submission documents.

These checks validate the design submission. Runtime and physical acceptance
remain the matrix and hardware work described above.

## 11. HUD and field evidence additions

The operator confirmed the RTK/GPS HUD at the top of the existing drawer.
`RtkGpsDashboard` takes `BaseStatus`, `RtkStats`, `GpsRawInt` and base/rover ages.
It renders receiver-reported fix state, satellites, dimensionless DOP, correction
age, bounded local-TX history and two independent survey progress indicators.
Missing/stale values remain explicit; both bars reaching their targets cannot
substitute for receiver validity. Receiver/transport scope changes clear history.

The desktop diagnostics bundle includes `serial-state.json` and the serial
profile. The snapshot contains the actual connection handle/generation tuple,
capture time, inventory/lease state, native policy and base session/status.
The [operator checklist](../verification/serial-rtk-hardware-checklist.md) maps
20 physical cases to setup, procedure, expected result and evidence capture.

Implementation refinement: core contains discovery, leases, candidate policy,
retry budgets and protocol verification. The native adapter coordinates intent
changes with existing application lifecycle and persistence. It uses existing
`ConnectionHandle`/`BaseError`/`MavlinkError` APIs rather than a second runtime
wrapper, and publishes structured runtime failures while retaining conventional
Tauri command error strings.
