# AF-02b.4 — Owned Motor contract

- Date: 2026-10-27.
- Status: interfaces approved; contract/data, priority and native actuation slices
  implemented and accepted in software. Dedicated capture/log and final delivery
  remain ([actuation evidence](../verification/af-02b-motor-actuation-20261027.md)).
- Scope: existing dedicated/borrowed Motor Test, permission, manual/preset/forward
  start, value updates, STOP/Release, tones, parameter/configuration, capture/log
  and native close. Full Module Host/SDK movement remains AF-07.

## Findings and retained mechanics

`MotorTestService` already uses a captured generation and source-scoped heartbeat/
ACTUATOR_TEST ACK route. Normal writes are bounded to 200 ms and Release uses a
2 s overall transport receipt. The pure session keeps failed Release pending,
tries the remaining motors and distinguishes idle/no-owned-output STOP. These
mechanics remain the effect path; no second connection or motor ticker is added.

The native adapters still authorize global state: permission is bound to the
target/generation but not the calling window/module; start/set/STOP resolve the
current running service. A delayed window callback or main preflight repair can
therefore affect a different owner's later session. Capture/log cancellation is
also global. At review, core state/Stopped publication could await a full observer
queue. The [priority control-path checkpoint](../verification/af-02b-motor-priority-20261027.md)
now supplies nonblocking events, retained state and a bounded urgent STOP channel;
native permission/operation admission remains the next slice.

## Signatures and errors

The initial contract-only slice adds these Rust-derived types under
`core::motor_test::operation_types`, before changing native effect entry points:

```rust
pub struct MotorRequestContext { pub session: RuntimeClientSession, pub request_id: RequestId }
pub struct MotorPermissionRef {
    pub owner: OperationOwner, pub target: TargetRef, pub revision: Revision,
}
pub struct MotorPermissionRequest {
    pub context: MotorRequestContext, pub enabled: bool,
    pub permission: Option<MotorPermissionRef>,
}
pub struct MotorPermissionStatus { pub permission: Option<MotorPermissionRef> }
pub struct MotorForwardSourceRef {
    pub link_id: LinkId, pub generation: TransportGeneration,
    pub mavlink_target: MavlinkTarget, pub port: ServoPort,
}
pub enum MotorStartSource {
    Manual { values: Vec<f32> }, Preset { preset: WaveformPreset },
    Forward { source: MotorForwardSourceRef },
}
pub struct MotorStartRequest {
    pub context: MotorRequestContext, pub permission: MotorPermissionRef,
    pub config_revision: Revision, pub source: MotorStartSource,
}
pub struct MotorValuesRequest {
    pub session: RuntimeClientSession, pub operation: OperationRef,
    pub request_id: RequestId, pub revision: Revision, pub values: Vec<f32>,
}
pub struct MotorStopRequest {
    pub session: RuntimeClientSession, pub operation: OperationRef,
    pub request_id: RequestId, pub emergency: bool,
}
pub struct MotorStartReply { pub operation: OperationRef, pub config_revision: Revision }
pub struct MotorValuesReceipt { pub operation: OperationRef, pub revision: Revision }
pub struct MotorReleaseReceipt { pub operation: OperationRef, pub released_motors: u8, pub state: SessionState }
pub enum MotorOperationError {
    Operation { error: OperationError }, Protocol { error: MotorProtocolError },
    ReconciliationRequired { operation: Box<OperationRef>, reason: String },
}
pub enum MotorProtocolError {
    Disabled, PermissionChanged, ServiceStopped, TimedOut,
    InvalidRequest { message: String }, ReleaseWrite { motor: u8, message: String },
}
pub fn validate_start(request: &MotorStartRequest) -> Result<(), MotorOperationError>;
pub fn validate_values(values: &[f32]) -> Result<(), MotorProtocolError>;
```

Enums are snake-case tagged on the wire. Opaque identities and all revisions/
generations remain strings. `WaveformPreset`, `PresetKind` and `SessionState`
export their existing definitions rather than duplicate interface models.
Validation checks scope and numeric shape; **native-issued authority is separately
checked against the actual window/permission registry**, not inferred from DTOs.

## Native migration boundaries

| Existing entry | Required owned input/result and behavior |
| --- | --- |
| `actuator_set_motor_control` | `MotorPermissionRequest` → `MotorPermissionStatus`; ON is a fresh native grant, OFF requires its exact ref and revokes before waiting |
| `actuator_start_manual/preset/forward` | `MotorStartRequest` → `MotorStartReply`; capture config/source before tone/preparation and validate again at commit/first write |
| `actuator_set_values` | `MotorValuesRequest` → `MotorValuesReceipt`; exact active operation, increasing revision, bounded latest-value update |
| `actuator_stop/estop` | `MotorStopRequest` → `MotorReleaseReceipt`; priority route, no new normal claim and no global cancel-current |
| owned status/cancel | exact Runtime session + operation → retained `OperationStatus` |
| tones/configuration | existing parameter/shell primitives with Motor context/permission/config revision and owned request identity |
| capture verify/start/stop | target-bound request; started file identity belongs to a retained capture operation; stop/cancel requires that operation |
| dedicated log adapters | reuse owned `LogService` and DTOs; picker before admission, exact cancel, safe deletion/read-back |
| native close/lifecycle | revoke only captured client grants; retire its actors and release owned outputs before closing dedicated I/O; borrowed main I/O/cache survives |

