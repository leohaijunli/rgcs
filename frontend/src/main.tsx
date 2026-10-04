import React from 'react'
import ReactDOM from 'react-dom/client'
import './design-system/index.css'
import './i18n'
import App from './App'
import ErrorBoundary from './components/ErrorBoundary'

// Offline-first: Cesium workers/assets are served from our own bundle.
window.CESIUM_BASE_URL = '/cesium/'

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
)