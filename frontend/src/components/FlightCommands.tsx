import { useTranslation } from 'react-i18next'
import { useEffect, useRef, useState } from 'react'
import type { CommandEventPayload } from '../stores/command'
import { useCommandStore, type CommandName } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useUiStore } from '../stores/ui'

/** How long a disruptive button stays armed while waiting for the second click. */
const CONFIRM_WINDOW_MS = 5000

/** Commands that need a second click (they change what the aircraft is doing). */
const CONFIRM: ReadonlyArray<CommandName> = ['rtl', 'pause']

/** Terminal ack results mapped onto the status line. */
const RESULT_LABELS: Record<string, string> = {
  accepted: 'plan.cmd.accepted',
  denied: 'plan.cmd.denied',
  unsupported: 'plan.cmd.unsupported',
  temporarily_rejected: 'plan.cmd.rejected',
  failed: 'plan.cmd.rejected',
}

export default function FlightCommands() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const link = useLinkStore((s) => s.link)
  const pending = useCommandStore((s) => s.pending)
  const lastEvent = useCommandStore((s) => s.lastEvent)
  const send = useCommandStore((s) => s.send)
  const [confirming, setConfirming] = useState<CommandName | null>(null)
  const confirmTimer = useRef<number | undefined>(undefined)

  useEffect(() => () => window.clearTimeout(confirmTimer.current), [])

  if (view !== 'flight' || !link?.fc_alive) return null

  // Disruptive commands (RTL, pause) arm on the first click; a second click
  // within the confirm window sends them. Others (continue) send immediately.
  const press = (name: CommandName) => {
    if (!CONFIRM.includes(name)) {
      void send(name)
      return
    }
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

  const status = pending
    ? { tone: 'text-warn', text: t('plan.cmd.sent') }
    : statusLine(t, lastEvent)

  return (
    <div className="pointer-events-auto absolute right-3 top-3 z-10 flex items-center gap-2 rounded border border-line bg-panel/85 p-1.5 shadow-xl backdrop-blur">
      {(['pause', 'continue', 'rtl'] as const).map((name) => {
        const armed = confirming === name
        const label = armed
          ? t(name === 'rtl' ? 'plan.cmd.confirmRtl' : 'plan.cmd.confirmPause')
          : t(`plan.${name}`)
        return (
          <button
            key={name}
            onClick={() => press(name)}
            disabled={pending !== null}
            className={`rounded-md border border-line px-3 py-1.5 text-xs font-medium transition-colors disabled:opacity-50 ${
              armed
                ? 'bg-warn text-canvas'
                : 'bg-canvas text-ink hover:bg-accent hover:text-canvas'
            }`}
          >
            {label}
          </button>
        )
      })}
      {status && <span className={`pr-1 text-xs ${status.tone}`}>{status.text}</span>}
    </div>
  )
}

/** Render the last command outcome, or null when there is nothing to show. */
function statusLine(
  t: (key: string, options?: Record<string, unknown>) => string,
  e: CommandEventPayload | null,
): { tone: string; text: string } | null {
  if (!e) return null
  if (e.kind === 'failed') {
    return {
      tone: 'text-error',
      text: t('plan.cmd.failed', { message: e.message ?? t('plan.cmd.unknown') }),
    }
  }
  const label = e.result ? RESULT_LABELS[e.result] : undefined
  if (!label) return { tone: 'text-muted', text: t('plan.cmd.unknown') }
  return {
    tone: e.result === 'accepted' ? 'text-ok' : 'text-warn',
    text: t(label),
  }
}
