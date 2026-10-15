// Alert center drawer (plan §3 A3, §7 U3): the one place every window's alerts
// and the shell's own event log can be inspected.
//
// It replaced the thin event-log drawer with three views over the shared
// stores — Alerts (deduplicated, severity/source filtered, acknowledged in
// Rust so every window agrees), Events (the local shell ring), and the Console
// (the FC's own STATUSTEXT lines). The header carries the alarm-sound controls
// (volume + a time-boxed mute that re-arms itself).

import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertOctagon, AlertTriangle, Check, Info, Volume2, VolumeX } from 'lucide-react'
import { useEventLog, type EventLevel } from '../stores/eventlog'
import { useAlerts } from '../stores/alerts'
import { useAlertMuted, useAlertSound } from '../stores/alertSound'
import { StatusDot, type Tone } from '../design-system/components'
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

const SEVERITY_ICON: Record<Severity, typeof Info> = {
  info: Info,
  warn: AlertTriangle,
  critical: AlertOctagon,
}

type AlertTab = 'alerts' | 'events' | 'console'
type SeverityFilter = 'all' | Severity

export default function AlertCenter({
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
  const volume = useAlertSound((s) => s.volume)
  const setVolume = useAlertSound((s) => s.setVolume)
  const mute = useAlertSound((s) => s.mute)
  const unmute = useAlertSound((s) => s.unmute)
  const { muted, remainingMs } = useAlertMuted()
  const [tab, setTab] = useState<AlertTab>('alerts')
  const [severity, setSeverity] = useState<SeverityFilter>('all')
  const [source, setSource] = useState<'all' | string>('all')

  const sources = useMemo(
    () => [...new Set(alerts.map((a) => a.source))].sort(),
    [alerts],
  )
  const shown = alerts.filter(
    (a) => (severity === 'all' || a.severity === severity) && (source === 'all' || a.source === source),
  )
  const fcEvents = events.filter((e) => e.source === 'fc')

  if (!open) return null

  const openAlerts = alerts.filter((a) => !a.acked).length
  const muteSeconds = Math.ceil(remainingMs / 1000)

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

        {/* Alarm-sound controls (U3): one volume, and a mute that expires. */}
        <div className="flex items-center gap-2 border-b border-line px-4 py-2 text-xs text-muted">
          <button
            data-testid="alert-mute"
            aria-pressed={muted}
            onClick={() => (muted ? unmute() : mute())}
            title={muted ? t('alerts.sound.unmute') : t('alerts.sound.mute')}
            className="touch-target flex h-7 items-center gap-1 rounded border border-line bg-canvas px-2 text-ink hover:bg-panel"
          >
            {muted ? <VolumeX size={13} /> : <Volume2 size={13} />}
            {muted ? `${t('alerts.sound.unmute')} · ${muteSeconds}s` : t('alerts.sound.mute')}
          </button>
          <label className="ml-auto flex items-center gap-1.5">
            {t('alerts.sound.volume')}
            <input
              data-testid="alert-volume"
              type="range"
              min={0}
              max={100}
              value={Math.round(volume * 100)}
              onChange={(e) => setVolume(Number(e.target.value) / 100)}
              className="w-24"
            />
          </label>
        </div>

        <div className="border-b border-line px-3 py-2">
          <div className="flex gap-0.5 rounded border border-line bg-canvas p-0.5">
            {(
              [
                {
                  id: 'alerts' as const,
                  label:
                    visibleCount > 0
                      ? `${t('alerts.title')} (${visibleCount})`
                      : t('alerts.title'),
                },
                { id: 'events' as const, label: t('alerts.eventsTab') },
                { id: 'console' as const, label: t('alerts.consoleTab') },
              ]
            ).map((it) => (
              <button
                key={it.id}
                data-testid={`alert-tab-${it.id}`}
                onClick={() => setTab(it.id)}
                className={`rounded px-3 py-1 text-xs transition-colors ${
                  tab === it.id ? 'bg-accent text-accent-ink' : 'text-muted hover:text-ink'
                }`}
              >
                {it.label}
              </button>
            ))}
          </div>

          {tab === 'alerts' && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[11px]">
              <span className="text-muted">{t('alerts.filterSeverity')}</span>
              {(['all', 'critical', 'warn', 'info'] as SeverityFilter[]).map((s) => (
                <button
                  key={s}
                  data-testid={`alert-filter-${s}`}
                  onClick={() => setSeverity(s)}
                  className={`touch-target rounded border px-2 py-0.5 ${
                    severity === s
                      ? 'border-accent text-accent'
                      : 'border-line text-muted hover:text-ink'
                  }`}
                >
                  {s === 'all' ? t('alerts.all') : t(`alerts.severity.${s}`)}
                </button>
              ))}
              {sources.length > 1 && (
                <label className="ml-auto flex items-center gap-1">
                  <span className="text-muted">{t('alerts.filterSource')}</span>
                  <select
                    data-testid="alert-filter-source"
                    value={source}
                    onChange={(e) => setSource(e.target.value)}
                    className="rounded border border-line bg-canvas px-1 py-0.5 text-[11px] text-ink"
                  >
                    <option value="all">{t('alerts.all')}</option>
                    {sources.map((s) => (
                      <option key={s} value={s}>
                        {t(`alerts.source.${s}`)}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
          )}
        </div>

        <div className="flex-1 overflow-y-auto p-3">
          {tab === 'alerts' ? (
            shown.length === 0 ? (
              <div className="text-xs text-muted">
                {alerts.length === 0 ? t('alerts.empty') : t('alerts.noMatches')}
              </div>
            ) : (
              <ul className="space-y-1">
                {shown.map((a) => {
                  const Icon = SEVERITY_ICON[a.severity]
                  return (
                    <li
                      key={a.id}
                      data-testid="alert-row"
                      data-alert-id={a.id}
                      data-acked={String(a.acked)}
                      data-severity={a.severity}
                      className={`flex items-start gap-2 rounded border border-line px-2 py-1 text-xs ${
                        a.acked ? 'bg-canvas opacity-60' : 'bg-canvas'
                      }`}
                    >
                      <Icon size={13} className={`mt-0.5 shrink-0 ${SEVERITY_CLASS[a.severity]}`} />
                      <span className={`mt-0.5 shrink-0 ${SEVERITY_CLASS[a.severity]}`}>
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
                  )
                })}
              </ul>
            )
          ) : tab === 'events' ? (
            events.length === 0 ? (
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
            )
          ) : fcEvents.length === 0 ? (
            <div className="text-xs text-muted">{t('alerts.consoleEmpty')}</div>
          ) : (
            <ul className="space-y-0.5">
              {fcEvents.map((e, i) => (
                <li
                  key={`${e.ts}-${i}`}
                  data-testid="console-line"
                  className="mono rounded bg-canvas px-2 py-1 text-[11px]"
                >
                  <span className="mr-2 text-muted">{new Date(e.ts).toLocaleTimeString()}</span>
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
