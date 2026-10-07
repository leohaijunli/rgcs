import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { ChevronDown, X } from 'lucide-react'
import { useLinkStore } from '../stores/link'

/** Time-of-day label for an error history entry. */
function formatTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString()
}

/**
 * Link error banner with history (issue #17).
 *
 * The backend delivers errors losslessly with a category and timestamp; the
 * store keeps a capped history, and this banner surfaces the latest one plus a
 * expandable list so bursts are not silently collapsed into a single message.
 */
export default function ErrorBanner() {
  const { t } = useTranslation()
  const lastError = useLinkStore((s) => s.lastError)
  const errorHistory = useLinkStore((s) => s.errorHistory)
  const droppedFrames = useLinkStore((s) => s.droppedFrames)
  const clearErrors = useLinkStore((s) => s.clearErrors)
  const [expanded, setExpanded] = useState(false)

  if (!lastError && droppedFrames === 0) return null

  const latest = errorHistory[0]

  return (
    <div className="absolute bottom-3 right-3 z-20 flex max-w-sm flex-col gap-2 rounded border border-line bg-panel/95 px-3 py-2 text-sm shadow-xl">
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
  )
}
