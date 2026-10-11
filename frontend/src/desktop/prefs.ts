// Persisted connection and vehicle preferences (docs/ARCHITECTURE.md, prefs
// addendum, task 0.8): the last endpoint, the auto-connect toggle and the
// initial vehicle position survive a restart.

import { invoke } from '@tauri-apps/api/core'
import { create } from 'zustand'
import { HOME_LAT, HOME_LON } from '../cesium/constants'
import { DEFAULT_ENDPOINT } from './endpoint'

const ENDPOINT_KEY = 'maggcs.endpoint'
const AUTOCONNECT_KEY = 'maggcs.autoconnect'
const POSITION_KEY = 'maggcs.initialPosition'
const MODEL_ORIENTATION_KEY = 'maggcs.modelOrientation'

/** Geographic position in degrees (WGS84). */
export interface LatLon {
  lat: number
  lon: number
}

/** UAV 3D-model orientation offsets in degrees (issues.md #43): added to the
 * MAVLink attitude before the model quaternion is composed, so a model whose
 * nose is not the asset's default axis can be re-aligned from Settings. */
export interface ModelOrientation {
  pitch: number
  roll: number
  yaw: number
}

/** Defaults keep the pre-setting behaviour (yaw −90 = asset nose correction). */
export const DEFAULT_MODEL_ORIENTATION: ModelOrientation = { pitch: 0, roll: 0, yaw: -90 }

function parseModelOrientation(raw: string | null): ModelOrientation {
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as Partial<ModelOrientation>
      const { pitch, roll, yaw } = parsed
      if (
        typeof pitch === 'number' &&
        Number.isFinite(pitch) &&
        typeof roll === 'number' &&
        Number.isFinite(roll) &&
        typeof yaw === 'number' &&
        Number.isFinite(yaw)
      ) {
        return { pitch, roll, yaw }
      }
    } catch {
      /* corrupt entry: fall through to the default */
    }
  }
  return { ...DEFAULT_MODEL_ORIENTATION }
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

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/**
 * Mirror the initial position to the SITL helper file (desktop shell only) so a
 * headless PX4 SITL run spawns at the same location (Settings -> Vehicle).
 * Non-fatal: the map settings still persist in `localStorage` regardless.
 */
export function syncSitlHome(position: LatLon): void {
  if (!inTauri()) return
  void invoke('set_sitl_home', { lat: position.lat, lon: position.lon }).catch(() => {
    /* ignored: localStorage remains the source of truth for the UI */
  })
}

interface PrefsState {
  endpoint: string
  autoConnect: boolean
  initialPosition: LatLon
  modelOrientation: ModelOrientation
  setModelOrientation: (o: ModelOrientation) => void
  setEndpoint: (endpoint: string) => void
  setAutoConnect: (autoConnect: boolean) => void
  /** Ignored unless the position is a valid lat/lon. */
  setInitialPosition: (position: LatLon) => void
}

export const usePrefsStore = create<PrefsState>((set) => ({
  endpoint: read(ENDPOINT_KEY, DEFAULT_ENDPOINT),
  autoConnect: read(AUTOCONNECT_KEY, 'true') !== 'false',
  initialPosition: parseInitialPosition(read(POSITION_KEY, '')),
  modelOrientation: parseModelOrientation(read(MODEL_ORIENTATION_KEY, '')),
  setEndpoint: (endpoint) => {
    write(ENDPOINT_KEY, endpoint)
    set({ endpoint })
  },
  setModelOrientation: (modelOrientation) => {
    write(MODEL_ORIENTATION_KEY, JSON.stringify(modelOrientation))
    set({ modelOrientation })
  },
  setAutoConnect: (autoConnect) => {
    write(AUTOCONNECT_KEY, String(autoConnect))
    set({ autoConnect })
  },
  setInitialPosition: (position) => {
    if (!isValidLatLon(position)) return
    write(POSITION_KEY, JSON.stringify(position))
    syncSitlHome(position)
    set({ initialPosition: position })
  },
}))
