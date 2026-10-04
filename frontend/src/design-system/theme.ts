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