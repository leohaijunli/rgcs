// Inspector window entry: a separate webview that does not load Cesium
// (ADR-016). Boots the same design tokens so plots match the main UI.

// uPlot's base CSS must be loaded before our overrides so `.u-over`/`.u-under`
// are positioned correctly (cursor, hover and zoom depend on it).
import 'uplot/dist/uPlot.min.css'
import './inspector.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { InspectorApp } from './InspectorApp'
import ErrorBoundary from '../components/ErrorBoundary'

const root = createRoot(document.getElementById('root') as HTMLElement)
root.render(
  <React.StrictMode>
    <ErrorBoundary
      fallback={(e) => (
        <pre style={{ padding: 16, color: 'var(--mg-ink)', whiteSpace: 'pre-wrap' }}>
          Signal Inspector crashed: {e.message}
        </pre>
      )}
    >
      <InspectorApp />
    </ErrorBoundary>
  </React.StrictMode>,
)
