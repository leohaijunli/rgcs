// Flight command state (issue #5): every request is bound to one exact Runtime
// client/target session. The typed IPC reply, not the legacy global event,
// supplies the terminal flight-controller verdict.

import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { Checklist } from '../generated-types/Checklist'
import type { ChecklistItemId } from '../generated-types/ChecklistItemId'
import type { FlightCommandIntent } from '../generated-types/FlightCommandIntent'
import type { FlightCommandKind } from '../generated-types/FlightCommandKind'
import type { FlightCommandReply } from '../generated-types/FlightCommandReply'
import type { FlightCommandRequest } from '../generated-types/FlightCommandRequest'
import type { ForceArmRequest } from '../generated-types/ForceArmRequest'
import type { Height } from '../generated-types/Height'
import type { PreflightRefusal } from '../generated-types/PreflightRefusal'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import {
  flightCommandContext,
  flightCommandError,
  isCurrentFlightRuntimeSession,
  operationMatchesSession,
  resetFlightRuntimeSession,
  sameRuntimeSession,
} from '../desktop/flightRuntime'
import { logEvent } from './eventlog'
import { useTelemetryStore } from './telemetry'
import { useTerrainStore } from './terrain'
import { buildPlanState, fetchChecklist, gateStatus, usePreflightUi } from './preflight'

/** Ground (AMSL) under a point, for the commands that must name a terrain
 * target: the mesh/DEM sample when terrain is loaded, else the vehicle's own
 * altitude (a vehicle on the ground knows its height). */
async function groundAmslAt(
  latDeg: number,
  lonDeg: number,
  fallbackAmslM: number,
): Promise<number> {
  try {
    const ground = await useTerrainStore.getState().samplePointAmsl(latDeg, lonDeg)
    if (ground != null && Number.isFinite(ground)) return ground
  } catch {
    /* no terrain: fall back to the vehicle's altitude */
  }
  return fallbackAmslM
}

/**
 * Commands the UI may send. `takeoff`/`land` map to `MAV_CMD_NAV_TAKEOFF` /
 * `MAV_CMD_NAV_LAND`; `pause`/`continue` (`MAV_CMD_DO_PAUSE_CONTINUE`) were
 * removed — PX4 v1.17 has no handler for them (SITL-confirmed NACK).
 */
export type CommandName =
  | 'takeoff'
  | 'land'
  | 'rtl'
  | 'set_mode'
  | 'arm_disarm'
  | 'arm'
  | 'disarm'
  | 'goto'
  | 'set_home'

/** Terminal vocabulary retained by the existing command-status components. */
export interface CommandEventPayload {
  command: string
  kind: 'sent' | 'completed' | 'failed'
  result?: string | null
  message?: string | null
}

interface CommandState {
  /** Command awaiting a terminal ack, or null when idle. */
  pending: CommandName | null
  /** Most recent event, kept for the status line. */
  lastEvent: CommandEventPayload | null
  /** Wall-clock time the command left the UI, or `null` when nothing is in
   * flight. The status strip measures the round trip from here (plan §7 U4). */
  sentAtMs: number | null
  /** Round trip of the last terminal ack, in ms (frozen at the ack). */
  lastElapsedMs: number | null
  send: (name: 'takeoff' | 'land' | 'rtl') => Promise<void>
  /** Switch the PX4 flight mode (MS3, ADR-017). */
  sendSetMode: (mainMode: number, subMode: number) => Promise<void>
  /** Arm or disarm the vehicle (MS3, ADR-017). Arming passes an optional L5
   * force request (blocked checklist) or the acknowledged `Warn` item ids
   * (U5); the backend refuses a blocked arm without one. */
  sendArmDisarm: (
    arm: boolean,
    force?: ForceArmRequest,
    ackedWarnings?: ChecklistItemId[],
  ) => Promise<boolean>
  /** The last preflight refusal (L5), rendered by the PreflightPanel. */
  preflight: PreflightRefusal | null
  dismissPreflight: () => void
  /** Fly the vehicle to a map point (`MAV_CMD_DO_REPOSITION`). */
  sendGoto: (latDeg: number, lonDeg: number) => Promise<void>
  /** Set the vehicle's home to a map point (`MAV_CMD_DO_SET_HOME`). */
  sendSetHome: (latDeg: number, lonDeg: number) => Promise<void>
  /** Map point of the last "Move to here" click, shown as a marker until the
   * vehicle arrives or the command fails. */
  gotoTarget: [number, number] | null
  clearGotoTarget: () => void
  /** HOME the vehicle accepted via "Set home" (lat, lon, AMSL m): the HOME
   * marker follows it, since there is no HOME_POSITION feed. */
  homeMarker: [number, number, number] | null
  handleEvent: (e: CommandEventPayload) => void
  /** Drop the last result from the status strip once it is acknowledged (L2:
   * a rejection is kept on screen until the operator confirms it). */
  dismissResult: () => void
  reset: () => void
}

