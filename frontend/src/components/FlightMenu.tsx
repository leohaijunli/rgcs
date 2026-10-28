// Flight command menu (operator request 2026-10-11): one nav-bar dropdown
// instead of a row of buttons — Takeoff / Land / RTL / Arm-Disarm.
//
// U4 (plan §7) settled how a command is confirmed, once for the whole app:
//   • medium risk (takeoff / land / RTL / arm-disarm) — hold the button for
//     `HOLD_MS` while the ring draws itself; a plain click cannot fire it,
//     which is what a stray tap on a touch screen used to do through the
//     "click twice within 5 s" pattern this replaced
//   • the confirm bar above the buttons carries the state the command will
//     run against (mode, height above ground, battery)
//   • the status strip on the nav bar keeps the round trip visible:
//     sent → awaiting ACK → result + elapsed
//
// Takeoff (MS3, ADR-017): the backend replicates PX4's `commander takeoff`
// flow (NAV_TAKEOFF -> ACK -> ARM), so no separate arm click is needed.

import { useTranslation } from 'react-i18next'
import { useEffect, useRef, useState } from 'react'
import {
  CommandStatusStrip,
  HoldButton,
  HOLD_MS,
  type Tone,
} from '../design-system/components'
import { useCommandProgress, useCommandStore } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { usePreflightUi } from '../stores/preflight'
import ConfirmContextBar from './ConfirmContextBar'

/** Terminal ack results the menu surfaces in the status strip. */
const RESULT_LABELS: Record<string, string> = {
  accepted: 'plan.cmd.accepted',
  denied: 'plan.cmd.denied',
  unsupported: 'plan.cmd.unsupported',
  temporarily_rejected: 'plan.cmd.rejected',
  failed: 'plan.cmd.rejected',
  // A7: an emergency command (RTL/land/arm) took the single session slot.
  superseded: 'plan.cmd.superseded',
}

type Translate = (key: string, options?: Record<string, unknown>) => string

/** Name a command for the status strip. */
function commandName(t: Translate, command: string, armed: boolean): string {
  switch (command) {
    case 'takeoff':
      return t('plan.takeoff')
    case 'land':
      return t('plan.land')
    case 'rtl':
      return t('plan.rtl')
    case 'set_mode':
      return t('link.mode')
    case 'arm_disarm':
      return armed ? t('plan.cmd.disarm') : t('plan.cmd.arm')
    case 'arm':
      return t('plan.cmd.arm')
    case 'disarm':
      return t('plan.cmd.disarm')
    case 'change_altitude':
      return t('flightTarget.altitudeTitle')
    case 'change_speed':
      return t('flightTarget.speedTitle')
    default:
      return command
  }
}

