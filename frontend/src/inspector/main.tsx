// Inspector window entry: a separate webview that does not load Cesium
// (ADR-016). Boots the same design tokens so plots match the main UI.

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