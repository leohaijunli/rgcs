// Step 2 — Pattern (P2 §4.2): Sweep / Cloverleaf parameters with a
// Common / Advanced split (P6: ~20 fields at once → 4 + folded rest), and the
// Generate / Regenerate action. The boundary comes from the Area step.

import { useState } from 'react'
import { getPlanPrefs } from '../../../desktop/appConfig'
import type { PlanPrefs } from '../../../generated-types/PlanPrefs'
import type { PlanPreset } from '../../../generated-types/PlanPreset'
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
import DrapePreview from './DrapePreview'
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
  onSize: (size: AreaStepProps['size']) => void
  /** Survey-area azimuth (U8), from the Area step. */
  heading: number
  onHeading: (heading: number) => void
}

export default function PatternStep({
  kind,
  onKind,
  sweep,
  onSweep,
  clover,
  onClover,
  size,
  onSize,
  heading,
  onHeading,
}: PatternStepProps) {
  const { t } = useTranslation()
  const [advanced, setAdvanced] = useState(false)
  const [presetName, setPresetName] = useState('')
  const [prefs, setPrefs] = useState<PlanPrefs>(() => getPlanPrefs())
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const setPlanStep = useUiStore((s) => s.setPlanStep)
  const requestFit = useUiStore((s) => s.requestFit)
  const insertPattern = useMissionStore((s) => s.insertPattern)
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const center = useMissionStore((s) => s.patternCenter)
  const setPatternCenter = useMissionStore((s) => s.setPatternCenter)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const setSurveyAglM = useMissionStore((s) => s.setSurveyAglM)
  const setDrapeSpec = useMissionStore((s) => s.setDrapeSpec)
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
  const meshSampler = useTerrainStore((s) => s.meshSampler)
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
      // Terrain mode needs ground to mean anything: the altitude input is an
      // AGL, and without terrain there is nothing to drape against — the
      // waypoints would silently carry AMSL-looking numbers (the reported
      // "50 AGL shows as 50 AMSL" bug). The 3D mesh counts as ground (the
      // drape measures against it), so require either.
      if (altitudeMode === 'terrain' && !terrainTile && !meshSampler) {
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
        else if (effectiveCenter) boundary = rectanglePolygon(effectiveCenter, size.width, size.height, heading)
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
        // Post-generate handoff (#plan review): show the result and frame it.
        setPlanStep('waypoints')
        requestFit()
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
        setPlanStep('waypoints')
        requestFit()
      }
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  /** U8: apply a saved preset (kind, AGL, speed, spacings, area size +
   * azimuth) to the current pattern state. */
  const applyPreset = (preset: PlanPreset) => {
    const homeAmsl = useMissionStore.getState().home?.[2] ?? 0
    onKind(preset.kind === 'cloverleaf' ? 'cloverleaf' : 'sweep')
    setSurveyAglM(preset.agl_m)
    onSweep({
      ...sweep,
      altitude_amsl_m: homeAmsl + preset.agl_m,
      line_spacing_m: preset.line_spacing_m,
      tie_spacing_m: preset.tie_spacing_m ?? null,
      speed_mps: preset.speed_mps > 0 ? preset.speed_mps : null,
    })
    onClover({ ...clover, altitude_amsl_m: homeAmsl + preset.agl_m })
    onSize({ width: preset.width_m, height: preset.height_m })
    onHeading(preset.heading_deg)
    // DR1: the drape parameters ride along with the preset.
    if (preset.min_clearance_dsm_m != null || preset.vertical_offset_m != null) {
      setDrapeSpec({
        ...(preset.min_clearance_dsm_m != null
          ? { minClearanceM: preset.min_clearance_dsm_m }
          : {}),
        ...(preset.vertical_offset_m != null
          ? { verticalOffsetM: preset.vertical_offset_m }
          : {}),
      })
    }
    setError(null)
  }

  /** U8: save the current pattern parameters as a named preset, persisted
   * in the app config (survives restarts). */
  const savePreset = (name: string) => {
    const trimmed = name.trim()
    if (!trimmed) return
    void import('../../../desktop/appConfig').then(({ getPlanPrefs, updatePlanPrefs }) => {
      const current = getPlanPrefs()
      const preset: PlanPreset = {
        name: trimmed,
        kind,
        agl_m: surveyAglM,
        speed_mps: kind === 'sweep' ? sweep.speed_mps ?? 0 : clover.speed_mps ?? 0,
        line_spacing_m: kind === 'sweep' ? sweep.line_spacing_m : 5,
        tie_spacing_m: kind === 'sweep' ? (sweep.tie_spacing_m ?? null) : null,
        width_m: size.width,
        height_m: size.height,
        heading_deg: heading,
        min_clearance_dsm_m: useMissionStore.getState().drapeSpec.minClearanceM,
        vertical_offset_m: useMissionStore.getState().drapeSpec.verticalOffsetM,
      }
      const others = current.presets.filter((p) => p.name !== trimmed)
      updatePlanPrefs({ ...current, presets: [...others, preset] })
      setPrefs(getPlanPrefs())
      setPresetName('')
    })
  }

  /** U8: set the line azimuth to the local geomagnetic declination
   * (embedded WMM2025) — survey lines parallel to the field, e.g. to keep
   * the vehicle's magnetic interference signature constant along a line. */
  const alignToField = async () => {
    const center = effectiveCenter ?? useMissionStore.getState().patternCenter
    if (!center) {
      setError(t('plan.pattern.noCenter'))
      return
    }
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const decl = await invoke<number>('geomag_declination', {
        lat: center.latitude_deg,
        lon: center.longitude_deg,
      })
      // Magnetic azimuth, normalized into [0, 360); tie lines follow at +90.
      onSweep({ ...sweep, line_azimuth_deg: ((decl % 360) + 360) % 360 })
      setError(null)
    } catch (e) {
      setError(String(e))
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
            <div>
              <Num label={t('plan.pattern.lineAzimuth')} value={sweep.line_azimuth_deg} onChange={(v) => onSweep({ ...sweep, line_azimuth_deg: v })} />
              {/* U8 (operator request): align the survey lines with the local
               * geomagnetic field — WMM2025 declination, one click. */}
              <button
                onClick={() => void alignToField()}
                title={t('plan.pattern.alignB')}
                className="mt-0.5 w-full rounded border border-line bg-canvas px-1 py-0.5 text-[10px] text-muted hover:text-ink"
              >
                ∥ B
              </button>
            </div>
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

      <PresetRow
        presets={prefs.presets}
        presetName={presetName}
        setPresetName={setPresetName}
        onLoad={applyPreset}
        onSave={() => savePreset(presetName)}
        t={t}
      />
      {kind === 'sweep' && !usePolygonBoundary && effectiveCenter && (
        <EstimateChip size={size} heading={heading} sweep={sweep} t={t} />
      )}
      <button
        disabled={busy || (!effectiveCenter && !usePolygonBoundary) || polygonBad}
        onClick={() => void onGenerate()}
        title={
          polygonBad
            ? t('plan.pattern.selfIntersects')
            : lastPattern
              ? t('plan.pattern.replaces')
              : undefined
        }
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
      {altitudeMode === 'terrain' && <DrapePreview />}
      {altitudeMode === 'terrain' && <DrapeParams />}
      <DrapeBlock />
    </div>
  )
}

