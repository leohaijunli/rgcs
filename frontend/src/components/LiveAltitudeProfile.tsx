// Live altitude profile chart (operator request 2026-10-17): actual AMSL
// from telemetry, DTM ground and DSM surface sampled under each recorded fix
// from the loaded dem-prep tile, and the rangefinder/lidar range from
// `DISTANCE_SENSOR` (R1). X = along-track distance in sync with the map's
// trail (both derive from the same telemetry snapshots); the window
// auto-follows the head past [`FOLLOW_WINDOW_M`]. Rendered in the map's
// bottom drawer (below the map), and reused by DR6's profile page.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import uPlot from 'uplot'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../stores/telemetry'
import { useTerrainStore } from '../stores/terrain'
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

interface Fix {
  lat: number
  lon: number
  amsl: number
  lidar: number | null
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
  const sampledToRef = useRef(0)
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const demFolder = useTerrainStore((s) => s.tile?.path ?? null)
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
          sampledToRef.current = fixes.length
          setDemError(null)
        })
        .catch((e) => setDemError(String(e)))
    }, SAMPLE_MS)
    return () => clearInterval(timer)
  }, [tauri, demFolder])

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
      const amsl = fixes.map((f) => f.amsl)
      const lidar = fixes.map((f) => f.lidar)
      const data = [dist, amsl, dtmRef.current, dsmRef.current, lidar] as unknown as uPlot.AlignedData
      const w = host.clientWidth || 640
      const xMin = total > FOLLOW_WINDOW_M ? total - FOLLOW_WINDOW_M : 0
      const xMax = Math.max(total, total * 1.02, 50)
      if (!plotRef.current) {
        host.textContent = ''
        const stroke = (c: string) => cssVar(c)
        const opts: uPlot.Options = {
          width: w,
          height: 180,
          legend: { show: true, live: false },
          scales: { x: { time: false, range: () => [xMin, xMax] } },
          axes: [
            { stroke: stroke('--mg-muted'), label: 'm', grid: { stroke: stroke('--mg-border') } },
            { stroke: stroke('--mg-muted'), label: 'm AMSL', grid: { stroke: stroke('--mg-border') } },
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
      } else {
        plotRef.current.setData(data)
        plotRef.current.setScale('x', { min: xMin, max: xMax })
      }
    }, REDRAW_MS)
    return () => clearInterval(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri, t])

  useEffect(() => {
    const onResize = () => {
      const host = hostRef.current
      if (host && plotRef.current) plotRef.current.setSize({ width: host.clientWidth || 640, height: 180 })
    }
    window.addEventListener('resize', onResize)
    return () => {
      window.removeEventListener('resize', onResize)
      plotRef.current?.destroy()
      plotRef.current = null
    }
  }, [])

  if (!tauri) return null

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2 text-[11px]">
        <span className={`mono ${lidarSeen ? 'text-ok' : 'text-muted'}`}>
          {lidarSeen ? t('drawer.profile.lidarUp') : t('drawer.profile.lidarDown')}
        </span>
        {demFolder ? (
          <span className="mono truncate text-muted" title={demFolder}>
            {demFolder.split(/[\\/]/).pop()}
          </span>
        ) : (
          <span className="text-warn">{t('drawer.profile.noDemHint')}</span>
        )}
        {demError && <span className="text-error">{demError}</span>}
      </div>
      <div ref={hostRef} className="min-h-[180px]" />
    </div>
  )
}