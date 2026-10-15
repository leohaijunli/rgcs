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
import { useAlertSound } from '../stores/alertSound'
import { usePrefsStore } from './prefs'

/** Mirrors `core::config::CONFIG_VERSION`; the core re-stamps it on save. */
const CONFIG_VERSION = 1

/** Coalesce rapid changes (orientation sliders) into one file write. */
const SAVE_DEBOUNCE_MS = 300

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
