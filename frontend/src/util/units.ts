// Central unit formatting (plan §7 U13): every window formats altitudes,
// speeds, byte counts, coordinates and elapsed times through these helpers
// so a metre is a metre everywhere. Layout note: the UI keeps metric units
// (m, m/s, nT) — the plan's "unit settings" only centralise the FORMATTING
// (precision, coordinate style), not unit conversion.

/** One decimal, fixed notation (a bare toFixed on −0.05 can emit "-0.1"). */
export function formatMeters(m: number, decimals = 1): string {
  const v = Math.abs(m) < 0.05 / 10 ** decimals ? 0 : m
  return v.toFixed(decimals)
}

/** Speed in m/s, one decimal. */
export function formatSpeed(mps: number): string {
  return `${mps.toFixed(1)} m/s`
}

/** Bytes → a human B/KB/MB/GB string (the DEM/log sizes). */
export function formatBytes(bytes: number | bigint): string {
  const b = typeof bytes === 'bigint' ? Number(bytes) : bytes
  if (!Number.isFinite(b) || b < 0) return '—'
  const units = ['B', 'KB', 'MB', 'GB']
  let v = b
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i += 1
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`
}

/** A WGS84 coordinate with `decimals` digits (6 ≈ 0.11 m, 7 ≈ 1.1 cm). */
export function formatCoord(deg: number, decimals = 7): string {
  return deg.toFixed(decimals)
}

/** Epoch microseconds (the recorder/store convention) → elapsed string. */
export function formatElapsedUs(us: number | bigint | null): string {
  if (us == null) return '—'
  const ms = typeof us === 'bigint' ? Number(us / 1000n) : Math.floor(us / 1000)
  if (!Number.isFinite(ms)) return '—'
  const s = Math.floor(ms / 1000)
  if (s < 60) return `${s} s`
  return `${Math.floor(s / 60)} min ${s % 60} s`
}
