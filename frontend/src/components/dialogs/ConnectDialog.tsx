import { useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { Plug, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { connectEndpoint, reportConnectError } from '../../desktop/connect'

const DEFAULT_ENDPOINT = 'udpin:0.0.0.0:14550'

/** Connection dialog: pick a MAVLink endpoint (UDP/TCP/serial) and connect. */
export default function ConnectDialog() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [endpoint, setEndpoint] = useState(DEFAULT_ENDPOINT)
  const [busy, setBusy] = useState(false)

  const connect = async () => {
    setBusy(true)
    try {
      await connectEndpoint(endpoint.trim())
      setOpen(false)
    } catch (e) {
      reportConnectError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger asChild>
        <button
          title={t('map.connect')}
          aria-label={t('map.connect')}
          onClick={() => setOpen(true)}
          className="flex h-11 w-11 items-center justify-center rounded text-muted hover:bg-canvas hover:text-ink"
        >
          <Plug size={16} />
        </button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 w-[min(92vw,420px)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl">
          <div className="mb-3 flex items-center justify-between">
            <Dialog.Title className="text-sm font-medium text-ink">{t('map.connect')}</Dialog.Title>
            <Dialog.Close asChild>
              <button
                aria-label="close"
                className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>
          <p className="mb-2 text-xs text-muted">{t('map.endpointHint')}</p>
          <input
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="udpin:0.0.0.0:14550"
            spellCheck={false}
            className="mono w-full rounded border border-line bg-canvas px-3 py-2 text-sm text-ink outline-none focus:border-accent"
          />
          <div className="mt-3 flex justify-end gap-2">
            <button
              onClick={() => setOpen(false)}
              className="rounded border border-line bg-canvas px-3 py-1.5 text-sm text-muted hover:text-ink"
            >
              {t('map.cancel')}
            </button>
            <button
              onClick={connect}
              disabled={busy}
              className="rounded bg-accent px-3 py-1.5 text-sm text-accent-ink disabled:opacity-50"
            >
              {busy ? '…' : t('map.connect')}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}