import { ChevronDown, Gauge, LineChart, Settings } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, type View } from '../stores/ui'
import { isTauri } from '../inspector/mock'
import { modeLabel } from '../util/modeLabel'
import SettingsDialog, { type SettingsTab } from './dialogs/SettingsDialog'
import LinkPopover from './LinkPopover'

const VIEWS: View[] = ['planning', 'flight']

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
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('connection')
  const view = useUiStore((s) => s.view)
  const setView = useUiStore((s) => s.setView)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)
  const sys = useTelemetryStore((s) => s.snapshot?.sys_status ?? null)

  const openConnection = () => {
    setSettingsTab('connection')
    setSettingsOpen(true)
  }

  const openInspector = () => {
    if (isTauri()) {
      void invoke('inspector_open')
    } else {
      window.open('inspector.html', '_blank')
    }
  }

  const openMotorTest = () => {
    if (isTauri()) {
      void invoke('actuator_open')
    } else {
      window.open('actuator.html', '_blank')
    }
  }

  const armed = heartbeat?.base_mode.safety_armed === true
  const mode = modeLabel(t, heartbeat)
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
        <Pill label={t('link.mode')} value={mode ?? '—'} tone={mode ? 'ok' : 'off'} />
        <Pill
          label={t('link.rtk')}
          value={gps ? `${t(`hud.fix.${fixKey(rtk ?? 'NO_GPS')}`)} ${gps.satellites_visible}sv` : '—'}
          tone={rtkTone}
        />
        <Pill label={t('link.batt')} value={batteryText} tone={batteryTone} />
        <LinkPopover onOpenSettings={openConnection} />

        {/* Tools ▾ groups the window launchers (motor-test plan §2.2-10) so
         * the TopBar stays pills + two icons. */}
        <ToolsPopover onInspector={openInspector} onMotorTest={openMotorTest} />
        <button
          aria-label={t('settings.menu')}
          onClick={() => setSettingsOpen(true)}
          className="flex h-11 w-11 items-center justify-center rounded border border-line bg-canvas text-muted hover:text-ink"
        >
          <Settings size={16} />
        </button>
      </div>
      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        tab={settingsTab}
        onTabChange={setSettingsTab}
      />
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

/** Tools ▾: the window launchers, one popover (motor-test plan §5). */
function ToolsPopover({
  onInspector,
  onMotorTest,
}: {
  onInspector: () => void
  onMotorTest: () => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    window.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div className="relative" ref={ref}>
      <button
        onClick={() => setOpen((o) => !o)}
        aria-label={t('tools.popover')}
        title={t('tools.popover')}
        className={`flex h-11 items-center gap-1.5 rounded border border-line px-2.5 ${
          open ? 'bg-panel text-ink' : 'bg-canvas text-muted hover:text-ink'
        }`}
      >
        <Gauge size={16} />
        <span className="hidden text-xs xl:inline">{t('tools.popover')}</span>
        <ChevronDown size={12} />
      </button>
      {open && (
        <div className="absolute right-0 top-full z-50 mt-1 w-52 rounded border border-line bg-panel p-1 shadow-2xl">
          <button
            onClick={() => {
              setOpen(false)
              onInspector()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <LineChart size={14} />
            {t('inspector.open')}
          </button>
          <button
            onClick={() => {
              setOpen(false)
              onMotorTest()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <Gauge size={14} />
            {t('tools.motorTest')}
          </button>
        </div>
      )}
    </div>
  )
}
