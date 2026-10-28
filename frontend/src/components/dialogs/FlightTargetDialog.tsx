// Live flight-target dialog (map context menu): change the height above the
// DTM under the vehicle (`MAV_CMD_DO_CHANGE_ALTITUDE`) or the cruising
// groundspeed for subsequent setpoints (`MAV_CMD_DO_CHANGE_SPEED`).
//
// Both are medium-risk (plan §7 U4): the send is a press-and-hold, and the
// dialog carries the state the command runs against — the current AGL/speed,
// the armed state and, for altitude, the fact that PX4 switches to Hold.
//
// The envelope mirrors the backend (`flight_command_operations.rs`,
// `MIN/MAX_ALTITUDE_AGL_M` and `MIN/MAX_SPEED_M_S`); the backend re-validates
// and refuses with the authoritative message.

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import { HoldButton } from '../../design-system/components'
import { useVehicleAgl } from '../../hooks/useVehicleAgl'
import { useCommandStore } from '../../stores/command'
import { useTelemetryStore } from '../../stores/telemetry'
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
  const [value, setValue] = useState('')

  // Seed only when the command kind opens/changes. Telemetry keeps updating
  // the readout above, but must never overwrite an operator's in-progress
  // edit.
  useEffect(() => {
    if (mode === null) return
    const seed = mode === 'altitude' ? aglM : speedMs
    setValue(seed != null ? seed.toFixed(1) : '')
    // `aglM` / `speedMs` are intentionally snapshots at the mode transition.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode])

  if (mode === null) return null

  const isAltitude = mode === 'altitude'
  const min = isAltitude ? ALTITUDE_AGL_MIN_M : SPEED_MIN_M_S
  const max = isAltitude ? ALTITUDE_AGL_MAX_M : SPEED_MAX_M_S
  const parsed = Number(value)
  const valid = value.trim() !== '' && Number.isFinite(parsed) && parsed >= min && parsed <= max
  const busy = pending !== null
  const reason = !armed
    ? t('flightTarget.mustBeArmed')
    : !valid
      ? t('flightTarget.invalid', { min, max, unit: isAltitude ? 'm' : 'm/s' })
      : null

  const submit = () => {
    if (reason !== null) return
    if (isAltitude) void sendChangeAltitude(parsed)
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
            {t(isAltitude ? 'flightTarget.altitudeBody' : 'flightTarget.speedBody')}
          </Dialog.Description>

          <dl className="mt-3 grid grid-cols-2 gap-x-3 gap-y-1 text-xs">
            <dt className="text-muted">{t(isAltitude ? 'flightTarget.currentAgl' : 'flightTarget.currentSpeed')}</dt>
            <dd className="mono text-right text-ink" data-testid="flight-target-current">
              {isAltitude
                ? aglM != null ? `${aglM.toFixed(1)} m` : '—'
                : speedMs != null ? `${speedMs.toFixed(1)} m/s` : '—'}
            </dd>
          </dl>

          <label className="mt-3 block text-xs text-muted" htmlFor="flight-target-value">
            {t(isAltitude ? 'flightTarget.targetAgl' : 'flightTarget.targetSpeed')}
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