interface PendingHome {
  token: symbol
  marker: [number, number, number]
}

/** Map-command effects are request-local so an older reply cannot move a newer marker. */
let pendingHome: PendingHome | null = null
let pendingGoto: symbol | null = null
let commandSession: RuntimeClientSession | null = null
let commandEpoch = 0
let latestFlightRequest: symbol | null = null

interface FlightRequestScope {
  token: symbol
  epoch: number
  context: FlightCommandRequest['context']
}

/** Terminal bookkeeping: stop the clock and freeze the round trip. */
function settled(sentAtMs: number | null): {
  sentAtMs: null
  lastElapsedMs: number | null
} {
  return {
    sentAtMs: null,
    lastElapsedMs: sentAtMs === null ? null : Date.now() - sentAtMs,
  }
}

export const useCommandStore = create<CommandState>((set) => ({
  pending: null,
  lastEvent: null,
  sentAtMs: null,
  lastElapsedMs: null,
  gotoTarget: null,
  homeMarker: null,
  preflight: null,
  clearGotoTarget: () => set({ gotoTarget: null }),
  dismissPreflight: () => set({ preflight: null }),

  send: async (name) => {
    const scope = await beginFlightRequest(name)
    if (!scope) return
    // One-click Takeoff is preflight-gated client-side while disarmed: the
    // backend enforces the gate on arm, so Takeoff refuses from the same
    // evaluated checklist instead of racing ahead of it. Once the vehicle is
    // armed (normally or via the forced arm), the operator's arm decision
    // stands — the preflight gate does not re-block Takeoff with the same
    // warnings, and PX4's own takeoff handling applies.
    if (name === 'takeoff') {
      const armed =
        useTelemetryStore.getState().snapshot?.heartbeat?.base_mode.safety_armed === true
      if (!armed) {
        try {
          const checklist = await fetchChecklist()
          if (!await validateFlightRequest(scope, name)) return
          const status = gateStatus(checklist)
          if (status !== 'allowed') {
            applyFailure(name, {
              preflight: refusalFromChecklist(status, checklist),
              result: null,
              message: refusalMessage(status),
            }, {})
            return
          }
        } catch {
          /* not connected: the send below fails with the real reason */
        }
      }
    }
    if (name === 'takeoff') {
      await executeFlightIntent(scope, 'takeoff', {
        kind: 'takeoff',
        plan_state: buildPlanState(),
        acknowledged_warnings: [],
      })
      return
    }
    if (name === 'rtl') {
      await executeFlightIntent(scope, 'rtl', { kind: 'return_to_launch' })
      return
    }

    // Name the terrain under the vehicle so PX4 lands on the surface rather
    // than a HOME plane. Without a fix the backend reports the real refusal.
    const fix = useTelemetryStore.getState().snapshot?.global_position
    const ground = fix
      ? await groundAmslAt(fix.latitude_deg, fix.longitude_deg, fix.altitude.meters)
      : null
    await executeFlightIntent(scope, 'land', {
      kind: 'land',
      ground_amsl: ground === null ? null : amslHeight(ground),
    })
  },

  sendSetMode: async (mainMode, subMode) => {
    const scope = await beginFlightRequest('set_mode')
    if (!scope) return
    await executeFlightIntent(scope, 'set_mode', {
      kind: 'set_mode',
      main_mode: mainMode,
      sub_mode: subMode,
    })
  },

  sendArmDisarm: async (arm, force, ackedWarnings) => {
    const command = arm ? 'arm' : 'disarm'
    const scope = await beginFlightRequest(command)
    if (!scope) return false
    const intent: FlightCommandIntent = arm
      ? {
          kind: 'arm',
          force: force ?? null,
          plan_state: buildPlanState(),
          acknowledged_warnings: ackedWarnings ?? [],
        }
      : { kind: 'disarm' }
    return executeFlightIntent(scope, command, intent, { clearPreflightOnReply: arm })
  },

  /** Fly the vehicle to a map point (`MAV_CMD_DO_REPOSITION`). */
  sendGoto: async (latDeg, lonDeg) => {
    const scope = await beginFlightRequest('goto')
    if (!scope) return
    const token = scope.token
    pendingGoto = token
    set({ gotoTarget: [latDeg, lonDeg] })
    await executeFlightIntent(
      scope,
      'goto',
      { kind: 'goto', latitude_deg: latDeg, longitude_deg: lonDeg },
      {
        onTerminal: (event) => {
          if (pendingGoto !== token) return
          if (event.result !== 'accepted') set({ gotoTarget: null })
          pendingGoto = null
        },
      },
    )
  },

  /** Set the vehicle's home to a map point (`MAV_CMD_DO_SET_HOME`). */
  sendSetHome: async (latDeg, lonDeg) => {
    const scope = await beginFlightRequest('set_home')
    if (!scope) return
    // The terrain height under the *clicked* point, not the vehicle's own
    // altitude: home is usually re-set while airborne over the next site, and
    // an airborne altitude bakes a home plane far above the ground (every
    // HOME-referenced height, the landing target included, then sits too
    // high). Falls back to the current fix when no terrain is loaded.
    const fix = useTelemetryStore.getState().snapshot?.global_position
    const alt = await groundAmslAt(latDeg, lonDeg, fix?.altitude.meters ?? 0)
    if (!isLatestFlightRequest(scope)) return
    const token = scope.token
    pendingHome = { token, marker: [latDeg, lonDeg, alt] }
    await executeFlightIntent(
      scope,
      'set_home',
      {
        kind: 'set_home',
        latitude_deg: latDeg,
        longitude_deg: lonDeg,
        altitude: amslHeight(alt),
      },
      {
        onTerminal: (event) => {
          const current = pendingHome
          if (current?.token !== token) return
          if (event.result === 'accepted') set({ homeMarker: current.marker })
          pendingHome = null
        },
      },
    )
  },

  // The executor emits a local `sent` phase and applies one scoped terminal
  // reply. No unowned global command event reaches this store.
  handleEvent: (e) => {
    // Event log (operator request): the nav bar no longer shows the ack
    // inline; every terminal outcome lands in the log instead.
    if (e.kind !== 'sent') {
      logCommandOutcome(e)
    }
    // Every terminal event freezes the round trip for the status strip.
    set((s) =>
      e.kind === 'sent'
        ? { lastEvent: e, pending: s.pending }
        : { lastEvent: e, pending: null, ...settled(s.sentAtMs) },
    )
  },

  dismissResult: () => {
    set({ lastEvent: null, lastElapsedMs: null })
  },

  reset: () => {
    commandEpoch += 1
    resetFlightRuntimeSession()
    commandSession = null
    latestFlightRequest = null
    pendingHome = null
    pendingGoto = null
    usePreflightUi.getState().closePanel()
    set({
      pending: null,
      lastEvent: null,
      sentAtMs: null,
      lastElapsedMs: null,
      gotoTarget: null,
      homeMarker: null,
      preflight: null,
    })
  },
}))

