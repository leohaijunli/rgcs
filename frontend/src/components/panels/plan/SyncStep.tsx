// Step 4 — Sync (P2 §4.2): `Upload to FC` is the primary action, `Download`
// secondary, and a ⋯ menu holds the file and destructive operations so exactly
// one clear exists per scope: `Clear local plan` and `Clear FC mission`
// (P3: three near-identical clears with different blast radius), both behind a
// two-click confirm. Progress and errors render under the primary button.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { exportPlanFile, importPlanFile } from '../../../mission/planfile'
import { compileWaypoints } from '../../../mission/compile'
import { useMissionStore, VERTICAL_DATUM_UNVERIFIED_ERROR } from '../../../stores/mission'
import { useTerrainStore } from '../../../stores/terrain'
import { TaskProgress } from '../../../design-system/components'
import { useLinkStore } from '../../../stores/link'
import type { SyncPhase } from '../../../mission/sync'
import SyncStatusLine from './SyncStatusLine'

/** How long a destructive item stays armed while waiting for the second click. */
const CONFIRM_WINDOW_MS = 5000

export interface SyncStepProps {
  /** Lifecycle phase + hashes (U17d), computed once in PlanSteps. */
  phase: SyncPhase
  planHash: string | null
  fcHash: string | null
  planHashFull: string | null
  fcHashFull: string | null
  confirmClearPlan: boolean
  setConfirmClearPlan: (v: boolean) => void
  confirmClearFc: boolean
  setConfirmClearFc: (v: boolean) => void
  /** Import notices render in the Waypoints step. */
  onImportNotice?: (items: string[]) => void
}

export default function SyncStep({
  phase,
  planHash,
  fcHash,
  planHashFull,
  fcHashFull,
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
  const anchorOffset = useTerrainStore((s) => s.amslAnchorOffsetM)
  const home = useMissionStore((s) => s.home)
  const blocks = useMissionStore((s) => s.blocks)
  const planBase = useMissionStore((s) => s.planBase)
  const busy = useMissionStore((s) => s.busy)
  const syncState = useMissionStore((s) => s.syncState)
  // U13: the wall clock the current busy phase started at (for the elapsed).
  const [taskStartedAtMs, setTaskStartedAtMs] = useState<number | null>(null)
  useEffect(() => {
    setTaskStartedAtMs(busy ? Date.now() : null)
  }, [busy, syncState])
  const lastEvent = useMissionStore((s) => s.lastEvent)
  const upload = useMissionStore((s) => s.upload)
  const download = useMissionStore((s) => s.download)
  const clear = useMissionStore((s) => s.clear)
  const clearPlan = useMissionStore((s) => s.clearPlan)
  const cancel = useMissionStore((s) => s.cancel)
  const cancelling = useMissionStore((s) => s.cancelRequested)
  const verifying = useMissionStore((s) => s.verifying)
  const operation = useMissionStore((s) => s.operation)
  const reconciliation = useMissionStore((s) => s.requiresReconciliation)
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
    // H7 gate: with a CGVD2013 source but no verified CGG2013/EGM96 chain,
    // the draped AMSL heights are not trustworthy — the export refuses
    // rather than shipping unverified absolute heights (plan §6 H7).
    if (useMissionStore.getState().drapeError === VERTICAL_DATUM_UNVERIFIED_ERROR) {
      onImportNotice?.([VERTICAL_DATUM_UNVERIFIED_ERROR])
      return
    }
    const drape = useMissionStore.getState().drape
    await exportPlanFile(
      compileWaypoints(waypoints, altitudeMode, homeAmsl),
      altitudeMode,
      {
        base: planBase ?? undefined,
        home,
        blocks,
        // Provenance (ADR-004): the DEM the drape was solved against — its
        // content/provenance hash and the tile folder. The 3D mesh is
        // display-only since A9, so it is no longer a drape reference.
        demVersion: drape?.demVersion ?? null,
        terrainRef: drape ? `dem:${drape.folder}` : null,
      },
    )
  }

  return (
    <div className="space-y-2" data-plan-field="sync">
      {hasItems && (
        <SyncStatusLine
          phase={phase}
          planHash={planHash}
          fcHash={fcHash}
          planHashFull={planHashFull}
          fcHashFull={fcHashFull}
        />
      )}
      {hasItems && anchorOffset != null && Math.abs(anchorOffset) >= 0.05 && (
        // The upload compensates the measured EKF-origin bias so the vehicle's
        // TRUE altitude lands on the planned one (field report 2026-10-21).
        // Shown for the audit: planned heights stay untouched on disk/export.
        <div className="text-[11px] text-muted" data-testid="anchor-compensation-note">
          {t('plan.anchorCompensation', { offset: anchorOffset.toFixed(1) })}
        </div>
      )}
      <div className="flex items-stretch gap-1.5">
        <button
          data-testid="mission-upload"
          disabled={!connected || busy || !hasItems}
          title={!connected ? t('plan.needLink') : undefined}
          onClick={() => void upload()}
          className="flex-1 rounded-md bg-accent px-2 py-1.5 text-sm text-canvas disabled:opacity-40"
        >
          {t('plan.upload')}
        </button>
        <button
          data-testid="mission-download"
          disabled={!connected || busy}
          onClick={() => void download()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.download')}
        </button>
        <div className="relative" ref={menuRef}>
          <button
            data-testid="mission-more"
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
                data-testid="mission-clear-local"
                onClick={armPlan}
                className={`w-full rounded px-2 py-1.5 text-left text-xs transition-colors ${
                  confirmClearPlan ? 'bg-warn text-canvas' : 'text-ink hover:bg-canvas'
                }`}
              >
                {confirmClearPlan ? t('plan.confirmClearPlan') : t('plan.clearPlan')}
              </button>
              <button
                data-testid="mission-clear-fc"
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
      {busy ? (
        // Cancellation retires the exact owned local exchange; the retained
        // result separately reports whether FC state still needs read-back.
        <TaskProgress
          label={cancelling ? t('plan.cancellingMission') : verifying ? t('plan.sync.verifying')
            : lastEvent?.op === 'set_current' ? t('plan.settingCurrent') : t(`plan.status.${syncState}`)}
          sent={lastEvent?.kind === 'progress' ? lastEvent.sent ?? undefined : undefined}
          total={lastEvent?.kind === 'progress' ? lastEvent.total ?? undefined : undefined}
          indeterminate={lastEvent?.kind !== 'progress'}
          startedAtMs={taskStartedAtMs}
          onCancel={cancelling ? undefined : () => void cancel()}
        />
      ) : lastEvent && lastEvent.kind === 'failed' ? (
        <div data-testid="mission-error" className={`rounded border px-2 py-1 text-[11px] ${reconciliation ? 'border-warn text-warn' : 'border-error text-error'}`}>
          {t('plan.failed')}: {lastEvent.message ?? '—'}
        </div>
      ) : (
        lastEvent && (
          <div className="text-[11px] text-ok">{t(`plan.done.${lastEvent.op}`)}</div>
        )
      )}
      {operation && (
        <div data-testid="mission-operation-id" title={`${operation.id} · generation ${operation.target.generation}`}
          className="mono truncate text-[10px] text-muted">
          {t('plan.missionOperation', { operation: operation.id, generation: operation.target.generation })}
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
