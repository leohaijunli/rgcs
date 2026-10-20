// Plan step card (U17): the collapsible shell shared by all four planning
// steps, carrying the workflow number and a status dot so the operator can
// read "where am I / what still needs attention" without opening each card.

import type { StepStatus } from '../../../mission/readiness'

/** Glyph + tone of each status (the text comes from `plan.stepStatus.*`). */
const DOT: Record<StepStatus, { cls: string; glyph: string }> = {
  done: { cls: 'text-ok', glyph: '✓' },
  blocked: { cls: 'text-error', glyph: '!' },
  todo: { cls: 'text-muted/60', glyph: '○' },
}

export interface PlanStepCardProps {
  /** Position in the visible workflow (1-based; cloverleaf skips Area). */
  index: number
  title: string
  status: StepStatus
  /** Spoken/tooltip label for the status dot. */
  statusLabel: string
  open: boolean
  onToggle: () => void
  children: React.ReactNode
}

export default function PlanStepCard({
  index,
  title,
  status,
  statusLabel,
  open,
  onToggle,
  children,
}: PlanStepCardProps) {
  const dot = DOT[status]
  return (
    <div className="panel rounded">
      <button
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center gap-2.5 px-2.5 py-2 text-left text-sm"
      >
        <span
          className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full border text-[11px] ${
            status === 'done' ? 'border-ok/40 bg-ok/15 text-ok' : 'border-line text-muted'
          }`}
        >
          {index}
        </span>
        <span className="flex-1 font-medium text-ink">{title}</span>
        <span className={`shrink-0 text-xs ${dot.cls}`} title={statusLabel} aria-label={statusLabel}>
          {dot.glyph}
        </span>
        <span className={`text-muted transition-transform ${open ? 'rotate-90' : ''}`}>▸</span>
      </button>
      {open && <div className="border-t border-line p-2.5">{children}</div>}
    </div>
  )
}
