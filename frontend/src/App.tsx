import { useTelemetryBridge } from './desktop/bridge'
import TopBar from './components/TopBar'
import RightInspector from './components/RightInspector'
import Dock from './components/Dock'
import Hud from './components/Hud'
import MockBanner from './components/MockBanner'
import MapView from './components/MapView'
import ErrorBanner from './components/ErrorBanner'
import HeartbeatBanner from './components/HeartbeatBanner'
import TeardownDialog from './components/dialogs/TeardownDialog'

export default function App() {
  useTelemetryBridge()

  return (
    <div className="flex h-screen flex-col bg-canvas text-ink">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <main className="relative min-w-0 flex-1">
          <MapView />
          <MockBanner />
          <Hud />
          <HeartbeatBanner />
          <ErrorBanner />
        </main>
        <RightInspector />
      </div>
      <Dock />
      <TeardownDialog />
    </div>
  )
}
