// Live flight-target dialog (map context menu): change the DTM-relative AGL
// height, or absolute AMSL without a ready DEM (`MAV_CMD_DO_CHANGE_ALTITUDE`),
// or the cruising groundspeed (`MAV_CMD_DO_CHANGE_SPEED`).
//
// Both are medium-risk (plan §7 U4): the send is a press-and-hold, and the
// dialog carries the state the command runs against — the current height/speed,
// the armed state and, for altitude, the fact that PX4 switches to Hold.
//
// The envelope mirrors the backend (`flight_command_operations.rs`,
// `MIN/MAX_ALTITUDE_AGL_M` and `MIN/MAX_SPEED_M_S`); the backend re-validates
// and refuses with the authoritative message.

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import { HoldButton } from '../../design-system/components'
import type { FlightAltitudeTarget } from '../../generated-types/FlightAltitudeTarget'
import { useVehicleAgl } from '../../hooks/useVehicleAgl'
import { useCommandStore } from '../../stores/command'
import { useTelemetryStore } from '../../stores/telemetry'
import { useTerrainStore } from '../../stores/terrain'
import { useUiStore, type FlightTargetMode } from '../../stores/ui'

/** Envelope of `MAV_CMD_DO_CHANGE_ALTITUDE`, metres above the DTM. */
export const ALTITUDE_AGL_MIN_M = 1
export const ALTITUDE_AGL_MAX_M = 120
/** Envelope of `MAV_CMD_DO_CHANGE_SPEED`, m/s (`MPC_XY_CRUISE`..`MPC_XY_VEL_MAX`). */
export const SPEED_MIN_M_S = 1
export const SPEED_MAX_M_S = 15

/** Current groundspeed from the latched velocity, the same value the HUD shows. */
function currentSpeedMs(snapshot: ReturnType<typeof useTelemetryStore.getState>['snapshot']): number | null {
  const velocity = snapshot?.global_position?.velocity
  if (!velocity) return null
  const speed = Math.hypot(velocity.x_m_s, velocity.y_m_s)
  return Number.isFinite(speed) ? speed : null
}

