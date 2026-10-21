// The target-change confirmation (L8): after the link comes up, the first
// heartbeat's sysid/vehicle is compared with the last confirmed target. The
// same target is recorded silently; a different one (a second GCS on the
// bus, a swapped FC) must be acknowledged by the operator before it becomes
// the new baseline — the dialog offers "it's correct" or "disconnect".

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import { useTelemetryStore } from '../../stores/telemetry'
import { useLinkStore } from '../../stores/link'
import { getLastTarget, setLastTarget } from '../../desktop/appConfig'
import { useTargetCheckStore } from '../../stores/target'

/** One check per link session: `restoredAtMs` marks lost→alive edges, so a
 * reconnect re-runs the comparison with the freshly-read heartbeat. */
export function evaluateTargetChange(): void {
  const { link, restoredAtMs } = useLinkStore.getState()
  const snap = useTelemetryStore.getState().snapshot
  const heartbeat = snap?.heartbeat
  if (!link?.fc_alive || !heartbeat) return
  const session = useTargetCheckStore.getState().checkedSession
  // `null` restoredAtMs (the first session) is session id 0 — never equal to
  // a real lost→alive timestamp, so the first connect is always evaluated.
  if (session === (restoredAtMs ?? 0)) return
  const previous = getLastTarget()
  const current = {
    system_id: heartbeat.system_id,
    vehicle: heartbeat.vehicle_type,
  }
  if (previous && previous.system_id === current.system_id && previous.vehicle === current.vehicle) {
    // Same target: record silently (refreshes after an explicit ack too).
    useTargetCheckStore.setState({ checkedSession: restoredAtMs ?? 0 })
    return
  }
  useTargetCheckStore.setState({
    change: { previous, current },
    checkedSession: restoredAtMs ?? 0,
  })
}

export default function TargetChangeDialog() {
  const { t } = useTranslation()
  const change = useTargetCheckStore((s) => s.change)
  const dismiss = useTargetCheckStore((s) => s.dismiss)
  const [busy, setBusy] = useState(false)

  // Re-evaluate on every telemetry update; the store guards per session.
  useEffect(
    () =>
      useTelemetryStore.subscribe(() => {
        evaluateTargetChange()
      }),
    [],
  )

  if (!change) return null

  const confirm = () => {
    setLastTarget(change.current)
    dismiss()
  }

  const disconnect = async () => {
    setBusy(true)
    await invoke('disconnect').catch(() => {})
    setBusy(false)
    dismiss()
  }

  return (
    <Dialog.Root open>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="target-change-dialog"
          className="fixed left-1/2 top-1/2 z-50 w-[min(94vw,460px)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-warn bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="text-sm font-medium text-ink">
            {t('targetChange.title')}
          </Dialog.Title>
          <Dialog.Description asChild>
            <div className="mt-2 space-y-1 text-xs text-muted">
              <div data-testid="target-change-current">
                {t('targetChange.current', {
                  sysid: change.current.system_id,
                  vehicle: change.current.vehicle,
                })}
              </div>
              {change.previous && (
                <div>
                  {t('targetChange.previous', {
                    sysid: change.previous.system_id,
                    vehicle: change.previous.vehicle,
                  })}
                </div>
              )}
              <div>{t('targetChange.hint')}</div>
            </div>
          </Dialog.Description>
          <div className="mt-3 flex justify-end gap-2">
            <button
              data-testid="target-change-disconnect"
              onClick={() => void disconnect()}
              disabled={busy}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel disabled:opacity-50"
            >
              {t('targetChange.disconnect')}
            </button>
            <button
              data-testid="target-change-confirm"
              onClick={confirm}
              disabled={busy}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs font-medium text-ink hover:bg-panel disabled:opacity-50"
            >
              {t('targetChange.confirm')}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
