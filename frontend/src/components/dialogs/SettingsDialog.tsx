import { useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import * as Tabs from '@radix-ui/react-tabs'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { connectEndpoint, disconnect, reportConnectError, shutdownApp } from '../../desktop/connect'
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
import { useLinkStore } from '../../stores/link'
import { useTelemetryStore } from '../../stores/telemetry'
import { useUiStore } from '../../stores/ui'
import { hasInboundPackets, lastPacketAgeMs, linkLevel, linkLevelTone } from '../../util/linkLevel'

const APP_VERSION = '0.1.0'
const KINDS: EndpointKind[] = ['udpin', 'udpout', 'tcpin', 'tcpout', 'serial']

export type SettingsTab = 'connection' | 'vehicle' | 'appearance' | 'logs' | 'devices' | 'about'

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
              {(['connection', 'vehicle', 'appearance', 'logs', 'devices', 'about'] as const).map((id) => (
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
                <AppearanceTab theme={theme} setTheme={setTheme} />
              </Tabs.Content>
              <Tabs.Content value="logs">
                <LogsTab />
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
  const [confirmShutdown, setConfirmShutdown] = useState(false)
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
          onClick={() => disconnect().catch(reportConnectError)}
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

      <button
        onClick={() => (confirmShutdown ? void shutdownApp() : setConfirmShutdown(true))}
        className="touch-target h-11 rounded border border-error px-4 text-error"
      >
        {confirmShutdown ? t('settings.shutdownConfirm') : t('settings.shutdown')}
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

  const useMapCenter = () => {
    if (!mapCenter) return
    setLat(formatDeg(mapCenter.lat))
    setLon(formatDeg(mapCenter.lon))
    setInitialPosition({ lat: mapCenter.lat, lon: mapCenter.lon })
  }

  const field =
    'mono touch-target mt-1 w-full rounded border border-line bg-canvas px-2 py-1 text-sm text-ink'

  return (
    <div className="text-sm">
      <div className="mb-1 text-xs text-muted">{t('settings.initialPosition.title')}</div>
      <div className="mb-3 text-xs text-muted">{t('settings.initialPosition.hint')}</div>
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
    </div>
  )
}

function AppearanceTab({
  theme,
  setTheme,
}: {
  theme: 'dark' | 'light'
  setTheme: (t: 'dark' | 'light') => void
}) {
  const { t } = useTranslation()
  return (
    <div className="text-sm">
      <div className="mb-2 text-xs text-muted">{t('settings.theme')}</div>
      <div className="flex gap-2">
        {(['dark', 'light'] as const).map((th) => (
          <button
            key={th}
            onClick={() => setTheme(th)}
            className={`touch-target h-11 flex-1 rounded border border-line px-4 ${
              theme === th ? 'bg-accent text-accent-ink' : 'bg-canvas text-muted hover:text-ink'
            }`}
          >
            {t(`theme.${th}`)}
          </button>
        ))}
      </div>
    </div>
  )
}

function LogsTab() {
  const { t } = useTranslation()
  return (
    <div className="text-sm">
      <div className="mb-3 text-xs text-muted">{t('settings.logs.placeholder')}</div>
      <div className="flex gap-2">
        <button disabled className="touch-target h-11 rounded border border-line bg-canvas px-4 text-muted opacity-50">
          {t('settings.logs.refresh')}
        </button>
        <button disabled className="touch-target h-11 rounded border border-line bg-canvas px-4 text-muted opacity-50">
          {t('settings.logs.download')}
        </button>
        <button disabled className="touch-target h-11 rounded border border-line bg-canvas px-4 text-muted opacity-50">
          {t('settings.logs.erase')}
        </button>
      </div>
    </div>
  )
}

function DevicesTab() {
  const { t } = useTranslation()
  const devices = useDevicesStore((s) => s.devices)
  return (
    <div className="text-sm">
      <div className="mb-2 text-xs text-muted">{t('settings.devices.hint')}</div>
      {devices.length === 0 ? (
        <div className="rounded border border-line bg-canvas p-3 text-muted">{t('settings.devices.none')}</div>
      ) : (
        <ul className="space-y-1">
          {devices.map((d) => (
            <li key={d.port_name} className="mono rounded border border-line bg-canvas px-3 py-2 text-xs">
              {d.port_name} · {d.device_id ? `${d.device_id.vendor_id.toString(16)}:${d.device_id.product_id.toString(16)}` : d.transport}
            </li>
          ))}
        </ul>
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
