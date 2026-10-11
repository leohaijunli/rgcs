// MODE ▾ popover (MS3, ADR-017): lists the switchable PX4 flight modes with
// their availability (resolved in core via `flight_mode_table`), sends
// `DO_SET_MODE` through the command service and confirms via the heartbeat
// readback — never optimistically.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import type { ModeAvailability } from '../generated-types/ModeAvailability'
import { useCommandStore } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { modeLabel } from '../util/modeLabel'

/** How long a confirm-required entry stays armed for the second click. */
const CONFIRM_WINDOW_MS = 5000
/** Readback window: the heartbeat must show the new mode within this. */
const READBACK_TIMEOUT_MS = 3000
/** localStorage key for the "advanced modes" preference (Manual/Acro/Offboard). */
const ADVANCED_KEY = 'maggcs.advancedModes'

/** PX4 custom_mode bit packing (ADR-017): main 16–23, sub 24–31. */
function packCustomMode(main: number, sub: number): number {
  return ((main & 0xff) << 16) | ((sub & 0xff) << 24)
}

interface Switching {
  id: string
  target: number
  sentAt: number
}

export default function ModePopover() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const [table, setTable] = useState<ModeAvailability[] | null>(null)
  const [tableError, setTableError] = useState<string | null>(null)
  const [confirming, setConfirming] = useState<string | null>(null)
  const [switching, setSwitching] = useState<Switching | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const [advanced, setAdvanced] = useState(
    () => localStorage.getItem(ADVANCED_KEY) === '1',
  )
  const confirmTimer = useRef<number | undefined>(undefined)
  const statusTimer = useRef<number | undefined>(undefined)

  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const pending = useCommandStore((s) => s.pending)
  const lastEvent = useCommandStore((s) => s.lastEvent)
  const sendSetMode = useCommandStore((s) => s.sendSetMode)
  const waypointCount = useMissionStore((s) => s.waypoints.length)
  const planDirty = useMissionStore((s) => s.dirty)
  const syncedHash = useMissionStore((s) => s.lastSyncedHash)

  const currentLabel = modeLabel(t, heartbeat)

  // Close on outside click / Esc.
  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    window.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  useEffect(
    () => () => {
      window.clearTimeout(confirmTimer.current)
      window.clearTimeout(statusTimer.current)
    },
    [],
  )

  // Resolve the table against the vehicle state whenever the popover opens.
  useEffect(() => {
    if (!open) return
    // The context is composed from the live stores; the availability rules
    // run in core (ADR-017). home_valid proxies to armed (PX4 publishes
    // home at arming; a dedicated HOME_POSITION feed is a follow-up).
    const context = {
      link_online: link?.link_state === 'connected',
      armed: heartbeat?.base_mode.safety_armed === true,
      mission_onboard: waypointCount > 0,
      mission_synced: !planDirty && syncedHash != null,
      home_valid: heartbeat?.base_mode.safety_armed === true,
      rc_available: heartbeat?.base_mode.manual_input_enabled === true,
      motor_test_running: false, // window-local; the test gates itself
    }
    invoke<ModeAvailability[]>('flight_mode_table', { context })
      .then((entries) => {
        setTable(entries)
        setTableError(null)
      })
      .catch((e) => setTableError(String(e)))
  }, [open, link, heartbeat, waypointCount, planDirty, syncedHash])

  const showStatus = useCallback((text: string) => {
    setStatus(text)
    window.clearTimeout(statusTimer.current)
    statusTimer.current = window.setTimeout(() => setStatus(null), 4000)
  }, [])

  // Readback confirmation (ADR-017): the heartbeat custom_mode is the truth.
  const customMode = heartbeat?.custom_mode ?? null
  useEffect(() => {
    if (!switching) return
    if (customMode !== null && customMode === switching.target) {
      showStatus('')
      setSwitching(null)
      return
    }
    // Terminal ack for the mode switch without a readback yet: an accepted
    // ack keeps waiting for the readback; anything else ends the wait.
    if (lastEvent?.command === 'set_mode' && lastEvent.kind !== 'sent') {
      if (lastEvent.kind === 'completed' && lastEvent.result === 'accepted') {
        return // accepted — wait for the readback window
      }
      setSwitching(null)
      showStatus(
        lastEvent.result
          ? t(`plan.cmd.${ackLabel(lastEvent.result)}`)
          : String(lastEvent.message ?? ''),
      )
    }
  }, [switching, customMode, lastEvent, showStatus, t])

  // Readback window expiry: the ack may say accepted but the mode never
  // changed — say so instead of guessing (no optimistic display).
  useEffect(() => {
    if (!switching) return
    const elapsed = Date.now() - switching.sentAt
    const timer = window.setTimeout(
      () => {
        setSwitching(null)
        showStatus(t('modePopover.noReadback'))
      },
      Math.max(0, READBACK_TIMEOUT_MS - elapsed),
    )
    return () => window.clearTimeout(timer)
  }, [switching, showStatus, t])

  const press = (entry: ModeAvailability) => {
    const { option } = entry
    if (pending === 'set_mode') return
    if (option.confirm && confirming !== option.id) {
      setConfirming(option.id)
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(
        () => setConfirming(null),
        CONFIRM_WINDOW_MS,
      )
      return
    }
    setConfirming(null)
    void sendSetMode(option.main_mode, option.sub_mode)
    showStatus('')
    setSwitching({
      id: option.id,
      target: packCustomMode(option.main_mode, option.sub_mode),
      sentAt: Date.now(),
    })
  }

  const toggleAdvanced = () => {
    setAdvanced((a) => {
      const next = !a
      localStorage.setItem(ADVANCED_KEY, next ? '1' : '0')
      return next
    })
  }

  const entries = (table ?? []).filter((e) => advanced || !e.option.advanced)
  const busy = pending === 'set_mode' || switching != null

  return (
    <div className="relative" ref={rootRef}>
      <button
        onClick={() => setOpen((o) => !o)}
        title={t('modePopover.title')}
        className="flex h-8 items-center gap-1 rounded border border-line bg-canvas px-2.5 text-xs hover:bg-panel"
      >
        <span className="text-muted">{t('link.mode')}</span>
        <span className="mono text-ink">{currentLabel ?? '—'}</span>
        <span className="text-[9px] text-muted">▾</span>
      </button>

      {open && (
        <div className="absolute right-0 top-full z-50 mt-1 w-56 rounded border border-line bg-panel p-3 shadow-2xl">
          <div className="mb-2 flex items-baseline justify-between">
            <span className="text-xs font-medium text-ink">
              {t('modePopover.title')}
            </span>
            <span className="mono text-[10px] text-muted">{currentLabel ?? '—'}</span>
          </div>

          <div className="flex flex-col gap-1">
            {tableError && (
              <span className="text-[11px] text-warn">{tableError}</span>
            )}
            {!table && !tableError && (
              <span className="text-[11px] text-muted">{t('modePopover.loading')}</span>
            )}
            {entries.map((entry) => {
              const isCurrent =
                customMode ===
                packCustomMode(entry.option.main_mode, entry.option.sub_mode)
              const disabled = !entry.enabled || busy
              return (
                <button
                  key={entry.option.id}
                  onClick={() => !disabled && press(entry)}
                  disabled={disabled}
                  title={entry.reason ?? undefined}
                  className={`flex items-center justify-between rounded border px-2 py-1 text-xs ${
                    disabled
                      ? 'cursor-not-allowed border-transparent bg-transparent text-muted opacity-70'
                      : 'border-line bg-canvas text-ink hover:bg-panel'
                  } ${isCurrent ? 'border-accent' : ''}`}
                >
                  <span>{entry.option.label}</span>
                  <span className="mono text-[9px] text-muted">
                    {isCurrent
                      ? t('modePopover.current')
                      : confirming === entry.option.id
                        ? t('modePopover.confirm')
                        : (entry.reason ?? '')}
                  </span>
                </button>
              )
            })}
          </div>

          {busy && (
            <div className="mt-2 text-[11px] text-muted">{t('modePopover.switching')}</div>
          )}
          {status && status !== '' && (
            <div className="mt-2 text-[11px] text-warn">{status}</div>
          )}

          <label className="mt-3 flex cursor-pointer items-center gap-2 text-[11px] text-muted">
            <input type="checkbox" checked={advanced} onChange={toggleAdvanced} />
            {t('modePopover.advanced')}
          </label>
        </div>
      )}
    </div>
  )
}

/** Terminal ack results the popover surfaces inline (shared vocabulary with
 * the flight commands). */
function ackLabel(result: string): string {
  const map: Record<string, string> = {
    accepted: 'accepted',
    denied: 'denied',
    unsupported: 'unsupported',
    temporarily_rejected: 'rejected',
    failed: 'rejected',
  }
  return map[result] ?? 'unknown'
}
