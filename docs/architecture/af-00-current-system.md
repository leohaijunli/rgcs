# AF-00 — Current system, ownership and extraction inventory

- Date: 2026-10-25
- Source baseline: `e0c1b45` plus the current working tree.
- Delivery owner: AF-00 in the [architecture-first plan](../plans/architecture-first-delivery-plan.md).
- Execution: the owner authorized continued implementation and waived repeated
  approval questions on 2026-10-25. Publish signatures before each implementation
  slice and retain the plan's engineering gates.
- Reference desktop: Linux x86_64, WebKitGTK, PX4 multirotor first.
- Companion evidence: [measured baseline](../verification/af-00-baseline-20261025.md)
  and [minimum contracts / AF-01 fixtures](runtime-contracts.md).

This is an inventory of current behavior. The migration destinations below are
work assignments, rather than claims that the new Runtime already exists.

## 1. Current composition and dependencies

```text
Tauri native process — lib.rs / AppState
  ├─ configuration, loaded mag calibration, diagnostics, alert pump
  ├─ SerialService (OnceCell)
  │    ├─ shared DeviceManager / inventory / non-cloneable worker leases
  │    └─ native FC and RTK automatic-intent supervisor
  ├─ primary ActiveLink — LinkId + ConnectionHandle / transport generation
  │    ├─ TelemetryHub + telemetry/health/event pump
  │    ├─ MissionService / CommandService
  │    ├─ ParamsRuntime → ParamService + retained parameter cache
  │    ├─ LogsRuntime
  │    └─ RtkRuntime ← independent RtkBaseServiceHandle
  ├─ InspectorState → live routed tap OR ULog replay → DSP Session → Channel
  ├─ InstrumentsState → primary-link runner → Channel
  ├─ MotorTestState
  │    ├─ dedicated target OR borrowed primary serial handle/ParamService
  │    ├─ separately owned SITL/source connection
  │    ├─ generation-bound permission / output sender / telemetry / scope
  │    └─ stream manager / capture / log transfer / run journal / tones
  ├─ RecorderState → raw-frame and alert feeds → bounded queue / writer thread
  └─ terrain server, one opened DEM cache, legacy drape monitor, project store

main.tsx → App → StartPage OR workspace
  ├─ telemetry bridge and process-lifetime frontend stores
  ├─ TopBar / dialogs / menus
  ├─ eager MapView → Cesium
  └─ eager RightInspector → all stage panels → mission / terrain / monitor stores

inspector.html / params.html / actuator.html → separate React/webview entries
map.html → separate view of MapView; gallery.html → design-system entry
```

The Rust services are embedded in Tauri. A WebKit renderer is a separate process,
but an independent native Runtime and bounded analysis-worker pool are future
AF-11/12 deliveries. `crates/server` is not a running headless Runtime.

## 2. Service, owner and stop inventory

Paths in this table are relative to `crates/app-tauri/src/`, unless prefixed `core/`
(meaning `crates/core/src/`). A cloned service/connection handle is not a second
physical-I/O owner.

