// MagGCS design-system React components (UI2): the primitives every window
// shares. Tokens only — no hardcoded colours (check:colors lint); variants
// map onto the Tailwind theme defined in index.css.

import type { ButtonHTMLAttributes, ReactNode } from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'

/** Shared size scale for buttons (touch targets stay ≥ 32px tall). */
const SIZES = {
  sm: 'h-7 px-2 text-xs',
  md: 'h-8 px-2.5 text-sm',
} as const

const VARIANTS = {
  default: 'border border-line bg-canvas text-muted hover:text-ink',
  primary: 'border border-accent bg-accent text-accent-ink font-medium hover:opacity-90',
  ghost: 'border border-transparent text-muted hover:bg-canvas hover:text-ink',
  danger: 'border border-error text-error hover:bg-canvas',
} as const

export type ButtonVariant = keyof typeof VARIANTS
export type ButtonSize = keyof typeof SIZES

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  size?: ButtonSize
}

/** The shared command button (params tray, dialogs, panels). */
export function Button({ variant = 'default', size = 'md', className = '', ...rest }: ButtonProps) {
  return (
    <button
      className={`rounded transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${SIZES[size]} ${VARIANTS[variant]} ${className}`}
      {...rest}
    />
  )
}

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Accessible name + tooltip (required — icon-only buttons need one). */
  title: string
  /** Highlighted state (tool armed, toggle on). */
  active?: boolean
  /** Square hit area; defaults to the 44px touch target of the map strip. */
  size?: number
  children: ReactNode
}

/** Square icon button (map toolbar, top bar). */
export function IconButton({
  title,
  active = false,
  size = 44,
  className = '',
  children,
  ...rest
}: IconButtonProps) {
  return (
    <button
      title={title}
      aria-label={title}
      style={{ width: size, height: size }}
      className={`flex items-center justify-center rounded transition-colors ${
        active ? 'bg-accent text-accent-ink' : 'text-muted hover:bg-canvas hover:text-ink'
      } ${className}`}
      {...rest}
    >
      {children}
    </button>
  )
}

/** Raised surface container (cards, side panels). */
export function Panel({ className = '', children }: { className?: string; children: ReactNode }) {
  return <div className={`panel rounded ${className}`}>{children}</div>
}

/** Anchored floating surface for toolbar dropdowns. Position via parent;
 * rows get the 4px stack spacing every existing popover uses. */
export function Popover({ className = '', children }: { className?: string; children: ReactNode }) {
  return (
    <div
      className={`absolute z-50 rounded border border-line bg-panel p-1.5 shadow-2xl ${className}`}
    >
      <div className="space-y-1">{children}</div>
    </div>
  )
}

/** Row toggle inside a Popover: label left, ON/OFF state right. */
export function PopoverToggle({
  label,
  on,
  onLabel,
  offLabel,
  onToggle,
}: {
  label: string
  on: boolean
  onLabel: string
  offLabel: string
  onToggle: () => void
}) {
  return (
    <button
      onClick={onToggle}
      aria-pressed={on}
      className="flex w-full touch-target items-center justify-between rounded border border-line bg-canvas px-3 py-1.5 text-sm text-ink hover:bg-panel"
    >
      <span className="truncate">{label}</span>
      <span className={`mono text-xs ${on ? 'text-ok' : 'text-muted'}`}>
        {on ? onLabel : offLabel}
      </span>
    </button>
  )
}

/** Row action inside a Popover: leading icon + label. */
export function PopoverAction({
  label,
  icon,
  onClick,
}: {
  label: string
  icon: ReactNode
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full touch-target items-center gap-2 rounded border border-line bg-canvas px-3 py-1.5 text-sm text-ink hover:bg-panel"
    >
      {icon}
      <span className="truncate">{label}</span>
    </button>
  )
}

export interface TabItem<T extends string> {
  id: T
  label: string
}

/** Segmented control (left-pane PX4/Custom, filter chips, nav pills). */
export function Tabs<T extends string>({
  items,
  active,
  onChange,
  className = '',
}: {
  items: Array<TabItem<T>>
  active: T
  onChange: (id: T) => void
  className?: string
}) {
  return (
    <div className={`flex gap-0.5 rounded border border-line bg-canvas p-0.5 ${className}`}>
      {items.map((it) => (
        <button
          key={it.id}
          onClick={() => onChange(it.id)}
          className={`rounded px-3 py-1 text-xs transition-colors ${
            active === it.id ? 'bg-accent text-accent-ink' : 'text-muted hover:text-ink'
          }`}
        >
          {it.label}
        </button>
      ))}
    </div>
  )
}

