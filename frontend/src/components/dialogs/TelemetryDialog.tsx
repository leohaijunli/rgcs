import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useUiStore } from '../../stores/ui'
import PfdCluster from '../instruments/PfdCluster'

/** Full instrument dashboard as an independent popup (off the map). */
export default function TelemetryDialog() {
  const { t } = useTranslation()
  const open = useUiStore((s) => s.dashboardOpen)
  const setOpen = useUiStore((s) => s.setDashboardOpen)

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 max-h-[90vh] w-[min(92vw,720px)] -translate-x-1/2 -translate-y-1/2 overflow-auto rounded-lg border border-line bg-panel p-4 shadow-2xl">
          <div className="mb-2 flex items-center justify-between">
            <Dialog.Title className="text-sm font-medium text-ink">
              {t('panels.telemetry')}
            </Dialog.Title>
            <Dialog.Close asChild>
              <button
                aria-label="close"
                className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>
          <PfdCluster />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}