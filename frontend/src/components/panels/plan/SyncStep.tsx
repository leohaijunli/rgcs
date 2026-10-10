// Step 4 — Sync (P2 §4.2): `Upload to FC` is the primary action, `Download`
// secondary, and a ⋯ menu holds the file and destructive operations so exactly
// one clear exists per scope: `Clear local plan` and `Clear FC mission`
// (P3: three near-identical clears with different blast radius), both behind a
// two-click confirm. Progress and errors render under the primary button.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { exportPlanFile, importPlanFile } from '../../../mission/planfile'
import { compileWaypoints } from '../../../mission/compile'
import { useMissionStore } from '../../../stores/mission'
import { useLinkStore } from '../../../stores/link'

/** How long a destructive item stays armed while waiting for the second click. */
const CONFIRM_WINDOW_MS = 5000

export interface SyncStepProps {
  confirmClearPlan: boolean
  setConfirmClearPlan: (v: boolean) => void
  confirmClearFc: boolean
  setConfirmClearFc: (v: boolean) => void
  /** Import notices render in the Waypoints step. */
  onImportNotice?: (items: string[]) => void
}

export default function SyncStep({
  confirmClearPlan,
  setConfirmClearPlan,
  confirmClearFc,
  setConfirmClearFc,
  onImportNotice,
}: SyncStepProps) {
  const { t } = useTranslation()
  const [menuOpen, setMenuOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const planTimer = useRef<number | undefined>(undefined)
  const fcTimer = useRef<number | undefined>(undefined)
  useEffect(
    () => () => {
      window.clearTimeout(planTimer.current)
      window.clearTimeout(fcTimer.current)
    },
    [],
  )

  const link = useLinkStore((s) => s.link)
  const connected = Boolean(link?.fc_alive)
  const waypoints = useMissionStore((s) => s.waypoints)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const home = useMissionStore((s) => s.home)
  const blocks = useMissionStore((s) => s.blocks)
  const planBase = useMissionStore((s) => s.planBase)
  const busy = useMissionStore((s) => s.busy)
  const syncState = useMissionStore((s) => s.syncState)
  const lastEvent = useMissionStore((s) => s.lastEvent)
  const upload = useMissionStore((s) => s.upload)
  const download = useMissionStore((s) => s.download)
  const clear = useMissionStore((s) => s.clear)
  const clearPlan = useMissionStore((s) => s.clearPlan)
  const applyImport = useMissionStore((s) => s.applyImport)

  const homeAmsl = home?.[2] ?? 0
  const hasItems =
    waypoints.length + blocks.reduce((n, b) => n + b.children.length, 0) > 0

  const armPlan = () => {
    if (!confirmClearPlan) {
      setConfirmClearPlan(true)
      window.clearTimeout(planTimer.current)
      planTimer.current = window.setTimeout(() => setConfirmClearPlan(false), CONFIRM_WINDOW_MS)
      return
    }
    window.clearTimeout(planTimer.current)
    setConfirmClearPlan(false)
    clearPlan()
  }

  const armFc = () => {
    if (!confirmClearFc) {
      setConfirmClearFc(true)
      window.clearTimeout(fcTimer.current)
      fcTimer.current = window.setTimeout(() => setConfirmClearFc(false), CONFIRM_WINDOW_MS)
      return
    }
    window.clearTimeout(fcTimer.current)
    setConfirmClearFc(false)
    setMenuOpen(false)
    void clear()
  }

  const onImport = async () => {
    setMenuOpen(false)
    const result = await importPlanFile()
    if (!result) return
    applyImport(result)
    onImportNotice?.(result.unsupported)
  }

  const onExport = async () => {
    setMenuOpen(false)
    await exportPlanFile(
      compileWaypoints(waypoints, altitudeMode, homeAmsl),
      altitudeMode,
      { base: planBase ?? undefined, home, blocks },
    )
  }

  return (
    <div className="space-y-2">
      <div className="flex items-stretch gap-1.5">
        <button
          disabled={!connected || busy || !hasItems}
          title={!connected ? t('plan.needLink') : undefined}
          onClick={() => void upload()}
          className="flex-1 rounded-md bg-accent px-2 py-1.5 text-sm text-canvas disabled:opacity-40"
        >
          {t('plan.upload')}
        </button>
        <button
          disabled={!connected || busy}
          onClick={() => void download()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.download')}
        </button>
        <div className="relative" ref={menuRef}>
          <button
            onClick={() => setMenuOpen((o) => !o)}
            title={t('plan.more')}
            aria-label={t('plan.more')}
            className="h-full rounded-md border border-line bg-panel px-2 text-sm hover:bg-canvas"
          >
            ⋯
          </button>
          {menuOpen && (
            <div className="absolute right-0 top-full z-50 mt-1 w-48 rounded border border-line bg-panel p-1 shadow-2xl">
              <MenuItem onClick={() => void onImport()}>{t('plan.import')}</MenuItem>
              <MenuItem onClick={() => void onExport()} disabled={!hasItems}>
                {t('plan.export')}
              </MenuItem>
              <div className="my-1 border-t border-line" />
              <button
                onClick={armPlan}
                className={`w-full rounded px-2 py-1.5 text-left text-xs transition-colors ${
                  confirmClearPlan ? 'bg-warn text-canvas' : 'text-ink hover:bg-canvas'
                }`}
              >
                {confirmClearPlan ? t('plan.confirmClearPlan') : t('plan.clearPlan')}
              </button>
              <button
                onClick={armFc}
                disabled={!connected || busy}
                title={!connected ? t('plan.needLink') : undefined}
                className={`mt-0.5 w-full rounded px-2 py-1.5 text-left text-xs transition-colors disabled:opacity-40 ${
                  confirmClearFc ? 'bg-error text-white' : 'text-error hover:bg-canvas'
                }`}
              >
                {confirmClearFc ? t('plan.confirmClearFc') : t('plan.clearFc')}
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Progress and errors render directly under the primary button. */}
      {(busy || lastEvent) && (
        <div className="text-xs text-muted">
          {busy ? (
            <span className="text-accent">{t(`plan.status.${syncState}`)}</span>
          ) : lastEvent && lastEvent.kind === 'failed' ? (
            <span className="text-error">
              {t('plan.failed')}: {lastEvent.message ?? '—'}
            </span>
          ) : lastEvent && lastEvent.kind === 'progress' ? (
            <span>
              {t(`plan.op.${lastEvent.op}`)} {lastEvent.sent}/{lastEvent.total}
            </span>
          ) : (
            lastEvent && <span className="text-ok">{t(`plan.done.${lastEvent.op}`)}</span>
          )}
        </div>
      )}
    </div>
  )
}

function MenuItem({
  onClick,
  disabled,
  children,
}: {
  onClick: () => void
  disabled?: boolean
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="w-full rounded px-2 py-1.5 text-left text-xs text-ink hover:bg-canvas disabled:opacity-40"
    >
      {children}
    </button>
  )
}
