// Motor / mag interference test window entry: a separate webview without
// Cesium (motor-test-plan §5). Boots the shared design tokens.

import './actuator.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ActuatorApp } from './ActuatorApp'
import ErrorBoundary from '../components/ErrorBoundary'

const root = createRoot(document.getElementById('root') as HTMLElement)
root.render(
  <React.StrictMode>
    <ErrorBoundary
      fallback={(e) => (
        <pre style={{ padding: 16, color: 'var(--mg-ink)', whiteSpace: 'pre-wrap' }}>
          Motor test window crashed: {e.message}
        </pre>
      )}
    >
      <ActuatorApp />
    </ErrorBoundary>
  </React.StrictMode>,
)