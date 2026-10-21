import { useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import * as Tabs from '@radix-ui/react-tabs'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { allAreas, isValidAreaInput, loadCustomAreas, presetArea, saveCustomAreas, type PresetArea } from '../../cesium/presets'
import { connectEndpoint, reportConnectError } from '../../desktop/connect'
import {
  BAUD_RATES,
  DEFAULT_DRAFT,
  ENDPOINT_PRESETS,
  draftToPreset,
  endpointError,
  formatEndpoint,
  parseEndpoint,
  type EndpointDraft,
  type EndpointKind,
} from '../../desktop/endpoint'
import {
  isValidLatitude,
  isValidLongitude,
  usePrefsStore,
} from '../../desktop/prefs'
import { useDevicesStore } from '../../stores/devices'
import { useTeardownStore } from '../../stores/teardown'
import { useLinkStore } from '../../stores/link'
import { useTelemetryStore } from '../../stores/telemetry'
import { useUiStore } from '../../stores/ui'
import type { ThemePref } from '../../design-system/theme'
import { hasInboundPackets, lastPacketAgeMs, linkLevel, linkLevelTone } from '../../util/linkLevel'

const APP_VERSION = '0.1.0'
const KINDS: EndpointKind[] = ['udpin', 'udpout', 'tcpin', 'tcpout', 'serial']

export type SettingsTab = 'connection' | 'vehicle' | 'appearance' | 'devices' | 'about'

const TONE_TEXT: Record<string, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  err: 'text-error',
  off: 'text-muted',
}