| Resource / service | Current owner and initialization | Current stop / retirement path | Extraction owner |
| --- | --- | --- | --- |
| Main shell and native composition | `lib.rs::run`, Tauri-managed `AppState` | `commands::shutdown_app` or main native `CloseRequested`; guarded lifecycle, auxiliary cleanup, serial shutdown, `app.exit` | AF-01, AF-12 |
| Configuration / initial SITL position file | `config_service`; load at setup, main frontend hydrates before render; bridge mirrors `sitl-home.json` | No task; cached config and files survive windows. Config saves replace a sibling temporary file | AF-00 inventory; configuration UI remains in its current scope |
| Mag calibration data | `CalibrationStore`, `calibration_service`; one sidecar loaded at setup, Inspector and Motor Test share it | Explicit `set_mag_calibration(null)` clears file/store; process-scoped value otherwise survives link replacement | AF-01 context design; target binding in later domain slices |
| Diagnostics / rolling file logger / panic hook | `diagnostics_service`, setup; intentionally process-lifetime logger guard | Process exit; export is an explicit file job | AF-11/12 |
| Alert center and native event pump | `AppState.alerts`, `lib.rs::setup` | Pump exits on closed receiver; otherwise process lifetime | AF-04, AF-12 |
| Serial discovery / inventory / leases | `SerialService`, initialized once at setup or `ensure`; `core/devices/discovery` owns scans; actual I/O worker holds each `SerialLease` | `quiesce`, role cancellation and `SerialService::shutdown`; discovery joins scans and waits for leases. Timeout is not a released lease | AF-01/02, reuse ADR-028 |
| Automatic FC / base attempts | Two `RoleControl` slots inside `SerialService`; one native supervisor drives configured roles | `pause_memory`, renewed intent cancellation, `begin_manual` joins retiring attempt, shutdown joins attempts | AF-01/02; preserve native role policy |
| Primary MAVLink transport | `ActiveLink.connection`; `connect`/`connect_probed_serial` reserve then install the worker | Replacement retires old services/hub then joins old worker; lifecycle `LinkStage`; worker closure releases serial lease | AF-01 |
| Telemetry aggregation / health / UI forwarding | Primary `ActiveLink.hub`; one hub and `telemetry_pump` per installation | `hub.shutdown`, closed channels terminate pump; current forwarder tasks are not centrally counted/joined | AF-01, AF-04 |
| Mission and command services | Primary `ActiveLink`, spawned at installation | Hub/link shutdown and final handle/channel drop; core service loops also stop on closed routed bus | AF-02, AF-09 |
| Parameter protocol and event forwarder | `ActiveLink.params: ParamsRuntime`; `AppState.params` retains last read-cache handle | `ParamsRuntime::stop`: abort forwarder, request shutdown, join protocol, abort on deadline; replaced link retires its runtime | AF-01/02, AF-06 |
| Parameters webview / local staging and metadata | `params_open` focuses/builds one window; React loads bundled metadata when mounted | React unregisters listeners/timers. Native `Destroyed` currently calls **current primary `svc.cancel()`** while preserving transport/cache | AF-02/06 owner-scoped cancellation |
| Primary FC log protocol / busy reservation | `ActiveLink.logs: LogsRuntime`, current `LogsState` is process-scoped | Close command channel and join/abort worker on deadline; `logs_cancel` cancels the current primary transfer | AF-02, AF-11 |
| Primary MAVLink console / shell exclusion | `ConsoleState`, lazily attaches one watcher to the primary handle on first command | `console_close` releases shell and detaches watcher; a borrowed Motor Test target detaches without releasing its shell; next attach replaces old watcher. No explicit lifecycle stage for this watcher | AF-02, AF-12 |
| Independent base receiver | `AppState.rtk.base: RtkBaseServiceHandle`; creation is inert, reader starts on configured native role or explicit start | `rtk_stop` requires exact `BaseSessionId`; full application lifecycle stops base. Vehicle-only disconnect preserves it | AF-01/02, preserve ADR-027/028 |
| Base-status event pump | `RtkState::ensure_base_events`, once at setup | Weak owner disappears / channel closes / `RtkState::drop` aborts it | AF-04/12 |
| Vehicle RTCM forwarding / foreign-injector observer | `ActiveLink.rtk: RtkRuntime`, consumes independent base's fresh corrections | `RtkRuntime::stop` retires publication owner, aborts/joins observers, shuts down/joins forwarder; drop aborts remaining tasks | AF-01/04 |
| Inspector live tap / batcher / DSP | `InspectorState`, channel registered by window; starts tap+batcher only with an attached link | `inspector_disconnect` removes its channel/subscription/traces, signals tap/replay; producer closure ends batcher. Stop senders are retained, task joins are not | AF-03/04/05 |
| Inspector ULog replay | `InspectorState.replay`; explicit `inspector_open_ulog` opens/indexes recording, supersedes live source | Replacement, `inspector_use_live`, or window detach signals replay stop. Opening/indexing currently occurs synchronously in command | AF-05/11 |
| Walk-instrument runner | `InstrumentsState`, channel registration and primary link create runner/forwarder | `detach` signals runner; `disconnect` also drops channel; explicit primary disconnect detaches it | AF-04; preserve existing source role |
| Motor Test target transport | `TargetLinkRuntime`; dedicated `AuxiliaryLinkRuntime` owns worker, borrowed runtime owns only tool-local tasks | `actuator_disconnect_target`, `actuator_close`, native destroy, `stop_auxiliary_links`, or `main_link_service::cleanup`; borrowed stop has no transport shutdown handle | AF-01/07, ADR-026 |
| Motor Test SITL/source transport | `SitlLinkRuntime`, separately selected endpoint/node/output bank | Explicit source disconnect, auxiliary cleanup/window cleanup; stop after owned output-release confirmation | AF-01/07 |
| Motor permission / active outputs | `MotorControlGate` and `RunningMotorTest`, target+generation-bound; opening window forces OFF | OFF/reconnect/retarget/close revoke permission; `stop_active_session` confirms owned release before cleanup. Failed release retains retry evidence. Idle STOP/E-STOP does not claim others' outputs | AF-02/07, ADR-025 |
| Motor telemetry / scope flusher | Target-owned routed telemetry; `ScopeBus` is process state, flusher starts when window opens | `detach_motor_telemetry`, task abort/join, `stop_scope_flusher`; pending scope is bounded. Native destroy returns early on failed release, so follow-up resource cleanup needs explicit evidence | AF-04/07 |
| Motor stream-rate manager / preferences | `ActuatorStreams`, one target-bound worker; saved profiles shared by target endpoint/node | `stream_service::stop` drops runtime, marks inactive, aborts task; does not merge other consumers' stream demands yet | AF-04/07 |
| Motor SD capture / shell / tones | `ActuatorCaptureRuntime` per target, separate from motor driving; tone state has its own generation/cancel/start lease | `stop_actuator_capture` closes requests and joins/aborts; tone cancel is synchronous; STOP is independent of shell/capture operations | AF-02/07 |
| Motor log transfers | `ActuatorLogsRuntime` per target, latest list/error retained by `MotorTestState` | `stop_actuator_logs`; sender closure, bounded join. Native destroy and explicit close do not currently use identical cleanup sequences | AF-02/07/11 |
| Motor coordinated test / recovery journal | `ActuatorRun`, optional ticker and versioned `motor-tests/<id>/manifest.json` | Owned abort, `interrupt_active`, target reconciliation. Records survive interruption; no automatic motor restart | AF-02/07/12 |
| Flight recorder | `RecorderState`; starts a fresh session per primary link, core queue/writer plus raw/alert feeds | `stop` signals feeds and flushes/joins writer; replacement uses background `retire`. Last counters/path retained | AF-09/10/12 |
| Project store / advisory lock | `AppState.project`, explicit create/open | `project_close` drops store/lock; process exit also releases OS lock. No worker created by default | AF-00 inventory; semantic model in AF-15 |
| Shared UI state | `AppState.shared`, versioned key/value state | Process lifetime; window snapshots and writes have expected revisions | AF-03/08 |
| Detached map webview | `map_open`, on demand; presentation only | Native destroy emits `map_window: closed`; lifecycle closes it with main | AF-03/08 |
| Local terrain server | `AppState.terrain3d`, starts on explicit terrain command | `terrain3d_stop` or application `TerrainStage`; stop joins accept/serve threads via blocking pool. Main link disconnect preserves server | AF-11; raster changes in AF-17 |
| Opened DEM / height grids | `AppState.dem` caches one `Arc<CogDem>` by folder; height grids have their existing core loaders | Another folder replaces cache; process exit drops it. No byte budget/pin accounting; opening reads complete DTM/DSM while holding cache lock | AF-04/11 accounting; AF-17 terrain algorithms |
| DEM preparation / mesh/download children | `terrain_service`, explicit commands; blocking `run_stage` or async `SURVEY_CHILD` slot | `terrain3d_cancel` uses process-global flag; `survey_prepare_cancel` uses global child/cancel slot. No generic job owner/revision/deadline or lifecycle stage for every child | AF-02/11; preserve terrain algorithm until AF-17 |
| Legacy drape monitor | `AppState.drape_monitor`, React `RightInspector`/store starts/steps it against current preview | `drape_monitor_stop` takes current monitor; stage change invokes stop. Recorder remains link-owned independently | AF-09/10 |

