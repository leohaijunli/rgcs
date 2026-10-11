// Motor / mag interference test window entry: a separate webview without
// Cesium (motor-test window). Boots the shared design tokens.

import './actuator.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ActuatorApp } from './ActuatorApp'
import ErrorBoundary from '../components/ErrorBoundary'

// Restore the persisted theme before first paint (the main window's
// settings own it; this window only mirrors `maggcs.theme` so its tokens
// and native controls pick the right color-scheme at boot).
document.documentElement.dataset.theme =
  localStorage.getItem('maggcs.theme') || 'dark'

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