interface ExecuteOptions {
  clearPreflightOnReply?: boolean
  onTerminal?: (event: CommandEventPayload) => void
}

/** Capture authority before checklist/terrain preparation. A later click retires
 * this request's display authority without cancelling another native owner. */
async function beginFlightRequest(
  command: CommandName,
): Promise<FlightRequestScope | null> {
  const token = Symbol(command)
  latestFlightRequest = token
  const epoch = commandEpoch
  const previousSession = commandSession
  useCommandStore.setState({
    pending: command,
    lastEvent: null,
    sentAtMs: Date.now(),
    lastElapsedMs: null,
  })

  let context: FlightCommandRequest['context']
  try {
    context = await flightCommandContext()
  } catch (error) {
    if (epoch === commandEpoch && latestFlightRequest === token) {
      applyFailure(command, flightCommandError(error), {})
    }
    return null
  }
  if (epoch !== commandEpoch || latestFlightRequest !== token) return null

  if (previousSession && !sameRuntimeSession(previousSession, context.session)) {
    // A replacement discovered without a link/heartbeat event still retires
    // old intent. Require a fresh click against the newly discovered target.
    commandEpoch += 1
    pendingHome = null
    pendingGoto = null
    usePreflightUi.getState().closePanel()
    useCommandStore.setState({
      pending: null,
      lastEvent: null,
      sentAtMs: null,
      lastElapsedMs: null,
      gotoTarget: null,
      homeMarker: null,
      preflight: null,
    })
    commandSession = context.session
    applyFailure(command, {
      message: 'The flight target changed; issue a new command for the current target',
      preflight: null,
      result: null,
    }, {})
    return null
  }
  commandSession = context.session
  return { token, epoch, context }
}