## 3. Lifecycle routes and boundaries

### Primary installation / replacement

`commands::connect` resolves an explicit endpoint/attachment, renews native manual
intent and reserves serial ownership before opening. It waits for first-open
result, then takes `AppState.ops`, checks current intent, confirms borrowed Motor
Test releases through `before_primary_replace`, resolves FC routing and installs
`ActiveLink`. Old RTK/Params/Logs services stop before old hub/worker closure.
Recorder retirement, open Inspector and registered instruments are reattached.

`connect_probed_serial` adopts the verified live transport rather than reopening
its port. Keep the discovery lease and current intent checks intact.

### Primary disconnect / main close / normal Quit

All three routes call `lifecycle_service::run` under `AppState.ops`:

```text
MotorTest → Rtk → Logs → Params → MissionAndCommand → Recorder → Link → Terrain
```

The supervisor refuses armed/airborne teardown using fresh telemetry. A failed or
timed-out Motor Test stage aborts before transport closure. Disconnect excludes
terrain and the independent base, then cleans auxiliary links, resets Inspector
source/instruments and publishes disconnected status. Full Quit also quiesces and
joins serial discovery/attempts and exits Tauri. Main native close is single-flight
and retains refusal text until consumed by a webview.

### Borrow / detach / generation change

`main_link_service::candidate` only offers a fresh, matching primary serial PX4
node with an existing parameter runtime. `actuator_use_main_target` validates the
exact offer under lifecycle/shell exclusion and clones the primary worker and
parameter service. An attachment watcher checks handle, generation and liveness,
revokes permission and rechecks attachment identity before asynchronous cleanup.
Cleanup removes tool-local resources. It preserves the primary I/O/cache.

