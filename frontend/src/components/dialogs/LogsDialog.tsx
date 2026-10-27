// FC SD-card logs (moved out of Settings into the Tools menu, following the
// motor-test plan's window-launcher grouping): MAVLink LOG list + download
// with target/operation-bound progress and durable local-file receipts.

import { useEffect, useRef, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useLinkStore } from '../../stores/link'
import { startLogsWatcher, useLogsStore } from '../../stores/logs'
import { CONSOLE_PRESETS, isReadOnlyConsoleCommand, useConsoleStore } from '../../stores/console'
import type { LogEntryInfo } from '../../generated-types/LogEntryInfo'
import type { RuntimeTargetRef } from '../../generated-types/RuntimeTargetRef'
import { consoleContext } from '../../desktop/consoleRuntime'
import AppLogo from '../AppLogo'

/** Modal shell for the FC logs panel (same skin as the Settings dialog). */
export default function LogsDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useTranslation()
  const closeConsole = useConsoleStore((state) => state.close)
  const cancelLogs = useLogsStore((state) => state.cancel)
  return (
    <Dialog.Root open={open} onOpenChange={(next) => {
      if (!next) { void closeConsole(); void cancelLogs() }
      onOpenChange(next)
    }}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex h-[min(94vh,900px)] w-[min(96vw,960px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-panel shadow-2xl">
          <div className="flex items-center justify-between border-b border-line px-4 py-3">
            <div className="flex items-center gap-2">
              <span className="tool-header-brand is-compact">
                <AppLogo variant="mark" />
              </span>
              <Dialog.Title className="text-sm font-medium text-ink">{t('tools.logs')}</Dialog.Title>
            </div>
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
          <div className="border-t border-line p-4">
            <ConsolePanel />
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
  const eraseAll = useLogsStore((s) => s.eraseAll)
  const remove = useLogsStore((s) => s.remove)
  const reconciliation = useLogsStore((s) => s.reconciliation)
  const consoleActive = useConsoleStore((s) => s.operation !== null)

  // Observer events are matched against the active native session/request.
  useEffect(() => {
    startLogsWatcher()
  }, [])

  const fcAlive = link?.fc_alive === true

  // Both destructive actions require two explicit confirmations; declining
  // either sends no command to the FC.
  const eraseAllLogs = async () => {
    if (!window.confirm(t('settings.logs.confirmEraseAll'))) return
    if (!window.confirm(t('settings.logs.confirmEraseAllLast'))) return
    await eraseAll()
  }

  const removeLog = async (entry: LogEntryInfo) => {
    const size = (entry.size / 1024 / 1024).toFixed(1)
    if (!window.confirm(t('settings.logs.confirmDelete', { id: entry.id, size }))) return
    if (!window.confirm(t('settings.logs.confirmDeleteLast', { id: entry.id }))) return
    await remove(entry.id, entry.size, entry.time_utc)
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <button
          onClick={() => void requestList()}
          disabled={!fcAlive || busy || consoleActive}
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
        <button
          onClick={() => void eraseAllLogs()}
          disabled={!fcAlive || busy || consoleActive || reconciliation || entries.length === 0}
          className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-error hover:bg-panel disabled:opacity-50"
        >
          {t('settings.logs.deleteAll')}
        </button>
        <span className="text-xs text-muted">{t('settings.logs.hint')}</span>
      </div>
       {error && <div role="status" className="break-words text-xs text-error">{error}</div>}
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
              <div className="flex flex-wrap items-center gap-2 text-xs">
                <span className="mono w-10 text-muted">#{entry.id}</span>
                <span className="mono w-32 text-muted">{date}</span>
                <span className="mono flex-1 text-ink">{(entry.size / 1024 / 1024).toFixed(1)} MB</span>
                {dl?.path ? (
                  <span className="text-ok">{t('settings.logs.saved')}</span>
                ) : (
                  <button
                    onClick={() => void download(entry.id, entry.size)}
                    disabled={!fcAlive || busy || consoleActive}
                    className="rounded border border-line bg-canvas px-2 py-0.5 text-[11px] text-ink hover:bg-panel disabled:opacity-50"
                  >
                    {t('settings.logs.download')}
                  </button>
                )}
                <button
                  onClick={() => void removeLog(entry)}
                  disabled={!fcAlive || busy || consoleActive || reconciliation}
                  className="rounded border border-line bg-canvas px-2 py-0.5 text-[11px] text-error hover:bg-panel disabled:opacity-50"
                >
                  {t('settings.logs.delete')}
                </button>
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

/** Height bounds for the resizable console output (px). */
const CONSOLE_MIN_H = 80
const CONSOLE_MAX_H = 600
const CONSOLE_H_KEY = 'maggcs.consoleHeight'

/** Persisted console height (px), falling back to the default. */
function loadConsoleHeight(): number {
  try {
    const saved = localStorage.getItem(CONSOLE_H_KEY)
    if (saved === null) return 160
    const v = Number(saved)
    if (Number.isFinite(v)) return Math.min(CONSOLE_MAX_H, Math.max(CONSOLE_MIN_H, v))
  } catch {
    /* storage unavailable */
  }
  return 160
}

/** MAVLink console (QGC-style): PX4 NuttShell over SERIAL_CONTROL, below
 * the FC-log list. Auto-scrolls; the output height is draggable (a grip
 * above the transcript, persisted); preset chips run the survey-relevant
 * commands; Enter sends. */
function ConsolePanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const transcript = useConsoleStore((s) => s.transcript)
  const busy = useConsoleStore((s) => s.busy)
  const send = useConsoleStore((s) => s.send)
  const startWatcher = useConsoleStore((s) => s.startWatcher)
  const nativeError = useConsoleStore((s) => s.error)
  const operation = useConsoleStore((s) => s.operation)
  const closeConsole = useConsoleStore((s) => s.close)
  const logsBusy = useLogsStore((s) => s.busy)
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [confirmation, setConfirmation] = useState<string | null>(null)
  const reviewedTarget = useRef<RuntimeTargetRef | null>(null)
  const [typed, setTyped] = useState('')
  const [height, setHeight] = useState(loadConsoleHeight)
  const outRef = useRef<HTMLDivElement | null>(null)
  const follow = useRef(true)
  const heightRef = useRef(height)

  useEffect(() => {
    startWatcher()
  }, [startWatcher])

  // Stick to the bottom unless the operator scrolled up.
  useEffect(() => {
    const el = outRef.current
    if (el && follow.current) el.scrollTop = el.scrollHeight
  }, [transcript, height])

  // Vertical drag on the grip: pointer capture keeps the gesture even when
  // the cursor leaves the grip; the delta inverts (drag up = taller).
  const onGripDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault()
    const grip = e.currentTarget
    grip.setPointerCapture(e.pointerId)
    const startY = e.clientY
    const startH = height
    const onMove = (ev: PointerEvent) => {
      const next = Math.min(CONSOLE_MAX_H, Math.max(CONSOLE_MIN_H, startH - (ev.clientY - startY)))
      heightRef.current = next
      setHeight(next)
    }
    const onUp = (ev: PointerEvent) => {
      grip.releasePointerCapture(ev.pointerId)
      grip.removeEventListener('pointermove', onMove)
      grip.removeEventListener('pointerup', onUp)
      try {
        localStorage.setItem(CONSOLE_H_KEY, String(heightRef.current))
      } catch {
        /* storage unavailable */
      }
    }
    grip.addEventListener('pointermove', onMove)
    grip.addEventListener('pointerup', onUp)
  }

  const fcAlive = link?.fc_alive === true

  const run = async (command: string) => {
    if (!isReadOnlyConsoleCommand(command)) {
      try {
        const context = await consoleContext()
        reviewedTarget.current = context.session.target
        setConfirmation(command)
        setTyped('')
      } catch (error) { setError(String(error)) }
      return
    }
    setError(null)
    try {
      await send(command)
    } catch (e) {
      setError(String(e))
    }
  }

  const runConfirmed = async () => {
    if (!confirmation || typed !== 'RUN') return
    const command = confirmation
    setConfirmation(null)
    setTyped('')
    setError(null)
    try { await send(command, true, reviewedTarget.current ?? undefined) } catch (e) { setError(String(e)) }
  }

  return (
    <div className="space-y-2">
      <div className="flex items-baseline justify-between">
        <span className="text-xs font-medium text-ink">{t('console.title')}</span>
        {operation && <button data-testid="console-close" onClick={() => void closeConsole()} className="touch-target text-xs text-warn">Close console</button>}
        <span className="text-[10px] text-muted">{t('console.hint')}</span>
      </div>
      <div
        onPointerDown={onGripDown}
        data-testid="console-grip"
        aria-label={t('console.resize')}
        className="flex h-2 cursor-ns-resize items-center justify-center rounded text-muted hover:bg-panel"
      >
        <span className="h-0.5 w-10 rounded bg-line" />
      </div>
      <div
        ref={outRef}
        data-testid="console-output"
        onScroll={() => { const el = outRef.current; if (el) follow.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 12 }}
        style={{ height: `${height}px` }}
        className="mono overflow-y-auto whitespace-pre-wrap break-words rounded border border-line bg-canvas p-2 text-[11px] leading-relaxed text-ink"
      >
        {transcript === '' ? <span className="text-muted">{t('console.empty')}</span> : transcript}
      </div>
      {confirmation && (
        <div data-testid="console-confirm" className="space-y-2 rounded border border-warn p-2 text-xs">
          <div className="text-warn">{t('console.confirmRisk')}</div>
          <code className="block break-all text-ink">{confirmation}</code>
          <input value={typed} onChange={(event) => setTyped(event.target.value)}
            aria-label={t('console.confirmKeyword')} placeholder="RUN"
            className="mono h-9 w-full rounded border border-line bg-canvas px-2 text-ink" />
          <div className="flex justify-end gap-2">
            <button onClick={() => setConfirmation(null)} className="h-8 rounded border border-line px-2">{t('console.cancel')}</button>
            <button data-testid="console-confirm-run" disabled={typed !== 'RUN'} onClick={() => void runConfirmed()}
              className="h-8 rounded border border-warn px-2 text-warn disabled:opacity-40">{t('console.send')}</button>
          </div>
        </div>
      )}
      <div className="flex flex-wrap gap-1">
        {CONSOLE_PRESETS.map((cmd) => (
          <button
            key={cmd}
            onClick={() => void run(cmd)}
            disabled={!fcAlive || busy || logsBusy}
            className="mono rounded border border-line bg-canvas px-1.5 py-0.5 text-[10px] text-muted hover:text-ink disabled:opacity-50"
          >
            {cmd}
          </button>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <input
          data-testid="console-command"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
             if (e.key !== 'Enter' || !fcAlive || busy || logsBusy) return
            const cmd = draft.trim()
            if (!cmd) return
            setDraft('')
            void run(cmd)
          }}
          placeholder={fcAlive ? t('console.placeholder') : t('console.noLink')}
           disabled={!fcAlive || logsBusy}
          className="mono h-9 flex-1 rounded border border-line bg-canvas px-2 text-sm text-ink disabled:opacity-50"
        />
        <button
          onClick={() => {
            const cmd = draft.trim()
            if (!cmd) return
            setDraft('')
            void run(cmd)
          }}
           disabled={!fcAlive || busy || logsBusy || draft.trim() === ''}
          className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel disabled:opacity-50"
        >
          {t('console.send')}
        </button>
      </div>
       {(error || nativeError) && <div className="break-words text-[11px] text-error">{error ?? nativeError}</div>}
    </div>
  )
}
