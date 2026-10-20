import './locale-shim'
import React from 'react'
import ReactDOM from 'react-dom/client'
import './design-system/index.css'
import './i18n'
import App from './App'
import ErrorBoundary from './components/ErrorBoundary'
import { initAppConfig } from './desktop/appConfig'

// Offline-first: Cesium workers/assets are served from our own bundle.
window.CESIUM_BASE_URL = '/cesium/'

// Load the Rust-owned prefs (A6) before first paint so the viewer/telemetry
// bootstrap from the saved values. The main window owns the config file.
void initAppConfig(true).finally(() => {
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  )
})