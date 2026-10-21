// Live altitude profile chart (operator request 2026-10-17, restyled to the
// Signal Inspector's toolbar/plot language 2026-10-21): actual AMSL from
// telemetry, the planned (drape) curve interpolated from the solver preview,
// DTM ground and DSM surface sampled under each recorded fix from the loaded
// dem-prep tile, and the rangefinder/lidar range from `DISTANCE_SENSOR` (R1).
// X = along-track distance in sync with the map's trail (both derive from the
// same telemetry snapshots); the window auto-follows the head past
// [`FOLLOW_WINDOW_M`]. Rendered in the map's bottom drawer (below the map).

import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import uPlot from 'uplot'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../stores/telemetry'
import { useTerrainStore } from '../stores/terrain'
import { useMissionStore } from '../stores/mission'
import { cssVar } from '../design-system/theme'
import { isTauri } from '../inspector/mock'

/** Distance decimation, matching the map trail's step. */
const MIN_STEP_M = 2
/** Redraw cadence. */
const REDRAW_MS = 1_000
/** DEM sampling cadence for fixes added since the last batch. */
const SAMPLE_MS = 1_500
/** A `DISTANCE_SENSOR` reading older than this is stale. */
const LIDAR_FRESH_MS = 3_000
/** Follow window: past this track length the x axis trails the head. */
const FOLLOW_WINDOW_M = 2_000
/** uPlot throws on ragged series: every column must match the x length. */
const PLOT_HEIGHT = 180

interface Fix {
  lat: number
  lon: number
  amsl: number
  lidar: number | null
}

/** ~`target` nice steps (1/2/5×10^k) across [min, max] — finer y ticks. */
function niceSplits(min: number, max: number, target: number): number[] {
  const span = max - min
  if (!(span > 0) || !Number.isFinite(span)) return [min, max]
  const raw = span / target
  const mag = Math.pow(10, Math.floor(Math.log10(raw)))
  const norm = raw / mag
  const step = (norm >= 5 ? 5 : norm >= 2 ? 2 : 1) * mag
  const out: number[] = []
  for (let v = Math.ceil(min / step) * step; v <= max + 1e-9; v += step) out.push(Number(v.toFixed(6)))
  return out
}

function haversineM(
  a: { lat: number; lon: number },
  b: { lat: number; lon: number },
): number {
  const R = 6_371_000
  const dLat = ((b.lat - a.lat) * Math.PI) / 180
  const dLon = ((b.lon - a.lon) * Math.PI) / 180
  const s =
    Math.sin(dLat / 2) ** 2 +
    Math.cos((a.lat * Math.PI) / 180) * Math.cos((b.lat * Math.PI) / 180) * Math.sin(dLon / 2) ** 2
  return 2 * R * Math.asin(Math.sqrt(s))
}

