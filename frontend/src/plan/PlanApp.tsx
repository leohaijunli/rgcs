// Flight Plan Design (ADR-016 pattern): the plan editor in its own OS
// window, without Cesium. The mission store lives in the main window —
// this window mirrors its snapshot and sends edit intents (`plan_edit`)
// that the main window applies. The map keeps living in the main window;
// this one buys screen space for the step-by-step design flow.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  onPlanSnapshot,
  sendPlanEdit,
  startPlanBridge,
  type PlanSnapshot,
} from './planBridge'
import { useMissionStore, type AltitudeMode } from '../stores/mission'
import { usePolygonStore } from '../stores/polygon'
import { useUiStore } from '../stores/ui'
import PlanSteps from '../components/panels/plan/PlanSteps'

export function PlanApp() {
  useEffect(() => {
    startPlanBridge()
  }, [])
  return <PlanMirror />
}

/** Replay snapshots into the local mission store so the existing PlanSteps
 * UI works unmodified; store actions are rerouted to the main window by
 * [`installEditBridge`]. */
function PlanMirror() {
  const [, setRev] = useState(-1)
  useEffect(
    () =>
      onPlanSnapshot((s) => {
        applySnapshot(s)
        setRev(s.rev)
      }),
    [],
  )
  useEffect(() => {
    installEditBridge()
  }, [])
  return (
    <div className="h-screen overflow-y-auto bg-canvas text-ink">
      <PlanHeader />
      <div className="mx-auto max-w-xl p-3">
        <PlanSteps />
      </div>
    </div>
  )
}

function PlanHeader() {
  const { t } = useTranslation()
  return (
    <div className="flex items-center justify-between border-b border-line bg-panel px-3 py-2">
      <span className="text-sm font-medium text-ink">{t('plan.windowTitle')}</span>
      <span className="text-[10px] text-muted">{t('plan.windowHint')}</span>
    </div>
  )
}

/** Copy a snapshot into the local stores. Waypoints/pattern/etc. come from
 * the main window; local-only UI (steps, form drafts) stays here. The
 * survey polygon and map centre are read-only mirrors (drawing happens on
 * the main window's map). */
function applySnapshot(s: PlanSnapshot): void {
  useMissionStore.setState({
    waypoints: s.waypoints as never,
    blocks: s.blocks as never,
    home: s.home,
    patternCenter: s.patternCenter,
    lastPattern: s.lastPattern as never,
    drape: s.drape as never,
    altitudeMode: s.altitudeMode as AltitudeMode,
    surveyAglM: s.surveyAglM,
    selectedSeq: s.selectedSeq,
    busy: s.busy,
    dirty: s.dirty,
    syncState: s.syncState as never,
    fcMatches: s.fcMatches,
  })
  const polygon = usePolygonStore.getState()
  const polygonChanged =
    polygon.closed !== s.polygon.closed ||
    polygon.vertices.length !== (s.polygon.vertices as unknown[]).length
  if (polygonChanged) {
    usePolygonStore.setState({
      vertices: s.polygon.vertices as never,
      closed: s.polygon.closed,
    })
  }
  if (useUiStore.getState().mapCenter !== s.mapCenter) {
    useUiStore.setState({ mapCenter: s.mapCenter })
  }
}

/**
 * Route the store actions PlanSteps calls back to the main window instead of
 * mutating the local mirror. Stateful backend calls the panels make directly
 * (`survey_generate_*`, `survey_compile_pattern`) are stateless Tauri
 * commands and keep working from this window; their result flows into
 * `insertPattern`, which is bridged. Polygon drawing stays a main-window
 * activity (it needs the map).
 */
let bridgeInstalled = false
function installEditBridge(): void {
  if (bridgeInstalled) return
  bridgeInstalled = true
  useMissionStore.setState({
    insertPattern: (plan, label, center, stats) => {
      void sendPlanEdit('insertPattern', { plan, label, center, stats })
    },
    clearPlan: () => {
      void sendPlanEdit('clearPlan')
    },
    setAltitudeMode: (mode) => {
      void sendPlanEdit('setAltitudeMode', { mode })
    },
    setSurveyAglM: (aglM) => {
      void sendPlanEdit('setSurveyAglM', { aglM })
    },
    setPatternCenter: (center) => {
      void sendPlanEdit('setPatternCenter', { center })
    },
    moveWaypoint: (from, to) => {
      void sendPlanEdit('moveWaypoint', { from, to })
    },
    removeWaypoint: (seq) => {
      void sendPlanEdit('removeWaypoint', { seq })
    },
    upload: async () => {
      await sendPlanEdit('upload')
    },
    drapeToTerrain: async (folder, aglM, minDsmClearanceM) => {
      await sendPlanEdit('drapeToTerrain', { folder, aglM, minDsmClearanceM })
      return null
    },
    addWaypoint: (latDeg, lonDeg, amslM) => {
      void sendPlanEdit('addWaypoint', { latDeg, lonDeg, amslM })
    },
    addWaypointAt: (latDeg, lonDeg) => {
      void sendPlanEdit('addWaypointAt', { latDeg, lonDeg })
    },
    updatePosition: (seq, lat, lon) => {
      void sendPlanEdit('updatePosition', { seq, lat, lon })
    },
    updateAltitude: (seq, altM) => {
      void sendPlanEdit('updateAltitude', { seq, altM })
    },
    setCurrent: (seq) => {
      void sendPlanEdit('setCurrent', { seq })
    },
    download: async () => {
      await sendPlanEdit('download')
    },
  } as Partial<ReturnType<typeof useMissionStore.getState>>)
}
