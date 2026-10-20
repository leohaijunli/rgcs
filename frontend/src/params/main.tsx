// Parameters window entry (parameters plan PM3): a separate webview without
// Cesium. Boots the shared design tokens.

import './params.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ParamsApp } from './ParamsApp'
import ErrorBoundary from '../components/ErrorBoundary'
import AlertLayer from '../components/AlertLayer'
import { initAppConfig } from '../desktop/appConfig'
import { applyThemePref, readThemePref } from '../design-system/theme'

// Restore the persisted theme before first paint (the main window's
// settings own it; this window only mirrors `maggcs.theme` so its tokens
// and native controls pick the right color-scheme at boot).
applyThemePref(readThemePref())

// Read-only hydrate of the shared config so the alert layer uses the saved
// volume; this window never writes the file (A6).
void initAppConfig().finally(() => {
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
        <AlertLayer />
      </ErrorBoundary>
    </React.StrictMode>,
  )
})