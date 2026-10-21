// The auto-connect confirmation dialog (L8): "Connect to X?" with a 3 s
// countdown (plan §4 L8: 自动连接前显示"将连接到 X"并留 3 s 取消窗口). The
// store owns the window — expiry fires the connect even when this dialog is
// not mounted; Cancel drops the request for this launch. Display only.

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import { useAutoConnectStore } from '../../stores/autoconnect'

const COUNTDOWN_STEP_MS = 100

export default function AutoConnectDialog() {
  const { t } = useTranslation()
  const endpoint = useAutoConnectStore((s) => s.pendingEndpoint)
  const deadlineMs = useAutoConnectStore((s) => s.deadlineMs)
  const cancel = useAutoConnectStore((s) => s.cancel)
  const [remainingMs, setRemainingMs] = useState(0)

  useEffect(() => {
    if (!endpoint) return
    const tick = () => setRemainingMs(Math.max(0, deadlineMs - Date.now()))
    const id = window.setInterval(tick, COUNTDOWN_STEP_MS)
    tick()
    return () => window.clearInterval(id)
  }, [endpoint, deadlineMs])

  if (!endpoint) return null
  const seconds = Math.ceil(remainingMs / 1000)

  return (
    <Dialog.Root open>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="autoconnect-dialog"
          className="fixed left-1/2 top-16 z-50 w-[min(94vw,420px)] -translate-x-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="text-sm font-medium text-ink">
            {t('autoconnect.title')}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-xs text-muted">
            {t('autoconnect.body', { endpoint, seconds })}
          </Dialog.Description>
          <div className="mt-3 flex justify-end gap-2">
            <button
              data-testid="autoconnect-cancel"
              onClick={cancel}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
            >
              {t('autoconnect.cancel')}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