export default function FlightTargetDialog() {
  const { t } = useTranslation()
  const mode: FlightTargetMode | null = useUiStore((s) => s.flightTarget)
  const onClose = useUiStore((s) => s.closeFlightTarget)
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const aglM = useVehicleAgl()
  const sendChangeAltitude = useCommandStore((s) => s.sendChangeAltitude)
  const sendChangeSpeed = useCommandStore((s) => s.sendChangeSpeed)
  const pending = useCommandStore((s) => s.pending)
  const armed = snapshot?.heartbeat?.base_mode.safety_armed === true
  const speedMs = currentSpeedMs(snapshot)
  const reportedAmsl = snapshot?.global_position?.altitude
  const amslM = reportedAmsl?.datum === 'AMSL_EGM96' && Number.isFinite(reportedAmsl.meters)
    ? reportedAmsl.meters
    : null
  const [altitudeKind, setAltitudeKind] = useState<FlightAltitudeTarget['kind']>('amsl')
  const [checkingTerrain, setCheckingTerrain] = useState(false)
  const [value, setValue] = useState('')

  // Resolve and latch the altitude reference when the dialog opens. A loaded
  // tile must actually sample under the vehicle; a mesh-only/stale AGL readout
  // is not proof that the backend's DTM is ready. Telemetry and a later DEM
  // load must never reinterpret an operator's in-progress edit.
  useEffect(() => {
    setCheckingTerrain(false)
    if (mode === null) return
    const seed = mode === 'altitude' ? amslM : speedMs
    setAltitudeKind('amsl')
    setValue(seed != null ? seed.toFixed(1) : '')
    if (mode !== 'altitude' || aglM == null || !Number.isFinite(aglM)) return
    const terrain = useTerrainStore.getState()
    const pos = snapshot?.global_position
    if (!terrain.tile || terrain.loading || !pos) return
    let cancelled = false
    setCheckingTerrain(true)
    void terrain.sampleDem([[pos.latitude_deg, pos.longitude_deg]])
      .then(([ground]) => {
        if (cancelled) return
        if (ground != null && Number.isFinite(ground)) {
          setAltitudeKind('agl')
          setValue(aglM.toFixed(1))
        }
      })
      .catch(() => { /* unavailable DEM: keep the AMSL reference */ })
      .finally(() => { if (!cancelled) setCheckingTerrain(false) })
    return () => { cancelled = true }
    // Altitude/speed and terrain are snapshots at the mode transition.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode])

  if (mode === null) return null

  const isAltitude = mode === 'altitude'
  const isAgl = isAltitude && altitudeKind === 'agl'
  const min = isAgl ? ALTITUDE_AGL_MIN_M : isAltitude ? undefined : SPEED_MIN_M_S
  const max = isAgl ? ALTITUDE_AGL_MAX_M : isAltitude ? undefined : SPEED_MAX_M_S
  const parsed = Number(value)
  const valid = value.trim() !== '' && Number.isFinite(parsed)
    && (min === undefined || parsed >= min) && (max === undefined || parsed <= max)
  const altitudeM = isAgl ? aglM : amslM
  const busy = pending !== null
  const reason = !armed
    ? t('flightTarget.mustBeArmed')
    : checkingTerrain
      ? t('flightTarget.checkingTerrain')
      : !valid
        ? isAltitude && !isAgl
          ? t('flightTarget.invalidAmsl')
          : t('flightTarget.invalid', { min, max, unit: isAltitude ? 'm' : 'm/s' })
        : null

  const submit = () => {
    if (reason !== null) return
    if (isAltitude) void sendChangeAltitude(isAgl
      ? { kind: 'agl', agl_m: parsed }
      : { kind: 'amsl', altitude: { datum: 'AMSL_EGM96', meters: parsed } })
    else void sendChangeSpeed(parsed)
    onClose()
  }

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40" />
        <Dialog.Content
          data-testid="flight-target-dialog"
          className="fixed left-1/2 top-20 z-50 w-[min(94vw,360px)] -translate-x-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="text-sm font-medium text-ink">
            {t(isAltitude ? 'flightTarget.altitudeTitle' : 'flightTarget.speedTitle')}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-xs text-muted">
            {t(isAgl ? 'flightTarget.altitudeBody' : isAltitude ? 'flightTarget.altitudeAmslBody' : 'flightTarget.speedBody')}
          </Dialog.Description>

          <dl className="mt-3 grid grid-cols-2 gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted">{t(isAgl ? 'flightTarget.currentAgl' : isAltitude ? 'flightTarget.currentAmsl' : 'flightTarget.currentSpeed')}</dt>
            <dd className="mono text-right text-ink" data-testid="flight-target-current">
              {isAltitude
                ? altitudeM != null ? `${altitudeM.toFixed(1)} m` : '—'
                : speedMs != null ? `${speedMs.toFixed(1)} m/s` : '—'}
            </dd>
          </dl>

          <label className="mt-3 block text-xs text-muted" htmlFor="flight-target-value">
            {t(isAgl ? 'flightTarget.targetAgl' : isAltitude ? 'flightTarget.targetAmsl' : 'flightTarget.targetSpeed')}
          </label>
          <input
            id="flight-target-value"
            data-testid="flight-target-input"
            type="number"
            inputMode="decimal"
            min={min}
            max={max}
            step={0.5}
            value={value}
            disabled={checkingTerrain}
            onChange={(event) => setValue(event.target.value)}
            className="mono mt-1 h-9 w-full rounded border border-line bg-canvas px-2 text-sm text-ink"
          />

          {isAltitude && (
            <p className="mt-2 text-[11px] text-muted">{t('flightTarget.holdNotice')}</p>
          )}

          <div className="mt-3 flex items-center justify-end gap-2">
            {reason !== null && (
              <span data-testid="flight-target-reason" className="mr-auto text-[11px] text-warn">
                {reason}
              </span>
            )}
            <button
              data-testid="flight-target-cancel"
              onClick={onClose}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
            >
              {t('flightTarget.cancel')}
            </button>
            <HoldButton
              data-testid="flight-target-send"
              variant="danger"
              size="md"
              disabled={reason !== null || busy}
              onConfirm={submit}
            >
              {t('flightTarget.send')}
            </HoldButton>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
