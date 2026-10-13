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

export function presetArea(id: string | null): PresetArea | null {
  return PRESET_AREAS.find((a) => a.id === id) ?? null
}