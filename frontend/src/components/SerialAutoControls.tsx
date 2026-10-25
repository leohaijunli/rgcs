import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import type { AutoConnectRole } from '../generated-types/AutoConnectRole'
import type { StableUsbId } from '../generated-types/StableUsbId'
import { startDeviceWatcher, useDevicesStore } from '../stores/devices'
import { startSerialAutoWatcher, useSerialAutoStore } from '../stores/serialAuto'

function sameIdentity(a: StableUsbId | null, b: StableUsbId | null): boolean {
  return a != null && b != null && a.serial_number === b.serial_number
    && a.device_id.vendor_id === b.device_id.vendor_id && a.device_id.product_id === b.device_id.product_id
}

export default function SerialAutoControls({ role }: { role: AutoConnectRole }) {
  const { t } = useTranslation()
  const snapshot = useSerialAutoStore((s) => s.snapshot)
  const busy = useSerialAutoStore((s) => s.busy)
  const error = useSerialAutoStore((s) => s.error)
  const save = useSerialAutoStore((s) => s.save)
  const pause = useSerialAutoStore((s) => s.pause)
  const resume = useSerialAutoStore((s) => s.resume)
  const inventory = useDevicesStore((s) => s.inventory)
  useEffect(() => {
    startSerialAutoWatcher()
    startDeviceWatcher()
    void useDevicesStore.getState().load()
  }, [])
  if (!snapshot) return null
  const state = role === 'primary_fc' ? snapshot.fc : snapshot.rtk
  const policy = role === 'primary_fc' ? snapshot.settings.fc.policy : snapshot.settings.rtk.policy
  const ports = inventory?.ports ?? []
  const saved = policy.saved
  const selected = saved ? ports.find((p) => sameIdentity(p.stable_id, saved.stable_id)) : undefined
  const value = saved ? selected?.port.instance_id ?? 'saved' : ''
  const change = (enabled?: boolean, device?: string) => {
    const settings = structuredClone(snapshot.settings)
    const target = role === 'primary_fc' ? settings.fc.policy : settings.rtk.policy
    if (enabled !== undefined) target.enabled = enabled
    if (device !== undefined) {
      const port = ports.find((p) => p.port.instance_id === device)
      target.saved = port ? { stable_id: port.stable_id, last_device: port.port.device } : null
      if (role === 'primary_fc') settings.fc.expected_target = null
    }
    void save(settings)
  }
  const receiver = snapshot.settings.rtk.receiver
  const paused = policy.paused_by_user || state.phase === 'failed' || state.phase === 'needs_selection'
  return (
    <section className="space-y-2 rounded border border-line bg-canvas p-3 text-xs" aria-label={t(`serialAuto.roles.${role}`)} data-testid={`serial-auto-${role}`}>
      <label className="flex min-h-8 items-center gap-2 text-ink">
        <input type="checkbox" checked={policy.enabled} disabled={busy} onChange={(e) => change(e.target.checked)} />
        <span>{t('serialAuto.enabled', { role: t(`serialAuto.roles.${role}`) })}</span>
      </label>
      <div className="flex items-start justify-between gap-2">
        <span className="text-muted">{t('serialAuto.status')}</span>
        <span className={state.phase === 'failed' ? 'text-error' : 'text-ink'}>{t(`serialAuto.phases.${state.phase}`)}</span>
      </div>
      {state.port && <div className="mono break-all text-muted">{state.port.device.port_name}{state.port.device.product ? ` · ${state.port.device.product}` : ''}</div>}
      {state.retry_in_ms != null && <div className="text-muted">{t('serialAuto.retry', { seconds: (state.retry_in_ms / 1000).toFixed(1) })}</div>}
      <label className="block text-muted">{t('serialAuto.preferred')}
        <select aria-label={t('serialAuto.preferred')} className="mt-1 min-h-8 w-full rounded border border-line bg-panel px-2 text-ink" value={value} disabled={busy}
          onChange={(e) => change(undefined, e.target.value)}>
          <option value="">{t('serialAuto.discoverKnown')}</option>
          {saved && !selected && <option value="saved">{saved.last_device.port_name} · {t('serialAuto.savedMissing')}</option>}
          {ports.map((p) => <option key={p.port.instance_id} value={p.port.instance_id}
            disabled={!p.stable_id || (role === 'rtk_base' && p.suggested_role === 'flight_controller')}>
            {p.port.device.port_name}{p.port.device.product ? ` · ${p.port.device.product}` : ''}{p.lease ? ` · ${t('serialAuto.inUse')}` : ''}{!p.stable_id ? ` · ${t('serialAuto.manualOnly')}` : ''}
          </option>)}
        </select>
      </label>
      <p className="text-[11px] leading-relaxed text-muted">{t('serialAuto.preferenceHint')}</p>
      {role === 'rtk_base' && <p className="text-[11px] text-muted">{receiver.mode.kind === 'survey_in'
        ? t('serialAuto.surveySettings', { accuracy: receiver.mode.config.accuracy_limit_m.toFixed(2), duration: receiver.mode.config.min_duration_s })
        : t('serialAuto.passiveSettings')}</p>}
      <div className="flex gap-2">
        <button type="button" className="touch-target min-h-8 rounded border border-line bg-panel px-3 text-ink" disabled={busy}
          onClick={() => void (paused ? resume(role) : pause(role))}>
          {t(paused ? 'serialAuto.resume' : 'serialAuto.pause')}
        </button>
        {snapshot.settings_error && <button type="button" className="touch-target min-h-8 rounded border border-line px-3 text-ink" disabled={busy}
          onClick={() => void save(snapshot.settings)}>{t('serialAuto.saveSettings')}</button>}
      </div>
      {(error || snapshot.settings_error || state.error) && <p role="alert" className="break-words text-error">{error ?? snapshot.settings_error ?? state.error?.message}</p>}
    </section>
  )
}
