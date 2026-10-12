// FC SD-card logs (moved out of Settings into the Tools menu, following the
// motor-test plan's window-launcher grouping): MAVLink LOG list + download
// with per-file progress, fed by the `logs_event` shell watcher.

import { useEffect } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useLinkStore } from '../../stores/link'
import { startLogsWatcher, useLogsStore } from '../../stores/logs'

/** Modal shell for the FC logs panel (same skin as the Settings dialog). */
export default function LogsDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useTranslation()
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex h-[min(85vh,560px)] w-[min(94vw,600px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-panel shadow-2xl">
          <div className="flex items-center justify-between border-b border-line px-4 py-3">
            <Dialog.Title className="text-sm font-medium text-ink">{t('tools.logs')}</Dialog.Title>
            <Dialog.Close asChild>
              <button
                aria-label="close"
                className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            <LogsPanel />
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function LogsPanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const entries = useLogsStore((s) => s.entries)
  const downloads = useLogsStore((s) => s.downloads)
  const busy = useLogsStore((s) => s.busy)
  const error = useLogsStore((s) => s.error)
  const requestList = useLogsStore((s) => s.requestList)
  const download = useLogsStore((s) => s.download)
  const cancel = useLogsStore((s) => s.cancel)

  // Idempotent watcher start (the shell events are cheap; the store only
  // updates on `logs_event`).
  useEffect(() => {
    startLogsWatcher()
  }, [])

  const fcAlive = link?.fc_alive === true

  return (
    <div className="space-y-3 text-sm">
      <div className="flex items-center gap-2">
        <button
          onClick={() => void requestList()}
          disabled={!fcAlive || busy}
          className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel disabled:opacity-50"
        >
          {t('settings.logs.refresh')}
        </button>
        {busy && (
          <button
            onClick={() => void cancel()}
            className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-warn hover:bg-panel"
          >
            {t('settings.logs.cancel')}
          </button>
        )}
        <span className="text-xs text-muted">
          {t('settings.logs.hint')}
        </span>
      </div>
      {error && <div className="text-xs text-error">{error}</div>}
      {!fcAlive && <div className="text-xs text-muted">{t('link.noLink')}</div>}
      {fcAlive && entries.length === 0 && !busy && (
        <div className="text-xs text-muted">{t('settings.logs.emptyList')}</div>
      )}
      <ul className="space-y-1">
        {entries.map((entry) => {
          const dl = downloads.get(entry.id)
          const pct = dl ? Math.min(100, Math.round((dl.fetched / Math.max(1, dl.total)) * 100)) : 0
          const date = entry.time_utc > 0 ? new Date(entry.time_utc * 1000).toISOString().slice(0, 16).replace('T', ' ') : '—'
          return (
            <li key={entry.id} className="rounded border border-line bg-canvas px-2 py-1.5">
              <div className="flex items-center gap-2 text-xs">
                <span className="mono w-10 text-muted">#{entry.id}</span>
                <span className="mono w-32 text-muted">{date}</span>
                <span className="mono flex-1 text-ink">{(entry.size / 1024 / 1024).toFixed(1)} MB</span>
                {dl?.path ? (
                  <span className="text-ok">{t('settings.logs.saved')}</span>
                ) : (
                  <button
                    onClick={() => void download(entry.id, entry.size)}
                    disabled={busy}
                    className="rounded border border-line bg-canvas px-2 py-0.5 text-[11px] text-ink hover:bg-panel disabled:opacity-50"
                  >
                    {t('settings.logs.download')}
                  </button>
                )}
              </div>
              {dl && !dl.path && (
                <div className="mt-1 h-1 rounded bg-panel">
                  <div className="h-1 rounded bg-accent" style={{ width: `${pct}%` }} />
                </div>
              )}
              {dl?.path && <div className="mt-0.5 truncate text-[10px] text-muted">{dl.path}</div>}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
