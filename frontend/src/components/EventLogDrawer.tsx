// Event log drawer (operator request 2026-10-11): everything the shell
// does that an operator should be able to look back at — command acks,
// mode switches, arm/disarm, RTK alarms — in one timestamped side drawer.

import { useTranslation } from 'react-i18next'
import { useEventLog, type EventLevel } from '../stores/eventlog'

const LEVEL_CLASS: Record<EventLevel, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  err: 'text-error',
  info: 'text-muted',
}

export default function EventLogDrawer({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}) {
  const { t } = useTranslation()
  const events = useEventLog((s) => s.events)
  const clear = useEventLog((s) => s.clear)

  if (!open) return null

  return (
    <div className="fixed inset-0 z-40">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <aside
        className="absolute right-0 top-0 flex h-full w-[380px] flex-col border-l border-line bg-panel shadow-2xl"
        role="dialog"
        aria-label={t('eventLog.title')}
      >
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 className="text-sm font-medium text-ink">{t('eventLog.title')}</h2>
          <div className="flex items-center gap-2">
            <button
              onClick={clear}
              className="touch-target rounded px-2 py-1 text-xs text-muted hover:text-ink"
            >
              {t('eventLog.clear')}
            </button>
            <button
              onClick={onClose}
              aria-label="close"
              className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
            >
              ×
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {events.length === 0 ? (
            <div className="text-xs text-muted">{t('eventLog.empty')}</div>
          ) : (
            <ul className="space-y-1">
              {events.map((e, i) => (
                <li
                  key={`${e.ts}-${i}`}
                  className="rounded border border-line bg-canvas px-2 py-1 text-xs"
                >
                  <span className="mono mr-2 text-[10px] text-muted">
                    {new Date(e.ts).toLocaleTimeString()}
                  </span>
                  <span className={`mono mr-2 text-[10px] uppercase ${LEVEL_CLASS[e.level]}`}>
                    {e.source}
                  </span>
                  <span className={LEVEL_CLASS[e.level]}>{e.text}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </aside>
    </div>
  )
}
