import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { BaseStatus } from '../generated-types/BaseStatus'
import type { GpsRawInt } from '../generated-types/GpsRawInt'
import type { RtkStats } from '../generated-types/RtkStats'
import { FIELD_STALE_MS } from '../hooks/useDataAge'

type MetricState = 'live' | 'stale' | 'unavailable'
interface RtkGpsDashboardProps {
  base: BaseStatus | null
  stats: RtkStats | null
  gps: GpsRawInt | null
  baseAgeMs: number
  roverAgeMs: number
}

const RTCM_STALE_S = 5
const HISTORY_SECONDS = 60
const MAX_SAMPLES = 60
const CHART_WIDTH = 280
const CHART_HEIGHT = 48
const CHART_PADDING = 3
const RING_RADIUS = 25
const RING_LENGTH = 2 * Math.PI * RING_RADIUS
const ACCURACY_UNITS_PER_M = 10_000
const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value)
const fraction = (value: number) => Math.max(0, Math.min(1, value))

function Readout({ label, value, unit = '', state, id }: { label: string; value: string; unit?: string; state: MetricState; id?: string }) {
  const { t } = useTranslation()
  return <div className="min-w-0 rounded border border-line bg-panel px-2 py-2" data-testid={id} data-state={state}>
    <div className="text-[10px] uppercase tracking-wide text-muted">{label}</div>
    <div className={`mono mt-1 truncate text-lg font-semibold ${state === 'live' ? 'text-ink' : 'text-muted'}`}>{value}<span className="ml-1 text-[10px] font-normal">{unit}</span></div>
    <div className="mt-1 text-[9px] tracking-wide text-muted">{t(`rtkHud.states.${state}`)}</div>
  </div>
}

