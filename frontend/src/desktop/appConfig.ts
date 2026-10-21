// Rust-backed application config (plan §3 A6): the versioned JSON file owned
// by `core::config`, read and written through the `config_load`/`config_save`
// Tauri commands. The main window hydrates it before first paint and persists
// every change; tool windows hydrate read-only so they pick up the same alert
// volume without racing the main window for writes.
//
// `localStorage` is still written by the individual stores (prefs.ts,
// alertSound.ts), so the app keeps working in a plain browser and the very
// first launch has values to migrate into the file.

import { invoke } from '@tauri-apps/api/core'
import type { AppConfig } from '../generated-types/AppConfig'
import type { LastTarget } from '../generated-types/LastTarget'
import type { PlanPrefs } from '../generated-types/PlanPrefs'
import type { SafetyConfig } from '../generated-types/SafetyConfig'
import { useAlertSound } from '../stores/alertSound'
import { usePrefsStore } from './prefs'

/** Mirrors `core::config::CONFIG_VERSION`; the core re-stamps it on save. */
const CONFIG_VERSION = 1

/** Coalesce rapid changes (orientation sliders) into one file write. */
const SAVE_DEBOUNCE_MS = 300

/** Mirrors `core::safety::SafetyConfig::default()`. Used only before the
 * file loads (or in a plain browser); the Rust side normalizes on save. */
const DEFAULT_SAFETY: SafetyConfig = {
  stale_after_ms: 3000,
  min_satellites: 6,
  warn_battery_percent: 30,
  critical_battery_percent: 15,
  warn_battery_voltage_mv: null,
  critical_battery_voltage_mv: null,
  critical_satellites: 4,
  warn_rssi: null,
  critical_rssi: null,
  warn_txbuf_percent: 20,
  critical_txbuf_percent: 5,
  warn_packet_loss_percent: 10,
  critical_packet_loss_percent: 25,
  require_rtk: false,
  require_home: true,
  require_mission: false,
  dem_max_age_years: 3,
  goto_max_distance_m: null,
  goto_min_clearance_m: null,
  require_on_ground: true,
  require_motor_test_idle: true,
}

/** Safety thresholds have no editor yet (L7/U5); keep the loaded values so a
 * save triggered by an unrelated setting cannot reset them to defaults. */
let safety = DEFAULT_SAFETY

/** Plan prefs default before the file loads (U8). */
const DEFAULT_PLAN: PlanPrefs = { last_center: null, presets: [] }
let plan = DEFAULT_PLAN

/** The last confirmed link target (L8); `null` before the first connect. */
let lastTarget: LastTarget | null = null

/** The manually chosen DEM folder (H6 override); `null` = full auto. */
let terrainOverrideDir: string | null = null

/** The loaded safety thresholds. Callers re-read this each render so a config
 * reload is reflected without a second store. */
export function getSafetyConfig(): SafetyConfig {
  return safety
}

/** The loaded planning prefs (U8): last survey centre + named presets. */
export function getPlanPrefs(): PlanPrefs {
  return plan
}

/** Update the planning prefs and persist (debounced) to the config file. */
export function updatePlanPrefs(next: PlanPrefs): void {
  plan = next
  scheduleSave()
}

/** The last confirmed link target (L8). */
export function getLastTarget(): LastTarget | null {
  return lastTarget
}

/** Record a confirmed link target and persist (L8). */
export function setLastTarget(next: LastTarget): void {
  lastTarget = next
  scheduleSave()
}

/** The manual DEM override (H6), if any. */
export function getTerrainOverrideDir(): string | null {
  return terrainOverrideDir
}

/** Record a manual DEM folder choice (H6) and persist. */
export function setTerrainOverrideDir(dir: string | null): void {
  terrainOverrideDir = dir
  scheduleSave()
}

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** The current store state as the persisted schema. */
function snapshot(): AppConfig {
  const prefs = usePrefsStore.getState()
  const orientation = prefs.modelOrientation
  return {
    version: CONFIG_VERSION,
    endpoint: prefs.endpoint,
    auto_connect: prefs.autoConnect,
    initial_position: {
      latitude_deg: prefs.initialPosition.lat,
      longitude_deg: prefs.initialPosition.lon,
    },
    model_orientation: {
      pitch: orientation.pitch,
      roll: orientation.roll,
      yaw: orientation.yaw,
    },
    alert_volume: useAlertSound.getState().volume,
    safety,
    plan,
    last_target: lastTarget,
    terrain_override_dir: terrainOverrideDir,
  }
}

/** Push a loaded config into the stores the UI reads from. */
function applyConfig(config: AppConfig): void {
  usePrefsStore.setState({
    endpoint: config.endpoint,
    autoConnect: config.auto_connect,
    initialPosition: {
      lat: config.initial_position.latitude_deg,
      lon: config.initial_position.longitude_deg,
    },
    modelOrientation: {
      pitch: config.model_orientation.pitch,
      roll: config.model_orientation.roll,
      yaw: config.model_orientation.yaw,
    },
  })
  useAlertSound.setState({ volume: config.alert_volume })
  safety = config.safety
  plan = config.plan ?? DEFAULT_PLAN
  lastTarget = config.last_target ?? null
  terrainOverrideDir = config.terrain_override_dir ?? null
}

let saveTimer: number | undefined
let subscribed = false

/** Debounced write-through of the current state to the config file. */
function scheduleSave(): void {
  if (!inTauri()) return
  if (saveTimer !== undefined) window.clearTimeout(saveTimer)
  saveTimer = window.setTimeout(() => {
    saveTimer = undefined
    void invoke('config_save', { config: snapshot() }).catch(() => {
      /* non-fatal: the stores also keep their localStorage copy */
    })
  }, SAVE_DEBOUNCE_MS)
}

function subscribePersist(): void {
  if (subscribed) return
  subscribed = true
  usePrefsStore.subscribe(scheduleSave)
  useAlertSound.subscribe(scheduleSave)
}

let initPromise: Promise<void> | null = null

/**
 * Load the Rust-owned config into the stores before first paint.
 *
 * @param owner - main window: migrate the `localStorage` bootstrap into the
 *   file on first launch and persist later changes. Tool windows pass `false`
 *   (read-only) so they cannot overwrite the main window's file.
 */
export function initAppConfig(owner = false): Promise<void> {
  if (initPromise) return initPromise
  initPromise = (async () => {
    if (!inTauri()) return
    try {
      const config = await invoke<AppConfig | null>('config_load')
      if (config) {
        applyConfig(config)
      } else if (owner) {
        // First launch with the new file: migrate the localStorage values the
        // stores bootstrapped from into `config.json`.
        await invoke('config_save', { config: snapshot() })
      }
      if (owner) subscribePersist()
    } catch {
      /* keep the compiled-in defaults; the app still starts */
    }
  })()
  return initPromise
}

// Dev-only handle for the Playwright checks (same pattern as `__mgMission`).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgAppConfig?: unknown }).__mgAppConfig = { getLastTarget }
}
