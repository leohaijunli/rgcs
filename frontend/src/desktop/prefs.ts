// Persisted connection and vehicle preferences (DEVELOPMENT_PLAN Phase 0
// addendum, task 0.8): the last endpoint, the auto-connect toggle and the
// initial vehicle position survive a restart.

import { create } from 'zustand'
import { HOME_LAT, HOME_LON } from '../cesium/constants'
import { DEFAULT_ENDPOINT } from './endpoint'

const ENDPOINT_KEY = 'maggcs.endpoint'
const AUTOCONNECT_KEY = 'maggcs.autoconnect'
const POSITION_KEY = 'maggcs.initialPosition'

/** Geographic position in degrees (WGS84). */
export interface LatLon {
  lat: number
  lon: number
}

/**
 * Position used before a vehicle reports its own fix: the mock feed's flight
 * path starts here, the HOME marker and the initial camera sit here, and
 * "go home" returns here. Operators set it in Settings → Vehicle.
 */
export const DEFAULT_INITIAL_POSITION: LatLon = { lat: HOME_LAT, lon: HOME_LON }

export function isValidLatitude(value: number): boolean {
  return Number.isFinite(value) && value >= -90 && value <= 90
}

export function isValidLongitude(value: number): boolean {
  return Number.isFinite(value) && value >= -180 && value <= 180
}

export function isValidLatLon(position: LatLon): boolean {
  return isValidLatitude(position.lat) && isValidLongitude(position.lon)
}

/** Parse a persisted position, falling back to the default when unusable. */
export function parseInitialPosition(raw: string | null): LatLon {
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as Partial<LatLon>
      const { lat, lon } = parsed
      if (typeof lat === 'number' && typeof lon === 'number' && isValidLatLon({ lat, lon })) {
        return { lat, lon }
      }
    } catch {
      /* corrupt entry: fall through to the default */
    }
  }
  return { ...DEFAULT_INITIAL_POSITION }
}

function read(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback
  } catch {
    return fallback
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    /* private mode */
  }
}

interface PrefsState {
  endpoint: string
  autoConnect: boolean
  initialPosition: LatLon
  setEndpoint: (endpoint: string) => void
  setAutoConnect: (autoConnect: boolean) => void
  /** Ignored unless the position is a valid lat/lon. */
  setInitialPosition: (position: LatLon) => void
}

export const usePrefsStore = create<PrefsState>((set) => ({
  endpoint: read(ENDPOINT_KEY, DEFAULT_ENDPOINT),
  autoConnect: read(AUTOCONNECT_KEY, 'true') !== 'false',
  initialPosition: parseInitialPosition(read(POSITION_KEY, '')),
  setEndpoint: (endpoint) => {
    write(ENDPOINT_KEY, endpoint)
    set({ endpoint })
  },
  setAutoConnect: (autoConnect) => {
    write(AUTOCONNECT_KEY, String(autoConnect))
    set({ autoConnect })
  },
  setInitialPosition: (position) => {
    if (!isValidLatLon(position)) return
    write(POSITION_KEY, JSON.stringify(position))
    set({ initialPosition: position })
  },
}))
