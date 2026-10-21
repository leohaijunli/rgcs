import {
  BadgeInfo,
  BatteryMedium,
  ChevronDown,
  FileDown,
  Gauge,
  LineChart,
  List,
  Satellite,
  Settings,
  ShieldCheck,
  ShieldOff,
  SlidersHorizontal,
  PackageOpen,
} from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../stores/telemetry'
import { useLinkStore } from '../stores/link'
import { useProjectStore } from '../stores/project'
import { useAlerts, raiseAlert } from '../stores/alerts'
import { FIELD_STALE_MS, useHeartbeatAge } from '../hooks/useDataAge'
import { useIsNarrow } from '../hooks/useIsNarrow'
import { getSafetyConfig } from '../desktop/appConfig'
import type { Severity } from '../generated-types/Severity'
import { IconButton, StatusDot, Tabs } from '../design-system/components'
import { useUiStore, type View } from '../stores/ui'
import { checklistBadge } from './panels/preflight/PreflightStagePanel'
import { fetchChecklist } from '../stores/preflight'
import { isTauri } from '../inspector/mock'
import AboutDialog from './dialogs/AboutDialog'
import ProjectHome from './dialogs/ProjectHome'
import SettingsDialog, { type SettingsTab } from './dialogs/SettingsDialog'
import LogsDialog from './dialogs/LogsDialog'
import LinkPopover from './LinkPopover'
import RtkDrawer from './RtkDrawer'
import ModePopover from './ModePopover'
import FlightMenu from './FlightMenu'
import SafetyCommands from './SafetyCommands'
import AlertCenter from './AlertCenter'

const VIEWS: View[] = ['prepare', 'plan', 'preflight', 'fly', 'review']

/** Polling cadence for the Preflight tab's readiness badge. The engine is
 * pure, so this is cheap; errors (not connected) just clear the badge. */
const BADGE_POLL_MS = 2_000

/** U2: one control height for every TopBar control. The old bar mixed a 44px
 * Tools button with 32px pills and buttons; the design scale's `md` row is the
 * single source (§7.1 "fix the 44 vs 32 mismatch"), and §7.4's floor for the
 * normal (non-field) mode is 32px. */
const CONTROL = 'h-8'

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
  icon,
  testId,
  onClick,
  title,
}: {
  label: string
  value?: string
  tone: 'ok' | 'warn' | 'err' | 'off'
  icon?: ReactNode
  testId?: string
  onClick?: () => void
  title?: string
}) {
  const cls = `flex ${CONTROL} items-center gap-1.5 rounded border border-line bg-canvas px-2.5 text-xs ${onClick ? 'cursor-pointer hover:bg-panel' : ''}`
  const inner = (
    <>
      {icon !== undefined && <span className="text-muted">{icon}</span>}
      <span className="text-muted">{label}</span>
      <StatusDot tone={pillTone(tone)} />
      {value && <span className="mono text-ink">{value}</span>}
    </>
  )
  return onClick ? (
    <button data-testid={testId} onClick={onClick} title={title} className={cls}>
      {inner}
    </button>
  ) : (
    <div data-testid={testId} className={cls}>
      {inner}
    </div>
  )
}

/** Thin rule between the three TopBar groups (§7.2.1 status / health / tools). */
function GroupDivider() {
  return <span aria-hidden="true" className={`${CONTROL} w-px bg-line`} />
}

interface HealthProps {
  gps: { fix_type: string; satellites_visible: number } | null
  rtkTone: 'ok' | 'warn' | 'err'
  batteryText: string | undefined
  batteryTone: 'ok' | 'warn' | 'err' | 'off'
  onOpenRtk: () => void
}

/** Health group: the vehicle's own health readouts. Wide screens show them
 * inline; at ≤1366 they fold into one "More" popover so the status group and
 * the safety pair always keep their room (U2). The *conditions* stay visible
 * regardless — battery and GNSS degradation also raise alerts (L7). */
function HealthGroup({ gps, rtkTone, batteryText, batteryTone, onOpenRtk }: HealthProps) {
  const { t } = useTranslation()
  const narrow = useIsNarrow()
  const fix = gps ? `${t(`hud.fix.${fixKey(gps.fix_type)}`)} ${gps.satellites_visible}sv` : undefined
  const rtk = (
    <Pill
      testId="rtk-pill"
      label={t('link.rtk')}
      value={fix ?? '—'}
      tone={rtkTone}
      icon={<Satellite size={13} />}
      onClick={onOpenRtk}
      title={t('rtk.title')}
    />
  )
  const batt = (
    <Pill
      testId="batt-pill"
      label={t('link.batt')}
      value={batteryText}
      tone={batteryTone}
      icon={<BatteryMedium size={13} />}
    />
  )
  if (!narrow) {
    return (
      <div data-testid="topbar-health" className="flex items-center gap-2">
        {rtk}
        {batt}
      </div>
    )
  }
  return (
    <div data-testid="topbar-health" className="relative flex items-center">
      <MorePopover title={t('topbar.more')} testId="topbar-more">
        {rtk}
        {batt}
      </MorePopover>
    </div>
  )
}

