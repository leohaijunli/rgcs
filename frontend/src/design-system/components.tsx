// MagGCS design-system React components (UI2): the primitives every window
// shares. Tokens only — no hardcoded colours (check:colors lint); variants
// map onto the Tailwind theme defined in index.css.

import type { ButtonHTMLAttributes, ReactNode } from 'react'

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