export default function LiveAltitudeProfile() {
  const { t } = useTranslation()
  const tauri = isTauri()
  const hostRef = useRef<HTMLDivElement | null>(null)
  const plotRef = useRef<uPlot | null>(null)
  const fixesRef = useRef<Fix[]>([])
  const dtmRef = useRef<(number | null)[]>([])
  const dsmRef = useRef<(number | null)[]>([])
  const plannedRef = useRef<(number | null)[]>([])
  const plannedSpecRef = useRef<string>('')
  const hostCleanup = useRef<(() => void) | null>(null)
  const sampledToRef = useRef(0)
  const lastFixAtRef = useRef(0)
  /** The live follow window. `scales.x.range` is a closure created ONCE with
   * the plot — capturing xMin/xMax directly would freeze the visible window
   * at the creation-time extent (the curve kept drawing past the right edge:
   * data updated, the picture did not scroll). */
  const xRangeRef = useRef<[number, number]>([0, 50])
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const demFolder = useTerrainStore((s) => s.tile?.path ?? null)
  const tile = useTerrainStore((s) => s.tile)
  const drapeSpecHash = useMissionStore((s) => s.drapePreview?.result.spec_hash ?? null)
  const drapePreview = useMissionStore((s) => s.drapePreview)
  const [demError, setDemError] = useState<string | null>(null)
  const [lidarSeen, setLidarSeen] = useState(false)
  const autoLoaded = useRef(false)

  // Auto-load the DEM once when the drawer first needs it. `ensureTile`
  // resolves it without a dialog when one is available: a remembered folder,
  // the prepared survey-area HRDEM session, or a DEM beside the app — the
  // folder picker is the last resort (H6: prefer the best source).
  useEffect(() => {
    if (!tauri || demFolder || autoLoaded.current) return
    autoLoaded.current = true
    void useTerrainStore.getState().ensureTile()
  }, [tauri, demFolder])

  // When a tile loads (auto or manually), reset the ground/surface buffers
  // so the fixes re-sample against the new DEM — never mix datums.
  const previousFolder = useRef<string | null>(null)
  useEffect(() => {
    if (previousFolder.current !== null && previousFolder.current !== demFolder) {
      dtmRef.current = []
      dsmRef.current = []
      sampledToRef.current = 0
    }
    previousFolder.current = demFolder
  }, [demFolder])

  // Record each new telemetry snapshot (distance-decimated), attaching a
  // fresh lidar reading to the head. Same event stream the map trail uses.
  // The AMSL is anchored into the DEM frame (`vehicleAmslAnchoredM`): the
  // reported `alt` carries PX4's EKF-origin error (~15 m low on a BC site),
  // which would draw the flown line below the DTM it actually cleared.
  useEffect(() => {
    if (!snapshot) return
    const pos = snapshot.global_position
    if (!pos) return
    const anchor = useTerrainStore.getState().amslAnchorOffsetM
    const homeGround = useTerrainStore.getState().homeGroundAmsl
    const fix = {
      lat: pos.latitude_deg,
      lon: pos.longitude_deg,
      amsl:
        anchor != null && Number.isFinite(anchor)
          ? pos.altitude.meters + anchor
          : homeGround != null && Number.isFinite(pos.relative_alt_m)
            ? homeGround + pos.relative_alt_m
            : pos.altitude.meters,
      lidar: null as number | null,
    }
    const ageMs =
      snapshot.field_ages.distance_sensor_at_ms != null
        ? Date.now() - snapshot.field_ages.distance_sensor_at_ms
        : null
    if (snapshot.distance_sensor != null && ageMs != null && ageMs <= LIDAR_FRESH_MS) {
      fix.lidar = snapshot.distance_sensor.current_m
      setLidarSeen(true)
    }
    const last = fixesRef.current[fixesRef.current.length - 1]
    if (last && haversineM(last, fix) < MIN_STEP_M) {
      last.lidar = fix.lidar ?? last.lidar
      return
    }
    fixesRef.current.push(fix)
    lastFixAtRef.current = Date.now()
  }, [snapshot])

  // DEM sampling: batch `terrain_sample` over the fixes since the last pass.
  useEffect(() => {
    if (!tauri || !demFolder) return
    const timer = setInterval(() => {
      const fixes = fixesRef.current
      const from = sampledToRef.current
      if (from >= fixes.length) return
      const batch = fixes.slice(from)
      void invoke<Array<{ dtm_m: number | null; dsm_m: number | null }>>('terrain_sample', {
        folder: demFolder,
        points: batch.map((f) => [f.lat, f.lon]),
      })
        .then((rows) => {
          for (const row of rows) {
            dtmRef.current.push(row.dtm_m)
            dsmRef.current.push(row.dsm_m)
          }
          // `from + rows.length`, NOT `fixes.length`: fixes that arrived while
          // the IPC was in flight belong to the NEXT batch — sampling them
          // here would desync the dtm/dsm columns from the fix stream.
          sampledToRef.current = from + rows.length
          setDemError(null)
        })
        .catch((e) => setDemError(String(e)))
    }, SAMPLE_MS)
    return () => clearInterval(timer)
  }, [tauri, demFolder])

  // The planned (drape) curve: nearest-sample `z_plan` from the solver
  // preview, per fix, cached per preview (a new solve re-interpolates). The
  // heights are DEM-datum like the DTM — the anchored AMSL is the same frame.
  const plannedSamples = useMemo(() => {
    if (!drapePreview) return null
    return drapePreview.result.lines.flatMap((l) => l.profile)
  }, [drapePreview])

  const interpolatePlanned = (fixes: Fix[]): (number | null)[] => {
    const out = plannedRef.current.slice(0, fixes.length)
    while (out.length < fixes.length) out.push(null)
    if (!plannedSamples || plannedSamples.length === 0) return out
    for (let i = plannedRef.current.length; i < fixes.length; i++) {
      const f = fixes[i]
      let best: number | null = null
      let bestD2 = Number.POSITIVE_INFINITY
      for (const sm of plannedSamples) {
        const d2 = (sm.y - f.lat) ** 2 + (sm.x - f.lon) ** 2
        if (d2 < bestD2) {
          bestD2 = d2
          best = sm.z_plan
        }
      }
      // A fix far from every planned line (ferry, manual detour) is a gap.
      // ~5 m in degrees·lat at 48°N; anything beyond reads as unplanned.
      out[i] = bestD2 <= (6 / 111_320) ** 2 ? (best as number) : null
    }
    plannedRef.current = out
    return out
  }

  // Chart redraw at a fixed cadence.
  useEffect(() => {
    if (!tauri) return
    const timer = setInterval(() => {
      const host = hostRef.current
      if (!host) return
      const fixes = fixesRef.current
      if (fixes.length < 2) {
        host.textContent = t('drawer.profile.noTrack')
        plotRef.current?.destroy()
        plotRef.current = null
        return
      }
      const dist: number[] = [0]
      for (let i = 1; i < fixes.length; i++) {
        dist.push(dist[i - 1] + haversineM(fixes[i - 1], fixes[i]))
      }
      const total = dist[dist.length - 1]
      // uPlot throws on ragged series: every column must be exactly as long
      // as x. The DEM sampler lags the fix stream (batched every 1.5 s), so
      // pad the trailing columns with null — spanGaps bridges them.
      const padTo = (arr: (number | null)[], n: number): (number | null)[] => {
        const out = arr.slice(0, n)
        while (out.length < n) out.push(null)
        return out
      }
      const n = fixes.length
      const planned =
        plannedSpecRef.current === drapeSpecHash
          ? interpolatePlanned(fixes)
          : (plannedRef.current = new Array(n).fill(null))
      plannedSpecRef.current = drapeSpecHash ?? ''
      const data = [
        dist,
        fixes.map((f) => f.amsl),
        padTo(planned, n),
        padTo(dtmRef.current, n),
        padTo(dsmRef.current, n),
        padTo(fixes.map((f) => f.lidar), n),
      ] as unknown as uPlot.AlignedData
      const w = host.clientWidth || 640
      const xMin = total > FOLLOW_WINDOW_M ? total - FOLLOW_WINDOW_M : 0
      const xMax = Math.max(total, total * 1.02, 50)
      xRangeRef.current = [xMin, xMax]
      try {
        if (!plotRef.current) {
          host.textContent = ''
          const stroke = (c: string) => cssVar(c)
          const opts: uPlot.Options = {
            width: w,
            height: PLOT_HEIGHT,
            legend: { show: true, live: false },
            scales: { x: { time: false, range: () => xRangeRef.current } },
            // Drag a box to zoom the y axis (x stays on the follow window);
            // double-click resets. Wheel zooms y around the cursor (below).
            cursor: { drag: { x: false, y: true, uni: 20 } },
            axes: [
              { stroke: stroke('--mg-muted'), label: 'm', grid: { stroke: stroke('--mg-border') } },
              {
                stroke: stroke('--mg-muted'),
                label: 'm AMSL',
                grid: { stroke: stroke('--mg-border') },
                // Finer y ticks: ~10 nice steps instead of uPlot's sparse
                // default (operator request).
                splits: (_self, _axisIdx, min, max) =>
                  niceSplits(min as number, max as number, 10),
              },
            ],
            series: [
              {},
              { label: t('drawer.profile.actual'), stroke: stroke('--mg-accent'), width: 2 },
              { label: t('drawer.profile.planned'), stroke: stroke('--mg-muted'), width: 1.5, dash: [6, 3], spanGaps: true },
              { label: t('drawer.profile.dtm'), stroke: stroke('--mg-ok'), width: 1.5, spanGaps: true },
              { label: t('drawer.profile.dsm'), stroke: stroke('--mg-warn'), width: 1.5, spanGaps: true },
              {
                label: t('drawer.profile.lidar'),
                stroke: stroke('--mg-mag'),
                width: 1,
                dash: [4, 3],
                spanGaps: true,
              },
            ],
          }
          plotRef.current = new uPlot(opts, data, host)
          if (import.meta.env?.DEV) {
            ;(window as unknown as { __mgProfile?: unknown }).__mgProfile = plotRef
          }
          // Wheel: zoom the y axis around the cursor (1.15× per notch).
          const onWheel = (e: WheelEvent) => {
            const plot = plotRef.current
            if (!plot) return
            e.preventDefault()
            const rect = host.getBoundingClientRect()
            const frac = 1 - (e.clientY - rect.top) / rect.height
            const { min, max } = plot.scales.y
            if (min == null || max == null || !Number.isFinite(min) || !Number.isFinite(max)) return
            const span = Math.max(1, (max - min) * (e.deltaY > 0 ? 1.15 : 1 / 1.15))
            const anchor = min + frac * (max - min)
            const lo = anchor - frac * span
            const hi = anchor + (1 - frac) * span
            plot.setScale('y', { min: lo, max: hi })
          }
          const onDblClick = () => {
            // Nulls restore the scale to auto (data-driven) — the reset.
            plotRef.current?.setScale('y', { min: null as unknown as number, max: null as unknown as number })
          }
          host.addEventListener('wheel', onWheel, { passive: false })
          host.addEventListener('dblclick', onDblClick)
          hostCleanup.current = () => {
            host.removeEventListener('wheel', onWheel)
            host.removeEventListener('dblclick', onDblClick)
          }
        } else {
          plotRef.current.setData(data)
        }
      } catch (e) {
        // A draw failure must not kill the interval: the next tick retries
        // with fresh (padded) columns.
        console.warn('altitude profile draw failed', e)
      }
    }, REDRAW_MS)
    return () => clearInterval(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri, t])

  useEffect(() => {
    const onResize = () => {
      const host = hostRef.current
      if (host && plotRef.current) plotRef.current.setSize({ width: host.clientWidth || 640, height: PLOT_HEIGHT })
    }
    window.addEventListener('resize', onResize)
    return () => {
      window.removeEventListener('resize', onResize)
      hostCleanup.current?.()
      plotRef.current?.destroy()
      plotRef.current = null
    }
  }, [])

  const [, forceTick] = useState(0)
  useEffect(() => {
    const id = setInterval(() => forceTick((n) => n + 1), 1_000)
    return () => clearInterval(id)
  }, [])

  if (!tauri) return null

  return (
    <div className="flex flex-col gap-1">
      {/* Inspector-style toolbar: status chips left, facts right. */}
      <div className="flex flex-wrap items-center gap-2 text-[11px]">
        <span className={`mono ${lidarSeen ? 'text-ok' : 'text-muted'}`}>
          {lidarSeen ? t('drawer.profile.lidarUp') : t('drawer.profile.lidarDown')}
        </span>
        {tile && (
          <span className="mono truncate text-muted" data-testid="profile-source-chip" title={demFolder ?? ''}>
            {tile.source === 'hrdem'
              ? t('terrain.sourceHrdem')
              : tile.source === 'geotiff'
                ? t('terrain.sourceGeotiff')
                : t('terrain.sourceLidar')}
            {tile.resolution_m != null ? ` · ${tile.resolution_m} m` : ''}
          </span>
        )}
        {!demFolder && <span className="text-warn">{t('drawer.profile.noDemHint')}</span>}
        {demError && <span className="mono truncate text-error">{demError}</span>}
        <span className="mono ml-auto text-muted" data-testid="profile-fixes" title={t('drawer.profile.fixesHint')}>
          {t('drawer.profile.fixes', { count: fixesRef.current.length, age: Math.round((Date.now() - lastFixAtRef.current) / 1000) })}
        </span>
        <span className="text-muted">{t('drawer.profile.title')}</span>
      </div>
      <div ref={hostRef} className="min-h-[180px] w-full" />
    </div>
  )
}