/** The "More" overflow surface (U2 narrow layout). */
function MorePopover({
  title,
  testId,
  children,
}: {
  title: string
  testId: string
  children: ReactNode
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      // N8: a pointerdown that repaints the trigger detaches `e.target`; a
      // detached node is never inside the popover, so ignore it instead of
      // closing on the very interaction that opened it.
      const target = e.target as Node
      if (!target.isConnected) return
      if (!ref.current?.contains(target)) setOpen(false)
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
        data-testid={testId}
        onClick={() => setOpen((o) => !o)}
        aria-label={title}
        title={title}
        className={`flex ${CONTROL} items-center gap-1 rounded border border-line bg-canvas px-2.5 text-xs text-muted hover:bg-panel hover:text-ink`}
      >
        {title}
        <ChevronDown size={12} />
      </button>
      {open && (
        <div
          data-testid={`${testId}-menu`}
          className="absolute right-0 top-full z-50 mt-1 flex w-max flex-col gap-1 rounded border border-line bg-panel p-1.5 shadow-2xl"
        >
          {children}
        </div>
      )}
    </div>
  )
}

export default function TopBar() {
  const { t } = useTranslation()
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('connection')
  const [projectOpen, setProjectOpen] = useState(false)
  const [aboutOpen, setAboutOpen] = useState(false)
  const [rtkDrawerOpen, setRtkDrawerOpen] = useState(false)
  const [logsOpen, setLogsOpen] = useState(false)
  const [logDrawerOpen, setLogDrawerOpen] = useState(false)
  const alertCount = useAlerts((s) => s.count)
  const alertSeverity = useAlerts((s) => s.highest)
  const view = useUiStore((s) => s.view)
  const setView = useUiStore((s) => s.setView)
  const project = useProjectStore((s) => s.project)
  const link = useLinkStore((s) => s.link)
  const heartbeatAgeMs = useHeartbeatAge()
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)
  const sys = useTelemetryStore((s) => s.snapshot?.sys_status ?? null)
  // The Preflight tab's `9/11` badge (U1): the same numbers the stage panel
  // and the arm gate read, so the tab can never read better than the truth.
  const [badge, setBadge] = useState<string | null>(null)
  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    const tick = () =>
      void fetchChecklist()
        .then((c) => {
          if (!cancelled) setBadge(checklistBadge(c)?.text ?? null)
        })
        .catch(() => {
          if (!cancelled) setBadge(null)
        })
    tick()
    const timer = setInterval(tick, BADGE_POLL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  const openConnection = () => {
    setSettingsTab('connection')
    setSettingsOpen(true)
  }

  // U5: a preflight repair entry asked for the settings surface directly.
  const settingsRequest = useUiStore((s) => s.settingsRequest)
  useEffect(() => {
    if (settingsRequest) {
      setSettingsTab(settingsRequest)
      setSettingsOpen(true)
      useUiStore.getState().clearSettingsRequest()
    }
  }, [settingsRequest])

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
  const safety = getSafetyConfig()
  const batteryPct = sys?.battery_remaining_percent
  const batteryTone: 'ok' | 'warn' | 'err' | 'off' =
    batteryPct == null
      ? 'off'
      : batteryPct < safety.critical_battery_percent
        ? 'err'
        : batteryPct < safety.warn_battery_percent
          ? 'warn'
          : 'ok'
  const batteryText = sys
    ? `${(sys.battery_voltage_mv / 1000).toFixed(1)}V ${sys.battery_remaining_percent}%`
    : undefined
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

      {/* The open project (PJ3): a pill that opens the project home; the
       * measurement type shows in the survey list (one registered kind). */}
      <button
        data-testid="project-pill"
        onClick={() => setProjectOpen(true)}
        title={project?.crs ?? t('project.title')}
        className={`flex ${CONTROL} max-w-[180px] items-center gap-1.5 rounded border px-2.5 text-xs ${
          project ? 'border-line bg-canvas text-ink hover:bg-panel' : 'border-dashed border-line text-muted hover:text-ink'
        }`}
      >
        <span className="truncate">{project ? project.name : t('project.noProject')}</span>
      </button>

      <Tabs
        items={VIEWS.map((v) => ({
          id: v,
          label:
            v === 'preflight' && badge !== null ? `${t(`nav.${v}`)} ${badge}` : t(`nav.${v}`),
        }))}
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

        {/* Status group (U2): armed / mode / link. Never folds — the whole
         * point of the group is that its three answers are always on screen. */}
        <div data-testid="topbar-status" className="flex items-center gap-2">
          <div
            data-testid="link-stale"
            data-stale={stale ? 'true' : 'false'}
            className={`flex items-center gap-2 ${stale ? 'opacity-45' : ''}`}
          >
            <div
              role="status"
              className={`flex ${CONTROL} items-center gap-1.5 rounded border px-2.5 text-xs font-medium ${
                armed ? 'border-ok bg-ok text-ok-ink' : 'border-line bg-canvas text-muted'
              }`}
            >
              {armed ? <ShieldCheck size={13} /> : <ShieldOff size={13} />}
              {armed ? t('link.armed') : t('link.disarmed')}
            </div>
            <FlightMenu />
            <ModePopover />
          </div>
          <LinkPopover onOpenSettings={openConnection} />
        </div>

        <GroupDivider />

        {/* L2 safety pair: never folds, never dims (an RTL is exactly when the
         * link is suspect). Placed with the status group so a narrow bar still
         * shows status + RTL/Hold before anything else. */}
        <SafetyCommands />

        <HealthGroup
          gps={gps}
          rtkTone={rtkTone}
          batteryText={batteryText}
          batteryTone={batteryTone}
          onOpenRtk={() => setRtkDrawerOpen(true)}
        />

        <GroupDivider />

        {/* Tools group (U2). */}
        <ToolsPopover
          onInspector={openInspector}
          onMotorTest={openMotorTest}
          onParams={openParams}
          onLogs={() => setLogsOpen(true)}
          onAbout={() => setAboutOpen(true)}
        />
        {/* One badge for every window's alert count (A3): severity-coloured
         * number that opens the alert center. */}
        <span className="relative">
          <IconButton
            title={alertCount > 0 ? t('alerts.open') : t('eventLog.title')}
            active={false}
            size={32}
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
        <IconButton
          title={t('settings.menu')}
          size={32}
          data-testid="settings-open"
          onClick={() => setSettingsOpen(true)}
        >
          <Settings size={16} />
        </IconButton>
      </div>
      <ProjectHome open={projectOpen} onOpenChange={setProjectOpen} />
      <AboutDialog open={aboutOpen} onOpenChange={setAboutOpen} />
      <RtkDrawer open={rtkDrawerOpen} onClose={() => setRtkDrawerOpen(false)} />
      <AlertCenter open={logDrawerOpen} onClose={() => setLogDrawerOpen(false)} />
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

/** A10: export the diagnostics bundle and surface the folder path. */
async function exportDiagnostics(): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const bundle = await invoke<{ folder: string; files: string[] }>('diagnostics_export')
    raiseAlert({
      source: 'system',
      severity: 'info',
      text: `Diagnostics exported to ${bundle.folder} (${bundle.files.length} files)`,
      key: 'diagnostics-exported',
    })
  } catch (e) {
    raiseAlert({
      source: 'system',
      severity: 'warn',
      text: `Diagnostics export failed: ${String(e)}`,
      key: 'diagnostics-exported',
    })
  }
}

