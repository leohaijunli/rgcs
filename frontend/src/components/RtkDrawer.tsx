// QGC-style receiver setup, Survey-In progress and independent rover/injection state.
import { useEffect, useState, type FormEvent } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import type { RtkBaseMode as BaseMode } from '../generated-types/RtkBaseMode'
import type { SerialDeviceInfo } from '../generated-types/SerialDeviceInfo'
import type { SerialPortRef } from '../generated-types/SerialPortRef'
import { useDevicesStore } from '../stores/devices'
import { baseRunning, startRtkWatcher, SURVEY_DEFAULTS, SURVEY_LIMITS, useRtkStore } from '../stores/rtk'
import { useTelemetryStore } from '../stores/telemetry'
import { useLinkStore } from '../stores/link'
import { FIELD_STALE_MS, useFieldAge } from '../hooks/useDataAge'
import SerialAutoControls from './SerialAutoControls'
import RtkGpsDashboard from './RtkGpsDashboard'
import { useSerialAutoStore } from '../stores/serialAuto'
import { RTK_SETTINGS_KEY } from '../util/rtkSettings'

const BAUD_RATES = [0, 9_600, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600]
const STALE_CORRECTIONS_S = 5
const SETTINGS_KEY = RTK_SETTINGS_KEY
const CONTROL = 'mt-1 min-h-8 w-full touch-target rounded border border-line bg-canvas px-2 py-1 text-xs text-ink disabled:opacity-60'
type Settings = { kind: BaseMode['kind']; accuracy: string; duration: string; baud: number }

function savedSettings(): Settings {
  const fallback: Settings = { kind: 'survey_in', accuracy: String(SURVEY_DEFAULTS.accuracy_limit_m), duration: String(SURVEY_DEFAULTS.min_duration_s), baud: 0 }
  try {
    const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? 'null') as Partial<Settings> | null
    if (!saved || !['survey_in', 'preconfigured'].includes(saved.kind ?? '')) return fallback
    return { ...fallback, kind: saved.kind ?? fallback.kind,
      accuracy: typeof saved.accuracy === 'string' ? saved.accuracy : fallback.accuracy,
      duration: typeof saved.duration === 'string' ? saved.duration : fallback.duration,
      baud: BAUD_RATES.includes(saved.baud ?? -1) ? saved.baud! : fallback.baud }
  } catch { return fallback }
}

function sameDevice(a: SerialDeviceInfo, b: SerialDeviceInfo): boolean {
  return a.port_name === b.port_name && a.serial_number === b.serial_number
    && a.device_id?.vendor_id === b.device_id?.vendor_id && a.device_id?.product_id === b.device_id?.product_id
    && a.manufacturer === b.manufacturer && a.product === b.product
}

