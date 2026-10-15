// Heartbeat-lost banner (plan §4 L3).
//
// Rust declares the FC dead after `DEFAULT_HEARTBEAT_TIMEOUT` (3 s) without a
// heartbeat and publishes `LinkStatus { fc_alive: false }`; from that moment
// every number on screen is history. This banner is the one place that says
// so: how long the link has been down (counting up, not a frozen zero), where
// the vehicle was last seen — position, height, battery — and a tone so the
// loss is noticed without watching the screen.
//
// It is deliberately *not* dismissible: the condition ends when the heartbeat
// comes back, and then a short "restored" notice confirms that mode, armed
// state and the mission were re-read (see `bridge.ts` / `mission.linkRestored`).

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { AlertTriangle, Volume2, VolumeX } from 'lucide-react'
import { beep } from '../audio/alarm'
import { useHeartbeatAge } from '../hooks/useDataAge'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'

/** How long the "link restored" notice stays after a reconnect. */
export const RESTORED_NOTICE_MS = 6000

/** Repeat interval of the loss tone while the FC is silent. */
export const LOSS_ALARM_REPEAT_MS = 5000

export default function HeartbeatBanner() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const restoredAtMs = useLinkStore((s) => s.restoredAtMs)
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const ageMs = useHeartbeatAge()
  const [soundOn, setSoundOn] = useState(true)
  const [restoredVisible, setRestoredVisible] = useState(false)

  // Exactly `LinkStatus::lost`: the transport is up but the FC went silent. An
  // intentionally disconnected link is not a lost heartbeat — the link
  // popover says "Disconnected" and this banner would sit there forever.
  const lost = link !== null && link.link_state === 'connected' && !link.fc_alive

  // The restored notice is time-boxed: the reconnect is an event, not a
  // state, so it expires on its own and the nav bar goes back to normal.
  useEffect(() => {
    if (restoredAtMs === null) return
    setRestoredVisible(true)
    const id = window.setTimeout(() => setRestoredVisible(false), RESTORED_NOTICE_MS)
    return () => window.clearTimeout(id)
  }, [restoredAtMs])

  // One beep when the loss starts, then a slow repeat: a persistent banner is
  // easy to stop seeing.
  useEffect(() => {
    if (!lost || !soundOn) return
    beep()
    const id = window.setInterval(beep, LOSS_ALARM_REPEAT_MS)
    return () => window.clearInterval(id)
  }, [lost, soundOn])

  if (!lost) {
    if (!restoredVisible) return null
    return (
      <div
        data-testid="link-restored"
        role="status"
        className="absolute left-1/2 top-3 z-30 -translate-x-1/2 whitespace-nowrap rounded border border-ok bg-panel/95 px-3 py-1.5 text-xs shadow-xl"
      >
        <span className="text-ok">●</span> <span className="text-ink">{t('link.restored')}</span>
      </div>
    )
  }

  const pos = snapshot?.global_position ?? null
  const sys = snapshot?.sys_status ?? null
  const ageText = Number.isFinite(ageMs)
    ? t('link.lostFor', { s: Math.max(0, Math.round(ageMs / 1000)) })
    : t('link.lostForUnknown')

  return (
    <div
      data-testid="heartbeat-banner"
      data-lost-ms={Number.isFinite(ageMs) ? Math.round(ageMs) : ''}
      role="alert"
      aria-live="assertive"
      className="absolute left-1/2 top-3 z-30 flex -translate-x-1/2 items-center gap-3 whitespace-nowrap rounded border border-warn bg-panel/95 px-3 py-2 text-sm shadow-xl"
    >
      <AlertTriangle size={16} className="shrink-0 text-warn" />
      <span className="font-medium text-warn">{t('link.lost')}</span>
      <span data-testid="heartbeat-age" className="mono text-xs text-ink">
        {ageText}
      </span>
      <span data-testid="heartbeat-last" className="text-xs text-muted">
        {t('link.lastKnown')}:{' '}
        {pos
          ? `${pos.latitude_deg.toFixed(5)}, ${pos.longitude_deg.toFixed(5)} · ${pos.altitude.meters.toFixed(1)} m AMSL`
          : '—'}
        {sys ? ` · ${sys.battery_remaining_percent}%` : ''}
      </span>
      <button
        data-testid="heartbeat-sound"
        aria-pressed={soundOn}
        title={soundOn ? t('link.soundOn') : t('link.soundOff')}
        onClick={() => setSoundOn((v) => !v)}
        className="touch-target flex h-7 items-center gap-1 rounded border border-line px-2 text-xs text-ink hover:bg-canvas"
      >
        {soundOn ? <Volume2 size={13} /> : <VolumeX size={13} />}
        {soundOn ? t('link.soundOn') : t('link.soundOff')}
      </button>
    </div>
  )
}
