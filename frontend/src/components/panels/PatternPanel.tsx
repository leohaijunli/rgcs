// Preset parameterised patterns: build the parameters, call `core::survey`
// through the Tauri commands, and append the generated waypoints to the plan.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  defaultCloverleaf,
  defaultSweep,
  generateCloverleaf,
  generateSweep,
  rectanglePolygon,
} from '../../mission/patterns'
import { selfIntersects } from '../../mission/polygon'
import { useMissionStore } from '../../stores/mission'
import { usePolygonStore } from '../../stores/polygon'
import { useUiStore } from '../../stores/ui'
import type { CloverleafPattern } from '../../generated-types/CloverleafPattern'
import type { GeoPoint } from '../../generated-types/GeoPoint'
import type { SurveyPattern } from '../../generated-types/SurveyPattern'

type Kind = 'sweep' | 'cloverleaf'

/** Default clearance above home for a generated pattern. */
const DEFAULT_ALT_AGL_M = 50

const ORIGIN = { latitude_deg: 0, longitude_deg: 0 }

export default function PatternPanel() {
  const { t } = useTranslation()
  const mapCenter = useUiStore((s) => s.mapCenter)
  const home = useMissionStore((s) => s.home)
  const insertPattern = useMissionStore((s) => s.insertPattern)
  const lastPattern = useMissionStore((s) => s.lastPattern)

  const homeAmsl = home?.[2] ?? 0
  const [kind, setKind] = useState<Kind>('sweep')
  const [sweep, setSweep] = useState<SurveyPattern>(() =>
    defaultSweep(ORIGIN, homeAmsl + DEFAULT_ALT_AGL_M),
  )
  const [clover, setClover] = useState<CloverleafPattern>(() =>
    defaultCloverleaf(ORIGIN, homeAmsl + DEFAULT_ALT_AGL_M),
  )
  const [size, setSize] = useState({ width: 500, height: 500 })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // A pinned centre (lat/lon inputs, "Use map centre", or the map drag handle),
  // so a generated pattern stays where it was even if the map moves.
  const center = useMissionStore((s) => s.patternCenter)
  const setPatternCenter = useMissionStore((s) => s.setPatternCenter)
  const effectiveCenter: GeoPoint | null =
    center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const polygonBad = polygonClosed && selfIntersects(polygonVertices)
  // A closed, valid polygon is the sweep boundary; otherwise fall back to the
  // rectangle quick mode around the map centre.
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
        const params: SurveyPattern = {
          ...sweep,
          polygon: boundary,
          tie_spacing_m: sweep.tie_spacing_m && sweep.tie_spacing_m > 0 ? sweep.tie_spacing_m : null,
          speed_mps: sweep.speed_mps && sweep.speed_mps > 0 ? sweep.speed_mps : null,
        }
        const plan = await generateSweep(params)
        insertPattern(plan, t('plan.pattern.kind.sweep'), effectiveCenter)
      } else {
        if (!effectiveCenter) return
        const params: CloverleafPattern = {
          ...clover,
          center: effectiveCenter,
          speed_mps: clover.speed_mps && clover.speed_mps > 0 ? clover.speed_mps : null,
        }
        const plan = await generateCloverleaf(params)
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
    <div className="panel rounded p-2">
      <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">
        {t('plan.pattern.title')}
      </div>
      <div className="mb-2 flex rounded-md border border-line bg-canvas p-0.5 text-xs">
        {(['sweep', 'cloverleaf'] as const).map((k) => (
          <button
            key={k}
            onClick={() => setKind(k)}
            className={`flex-1 rounded px-1 py-1 transition-colors ${
              kind === k ? 'bg-accent text-canvas' : 'text-muted hover:text-ink'
            }`}
          >
            {t(`plan.pattern.kind.${k}`)}
          </button>
        ))}
      </div>

      <div className="mb-1.5 grid grid-cols-2 items-end gap-1.5">
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

      {kind === 'sweep' ? (
        <div className="grid grid-cols-2 gap-1.5">
          <Num
            label={t('plan.pattern.width')}
            value={size.width}
            onChange={(v) => setSize({ ...size, width: v })}
          />
          <Num
            label={t('plan.pattern.height')}
            value={size.height}
            onChange={(v) => setSize({ ...size, height: v })}
          />
          <Num
            label={t('plan.pattern.lineAzimuth')}
            value={sweep.line_azimuth_deg}
            onChange={(v) => setSweep({ ...sweep, line_azimuth_deg: v })}
          />
          <Num
            label={t('plan.pattern.spacing')}
            value={sweep.line_spacing_m}
            onChange={(v) => setSweep({ ...sweep, line_spacing_m: v })}
          />
          <Num
            label={t('plan.pattern.tieSpacing')}
            value={sweep.tie_spacing_m ?? 0}
            onChange={(v) => setSweep({ ...sweep, tie_spacing_m: v })}
          />
          <Num
            label={t('plan.pattern.tieAzimuth')}
            value={sweep.tie_azimuth_deg}
            onChange={(v) => setSweep({ ...sweep, tie_azimuth_deg: v })}
          />
          <Num
            label={t('plan.pattern.leadIn')}
            value={sweep.lead_in_m}
            onChange={(v) => setSweep({ ...sweep, lead_in_m: v })}
          />
          <Num
            label={t('plan.pattern.leadOut')}
            value={sweep.lead_out_m}
            onChange={(v) => setSweep({ ...sweep, lead_out_m: v })}
          />
          <Num
            label={t('plan.pattern.altitude')}
            value={sweep.altitude_amsl_m}
            onChange={(v) => setSweep({ ...sweep, altitude_amsl_m: v })}
          />
          <Num
            label={t('plan.pattern.speed')}
            value={sweep.speed_mps ?? 0}
            onChange={(v) => setSweep({ ...sweep, speed_mps: v })}
          />
          <label className="col-span-2 flex items-center gap-2 text-xs text-muted">
            <input
              type="checkbox"
              checked={sweep.alternate}
              onChange={(e) => setSweep({ ...sweep, alternate: e.target.checked })}
            />
            {t('plan.pattern.alternate')}
          </label>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-1.5">
          <Num
            label={t('plan.pattern.radius')}
            value={clover.radius_m}
            onChange={(v) => setClover({ ...clover, radius_m: v })}
          />
          <Num
            label={t('plan.pattern.petals')}
            step="2"
            value={clover.petals}
            onChange={(v) => setClover({ ...clover, petals: Math.round(v) })}
          />
          <Num
            label={t('plan.pattern.samples')}
            step="2"
            value={clover.samples_per_petal}
            onChange={(v) => setClover({ ...clover, samples_per_petal: Math.max(1, Math.round(v)) })}
          />
          <Num
            label={t('plan.pattern.heading')}
            value={clover.start_heading_deg}
            onChange={(v) => setClover({ ...clover, start_heading_deg: v })}
          />
          <Num
            label={t('plan.pattern.altitude')}
            value={clover.altitude_amsl_m}
            onChange={(v) => setClover({ ...clover, altitude_amsl_m: v })}
          />
          <Num
            label={t('plan.pattern.speed')}
            value={clover.speed_mps ?? 0}
            onChange={(v) => setClover({ ...clover, speed_mps: v })}
          />
        </div>
      )}

      {kind === 'sweep' && (
        <div className="mt-1 mb-1 flex flex-col gap-0.5 text-[11px]">
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
      )}

      <button
        disabled={busy || (!effectiveCenter && !usePolygonBoundary) || polygonBad}
        onClick={() => void onGenerate()}
        title={lastPattern ? t('plan.pattern.replaces') : undefined}
        className="mt-2 w-full rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
      >
        {lastPattern ? t('plan.pattern.regenerate') : t('plan.pattern.generate')}
      </button>

      {!effectiveCenter && !usePolygonBoundary && (
        <div className="mt-1 text-[11px] text-warn">{t('plan.pattern.polygonHint')}</div>
      )}
      {lastPattern && (
        <div className="mt-1 text-[11px] text-muted">{t('plan.pattern.replaces')}</div>
      )}
      {error && <div className="mt-1 text-[11px] text-error">{t('plan.pattern.failed', { message: error })}</div>}
      {lineCounts && (
        <div className="mt-1 flex flex-wrap gap-x-2 gap-y-0.5 text-[11px]">
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
