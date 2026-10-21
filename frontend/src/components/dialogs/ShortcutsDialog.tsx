// The keyboard shortcut sheet (U12): `?` (Shift+/) opens it, Esc closes.
// Deliberately minimal — the plan forbids single-key shortcuts for dangerous
// commands, and this sheet only *lists* what exists (map tools W/P, undo /
// redo, Esc close, field-mode note); it adds no new bindings.

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import { isTextEntry } from '../../util/keys'

export function useShortcutsSheet(): { open: boolean; setOpen: (o: boolean) => void } {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === '?' && !e.ctrlKey && !e.metaKey && !e.altKey && !isTextEntry(e.target)) {
        e.preventDefault()
        setOpen((o) => !o)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
  return { open, setOpen }
}

const ROWS: Array<{ keys: string; action: string }> = [
  { keys: 'W', action: 'shortcuts.addWaypoint' },
  { keys: 'P', action: 'shortcuts.polygon' },
  { keys: 'Ctrl+Z / Ctrl+Shift+Z', action: 'shortcuts.undoRedo' },
  { keys: 'Esc', action: 'shortcuts.close' },
  { keys: '?', action: 'shortcuts.thisSheet' },
]

export default function ShortcutsDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (o: boolean) => void
}) {
  const { t } = useTranslation()
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="shortcuts-dialog"
          className="fixed left-1/2 top-1/2 z-50 w-[min(94vw,420px)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="text-sm font-medium text-ink">
            {t('shortcuts.title')}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-xs text-muted">
            {t('shortcuts.note')}
          </Dialog.Description>
          <table className="mt-3 w-full text-xs">
            <tbody>
              {ROWS.map((row) => (
                <tr key={row.keys} className="border-t border-line">
                  <td className="mono py-1.5 pr-3 text-ink">{row.keys}</td>
                  <td className="py-1.5 text-muted">{t(row.action)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="mt-3 flex justify-end">
            <button
              data-testid="shortcuts-close"
              onClick={() => onOpenChange(false)}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
            >
              {t('shortcuts.close')}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
