# AF-02b.4b — Owned flight commands

- Date: 2026-10-26.
- The owner confirmed the safety-fix signatures and continued acceptance work.
- Scope: Takeoff, Land, RTL, Set mode, Arm/Disarm, Go to and Set home through
  the existing single command worker and one captured primary Runtime target.
- Parent: [Runtime contracts](runtime-contracts.md) and
  [owned command transactions](af-02b4a-command-ownership.md).

## Reviewed interfaces

The wire types originate in `core::commands::operation_types`:

```rust
pub struct FlightCommandRequest {
    pub context: CommandRequestContext,
    pub intent: FlightCommandIntent,
}

pub struct FlightCommandReply {
    pub operation: OperationRef,
    pub command: FlightCommandKind,
    pub steps: Vec<FlightCommandStepResult>,
}

pub enum FlightCommandError {
    Command { error: CommandOperationError },
    Preflight { refusal: PreflightRefusal },
}

pub async fn flight_command_execute(
    app: AppHandle,
    window: WebviewWindow,
    request: FlightCommandRequest,
) -> Result<FlightCommandReply, FlightCommandError>;

// Native preparation/dispatch guard; not a client authorization API.
fn recheck_arm(&self) -> Result<(), FlightCommandError>;
```

`FlightCommandIntent` is a tagged enum containing `Takeoff { plan_state,
acknowledged_warnings }`, `Land { ground_amsl }`, `ReturnToLaunch`,
`SetMode { main_mode, sub_mode }`, `Arm { force, plan_state,
acknowledged_warnings }`, `Disarm`, `Goto { latitude_deg, longitude_deg }` and
`SetHome { latitude_deg, longitude_deg, altitude }`. Absolute supplied heights
are `Height` with the `AMSL_EGM96` datum; invalid coordinates/datums fail before
dispatch. The existing `command_cancel` and `command_operation_status` require
the native caller and exact original session/operation, for both normal and
emergency command operations.

The flight UI captures request authority before asynchronous checklist or terrain
preparation. Each request has a local identity in addition to its native-issued
session and fresh `RequestId`. Only the newest request in the current target
scope may settle the visible pending/status/preflight state. The confirmation
panel uses the reviewed result:

```ts
sendArmDisarm(
  arm: boolean,
  force?: ForceArmRequest,
  ackedWarnings?: ChecklistItemId[],
): Promise<boolean> // true only for this current request's accepted verdict
```

## Write and failure analysis

1. **Wrong owner or replaced target:** native IPC injects the real caller;
   authorization, target binding, lease cancellation and transport generation are
   checked before work enters the command worker. Slow UI preparation keeps the
   clicked target context, rather than acquiring a later primary target. Retries
   cannot cross the captured transport generation.
2. **Takeoff partially accepted:** use one operation for `NAV_TAKEOFF` and the
   optional normal `ARM`. A denied takeoff never arms. Re-evaluate preflight
   immediately before the follow-up arm, after any ACK wait. Cancellation,
   target replacement or an admitted emergency in the inter-step gap prevents
   that arm. Failure after an accepted step requires reconciliation.
3. **Forced arm:** only bypass the GCS checklist; retain PX4 prearm checks and
   send zero in the force/kill parameter. Durable audit on the captured target's
   recorder is a prerequisite. Audit failure, owner loss, stale telemetry or an
   expired preparation budget produces no arm write. Recheck the physical gate
   after asynchronous audit persistence.
4. **Emergency admission:** Land, RTL and Arm/Disarm use the standalone
   `OperationResource::EmergencyCommand` marker. It does not conflict with normal
   claims; two of the default 32 active metadata slots are reserved for emergency
   work. Native arbitration supersedes normal command operations, including
   inter-step gaps, before the single protocol worker arbitrates the write.
   Other emergencies still obey that worker's existing single-slot policy.
5. **Lost ACK or cancelled dispatched work:** local retry retirement is not
   physical undo. Retain `OperationOutcome::Uncertain` and the generation-wide
   same-`MAV_CMD` ACK quarantine. A confirmed local retirement releases the flight
   protocol claim; unconfirmed cleanup retains it. Emergency frames remain
   sendable, but ambiguous ACKs cannot establish success. Reconnect never replays
   intent. Reboot retains its stricter whole-target reconciliation claims.
6. **Late UI replies:** obsolete target/session replies have no UI authority.
   Same-target displaced replies may be logged and reconcile their own map
   marker, but cannot clear a newer emergency's pending state or close its
   preflight panel. A denied/uncertain arm keeps confirmation available.

Budgets: 10 s per ordinary/emergency operation, 20 s for the entire takeoff
sequence, including preparation and queueing; 100 ms target observation;
500 ms maximum transport write; 1 s owned cancellation receipt. The command
worker retains the existing 1 s retry/three-retry policy.

## Measurable acceptance

- Foreign/stale/prepared-owner-loss requests produce zero flight-command writes.
- A denied takeoff and a newly blocked/cancelled/superseded inter-step gap produce
  zero follow-up arm writes.
- Closing idle A leaves B's command, primary transport and parameter cache alive;
  cancelling/closing the owner retires its retries within the 1 s receipt budget.
- Old operation cancellation cannot stop later work. An ACK timeout is uncertain,
  not accepted; same-command ambiguity is retained for that generation.
- UI click/hold grading, always-visible emergency controls, structured preflight,
  stale preparation/replies and map markers pass browser regression with zero
  page errors. Retain screenshots and manual reproduction steps.
- Regenerate all core/native Rust-derived TypeScript types, compare the complete
  generated file set/content, then pass full Rust/protocol tests, strict frontend
  checks, clippy, formatting, production frontend and release desktop builds.

Verification uses isolated loopback UDP peers, test-created Unix PTYs and browser
IPC fixtures. Physical aircraft/electrical behavior and the remaining
Mission/Shell/Logs/Motor adapter matrix retain their own evidence gates.
