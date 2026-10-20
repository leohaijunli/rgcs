// Plan view, right panel (P2 §4.2): a header that is always visible (altitude
// mode + undo/redo) above the always-visible summary strip and four collapsible
// step cards, exactly one expanded at a time. The step order is the workflow
// order: area → pattern → waypoints → sync. U17: each step card carries its
// workflow number and a status dot (readiness.ts), and the summary strip shows
// area/pattern/items/datum/sync without opening a card.

import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { planSyncStatus, planSyncPhase } from '../../../mission/sync'
import { itemsHash, shortHash } from '../../../mission/hash'
import { planSteps, planBlockers } from '../../../mission/readiness'
import { selfIntersects } from '../../../mission/polygon'
import { useTelemetryStore } from '../../../stores/telemetry'
import { getPlanPrefs } from '../../../desktop/appConfig'
import AltitudeProfile from './AltitudeProfile'
import { useMissionStore, type AltitudeMode } from '../../../stores/mission'
import { useUiStore, type PlanStep } from '../../../stores/ui'
import { usePolygonStore } from '../../../stores/polygon'
import AreaStep from './AreaStep'
import PatternStep, { type PatternKind } from './PatternStep'
import PlanStepCard from './PlanStepCard'
import PlanSummary from './PlanSummary'
import WaypointsStep from './WaypointsStep'
import SyncStep from './SyncStep'
import type { CloverleafPattern } from '../../../generated-types/CloverleafPattern'
import type { SurveyPattern } from '../../../generated-types/SurveyPattern'
import { defaultCloverleaf, defaultSweep } from '../../../mission/patterns'

const BASE_STEPS: Array<{ id: PlanStep; titleKey: string }> = [
  { id: 'area', titleKey: 'plan.step.area' },
  { id: 'pattern', titleKey: 'plan.step.pattern' },
  { id: 'waypoints', titleKey: 'plan.step.waypoints' },
  { id: 'sync', titleKey: 'plan.step.sync' },
]

const MODES: AltitudeMode[] = ['relative', 'amsl', 'agl', 'terrain']

/** Default clearance above home for a generated pattern. */
const DEFAULT_ALT_AGL_M = 40
const ORIGIN = { latitude_deg: 0, longitude_deg: 0 }

