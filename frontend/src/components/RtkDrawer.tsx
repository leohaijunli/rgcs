// RTK GPS drawer (R4, operator request: QGroundControl-style side drawer):
// base-station port selection + injection start/stop, injection health
// (source, B/s, drops, stall alarm) and the rover's current fix — the
// everything-RTK surface reachable from the TopBar RTK pill.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useDevicesStore } from '../stores/devices'
import { startRtkWatcher, useRtkStore } from '../stores/rtk'
import { useTelemetryStore } from '../stores/telemetry'
import { useLinkStore } from '../stores/link'

/** Baud rates the F9P base link can run at. */
const BAUD_RATES = [38_400, 115_200, 230_400, 460_800, 921_600]

export default function RtkDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation()
  const devices = useDevicesStore((s) => s.devices)
  const loadDevices = useDevicesStore((s) => s.load)
  const stats = useRtkStore((s) => s.stats)
  const foreign = useRtkStore((s) => s.foreign)
  const busy = useRtkStore((s) => s.busy)
  const startSerial = useRtkStore((s) => s.startSerial)
  const stop = useRtkStore((s) => s.stop)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)
  const link = useLinkStore((s) => s.link)

  const [port, setPort] = useState('')
  const [baud, setBaud] = useState(38_400)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      startRtkWatcher()
      void loadDevices()
    }
  }, [open, loadDevices])

  // Default the port to the first pluggable device once the list arrives.
  useEffect(() => {
    if (!port && devices.length > 0) setPort(devices[0].port_name)
  }, [devices, port])

  if (!open) return null

  const injecting = stats?.source != null

  const start = async () => {
    setError(null)
    if (!port) {
      setError(t('rtk.noPort'))
      return
    }
    const err = await startSerial(port, baud)
    if (err) setError(err)
  }

  const stopAll = async () => {
    setError(null)
    await stop()
  }

  return (
    <div className="fixed inset-0 z-40">
      {/* Backdrop click closes, like QGC's drawer. */}
      <div className="absolute inset-0 bg-black/40" onClick={onClose} />
      <aside
        className="absolute right-0 top-0 flex h-full w-[340px] flex-col border-l border-line bg-panel shadow-2xl"
        role="dialog"
        aria-label={t('rtk.title')}
      >
        <div className="flex items-center justify-between border-b border-line px-4 py-3">
          <h2 className="text-sm font-medium text-ink">{t('rtk.title')}</h2>
          <button
            onClick={onClose}
            aria-label="close"
            className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
          >
            ×
          </button>
        </div>

        <div className="flex-1 space-y-4 overflow-y-auto p-4">
          {/* Rover status */}
          <section>
            <h3 className="mb-1 text-[11px] uppercase tracking-wide text-muted">
              {t('rtk.rover')}
            </h3>
            <div className="rounded border border-line bg-canvas p-2 text-xs">
              <Row
                k={t('link.fix')}
                v={gps ? t(`hud.fix.${fixKey(gps.fix_type)}`) : t('link.never')}
                tone={
                  gps?.fix_type === 'RTK_FIXED'
                    ? 'text-ok'
                    : gps?.fix_type === 'RTK_FLOAT'
                      ? 'text-warn'
                      : 'text-muted'
                }
              />
              <Row
                k={t('rtk.satellites')}
                v={gps ? String(gps.satellites_visible) : '—'}
              />
            </div>
          </section>

          {/* Base station / injection source */}
          <section>
            <h3 className="mb-1 text-[11px] uppercase tracking-wide text-muted">
              {t('rtk.base')}
            </h3>
            <div className="space-y-2 rounded border border-line bg-canvas p-2">
              <label className="block text-xs text-muted">
                {t('rtk.port')}
                <select
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  disabled={injecting || busy || devices.length === 0}
                  className="mt-1 w-full rounded border border-line bg-canvas px-2 py-1 text-xs text-ink"
                >
                  {devices.length === 0 && <option value="">{t('rtk.noDevices')}</option>}
                  {devices.map((d) => (
                    <option key={d.port_name} value={d.port_name}>
                      {d.port_name}
                      {d.product ? ` — ${d.product}` : ''}
                    </option>
                  ))}
                </select>
              </label>
              <label className="block text-xs text-muted">
                {t('rtk.baud')}
                <select
                  value={baud}
                  onChange={(e) => setBaud(Number(e.target.value))}
                  disabled={injecting || busy}
                  className="mt-1 w-full rounded border border-line bg-canvas px-2 py-1 text-xs text-ink"
                >
                  {BAUD_RATES.map((b) => (
                    <option key={b} value={b}>
                      {b}
                    </option>
                  ))}
                </select>
              </label>
              <div className="flex gap-2">
                {injecting ? (
                  <button
                    onClick={() => void stopAll()}
                    disabled={busy || !link?.fc_alive}
                    className="flex-1 rounded border border-line bg-canvas px-2 py-1.5 text-xs font-medium text-ink hover:bg-panel disabled:opacity-50"
                  >
                    {t('rtk.stop')}
                  </button>
                ) : (
                  <button
                    onClick={() => void start()}
                    disabled={busy || !link?.fc_alive || devices.length === 0}
                    className="flex-1 rounded border border-accent bg-accent px-2 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-50"
                  >
                    {t('rtk.start')}
                  </button>
                )}
              </div>
              {error && <div className="text-[11px] text-error">{error}</div>}
            </div>
          </section>

          {/* Injection health (1 Hz stats) */}
          <section>
            <h3 className="mb-1 text-[11px] uppercase tracking-wide text-muted">
              {t('rtk.injection')}
            </h3>
            <div className="rounded border border-line bg-canvas p-2 text-xs">
              {injecting && stats ? (
                <>
                  <Row k={t('rtk.source')} v={stats.source ?? '—'} />
                  <Row k={t('rtk.bytesPerSecond')} v={stats.bytes_per_second.toFixed(0)} />
                  <Row k={t('rtk.totalFrames')} v={String(stats.total_frames)} />
                  <Row
                    k={t('rtk.dropped')}
                    v={String(stats.dropped_chunks)}
                    tone={stats.dropped_chunks > 0 ? 'text-warn' : undefined}
                  />
                  {Object.keys(stats.frames_per_type).length > 0 && (
                    <Row
                      k={t('rtk.types')}
                      v={Object.entries(stats.frames_per_type)
                        .map(([type, n]) => `${type}×${n}`)
                        .join(' ')}
                    />
                  )}
                  {stats.stalled && (
                    <div className="mt-1 text-warn">{t('rtk.stalled')}</div>
                  )}
                </>
              ) : (
                <span className="text-muted">{t('rtk.injectionOff')}</span>
              )}
            </div>
            {/* N4/ADR-008: somebody else (QGC) is injecting on this link too. */}
            {foreign?.active && (
              <div className="mt-1 rounded border border-warn bg-canvas px-2 py-1 text-[11px] text-warn">
                {t('rtk.foreignActive')} ({String(foreign.packets)})
              </div>
            )}
          </section>

          <p className="text-[11px] leading-relaxed text-muted">{t('rtk.hint')}</p>
        </div>
      </aside>
    </div>
  )
}

function Row({ k, v, tone }: { k: string; v: string; tone?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-0.5">
      <span className="text-muted">{k}</span>
      <span className={`mono truncate ${tone ?? 'text-ink'}`}>{v}</span>
    </div>
  )
}

function fixKey(fix: string): string {
  const map: Record<string, string> = {
    NO_GPS: 'noGps',
    NO_FIX: 'noFix',
    FIX2D: 'fix2d',
    FIX3D: 'fix3d',
    DGPS: 'dgps',
    RTK_FLOAT: 'rtkFloat',
    RTK_FIXED: 'rtkFixed',
    STATIC: 'static',
    PPP: 'ppp',
  }
  return map[fix] ?? 'noGps'
}