function isCurrentFlightRequest(scope: FlightRequestScope): boolean {
  return scope.epoch === commandEpoch
    && sameRuntimeSession(commandSession, scope.context.session)
    && isCurrentFlightRuntimeSession(scope.context.session)
}

function isLatestFlightRequest(scope: FlightRequestScope): boolean {
  return latestFlightRequest === scope.token && isCurrentFlightRequest(scope)
}

/** Revalidate the captured native session after every asynchronous preparation,
 * including a same-endpoint/generation replacement the UI has not observed. */
async function validateFlightRequest(
  scope: FlightRequestScope,
  command: CommandName,
  options: ExecuteOptions = {},
): Promise<boolean> {
  if (!isLatestFlightRequest(scope)) return false
  try {
    await invoke('runtime_client_validate', { session: scope.context.session })
    return isLatestFlightRequest(scope)
  } catch (error) {
    if (isLatestFlightRequest(scope)) applyFailure(command, flightCommandError(error), options)
    return false
  }
}

/** An unchanged endpoint/heartbeat does not prove native generation continuity.
 * Revalidate terminal authority as well as dispatch authority. */
async function validateFlightReply(scope: FlightRequestScope): Promise<boolean> {
  if (!isCurrentFlightRequest(scope)) return false
  try {
    await invoke('runtime_client_validate', { session: scope.context.session })
    return isCurrentFlightRequest(scope)
  } catch {
    if (isCurrentFlightRequest(scope)) useCommandStore.getState().reset()
    return false
  }
}

/** Execute against the already captured session. Same-target displaced outcomes
 * can reconcile their own marker/log, but cannot settle a newer pending command. */
async function executeFlightIntent(
  scope: FlightRequestScope,
  command: CommandName,
  intent: FlightCommandIntent,
  options: ExecuteOptions = {},
): Promise<boolean> {
  if (!await validateFlightRequest(scope, command, options)) return false

  const request: FlightCommandRequest = { context: scope.context, intent }
  const replyPromise = invoke<FlightCommandReply>('flight_command_execute', { request })
  if (isLatestFlightRequest(scope)) {
    useCommandStore.setState({
      lastEvent: { command, kind: 'sent', result: null, message: null },
    })
  }

  try {
    const reply = await replyPromise
    if (
      !isCurrentFlightRequest(scope)
      || !operationMatchesSession(reply.operation, scope.context.session)
      || commandFromKind(reply.command) !== command
      || reply.command !== intent.kind
    ) return false
    if (!await validateFlightReply(scope)) return false

    const verdict = reply.steps.find((step) => step.result !== 'accepted')
      ?? reply.steps[reply.steps.length - 1]
    const event: CommandEventPayload = verdict
      ? { command, kind: 'completed', result: verdict.result, message: null }
      : { command, kind: 'failed', result: null, message: 'Flight command returned no steps' }
    options.onTerminal?.(event)
    if (isLatestFlightRequest(scope)) {
      if (options.clearPreflightOnReply && event.result === 'accepted') {
        useCommandStore.setState({ preflight: null })
      }
      useCommandStore.getState().handleEvent(event)
      return event.result === 'accepted'
    }
    logCommandOutcome(event)
    return false
  } catch (error) {
    if (!await validateFlightReply(scope)) return false
    const failure = flightCommandError(error)
    if (isLatestFlightRequest(scope)) {
      applyFailure(command, failure, options)
    } else {
      const event: CommandEventPayload = {
        command,
        kind: 'failed',
        result: failure.result,
        message: failure.message,
      }
      options.onTerminal?.(event)
      logCommandOutcome(event)
    }
    return false
  }
}

