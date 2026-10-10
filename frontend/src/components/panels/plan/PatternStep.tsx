// Step 2 — Pattern (P2 §4.2): Sweep / Cloverleaf parameters with a
// Common / Advanced split (P6: ~20 fields at once → 4 + folded rest), and the
// Generate / Regenerate action. The boundary comes from the Area step.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  generateCloverleaf,
  generateSweep,
  rectanglePolygon,
} from '../../../mission/patterns'
import { selfIntersects } from '../../../mission/polygon'
import { useMissionStore } from '../../../stores/mission'
import { usePolygonStore } from '../../../stores/polygon'
import { useUiStore } from '../../../stores/ui'
import type { CloverleafPattern } from '../../../generated-types/CloverleafPattern'
import type { GeoPoint } from '../../../generated-types/GeoPoint'
import type { SurveyPattern } from '../../../generated-types/SurveyPattern'
import type { AreaStepProps } from './AreaStep'

export type PatternKind = 'sweep' | 'cloverleaf'

export interface PatternStepProps {
  kind: PatternKind
  onKind: (kind: PatternKind) => void
  sweep: SurveyPattern
  onSweep: (sweep: SurveyPattern) => void
  clover: CloverleafPattern
  onClover: (clover: CloverleafPattern) => void
  /** Rectangle size, shared with the Area step. */
  size: AreaStepProps['size']
}