Dedicated bench and SITL source use separate leases/owners. Base sessions remain
independent of the flight-side forwarding attachment. A target/reference is not an
authorization to redirect an old operation to a new primary selection.

### Windows and frontend effects

Inspector and Parameters have native `Destroyed` hooks; Motor Test also revokes
permission synchronously before asynchronous cleanup. Frontend cleanup alone
does not prove native cleanup completed. Main and map lifetime rules differ.
Current tool opens focus an existing native label, but `get_webview_window` plus
build is not a per-module/target single-flight loader.

## 4. Eager work, mutable state and resource observations

| Location | Observed current behavior | Follow-up |
| --- | --- | --- |
| `frontend/src/App.tsx` | Statically imports MapView, ProfileDrawer, RightInspector and dialogs even when StartPage is rendered | AF-03 lazy workspace/map boundary |
| `RightInspector.tsx` | Statically imports all five stage panels; effect calls monitor `ensure`/`stop` based on navigation | AF-03 loaders; AF-10 monitor ownership |
| `stores/mission.ts`, `terrain.ts`, `drapeMonitor.ts` | Mixed static/dynamic cyclic imports; Vite reports that dynamic imports do not split these stores | AF-03 host/services; domain rewrite remains AF-15/17 |
| `stores/drapeMonitor.ts` | Module-level telemetry subscription; steps current mutable preview/spec/terrain and synthesizes fix wall-clock at submission | AF-09 frozen baseline; AF-10 timestamp/freshness contract |
| `stores/devices.ts`, `serialAuto.ts` | Once-per-webview watchers retain event subscriptions rather than returning a module-owned dispose handle | AF-03/04 |
| `desktop/appConfig.ts` | One init promise and process-lifetime preference subscriptions; main owner saves, tools hydrate read-only | Keep preference ownership separate from tool operation ownership |
| `inspector_service.rs` | DSP Session holder exists at setup; traces/pipelines are empty on clean idle startup. Sample channel 4096; batcher computes DSP under session lock | AF-04 bounded batch/traces/DSP budget; AF-11 isolation |
| `params/ParamsApp.tsx` | Bundled 1912-parameter metadata is fetched only on tool mount; row reads serialized, cached values/transactions currently use primary service | AF-06 retain metadata/cache semantics |
| `lib.rs::setup` | Config/calibration/logger, alert pump, base-status pump, inventory and role supervisor start without optional tool windows | Essential process services; preserve configured native roles |
| `terrain_service::open_dem_cached` | Full raster open under global cache lock, path-only cache identity | Record existing behavior; budget/job identity in AF-04/11 and source revision in AF-17 |
| `terrain_service::{run_stage,run_cancellable}` | Global cancellation state/child; synchronous pipe/file work is not a generic bounded job service | AF-02/11 |
| `AppState` / frontend stores | Parameter/alert/log/calibration/recording/DEM/monitor holders are process-wide; primary getters can select a later link | AF-01 explicit context, AF-02 operation ownership, AF-09 execution snapshot |

