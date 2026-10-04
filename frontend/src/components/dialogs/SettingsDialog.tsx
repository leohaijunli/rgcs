import { useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import * as Tabs from '@radix-ui/react-tabs'
import { Settings, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { connectEndpoint, disconnect, reportConnectError } from '../../desktop/connect'
import { useDevicesStore } from '../../stores/devices'
import { useLinkStore } from '../../stores/link'
import { useUiStore } from '../../stores/ui'

const DEFAULT_ENDPOINT = 'udpin:0.0.0.0:14550'
const APP_VERSION = '0.1.0'

/** Single settings surface: every preference grouped under tabs. */
export default function SettingsDialog() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)
  const link = useLinkStore((s) => s.link)

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Trigger asChild>
        <button
          aria-label={t('settings.menu')}
          className="flex h-11 w-11 items-center justify-center rounded border border-line bg-canvas text-muted hover:text-ink"
        >
          <Settings size={16} />
        </button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 flex max-h-[85vh] w-[min(94vw,680px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-line bg-panel shadow-2xl">
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

          <Tabs.Root defaultValue="connection" className="flex min-h-0 flex-1">
            <Tabs.List className="flex w-40 shrink-0 flex-col gap-0.5 border-r border-line p-2">
              {(['connection', 'appearance', 'logs', 'devices', 'about'] as const).map((tab) => (
                <Tabs.Trigger
                  key={tab}
                  value={tab}
                  className="rounded px-3 py-2 text-left text-sm text-muted transition-colors data-[state=active]:bg-accent data-[state=active]:text-accent-ink hover:text-ink"
                >
                  {t(`settings.tab.${tab}`)}
                </Tabs.Trigger>
              ))}
            </Tabs.List>

            <div className="min-h-0 flex-1 overflow-y-auto p-4">
              <Tabs.Content value="connection">
                <ConnectionTab />
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

function ConnectionTab() {
  const { t } = useTranslation()
  const [endpoint, setEndpoint] = useState(DEFAULT_ENDPOINT)
  const [busy, setBusy] = useState(false)
  const link = useLinkStore((s) => s.link)

  const connect = async () => {
    setBusy(true)
    try {
      await connectEndpoint(endpoint.trim())
    } catch (e) {
      reportConnectError(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 text-sm">
      <div>
        <label className="mb-1 block text-xs text-muted">{t('map.endpointHint')}</label>
        <input
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
          spellCheck={false}
          className="mono w-full rounded border border-line bg-canvas px-3 py-2 text-ink outline-none focus:border-accent"
        />
      </div>
      <div className="flex items-center justify-between">
        <span className="text-muted">{t('settings.linkStatus')}:</span>
        <span className={`mono ${link?.fc_alive ? 'text-ok' : 'text-error'}`}>
          {link ? t(`link.${link.link_state.toLowerCase()}`) : t('link.noLink')}
        </span>
      </div>
      <div className="flex gap-2">
        <button
          onClick={connect}
          disabled={busy}
          className="rounded bg-accent px-4 py-2 text-accent-ink disabled:opacity-50"
        >
          {busy ? '…' : t('map.connect')}
        </button>
        <button
          onClick={() => disconnect().catch(reportConnectError)}
          className="rounded border border-line bg-canvas px-4 py-2 text-muted hover:text-ink"
        >
          {t('settings.disconnect')}
        </button>
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
            className={`flex-1 rounded border border-line px-4 py-2 ${
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
        <button disabled className="rounded border border-line bg-canvas px-4 py-2 text-muted opacity-50">
          {t('settings.logs.refresh')}
        </button>
        <button disabled className="rounded border border-line bg-canvas px-4 py-2 text-muted opacity-50">
          {t('settings.logs.download')}
        </button>
        <button disabled className="rounded border border-line bg-canvas px-4 py-2 text-muted opacity-50">
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