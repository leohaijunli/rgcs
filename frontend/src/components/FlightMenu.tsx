// Flight command menu (operator request 2026-10-11): one nav-bar dropdown
// instead of a row of buttons — Takeoff / Land / RTL / Arm-Disarm behind
// two-click confirms. The ack outcome is not shown inline any more; it
// lands in the event log drawer (which is also where "Accepted" went).
//
// Takeoff (MS3, ADR-017): the backend replicates PX4's `commander takeoff`
// flow (NAV_TAKEOFF -> ACK -> ARM), so no separate arm click is needed.

import { useTranslation } from 'react-i18next'
import { useEffect, useRef, useState } from 'react'
import { useCommandStore, type CommandName } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'

/** How long a confirm-required entry stays armed for the second click. */
const CONFIRM_WINDOW_MS = 5000

/** Terminal ack results the menu surfaces in the popover status line. */
const RESULT_LABELS: Record<string, string> = {
  accepted: 'plan.cmd.accepted',
  denied: 'plan.cmd.denied',
  unsupported: 'plan.cmd.unsupported',
  temporarily_rejected: 'plan.cmd.rejected',
  failed: 'plan.cmd.rejected',
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
  const lastEvent = useCommandStore((s) => s.lastEvent)
  const send = useCommandStore((s) => s.send)
  const sendArmDisarm = useCommandStore((s) => s.sendArmDisarm)
  const [confirming, setConfirming] = useState<string | null>(null)
  const confirmTimer = useRef<number | undefined>(undefined)

  useEffect(() => () => window.clearTimeout(confirmTimer.current), [])

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

  // L3: keep the menu on the bar when the heartbeat is gone — vanishing read
  // as a UI bug. It is greyed out with the reason, because a flight command
  // against a dead link can only fail. The menu itself cannot open.
  if (!link?.fc_alive) {
    const reason = t('plan.cmd.noLink')
    return (
      <div className="relative" ref={rootRef}>
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

  // Disruptive commands arm on the first click; a second click within the
  // confirm window sends them.
  const press = (name: CommandName) => {
    if (confirming !== name) {
      setConfirming(name)
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(
        () => setConfirming(null),
        CONFIRM_WINDOW_MS,
      )
      return
    }
    window.clearTimeout(confirmTimer.current)
    setConfirming(null)
    void send(name)
  }

  // Arm/disarm (MS3, ADR-017): both directions behind the two-click
  // confirm; PX4 runs its own preflight checks and answers on the ack.
  const pressArm = () => {
    if (confirming !== 'arm_disarm') {
      setConfirming('arm_disarm')
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(
        () => setConfirming(null),
        CONFIRM_WINDOW_MS,
      )
      return
    }
    window.clearTimeout(confirmTimer.current)
    setConfirming(null)
    void sendArmDisarm(!armed)
  }

  const busy = pending !== null
  // The status line only shows failures: an "Accepted" on the nav bar is
  // noise (operator request) — the ack lands in the event log instead.
  const failure =
    !busy && lastEvent && lastEvent.kind !== 'sent' && lastEvent.result !== 'accepted'
      ? lastEvent
      : null

  const entries: Array<{
    key: string
    label: string
    danger?: boolean
    action: () => void
  }> = [
    { key: 'takeoff', label: t('plan.takeoff'), action: () => press('takeoff') },
    { key: 'land', label: t('plan.land'), action: () => press('land') },
    { key: 'rtl', label: t('plan.rtl'), action: () => press('rtl') },
    {
      key: 'arm_disarm',
      label: armed ? t('plan.cmd.disarm') : t('plan.cmd.arm'),
      danger: !armed,
      action: pressArm,
    },
  ]

  return (
    <div className="relative" ref={rootRef}>
      <button
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
        <div className="absolute right-0 top-full z-50 mt-1 w-48 rounded border border-line bg-panel p-2 shadow-2xl">
          <div className="flex flex-col gap-1">
            {entries.map((entry) => {
              const isConfirming = confirming === entry.key
              return (
                <button
                  key={entry.key}
                  onClick={entry.action}
                  disabled={busy}
                  className={`rounded border px-2 py-1.5 text-xs font-medium transition-colors disabled:opacity-50 ${
                    isConfirming
                      ? entry.danger
                        ? 'border-warn bg-warn text-canvas'
                        : 'border-warn bg-canvas text-warn'
                      : 'border-line bg-canvas text-ink hover:bg-panel'
                  }`}
                >
                  {isConfirming
                    ? t(
                        entry.key === 'takeoff'
                          ? 'plan.cmd.confirmTakeoff'
                          : entry.key === 'land'
                            ? 'plan.cmd.confirmLand'
                            : entry.key === 'rtl'
                              ? 'plan.cmd.confirmRtl'
                              : armed
                                ? 'plan.cmd.confirmDisarm'
                                : 'plan.cmd.confirmArm',
                      )
                    : entry.label}
                </button>
              )
            })}
          </div>
          {busy && (
            <div className="mt-1.5 text-[11px] text-warn">{t('plan.cmd.sent')}</div>
          )}
          {failure && (
            <div className="mt-1.5 text-[11px] text-error">
              {failure.result
                ? t(RESULT_LABELS[failure.result] ?? 'plan.cmd.unknown')
                : t('plan.cmd.failed', {
                    message: failure.message ?? t('plan.cmd.unknown'),
                  })}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
