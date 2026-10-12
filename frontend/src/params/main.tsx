// Parameters window entry (parameters plan PM3): a separate webview without
// Cesium. Boots the shared design tokens.

import './params.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ParamsApp } from './ParamsApp'
import ErrorBoundary from '../components/ErrorBoundary'
import { applyThemePref, readThemePref } from '../design-system/theme'

// Restore the persisted theme before first paint (the main window's
// settings own it; this window only mirrors `maggcs.theme` so its native
// controls and tokens pick the right color-scheme at boot).
applyThemePref(readThemePref())

const root = createRoot(document.getElementById('root') as HTMLElement)
root.render(
  <React.StrictMode>
    <ErrorBoundary
      fallback={(e) => (
        <pre style={{ padding: 16, color: 'var(--mg-ink)', whiteSpace: 'pre-wrap' }}>
          Parameters window crashed: {e.message}
        </pre>
      )}
    >
      <ParamsApp />
    </ErrorBoundary>
  </React.StrictMode>,
)