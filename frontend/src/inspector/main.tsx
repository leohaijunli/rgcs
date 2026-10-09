// Inspector window entry: a separate webview that does not load Cesium
// (ADR-016). Boots the same design tokens so plots match the main UI.

import './inspector.css'
import React from 'react'
import { createRoot } from 'react-dom/client'
import { InspectorApp } from './InspectorApp'

const root = createRoot(document.getElementById('root') as HTMLElement)
root.render(
  <React.StrictMode>
    <InspectorApp />
  </React.StrictMode>,
)