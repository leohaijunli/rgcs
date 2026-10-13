// Step 2 — Pattern (P2 §4.2): Sweep / Cloverleaf parameters with a
// Common / Advanced split (P6: ~20 fields at once → 4 + folded rest), and the
// Generate / Regenerate action. The boundary comes from the Area step.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  compilePattern,
  generateCloverleaf,
  generateSweep,
  rectanglePolygon,
} from '../../../mission/patterns'
import { selfIntersects } from '../../../mission/polygon'
import { useMissionStore } from '../../../stores/mission'
import { usePolygonStore } from '../../../stores/polygon'
import { useTerrainStore } from '../../../stores/terrain'
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
  const setPatternCenter = useMissionStore((s) => s.setPatternCenter)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const setSurveyAglM = useMissionStore((s) => s.setSurveyAglM)
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const polygonBad = polygonClosed && selfIntersects(polygonVertices)

  const effectiveCenter: GeoPoint | null =
    center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const usePolygonBoundary = polygonClosed && !polygonBad
  // Terrain mode: the altitude input IS the survey AGL — bound to its own
  // store field (default 40, pure AGL semantics), never to the AMSL-bearing
  // pattern field. Mixing the two is what made "50 AGL" show up as
  // "50 AMSL" in the waypoint list.
  const terrainTile = useTerrainStore((s) => s.tile)
  const surveyAglM = useMissionStore((s) => s.surveyAglM)
  const altitudeLabel =
    altitudeMode === 'terrain' ? t('plan.pattern.altitudeAgl') : t('plan.pattern.altitude')
  const altitudeValue =
    altitudeMode === 'terrain' ? surveyAglM : kind === 'sweep' ? sweep.altitude_amsl_m : clover.altitude_amsl_m
  const onAltitudeChange = (v: number) => {
    if (altitudeMode === 'terrain') setSurveyAglM(v)
    else if (kind === 'sweep') onSweep({ ...sweep, altitude_amsl_m: v })
    else onClover({ ...clover, altitude_amsl_m: v })
  }


  const onGenerate = async () => {
    setError(null)
    setBusy(true)
    try {
      // Terrain mode needs a DEM to mean anything: the altitude input is an
      // AGL, and without terrain there is nothing to drape against — the
      // waypoints would silently carry AMSL-looking numbers (the reported
      // "50 AGL shows as 50 AMSL" bug). Require the tile instead.
      if (altitudeMode === 'terrain' && !terrainTile) {
        setError(t('plan.pattern.terrainNeedsDem'))
        setBusy(false)
        return
      }
      // The AGL the auto re-drape applies (the store subscription runs after
      // insertPattern). The generator writes home+AGL as a placeholder AMSL;
      // the drape rewrites every waypoint right after.
      const aglM = surveyAglM
      const placeholder = (useMissionStore.getState().home?.[2] ?? 0) + aglM
      if (kind === 'sweep') {
        let boundary: GeoPoint[]
        if (usePolygonBoundary) boundary = polygonVertices
        else if (effectiveCenter) boundary = rectanglePolygon(effectiveCenter, size.width, size.height)
        else return
        const plan = await generateSweep({
          ...sweep,
          altitude_amsl_m: altitudeMode === 'terrain' ? placeholder : sweep.altitude_amsl_m,
          polygon: boundary,
          tie_spacing_m: sweep.tie_spacing_m && sweep.tie_spacing_m > 0 ? sweep.tie_spacing_m : null,
          speed_mps: sweep.speed_mps && sweep.speed_mps > 0 ? sweep.speed_mps : null,
        })
        const compiled = await compilePattern(plan, {
          mag_trigger: { kind: 'gcs_side' },
          cruise_speed_mps: sweep.speed_mps && sweep.speed_mps > 0 ? sweep.speed_mps : null,
          coverage_polygon: boundary,
        })
        insertPattern(compiled, t('plan.pattern.kind.sweep'), effectiveCenter, compiled.stats)
      } else {
        if (!effectiveCenter) {
          setError(t('plan.pattern.noCenter'))
          return
        }
        const plan = await generateCloverleaf({
          ...clover,
          altitude_amsl_m: altitudeMode === 'terrain' ? placeholder : clover.altitude_amsl_m,
          center: effectiveCenter,
          speed_mps: clover.speed_mps && clover.speed_mps > 0 ? clover.speed_mps : null,
        })
        const compiled = await compilePattern(plan, {
          mag_trigger: { kind: 'gcs_side' },
          cruise_speed_mps: clover.speed_mps && clover.speed_mps > 0 ? clover.speed_mps : null,
          coverage_polygon: null,
        })
        insertPattern(compiled, t('plan.pattern.kind.cloverleaf'), effectiveCenter, compiled.stats)
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
  const stats = lastPattern?.stats ?? null

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
            <Num label={altitudeLabel} value={altitudeValue} onChange={onAltitudeChange} />
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
          {/* Cloverleaf centre: the calibration centre is not the rectangle
           * fallback's — the operator must be able to position it directly
           * (operator report 2026-10-11). Shared with the sweep's rectangle
           * via the store's patternCenter. */}
          <div className="grid grid-cols-2 items-end gap-1.5">
            <Num
              label={t('plan.pattern.lat')}
              value={effectiveCenter?.latitude_deg ?? 0}
              onChange={(v) =>
                setPatternCenter({ latitude_deg: v, longitude_deg: effectiveCenter?.longitude_deg ?? 0 })
              }
            />
            <Num
              label={t('plan.pattern.lon')}
              value={effectiveCenter?.longitude_deg ?? 0}
              onChange={(v) =>
                setPatternCenter({ latitude_deg: effectiveCenter?.latitude_deg ?? 0, longitude_deg: v })
              }
            />
            <button
              onClick={() => {
                if (mapCenter) setPatternCenter({ latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon })
              }}
              className="col-span-2 rounded border border-line bg-canvas px-2 py-1 text-[11px] text-ink hover:bg-panel"
            >
              {t('plan.pattern.useMapCentre')}
            </button>
          </div>
          <div className="grid grid-cols-2 gap-1.5">
            <Num label={altitudeLabel} value={altitudeValue} onChange={onAltitudeChange} />
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
      {stats && (
        <div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-muted">
          <span>{t('plan.pattern.stats.total')} {stats.total_length_m.toFixed(0)} m</span>
          <span>
            {t('plan.pattern.stats.eta')}{' '}
            {stats.estimated_time_s >= 90
              ? `${Math.round(stats.estimated_time_s / 60)} min`
              : `${Math.round(stats.estimated_time_s)} s`}
          </span>
          {stats.coverage_m2 != null && (
            <span>{t('plan.pattern.stats.coverage')} {(stats.coverage_m2 / 10_000).toFixed(2)} ha</span>
          )}
        </div>
      )}
      <DrapeBlock />
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


/** S3/D5: terrain draping over the auto-picked dem-prep tile (N1). In
 * `terrain` altitude mode the re-drape runs automatically on every plan
 * change (the mission store subscription) — this block only reports. Other
 * modes keep the one-shot button. */
function DrapeBlock() {
  const { t } = useTranslation()
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const drape = useMissionStore((s) => s.drape)
  const drapeToTerrain = useMissionStore((s) => s.drapeToTerrain)
  const ensureTile = useTerrainStore((s) => s.ensureTile)
  const tile = useTerrainStore((s) => s.tile)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const auto = altitudeMode === 'terrain'

  const run = async () => {
    setBusy(true)
    setError(null)
    // N1: the terrain store auto-picks the tile covering the survey area;
    // it only opens a folder dialog when nothing was loaded yet.
    const path = await ensureTile()
    if (!path) {
      setBusy(false)
      setError(t('terrain.uncovered'))
      return
    }
    const err = await drapeToTerrain(path, 40, 5)
    setBusy(false)
    if (err) setError(err)
  }

  return (
    <div className="space-y-1">
      {auto ? (
        <div className="text-[11px] text-muted">{t('plan.pattern.drape.auto')}</div>
      ) : (
        <button
          onClick={() => void run()}
          disabled={busy}
          className="w-full rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
        >
          {busy ? t('plan.pattern.drape.busy') : t('plan.pattern.drape.run')}
        </button>
      )}
      {error && <div className="text-[11px] text-error">{error}</div>}
      {auto && !tile && (
        <div className="text-[11px] text-warn">{t('plan.pattern.drape.noDem')}</div>
      )}
{drape && (
        <div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-muted">
          <span>
            {tile?.source === 'geotiff'
              ? t('plan.pattern.drape.sourceGeotiff')
              : t('plan.pattern.drape.sourceLidar')}
          </span>
          <span>
            {t('plan.pattern.drape.clearance')}{' '}
            {drape.report.min_dsm_clearance_m.toFixed(1)} m
          </span>
          <span>
            {t('plan.pattern.drape.raised')} {drape.report.raised}
          </span>
          <span>
            {t('plan.pattern.drape.version')} {drape.report.dem_version}
          </span>
        </div>
      )}
    </div>
  )
}