export type Tone = 'ok' | 'warn' | 'error' | 'muted' | 'accent'

const TONE_BG: Record<Tone, string> = {
  ok: 'bg-ok',
  warn: 'bg-warn',
  error: 'bg-error',
  muted: 'bg-muted',
  accent: 'bg-accent',
}

/** 6px status dot for link/armed/health readouts. */
export function StatusDot({ tone, title }: { tone: Tone; title?: string }) {
  return (
    <span
      title={title}
      className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${TONE_BG[tone]}`}
    />
  )
}

/** Hold-to-confirm length for the medium-risk commands (plan §7 U4). */
export const HOLD_MS = 1000

export interface HoldButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onClick' | 'children'> {
  /** Runs when the hold reaches `holdMs`. A plain click never fires it. */
  onConfirm: () => void
  /** What the hold is about to execute, shown in place of the label while the
   * ring fills (plan §7 U4: the operator sees the action before it runs). */
  hint?: string
  /** Hold length; defaults to [`HOLD_MS`]. */
  holdMs?: number
  variant?: ButtonVariant
  size?: ButtonSize
  children: ReactNode
}

/**
 * Medium-risk command button (plan §7 U4): the action fires only after a
 * deliberate press-and-hold, so a stray click cannot take off, land, RTL or
 * arm the aircraft. The outline draws itself around the button while the hold
 * runs; Esc or a cancelled pointer ends it with no side effect, and a release
 * is still a completed hold if the press outlasted the window (a busy renderer
 * can delay the timer). Keyboard users hold Enter/Space.
 */
export function HoldButton({
  onConfirm,
  hint,
  holdMs = HOLD_MS,
  variant = 'default',
  size = 'md',
  className = '',
  children,
  ...rest
}: HoldButtonProps) {
  const [holding, setHolding] = useState(false)
  const [progress, setProgress] = useState(0)
  const frame = useRef<number | null>(null)
  const timer = useRef<number | null>(null)
  const active = useRef(false)
  const fired = useRef(false)
  const startedAt = useRef(0)
  const confirmRef = useRef(onConfirm)
  confirmRef.current = onConfirm

  const clear = useCallback(() => {
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current)
      frame.current = null
    }
    if (timer.current !== null) {
      window.clearTimeout(timer.current)
      timer.current = null
    }
  }, [])

  /** Commit: one send per press, whichever path gets here first. */
  const fire = useCallback(() => {
    if (fired.current || !active.current) return
    fired.current = true
    clear()
    setHolding(false)
    setProgress(1)
    confirmRef.current()
  }, [clear])

  /** Drop the press without sending. */
  const cancel = useCallback(() => {
    active.current = false
    fired.current = false
    clear()
    setHolding(false)
    setProgress(0)
  }, [clear])

  /**
   * The pointer/key came back up. A release after the full window is a
   * completed hold even if the timer never ran: a busy renderer (software GL,
   * a full map) can delay a 1 s timer by seconds, and an operator who held for
   * a second and let go did what the button asked for.
   */
  const release = useCallback(() => {
    if (!fired.current && active.current) {
      if (performance.now() - startedAt.current >= holdMs) {
        fire()
        return
      }
    }
    cancel()
  }, [cancel, fire, holdMs])

  const start = useCallback(() => {
    // One shot per press: key auto-repeat must not queue a second hold.
    if (active.current) return
    active.current = true
    fired.current = false
    setHolding(true)
    setProgress(0)
    startedAt.current = performance.now()
    // The ring is animation; the *commit* is a timer, so a starved frame loop
    // cannot swallow a deliberate hold.
    timer.current = window.setTimeout(fire, holdMs)
    const step = (now: number) => {
      setProgress(Math.min(1, (now - startedAt.current) / holdMs))
      frame.current = now - startedAt.current >= holdMs ? null : requestAnimationFrame(step)
    }
    frame.current = requestAnimationFrame(step)
  }, [fire, holdMs])

  useEffect(() => cancel, [cancel])

  useEffect(() => stop, [stop])

  return (
    <button
      type="button"
      aria-busy={holding}
      title={hint}
      data-holding={holding}
      onPointerDown={(e) => {
        if (rest.disabled || e.button !== 0) return
        // Capture the pointer: the release then reaches this button even if
        // the finger drifts, and nothing else in the page can end the hold.
        e.currentTarget.setPointerCapture?.(e.pointerId)
        start()
      }}
      onPointerUp={release}
      onPointerCancel={cancel}
      onKeyDown={(e) => {
        if (rest.disabled) return
        if (e.key === 'Escape') {
          cancel()
          return
        }
        if (e.key !== 'Enter' && e.key !== ' ') return
        e.preventDefault()
        start()
      }}
      onKeyUp={(e) => {
        if (e.key === 'Enter' || e.key === ' ') release()
      }}
      className={`relative flex touch-none select-none items-center justify-center overflow-hidden rounded border transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${SIZES[size]} ${VARIANTS[variant]} ${className}`}
      {...rest}
    >
      {/* The label and the hint are both always mounted and only toggled with
       * `hidden`: swapping the children on pointerdown detaches the very node
       * the press started on, and a popover that closes on an outside
       * pointerdown then sees a detached target and closes itself. */}
      <span className={`truncate ${holding ? 'hidden' : ''}`}>{children}</span>
      {hint !== undefined && (
        <span className={`truncate text-warn ${holding ? '' : 'hidden'}`}>{hint}</span>
      )}
      <svg
        aria-hidden="true"
        viewBox="0 0 100 100"
        preserveAspectRatio="none"
        className="pointer-events-none absolute inset-0 h-full w-full"
      >
        {/* Only while the ring means something: a degenerate dash still
         * paints a hairline at the path start. */}
        {progress > 0 && (
          <rect
            x="1"
            y="1"
            width="98"
            height="98"
            rx="3"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            vectorEffect="non-scaling-stroke"
            pathLength={100}
            strokeDasharray={`${progress * 100} 100`}
            className="text-warn"
          />
        )}
      </svg>
    </button>
  )
}

/** One label/value readout inside a [`ConfirmBar`]. */
export interface ConfirmField {
  /** Stable id — also the `data-testid` suffix (`confirm-<key>`). */
  key: string
  label: string
  value: string | null
  tone?: Tone
}

const TONE_TEXT: Record<Tone, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  error: 'text-error',
  muted: 'text-ink',
  accent: 'text-accent',
}

/**
 * The state a command is about to run against (plan §7 U4): flight mode,
 * height above ground, battery. Sits directly above the medium-risk buttons so
 * the operator confirms against real numbers, not memory.
 */
export function ConfirmBar({
  fields,
  note,
  className = '',
}: {
  fields: ConfirmField[]
  note?: string | null
  className?: string
}) {
  return (
    <div
      data-testid="confirm-bar"
      className={`rounded border border-line bg-canvas px-2 py-1 ${className}`}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5">
        {fields.map((field) => (
          <span
            key={field.key}
            data-testid={`confirm-${field.key}`}
            className="flex items-baseline gap-1 text-xs"
          >
            <span className="text-muted">{field.label}</span>
            <span className={`mono ${TONE_TEXT[field.tone ?? 'muted']}`}>
              {field.value ?? '—'}
            </span>
          </span>
        ))}
      </div>
      {note != null && note !== '' && (
        <div className="mt-0.5 text-[11px] text-warn">{note}</div>
      )}
    </div>
  )
}

/** Format a command round-trip for the status strip. */
export function formatElapsedMs(ms: number | null): string | null {
  if (ms == null || !Number.isFinite(ms)) return null
  return `${(ms / 1000).toFixed(1)} s`
}

/**
 * The one command status strip (plan §7 U4): sent → awaiting ack → result plus
 * how long the round trip took. Never optimistic (ADR-017) — the phase text
 * only moves when the backend reports it.
 */
export function CommandStatusStrip({
  command,
  phase,
  elapsedMs,
  tone = 'muted',
  className = '',
}: {
  command: string | null
  phase: string | null
  elapsedMs: number | null
  tone?: Tone
  className?: string
}) {
  if (command === null || phase === null) return null
  const elapsed = formatElapsedMs(elapsedMs)
  return (
    <div
      data-testid="command-status"
      data-command={command}
      className={`flex items-center gap-1.5 rounded border border-line bg-canvas px-2 py-0.5 text-xs ${className}`}
    >
      <span className="text-muted">{command}</span>
      <span className={TONE_TEXT[tone]}>{phase}</span>
      {elapsed !== null && (
        <span data-testid="command-status-elapsed" className="mono text-muted">
          {elapsed}
        </span>
      )}
    </div>
  )
}