Actuation holds `ActuatorOutput` from start preparation until an owned Release
receipt. Enabling permission alone does not start a ticker, send output, or claim
running-state success. Start replies must correspond to actual bounded control
write admission, not merely enqueueing a legacy Start command.

Capture and actuation stay independent. Capture needs a dedicated resource/pin;
shell is reserved only for its bounded exchanges so a recording does not deadlock
the run coordinator's start tone. Existing capture → pre-roll → motor → release →
post-roll → capture-stop ordering and test/file identity remain intact. Resource
extension and coordinated native calls are reviewed with the capture slice.

Normal public STOP/cancel remains owner-specific. Native lifecycle and documented
safety arbitration can stop a known target operation through a trusted path;
they cannot use an arbitrary renderer ref as authority. Main preflight repair
must migrate its unscoped `actuator_stop` call accordingly.

## Serial-write and failure risks

1. **Wrong owner/target/config/source:** authorize actual native window, exact
   permission/operation, target generation, output config revision and forward
   source before preparation, commit, each queued write and result publication.
   Old OFF/set/STOP cannot revoke or release B. Generation replacement never
   redirects a Release to a new FC.
2. **Default OFF/start races:** no persisted permission. Owner close, OFF or
   target loss synchronously revokes that grant. The existing short gate fence
   linearizes start enqueue against revocation; pending tone/config checks are
   cancelled before a first output can be sent.
3. **Malformed demand:** accept only 1–16 finite normalized values in `[0, 1]`;
   compare width with the captured mapping before dispatch. Preset amplitude,
   frequency and duration must be finite; periodic sources cannot exceed the
   10 Hz ticker's Nyquist frequency. Native output envelope/slew/interlocks still
   apply. No invalid numeric request is silently converted to a different demand.
4. **Release meaning:** `param2 <= 0` returns PX4 to configured disarmed output,
   not guaranteed zero PWM or stopped rotors. The 0.3 s FC timeout is existing
   protocol configuration, not an electrical measurement. Keep separate
   transport, FC read-back and bench evidence.
5. **STOP/backpressure:** urgent stop bypasses ordinary updates and observer
   publication. Failed release attempts the remaining owned motors, suppresses
   refreshes and preserves a retryable claim/diagnostic. Drop/abort cannot invent
   a Release receipt. Idle/no-owned-output requests send zero release frames.
6. **Capture permanence:** `logger on/off`, rm and LOG_ERASE are real FC effects.
   Logger stop checks the captured owned file; a changed external logger/file
   does not authorize stopping it. Interrupted files/runs keep explicit recovery
   state. Deletion uses fresh disarmed/logger/path/id proofs and read-back.
7. **Configuration/tone:** parameter writes reuse owned echo/read-back and reboot
   policies. DShot tone checks output/protocol/map revision and playback fence;
   shell completion is not an acoustic or stationary-rotor receipt.

Budgets retained at this stage: control write 200 ms, full Release 2 s, tone/
capture finalization 12 s, existing 1 s pre/post-roll and 0.3 s FC output timeout.
Native start/owned-session/parameter budgets are explicit configuration; no
unbounded queue/lock wait or observer event is part of the control path.

## Measurable acceptance and slices

1. **Contract/data slice:** generated canonical/frontend DTOs match; serde keeps
   full-width identities/revisions; foreign permission/operation and NaN/infinite/
   out-of-range/count/preset demands reject. Existing Motor tests still pass.
2. **Permission/actuation slice:** zero writes from foreign, unconfirmed, stale,
   prepared-owner-loss or obsolete config/source; A/B/old OFF/set/STOP fixtures;
   default OFF/passive and actual start receipt. Native close/reopen and saturated
   update/event queues cannot exceed Release's configured deadline.
3. **Capture/log/config slice:** independent capture/motor lifetime; exact file/
   run owner; interruption/recovery; obsolete logger cannot stop a new file;
   dedicated versus borrowed ownership and complete download/deletion checks.
4. **Delivery slice:** production native PTY/UDP faults, browser responsive/field
   screenshots and manual steps, complete generated/static/Rust/release gates,
   followed by the cross-adapter G1 decision. Electrical/acoustic/output-timeout
   bench qualification is recorded separately after software checks.

The interface/data slice alone does not complete Motor migration or close G1.
