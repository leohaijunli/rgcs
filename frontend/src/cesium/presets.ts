// Preset flight areas (预设飞行区域): named take-off/home positions shared
// with the SITL scripts (`scripts/sitl/sitl-home.sh` mirrors this list as a
// case statement — keep the two in sync). Applying a preset sets the
// Settings → Vehicle initial position, which drives the HOME marker, the
// initial camera and the mock feed in the app, and `sitl-home.sh` resolves
// the same coordinates for PX4 SITL.
//
// BC areas are LiDAR-covered (BC provincial 2019–2023 campaign); Chilliwack
// is the recommended SITL drape-verification area: flat Fraser-valley floor
// for take-off with Vedder Mountain / Mount Cheam (~1200 m) right next to it,
// so a 40 m AGL drape has real relief to track.

export interface PresetArea {
  id: string
  name: string
  lat: number
  lon: number
  /** Ground AMSL in metres — the SITL home altitude for the area. */
  alt: number
}

export const PRESET_AREAS: PresetArea[] = [
  { id: 'sidney', name: 'Sidney, BC', lat: 48.6493, lon: -123.3982, alt: 5 },
  { id: 'renfrew', name: 'Renfrew, ON', lat: 45.4726, lon: -76.6858, alt: 120 },
  { id: 'chilliwack', name: 'Chilliwack, BC', lat: 49.1577, lon: -121.951, alt: 10 },
]

/** localStorage key for the operator's own areas (Settings → Vehicle). */
const CUSTOM_AREAS_KEY = 'maggcs.presetAreas'

/** The operator's custom areas, or an empty list (corrupt entries drop). */
export function loadCustomAreas(): PresetArea[] {
  try {
    const raw = localStorage.getItem(CUSTOM_AREAS_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed.filter(
      (a): a is PresetArea =>
        typeof a === 'object' &&
        a !== null &&
        typeof (a as PresetArea).id === 'string' &&
        typeof (a as PresetArea).name === 'string' &&
        typeof (a as PresetArea).lat === 'number' &&
        typeof (a as PresetArea).lon === 'number' &&
        typeof (a as PresetArea).alt === 'number',
    )
  } catch {
    return []
  }
}

/** Persist the operator's custom areas (best-effort; private mode drops). */
export function saveCustomAreas(areas: PresetArea[]): void {
  try {
    localStorage.setItem(CUSTOM_AREAS_KEY, JSON.stringify(areas))
  } catch {
    /* storage unavailable */
  }
}

/** Every selectable area: the built-in presets first, then custom ones. */
export function allAreas(custom: PresetArea[]): PresetArea[] {
  return [...PRESET_AREAS, ...custom]
}

/** Validate one decimal-degree field for the add form. */
export function isValidAreaInput(lat: number, lon: number): boolean {
  return (
    Number.isFinite(lat) && lat >= -90 && lat <= 90 &&
    Number.isFinite(lon) && lon >= -180 && lon <= 180
  )
}

export function presetArea(id: string | null): PresetArea | null {
  return allAreas(loadCustomAreas()).find((a) => a.id === id) ?? null
}