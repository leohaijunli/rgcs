// The U5 preflight checklist surface: the full evaluated checklist, grouped
// and with a readiness badge, plus the two ways forward when the gate blocks
// an arm/takeoff.
//
//   Pass       — green dot
//   Warn       — amber dot; must be confirmed item-by-item before arming
//   Fail/Unknown — red/muted dot with the reason and a repair entry; the only
//              way past is the forced arm (keyword `FORCE ARM` + fixed reason
//              + optional note), which bypasses **only** the GCS checklist —
//              PX4 prearm still applies (no 21196).
//
// The gate is enforced in core (`flight_command_execute`); this dialog is the
// operator's way to see the reasons and to pass the gate with intent.

import { useEffect, useState, type ReactNode } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { ShieldAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { Checklist } from '../generated-types/Checklist'
import type { ChecklistItem } from '../generated-types/ChecklistItem'
import type { ChecklistGroup } from '../generated-types/ChecklistGroup'
import { useUiStore } from '../stores/ui'
import type { ForceArmReason } from '../generated-types/ForceArmReason'
import { invoke } from '@tauri-apps/api/core'
import { useCommandStore } from '../stores/command'
import { useTelemetryStore } from '../stores/telemetry'
import { useMissionStore } from '../stores/mission'
import { fetchChecklist, gateStatus, overallOf, usePreflightUi } from '../stores/preflight'

/** Keep in sync with `core::safety::FORCE_ARM_KEYWORD`. Uppercase on purpose:
 * the comparison is verbatim, so a stray Enter cannot confirm. */
const FORCE_ARM_KEYWORD = 'FORCE ARM'

const REASONS: Array<{ id: ForceArmReason; label: string }> = [
  { id: 'safety_override', label: 'On-site safety override' },
  { id: 'weather', label: 'Weather' },
  { id: 'sensor_timeout', label: 'Sensor data gap' },
  { id: 'time_pressure', label: 'Time pressure' },
]

/** Groups in display order; only groups with items render. Magnetic/diurnal
 * arrive with the M2/M7 profiles. */
const GROUP_ORDER: ChecklistGroup[] = ['link', 'navigation', 'power', 'system', 'mission', 'terrain']

/** Where a repair entry sends the operator, closing the panel first. */
function navigateFor(
  view: 'prepare' | 'plan',
  step?: 'area' | 'pattern',
  field?: string,
): void {
  const ui = useUiStore.getState()
  ui.setView(view)
  if (step) ui.focusPlanField(step, field ?? step)
}

/** A repair entry for a failed item: a real action where one exists, else just
 * the hint text. Environmental Fails (GPS fix, battery) have no GCS-side
 * repair — the hint is the entry point. */
function repairFor(
  item: ChecklistItem,
  t: (key: string, opts?: Record<string, unknown>) => string,
  after: () => void,
  close: () => void,
): { label: string; action: (() => void) | null } | null {
  const go = (fn: () => void) => () => {
    after()
    // Entries that navigate away must not leave the dialog covering the view.
    close()
    fn()
  }
  switch (item.id) {
    case 'link_fresh':
      return {
        label: t('preflight.fix.openConnection'),
        action: go(() => useUiStore.getState().openSettings('connection')),
      }
    case 'mission_ready':
      return { label: t('preflight.fix.uploadMission'), action: go(() => void useMissionStore.getState().upload()) }
    case 'motor_test_idle':
      return { label: t('preflight.fix.stopMotorTest'), action: go(() => void invoke('actuator_stop')) }
    case 'sensor_health':
      return { label: t('preflight.fix.openInspector'), action: go(() => void invoke('inspector_open')) }
    case 'home_position': {
      const fix = useTelemetryStore.getState().snapshot?.global_position
      if (!fix) return null
      return {
        label: t('preflight.fix.setHome'),
        action: go(() => void useCommandStore.getState().sendSetHome(fix.latitude_deg, fix.longitude_deg)),
      }
    }
    case 'dem_coverage':
      return {
        label: t('preflight.fix.prepareSurvey'),
        action: go(() => navigateFor('prepare', 'area')),
      }
    case 'drape_thresholds':
    case 'drape_item_count':
      return {
        label: t('preflight.fix.openDrapeParams'),
        action: go(() => navigateFor('plan', 'pattern', 'drape-params')),
      }
    default:
      return null
  }
}

/**
 * The out-of-band span acceptance (U5/DR4): the solver's infeasible spans are
 * real terrain constraints the checklist cannot wave away — the operator must
 * look at them (line, extent, cause, worst deficit) and accept *this* solve.
 * A new solve invalidates the acceptance by `spec_hash` comparison.
 */
function InfeasibleAccept({ onAccepted }: { onAccepted: () => void }) {
  const { t } = useTranslation()
  const preview = useMissionStore((s) => s.drapePreview)
  const acceptedFor = useMissionStore((s) => s.infeasibleAcceptedFor)
  const accept = useMissionStore((s) => s.acceptInfeasible)
  const spans = preview?.result.infeasible ?? []
  if (spans.length === 0) return null
  const accepted = acceptedFor === preview?.result.spec_hash
  return (
    <div className="mt-1 space-y-1">
      {spans.map((sp, i) => (
        <div key={i} className="text-muted">
          {t('preflight.infeasibleSpan', {
            line: sp.line_id,
            from: Math.round(sp.s_start),
            to: Math.round(sp.s_end),
            reason: t(`plan.pattern.drapePreview.reason.${sp.reason}`),
            deficit: sp.max_deficit_m.toFixed(1),
          })}
        </div>
      ))}
      <button
        data-testid="preflight-accept-infeasible"
        onClick={() => {
          if (preview) accept(preview.result.spec_hash)
          onAccepted()
        }}
        disabled={accepted}
        className="touch-target rounded border border-line bg-canvas px-1.5 py-0.5 text-[11px] text-accent hover:bg-panel disabled:opacity-50"
      >
        {accepted
          ? t('preflight.infeasibleAccepted')
          : t('preflight.acceptInfeasible', { count: spans.length })}
      </button>
    </div>
  )
}

export default function PreflightPanel() {
  const { t } = useTranslation()
  const refusal = useCommandStore((s) => s.preflight)
  const dismiss = useCommandStore((s) => s.dismissPreflight)
  const sendArmDisarm = useCommandStore((s) => s.sendArmDisarm)
  const uiOpen = usePreflightUi((s) => s.open)
  const closePanel = usePreflightUi((s) => s.closePanel)
  const [checklist, setChecklist] = useState<Checklist | null>(null)
  const [fetching, setFetching] = useState(false)
  const [typed, setTyped] = useState('')
  const [reason, setReason] = useState<ForceArmReason>('safety_override')
  const [note, setNote] = useState('')
  const [ackd, setAckd] = useState<ReadonlySet<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** Bumped when a repair entry changes state the checklist reads (the
   * out-of-band acceptance): re-fetch so the row updates in place. */
  const [refresh, setRefresh] = useState(0)

  const open = refusal !== null || uiOpen
  useEffect(() => {
    if (open) return
    setChecklist(null)
    setTyped('')
    setNote('')
    setAckd(new Set())
    setBusy(false)
    setError(null)
  }, [open])

  const close = () => {
    dismiss()
    closePanel()
    // Radix keeps the dialog content mounted across open/close, so the form
    // state (busy included) must be reset here — otherwise the next time the
    // panel opens the confirm button is still stuck on its "Working…" state.
    setTyped('')
    setNote('')
    setAckd(new Set())
    setBusy(false)
    setError(null)
  }

  // Fetch the full checklist whenever the panel opens (a refusal opens it with
  // the refused subset, but the panel shows every group).
  useEffect(() => {
    if (!open) return
    let cancelled = false
    setFetching(true)
    setError(null)
    void fetchChecklist()
      .then((c) => {
        if (!cancelled) setChecklist(c)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setFetching(false)
      })
    return () => {
      cancelled = true
    }
  }, [open, refresh])

  const items = checklist?.items ?? refusal?.items ?? []
  const status = checklist ? gateStatus(checklist) : refusal ? refusal.kind : null
  const overall = checklist ? overallOf(checklist) : null
  const isBlocked = status === 'blocked'
  const isAck = status === 'requires_ack'
  const warnItems = items.filter((i) => i.status === 'warn')
  const allAckd = warnItems.length > 0 && warnItems.every((i) => ackd.has(i.id))
  const keywordOk = isBlocked && typed === FORCE_ARM_KEYWORD
  const ready = isBlocked ? keywordOk : isAck ? allAckd : false

  const confirm = async () => {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    try {
      let accepted: boolean
      if (isBlocked) {
        accepted = await sendArmDisarm(true, { reason, note: note.trim() === '' ? null : note.trim() })
      } else {
        // The operator confirmed every `Warn` item in the panel: pass the
        // acknowledged ids so the backend's gate treats them as Pass (U5) —
        // otherwise it would re-refuse the same warnings it just asked for.
        accepted = await sendArmDisarm(true, undefined, warnItems.map((i) => i.id))
      }
      if (accepted) {
        close()
      } else {
        const event = useCommandStore.getState().lastEvent
        setError(event?.message ?? event?.result ?? t('preflight.armRefused'))
        setBusy(false)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  const passCount = items.filter((i) => i.status === 'pass').length
  const badge =
    overall === 'pass'
      ? 'text-ok'
      : overall === 'warn'
        ? 'text-warn'
        : overall === 'unknown'
          ? 'text-muted'
          : 'text-error'

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="preflight-dialog"
          className="fixed left-1/2 top-1/2 z-50 max-h-[88vh] w-[min(94vw,520px)] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-lg border border-line bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="flex items-center justify-between text-sm font-medium text-ink">
            <span>{t('preflight.title')}</span>
            {checklist && (
              <span data-testid="preflight-ready" className={`mono text-xs ${badge}`}>
                {t('preflight.ready', { pass: passCount, total: items.length })}
              </span>
            )}
          </Dialog.Title>
          <Dialog.Description className="sr-only">{t('preflight.title')}</Dialog.Description>

          {fetching && items.length === 0 && (
            <div className="mt-3 text-xs text-muted">{t('preflight.checking')}</div>
          )}

          {refusal !== null && (
            <div className="mt-3 flex items-start gap-2 rounded border border-error px-3 py-2 text-xs text-error">
              <ShieldAlert size={14} className="mt-0.5 shrink-0" />
              <span>{refusal.message}</span>
            </div>
          )}

          {items.length > 0 && (
            <div className="mt-3 space-y-3">
              {GROUP_ORDER.map((group) => {
                const groupItems = items.filter((i) => i.group === group)
                if (groupItems.length === 0) return null
                return (
                  <div key={group} className="rounded border border-line px-2.5 py-2">
                    <div className="text-xs font-medium text-muted">
                      {t(`preflight.group.${group}`)}
                    </div>
                    <ul className="mt-1.5 space-y-1.5">
                      {groupItems.map((item) => (
                        <li key={item.id} className="flex items-start gap-2 text-xs">
                          <span aria-hidden className={statusDot(item.status)} />
                          <div className="min-w-0 flex-1">
                            <div className="text-ink">{item.detail}</div>
                            {item.id === 'drape_infeasible' && item.status !== 'pass' && (
                              <InfeasibleAccept onAccepted={() => setRefresh((n) => n + 1)} />
                            )}
                            {item.id !== 'drape_infeasible' &&
                              item.status !== 'pass' &&
                              renderRepair(item, t, () => setRefresh((n) => n + 1), close)}
                          </div>
                          {isAck && item.status === 'warn' && (
                            <input
                              type="checkbox"
                              aria-label={t('preflight.ackItem')}
                              checked={ackd.has(item.id)}
                              onChange={(e) => {
                                const next = new Set(ackd)
                                if (e.target.checked) next.add(item.id)
                                else next.delete(item.id)
                                setAckd(next)
                              }}
                            />
                          )}
                        </li>
                      ))}
                    </ul>
                  </div>
                )
              })}
            </div>
          )}

          {isAck && warnItems.length > 0 && (
            <div className="mt-3 text-xs text-warn">{t('preflight.ackHint')}</div>
          )}

          {isBlocked && (
            <div className="mt-3 space-y-2.5">
              <div className="text-xs text-warn">{t('preflight.forceHint')}</div>
              <label className="block text-xs text-muted">
                <span className="mb-1 block">
                  {t('preflight.typeKeyword', { keyword: FORCE_ARM_KEYWORD })}
                </span>
                <input
                  data-testid="preflight-keyword"
                  value={typed}
                  autoFocus
                  spellCheck={false}
                  autoComplete="off"
                  onChange={(e) => setTyped(e.target.value)}
                  placeholder={FORCE_ARM_KEYWORD}
                  className="mono h-9 w-full rounded border border-line bg-canvas px-2 text-sm text-ink outline-none focus:border-accent"
                />
              </label>
              <label className="block text-xs text-muted">
                <span className="mb-1 block">{t('preflight.reason')}</span>
                <select
                  data-testid="preflight-reason"
                  value={reason}
                  onChange={(e) => setReason(e.target.value as ForceArmReason)}
                  className="h-9 w-full rounded border border-line bg-canvas px-2 text-sm text-ink outline-none focus:border-accent"
                >
                  {REASONS.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="block text-xs text-muted">
                <span className="mb-1 block">{t('preflight.note')}</span>
                <input
                  data-testid="preflight-note"
                  value={note}
                  spellCheck={false}
                  onChange={(e) => setNote(e.target.value)}
                  placeholder={t('preflight.notePlaceholder')}
                  className="h-9 w-full rounded border border-line bg-canvas px-2 text-sm text-ink outline-none focus:border-accent"
                />
              </label>
            </div>
          )}

          {error !== null && (
            <div data-testid="preflight-error" className="mt-3 text-xs text-error">
              {t('preflight.failed', { message: error })}
            </div>
          )}

          <div className="mt-4 flex justify-end gap-2">
            <button
              data-testid="preflight-cancel"
              onClick={close}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
            >
              {t('preflight.cancel')}
            </button>
            {isBlocked && (
              <button
                data-testid="preflight-confirm"
                onClick={() => void confirm()}
                disabled={!ready || busy}
                title={!keywordOk ? t('preflight.keywordRequired') : undefined}
                className="touch-target h-9 rounded border border-error bg-canvas px-3 text-xs font-medium text-error disabled:opacity-50"
              >
                {busy ? t('preflight.working') : t('preflight.forceArm')}
              </button>
            )}
            {isAck && (
              <button
                data-testid="preflight-confirm"
                onClick={() => void confirm()}
                disabled={!allAckd || busy}
                className="touch-target h-9 rounded border border-error bg-canvas px-3 text-xs font-medium text-error disabled:opacity-50"
              >
                {busy ? t('preflight.working') : t('preflight.ackArm')}
              </button>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function statusDot(status: Checklist['items'][number]['status']): string {
  switch (status) {
    case 'pass':
      return 'mt-1 h-2 w-2 shrink-0 rounded-full bg-ok'
    case 'warn':
      return 'mt-1 h-2 w-2 shrink-0 rounded-full bg-warn'
    case 'fail':
      return 'mt-1 h-2 w-2 shrink-0 rounded-full bg-error'
    case 'unknown':
      return 'mt-1 h-2 w-2 shrink-0 rounded-full bg-muted'
  }
}

function renderRepair(
  item: ChecklistItem,
  t: (key: string, opts?: Record<string, unknown>) => string,
  after: () => void,
  close: () => void,
): ReactNode {
  if (item.fix_hint == null) return null
  const entry = repairFor(item, t, after, close)
  if (entry) {
    return (
      <button
        data-testid={`preflight-fix-${item.id}`}
        onClick={() => entry.action?.()}
        className="mt-0.5 rounded border border-line bg-canvas px-1.5 py-0.5 text-[11px] text-accent hover:bg-panel"
      >
        {entry.label}
      </button>
    )
  }
  return <div className="mt-0.5 text-muted">{item.fix_hint}</div>
}
