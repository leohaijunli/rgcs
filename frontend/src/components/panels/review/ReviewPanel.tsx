// Stage 5 — Review (U1): the last flight's record summary — session state,
// file location and counters from the recorder. The full event log lives in
// the alert center (TopBar's list button); line QC and the re-fly list land
// with M3/M6.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { isTauri } from '../../../inspector/mock'
import type { DrapeFlightReport } from '../../../generated-types/DrapeFlightReport'
import type { DemBiasOutcome } from '../../../generated-types/DemBiasOutcome'

interface RecorderStatus {
  running: boolean
  dir: string | null
  stats: { frames: number; drape: number; events: number; dropped: number; bytes: number } | null
  /** The session's forced-arm audit (L5), if any. */
  forced_arm: { reason: string; note: string | null; time_utc_us: number } | null
}

const POLL_MS = 2_000

export default function ReviewPanel() {
  const { t } = useTranslation()
  const [status, setStatus] = useState<RecorderStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [drape, setDrape] = useState<DrapeFlightReport | null>(null)
  const [bias, setBias] = useState<DemBiasOutcome | null>(null)
  const [exported, setExported] = useState<string | null>(null)

  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    const tick = async () => {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        const s = await invoke<RecorderStatus>('recorder_status')
        if (cancelled) return
        setStatus(s)
        setError(null)
        // The drape summary (DR7): only when a session log exists.
        try {
          const report = await invoke<DrapeFlightReport | null>('drape_report')
          if (!cancelled) setDrape(report)
          const bias = await invoke<DemBiasOutcome | null>('drape_bias')
          if (!cancelled) setBias(bias)
        } catch {
          setDrape(null)
          setBias(null)
        }
      } catch (e) {
        if (!cancelled) setError(String(e))
      }
    }
    tick()
    const timer = setInterval(tick, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  return (
    <div className="space-y-2" data-testid="review-panel">
      {error && <div className="text-[11px] text-warn">{error}</div>}
      <div className="rounded border border-line bg-canvas px-2 py-1.5 text-xs">
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('review.recording')}</span>
          <span className={status?.running ? 'text-ok' : 'text-muted'}>
            {status?.running ? t('review.running') : t('review.idle')}
          </span>
        </div>
        <div className="mt-1 flex items-center justify-between">
          <span className="text-muted">{t('review.sessionDir')}</span>
          <span className="mono max-w-[150px] truncate" title={status?.dir ?? undefined}>
            {status?.dir ? status.dir.split(/[\\/]/).pop() : '—'}
          </span>
        </div>
        {status?.stats && (
          <div className="mt-1 flex items-center justify-between">
            <span className="text-muted">{t('review.frames')}</span>
            <span className="mono">
              {status.stats.frames.toLocaleString()} · {status.stats.events.toLocaleString()} ev ·{' '}
              {status.stats.dropped} drop
            </span>
          </div>
        )}
        {status?.forced_arm && (
          <div className="mt-1 flex items-center justify-between" data-testid="forced-arm-mark">
            <span className="text-warn">{t('review.forcedArm')}</span>
            <span className="text-warn" title={status.forced_arm.note ?? undefined}>
              {status.forced_arm.reason} ·{' '}
              {new Date(status.forced_arm.time_utc_us / 1000).toLocaleString()}
            </span>
          </div>
        )}
      </div>
      {drape != null && drape.lines.length > 0 && (
        <div className="rounded border border-line bg-canvas px-2 py-1.5 text-xs" data-testid="drape-summary">
          <div className="mb-1 text-muted">{t('review.drapeSummary')}</div>
          <table className="w-full">
            <tbody>
              {drape.lines.map((l) => (
                <tr key={l.line_id}>
                  <td className="mono pr-2 text-ink">{l.line_id}</td>
                  <td className="mono pr-2 text-muted">
                    {t('review.inBand', { pct: Math.round(l.in_band_ratio * 100) })}
                  </td>
                  <td className="mono pr-2 text-muted">
                    e_agl {l.e_agl_worst_m >= 0 ? '+' : ''}
                    {l.e_agl_worst_m.toFixed(1)} m
                  </td>
                  {l.min_clearance_m != null && (
                    <td className="mono text-muted">clr {l.min_clearance_m.toFixed(1)} m</td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
          <button
            onClick={() => {
              void import('@tauri-apps/api/core').then(async ({ invoke }) => {
                try {
                  const dir = await invoke<string | null>('drape_export')
                  if (dir) setExported(dir)
                } catch {
                  /* the report said it existed */
                }
              })
            }}
            className="mt-1 w-full rounded border border-line px-2 py-1 text-[11px] text-muted hover:bg-panel hover:text-ink"
            data-testid="drape-export"
          >
            {t('review.drapeExport')}
          </button>
          {exported != null && (
            <div className="mt-1 truncate text-[10px] text-ok" title={exported}>
              {exported}/drape-summary.csv
            </div>
          )}
          {typeof bias === 'object' && bias && 'estimate' in bias && (
            <div className="mt-1 text-[11px]" data-testid="drape-bias">
              <span className="text-muted">{t('review.biasEstimate')}: </span>
              <span className="mono text-ink">
                {bias.estimate.bias_m >= 0 ? '+' : ''}
                {bias.estimate.bias_m.toFixed(2)} ± {bias.estimate.uncertainty_m.toFixed(2)} m
              </span>
              <div className="text-muted">
                {t('review.offsetCandidate', {
                  m:
                    (bias.estimate.vertical_offset_candidate_m >= 0 ? '+' : '') +
                    bias.estimate.vertical_offset_candidate_m.toFixed(2),
                })}
              </div>
            </div>
          )}
          {typeof bias === 'object' && bias && 'insufficient_samples' in bias && (
            <div className="mt-1 text-[11px] text-muted">
              {t('review.biasTooFew', { n: bias.insufficient_samples.samples })}
            </div>
          )}
        </div>
      )}
      <p className="text-[11px] text-muted">{t('review.eventLogHint')}</p>
    </div>
  )
}
