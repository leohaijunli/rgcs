import { ChevronDown, FileDown, Gauge, LineChart, List, Settings, SlidersHorizontal } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../stores/telemetry'
import { useLinkStore } from '../stores/link'
import { useAlerts } from '../stores/alerts'
import { FIELD_STALE_MS, useHeartbeatAge } from '../hooks/useDataAge'
import type { Severity } from '../generated-types/Severity'
import { IconButton, StatusDot, Tabs } from '../design-system/components'
import { useUiStore, type View } from '../stores/ui'
import { isTauri } from '../inspector/mock'
import SettingsDialog, { type SettingsTab } from './dialogs/SettingsDialog'
import LogsDialog from './dialogs/LogsDialog'
import LinkPopover from './LinkPopover'
import RtkDrawer from './RtkDrawer'
import ModePopover from './ModePopover'
import FlightMenu from './FlightMenu'
import SafetyCommands from './SafetyCommands'
import EventLogDrawer from './EventLogDrawer'

const VIEWS: View[] = ['planning', 'flight']

/** Badge colour per severity — tokens only, and the number carries the
 * meaning as well as the colour (design principle: never colour alone). */
const BADGE_TONE: Record<Severity, { border: string; text: string }> = {
  info: { border: 'border-line', text: 'text-muted' },
  warn: { border: 'border-warn', text: 'text-warn' },
  critical: { border: 'border-error', text: 'text-error' },
}

function pillTone(kind: 'ok' | 'warn' | 'err' | 'off'): 'ok' | 'warn' | 'error' | 'muted' {
  switch (kind) {
    case 'ok':
      return 'ok'
    case 'warn':
      return 'warn'
    case 'err':
      return 'error'
    default:
      return 'muted'
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
      <StatusDot tone={pillTone(tone)} />
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
  const [rtkDrawerOpen, setRtkDrawerOpen] = useState(false)
  const [logsOpen, setLogsOpen] = useState(false)
  const [logDrawerOpen, setLogDrawerOpen] = useState(false)
  const alertCount = useAlerts((s) => s.count)
  const alertSeverity = useAlerts((s) => s.highest)
  const view = useUiStore((s) => s.view)
  const setView = useUiStore((s) => s.setView)
  const link = useLinkStore((s) => s.link)
  const heartbeatAgeMs = useHeartbeatAge()
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

  const openParams = () => {
    if (isTauri()) {
      void invoke('params_open')
    } else {
      window.open('params.html', '_blank')
    }
  }

  const armed = heartbeat?.base_mode.safety_armed === true
  const rtk = gps?.fix_type ?? null
  const rtkTone: 'ok' | 'warn' | 'err' =
    rtk === 'RTK_FIXED' ? 'ok' : rtk === 'RTK_FLOAT' ? 'warn' : 'err'
  const batteryPct = sys?.battery_remaining_percent
  const batteryTone: 'ok' | 'warn' | 'err' | 'off' =
    batteryPct == null ? 'off' : batteryPct < 15 ? 'err' : batteryPct < 30 ? 'warn' : 'ok'
  const batteryText = sys ? `${(sys.battery_voltage_mv / 1000).toFixed(1)}V ${sys.battery_remaining_percent}%` : undefined
  // L3: mode / armed / RTK / battery are heartbeat readouts. Once the FC is
  // silent they are history, and a live-looking "ARMED · RTK FIXED" is the
  // dangerous failure mode — so the group dims and carries the age of its
  // last sample.
  const linkLost = link !== null && !link.fc_alive
  const stale = linkLost || heartbeatAgeMs > FIELD_STALE_MS
  const staleSeconds = Number.isFinite(heartbeatAgeMs) ? Math.round(heartbeatAgeMs / 1000) : null
  const staleText =
    staleSeconds !== null ? t('link.staleSince', { s: staleSeconds }) : t('link.noLink')

  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-panel px-3">
      <div className="flex items-baseline gap-2">
        <span className="text-lg font-semibold tracking-wide text-accent">{t('app.title')}</span>
      </div>

      <Tabs
        items={VIEWS.map((v) => ({ id: v, label: t(`nav.${v}`) }))}
        active={view}
        onChange={setView}
        className="text-sm [&>button]:px-3 [&>button]:py-1.5"
      />

      <div className="ml-auto flex items-center gap-2">
        {stale && (
          <span data-testid="link-stale-age" className="text-[10px] text-warn">
            {staleText}
          </span>
        )}
        <div
          data-testid="link-stale"
          data-stale={stale ? 'true' : 'false'}
          className={`flex items-center gap-2 ${stale ? 'opacity-45' : ''}`}
        >
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
          <FlightMenu />
          <ModePopover />
          <Pill
            label={t('link.rtk')}
            value={gps ? `${t(`hud.fix.${fixKey(rtk ?? 'NO_GPS')}`)} ${gps.satellites_visible}sv` : '—'}
            tone={rtkTone}
            onClick={() => setRtkDrawerOpen(true)}
            title={t('rtk.title')}
          />
          <Pill label={t('link.batt')} value={batteryText} tone={batteryTone} />
        </div>
        {/* L2: the safety pair sits outside the stale group — dimming RTL is
         * not the same as losing the link, and the buttons stay reachable
         * while another command is in flight (A7 arbitrates). */}
        <SafetyCommands />
        <LinkPopover onOpenSettings={openConnection} />

        {/* Tools ▾ groups the window launchers (motor-test plan §2.2-10) so
         * the TopBar stays pills + two icons. */}
        <ToolsPopover
          onInspector={openInspector}
          onMotorTest={openMotorTest}
          onParams={openParams}
          onLogs={() => setLogsOpen(true)}
        />
        {/* One badge for every window's alert count (A3): severity-coloured
         * number that opens the alert center. */}
        <span className="relative">
          <IconButton
            title={alertCount > 0 ? t('alerts.open') : t('eventLog.title')}
            active={false}
            data-testid="alert-open"
            onClick={() => setLogDrawerOpen(true)}
          >
            <span className={alertSeverity ? BADGE_TONE[alertSeverity].text : undefined}>
              <List size={16} />
            </span>
          </IconButton>
          {alertCount > 0 && (
            <span
              data-testid="alert-badge"
              className={`pointer-events-none absolute -right-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full border bg-panel px-1 text-[10px] font-semibold tabular-nums ${
                BADGE_TONE[alertSeverity ?? 'info'].border
              } ${BADGE_TONE[alertSeverity ?? 'info'].text}`}
            >
              {alertCount > 99 ? '99+' : alertCount}
            </span>
          )}
        </span>
        <IconButton title={t('settings.menu')} onClick={() => setSettingsOpen(true)}>
          <Settings size={16} />
        </IconButton>
      </div>
      <RtkDrawer open={rtkDrawerOpen} onClose={() => setRtkDrawerOpen(false)} />
      <EventLogDrawer open={logDrawerOpen} onClose={() => setLogDrawerOpen(false)} />
      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        tab={settingsTab}
        onTabChange={setSettingsTab}
      />
      <LogsDialog open={logsOpen} onOpenChange={setLogsOpen} />
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
  onParams,
  onLogs,
}: {
  onInspector: () => void
  onMotorTest: () => void
  onParams: () => void
  onLogs: () => void
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
          <button
            onClick={() => {
              setOpen(false)
              onParams()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <SlidersHorizontal size={14} />
            {t('tools.parameters')}
          </button>
          <button
            onClick={() => {
              setOpen(false)
              onLogs()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <FileDown size={14} />
            {t('tools.logs')}
          </button>
        </div>
      )}
    </div>
  )
}
