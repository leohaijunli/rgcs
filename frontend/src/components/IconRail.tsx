import { Layers, Map as MapIcon, Plane } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useUiStore, type DrawerId } from '../stores/ui'

const ITEMS: Array<{ id: NonNullable<DrawerId>; icon: typeof MapIcon; label: string }> = [
  { id: 'missions', icon: MapIcon, label: 'panels.missions' },
  { id: 'vehicles', icon: Plane, label: 'panels.vehicles' },
  { id: 'layers', icon: Layers, label: 'panels.layers' },
]

export default function IconRail() {
  const { t } = useTranslation()
  const drawer = useUiStore((s) => s.drawer)
  const toggleDrawer = useUiStore((s) => s.toggleDrawer)

  return (
    <nav className="flex w-12 shrink-0 flex-col items-center border-r border-line bg-panel py-2">
      {ITEMS.map(({ id, icon: Icon, label }) => {
        const active = drawer === id
        return (
          <button
            key={id}
            title={t(label)}
            aria-label={t(label)}
            onClick={() => toggleDrawer(id)}
            className={`touch-target my-1 flex h-12 w-12 items-center justify-center rounded transition-colors ${
              active ? 'bg-accent text-accent-ink' : 'text-muted hover:bg-canvas hover:text-ink'
            }`}
          >
            <Icon size={20} strokeWidth={1.75} />
          </button>
        )
      })}
    </nav>
  )
}