export default function RtkDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation()
  const devices = useDevicesStore((s) => s.devices)
  const base = useRtkStore((s) => s.base)
  const baseAt = useRtkStore((s) => s.baseReceivedAtMs)
  const stats = useRtkStore((s) => s.stats)
  const foreign = useRtkStore((s) => s.foreign)
  const ports = useRtkStore((s) => s.ports)
  const portsError = useRtkStore((s) => s.portsError)
  const loadPorts = useRtkStore((s) => s.loadPorts)
  const busy = useRtkStore((s) => s.busy)
  const startSerial = useRtkStore((s) => s.startSerial)
  const stop = useRtkStore((s) => s.stop)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)
  const gpsAt = useTelemetryStore((s) => s.snapshot?.field_ages?.gps_at_ms)
  const gpsAge = useFieldAge(gpsAt)
  const baseAge = useFieldAge(baseAt ?? undefined)
  const fcSession = useSerialAutoStore((s) => s.snapshot?.fc.session)
  const hudScope = `${base?.session_id ?? 'none'}:${fcSession?.kind === 'fc' ? `${fcSession.connection_id}:${fcSession.generation}` : 'offline'}`
  const link = useLinkStore((s) => s.link)
  const [settings, setSettings] = useState(savedSettings)
  const [selected, setSelected] = useState<SerialDeviceInfo | null>(null)
  const [selectedRef, setSelectedRef] = useState<SerialPortRef | null>(null)
  const [suggested, setSuggested] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const running = baseRunning(base?.phase)

  useEffect(() => { if (open) { startRtkWatcher(); void loadPorts() } }, [open, loadPorts, devices, link?.endpoint, link?.link_state])
  useEffect(() => {
    if (running) return
    if (selected && !ports.some((p) => sameDevice(p.device, selected) && !p.unavailable_reason
      && (!selectedRef || p.port_ref?.instance_id === selectedRef.instance_id))) {
      setSelected(null)
      setSelectedRef(null)
      setError(t('rtk.deviceChanged'))
      return
    }
    if (!selected && !suggested) {
      const candidates = ports.filter((p) => p.suggested_role === 'rtk_base_station' && !p.unavailable_reason)
      if (candidates.length === 1) { setSelected(candidates[0].device); setSelectedRef(candidates[0].port_ref ?? null); setSuggested(true) }
    }
  }, [ports, running, selected, selectedRef, suggested, t])
  useEffect(() => {
    if (!running || !base?.mode) return
    const mode = base.mode
    setSettings((s) => ({ ...s, kind: mode.kind, baud: base.serial?.baud_rate ?? s.baud,
      ...(mode.kind === 'survey_in' ? { accuracy: String(mode.config.accuracy_limit_m), duration: String(mode.config.min_duration_s) } : {}) }))
  }, [base?.session_id, running, base?.mode, base?.serial?.baud_rate])

  const kind = running && base?.mode ? base.mode.kind : settings.kind
  const accuracy = Number(settings.accuracy)
  const duration = Number(settings.duration)
  const invalidAccuracy = !Number.isFinite(accuracy) || accuracy < SURVEY_LIMITS.minAccuracy || accuracy > SURVEY_LIMITS.maxAccuracy
  const invalidDuration = !Number.isInteger(duration) || duration < SURVEY_LIMITS.minDuration || duration > SURVEY_LIMITS.maxDuration
  const invalid = kind === 'survey_in' && (invalidAccuracy || invalidDuration)
  const survey = base?.survey_in
  const baseStale = running && baseAge > FIELD_STALE_MS
  const hasLink = link?.link_state === 'connected'
  const dataAge = base?.seconds_since_data == null ? null : base.seconds_since_data + baseAge / 1000
  const rtcmAge = base?.seconds_since_rtcm == null ? null : base.seconds_since_rtcm + baseAge / 1000
  const displayedError = error ?? (base?.phase === 'failed' ? base.error : null) ?? portsError
  const selectedPort = ports.find((p) => selected && sameDevice(p.device, selected))
  const patchSettings = (patch: Partial<Settings>) => { setSettings((s) => ({ ...s, ...patch })); setError(null) }

  const start = async (event: FormEvent) => {
    event.preventDefault()
    setError(null)
    if (!selected || !selectedPort || selectedPort.unavailable_reason) { setError(t('rtk.noPort')); return }
    if (invalid) return
    const mode: BaseMode = kind === 'survey_in'
      ? { kind, config: { accuracy_limit_m: accuracy, min_duration_s: duration } } : { kind }
    try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings)) } catch { /* Preferences can be unavailable. */ }
    setError(await startSerial(selected, settings.baud, mode, selectedRef))
  }

  return (
    <Dialog.Root open={open} onOpenChange={(next) => { if (!next) onClose() }}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <Dialog.Content aria-describedby="rtk-description" className="fixed right-0 top-0 z-50 flex h-full w-[380px] max-w-full flex-col border-l border-line bg-panel shadow-2xl">
          <div className="flex shrink-0 items-center justify-between border-b border-line px-4 py-3">
            <Dialog.Title className="text-sm font-medium text-ink">{t('rtk.title')}</Dialog.Title>
            <Dialog.Close aria-label="Close RTK GPS" className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink">×</Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
            <RtkGpsDashboard key={hudScope} base={base} stats={stats} gps={gps} baseAgeMs={baseAge} roverAgeMs={gpsAge} />
            <SerialAutoControls role="rtk_base" />
            <section>
              <Heading>{t('rtk.rover')}</Heading>
              <div className="rounded border border-line bg-canvas p-2 text-xs">
                <Row k={t('link.fix')} v={gps ? t(`hud.fix.${fixKey(gps.fix_type)}`) : t('link.never')}
                  tone={gpsAge > FIELD_STALE_MS ? 'text-muted' : gps?.fix_type === 'RTK_FIXED' ? 'text-ok' : 'text-muted'} />
                <Row k={t('rtk.satellites')} v={gps ? String(gps.satellites_visible) : '—'} />
                {gps && gpsAge > FIELD_STALE_MS && <p className="mt-1 text-warn">{t('rtk.roverStale')}</p>}
              </div>
            </section>

            <form id="rtk-base-form" onSubmit={(e) => void start(e)} noValidate>
              <Heading>{t('rtk.base')}</Heading>
              <div className="space-y-3 rounded border border-line bg-canvas p-3">
                <label className="block text-xs text-muted">{t('rtk.port')}
                  <select aria-label={t('rtk.port')} className={CONTROL} value={running ? base?.serial?.port_name ?? selected?.port_name ?? '' : selected?.port_name ?? ''}
                    disabled={running || busy != null} onChange={(e) => { const port = ports.find((p) => p.device.port_name === e.target.value); setSelected(port?.device ?? null); setSelectedRef(port?.port_ref ?? null); setSuggested(true); setError(null) }}>
                    <option value="">{t(ports.length ? 'rtk.choosePort' : 'rtk.noDevices')}</option>
                    {running && base?.serial && !ports.some((p) => p.device.port_name === base.serial?.port_name) && <option value={base.serial.port_name}>{base.serial.port_name}</option>}
                    {ports.map((p) => <option key={p.device.port_name} value={p.device.port_name} disabled={p.unavailable_reason != null}>
                      {p.device.port_name}{p.device.product ? ` — ${p.device.product}` : ''} · {p.unavailable_reason ?? t(`rtk.deviceRole.${p.suggested_role}`)}
                    </option>)}
                  </select>
                </label>
                <label className="block text-xs text-muted">{t('rtk.baseMode')}
                  <select aria-label={t('rtk.baseMode')} className={CONTROL} value={kind} disabled={running || busy != null}
                    onChange={(e) => patchSettings({ kind: e.target.value as BaseMode['kind'], baud: e.target.value === 'preconfigured' && settings.baud === 0 ? 115_200 : settings.baud })}>
                    <option value="survey_in">{t('rtk.surveyMode')}</option>
                    <option value="preconfigured">{t('rtk.preconfiguredMode')}</option>
                  </select>
                </label>
                <label className="block text-xs text-muted">{t('rtk.baud')}
                  <select aria-label={t('rtk.baud')} className={CONTROL} value={settings.baud} disabled={running || busy != null} onChange={(e) => patchSettings({ baud: Number(e.target.value) })}>
                    {BAUD_RATES.filter((b) => b !== 0 || kind === 'survey_in').map((baud) => <option key={baud} value={baud}>{baud === 0 ? t('rtk.autoBaud') : baud}</option>)}
                  </select>
                </label>
                {kind === 'survey_in' && <>
                  <label className="block text-xs text-muted">{t('rtk.accuracy')}
                    <input className={CONTROL} type="number" inputMode="decimal" min={SURVEY_LIMITS.minAccuracy} max={SURVEY_LIMITS.maxAccuracy} step="0.01"
                      value={settings.accuracy} disabled={running || busy != null} aria-invalid={invalidAccuracy}
                      onChange={(e) => patchSettings({ accuracy: e.target.value })} />
                  </label>
                  <label className="block text-xs text-muted">{t('rtk.duration')}
                    <input className={CONTROL} type="number" inputMode="numeric" min={SURVEY_LIMITS.minDuration} max={SURVEY_LIMITS.maxDuration} step="1"
                      value={settings.duration} disabled={running || busy != null} aria-invalid={invalidDuration}
                      onChange={(e) => patchSettings({ duration: e.target.value })} />
                  </label>
                  {invalid && <p className="text-[11px] text-error">{t('rtk.invalidSurvey')}</p>}
                  <p className="text-[11px] leading-relaxed text-muted">{t('rtk.surveyHint')}</p>
                  <p className="text-[11px] leading-relaxed text-muted">{t('rtk.writeScope')}</p>
                </>}
              </div>
            </form>

            {base?.session_id && <section aria-label={t('rtk.baseStatus')}>
              <Heading>{t('rtk.baseStatus')}</Heading>
              <div className="rounded border border-line bg-canvas p-2 text-xs">
                <Row k={t('rtk.status')} v={base.phase === 'ready' && base.mode?.kind === 'preconfigured' ? t('rtk.passiveReady') : t(`rtk.phase.${base.phase}`)}
                  tone={baseStale ? 'text-muted' : base.phase === 'ready' ? 'text-ok' : base.phase === 'failed' ? 'text-error' : 'text-muted'} />
                {base.receiver && <Row k={t('rtk.receiver')} v={base.receiver} />}
                {base.mode?.kind === 'survey_in' && <>
                  <Row k={t('rtk.observed')} v={survey ? `${survey.duration_s} / ${base.mode.config.min_duration_s} s` : '—'} />
                  <Row k={t('rtk.estimatedAccuracy')} v={survey ? `${survey.mean_accuracy_m.toFixed(2)} m` : '—'} />
                  <Row k={t('rtk.observations')} v={survey ? String(survey.observations) : '—'} />
                </>}
                {baseStale && <p className="mt-1 text-warn">{t('rtk.baseStale')}</p>}
              </div>
            </section>}

            <section>
              <Heading>{t('rtk.injection')}</Heading>
              <div className="rounded border border-line bg-canvas p-2 text-xs">
                {running && <>
                  <Row k={t('rtk.dataAge')} v={dataAge == null ? t('rtk.waiting') : `${dataAge.toFixed(1)} s`} />
                  <Row k={t('rtk.correctionAge')} v={rtcmAge == null ? t('rtk.waiting') : `${rtcmAge.toFixed(1)} s`} />
                </>}
                {stats && hasLink ? <>
                  <Row k={t('rtk.bytesPerSecond')} v={stats.bytes_per_second.toFixed(0)} />
                  <Row k={t('rtk.totalFrames')} v={String(stats.total_frames)} />
                  <Row k={t('rtk.dropped')} v={String(stats.dropped_chunks)} tone={stats.dropped_chunks > 0 ? 'text-warn' : undefined} />
                  {Object.keys(stats.frames_per_type).length > 0 && <Row k={t('rtk.types')} v={Object.entries(stats.frames_per_type).map(([type, n]) => `${type}×${n}`).join(' ')} />}
                  {stats.error && <p className="mt-1 break-words text-error">{stats.error}</p>}
                </> : <p className="py-1 text-muted">{t(running && !hasLink ? 'rtk.waitingVehicle' : base?.phase === 'surveying' ? 'rtk.waitingSurvey' : 'rtk.injectionOff')}</p>}
                {base?.phase === 'ready' && rtcmAge != null && rtcmAge > STALE_CORRECTIONS_S && <p className="mt-1 text-warn">{t('rtk.stalled')}</p>}
                {base?.phase === 'ready' && rtcmAge == null && <p className="mt-1 text-muted">{t('rtk.waitingCorrections')}</p>}
                {stats?.stalled && <p className="mt-1 text-warn">{t('rtk.sourceStalled')}</p>}
                {running && hasLink && !link.fc_alive && <p className="mt-1 text-warn">{t('rtk.heartbeatLost')}</p>}
              </div>
              {foreign?.active && <p className="mt-1 rounded border border-warn bg-canvas px-2 py-1 text-[11px] text-warn">{t('rtk.foreignActive')} ({String(foreign.packets)})</p>}
            </section>
            <p id="rtk-description" className="text-[11px] leading-relaxed text-muted">{t('rtk.hint')}</p>
          </div>
          <div className="shrink-0 space-y-2 border-t border-line bg-panel px-4 py-3">
            {displayedError && <p role="alert" className="break-words text-xs text-error">{displayedError}</p>}
            {running || busy === 'starting' ? <button type="button" onClick={() => { setError(null); void stop().then(setError) }}
              disabled={busy === 'stopping' || !base?.session_id || (busy === 'starting' && !running)}
              className="min-h-8 w-full touch-target rounded border border-line bg-canvas px-3 py-2 text-xs font-medium text-ink disabled:opacity-50">
              {t(busy === 'stopping' ? 'rtk.stopping' : base?.phase === 'identifying' || base?.phase === 'configuring' ? 'rtk.cancel' : 'rtk.stop')}
            </button> : <button type="submit" form="rtk-base-form" disabled={busy != null || !selectedPort || selectedPort.unavailable_reason != null || invalid}
              className="min-h-8 w-full touch-target rounded border border-accent bg-accent px-3 py-2 text-xs font-medium text-accent-ink disabled:opacity-50">{t('rtk.start')}</button>}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function Heading({ children }: { children: React.ReactNode }) {
  return <h3 className="mb-1 text-[11px] uppercase tracking-wide text-muted">{children}</h3>
}

function Row({ k, v, tone }: { k: string; v: string; tone?: string }) {
  return <div className="flex items-baseline justify-between gap-3 py-0.5"><span className="shrink-0 text-muted">{k}</span><span title={v} className={`mono min-w-0 break-words text-right ${tone ?? 'text-ink'}`}>{v}</span></div>
}

function fixKey(fix: string): string {
  const map: Record<string, string> = { NO_GPS: 'noGps', NO_FIX: 'noFix', FIX2D: 'fix2d', FIX3D: 'fix3d', DGPS: 'dgps', RTK_FLOAT: 'rtkFloat', RTK_FIXED: 'rtkFixed', STATIC: 'static', PPP: 'ppp' }
  return map[fix] ?? 'noGps'
}
