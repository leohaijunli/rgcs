// Detachable map window entry (plan §7 U15): the same MapView + alert layer
// the embedded map uses, in its own webview. It is a VIEW ONLY: no command
// authority (RTL/Hold/checklist stay in the main window), so a crash here
// cannot strand the operator without controls. The alert banner is mounted
// here too, so a Critical stays visible on a full-screen map (U3/U15).

import '../design-system/index.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { useTranslation } from 'react-i18next'
import '../i18n'
import MapView from '../components/MapView'
import MapErrorBoundary from '../components/MapErrorBoundary'
import AlertLayer from '../components/AlertLayer'
import HeartbeatBanner from '../components/HeartbeatBanner'
import { initAppConfig } from '../desktop/appConfig'
import { useTelemetryBridge } from '../desktop/bridge'
import { applyFieldMode, applyThemePref, readFieldMode, readThemePref } from '../design-system/theme'

applyThemePref(readThemePref())
applyFieldMode(readFieldMode())

/** A slim, click-through caption: which window this is, and that closing it
 * re-embeds the map in the main window (U15 window semantics). */
/** The bridge must run here too: link, telemetry and mission events are what
 * MapView draws (without it the detached map is an empty globe). */
function MapWindowShell() {
  useTelemetryBridge()
  return (
    <div className="relative h-screen w-screen bg-canvas text-ink">
      <MapErrorBoundary>
        <MapView />
      </MapErrorBoundary>
      <MapWindowBar />
      <HeartbeatBanner />
      <AlertLayer />
    </div>
  )
}

function MapWindowBar() {
  const { t } = useTranslation()
  return (
    <div className="pointer-events-none absolute left-2 top-2 z-20 rounded border border-line bg-panel/90 px-2 py-1 text-[11px] text-muted">
      {t('mapWindow.caption')}
    </div>
  )
}

void initAppConfig().finally(() => {
  const root = createRoot(document.getElementById('root') as HTMLElement)
  root.render(
    <React.StrictMode>
      <MapWindowShell />
    </React.StrictMode>,
  )
})
