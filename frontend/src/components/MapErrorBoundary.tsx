// A8: the map fails alone. The Cesium globe renders inside this boundary so
// a render crash or a WebGL context loss takes out only the map — the TopBar,
// the safety commands, the alerts and the drape panel (the app's spine) keep
// running, and the operator gets one button to bring the map back.
import { Component, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

interface Props {
  children: ReactNode
}

interface State {
  error: Error | null
  /** Bumped by Retry: remounts the whole map subtree (fresh WebGL context). */
  attempt: number
}

export default class MapErrorBoundary extends Component<Props, State> {
  state: State = { error: null, attempt: 0 }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error }
  }

  componentDidCatch(error: Error) {
    console.error('[MapErrorBoundary]', error)
  }

  // WebGL context loss arrives as a DOM event, outside React's render cycle
  // — a boundary cannot catch it there, so the viewer reports it here via a
  // custom event instead.
  componentDidMount() {
    window.addEventListener('maggcs:map-failed', this.onMapFailed as EventListener)
  }

  componentWillUnmount() {
    window.removeEventListener('maggcs:map-failed', this.onMapFailed as EventListener)
  }

  private readonly onMapFailed = (e: Event) => {
    const detail = (e as CustomEvent<string>).detail
    this.setState({ error: new Error(detail ?? 'map failed') })
  }

  render() {
    if (this.state.error) {
      return <MapFailed error={this.state.error} onRetry={() => this.setState({ error: null, attempt: this.state.attempt + 1 })} />
    }
    return (
      <div key={this.state.attempt} className="h-full w-full">
        {this.props.children}
      </div>
    )
  }
}

function MapFailed({ error, onRetry }: { error: Error; onRetry: () => void }) {
  const { t } = useTranslation()
  return (
    <div
      className="flex h-full flex-col items-center justify-center gap-2 bg-canvas p-6 text-center"
      data-testid="map-failed"
    >
      <div className="text-sm font-medium text-error">{t('mapBoundary.title')}</div>
      <pre className="mono max-h-24 max-w-full overflow-auto rounded border border-line bg-panel p-3 text-xs text-ink">
        {error.message}
      </pre>
      <div className="text-[11px] text-muted">{t('mapBoundary.note')}</div>
      <button
        onClick={onRetry}
        className="rounded border border-line bg-panel px-4 py-2 text-sm text-ink hover:text-accent"
      >
        {t('mapBoundary.retry')}
      </button>
    </div>
  )
}