/** Single settings surface: every preference grouped under tabs. */
export default function SettingsDialog({
  open,
  onOpenChange,
  tab,
  onTabChange,
}: {
  open: boolean
  onOpenChange: (o: boolean) => void
  tab: SettingsTab
  onTabChange: (t: SettingsTab) => void
}) {
  const { t } = useTranslation()
  const theme = useUiStore((s) => s.theme)
  const fieldMode = useUiStore((s) => s.fieldMode)
  const setFieldMode = useUiStore((s) => s.setFieldMode)
  const setTheme = useUiStore((s) => s.setTheme)
  const link = useLinkStore((s) => s.link)

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        {/* Fixed height: switching tabs must not resize the dialog (task 0.7). */}
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex h-[min(85vh,560px)] w-[min(94vw,680px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-panel shadow-2xl">
          <div className="flex items-center justify-between border-b border-line px-4 py-3">
            <Dialog.Title className="text-sm font-medium text-ink">{t('settings.menu')}</Dialog.Title>
            <Dialog.Close asChild>
              <button
                aria-label="close"
                className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>

          <Tabs.Root
            value={tab}
            onValueChange={(v) => onTabChange(v as SettingsTab)}
            className="flex min-h-0 flex-1"
          >
            <Tabs.List className="flex w-40 shrink-0 flex-col gap-0.5 border-r border-line p-2">
              {(['connection', 'vehicle', 'appearance', 'devices', 'about'] as const).map((id) => (
                <Tabs.Trigger
                  key={id}
                  value={id}
                  className="touch-target rounded px-3 py-2 text-left text-sm text-muted transition-colors data-[state=active]:bg-accent data-[state=active]:text-accent-ink hover:text-ink"
                >
                  {t(`settings.tab.${id}`)}
                </Tabs.Trigger>
              ))}
            </Tabs.List>

            <div className="min-h-0 flex-1 overflow-y-auto p-4">
              <Tabs.Content value="connection">
                <ConnectionTab />
              </Tabs.Content>
              <Tabs.Content value="vehicle">
                <VehicleTab />
              </Tabs.Content>
              <Tabs.Content value="appearance">
                <AppearanceTab theme={theme} setTheme={setTheme} fieldMode={fieldMode} setFieldMode={setFieldMode} />
              </Tabs.Content>
              <Tabs.Content value="devices">
                <DevicesTab />
              </Tabs.Content>
              <Tabs.Content value="about">
                <AboutTab link={link} />
              </Tabs.Content>
            </div>
          </Tabs.Root>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function FieldError({ message }: { message: string | null }) {
  if (!message) return null
  return <div className="mt-1 text-xs text-error">{message}</div>
}

function ConnectionTab() {
  const { t } = useTranslation()
  const savedEndpoint = usePrefsStore((s) => s.endpoint)
  const autoConnect = usePrefsStore((s) => s.autoConnect)
  const setAutoConnect = usePrefsStore((s) => s.setAutoConnect)
  const [draft, setDraft] = useState<EndpointDraft>(
    () => parseEndpoint(savedEndpoint) ?? DEFAULT_DRAFT,
  )
  const [busy, setBusy] = useState(false)
  const requestTeardown = useTeardownStore((s) => s.request)
  const link = useLinkStore((s) => s.link)
  const errorHistory = useLinkStore((s) => s.errorHistory)
  const droppedFrames = useLinkStore((s) => s.droppedFrames)
  const fieldAges = useTelemetryStore((s) => s.snapshot?.field_ages)
  const devices = useDevicesStore((s) => s.devices)

  const fieldError = endpointError(draft)
  const update = (patch: Partial<EndpointDraft>) => setDraft((d) => ({ ...d, ...patch }))

  const level = linkLevel(link, hasInboundPackets(fieldAges))
  // Without a link the mock feed still fills `field_ages`; don't imply packets.
  const age = link ? lastPacketAgeMs(fieldAges) : null
  const lastPacket = age === null ? t('settings.never') : `${(age / 1000).toFixed(1)} s`

  const connect = async () => {
    if (fieldError) return
    setBusy(true)
    try {
      const endpoint = formatEndpoint(draft)
      usePrefsStore.getState().setEndpoint(endpoint)
      await connectEndpoint(endpoint)
    } catch (e) {
      reportConnectError(e)
    } finally {
      setBusy(false)
    }
  }

  const inputClass = (invalid: boolean) =>
    `mono h-11 w-full rounded border bg-canvas px-3 text-sm text-ink outline-none ${
      invalid ? 'border-error' : 'border-line focus:border-accent'
    }`

  return (
    <div className="space-y-3 text-sm">
      <div className="grid grid-cols-2 gap-3">
        <div>
          <label className="mb-1 block text-xs text-muted">{t('settings.endpoint.type')}</label>
          <select
            value={draft.kind}
            onChange={(e) => update({ kind: e.target.value as EndpointKind })}
            className={inputClass(false)}
          >
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`settings.endpoint.kind.${k}`)}
              </option>
            ))}
          </select>
        </div>
        {draft.kind === 'serial' ? (
          <div>
            <label className="mb-1 block text-xs text-muted">{t('settings.endpoint.baud')}</label>
            <select
              value={draft.baud}
              onChange={(e) => update({ baud: e.target.value })}
              className={inputClass(fieldError === 'baud')}
            >
              {BAUD_RATES.map((b) => (
                <option key={b} value={b}>
                  {b}
                </option>
              ))}
            </select>
            <FieldError
              message={fieldError === 'baud' ? t('settings.endpoint.invalidBaud') : null}
            />
          </div>
        ) : (
          <div>
            <label className="mb-1 block text-xs text-muted">{t('settings.endpoint.port')}</label>
            <input
              value={draft.port}
              onChange={(e) => update({ port: e.target.value })}
              spellCheck={false}
              className={inputClass(fieldError === 'port')}
            />
            <FieldError
              message={fieldError === 'port' ? t('settings.endpoint.invalidPort') : null}
            />
          </div>
        )}
        <div className="col-span-2">
          <label className="mb-1 block text-xs text-muted">
            {draft.kind === 'serial'
              ? t('settings.endpoint.serialPort')
              : t('settings.endpoint.host')}
          </label>
          {draft.kind === 'serial' ? (
            <select
              value={draft.host}
              onChange={(e) => update({ host: e.target.value })}
              className={inputClass(fieldError === 'host')}
            >
              {devices.length === 0 && <option value="">{t('settings.endpoint.noSerial')}</option>}
              {devices.map((d) => (
                <option key={d.port_name} value={d.port_name}>
                  {d.port_name}
                </option>
              ))}
            </select>
          ) : (
            <input
              value={draft.host}
              onChange={(e) => update({ host: e.target.value })}
              spellCheck={false}
              className={inputClass(fieldError === 'host')}
            />
          )}
          <FieldError
            message={fieldError === 'host' ? t('settings.endpoint.invalidHost') : null}
          />
        </div>
      </div>

      <div>
        <div className="mb-1 text-xs text-muted">{t('settings.endpoint.presets')}</div>
        <div className="flex flex-wrap gap-2">
          {ENDPOINT_PRESETS.map((p) => (
            <button
              key={p.id}
              onClick={() => setDraft(draftToPreset(p))}
              className="touch-target rounded border border-line bg-canvas px-3 py-2 text-xs text-muted hover:text-ink"
            >
              {t(`settings.endpoint.preset.${p.id}`)}
            </button>
          ))}
        </div>
      </div>

      <div className="flex gap-2">
        <button
          onClick={connect}
          disabled={busy || fieldError !== null}
          className="touch-target h-11 rounded bg-accent px-4 text-accent-ink disabled:opacity-50"
        >
          {busy ? '…' : t('map.connect')}
        </button>
        <button
          data-testid="settings-disconnect"
          onClick={() => requestTeardown('disconnect')}
          className="touch-target h-11 rounded border border-line bg-canvas px-4 text-muted hover:text-ink"
        >
          {t('settings.disconnect')}
        </button>
      </div>

      <label className="flex items-center gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={autoConnect}
          onChange={(e) => setAutoConnect(e.target.checked)}
          className="h-4 w-4"
        />
        {t('settings.endpoint.autoConnect')}
      </label>

      {/* Link diagnostics: four-level state + "are packets arriving?" (task 0.5). */}
      <div className="space-y-1 rounded border border-line bg-canvas p-3 text-xs">
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('settings.linkStatus')}</span>
          <span className={`flex items-center gap-1.5 font-medium ${TONE_TEXT[linkLevelTone(level)]}`}>
            <span className={`h-1.5 w-1.5 rounded-full bg-current`} />
            {t(`settings.linkState.${level}`)}
          </span>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('settings.lastPacket')}</span>
          <span className="mono text-ink">{lastPacket}</span>
        </div>
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('settings.droppedFrames')}</span>
          <span className="mono text-ink">{droppedFrames}</span>
        </div>
        <div className="mono truncate text-muted">{link?.endpoint || '—'}</div>
        <div className="border-t border-line pt-1">
          <div className="mb-0.5 text-muted">{t('settings.recentErrors')}</div>
          {errorHistory.length === 0 ? (
            <div className="text-muted">{t('settings.noErrors')}</div>
          ) : (
            <ul className="max-h-24 space-y-0.5 overflow-y-auto">
              {errorHistory.slice(0, 10).map((e, i) => (
                <li key={`${e.at_ms}-${i}`} className="truncate text-error">
                  <span className="text-muted">
                    {new Date(e.at_ms).toLocaleTimeString()}{' '}
                  </span>
                  {t(`link.error.${e.kind}`)}: {e.message}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {/* The confirm dialog replaces the old two-click button: it shows the
       * refusal or asks for the keyword the shell names (L4). */}
      <button
        data-testid="settings-shutdown"
        onClick={() => requestTeardown('shutdown_app')}
        className="touch-target h-11 rounded border border-error px-4 text-error"
      >
        {t('settings.shutdown')}
      </button>
    </div>
  )
}

/** Six-decimal text, so the input round-trips the stored value exactly. */
function formatDeg(deg: number): string {
  return deg.toFixed(6)
}

/**
 * Initial vehicle position (mock feed origin, HOME marker and initial camera
 * until a live fix arrives). Committed on blur/Enter, only when both fields
 * are valid lat/lon.
 */
function VehicleTab() {
  const { t } = useTranslation()
  const saved = usePrefsStore((s) => s.initialPosition)
  const setInitialPosition = usePrefsStore((s) => s.setInitialPosition)
  const modelOrientation = usePrefsStore((s) => s.modelOrientation)
  const setModelOrientation = usePrefsStore((s) => s.setModelOrientation)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const [lat, setLat] = useState(() => formatDeg(saved.lat))
  const [lon, setLon] = useState(() => formatDeg(saved.lon))

  const latValue = Number(lat)
  const lonValue = Number(lon)
  const latError =
    lat.trim() === '' || !isValidLatitude(latValue)
      ? t('settings.initialPosition.invalidLatitude')
      : null
  const lonError =
    lon.trim() === '' || !isValidLongitude(lonValue)
      ? t('settings.initialPosition.invalidLongitude')
      : null

  const commit = () => {
    if (latError || lonError) return
    setInitialPosition({ lat: latValue, lon: lonValue })
  }

  const [customAreas, setCustomAreas] = useState<PresetArea[]>(() => loadCustomAreas())
  const [areaName, setAreaName] = useState('')
  const [areaError, setAreaError] = useState<string | null>(null)

  const persistCustom = (areas: PresetArea[]) => {
    setCustomAreas(areas)
    saveCustomAreas(areas)
  }

  const useMapCenter = () => {
    if (!mapCenter) return
    setLat(formatDeg(mapCenter.lat))
    setLon(formatDeg(mapCenter.lon))
    setInitialPosition({ lat: mapCenter.lat, lon: mapCenter.lon })
  }

  const applyPreset = (id: string) => {
    const area = presetArea(id)
    if (!area) return
    setLat(formatDeg(area.lat))
    setLon(formatDeg(area.lon))
    setInitialPosition({ lat: area.lat, lon: area.lon })
  }

  const addCustomArea = () => {
    const name = areaName.trim()
    const latV = Number(lat)
    const lonV = Number(lon)
    if (!name) {
      setAreaError(t('settings.areas.nameRequired'))
      return
    }
    if (!isValidAreaInput(latV, lonV)) {
      setAreaError(t('settings.areas.coordsInvalid'))
      return
    }
    // A fresh id per entry (name collisions stay distinct entries).
    const area: PresetArea = {
      id: `custom-${Date.now().toString(36)}`,
      name,
      lat: latV,
      lon: lonV,
      alt: 0,
    }
    persistCustom([...customAreas, area])
    setAreaName('')
    setAreaError(null)
  }

  const removeCustomArea = (id: string) => {
    persistCustom(customAreas.filter((a) => a.id !== id))
  }

  const field =
    'mono touch-target mt-1 w-full rounded border border-line bg-canvas px-2 py-1 text-sm text-ink'

  return (
    <div className="text-sm">
      <div className="mb-1 text-xs text-muted">{t('settings.initialPosition.title')}</div>
      <div className="mb-3 text-xs text-muted">{t('settings.initialPosition.hint')}</div>
      <label className="block">
        <span className="text-xs text-muted">{t('settings.initialPosition.preset')}</span>
        <select
          value=""
          onChange={(e) => applyPreset(e.target.value)}
          className={field}
        >
          <option value="">—</option>
          {allAreas(customAreas).map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </label>
      <div className="grid grid-cols-2 gap-3">
        <label className="block">
          <span className="text-xs text-muted">{t('settings.initialPosition.latitude')}</span>
          <input
            value={lat}
            inputMode="decimal"
            onChange={(e) => setLat(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => e.key === 'Enter' && commit()}
            className={field}
          />
          <FieldError message={latError} />
        </label>
        <label className="block">
          <span className="text-xs text-muted">{t('settings.initialPosition.longitude')}</span>
          <input
            value={lon}
            inputMode="decimal"
            onChange={(e) => setLon(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => e.key === 'Enter' && commit()}
            className={field}
          />
          <FieldError message={lonError} />
        </label>
      </div>
      <button
        onClick={useMapCenter}
        disabled={!mapCenter}
        className="touch-target mt-3 h-11 rounded border border-line px-4 text-muted hover:text-ink disabled:opacity-50"
      >
        {t('settings.initialPosition.useMapCenter')}
      </button>
      <div className="mono mt-3 text-xs text-muted">
        {formatDeg(saved.lat)}, {formatDeg(saved.lon)}
      </div>

      {/* Custom preset areas: the operator's own take-off sites, persisted in
       * localStorage and offered in the dropdown above. Added from whatever
       * lat/lon is committed right now (map centre or typed). */}
      <div className="mt-5 mb-1 text-xs text-muted">{t('settings.areas.title')}</div>
      <div className="mb-3 text-xs text-muted">{t('settings.areas.hint')}</div>
      <div className="flex items-center gap-2">
        <input
          value={areaName}
          onChange={(e) => setAreaName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && addCustomArea()}
          placeholder={t('settings.areas.namePlaceholder')}
          className={`${field} mt-0 flex-1`}
        />
        <button
          onClick={addCustomArea}
          className="touch-target h-9 shrink-0 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
        >
          {t('settings.areas.add')}
        </button>
      </div>
      <FieldError message={areaError} />
      {customAreas.length > 0 && (
        <ul className="mt-2 space-y-1">
          {customAreas.map((a) => (
            <li key={a.id} className="flex items-center gap-2 text-xs">
              <span className="flex-1 truncate text-ink">{a.name}</span>
              <span className="mono text-muted">
                {formatDeg(a.lat)}, {formatDeg(a.lon)}
              </span>
              <button
                onClick={() => removeCustomArea(a.id)}
                aria-label={`remove ${a.name}`}
                className="rounded px-1 text-muted hover:text-error"
              >
                <X size={12} />
              </button>
            </li>
          ))}
        </ul>
      )}

      {/* UAV model orientation offsets (issues.md #43): added to the MAVLink
       * attitude when the 3D model is drawn, so a model whose nose is not the
       * asset's default axis can be re-aligned without touching code. */}
      <div className="mt-5 mb-1 text-xs text-muted">{t('settings.modelOrientation.title')}</div>
      <div className="mb-3 text-xs text-muted">{t('settings.modelOrientation.hint')}</div>
      <div className="grid grid-cols-3 gap-3">
        {(['pitch', 'roll', 'yaw'] as const).map((axis) => (
          <label key={axis} className="block">
            <span className="text-xs text-muted">{t(`settings.modelOrientation.${axis}`)}</span>
            <input
              value={modelOrientation[axis]}
              inputMode="decimal"
              type="number"
              step="1"
              onChange={(e) =>
                setModelOrientation({ ...modelOrientation, [axis]: Number(e.target.value) })
              }
              className={field}
            />
          </label>
        ))}
      </div>
    </div>
  )
}

const THEME_OPTIONS: readonly ThemePref[] = ['dark', 'light', 'system', 'night-red']

function AppearanceTab({
  theme,
  setTheme,
  fieldMode,
  setFieldMode,
}: {
  theme: ThemePref
  setTheme: (t: ThemePref) => void
  fieldMode: boolean
  setFieldMode: (on: boolean) => void
}) {
  const { t } = useTranslation()
  return (
    <div className="text-sm">
      <div className="mb-2 text-xs text-muted">{t('settings.theme')}</div>
      <div className="grid grid-cols-2 gap-2">
        {THEME_OPTIONS.map((th) => (
          <button
            key={th}
            onClick={() => setTheme(th)}
            className={`touch-target h-11 rounded border border-line px-4 ${
              theme === th ? 'bg-accent text-accent-ink' : 'bg-canvas text-muted hover:text-ink'
            }`}
          >
            {t(`theme.${th === 'night-red' ? 'nightRed' : th}`)}
          </button>
        ))}
      </div>
      <div className="mt-3 mb-2 text-xs text-muted">{t('settings.fieldMode')}</div>
      <label
        data-testid="field-mode-toggle"
        className="flex cursor-pointer items-center gap-2 rounded border border-line bg-canvas px-3 py-2 text-xs text-ink"
      >
        <input
          type="checkbox"
          checked={fieldMode}
          onChange={(e) => setFieldMode(e.target.checked)}
        />
        <span>{t('settings.fieldModeHint')}</span>
      </label>
    </div>
  )
}


function DevicesTab() {
  const { t } = useTranslation()
  const devices = useDevicesStore((s) => s.devices)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)

  // USB devices with a known VID/PID are the udev-rule candidates
  // (issues.md #42): the rule grants world-read/write so the app can open
  // the port without root.
  const usbDevices = devices.filter((d) => d.device_id != null)
  const vidPids = usbDevices.map(
    (d) =>
      [
        d.device_id!.vendor_id.toString(16).padStart(4, '0'),
        d.device_id!.product_id.toString(16).padStart(4, '0'),
      ] as [string, string],
  )

  const install = async () => {
    setBusy(true)
    setResult(null)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const text = await invoke<string>('install_udev_rules', { vidPids })
      setResult({ ok: true, text })
    } catch (e) {
      setResult({ ok: false, text: String(e) })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="text-sm">
      <div className="mb-2 text-xs text-muted">{t('settings.devices.hint')}</div>
      {devices.length === 0 ? (
        <div className="rounded border border-line bg-canvas p-3 text-muted">{t('settings.devices.none')}</div>
      ) : (
        <>
          <ul className="space-y-1">
            {devices.map((d) => (
              <li key={d.port_name} className="mono rounded border border-line bg-canvas px-3 py-2 text-xs">
                {d.port_name} · {d.device_id ? `${d.device_id.vendor_id.toString(16)}:${d.device_id.product_id.toString(16)}` : d.transport}
              </li>
            ))}
          </ul>
          {usbDevices.length > 0 && (
            <div className="mt-3">
              <button
                onClick={() => void install()}
                disabled={busy}
                className="touch-target h-11 w-full rounded border border-line px-4 text-muted hover:text-ink disabled:opacity-50"
              >
                {busy ? '…' : t('settings.devices.fixPermissions')}
              </button>
              <div className="mt-1 text-xs text-muted">{t('settings.devices.fixHint')}</div>
              {result && (
                <div
                  className={`mt-2 rounded border px-3 py-2 text-xs ${
                    result.ok ? 'border-ok bg-ok/10 text-ok' : 'border-error bg-error/10 text-error'
                  }`}
                >
                  {result.text}
                </div>
              )}
            </div>
          )}
        </>
      )}
    </div>
  )
}

function AboutTab({ link }: { link: ReturnType<typeof useLinkStore.getState>['link'] }) {
  const { t } = useTranslation()
  return (
    <div className="space-y-1 text-sm text-muted">
      <div>MagGCS v{APP_VERSION}</div>
      <div>{t('settings.about.license')}</div>
      <div className="mono text-xs">{link?.endpoint || '—'}</div>
    </div>
  )
}