export default function PatternStep({ kind, onKind, sweep, onSweep, clover, onClover, size }: PatternStepProps) {
  const { t } = useTranslation()
  const [advanced, setAdvanced] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const insertPattern = useMissionStore((s) => s.insertPattern)
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const center = useMissionStore((s) => s.patternCenter)
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const polygonBad = polygonClosed && selfIntersects(polygonVertices)

  const effectiveCenter: GeoPoint | null =
    center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const usePolygonBoundary = polygonClosed && !polygonBad

  const onGenerate = async () => {
    setError(null)
    setBusy(true)
    try {
      if (kind === 'sweep') {
        let boundary: GeoPoint[]
        if (usePolygonBoundary) boundary = polygonVertices
        else if (effectiveCenter) boundary = rectanglePolygon(effectiveCenter, size.width, size.height)
        else return
        const plan = await generateSweep({
          ...sweep,
          polygon: boundary,
          tie_spacing_m: sweep.tie_spacing_m && sweep.tie_spacing_m > 0 ? sweep.tie_spacing_m : null,
          speed_mps: sweep.speed_mps && sweep.speed_mps > 0 ? sweep.speed_mps : null,
        })
        insertPattern(plan, t('plan.pattern.kind.sweep'), effectiveCenter)
      } else {
        if (!effectiveCenter) return
        const plan = await generateCloverleaf({
          ...clover,
          center: effectiveCenter,
          speed_mps: clover.speed_mps && clover.speed_mps > 0 ? clover.speed_mps : null,
        })
        insertPattern(plan, t('plan.pattern.kind.cloverleaf'), effectiveCenter)
      }
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  const lineCounts = lastPattern
    ? {
        survey: lastPattern.lines.filter((l) => l.kind === 'survey').length,
        tie: lastPattern.lines.filter((l) => l.kind === 'tie').length,
        cal: lastPattern.lines.filter((l) => l.kind === 'calibration').length,
      }
    : null

  return (
    <div className="space-y-2">
      <div className="flex rounded-md border border-line bg-canvas p-0.5 text-xs">
        {(['sweep', 'cloverleaf'] as const).map((k) => (
          <button
            key={k}
            onClick={() => onKind(k)}
            className={`flex-1 rounded px-1 py-1 transition-colors ${
              kind === k ? 'bg-accent text-canvas' : 'text-muted hover:text-ink'
            }`}
          >
            {t(`plan.pattern.kind.${k}`)}
          </button>
        ))}
      </div>

      {kind === 'sweep' ? (
        <>
          {/* Common (P6): the four fields that define the flight. */}
          <div className="grid grid-cols-2 gap-1.5">
            <Num label={t('plan.pattern.spacing')} value={sweep.line_spacing_m} onChange={(v) => onSweep({ ...sweep, line_spacing_m: v })} />
            <Num label={t('plan.pattern.lineAzimuth')} value={sweep.line_azimuth_deg} onChange={(v) => onSweep({ ...sweep, line_azimuth_deg: v })} />
            <Num label={t('plan.pattern.altitude')} value={sweep.altitude_amsl_m} onChange={(v) => onSweep({ ...sweep, altitude_amsl_m: v })} />
            <Num label={t('plan.pattern.speed')} value={sweep.speed_mps ?? 0} onChange={(v) => onSweep({ ...sweep, speed_mps: v })} />
          </div>
          {/* Advanced: folded by default (P6). */}
          <button
            onClick={() => setAdvanced((a) => !a)}
            className="w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-muted hover:text-ink"
          >
            {advanced ? '▾ ' : '▸ '}
            {t('plan.pattern.advanced')}
          </button>
          {advanced && (
            <div className="grid grid-cols-2 gap-1.5">
              <Num label={t('plan.pattern.tieSpacing')} value={sweep.tie_spacing_m ?? 0} onChange={(v) => onSweep({ ...sweep, tie_spacing_m: v })} />
              <Num label={t('plan.pattern.tieAzimuth')} value={sweep.tie_azimuth_deg} onChange={(v) => onSweep({ ...sweep, tie_azimuth_deg: v })} />
              <Num label={t('plan.pattern.leadIn')} value={sweep.lead_in_m} onChange={(v) => onSweep({ ...sweep, lead_in_m: v })} />
              <Num label={t('plan.pattern.leadOut')} value={sweep.lead_out_m} onChange={(v) => onSweep({ ...sweep, lead_out_m: v })} />
              <label className="col-span-2 flex items-center gap-2 text-xs text-muted">
                <input
                  type="checkbox"
                  checked={sweep.alternate}
                  onChange={(e) => onSweep({ ...sweep, alternate: e.target.checked })}
                />
                {t('plan.pattern.alternate')}
              </label>
            </div>
          )}
          <div className="flex flex-col gap-0.5 text-[11px]">
            {usePolygonBoundary ? (
              <span className="text-accent">{t('plan.pattern.usePolygon')}</span>
            ) : polygonClosed && polygonBad ? (
              <span className="text-error">{t('plan.pattern.boundarySelfIntersect')}</span>
            ) : (
              <span className="text-muted">
                {t('plan.pattern.useRectangle')}
                {!effectiveCenter && ` · ${t('plan.pattern.noCenter')}`}
              </span>
            )}
          </div>
        </>
      ) : (
        <>
          <div className="grid grid-cols-2 gap-1.5">
            <Num label={t('plan.pattern.altitude')} value={clover.altitude_amsl_m} onChange={(v) => onClover({ ...clover, altitude_amsl_m: v })} />
            <Num label={t('plan.pattern.speed')} value={clover.speed_mps ?? 0} onChange={(v) => onClover({ ...clover, speed_mps: v })} />
          </div>
          <button
            onClick={() => setAdvanced((a) => !a)}
            className="w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-muted hover:text-ink"
          >
            {advanced ? '▾ ' : '▸ '}
            {t('plan.pattern.advanced')}
          </button>
          {advanced && (
            <div className="grid grid-cols-2 gap-1.5">
              <Num label={t('plan.pattern.radius')} value={clover.radius_m} onChange={(v) => onClover({ ...clover, radius_m: v })} />
              <Num label={t('plan.pattern.petals')} step="2" value={clover.petals} onChange={(v) => onClover({ ...clover, petals: Math.round(v) })} />
              <Num label={t('plan.pattern.samples')} step="2" value={clover.samples_per_petal} onChange={(v) => onClover({ ...clover, samples_per_petal: Math.max(1, Math.round(v)) })} />
              <Num label={t('plan.pattern.heading')} value={clover.start_heading_deg} onChange={(v) => onClover({ ...clover, start_heading_deg: v })} />
            </div>
          )}
        </>
      )}

      <button
        disabled={busy || (!effectiveCenter && !usePolygonBoundary) || polygonBad}
        onClick={() => void onGenerate()}
        title={lastPattern ? t('plan.pattern.replaces') : undefined}
        className="w-full rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
      >
        {lastPattern ? t('plan.pattern.regenerate') : t('plan.pattern.generate')}
      </button>

      {!effectiveCenter && !usePolygonBoundary && (
        <div className="text-[11px] text-warn">{t('plan.pattern.polygonHint')}</div>
      )}
      {lastPattern && (
        <div className="text-[11px] text-muted">{t('plan.pattern.replaces')}</div>
      )}
      {error && <div className="text-[11px] text-error">{t('plan.pattern.failed', { message: error })}</div>}
      {lineCounts && (
        <div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[11px]">
          <span className="text-accent">
            {t('plan.pattern.legend.survey')} {lineCounts.survey}
          </span>
          <span className="text-warn">
            {t('plan.pattern.legend.tie')} {lineCounts.tie}
          </span>
          <span className="text-mag">
            {t('plan.pattern.legend.cal')} {lineCounts.cal}
          </span>
        </div>
      )}
    </div>
  )
}

function Num({
  label,
  value,
  onChange,
  step = '1',
}: {
  label: string
  value: number
  onChange: (v: number) => void
  step?: string
}) {
  return (
    <label className="flex flex-col gap-0.5 text-[10px] text-muted">
      <span className="truncate">{label}</span>
      <input
        type="number"
        step={step}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="mono w-full rounded border border-line bg-canvas px-1.5 py-1 text-xs text-ink"
      />
    </label>
  )
}
