import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import { useLinkStore } from '../stores/link'
import { useUiStore } from '../stores/ui'

type CommandName = 'pause' | 'resume' | 'rtl'

const COMMANDS: Array<{ name: CommandName; label: string; tone?: string }> = [
  { name: 'pause', label: 'plan.pause' },
  { name: 'resume', label: 'plan.resume' },
  { name: 'rtl', label: 'plan.rtl', tone: 'bg-warn text-canvas' },
]

export default function FlightCommands() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const link = useLinkStore((s) => s.link)
  if (view !== 'flight' || !link?.fc_alive) return null

  const send = async (name: CommandName) => {
    try {
      await invoke('send_command', { name })
    } catch {
      /* best-effort */
    }
  }

  return (
    <div className="pointer-events-auto absolute right-3 top-3 z-10 flex gap-1.5 rounded border border-line bg-panel/85 p-1.5 shadow-xl backdrop-blur">
      {COMMANDS.map((c) => (
        <button
          key={c.name}
          onClick={() => void send(c.name)}
          className={`rounded-md border border-line bg-canvas px-3 py-1.5 text-xs font-medium text-ink transition-colors hover:bg-accent hover:text-canvas ${
            c.tone ?? ''
          }`}
        >
          {t(c.label)}
        </button>
      ))}
    </div>
  )
}