Native idle measurements found three processes and no optional windows, live tap,
base session, motor control, capture/run, recorder or served terrain. Browser
StartPage still loads **4,961,198 decoded JavaScript bytes** including the Cesium-
containing shared chunk. Separate Inspector/Parameters/Motor Test production
entries do not preload that heavy chunk. Registration, code loading, pipeline
allocation, view presence and operational ownership need distinct metrics.

## 5. Current command and event directory

This is the AF-00 inventory snapshot. AF-02b.4b replaces its general flight intent
entries with typed `flight_command_execute`; current owned Parameters/command
APIs and their reply authority are in [Runtime contracts](runtime-contracts.md).

`lib.rs::invoke_handler` is the authoritative registration list. Grouping here
covers its service families; the linked source remains the exact command spelling.

| API family | Commands / source | Publication / subscription |
| --- | --- | --- |
| Main connection / telemetry | `connect`, `disconnect`, `link_status`, `get_snapshot`, `enumerate_devices`, `set_sitl_home` in `commands.rs` | `telemetry`, `link`, `link_error`, `telemetry_dropped`, `statustext` |
| Native serial policy | `serial_inventory`, `serial_resolve_endpoint`, `serial_auto_connect_status/save/pause/resume` | `serial_inventory`, `devices_changed`, `serial_auto_connect_status` |
| Lifecycle / system / diagnostics | `shutdown_app`, `teardown_guard`, `take_teardown_refusal`, `clock_info`, `tools_status`, `udev_rules_present`, `install_udev_rules`, `diagnostics_export` | `teardown_refused`; system alerts |
| Flight intents / stream request | `send_command`, `send_goto`, `send_set_home`, `send_set_mode`, `send_arm_disarm`, `flight_mode_table`, `preflight_checklist`, `set_message_interval` | `command`; `statustext` correlation |
| Mission / legacy planning | `mission_upload/download/clear/set_current`, `survey_generate_sweep/cloverleaf`, `survey_compile_pattern` | `mission`, `mission_plan` |
| Configuration / calibration data | `config_load/save`, `set_mag_calibration`, `mag_calibration_status`, `geomag_declination` | Snapshot/results; no new FC calibration procedure |
| Project / persistence | `project_create/open/close/delete/status/save/list_surveys/load_survey/save_survey/list_sessions/save_session/export/import` | Command results |
| Alerts | `alert_snapshot/raise/ack/ack_all/clear_acknowledged` | `alert` |
| Inspector | `inspector_open/close/connect/disconnect/status/set_traces/catalog/list_algorithms/open_ulog/replay/replay_status/use_live/source` | Tauri `Channel<SampleFrame>`: sequence, columnar traces, spectra |
| Parameters | `params_open/pull/cancel/set/fc_version/reboot/read/cache_get/cache_len` | `params_event` |
| Motor target / source / drive | `actuator_open/close/status`, `actuator_connect_target/disconnect_target/use_main_target`, `actuator_connect_sitl/disconnect_sitl`, `actuator_refresh_mapping`, `actuator_set_motor_control`, `actuator_start_manual/start_preset/start_forward/set_values/stop/estop/reset_emergency`, alarm/tone/parameter commands | `actuator_event`, `actuator_stop_warning`, `actuator_telemetry`, `actuator_alarm`, `actuator_samples` |
| Motor auxiliary operations | `actuator_stream_catalog/status/set_rate`, `actuator_capture_verify/start/stop/status`, `actuator_run_start/status/abort`, `actuator_logs_request_list/download/delete/erase_all/cancel/status` | `actuator_capture`, `actuator_run`, `actuator_logs`; stream status is polled |
| Main console / FC logs | `console_send/close`, `logs_request_list/download/cancel/erase_all/delete` | `console_output`, `logs_event` |
| RTK base / flight-side forwarding | `rtk_start_serial/stop/serial_ports/base_status/link_status/last_stats/foreign_state` | `rtk_base_status`, `rtk_link_status`; source and injection status have separate identities |
| Recorder / instruments | `recorder_status/log_event`, `instruments_connect/disconnect/status` | Recorder files/results; `Channel<InstrumentEvent>` |
| Map / shared UI | `map_open/close/status`, `shared_state_get/snapshot/set` | `map_window`, `shared_state_changed` |
| Terrain / height / legacy drape | `terrain_*`, `terrain3d_*`, `survey_prepare*`, `geoid_sample`, `vertical_datum_offset`, `drape_preview/monitor_start/monitor_stop/monitor_ack/monitor_step/report/export/bias` in `terrain_service.rs` | `terrain3d_progress`; command results / polled monitor |