/** Drape parameters (DR1, ADR-022): the required DSM clearance, the vertical
 * offset with its note, and the advanced fields folded away. The clearance
 * has NO default — an empty field refuses the drape with the reason. */
function DrapeParams() {
  const { t } = useTranslation()
  const spec = useMissionStore((s) => s.drapeSpec)
  const setDrapeSpec = useMissionStore((s) => s.setDrapeSpec)
  const missing = spec.minClearanceM == null || !Number.isFinite(spec.minClearanceM)
  return (
    <div className="space-y-1.5 rounded border border-line bg-canvas p-2" data-plan-field="drape-params">
      <div className="grid grid-cols-2 gap-1.5">
        <label className="flex flex-col gap-0.5 text-[10px]">
          <span className={missing ? 'truncate text-error' : 'truncate text-muted'}>
            {t('plan.pattern.drapeParams.minClearance')} *
          </span>
          <input
            type="number"
            step="0.5"
            min="0"
            value={spec.minClearanceM ?? ''}
            placeholder={t('plan.pattern.drapeParams.required')}
            data-plan-field="min-clearance"
            onChange={(e) =>
              setDrapeSpec({
                minClearanceM: e.target.value === '' ? null : parseFloat(e.target.value),
              })
            }
            className={`mono w-full rounded border bg-panel px-1.5 py-1 text-xs text-ink ${
              missing ? 'border-error' : 'border-line'
            }`}
          />
        </label>
        <Num
          label={t('plan.pattern.drapeParams.verticalOffset')}
          value={spec.verticalOffsetM}
          onChange={(v) =>
            setDrapeSpec({ verticalOffsetM: Number.isFinite(v) ? v : 0 })
          }
        />
      </div>
      <input
        value={spec.verticalOffsetNote}
        onChange={(e) => setDrapeSpec({ verticalOffsetNote: e.target.value })}
        placeholder={t('plan.pattern.drapeParams.offsetNote')}
        data-plan-field="offset-note"
        className="w-full rounded border border-line bg-panel px-1.5 py-1 text-xs text-ink"
      />
      <details className="text-[10px] text-muted">
        <summary className="cursor-pointer select-none">{t('plan.pattern.drapeParams.advanced')}</summary>
        <div className="mt-1 grid grid-cols-2 gap-1.5">
          <Num label={t('plan.pattern.drapeParams.posMargin')} value={spec.posMarginM} onChange={(v) => setDrapeSpec({ posMarginM: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.dsmFootprint')} value={spec.dsmFootprintM} onChange={(v) => setDrapeSpec({ dsmFootprintM: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.maxClimb')} value={spec.maxClimbMps} onChange={(v) => setDrapeSpec({ maxClimbMps: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.maxDescent')} value={spec.maxDescentMps} onChange={(v) => setDrapeSpec({ maxDescentMps: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.sampleStep')} value={spec.sampleStepM} onChange={(v) => setDrapeSpec({ sampleStepM: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.runin')} value={spec.runinM} onChange={(v) => setDrapeSpec({ runinM: Number.isFinite(v) ? v : 0 })} />
          <Num label={t('plan.pattern.drapeParams.wpTol')} value={spec.wpTolM} onChange={(v) => setDrapeSpec({ wpTolM: Number.isFinite(v) ? v : 0 })} />
        </div>
      </details>
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


/** U8: named parameter presets — load applies one; save stores the
 * current pattern parameters under the typed name (persisted in config). */
function PresetRow({
  presets,
  presetName,
  setPresetName,
  onLoad,
  onSave,
  t,
}: {
  presets: PlanPreset[]
  presetName: string
  setPresetName: (v: string) => void
  onLoad: (p: PlanPreset) => void
  onSave: () => void
  t: (k: string, o?: Record<string, string | number>) => string
}) {
  return (
    <div className="rounded border border-line bg-canvas p-1.5 text-xs">
      <div className="flex items-center gap-1">
        <select
          value=""
          onChange={(e) => {
            const p = presets.find((x) => x.name === e.target.value)
            if (p) onLoad(p)
          }}
          className="flex-1 rounded border border-line bg-panel px-1 py-1 text-xs text-ink"
          title={t('plan.pattern.presetLoad')}
        >
          <option value="">{t('plan.pattern.presetLoad')}</option>
          {presets.map((p) => (
            <option key={p.name} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
        <input
          value={presetName}
          onChange={(e) => setPresetName(e.target.value)}
          placeholder={t('plan.pattern.presetName')}
          className="flex-1 rounded border border-line bg-panel px-1 py-1 text-xs text-ink"
        />
        <button
          onClick={onSave}
          disabled={!presetName.trim()}
          className="rounded border border-line bg-canvas px-2 py-1 text-[11px] text-ink hover:bg-panel disabled:opacity-40"
        >
          {t('plan.pattern.presetSave')}
        </button>
      </div>
    </div>
  )
}


/**
 * U8 pre-generate estimate for the rectangle sweep: line count, total
 * length and a rough ETA, before Generate commits anything. */
function EstimateChip({
  size,
  heading,
  sweep,
  t,
}: {
  size: AreaStepProps['size']
  heading: number
  sweep: SurveyPattern
  t: (k: string, o?: Record<string, string | number>) => string
}) {
  if (sweep.line_spacing_m <= 0) return null
  const lines = Math.floor(size.width / sweep.line_spacing_m) + 1
  const run = size.height + (sweep.lead_in_m + sweep.lead_out_m)
  const total = lines * run + (lines - 1) * sweep.line_spacing_m
  const speed = sweep.speed_mps && sweep.speed_mps > 0 ? sweep.speed_mps : null
  const eta = speed ? `${Math.round(total / speed)} s` : '—'
  return (
    <div className="rounded border border-line bg-canvas px-2 py-1 text-[11px] text-muted">
      <span>
        ≈ {t('plan.pattern.estimate', { lines, km: (total / 1000).toFixed(2), eta })}
      </span>
      <span className="text-muted"> · </span>
      <span>{t('plan.pattern.areaAzimuth')} {heading}°</span>
    </div>
  )
}

/**
 * S3/D5 → A9: terrain draping runs the one solver pipeline (`drape_preview`,
 * solve → compile) — in `terrain` mode automatically on every plan change
 * (the mission store subscription), in other modes as a one-shot button.
 * This block only reports: the compiled route, the landing targets, and the
 * DEM the plan was draped against. */
function DrapeBlock() {
  const { t } = useTranslation()
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const drape = useMissionStore((s) => s.drape)
  const drapeError = useMissionStore((s) => s.drapeError)
  const drapeToTerrain = useMissionStore((s) => s.drapeToTerrain)
  const ensureTile = useTerrainStore((s) => s.ensureTile)
  const tile = useTerrainStore((s) => s.tile)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const auto = altitudeMode === 'terrain'

  const run = async () => {
    setBusy(true)
    setError(null)
    // DR1: the clearance is required before anything runs (no silent default).
    const spec = useMissionStore.getState().drapeSpec
    if (spec.minClearanceM == null || !Number.isFinite(spec.minClearanceM)) {
      setBusy(false)
      setError(t('plan.pattern.drapeParams.clearanceRequired'))
      return
    }
    // N1: the terrain store auto-picks the tile covering the survey area —
    // a remembered folder, the prepared survey HRDEM session, or a DEM beside
    // the app; it opens a dialog only when none of those exist.
    const path = await ensureTile()
    if (!path) {
      setBusy(false)
      setError(t('terrain.uncovered'))
      return
    }
    const err = await drapeToTerrain()
    setBusy(false)
    if (err) setError(err)
  }

  return (
    <div className="space-y-1" data-plan-field="drape">
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
      {auto && drapeError && (
        <div className="text-[11px] text-error">
          {t('plan.pattern.drape.failed', { reason: drapeError })}
        </div>
      )}
      {auto && !tile && (
        <div className="text-[11px] text-warn">{t('plan.pattern.drape.noDem')}</div>
      )}
      {drape && (
        <div className="flex flex-wrap gap-x-2 gap-y-0.5 text-[11px]">
          <span className="text-muted">
            {tile?.source === 'geotiff'
              ? t('plan.pattern.drape.sourceGeotiff')
              : t('plan.pattern.drape.sourceLidar')}
          </span>
          {/* The tightest planned clearance the solver produced; `null` on a
           * DTM-only source — the panel says so instead of a fake number. */}
          <span className="text-muted">
            {t('plan.pattern.drape.clearance')}{' '}
            {drape.minClearanceM != null
              ? `${drape.minClearanceM.toFixed(1)} m`
              : t('plan.pattern.drape.noDsm')}
          </span>
          <span>
            {t('plan.pattern.drape.items')} {drape.compiledCount}
          </span>
          {drape.pinned > 0 && (
            <span>
              {t('plan.pattern.drape.pinned')} {drape.pinned}
            </span>
          )}
          {/* Landing items are draped onto the ground, not 40 m above it —
           * the one height in the plan that must not carry the survey AGL. */}
          {drape.landed > 0 && (
            <span className="text-ink">
              {t('plan.pattern.drape.landed')} {drape.landed} (
              {t('plan.pattern.drape.landing')})
            </span>
          )}
          <span>
            {t('plan.pattern.drape.version')} {drape.demVersion}
          </span>
        </div>
      )}
    </div>
  )
}
