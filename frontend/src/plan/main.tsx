// Flight-plan window entry: a separate webview that does not load Cesium
// (ADR-016 pattern). Mirrors the main window's mission state; edits are
// routed back (see `planBridge.ts`).

import React from 'react'
import { createRoot } from 'react-dom/client'
import { PlanApp } from './PlanApp'
import ErrorBoundary from '../components/ErrorBoundary'
import { applyThemePref, readThemePref } from '../design-system/theme'

// Restore the persisted theme before first paint (mirrors `maggcs.theme`).
applyThemePref(readThemePref())

const root = createRoot(document.getElementById('root') as HTMLElement)
root.render(
  <React.StrictMode>
    <ErrorBoundary
      fallback={(e) => (
        <pre style={{ padding: 16, color: 'var(--mg-ink)', whiteSpace: 'pre-wrap' }}>
          Flight plan window crashed: {e.message}
        </pre>
      )}
    >
      <PlanApp />
    </ErrorBoundary>
  </React.StrictMode>,
)
