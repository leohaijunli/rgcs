import { useCallback, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { useUiStore, type DockTab } from '../stores/ui'

const TABS: DockTab[] = ['profile', 'qc', 'log']

export default function Dock() {
  const { t } = useTranslation()
  const dockOpen = useUiStore((s) => s.dockOpen)
  const dockHeight = useUiStore((s) => s.dockHeight)
  const setDockHeight = useUiStore((s) => s.setDockHeight)
  const setDockOpen = useUiStore((s) => s.setDockOpen)
  const activeTab = useUiStore((s) => s.dockTab)
  const setDockTab = useUiStore((s) => s.setDockTab)
  const startY = useRef(0)
  const startH = useRef(0)

  const onResizeStart = useCallback(
    (e: React.PointerEvent) => {
      startY.current = e.clientY
      startH.current = dockHeight
      const move = (ev: PointerEvent) => {
        setDockHeight(startH.current + (startY.current - ev.clientY))
      }
      const up = () => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
    },
    [dockHeight, setDockHeight],
  )

  if (!dockOpen) {
    return (
      <div className="flex h-7 shrink-0 items-center gap-0.5 border-t border-line bg-panel px-2">
        {TABS.map((tab) => (
          <button
            key={tab}
            onClick={() => {
              setDockTab(tab)
              setDockOpen(true)
            }}
            className={`rounded px-2 py-1 text-xs ${
              tab === activeTab ? 'text-accent' : 'text-muted hover:text-ink'
            }`}
          >
            {t(`dock.${tab}`)}
          </button>
        ))}
        <span className="ml-auto pr-2 text-xs text-muted">▴</span>
      </div>
    )
  }

  return (
    <footer
      className="flex shrink-0 flex-col border-t border-line bg-panel"
      style={{ height: dockHeight }}
    >
      <div
        className="group flex h-2 shrink-0 cursor-ns-resize items-center justify-center border-b border-line"
        onPointerDown={onResizeStart}
      >
        <div className="h-0.5 w-16 rounded bg-muted opacity-40 group-hover:opacity-80" />
      </div>
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line px-2">
        {TABS.map((tab) => (
          <button
            key={tab}
            onClick={() => setDockTab(tab)}
            className={`rounded px-2 py-1 text-xs ${
              tab === activeTab ? 'bg-accent text-accent-ink' : 'text-muted hover:text-ink'
            }`}
          >
            {t(`dock.${tab}`)}
          </button>
        ))}
        <button
          onClick={() => setDockOpen(false)}
          className="ml-auto rounded px-2 py-1 text-xs text-muted hover:text-ink"
        >
          ▾
        </button>
      </div>
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto p-3 text-sm text-muted">
        {activeTab === 'profile' && <span>{t('views.profilePlaceholder')}</span>}
        {activeTab === 'qc' && <span>{t('dock.qcPlaceholder')}</span>}
        {activeTab === 'log' && <span>{t('dock.logPlaceholder')}</span>}
      </div>
    </footer>
  )
}