export default function RtkGpsDashboard({ base, stats, gps, baseAgeMs, roverAgeMs }: RtkGpsDashboardProps): JSX.Element {
  const { t } = useTranslation()
  const [history, setHistory] = useState<Array<{ at: number; value: number }>>([])
  const previous = useRef<{ stats: RtkStats | null; scope: string }>({ stats: null, scope: '' })
  useEffect(() => {
    const scope = `${base?.session_id ?? ''}:${stats?.connection_generation ?? ''}:${stats?.source ?? ''}`
    if (previous.current.stats === stats && previous.current.scope === scope) return
    const reset = previous.current.scope !== scope
    previous.current = { stats, scope }
    if (!stats || !finite(stats.bytes_per_second) || stats.bytes_per_second < 0) { setHistory([]); return }
    const at = Date.now()
    setHistory((points) => [...(reset ? [] : points).filter((p) => at - p.at <= HISTORY_SECONDS * 1000).slice(-(MAX_SAMPLES - 1)), { at, value: stats.bytes_per_second }])
  }, [stats, base?.session_id])

  const activeBase = base != null && ['identifying', 'configuring', 'surveying', 'ready'].includes(base.phase)
  const baseState: MetricState = !base ? 'unavailable' : !activeBase || baseAgeMs > FIELD_STALE_MS ? 'stale' : 'live'
  const roverState: MetricState = !gps ? 'unavailable' : roverAgeMs > FIELD_STALE_MS ? 'stale' : 'live'
  const correctionAge = finite(base?.seconds_since_rtcm) && finite(baseAgeMs) ? Math.max(0, base.seconds_since_rtcm + baseAgeMs / 1000) : null
  const correctionState: MetricState = correctionAge == null ? 'unavailable' : baseState !== 'live' || correctionAge > RTCM_STALE_S ? 'stale' : 'live'
  const txState: MetricState = !stats ? 'unavailable' : baseState !== 'live' || stats.stalled ? 'stale' : 'live'
  const satellites = finite(gps?.satellites_visible) && gps.satellites_visible !== 255 ? gps.satellites_visible : null
  const fix = !gps ? 'unavailable' : gps.fix_type === 'RTK_FIXED' ? 'fixed' : gps.fix_type === 'RTK_FLOAT' ? 'float'
    : gps.fix_type === 'FIX3D' ? 'threeD' : gps.fix_type === 'FIX2D' ? 'twoD' : ['NO_GPS', 'NO_FIX'].includes(gps.fix_type) ? 'noFix' : 'other'
  const fixTone = roverState !== 'live' ? 'border-line text-muted' : fix === 'fixed' ? 'border-ok text-ok' : 'border-warn text-warn'
  const survey = base?.survey_in
  const target = base?.mode?.kind === 'survey_in' ? base.mode.config : null
  const elapsed = survey && target ? fraction(survey.duration_s / target.min_duration_s) : 0
  const precision = survey && target ? fraction(target.accuracy_limit_m / Math.max(survey.mean_accuracy_m, 1 / ACCURACY_UNITS_PER_M)) : 0
  const ready = baseState === 'live' && base?.phase === 'ready' && survey?.valid === true && survey.active === false
  const peak = Math.max(1, ...history.map((p) => p.value))
  const lastTime = history[history.length - 1]?.at ?? 0
  const chartSpanMs = Math.max(1000, Math.min(HISTORY_SECONDS * 1000, lastTime - (history[0]?.at ?? lastTime)))
  const line = history.map((point, index) => {
    const x = CHART_PADDING + (1 - Math.min(chartSpanMs, lastTime - point.at) / chartSpanMs) * (CHART_WIDTH - 2 * CHART_PADDING)
    const y = CHART_HEIGHT - CHART_PADDING - point.value / peak * (CHART_HEIGHT - 2 * CHART_PADDING)
    return `${index === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`
  }).join(' ')

  return <section aria-label={t('rtkHud.title')} data-testid="rtk-gps-dashboard" className="space-y-2 rounded border border-line bg-canvas p-3">
    <div className="flex items-center justify-between gap-2">
      <h3 className="text-[11px] font-medium uppercase tracking-wide text-ink">{t('rtkHud.title')}</h3>
      <span className="text-[10px] text-muted">{t('rtkHud.receiverReported')}</span>
    </div>
    <div className={`flex items-center justify-between rounded border px-3 py-2 ${fixTone}`} data-testid="rtk-hud-fix" data-state={roverState}>
      <div><div className="text-[10px] uppercase tracking-wide">{t('rtkHud.solution')}</div><strong className="mono text-2xl tracking-wide">{t(`rtkHud.fix.${fix}`)}</strong></div>
      <span className="text-[10px]">{t(`rtkHud.states.${roverState}`)}</span>
    </div>
    <div className="grid grid-cols-3 gap-2">
      <Readout label={t('rtkHud.satellites')} value={satellites == null ? '—' : String(satellites)} state={satellites == null ? 'unavailable' : roverState} />
      <Readout label="HDOP" value={finite(gps?.hdop) ? gps.hdop.toFixed(2) : '—'} state={finite(gps?.hdop) ? roverState : 'unavailable'} />
      <Readout label="VDOP" value={finite(gps?.vdop) ? gps.vdop.toFixed(2) : '—'} state={finite(gps?.vdop) ? roverState : 'unavailable'} />
    </div>
    <div className="grid grid-cols-2 gap-2">
      <div className="flex items-center gap-2 rounded border border-line bg-panel p-2" data-testid="rtk-hud-age" data-state={correctionState}>
        <div className={`relative h-16 w-16 shrink-0 ${correctionState === 'live' ? 'text-ok' : correctionState === 'stale' ? 'text-warn' : 'text-muted'}`}>
          <svg viewBox="0 0 64 64" className="h-full w-full -rotate-90" aria-hidden="true">
            <circle cx="32" cy="32" r={RING_RADIUS} fill="none" stroke="currentColor" strokeWidth="3" className="text-line" />
            {correctionAge != null && <circle cx="32" cy="32" r={RING_RADIUS} fill="none" stroke="currentColor" strokeWidth="3" strokeDasharray={`${fraction(correctionAge / RTCM_STALE_S) * RING_LENGTH} ${RING_LENGTH}`} />}
          </svg>
          <div className="mono absolute inset-0 flex items-center justify-center text-xs">{correctionAge == null ? '—' : `${correctionAge.toFixed(1)}s`}</div>
        </div>
        <div className="min-w-0 text-[10px] text-muted"><div>{t('rtkHud.rtcmAge')}</div><div className="mt-1">{t(`rtkHud.states.${correctionState}`)}</div></div>
      </div>
      <Readout label={t('rtkHud.localTx')} value={stats && finite(stats.bytes_per_second) ? stats.bytes_per_second.toFixed(0) : '—'} unit="B/s" state={txState} id="rtk-hud-tx" />
    </div>
    <div className="rounded border border-line bg-panel px-2 py-1" data-testid="rtk-hud-trend" data-samples={history.length}>
      <div className="flex justify-between text-[9px] text-muted"><span>{t('rtkHud.trend')}</span><span>≤{HISTORY_SECONDS} s</span></div>
      <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className={`h-12 w-full ${txState === 'live' ? 'text-accent' : 'text-muted'}`} role="img" aria-label={t('rtkHud.trend')}>
        <path d={`M0,${CHART_HEIGHT - CHART_PADDING}H${CHART_WIDTH}`} fill="none" stroke="currentColor" className="text-line" />
        {history.length > 1 && <path d={line} fill="none" stroke="currentColor" strokeWidth="2" />}
        {history.length === 1 && <circle cx={CHART_WIDTH - CHART_PADDING} cy={CHART_HEIGHT - CHART_PADDING - history[0].value / peak * (CHART_HEIGHT - 2 * CHART_PADDING)} r="2" fill="currentColor" />}
        {history.length === 0 && <text x={CHART_WIDTH / 2} y={CHART_HEIGHT / 2} textAnchor="middle" dominantBaseline="middle" fill="currentColor" fontSize="10">{t('rtkHud.noSamples')}</text>}
      </svg>
    </div>
    {target && <div className={`space-y-2 rounded border border-line bg-panel p-2 ${baseState === 'live' ? '' : 'opacity-60'}`} data-testid="rtk-hud-survey" data-state={survey ? baseState : 'unavailable'}>
      <div className="flex justify-between text-[10px]"><span className="text-muted">{t('rtkHud.survey')}</span><strong className={ready ? 'text-ok' : 'text-muted'}>{t(ready ? 'rtkHud.valid' : 'rtkHud.waitingValidity')}</strong></div>
      <div className="flex justify-between text-[10px] text-muted"><span>{t('rtkHud.time')}</span><span className="mono">{survey?.duration_s ?? '—'} s / ≥{target.min_duration_s} s</span></div>
      <div role="progressbar" aria-label={t('rtkHud.time')} aria-valuenow={Math.round(elapsed * 100)} aria-valuemin={0} aria-valuemax={100} className="h-1.5 overflow-hidden rounded bg-line"><div className="h-full bg-accent" style={{ width: `${elapsed * 100}%` }} /></div>
      <div className="flex justify-between text-[10px] text-muted"><span>{t('rtkHud.precision')}</span><span className="mono">{survey ? survey.mean_accuracy_m.toFixed(3) : '—'} m / ≤{target.accuracy_limit_m.toFixed(2)} m</span></div>
      <div role="progressbar" aria-label={t('rtkHud.precision')} aria-valuenow={Math.round(precision * 100)} aria-valuemin={0} aria-valuemax={100} className="h-1.5 overflow-hidden rounded bg-line"><div className="h-full bg-accent" style={{ width: `${precision * 100}%` }} /></div>
    </div>}
    <p className="text-[9px] leading-relaxed text-muted">{t('rtkHud.evidenceHint')}</p>
  </section>
}
