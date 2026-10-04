import * as Popover from '@radix-ui/react-popover'
import { Settings } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useDevicesStore } from '../stores/devices'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, type View } from '../stores/ui'
import { px4Mode } from '../util/px4mode'

const VIEWS: View[] = ['planning', 'flight', 'data']

function pillTone(kind: 'ok' | 'warn' | 'err' | 'off'): string {
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

function Pill({
  label,
  value,
  tone,
  onClick,
  title,
}: {
  label: string
  value?: string
  tone: 'ok' | 'warn' | 'err' | 'off'
  onClick?: () => void
  title?: string
}) {
  const cls = `flex h-8 items-center gap-2 rounded border border-line bg-canvas px-2.5 text-xs ${onClick ? 'cursor-pointer hover:bg-panel' : ''}`
  const inner = (
    <>
      <span className="text-muted">{label}</span>
      <span className={`h-1.5 w-1.5 rounded-full ${pillTone(tone)}`} />
      {value && <span className="mono text-ink">{value}</span>}
    </>
  )
  return onClick ? (
    <button onClick={onClick} title={title} className={cls}>
      {inner}
    </button>
  ) : (
    <div className={cls}>{inner}</div>
  )
}

export default function TopBar() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const setView = useUiStore((s) => s.setView)
  const theme = useUiStore((s) => s.theme)
  const setTheme = useUiStore((s) => s.setTheme)
  const link = useLinkStore((s) => s.link)
  const devices = useDevicesStore((s) => s.devices)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)
  const sys = useTelemetryStore((s) => s.snapshot?.sys_status ?? null)

  const armed = heartbeat?.base_mode.safety_armed === true
  const mode = heartbeat
    ? heartbeat.autopilot === 'px4'
      ? px4Mode(heartbeat.custom_mode)
      : t(`mode.${heartbeat.flight_state.toLowerCase()}`)
    : null
  const rtk = gps?.fix_type ?? null
  const rtkTone: 'ok' | 'warn' | 'err' =
    rtk === 'RTK_FIXED' ? 'ok' : rtk === 'RTK_FLOAT' ? 'warn' : 'err'
  const batteryPct = sys?.battery_remaining_percent
  const batteryTone: 'ok' | 'warn' | 'err' | 'off' =
    batteryPct == null ? 'off' : batteryPct < 15 ? 'err' : batteryPct < 30 ? 'warn' : 'ok'
  const batteryText = sys ? `${(sys.battery_voltage_mv / 1000).toFixed(1)}V ${sys.battery_remaining_percent}%` : undefined

  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-panel px-3">
      <div className="flex items-baseline gap-2">
        <span className="text-lg font-semibold tracking-wide text-accent">{t('app.title')}</span>
        <span className="hidden text-xs text-muted lg:inline">{t('app.tagline')}</span>
      </div>

      <nav className="flex items-center gap-0.5 rounded border border-line bg-canvas p-0.5">
        {VIEWS.map((v) => (
          <button
            key={v}
            onClick={() => setView(v)}
            className={`rounded px-3 py-1.5 text-sm transition-colors ${
              view === v ? 'bg-accent text-accent-ink' : 'text-muted hover:text-ink'
            }`}
          >
            {t(`nav.${v}`)}
          </button>
        ))}
      </nav>

      <div className="ml-auto flex items-center gap-2">
        <div
          role="status"
          className={`flex h-8 items-center gap-1.5 rounded border px-2.5 text-xs font-medium ${
            armed
              ? 'border-ok bg-ok text-ok-ink'
              : 'border-line bg-canvas text-muted'
          }`}
        >
          <span className={`h-1.5 w-1.5 rounded-full ${armed ? 'bg-ok-ink' : 'bg-muted'}`} />
          {armed ? t('link.armed') : t('link.disarmed')}
        </div>
        <Pill label={t('link.mode')} value={mode ?? '—'} tone="off" />
        <Pill
          label={t('link.rtk')}
          value={gps ? `${t(`hud.fix.${fixKey(rtk ?? 'NO_GPS')}`)} ${gps.satellites_visible}sv` : '—'}
          tone={rtkTone}
        />
        <Pill label={t('link.batt')} value={batteryText} tone={batteryTone} />
        <Pill
          label={t('link.fc')}
          value={link ? t(`link.${link.link_state.toLowerCase()}`) : t('link.noLink')}
          tone={link?.fc_alive ? 'ok' : 'err'}
        />
        <Pill
          label={t('link.devices')}
          value={devices.length ? String(devices.length) : '—'}
          tone={devices.length ? 'ok' : 'off'}
        />

        <Popover.Root>
          <Popover.Trigger asChild>
            <button
              aria-label={t('settings.menu')}
              className="flex h-8 w-8 items-center justify-center rounded border border-line bg-canvas text-muted hover:text-ink"
            >
              <Settings size={16} />
            </button>
          </Popover.Trigger>
          <Popover.Portal>
            <Popover.Content
              align="end"
              className="z-50 w-48 rounded border border-line bg-panel p-2 text-sm shadow-xl"
            >
              <div className="mb-1 px-1 text-xs text-muted">{t('settings.theme')}</div>
              <div className="flex gap-1">
                {(['dark', 'light'] as const).map((th) => (
                  <button
                    key={th}
                    onClick={() => setTheme(th)}
                    className={`flex-1 rounded px-2 py-1 text-xs ${
                      theme === th ? 'bg-accent text-accent-ink' : 'bg-canvas text-muted hover:text-ink'
                    }`}
                  >
                    {t(`theme.${th}`)}
                  </button>
                ))}
              </div>
              <Popover.Arrow className="fill-panel" />
            </Popover.Content>
          </Popover.Portal>
        </Popover.Root>
      </div>
    </header>
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