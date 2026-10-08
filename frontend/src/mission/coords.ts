// Coordinate entry (improve_plan WS-G G2): turn what an operator reads off a
// handheld GPS, a paper map or an RTK base report into a decimal lat/lon.
//
// Supported forms (the two coordinates are separated by a comma or semicolon):
//
//   -123.3982                     48.6493
//   48.6493, -123.3982            decimal degrees, latitude first
//   48°38'57.5"N, 123°23'53.5"W   degrees / minutes / seconds
//   48 38 57.5 N, 123 23 53.5 W   the same, space separated
//   -123.3982, 48.6493            order corrected when only one reading can
//                                 be a latitude (|value| > 90)
//
// UTM is deliberately not accepted yet: zone 10 arrives with the survey
// clipper in WS-C, where a projection implementation can be tested against a
// reference implementation.

/** Which coordinate axes a value may stand for. */
export type Axis = 'lat' | 'lon'

export interface ParsedCoordinates {
  lat: number
  lon: number
  /** How the input was written, for the confirmation readout. */
  format: 'decimal' | 'dms'
}

export type CoordError = 'empty' | 'format' | 'range'

export type CoordParseResult =
  | { ok: true; value: ParsedCoordinates }
  | { ok: false; error: CoordError }

interface Token {
  degrees: number
  /** N/S/E/W as written, or null when the value carried a sign instead. */
  hemisphere: 'N' | 'S' | 'E' | 'W' | null
}

const HEMISPHERES = ['N', 'S', 'E', 'W'] as const

/** Parse one half of the input: a decimal degree or a D/M/S triple. */
function parseToken(raw: string): Token | null {
  let text = raw.trim()
  if (text === '') return null

  let hemisphere: Token['hemisphere'] = null
  const tail = /([NSEWnsew])\s*$/.exec(text)
  if (tail) {
    hemisphere = tail[1].toUpperCase() as Token['hemisphere']
    text = text.slice(0, tail.index)
  } else {
    const head = /^\s*([NSEWnsew])\b/.exec(text)
    if (head) {
      hemisphere = head[1].toUpperCase() as Token['hemisphere']
      text = text.slice(head[0].length)
    }
  }

  // Strip the usual symbols; anything else that is not a number is a typo.
  text = text.replace(/[°º'′"″]/g, ' ').trim()
  if (text === '' || !/^[\d+\-.\s]+$/.test(text)) return null

  const parts = text.split(/\s+/)
  if (parts.length > 3) return null
  const nums = parts.map((p) => Number(p))
  if (nums.some((n) => !Number.isFinite(n))) return null

  const [deg, min = 0, sec = 0] = nums
  if (parts.length > 1) {
    // D/M/S: the sign belongs to the degrees, minutes and seconds are unsigned.
    if (!Number.isInteger(deg) || min < 0 || sec < 0 || min >= 60 || sec >= 60) return null
  }
  const sign = deg < 0 ? -1 : 1
  const magnitude = Math.abs(deg) + min / 60 + sec / 3600
  return { degrees: sign * magnitude, hemisphere }
}

function axisOf(hemisphere: Token['hemisphere']): Axis | null {
  if (hemisphere === 'N' || hemisphere === 'S') return 'lat'
  if (hemisphere === 'E' || hemisphere === 'W') return 'lon'
  return null
}

function signOf(hemisphere: Token['hemisphere']): number {
  return hemisphere === 'S' || hemisphere === 'W' ? -1 : 1
}

function inRange(value: number, axis: Axis): boolean {
  return axis === 'lat' ? Math.abs(value) <= 90 : Math.abs(value) <= 180
}

/**
 * Parse an operator-typed coordinate pair.
 *
 * Ambiguity is resolved conservatively: a hemisphere suffix always decides the
 * axis, and a bare pair is read as `lat, lon` unless only one value can be a
 * latitude. Refusing the rest keeps a typo from silently becoming a waypoint
 * somewhere on the other side of the planet.
 */
export function parseCoordinates(input: string): CoordParseResult {
  const trimmed = input.trim()
  if (trimmed === '') return { ok: false, error: 'empty' }

  const halves = trimmed.split(/[,;]/).map((s) => s.trim())
  if (halves.length !== 2 || halves.some((h) => h === '')) return { ok: false, error: 'format' }

  const tokens = halves.map(parseToken)
  if (tokens.some((t) => t === null)) return { ok: false, error: 'format' }
  const [first, second] = tokens as [Token, Token]

  const firstAxis = axisOf(first.hemisphere)
  const secondAxis = axisOf(second.hemisphere)

  let latToken: Token
  let lonToken: Token
  if (firstAxis && secondAxis) {
    if (firstAxis === secondAxis) return { ok: false, error: 'format' }
    latToken = firstAxis === 'lat' ? first : second
    lonToken = firstAxis === 'lat' ? second : first
  } else if (firstAxis || secondAxis) {
    // One half is labelled: it decides its own axis, the other takes the rest.
    const latIsFirst = firstAxis === 'lat' || secondAxis === 'lon'
    latToken = latIsFirst ? first : second
    lonToken = latIsFirst ? second : first
  } else if (Math.abs(first.degrees) > 90 && Math.abs(second.degrees) <= 90) {
    // Only one reading can be a latitude, so the order was reversed.
    latToken = second
    lonToken = first
  } else {
    latToken = first
    lonToken = second
  }

  // A hemisphere letter overrides the sign; a bare value keeps its own.
  const signed = (t: Token) =>
    t.hemisphere ? signOf(t.hemisphere) * Math.abs(t.degrees) : t.degrees
  const lat = signed(latToken)
  const lon = signed(lonToken)
  if (!inRange(lat, 'lat') || !inRange(lon, 'lon')) return { ok: false, error: 'range' }

  const dms = halves.some((h) => /[°º'′"″]|\s/.test(h.trim()))
  return { ok: true, value: { lat, lon, format: dms ? 'dms' : 'decimal' } }
}

/** Format a decimal degree as `48°38'57.5"N`. */
export function formatDms(deg: number, axis: Axis): string {
  const hemisphere = axis === 'lat' ? (deg < 0 ? 'S' : 'N') : deg < 0 ? 'W' : 'E'
  const magnitude = Math.abs(deg)
  let degrees = Math.floor(magnitude)
  const minutesFull = (magnitude - degrees) * 60
  let minutes = Math.floor(minutesFull)
  let seconds = (minutesFull - minutes) * 60
  // 59.96" rounds to an invalid 60.0": carry instead of printing it.
  if (Number(seconds.toFixed(1)) >= 60) {
    seconds = 0
    minutes += 1
    if (minutes >= 60) {
      minutes = 0
      degrees += 1
    }
  }
  return `${degrees}°${minutes}'${seconds.toFixed(1)}"${hemisphere}`
}

/** Fixed-precision decimal, so a value round-trips through the input. */
export function formatDecimal(deg: number): string {
  return deg.toFixed(6)
}

export { HEMISPHERES }
