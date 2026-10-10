// Step 1 — Area (P2 §4.2): how the survey boundary is defined. The polygon is
// drawn with the map tool (its readout moved here from the map overlay), the
// rectangle is sized around a centre, and the centre can be typed by hand.
// `Clear area` is the one clear for this scope.

import { useTranslation } from 'react-i18next'
import { rectanglePolygon } from '../../../mission/patterns'
import { polygonArea, polygonPerimeter, selfIntersects } from '../../../mission/polygon'
import { useMissionStore } from '../../../stores/mission'
import { usePolygonStore } from '../../../stores/polygon'
import { useUiStore } from '../../../stores/ui'
import type { GeoPoint } from '../../../generated-types/GeoPoint'

export interface AreaStepProps {
  /** Rectangle size (metres), shared with PatternStep's Generate. */
  size: { width: number; height: number }
  onSize: (size: { width: number; height: number }) => void
}

export default function AreaStep({ size, onSize }: AreaStepProps) {
  const { t } = useTranslation()
  const vertices = usePolygonStore((s) => s.vertices)
  const closed = usePolygonStore((s) => s.closed)
  const reset = usePolygonStore((s) => s.reset)
  const mapTool = useUiStore((s) => s.mapTool)
  const setMapTool = useUiStore((s) => s.setMapTool)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const center = useMissionStore((s) => s.patternCenter)
  const setPatternCenter = useMissionStore((s) => s.setPatternCenter)

  const bad = closed && selfIntersects(vertices)
  const areaHectares = polygonArea(vertices) / 10000
  const perimeter = polygonPerimeter(vertices)
  const effectiveCenter: GeoPoint | null =
    center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const rect = effectiveCenter ? rectanglePolygon(effectiveCenter, size.width, size.height) : []

  return (
    <div className="space-y-2">
      {/* Boundary source: drawn polygon or rectangle around the centre. */}
      <div className="flex rounded-md border border-line bg-canvas p-0.5 text-xs">
        <button
          onClick={() => setMapTool(mapTool === 'polygon' ? 'select' : 'polygon')}
          className={`flex-1 rounded px-1 py-1 transition-colors ${
            mapTool === 'polygon' ? 'bg-accent text-canvas' : 'text-muted hover:text-ink'
          }`}
        >
          {t('plan.area.drawPolygon')}
        </button>
      </div>
      {mapTool === 'polygon' && !closed && (
        <div className="text-[11px] text-muted">{t('map.polygonHint')}</div>
      )}

      {/* Polygon readout (moved from the map overlay, P2). */}
      {vertices.length > 0 ? (
        <div
          className={`rounded border px-2 py-1.5 text-xs ${
            bad ? 'border-error/60 bg-error/10 text-error' : 'border-line bg-canvas text-ink'
          }`}
        >
          <div className="mono">
            {t('map.polygonReadout', {
              count: vertices.length,
              area: areaHectares.toFixed(1),
              perimeter: Math.round(perimeter),
            })}
          </div>
          {closed && !bad && <div>{t('map.polygonClosed')}</div>}
          {bad && <div className="font-medium">{t('map.polygonSelfIntersect')}</div>}
          <button
            onClick={reset}
            className="mt-1 rounded border border-line bg-panel px-2 py-0.5 text-[11px] text-ink hover:bg-canvas"
          >
            {t('plan.area.clear')}
          </button>
        </div>
      ) : (
        <div className="text-[11px] text-muted">{t('plan.pattern.polygonHint')}</div>
      )}

      {/* Manual centre + rectangle fallback (sweep boundary when no polygon). */}
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
        <Num
          label={t('plan.pattern.width')}
          value={size.width}
          onChange={(v) => onSize({ ...size, width: v })}
        />
        <Num
          label={t('plan.pattern.height')}
          value={size.height}
          onChange={(v) => onSize({ ...size, height: v })}
        />
      </div>
      {rect.length === 4 && (
        <div className="mono text-[11px] text-muted">
          {t('map.polygonReadout', {
            count: 4,
            area: ((size.width * size.height) / 10000).toFixed(1),
            perimeter: Math.round(2 * (size.width + size.height)),
          })}
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
