import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import {
  DEFAULT_MOTOR_TONES,
  type MotorTone,
  type MotorToneSettings,
  type MotorToneStatus,
} from './benchTypes'

const STORAGE_KEY = 'maggcs.actuator.tones'
const CHOICES: { value: MotorTone; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'rising', label: 'Rising · 1' },
  { value: 'falling', label: 'Falling · 2' },
  { value: 'sweep', label: 'Sweep · 3' },
  { value: 'triple', label: 'Triple · 4' },
]

function loadSettings(): MotorToneSettings {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null')
    if (saved && typeof saved === 'object' && 'start' in saved && 'stop' in saved
      && CHOICES.some(({ value }) => value === saved.start)
      && CHOICES.some(({ value }) => value === saved.stop)) {
      return saved as MotorToneSettings
    }
  } catch { /* Restore defaults for an unavailable or invalid preference. */ }
  return { ...DEFAULT_MOTOR_TONES }
}

export default function MotorToneControls(props: {
  tauri: boolean
  supported: boolean
  locked: boolean
  status: MotorToneStatus
  onReady: (ready: boolean) => void
  onChange: () => void
}) {
  const [settings, setSettings] = useState(loadSettings)
  const [saving, setSaving] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const { tauri, onReady } = props
  useEffect(() => {
    let disposed = false
    const saved = loadSettings()
    const sync = tauri ? invoke('actuator_set_tones', { settings: saved }) : Promise.resolve()
    void sync.then(() => {
      if (!disposed) { setSettings(saved); onReady(true) }
    }).catch((error: unknown) => {
      if (!disposed) setError(String(error))
    }).finally(() => { if (!disposed) setSaving(false) })
    return () => { disposed = true }
  }, [tauri, onReady])

  const change = async (field: keyof MotorToneSettings, value: MotorTone) => {
    const next = { ...settings, [field]: value }
    setSaving(true)
    setError(null)
    onReady(false)
    props.onChange()
    try {
      if (tauri) await invoke('actuator_set_tones', { settings: next })
      setSettings(next)
      onReady(true)
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next))
    } catch (error) {
      setError(String(error))
    } finally {
      setSaving(false)
    }
  }

  return <section className="card motor-tone-card" data-testid="motor-tones">
    <h3>ESC tones</h3>
    {(['start', 'stop'] as const).map((field) => <label className="motor-tone-row" key={field}>
      <span>{field === 'start' ? 'Start tone' : 'Stop tone'}</span>
      <select
        className="phrase-input"
        aria-label={field === 'start' ? 'Start tone' : 'Stop tone'}
        value={settings[field]}
        title={field === 'start' ? 'Choose the ESC sound before an enabled motor test starts. Changing this preference plays nothing.' : 'Choose the sound after Motor Test releases its outputs. Motor control OFF suppresses new tones.'}
        disabled={saving || props.locked || props.status.busy}
        onChange={(event) => { void change(field, event.target.value as MotorTone) }}
      >
        {CHOICES.map(({ value, label }) => <option key={value} value={value}>{label}</option>)}
      </select>
    </label>)}
    <div className="estop-note">{props.supported
      ? 'AM32 / DShot · start before output, stop after release.'
      : 'DShot outputs required. Saved choices apply when available.'}</div>
    <div className="estop-note mono" aria-live="polite" data-testid="motor-tone-status">
      {props.status.busy ? `${props.status.trigger === 'stopped' ? 'Stop' : 'Start'} tone · ${props.status.phase}` : ''}
    </div>
    {(error || props.status.error) && <div className="estop-note text-error" role="alert">
      {error ?? props.status.error}
    </div>}
  </section>
}
