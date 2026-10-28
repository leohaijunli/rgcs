import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useTelemetryBridge } from './desktop/bridge'
import { isTauri } from './inspector/mock'
import { useProjectStore } from './stores/project'
import StartPage from './components/StartPage'
import { useMissionStore } from './stores/mission'
import TopBar from './components/TopBar'
import RightInspector from './components/RightInspector'
import Dock from './components/Dock'
import Hud from './components/Hud'
import MockBanner from './components/MockBanner'
import MapView from './components/MapView'
import MapErrorBoundary from './components/MapErrorBoundary'
import ProfileDrawer from './components/ProfileDrawer'
import ErrorBanner from './components/ErrorBanner'
import AlertLayer from './components/AlertLayer'
import HeartbeatBanner from './components/HeartbeatBanner'
import { isTextEntry } from './util/keys'
import ShortcutsDialog, { useShortcutsSheet } from './components/dialogs/ShortcutsDialog'
import FlightTargetDialog from './components/dialogs/FlightTargetDialog'
import AutoConnectDialog from './components/dialogs/AutoConnectDialog'
import TargetChangeDialog from './components/dialogs/TargetChangeDialog'
import TeardownDialog from './components/dialogs/TeardownDialog'
import PreflightPanel from './components/PreflightPanel'
import { useDetachedMap } from './stores/detachedMap'

/** True when the event target is a text field where the browser's own undo
 * should win (Ctrl+Z inside an input must not revert the whole plan). */
/** U8: global plan undo/redo. Ctrl/Cmd+Z undoes, Ctrl/Cmd+Shift+Z (or Ctrl+Y)
 * redoes. Text fields keep their native behaviour. */
function usePlanHistoryShortcuts(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return
      if (isTextEntry(e.target)) return
      const key = e.key.toLowerCase()
      if (key === 'z') {
        e.preventDefault()
        if (e.shiftKey) useMissionStore.getState().redo()
        else useMissionStore.getState().undo()
      } else if (key === 'y') {
        e.preventDefault()
        useMissionStore.getState().redo()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}

/** Connection/lifecycle surfaces shared by the start page and the workspace:
 * the telemetry bridge runs in both, so auto-connect, target-change, alert,
 * heartbeat and error feedback must be mounted in both (finding S1). */
function LifecycleOverlays() {
  return (
    <>
      <HeartbeatBanner />
      <AutoConnectDialog />
      <TargetChangeDialog />
      <AlertLayer />
      <ErrorBanner />
    </>
  )
}

export default function App() {
  useTelemetryBridge()
  usePlanHistoryShortcuts()
  // U12: `?` opens the keyboard shortcut sheet (Esc closes; no dangerous
  // command carries a single-key shortcut).
  const { t } = useTranslation()
  const { open: shortcutsOpen, setOpen: setShortcutsOpen } = useShortcutsSheet()
  // The forced start page (operator decision 2026-10-18): with no project
  // open the app shows project selection only — recent projects listed for
  // one click, plus the folder picker and the create wizard. The browser
  // mock has no Tauri APIs behind it, so it keeps the main UI (dev + the
  // screenshot harness) unless `?start` previews the page.
  const project = useProjectStore((s) => s.project)
  const startDismissed = useProjectStore((s) => s.startDismissed)
  const showStart =
    project === null &&
    !startDismissed &&
    (isTauri() || new URLSearchParams(window.location.search).has('start'))

  // Headless-harness escape (`?nomap`): the Cesium WebGL renderer crashes the
  // page under SwiftShader on some runners (A8 artifact), and every UI surface
  // except the map itself is testable without it. Production is unaffected.
  const noMap = new URLSearchParams(window.location.search).has('nomap')
  // U15: the map can live in its own webview — the main window then runs
  // map-free (bigger HUD/strip/checklist) and shows a re-embed affordance.
  const detachedMap = useDetachedMap()
  const hideMap = noMap || detachedMap.detached

  if (showStart) {
    return (
      <>
        <StartPage />
        <LifecycleOverlays />
      </>
    )
  }

  return (
    <div className="flex h-screen flex-col bg-canvas text-ink">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <main className="relative flex min-w-0 flex-1 flex-col">
          <div className="relative min-h-0 flex-1">
            {/* Detached map (U15): the map area is *gone*, not blank — the
             * HUD, the command strip and the profile drawer take the room,
             * and only a slim bar names the state and offers the way back
             * (a big empty panel would just waste the screen). */}
            {detachedMap.detached && (
              <div
                data-testid="map-detached-strip"
                className="flex items-center gap-2 border-b border-line bg-panel px-3 py-1 text-[11px] text-muted"
              >
                <span>{t('mapWindow.detachedHere')}</span>
                <button
                  data-testid="map-bring-back"
                  onClick={() => void detachedMap.bringBack()}
                  className="rounded border border-line bg-canvas px-2 py-0.5 text-[11px] text-ink hover:bg-panel"
                >
                  {t('mapWindow.bringBack')}
                </button>
              </div>
            )}
            {!hideMap && (
              <MapErrorBoundary>
                <MapView />
              </MapErrorBoundary>
            )}
            <MockBanner />
            <Hud />
            <LifecycleOverlays />
            {/* Mounted at the shell, not the map: the map context menu opens
                it, and the headless (`?nomap`) harness has no map at all. */}
            <FlightTargetDialog />
            <ShortcutsDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
          </div>
          <ProfileDrawer defaultOpen={detachedMap.detached} />
        </main>
        <RightInspector />
      </div>
      <Dock />
      <TeardownDialog />
      <PreflightPanel />
    </div>
  )
}
