// Parameters window entry (parameters plan PM3): a separate webview without
// Cesium. Boots the shared design tokens.

import './params.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { ParamsApp } from './ParamsApp'
import ErrorBoundary from '../components/ErrorBoundary'

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