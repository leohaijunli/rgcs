import { useEffect } from 'react'
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
import AutoConnectDialog from './components/dialogs/AutoConnectDialog'
import TargetChangeDialog from './components/dialogs/TargetChangeDialog'
import TeardownDialog from './components/dialogs/TeardownDialog'
import PreflightPanel from './components/PreflightPanel'

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

export default function App() {
  useTelemetryBridge()
  usePlanHistoryShortcuts()
  // U12: `?` opens the keyboard shortcut sheet (Esc closes; no dangerous
  // command carries a single-key shortcut).
  const { open: shortcutsOpen, setOpen: setShortcutsOpen } = useShortcutsSheet()
  // The forced start page (operator decision 2026-10-18): with no project
  // open the app shows project selection only — recent projects listed for
  // one click, plus the folder picker and the create wizard. The browser
  // mock has no Tauri APIs behind it, so it keeps the main UI (dev + the
  // screenshot harness) unless `?start` previews the page.
  const project = useProjectStore((s) => s.project)
  const showStart =
    project === null &&
    (isTauri() || new URLSearchParams(window.location.search).has('start'))

  // Headless-harness escape (`?nomap`): the Cesium WebGL renderer crashes the
  // page under SwiftShader on some runners (A8 artifact), and every UI surface
  // except the map itself is testable without it. Production is unaffected.
  const noMap = new URLSearchParams(window.location.search).has('nomap')

  if (showStart) {
    return <StartPage />
  }

  return (
    <div className="flex h-screen flex-col bg-canvas text-ink">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <main className="relative flex min-w-0 flex-1 flex-col">
          <div className="relative min-h-0 flex-1">
            {noMap ? null : (
              <MapErrorBoundary>
                <MapView />
              </MapErrorBoundary>
            )}
            <MockBanner />
            <Hud />
            <HeartbeatBanner />
            <AutoConnectDialog />
            <TargetChangeDialog />
            <ShortcutsDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
            <AlertLayer />
            <ErrorBanner />
          </div>
          <ProfileDrawer />
        </main>
        <RightInspector />
      </div>
      <Dock />
      <TeardownDialog />
      <PreflightPanel />
    </div>
  )
}
