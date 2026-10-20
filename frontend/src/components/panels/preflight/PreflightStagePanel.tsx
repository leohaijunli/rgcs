// Stage 3 — Preflight (U1): the live checklist (U5), polled from the A5
// engine. Read-only on purpose: the arm/force-arm flow stays in the arm-gate
// dialog (the checklist here tells the operator what will block before they
// try), and the badge on the TopBar's Preflight tab carries the same numbers.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { fetchChecklist } from '../../../stores/preflight'
import { isTauri } from '../../../inspector/mock'
import type { Checklist } from '../../../generated-types/Checklist'
import type { ChecklistStatus } from '../../../generated-types/ChecklistStatus'
import { StatusDot } from '../../../design-system/components'

/** Re-fetch cadence; the engine is pure, so polling is free. */
const POLL_MS = 2_000

const DOT: Record<ChecklistStatus, 'ok' | 'warn' | 'error' | 'muted'> = {
  pass: 'ok',
  warn: 'warn',
  fail: 'error',
  unknown: 'muted',
}

/** The `9/11` readiness badge (U1/U5): passes over evaluated items. */
export function checklistBadge(
  checklist: Checklist | null,
): { text: string; allPass: boolean } | null {
  if (!checklist || checklist.items.length === 0) return null
  const pass = checklist.items.filter((i) => i.status === 'pass').length
  return { text: `${pass}/${checklist.items.length}`, allPass: pass === checklist.items.length }
}

export default function PreflightStagePanel() {
  const { t } = useTranslation()
  const [checklist, setChecklist] = useState<Checklist | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    const tick = () => {
      void fetchChecklist()
        .then((c) => {
          if (!cancelled) {
            setChecklist(c)
            setError(null)
          }
        })
        .catch((e) => {
          if (!cancelled) setError(e instanceof Error ? e.message : String(e))
        })
    }
    tick()
    const timer = setInterval(tick, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  const badge = checklistBadge(checklist)
  return (
    <div className="space-y-2" data-testid="preflight-stage-panel">
      <div className="flex items-center justify-between text-xs">
        <span className="text-muted">{t('preflight.overall')}</span>
        {badge && (
          <span
            data-testid="preflight-badge"
            className={`mono rounded border px-1.5 py-0.5 text-[11px] font-semibold ${
              badge.allPass ? 'border-ok text-ok' : 'border-warn text-warn'
            }`}
          >
            {badge.text}
          </span>
        )}
      </div>
      {error && <div className="text-[11px] text-warn">{error}</div>}
      {!isTauri() && <div className="text-[11px] text-muted">{t('preflight.noLink')}</div>}
      <div className="space-y-1">
        {checklist?.items.map((item) => (
          <div
            key={item.id}
            className="flex items-start gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs"
            title={item.fix_hint ?? item.detail}
          >
            <span className="mt-0.5">
              <StatusDot tone={DOT[item.status]} />
            </span>
            <span className="min-w-0 flex-1 text-ink">{item.detail}</span>
          </div>
        ))}
      </div>
    </div>
  )
}
