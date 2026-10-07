import { useTranslation } from 'react-i18next'
import { useEffect, useRef, useState } from 'react'
import type { CommandEventPayload } from '../stores/command'
import { useCommandStore } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useUiStore } from '../stores/ui'

/** How long the RTL button stays armed while waiting for the second click. */
const CONFIRM_WINDOW_MS = 5000

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
  const [confirming, setConfirming] = useState(false)
  const confirmTimer = useRef<number | undefined>(undefined)

  useEffect(() => () => window.clearTimeout(confirmTimer.current), [])

  if (view !== 'flight' || !link?.fc_alive) return null

  // RTL is disruptive, so the first click only arms the button; a second
  // click within the confirm window sends it.
  const onRtl = () => {
    if (!confirming) {
      setConfirming(true)
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(
        () => setConfirming(false),
        CONFIRM_WINDOW_MS,
      )
      return
    }
    window.clearTimeout(confirmTimer.current)
    setConfirming(false)
    void send('rtl')
  }

  const status = pending
    ? { tone: 'text-warn', text: t('plan.cmd.sent') }
    : statusLine(t, lastEvent)

  return (
    <div className="pointer-events-auto absolute right-3 top-3 z-10 flex items-center gap-2 rounded border border-line bg-panel/85 p-1.5 shadow-xl backdrop-blur">
      <button
        onClick={onRtl}
        className={`rounded-md border border-line px-3 py-1.5 text-xs font-medium transition-colors ${
          confirming
            ? 'bg-warn text-canvas'
            : 'bg-canvas text-ink hover:bg-accent hover:text-canvas'
        }`}
      >
        {confirming ? t('plan.cmd.confirmRtl') : t('plan.rtl')}
      </button>
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