/** Tools ▾: the window launchers, one popover (motor-test plan §5). */
function ToolsPopover({
  onInspector,
  onMotorTest,
  onParams,
  onLogs,
  onAbout,
}: {
  onInspector: () => void
  onMotorTest: () => void
  onParams: () => void
  onLogs: () => void
  onAbout: () => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      const target = e.target as Node
      if (!target.isConnected) return
      if (!ref.current?.contains(target)) setOpen(false)
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
        data-testid="tools-open"
        onClick={() => setOpen((o) => !o)}
        aria-label={t('tools.popover')}
        title={t('tools.popover')}
        className={`flex ${CONTROL} items-center gap-1.5 rounded border border-line px-2.5 ${
          open ? 'bg-panel text-ink' : 'bg-canvas text-muted hover:text-ink'
        }`}
      >
        <Gauge size={16} />
        <span className="hidden text-xs 2xl:inline">{t('tools.popover')}</span>
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
          <button
            data-testid="diagnostics-export"
            onClick={() => {
              setOpen(false)
              void exportDiagnostics()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <PackageOpen size={14} />
            {t('tools.diagnostics')}
          </button>
          <button
            data-testid="about-open"
            onClick={() => {
              setOpen(false)
              onAbout()
            }}
            className="flex w-full items-center gap-2 rounded px-2 py-2 text-left text-sm text-ink hover:bg-canvas"
          >
            <BadgeInfo size={14} />
            {t('about.menu')}
          </button>
        </div>
      )}
    </div>
  )
}
