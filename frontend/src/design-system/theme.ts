// Reads design tokens from the active theme so canvas instruments redraw
// correctly in both themes without hardcoded colors (check:colors lint).

export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

export function themeVars(): Record<string, string> {
  const names = [
    '--mg-bg',
    '--mg-bg-raised',
    '--mg-border',
    '--mg-ink',
    '--mg-muted',
    '--mg-accent',
    '--mg-ok',
    '--mg-warn',
    '--mg-error',
    '--mg-sky',
    '--mg-ground',
    '--mg-instrument',
  ]
  const out: Record<string, string> = {}
  for (const n of names) out[n] = cssVar(n)
  return out
}
/** Stored theme preference (UI5): an explicit colour scheme, the OS
 * preference, or the night-red palette for dark-adapted night flights. */
export type ThemePref = 'dark' | 'light' | 'system' | 'night-red'

/** The `data-theme` value actually applied to the document. */
export type ResolvedTheme = 'dark' | 'light' | 'night-red'

const THEME_KEY = 'maggcs.theme'

/** Coerce whatever is in storage (old builds stored only dark/light) into a
 * valid preference. */
export function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY)
    if (v === 'light' || v === 'system' || v === 'night-red') return v
  } catch {
    /* private mode */
  }
  return 'dark'
}

export function saveThemePref(pref: ThemePref): void {
  try {
    localStorage.setItem(THEME_KEY, pref)
  } catch {
    /* private mode */
  }
}

/** `system` follows `prefers-color-scheme` live; everything else is fixed. */
export function resolveThemePref(pref: ThemePref): ResolvedTheme {
  if (pref === 'system') {
    return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
  }
  return pref
}

let mediaCleanup: (() => void) | null = null

/** Apply the preference to `<html data-theme>`; for `system` re-resolve on
 * OS changes until the next `applyThemePref` call replaces the listener. */
export function applyThemePref(pref: ThemePref): void {
  mediaCleanup?.()
  mediaCleanup = null
  const set = (t: ResolvedTheme) => {
    document.documentElement.dataset.theme = t
  }
  if (pref === 'system') {
    const mq = window.matchMedia('(prefers-color-scheme: light)')
    const onChange = (): void => set(mq.matches ? 'light' : 'dark')
    onChange()
    mq.addEventListener('change', onChange)
    mediaCleanup = () => mq.removeEventListener('change', onChange)
  } else {
    set(pref)
  }
}

// ---------------------------------------------------------------------------
// Field mode (plan §7 U12): a touch/outdoor profile — ≥44 px targets, 14 px
// base text, fewer animations. Applied as an `html.field-mode` class next to
// `data-theme`, so it composes with every palette including `night-red`.
// ---------------------------------------------------------------------------

const FIELD_MODE_KEY = 'maggcs.fieldMode'

/** Read the persisted field-mode flag (`false` = never set). */
export function readFieldMode(): boolean {
  try {
    return localStorage.getItem(FIELD_MODE_KEY) === 'true'
  } catch {
    return false
  }
}

export function saveFieldMode(on: boolean): void {
  try {
    localStorage.setItem(FIELD_MODE_KEY, on ? 'true' : 'false')
  } catch {
    /* private mode */
  }
}

/** Apply (or remove) the `field-mode` class on `<html>`. */
export function applyFieldMode(on: boolean): void {
  document.documentElement.classList.toggle('field-mode', on)
}