export default function PlanSteps() {
  const { t } = useTranslation()
  const planStep = useUiStore((s) => s.planStep)
  const setPlanStep = useUiStore((s) => s.setPlanStep)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const setAltitudeMode = useMissionStore((s) => s.setAltitudeMode)
  const waypoints = useMissionStore((s) => s.waypoints)
  const blocks = useMissionStore((s) => s.blocks)
  const home = useMissionStore((s) => s.home)
  const dirty = useMissionStore((s) => s.dirty)
  const fcMatches = useMissionStore((s) => s.fcMatches)
  const lastSyncedHash = useMissionStore((s) => s.lastSyncedHash)
  const verifying = useMissionStore((s) => s.verifying)
  const busy = useMissionStore((s) => s.busy)
  const syncState = useMissionStore((s) => s.syncState)
  const planBase = useMissionStore((s) => s.planBase)
  const undo = useMissionStore((s) => s.undo)
  const redo = useMissionStore((s) => s.redo)
  const canUndo = useMissionStore((s) => s.undoStack.length > 0)
  const canRedo = useMissionStore((s) => s.redoStack.length > 0)
  // U17: the step statuses and the summary strip are derived from these facts.
  const patternCenter = useMissionStore((s) => s.patternCenter)
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const drapeError = useMissionStore((s) => s.drapeError)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const planFocus = useUiStore((s) => s.planFocus)
  const focusPlanField = useUiStore((s) => s.focusPlanField)
  const clearPlanFocus = useUiStore((s) => s.clearPlanFocus)

  // Pattern form state, shared by the Area and Pattern steps (the boundary
  // feeds Generate).
  const [kind, setKind] = useState<PatternKind>('sweep')
  // A cloverleaf is a calibration manoeuvre, not an area survey: the drawn
  // polygon step is meaningless for it, so the flow skips Area entirely
  // (operator request). Switching back to sweep restores it.
  const steps = kind === 'cloverleaf' ? BASE_STEPS.filter((x) => x.id !== 'area') : BASE_STEPS
  const [sweep, setSweep] = useState<SurveyPattern>(() =>
    defaultSweep(ORIGIN, (home?.[2] ?? 0) + DEFAULT_ALT_AGL_M),
  )
  const [clover, setClover] = useState<CloverleafPattern>(() =>
    defaultCloverleaf(ORIGIN, (home?.[2] ?? 0) + DEFAULT_ALT_AGL_M),
  )
  // U8: the survey area defaults to the 100 x 100 m survey scope (plan
  // decision 3), the rectangle's azimuth rotates the boundary (ENU). Held in
  // the ui store so the 3D-mesh / prepare actions target the same AOI.
  const size = useUiStore((s) => s.surveySize)
  const setSize = useUiStore((s) => s.setSurveySize)
  const heading = useUiStore((s) => s.surveyHeading)
  const setHeading = useUiStore((s) => s.setSurveyHeading)
  // U8 default centre, seeded once on mount: the live vehicle position →
  // HOME → the last used survey centre (persisted config) → the map view's
  // centre (the Area step's standing fallback).
  useEffect(() => {
    const mission = useMissionStore.getState()
    if (mission.patternCenter) return
    const vehicle = useTelemetryStore.getState().snapshot?.global_position
    if (vehicle) {
      mission.setPatternCenter({ latitude_deg: vehicle.latitude_deg, longitude_deg: vehicle.longitude_deg })
      return
    }
    if (mission.home) {
      mission.setPatternCenter({ latitude_deg: mission.home[0], longitude_deg: mission.home[1] })
      return
    }
    const last = getPlanPrefs().last_center
    if (last) mission.setPatternCenter({ latitude_deg: last[0], longitude_deg: last[1] })
    // No vehicle, no HOME, no history: keep null — the Area step falls back
    // to the map centre and the operator can type one.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Two-click confirm state lives here so collapsing the Sync card never
  // leaves a stale armed button.
  const [confirmClearPlan, setConfirmClearPlan] = useState(false)
  const [confirmClearFc, setConfirmClearFc] = useState(false)
  // Import notices render in the Waypoints step.
  const [importNotice, setImportNotice] = useState<string[]>([])

  // U17b: reveal the field a blocker chip points at — scroll it into view,
  // flash it, and move keyboard focus there. `planFocus.nonce` changes on
  // every click, so clicking the same chip twice re-runs this.
  useEffect(() => {
    if (!planFocus) return
    const el = document.querySelector<HTMLElement>(`[data-plan-field="${planFocus.field}"]`)
    if (!el) {
      clearPlanFocus()
      return
    }
    el.scrollIntoView({ block: 'center', behavior: 'smooth' })
    el.setAttribute('tabindex', '-1')
    el.focus({ preventScroll: true })
    el.classList.add('plan-field-flash')
    const timer = window.setTimeout(() => {
      el.classList.remove('plan-field-flash')
      clearPlanFocus()
    }, 1800)
    return () => {
      window.clearTimeout(timer)
      el.classList.remove('plan-field-flash')
    }
  }, [planFocus, clearPlanFocus])

  const itemCount = waypoints.length + blocks.reduce((n, b) => n + b.children.length, 0)
  const sync = planSyncStatus({ itemCount, dirty, fcMatches, lastSyncedHash })
  // U17d: the lifecycle phase (adds the in-flight upload/read-back states) and
  // the compact plan / FC hashes for the sync line. `flyable()` is the store's
  // canonical compiled list, so the hash matches what an upload would send.
  const phase = planSyncPhase({
    itemCount,
    dirty,
    fcMatches,
    lastSyncedHash,
    busy,
    syncState,
    verifying,
  })
  const { planHash, planHashFull, fcHash, fcHashFull } = useMemo(() => {
    const items = useMissionStore.getState().flyable()
    if (items.length === 0) {
      return { planHash: null, planHashFull: null, fcHash: null, fcHashFull: null }
    }
    const canonical = itemsHash(items)
    return {
      planHash: shortHash(canonical),
      planHashFull: canonical,
      fcHash: lastSyncedHash ? shortHash(lastSyncedHash) : null,
      fcHashFull: lastSyncedHash,
    }
  }, [waypoints, altitudeMode, home, blocks, planBase, lastSyncedHash])
  // U17: every step's status comes from the same facts (readiness.ts) so the
  // dots, the summary and the (later) blocker navigation cannot drift apart.
  const polygonValid =
    polygonClosed && polygonVertices.length >= 3 && !selfIntersects(polygonVertices)
  const facts = {
    hasArea: polygonValid || patternCenter != null || mapCenter != null,
    hasPattern: lastPattern != null,
    itemCount,
    sync,
    drapeError,
  }
  const stepStates = planSteps(facts)
  const blockers = planBlockers(facts)
  const statusById = new Map(stepStates.map((s) => [s.id, s.status]))

  return (
    <div className="flex h-full flex-col gap-2">
      {/* Header: always visible. Altitude mode + undo/redo; the sync state
       * lives in the summary strip below (U17). */}
      <div className="panel rounded p-2">
        <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">{t('plan.altitude')}</div>
        <div className="grid grid-cols-2 gap-0.5 rounded-md border border-line bg-canvas p-0.5 text-xs">
          {MODES.map((m) => (
            <button
              key={m}
              onClick={() => setAltitudeMode(m)}
              className={`rounded px-1 py-1 transition-colors ${
                altitudeMode === m ? 'bg-accent text-canvas' : 'text-muted hover:text-ink'
              }`}
            >
              {t(`plan.mode.${m}`)}
            </button>
          ))}
        </div>
        <div className="mt-1 text-[10px] text-muted" title={t(`plan.modeHint.${altitudeMode}`)}>
          {t(`plan.modeHint.${altitudeMode}`)}
        </div>
        <div className="mt-1 flex items-center gap-1">
          {/* Plan-edit undo/redo (U8). The buttons mirror Ctrl+Z / Ctrl+Shift+Z. */}
          <button
            onClick={undo}
            disabled={!canUndo}
            title={t('plan.undoHint')}
            className="rounded border border-line bg-canvas px-1.5 py-0.5 text-xs text-ink hover:bg-panel disabled:opacity-40"
          >
            ↶ {t('plan.undo')}
          </button>
          <button
            onClick={redo}
            disabled={!canRedo}
            title={t('plan.redoHint')}
            className="rounded border border-line bg-canvas px-1.5 py-0.5 text-xs text-ink hover:bg-panel disabled:opacity-40"
          >
            ↷ {t('plan.redo')}
          </button>
        </div>
      </div>

      {/* U17: the always-visible plan summary — area, pattern, items, datum
       * and FC sync state in one strip, above the step cards. */}
      <PlanSummary
        kind={kind}
        size={size}
        heading={heading}
        lineSpacingM={sweep.line_spacing_m}
        itemCount={itemCount}
        altitudeMode={altitudeMode}
        phase={phase}
        planHash={planHash}
        fcHash={fcHash}
        planHashFull={planHashFull}
        fcHashFull={fcHashFull}
        blockers={blockers}
        onFix={(b) => focusPlanField(b.step, b.field)}
      />

      {steps.map(({ id, titleKey }, i) => {
        const status = statusById.get(id) ?? 'todo'
        return (
        <PlanStepCard
          key={id}
          index={i + 1}
          title={t(titleKey)}
          status={status}
          statusLabel={t(`plan.stepStatus.${status}`)}
          open={planStep === id}
          onToggle={() => setPlanStep(id)}
        >
          {id === 'area' ? <AreaStep size={size} onSize={setSize} heading={heading} onHeading={setHeading} /> : null}
          {id === 'pattern' ? (
            <PatternStep
              kind={kind}
              onKind={(k) => {
                setKind(k)
                if (k === 'cloverleaf' && useUiStore.getState().planStep === 'area') {
                  useUiStore.getState().setPlanStep('pattern')
                }
              }}
              sweep={sweep}
              onSweep={setSweep}
              clover={clover}
              onClover={setClover}
              size={size}
              onSize={setSize}
              heading={heading}
              onHeading={setHeading}
            />
          ) : null}
          {id === 'waypoints' ? (
            <WaypointsStep importNotice={importNotice} />
          ) : null}
          {id === 'sync' ? (
            <SyncStep
              phase={phase}
              planHash={planHash}
              fcHash={fcHash}
              planHashFull={planHashFull}
              fcHashFull={fcHashFull}
              confirmClearPlan={confirmClearPlan}
              setConfirmClearPlan={setConfirmClearPlan}
              confirmClearFc={confirmClearFc}
              setConfirmClearFc={setConfirmClearFc}
              onImportNotice={setImportNotice}
            />
          ) : null}
        </PlanStepCard>
        )
      })}

      {/* Design-stage altitude profile: planned line vs the DEM ground
       * under it — the clearance/undulation check without the globe. */}
      <AltitudeProfile />
    </div>
  )
}