function logCommandOutcome(event: CommandEventPayload): void {
  const level = event.result === 'accepted' ? 'ok' : event.result == null ? 'err' : 'warn'
  logEvent('command', level, `${event.command}: ${event.result ?? event.message ?? 'failed'}`)
}

function applyFailure(
  command: CommandName,
  failure: ReturnType<typeof flightCommandError>,
  options: ExecuteOptions,
): void {
  const event: CommandEventPayload = {
    command,
    kind: 'failed',
    result: failure.result,
    message: failure.message,
  }
  if (failure.preflight) {
    useCommandStore.setState({ preflight: failure.preflight })
  }
  options.onTerminal?.(event)
  useCommandStore.getState().handleEvent(event)
}

function commandFromKind(kind: FlightCommandKind): CommandName | null {
  const names: Record<string, CommandName> = {
    takeoff: 'takeoff',
    land: 'land',
    return_to_launch: 'rtl',
    set_mode: 'set_mode',
    arm: 'arm',
    disarm: 'disarm',
    goto: 'goto',
    set_home: 'set_home',
  }
  return names[kind] ?? null
}

function amslHeight(meters: number): Height {
  return { datum: 'AMSL_EGM96', meters }
}

/** Client-side refusal for the Takeoff gate (the backend gate covers arm). */
function refusalFromChecklist(status: 'requires_ack' | 'blocked' | 'no_link', checklist: Checklist): PreflightRefusal {
  const items =
    status === 'requires_ack'
      ? checklist.items.filter((i) => i.status === 'warn')
      : status === 'blocked'
        ? checklist.items.filter((i) => i.status === 'fail' || i.status === 'unknown')
        : []
  return { kind: status, items, message: refusalMessage(status) }
}

function refusalMessage(status: 'requires_ack' | 'blocked' | 'no_link'): string {
  switch (status) {
    case 'requires_ack':
      return 'Confirm the listed warnings before takeoff'
    case 'blocked':
      return 'Preflight blocks takeoff'
    case 'no_link':
      return 'No link — cannot take off'
  }
}

/** How often the in-flight elapsed time is refreshed (plan §7 U4). */
const PROGRESS_TICK_MS = 200

/** Where a command sits in its round trip (plan §7 U4 status strip). */
export type CommandPhase = 'sending' | 'awaiting_ack' | 'done'

export interface CommandProgress {
  /** The command in flight, or the last one that finished; null before any. */
  command: string | null
  phase: CommandPhase | null
  /** Milliseconds since the command left the UI — live while in flight. */
  elapsedMs: number | null
  /** The event the phase text is derived from. */
  result: CommandEventPayload | null
}

/**
 * Live view of the command round trip behind the status strip: "sent →
 * awaiting ACK → result + elapsed" (plan §7 U4). While a command is pending
 * the elapsed time ticks; once it ends the value frozen at the ack is shown,
 * so the strip never drifts away from when the FC answered.
 */
export function useCommandProgress(): CommandProgress {
  const pending = useCommandStore((s) => s.pending)
  const sentAtMs = useCommandStore((s) => s.sentAtMs)
  const lastEvent = useCommandStore((s) => s.lastEvent)
  const lastElapsedMs = useCommandStore((s) => s.lastElapsedMs)
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (pending === null) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), PROGRESS_TICK_MS)
    return () => window.clearInterval(id)
  }, [pending, sentAtMs])

  if (pending !== null) {
    return {
      command: pending,
      phase: lastEvent?.kind === 'sent' ? 'awaiting_ack' : 'sending',
      elapsedMs: sentAtMs === null ? null : now - sentAtMs,
      result: lastEvent,
    }
  }
  if (lastEvent === null) {
    return { command: null, phase: null, elapsedMs: null, result: null }
  }
  return {
    command: lastEvent.command,
    phase: 'done',
    elapsedMs: lastElapsedMs,
    result: lastEvent,
  }
}
