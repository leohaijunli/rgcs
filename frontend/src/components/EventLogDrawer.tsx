// Event log drawer (operator request 2026-10-11): everything the shell
// does that an operator should be able to look back at — command acks,
// mode switches, arm/disarm, RTK alarms — in one timestamped side drawer.
//
// Since A3 it is also the **alert center surface**: the Alerts tab lists the
// backend's alert center (deduplicated, severity-coloured, acknowledged in
// Rust so every window agrees), the Events tab keeps the local event ring.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Check } from 'lucide-react'
import { useEventLog, type EventLevel } from '../stores/eventlog'
import { useAlerts } from '../stores/alerts'
import { StatusDot, Tabs, type Tone } from '../design-system/components'
import type { Severity } from '../generated-types/Severity'

const LEVEL_CLASS: Record<EventLevel, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  err: 'text-error',
  info: 'text-muted',
}

const SEVERITY_CLASS: Record<Severity, string> = {
  info: 'text-muted',
  warn: 'text-warn',
  critical: 'text-error',
}

const SEVERITY_TONE: Record<Severity, Tone> = {
  info: 'muted',
  warn: 'warn',
  critical: 'error',
}

type AlertTab = 'alerts' | 'events'

export default function EventLogDrawer({
  open,
  onClose,
}: {
  open: boolean
  onClose: () => void
}) {
  const { t } = useTranslation()
  const events = useEventLog((s) => s.events)
  const clearEvents = useEventLog((s) => s.clear)
  const alerts = useAlerts((s) => s.alerts)
  const visibleCount = useAlerts((s) => s.count)
  const ack = useAlerts((s) => s.ack)
  const ackAll = useAlerts((s) => s.ackAll)
  const clearAcknowledged = useAlerts((s) => s.clearAcknowledged)
  const [tab, setTab] = useState<AlertTab>('alerts')

  if (!open) return null

  const openAlerts = alerts.filter((a) => !a.acked).length

  return (
    <div className="fixed inset-0 z-40">
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <aside
        data-testid="alert-drawer"
        className="absolute right-0 top-0 flex h-full w-[380px] flex-col border-l border-line bg-panel shadow-2xl"
        role="dialog"
        aria-label={t('alerts.open')}
      >
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 className="text-sm font-medium text-ink">{t('alerts.title')}</h2>
          <div className="flex items-center gap-2">
            {openAlerts > 0 && (
              <button
                data-testid="alert-ack-all"
                onClick={ackAll}
                className="touch-target rounded px-2 py-1 text-xs text-ink hover:text-accent"
              >
                {t('alerts.ackAll')}
              </button>
            )}
            <button
              data-testid="alert-clear"
              onClick={() => {
                clearAcknowledged()
                clearEvents()
              }}
              className="touch-target rounded px-2 py-1 text-xs text-muted hover:text-ink"
            >
              {t('alerts.clear')}
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

        <div className="border-b border-line px-3 py-2">
          <Tabs
            items={[
              {
                id: 'alerts',
                label:
                  visibleCount > 0 ? `${t('alerts.title')} (${visibleCount})` : t('alerts.title'),
              },
              { id: 'events', label: t('alerts.eventsTab') },
            ]}
            active={tab}
            onChange={setTab}
          />
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {tab === 'alerts' ? (
            alerts.length === 0 ? (
              <div className="text-xs text-muted">{t('alerts.empty')}</div>
            ) : (
              <ul className="space-y-1">
                {alerts.map((a) => (
                  <li
                    key={a.id}
                    data-testid="alert-row"
                    data-alert-id={a.id}
                    data-acked={String(a.acked)}
                    className={`flex items-start gap-2 rounded border border-line px-2 py-1 text-xs ${
                      a.acked ? 'bg-canvas opacity-60' : 'bg-canvas'
                    }`}
                  >
                    <span className={`mono mt-0.5 text-[10px] ${SEVERITY_CLASS[a.severity]}`}>
                      {t(`alerts.severity.${a.severity}`)}
                    </span>
                    <span className="mt-1.5">
                      <StatusDot tone={SEVERITY_TONE[a.severity]} />
                    </span>
                    <span className="flex-1">
                      <span className="mr-1 text-[10px] uppercase text-muted">
                        {t(`alerts.source.${a.source}`)}
                      </span>
                      <span className={a.acked ? 'text-muted' : 'text-ink'}>{a.text}</span>
                      <span className="ml-2 text-[10px] text-muted">
                        {new Date(a.last_ts_ms).toLocaleTimeString()}
                        {a.count > 1 && ` · ×${a.count}`}
                      </span>
                    </span>
                    {!a.acked && (
                      <button
                        data-testid="alert-ack"
                        aria-label={t('alerts.ack')}
                        title={t('alerts.ack')}
                        onClick={() => ack(a.id)}
                        className="touch-target flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted hover:text-ink"
                      >
                        <Check size={12} />
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )
          ) : events.length === 0 ? (
            <div className="text-xs text-muted">{t('eventLog.empty')}</div>
          ) : (
            <ul className="space-y-1">
              {events.map((e, i) => (
                <li
                  key={`${e.ts}-${i}`}
                  data-testid="event-row"
                  data-event-source={e.source}
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