export default function FlightMenu() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const link = useLinkStore((s) => s.link)
  const armed = useTelemetryStore(
    (s) => s.snapshot?.heartbeat?.base_mode.safety_armed === true,
  )
  const pending = useCommandStore((s) => s.pending)
  const send = useCommandStore((s) => s.send)
  const sendArmDisarm = useCommandStore((s) => s.sendArmDisarm)
  const dismissResult = useCommandStore((s) => s.dismissResult)
  const openPreflight = usePreflightUi((s) => s.openPanel)
  const progress = useCommandProgress()

  // Close on outside click / Esc.
  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    window.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  // The round trip always stays visible, whatever the link does.
  const status = progressText(t, progress.phase, progress.result)
  const strip = (
    <CommandStatusStrip
      command={progress.command === null ? null : commandName(t, progress.command, armed)}
      phase={status?.text ?? null}
      elapsedMs={progress.elapsedMs}
      tone={status?.tone}
      onDismiss={progress.phase === 'done' ? dismissResult : undefined}
      dismissLabel={t('plan.cmd.dismiss')}
    />
  )

  // L3: keep the menu on the bar when the heartbeat is gone — vanishing read
  // as a UI bug. It is greyed out with the reason, because a flight command
  // against a dead link can only fail. The menu itself cannot open.
  if (!link?.fc_alive) {
    const reason = t('plan.cmd.noLink')
    return (
      <div className="relative flex items-center gap-1.5" ref={rootRef}>
        {strip}
        <button
          data-testid="flight-menu-blocked"
          disabled
          title={reason}
          aria-label={reason}
          className="flex h-8 cursor-not-allowed items-center gap-1 rounded border border-line bg-canvas px-2.5 text-xs text-muted"
        >
          {t('plan.cmd.menu')}
          <span className="text-[9px] text-muted">▾</span>
          <span className="ml-1 text-[10px] text-warn">{t('link.noLink')}</span>
        </button>
      </div>
    )
  }

  const busy = pending !== null

  const entries: Array<{
    key: string
    label: string
    hint: string
    danger?: boolean
    /** A menu action, not a flight command: a plain click fires it (U5). */
    link?: boolean
    action: () => void
  }> = [
    {
      key: 'preflight',
      label: t('preflight.title'),
      hint: t('preflight.openHint'),
      link: true,
      action: () => openPreflight(),
    },
    {
      key: 'takeoff',
      label: t('plan.takeoff'),
      hint: t('plan.cmd.willTakeoff'),
      action: () => void send('takeoff'),
    },
    {
      key: 'land',
      label: t('plan.land'),
      hint: t('plan.cmd.willLand'),
      action: () => void send('land'),
    },
    {
      key: 'rtl',
      label: t('plan.rtl'),
      hint: t('plan.cmd.willRtl'),
      action: () => void send('rtl'),
    },
    {
      key: 'arm_disarm',
      label: armed ? t('plan.cmd.disarm') : t('plan.cmd.arm'),
      hint: armed ? t('plan.cmd.willDisarm') : t('plan.cmd.willArm'),
      danger: !armed,
      // Arm/disarm (MS3, ADR-017): PX4 runs its own preflight checks and
      // answers on the ack.
      action: () => void sendArmDisarm(!armed),
    },
  ]

  return (
    <div className="relative flex items-center gap-1.5" ref={rootRef}>
      {strip}
      <button
        data-testid="flight-menu"
        onClick={() => setOpen((o) => !o)}
        title={t('plan.cmd.menu')}
        className={`flex h-8 items-center gap-1 rounded border px-2.5 text-xs ${
          armed ? 'border-ok bg-ok text-ok-ink' : 'border-line bg-canvas text-ink hover:bg-panel'
        }`}
      >
        {t('plan.cmd.menu')}
        <span className="text-[9px] text-muted">▾</span>
      </button>

      {open && (
        <div className="absolute right-0 top-full z-50 mt-1 w-64 rounded border border-line bg-panel p-2 shadow-2xl">
          <ConfirmContextBar />
          <div className="mt-2 flex flex-col gap-1">
            {entries.map((entry) =>
              entry.link ? (
                <button
                  key={entry.key}
                  data-testid={`flight-cmd-${entry.key}`}
                  onClick={entry.action}
                  title={entry.hint}
                  className="flex h-9 w-full items-center rounded border border-line bg-canvas px-2 text-xs text-ink hover:bg-panel"
                >
                  {entry.label}
                </button>
              ) : (
                <HoldButton
                  key={entry.key}
                  data-testid={`flight-cmd-${entry.key}`}
                  variant={entry.danger ? 'danger' : 'default'}
                  disabled={busy}
                  hint={entry.hint}
                  onConfirm={entry.action}
                  className="w-full"
                >
                  {entry.label}
                </HoldButton>
              ),
            )}
          </div>
          <div className="mt-1.5 text-[10px] text-muted">
            {t('plan.cmd.holdNote', { s: HOLD_MS / 1000 })}
          </div>
        </div>
      )}
    </div>
  )
}

/** Strip copy for the current phase; `null` before the first command. */
function progressText(
  t: Translate,
  phase: string | null,
  result: { kind: string; result?: string | null; message?: string | null } | null,
): { text: string; tone: Tone } | null {
  if (phase === null) return null
  if (phase === 'sending') return { text: t('plan.cmd.sending'), tone: 'muted' }
  if (phase === 'awaiting_ack') return { text: t('plan.cmd.sent'), tone: 'muted' }
  if (result === null) return null
  if (result.result) {
    return {
      text: t(RESULT_LABELS[result.result] ?? 'plan.cmd.unknown'),
      tone: result.result === 'accepted' ? 'ok' : 'warn',
    }
  }
  return {
    text: t('plan.cmd.failed', { message: result.message ?? t('plan.cmd.unknown') }),
    tone: 'error',
  }
}
