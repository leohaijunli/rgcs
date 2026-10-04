import { useTelemetryBridge } from './desktop/bridge'
import { useUiStore } from './stores/ui'
import TopBar from './components/TopBar'
import IconRail from './components/IconRail'
import Drawer from './components/Drawer'
import RightInspector from './components/RightInspector'
import Dock from './components/Dock'
import Hud from './components/Hud'
import MapView from './components/MapView'
import ErrorBanner from './components/ErrorBanner'
import ConnectDialog from './components/dialogs/ConnectDialog'
import QcDialog from './components/dialogs/QcDialog'

export default function App() {
  const drawer = useUiStore((s) => s.drawer)
  useTelemetryBridge()

  return (
    <div className="flex h-screen flex-col bg-canvas text-ink">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <IconRail />
        {drawer && <Drawer />}
        <main className="relative min-w-0 flex-1">
          <MapView />
          <Hud />
          <ErrorBanner />
        </main>
        <RightInspector />
      </div>
      <Dock />
      <ConnectDialog />
      <QcDialog />
    </div>
  )
}