Most current control events identify operation kind or sequence, rather than a
Runtime client/operation/target context. RTK and Motor Test already carry selected
owner/generation metadata. Preserve those semantics while introducing common
envelopes; do not add a second device-write path for comparison.

## 6. Configured deadlines and queue limits

These are code/config values, not measured electrical latency guarantees.

| Boundary | Current setting | Source / existing verification |
| --- | --- | --- |
| Lifecycle | 2 s per stage; 10 s best-effort total; mandatory stages retained; failed motor release aborts teardown | `core/lifecycle`; stage ordering/deadline/failure tests; app lifecycle adapter tests |
| Motor STOP | 2 s overall, including queueing/release; normal control write 200 ms | `core/motor_test/service`, `actuator_service`; release receipt/blocked transport tests |
| Motor heartbeat / refresh | App safety envelope 1.5 s; sender/source defaults 10 Hz; primary offer heartbeat fresh ≤3 s | `motor_test_state::motor_test_limits`, `main_link_service`; stale heartbeat/generation/PTY tests |
| Serial worker write | 500 ms I/O timeout | `core/mavlink/connection`; bounded/cancellable generation-write tests |
| First connect / target discovery | Main first-open 5 s; target discovery 5 s; dedicated motor first-open 3 s | `commands`, `actuator_service`; software connection fixtures |
| Serial discovery / policy | Scan every 500 ms, enumeration deadline 2 s; supervisor tick 100 ms; settings I/O 2 s | `core/devices/discovery`, `serial_service`; coalesced scan, slow I/O and stale-intent tests |
| Base stop / identification | Stop/join 800 ms; identification 16 s; Survey-In report freshness 5 s | `core/rtk/base`, `core/rtk/ubx`; unanswered probe cancellation and RAM configuration fixtures |
| RTCM forwarding | 64 whole corrections; max correction age 2 s; shared frame write deadline 500 ms; service stop 1 s | `core/rtk/service`; overflow/blocked transport/stale generation tests |
| Motor stream requests | 16 queued applies; 5 s apply deadline; 1 s guarded write; publish every 200 ms | `stream_service`; reconnect and slow preference-I/O fixtures |
| Parameter protocol | 8 command slots; 256 event slots; batches of 100; 1 s per-send echo; 5 s transaction budget | `core/params/service`; protocol and adapter tests |
| Command protocol | 8 queued requests, 16 events; retry after 1 s, maximum 3 retries; tick 200 ms | `core/commands`; priority/retry/ACK tests. Caller wait timeout does not itself stop the operation |
| Mission protocol | Retry after 500 ms, maximum 5 retries; tick 100 ms; whole operation 30 s | `core/mission/protocol`; deterministic upload/download/retry fixtures |
| MAVLink / telemetry | 4096 decoded and raw events; 256 outbound; latest snapshot watch at 20 Hz; 64 error and 64 status-text events | `core/mavlink/connection`, `telemetry/hub`; lag, source filtering and snapshot latency tests |
| Inspector / motor scope | Inspector sample channel 4096; frame window 33 ms; Motor scope pending 20,000 samples | `signals/tap`, `inspector_service`, `motor_test/scope`; bounded/drain/stop fixtures |
| Recorder | Queue 8192; writer flush every 500 ms; forced-arm audit wait 5 s; background retire budget 2 s | `core/recorder/service`, `recorder_service`; queue/drop/durable-audit tests |

