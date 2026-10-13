// Flight-plan window entry: a separate webview that does not load Cesium
// (ADR-016 pattern). Mirrors the main window's mission state; edits are
// routed back (see `planBridge.ts`).

import React from 'react'
import { createRoot } from 'react-dom/client'
import { PlanApp } from './PlanApp'
import ErrorBoundary from '../components/ErrorBoundary'
import { applyThemePref, readThemePref } from '../design-system/theme'
import '../i18n'

// Boot diagnostics: the webview has no visible console, so any error before
// (or outside) React lands in the DOM itself — a blank window always has a
// written reason on it.
function showBootError(where: string, err: unknown): void {
  const pre = document.createElement('pre')
  pre.id = 'boot-error'
  pre.style.cssText = 'padding:16px;color:#b00;white-space:pre-wrap;font-size:12px'
  pre.textContent = `[${where}] ${String(err)}`
  document.body.appendChild(pre)
}
window.addEventListener('error', (e) => showBootError('error', e.error ?? e.message))
window.addEventListener('unhandledrejection', (e) => showBootError('rejection', e.reason))

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
