import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, ChevronDown, X } from 'lucide-react'
import { useLinkStore } from '../stores/link'
import { useAlerts } from '../stores/alerts'

/** Time-of-day label for an error history entry. */
function formatTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString()
}

/**
 * The banner stack: the newest unacknowledged Critical alert on top (plan §3
 * A3, U3), then the link error banner with history (issue #17).
 *
 * Critical alerts are latched in the backend, so a banner only disappears
 * when the operator acknowledges it — never because the volume of the next
 * message pushed it out of the way.
 */
export default function ErrorBanner() {
  const { t } = useTranslation()
  const critical = useAlerts((s) => s.banner)
  const ackAlert = useAlerts((s) => s.ack)
  const lastError = useLinkStore((s) => s.lastError)
  const errorHistory = useLinkStore((s) => s.errorHistory)
  const droppedFrames = useLinkStore((s) => s.droppedFrames)
  const clearErrors = useLinkStore((s) => s.clearErrors)
  const [expanded, setExpanded] = useState(false)

  const showLinkError = Boolean(lastError) || droppedFrames > 0
  if (!critical && !showLinkError) return null

  const latest = errorHistory[0]

  return (
    <div className="absolute bottom-3 right-3 z-20 flex max-w-sm flex-col gap-2">
      {critical && (
        <div
          data-testid="alert-banner"
          role="alert"
          className="rounded border border-error bg-panel/95 px-3 py-2 text-sm shadow-xl"
        >
          <div className="flex items-center gap-3">
            <AlertTriangle size={14} className="shrink-0 text-error" />
            <span className="flex-1 text-ink">{critical.text}</span>
            {critical.count > 1 && (
              <span className="mono text-[10px] text-muted">×{critical.count}</span>
            )}
            <button
              data-testid="alert-banner-ack"
              onClick={() => ackAlert(critical.id)}
              className="touch-target flex h-7 items-center rounded border border-line px-2 text-xs text-ink hover:bg-canvas"
            >
              {t('alerts.ack')}
            </button>
          </div>
          <div className="mt-1 text-[10px] text-muted">
            {t(`alerts.source.${critical.source}`)} · {formatTime(critical.last_ts_ms)}
          </div>
        </div>
      )}

      {showLinkError && (
        <div className="flex flex-col gap-2 rounded border border-line bg-panel/95 px-3 py-2 text-sm shadow-xl">
          <div className="flex items-center gap-3">
            <span className="text-error">●</span>
            <span className="flex-1 text-ink">
              {latest ? `${t(`link.error.${latest.kind}`)}: ${latest.message}` : lastError}
            </span>
            {errorHistory.length > 1 && (
              <button
                aria-label={t('link.error.history')}
                onClick={() => setExpanded((v) => !v)}
                className="touch-target flex h-7 items-center gap-1 rounded px-1 text-xs text-muted hover:text-ink"
              >
                +{errorHistory.length - 1}
                <ChevronDown size={12} className={expanded ? 'rotate-180' : ''} />
              </button>
            )}
            <button
              aria-label={t('link.error.dismiss')}
              onClick={() => {
                setExpanded(false)
                clearErrors()
              }}
              className="touch-target flex h-7 w-7 items-center justify-center rounded text-muted hover:text-ink"
            >
              <X size={14} />
            </button>
          </div>

          {droppedFrames > 0 && (
            <div className="text-xs text-warn">
              {t('link.error.dropped', { count: droppedFrames })}
            </div>
          )}

          {expanded && (
            <ul className="max-h-40 overflow-y-auto border-t border-line pt-1 text-xs text-muted">
              {errorHistory.map((e, i) => (
                <li key={`${e.at_ms}-${i}`} className="flex gap-2 py-0.5">
                  <span className="shrink-0 tabular-nums">{formatTime(e.at_ms)}</span>
                  <span className="text-ink">{e.message}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  )
}
