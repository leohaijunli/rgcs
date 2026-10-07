// Browser-dev mock telemetry: keeps the UI alive when running in a plain
// browser (no Tauri runtime, no MAVLink link). The real desktop shell
// receives live data via Tauri events (see desktop/bridge.ts).

import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'
import { useTelemetryStore } from '../stores/telemetry'

const HOME_LAT = 48.6493
const HOME_LON = -123.3982

/** Tick period of the mock feed, in seconds. */
const TICK_S = 0.25
const M_PER_DEG_LAT = 111_320

/** Position on the mock's flight path at tick `t`. */
function pathAt(t: number) {
  return {
    lat: HOME_LAT + 0.004 * Math.sin(t / 40),
    lon: HOME_LON + 0.006 * Math.cos(t / 50),
    alt: 100 + 8 * Math.sin(t / 60),
  }
}

/** Start a sine-wave flight loop; returns the interval id. */
export function startMockFeed(): number {
  useTelemetryStore.getState().setMock(true)
  let t = 0
  const id = window.setInterval(() => {
    t += 1
    const { lat, lon, alt } = pathAt(t)

    // NED velocity derived from the path itself. The map dead-reckons the
    // marker and draws the forward projection from this, so a velocity that
    // disagrees with the trajectory would make the marker lag and jump.
    const ahead = pathAt(t + 1)
    const mPerDegLon = M_PER_DEG_LAT * Math.cos((lat * Math.PI) / 180)
    const vn = ((ahead.lat - lat) * M_PER_DEG_LAT) / TICK_S
    const ve = ((ahead.lon - lon) * mPerDegLon) / TICK_S
    const vd = -(ahead.alt - alt) / TICK_S
    const heading = (Math.atan2(ve, vn) * (180 / Math.PI) + 360) % 360

    const snap: TelemetrySnapshot = {
      last_heartbeat_at_ms: Date.now(),
      heartbeat: {
        system_id: 1,
        component_id: 1,
        vehicle_type: 'quadrotor',
        autopilot: 'px4',
        base_mode: {
          custom_mode_enabled: true,
          test_enabled: false,
          auto_enabled: true,
          guided_enabled: true,
          stabilize_enabled: false,
          hil_enabled: false,
          manual_input_enabled: false,
          safety_armed: true,
        },
        // PX4 packs main mode in bits 16-23, sub mode in 24-31:
        // AUTO + LOITER, so the mode pill reads "Auto·Loiter".
        custom_mode: (4 << 16) | (3 << 24),
        flight_state: 'active',
        mavlink_version: 3,
      },
      global_position: {
        time_boot_ms: t * 1000,
        latitude_deg: lat,
        longitude_deg: lon,
        altitude: { datum: 'AMSL_EGM96', meters: alt },
        relative_alt_m: alt - 80,
        velocity: { x_m_s: vn, y_m_s: ve, z_m_s: vd },
        heading_deg: heading,
      },
      attitude: {
        time_boot_ms: t * 1000,
        roll_deg: Math.sin(t / 20) * 5,
        pitch_deg: Math.cos(t / 25) * 3,
        yaw_deg: heading,
        roll_speed_deg_s: 0,
        pitch_speed_deg_s: 0,
        yaw_speed_deg_s: 0,
      },
      sys_status: {
        sensors: { present: 0, enabled: 0, health: 0 },
        battery_voltage_mv: 16700,
        battery_current_ma: 2200,
        battery_remaining_percent: 88,
      },
      battery: {
        battery_id: 0,
        voltage_cells_mv: [4100, 4090, 4080, 4070],
        current_ma: 2200,
        remaining_percent: 88,
        temperature_deg_c: 32,
      },
      gps: {
        fix_type: 'RTK_FIXED',
        satellites_visible: 20,
        latitude_deg: lat,
        longitude_deg: lon,
        altitude: { datum: 'AMSL_EGM96', meters: alt },
        hdop: 0.7,
        vdop: 1.1,
        velocity_m_s: Math.hypot(vn, ve),
        course_over_ground_deg: heading,
      },
      field_ages: {
        heartbeat_at_ms: Date.now(),
        global_position_at_ms: Date.now(),
        attitude_at_ms: Date.now(),
        sys_status_at_ms: Date.now(),
        battery_at_ms: Date.now(),
        gps_at_ms: Date.now(),
      },
    }
    useTelemetryStore.getState().applySnapshot(snap)
  }, 250)
  return id
}
