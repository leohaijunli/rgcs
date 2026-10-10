import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { disconnect } from '../desktop/connect'
import { useDataAge, useFieldAge } from '../hooks/useDataAge'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { fcStatusLabel } from '../util/linkLabel'
import { linkLevel, linkLevelTone } from '../util/linkLevel'

/** Toggle tone: a `button` that owns the arrow; the popover anchors below it. */
export default function LinkPopover({
  onOpenSettings,
}: {
  onOpenSettings: () => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const heartbeatAge = useFieldAge(
    useTelemetryStore((s) => s.snapshot?.field_ages.heartbeat_at_ms),
  )
  const packetAge = useDataAge(1000)

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

  const level = linkLevel(link, Number.isFinite(packetAge))
  const stateLabel = fcStatusLabel(t, link)
  const connected = link?.link_state === 'connected'
  const dotTone = dotClass(linkLevelTone(level))

  return (
    <div className="relative" ref={rootRef}>
      <button
        onClick={() => setOpen((o) => !o)}
        title={t('link.connectionDetails')}
        className="flex h-8 items-center gap-1 rounded border border-line bg-canvas px-2.5 text-xs hover:bg-panel"
      >
        <span className="text-muted">{t('link.fc')}</span>
        <span className={`h-1.5 w-1.5 rounded-full ${dotTone}`} />
        <span className="mono text-ink">{stateLabel}</span>
        <span className="text-[9px] text-muted">▾</span>
      </button>

      {open && (
        <div className="absolute right-0 top-full z-50 mt-1 w-64 rounded border border-line bg-panel p-3 shadow-2xl">
          <div className="mb-2 flex items-center justify-between">
            <span className="text-xs font-medium text-ink">{t('panels.vehicle')}</span>
            <span className={`mono text-[10px] ${connected ? 'text-ok' : 'text-error'}`}>
              {stateLabel}
            </span>
          </div>
          <dl className="space-y-1.5 text-xs">
            <Row
              k={t('link.systemId')}
              v={heartbeat ? `${heartbeat.system_id} / ${heartbeat.component_id}` : '—'}
            />
            <Row k={t('link.vehicleType')} v={heartbeat ? heartbeat.vehicle_type : '—'} />
            <Row k={t('link.autopilot')} v={heartbeat ? heartbeat.autopilot : '—'} />
            <Row
              k={t('link.heartbeatAge')}
              v={
                heartbeat
                  ? `${(heartbeatAge / 1000).toFixed(1)} s`
                  : t('link.never')
              }
            />
            <Row k={t('link.endpoint')} v={link ? link.endpoint : '—'} />
          </dl>
          <div className="mt-3 flex flex-col gap-1.5">
            {connected ? (
              <button
                onClick={() => void disconnect()}
                className="rounded border border-line bg-canvas px-2 py-1 text-xs text-ink hover:bg-panel"
              >
                {t('link.disconnect')}
              </button>
            ) : null}
            <button
              onClick={() => {
                setOpen(false)
                onOpenSettings()
              }}
              className="rounded border border-line bg-canvas px-2 py-1 text-xs text-ink hover:bg-panel"
            >
              {t('settings.tab.connection')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <span className="text-muted">{k}</span>
      <span className="mono truncate text-ink">{v}</span>
    </div>
  )
}

function dotClass(kind: 'ok' | 'warn' | 'err' | 'off'): string {
  switch (kind) {
    case 'ok':
      return 'bg-ok'
    case 'warn':
      return 'bg-warn'
    case 'err':
      return 'bg-error'
    default:
      return 'bg-muted'
  }
}