Task counts, per-module cache bytes, DSP trace/analyzer limits, Tauri Channel backlog
and generic worker/job quotas are not exposed as common resource counters yet.
AF-04/11 must supply these before an overload/control-release qualification claim.
Hardware traces use the [serial/RTK checklist](../verification/serial-rtk-hardware-checklist.md)
and [Motor Test remediation plan](../plans/motor-test-review-remediation-plan.md).

## 7. AF-01 extraction constraints found by AF-00

1. `telemetry::VehicleId` already means MAVLink routing, not logical aircraft
   identity. New Runtime types must use a separate namespace/generated name.
2. Keep existing `LinkId` identity and actual `ConnectionHandle::generation`
   distinct; never infer physical FC boot identity from either. ADR-028 LINK-05
   still needs installed-topology traces.
3. A target snapshot / borrowed attachment must reuse the existing handle and
   parameter service/cache. Do not reserve or open a borrowed port again.
4. `ParamsState.last_cache` retains the previous cache after disconnect. Scoped
   readers must carry provenance and must not label that cache as the next target.
5. Native Parameters close, current log cancel and terrain cancellation select
   current global work. AF-02/06 replace this with owner-bound operations.
6. Telemetry and mission forwarders currently broadcast without a common target
   envelope. Delayed old results need an identity check before publication.
7. `LogsState::spawn` initializes `LogProtocol::new(1, 1)` even when primary
   discovery selected another node; audit this adapter while binding service targets.
8. Current native Motor Test destroy/explicit close and release-failure branches
   have different cleanup coverage. AF-07 must exercise every branch rather than
   assuming the common helper is used everywhere.
9. `RightInspector` owns monitor start/stop; mission completion hashes mutable
   editor state. These are AF-09/10 fixtures, independently of the new planner.

## 8. G0 handoff

The inventory, reference machine, reproducible launch profile, existing protocol/
ownership fixtures, deadlines, API vocabulary and measured evidence are recorded.
The owner has authorized implementation without repeated approval questions.
Proceed in small AF-01 slices against the companion contract and fixtures.
G1/G2/G-ARCH and later feature/physical-flight gates require their own evidence.
