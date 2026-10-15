// The shared alert layer (plan §7 U3): one component every window mounts, so
// a Critical alert is seen wherever the operator is looking — the main window,
// the Inspector, the Actuator or the Params window.
//
// It owns four surfaces over the A3 store:
//   • the full-screen red flash — a short burst when a new Critical arrives
//     (never on a latched Critical reappearing at boot), skipped under
//     `prefers-reduced-motion`
//   • the top Critical banner, latched until the operator acknowledges it
//   • non-blocking Warning toasts that expire on their own
//   • a per-severity alarm tone, muted by the time-boxed mute in `alertSound`
//
// The `alert` + `statustext` stream is wired here too (`useAlertStream`), so
// mounting the layer is all a window has to do.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, X } from 'lucide-react'
import '../i18n'
import { useAlerts, useAlertStream } from '../stores/alerts'
import { alertBeep } from '../audio/alarm'
import { prefersReducedMotion, useAlertSound } from '../stores/alertSound'
import type { Alert } from '../generated-types/Alert'

/** A Warning toast lives this long, then folds away; the alert itself stays
 * in the drawer until acknowledged. */
export const TOAST_MS = 8000
/** How long the red flash burst lasts. */
export const FLASH_MS = 1000

function formatTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString()
}

/**
 * The alerts that appeared since the previous render, ignoring the first
 * snapshot (a latched Critical present at boot must not re-flash the screen or
 * re-sound the alarm just because a window reloaded).
 */
function useNewAlerts(): Alert[] {
  const visible = useAlerts((s) => s.visible)
  const known = useRef<Set<string> | null>(null)
  const [added, setAdded] = useState<Alert[]>([])
  useEffect(() => {
    const ids = new Set(visible.map((a) => a.id))
    if (known.current === null) {
      known.current = ids
      return
    }
    const fresh = visible.filter((a) => !known.current?.has(a.id))
    known.current = ids
    if (fresh.length > 0) setAdded(fresh)
  }, [visible])
  return added
}

/** The red flash itself: keyed on the token so each new Critical restarts the
 * animation, and it unmounts when the burst is over. */
function AlertFlash({ token }: { token: number }) {
  const [shown, setShown] = useState(false)
  useEffect(() => {
    if (token === 0 || prefersReducedMotion()) return
    setShown(true)
    const id = window.setTimeout(() => setShown(false), FLASH_MS)
    return () => window.clearTimeout(id)
  }, [token])
  if (!shown) return null
  return (
    <div
      key={token}
      data-testid="alert-flash"
      aria-hidden="true"
      className="pointer-events-none fixed inset-0 z-[60] mg-alert-flash"
    />
  )
}

interface Toast {
  key: number
  alert: Alert
}

export default function AlertLayer() {
  useAlertStream()
  const { t } = useTranslation()
  const added = useNewAlerts()
  const banner = useAlerts((s) => s.banner)
  const ack = useAlerts((s) => s.ack)
  const [flashToken, setFlashToken] = useState(0)
  const [toasts, setToasts] = useState<Toast[]>([])
  const toastSeq = useRef(0)

  // One reaction per batch of newly-seen alerts: flash on Critical, sound the
  // worst severity, and queue a toast for each Warning.
  useEffect(() => {
    if (added.length === 0) return
    const worst = added.some((a) => a.severity === 'critical')
      ? 'critical'
      : added.some((a) => a.severity === 'warn')
        ? 'warn'
        : 'info'
    if (worst === 'critical') setFlashToken((n) => n + 1)
    const { volume, mutedUntil } = useAlertSound.getState()
    if (mutedUntil <= Date.now()) alertBeep(worst, volume)
    const warns = added.filter((a) => a.severity === 'warn')
    if (warns.length > 0) {
      setToasts((cur) => [
        ...cur,
        ...warns.map((a) => ({ key: (toastSeq.current += 1), alert: a })),
      ])
    }
  }, [added])

  // Toasts expire on their own.
  useEffect(() => {
    if (toasts.length === 0) return
    const timers = toasts.map((toast) =>
      window.setTimeout(
        () => setToasts((cur) => cur.filter((x) => x.key !== toast.key)),
        TOAST_MS,
      ),
    )
    return () => timers.forEach((id) => window.clearTimeout(id))
  }, [toasts])

  return (
    <>
      <AlertFlash token={flashToken} />

      {banner !== null && (
        <div
          data-testid="alert-banner"
          role="alert"
          className="absolute left-1/2 top-3 z-40 flex max-w-[80%] -translate-x-1/2 items-center gap-3 rounded border border-error bg-panel/95 px-3 py-2 text-sm shadow-xl"
        >
          <AlertTriangle size={16} className="shrink-0 text-error" />
          <span className="truncate font-medium text-ink">{banner.text}</span>
          {banner.count > 1 && (
            <span className="mono shrink-0 text-[10px] text-muted">×{banner.count}</span>
          )}
          <span className="shrink-0 text-[10px] text-muted">
            {t(`alerts.source.${banner.source}`)} · {formatTime(banner.last_ts_ms)}
          </span>
          <button
            data-testid="alert-banner-ack"
            onClick={() => ack(banner.id)}
            className="touch-target flex h-7 shrink-0 items-center rounded border border-line px-2 text-xs text-ink hover:bg-canvas"
          >
            {t('alerts.ack')}
          </button>
        </div>
      )}

      {toasts.length > 0 && (
        <div
          data-testid="alert-toasts"
          className="pointer-events-none absolute right-3 top-3 z-40 flex w-80 flex-col gap-2"
        >
          {toasts.map((toast) => (
            <div
              key={toast.key}
              data-testid="alert-toast"
              data-severity={toast.alert.severity}
              role="status"
              className="pointer-events-auto flex items-start gap-2 rounded border border-warn bg-panel/95 px-3 py-2 text-sm shadow-xl"
            >
              <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warn" />
              <span className="flex-1">
                <span className="text-ink">{toast.alert.text}</span>
                <span className="mt-0.5 block text-[10px] text-muted">
                  {t(`alerts.source.${toast.alert.source}`)} · {formatTime(toast.alert.last_ts_ms)}
                </span>
              </span>
              <button
                data-testid="alert-toast-ack"
                onClick={() => {
                  ack(toast.alert.id)
                  setToasts((cur) => cur.filter((x) => x.key !== toast.key))
                }}
                className="touch-target flex h-6 items-center rounded border border-line px-1.5 text-[11px] text-ink hover:bg-canvas"
              >
                {t('alerts.ack')}
              </button>
              <button
                data-testid="alert-toast-dismiss"
                aria-label={t('alerts.dismiss')}
                title={t('alerts.dismiss')}
                onClick={() => setToasts((cur) => cur.filter((x) => x.key !== toast.key))}
                className="touch-target flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={12} />
              </button>
            </div>
          ))}
        </div>
      )}

      {/* Screen-reader announcement of the latest latched alert. */}
      <div data-testid="alert-live" aria-live="assertive" role="status" className="sr-only">
        {banner !== null ? `${t('alerts.severity.critical')}: ${banner.text}` : ''}
      </div>
    </>
  )
}
