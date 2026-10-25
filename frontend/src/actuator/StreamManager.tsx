import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

interface StandardStream {
  id: number
  name: string
  fields: { name: string; wire_type: string; units: string | null }[]
  px4_support: 'documented_v1_17' | 'unknown'
  px4_mapping: { uorb_sources: string[]; source_url: string; note: string } | null
}
interface StreamTarget {
  connection_id: number
  target_system_id: number
  target_component_id: number
}
type StreamRate = { kind: 'hertz'; hz: number } | { kind: 'disabled' }
interface StreamRequest { message_id: number; rate: StreamRate; allow_unknown_support: boolean }
interface StreamStatus {
  message_id: number
  requested_rate: StreamRate
  observed_hz: number | null
  last_seen_age_ms: number | null
  receive_state: 'waiting' | 'receiving' | 'stale'
  error: string | null
}
interface StreamSnapshot {
  target: StreamTarget | null
  endpoint: string | null
  transport_generation: number
  ready: boolean
  statuses: StreamStatus[]
  error: string | null
}

const EMPTY: StreamSnapshot = { target: null, endpoint: null, transport_generation: 0, ready: false, statuses: [], error: null }
const POLL_MS = 500
const SEARCH_MS = 120
const MICROS_PER_SECOND = 1_000_000
const MAX_INTERVAL_US = 2_147_483_647
const PANEL_MARGIN_PX = 10

function rateValue(text: string): { kind: 'hertz'; hz: number } | null {
  const hz = Number(text)
  const interval = MICROS_PER_SECOND / hz
  const wire = Math.fround(Math.round(interval))
  return text.trim() !== '' && Number.isFinite(hz) && hz > 0
    && interval >= 1 && interval <= MAX_INTERVAL_US && wire <= MAX_INTERVAL_US
    ? { kind: 'hertz', hz } : null
}
function scopeKey(snapshot: StreamSnapshot): string {
  return `${snapshot.target?.connection_id ?? 'none'}:${snapshot.transport_generation}`
}

interface Pending {
  key: string
  target: StreamTarget
  next: StreamRequest | null
}

export default function StreamManager({ tauri }: { tauri: boolean }) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<StandardStream[]>([])
  const [catalog, setCatalog] = useState<Record<number, StandardStream>>({})
  const [selected, setSelected] = useState<StandardStream | null>(null)
  const [rate, setRate] = useState('10')
  const [confirmed, setConfirmed] = useState(false)
  const [snapshot, setSnapshot] = useState<StreamSnapshot>(EMPTY)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<Set<number>>(new Set())
  const snapshotRef = useRef(snapshot)
  snapshotRef.current = snapshot
  const pending = useRef(new Map<number, Pending>())
  const mutation = useRef(0)
  const mounted = useRef(true)
  const panelRef = useRef<HTMLElement>(null)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false; pending.current.clear() }
  }, [])

  // Native scrolling does not account for the fixed capture bar or sticky
  // safety header around this independently scrolling workspace.
  useEffect(() => {
    if (!open) return
    const panel = panelRef.current
    const root = panel?.closest('.actuator-grid')
    if (!panel || !root) return
    let frame = 0
    const position = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        const mid = panel.closest<HTMLElement>('.actuator-mid')
        if (!mid) return
        const scrollPane = getComputedStyle(mid).overflowY === 'visible' ? root : mid
        const pane = scrollPane.getBoundingClientRect()
        const header = root.querySelector('.actuator-header')?.getBoundingClientRect()
        const drawer = root.querySelector('.logging-drawer')
        const drawerBounds = drawer?.getBoundingClientRect()
        const fixedDrawer = drawer && getComputedStyle(drawer).position === 'fixed'
        if (fixedDrawer && drawerBounds) {
          mid.style.setProperty('--stream-bottom-clearance', `${pane.bottom - drawerBounds.top + PANEL_MARGIN_PX * 2}px`)
        } else {
          mid.style.removeProperty('--stream-bottom-clearance')
        }
        const top = Math.max(pane.top, header?.bottom ?? 0) + PANEL_MARGIN_PX
        const bottom = Math.min(window.innerHeight, pane.bottom, fixedDrawer ? drawerBounds?.top ?? window.innerHeight : window.innerHeight) - PANEL_MARGIN_PX
        const bounds = panel.getBoundingClientRect()
        const desired = top + Math.max(0, (bottom - top - bounds.height) / 2)
        scrollPane.scrollTop += bounds.top - desired
      })
    }
    const observer = new ResizeObserver(position)
    for (const element of [panel, root, root.querySelector('.actuator-header'), root.querySelector('.logging-drawer')]) {
      if (element) observer.observe(element)
    }
    window.addEventListener('resize', position)
    position()
    return () => {
      cancelAnimationFrame(frame)
      observer.disconnect()
      window.removeEventListener('resize', position)
      panel.closest<HTMLElement>('.actuator-mid')?.style.removeProperty('--stream-bottom-clearance')
    }
  }, [open])

  useEffect(() => {
    if (!open || !tauri) return
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      const revision = mutation.current
      try {
        const next = await invoke<StreamSnapshot>('actuator_stream_status')
        if (!disposed && revision === mutation.current) {
          if (scopeKey(next) !== scopeKey(snapshotRef.current)) {
            setSelected(null)
            setConfirmed(false)
            setError(null)
          }
          snapshotRef.current = next
          setSnapshot(next)
        }
      } catch (error) { if (!disposed) setError(String(error)) }
      finally { if (!disposed) timer = setTimeout(() => { void poll() }, POLL_MS) }
    }
    void poll()
    return () => { disposed = true; clearTimeout(timer) }
  }, [open, tauri])

  useEffect(() => {
    if (!open || !tauri) return
    let disposed = false
    const timer = setTimeout(() => {
      void invoke<StandardStream[]>('actuator_stream_catalog', { query }).then((rows) => {
        if (disposed) return
        setResults(rows)
        setCatalog((previous) => ({ ...previous, ...Object.fromEntries(rows.map((row) => [row.id, row])) }))
      }).catch((error: unknown) => { if (!disposed) setError(String(error)) })
    }, SEARCH_MS)
    return () => { disposed = true; clearTimeout(timer) }
  }, [open, query, tauri])

  const apply = (request: StreamRequest) => {
    const current = snapshotRef.current
    if (!tauri || !current.ready || !current.target) return
    const id = request.message_id
    const key = scopeKey(current)
    const previous = pending.current.get(id)
    if (previous?.key === key) { previous.next = request; return }
    const job: Pending = { key, target: current.target, next: request }
    pending.current.set(id, job)
    setBusy(new Set(pending.current.keys()))
    setError(null)
    void (async () => {
      while (mounted.current && job.next && job.key === scopeKey(snapshotRef.current)) {
        const next = job.next
        job.next = null
        mutation.current += 1
        try {
          const status = await invoke<StreamStatus>('actuator_stream_set_rate', { target: job.target, request: next })
          if (mounted.current && job.key === scopeKey(snapshotRef.current)) {
            setSnapshot((previous) => {
              if (scopeKey(previous) !== job.key) return previous
              const updated = { ...previous, statuses: [...previous.statuses.filter((row) => row.message_id !== id), status].sort((a, b) => a.message_id - b.message_id) }
              snapshotRef.current = updated
              return updated
            })
          }
        } catch (error) {
          if (mounted.current && !job.next && job.key === scopeKey(snapshotRef.current)) setError(String(error))
        }
      }
      if (pending.current.get(id) === job) pending.current.delete(id)
      if (mounted.current) setBusy(new Set(pending.current.keys()))
    })()
  }

  const validRate = rateValue(rate)
  const sourceMatches = results.filter((message) => message.px4_mapping?.uorb_sources.some((source) => source.toLowerCase() === query.trim().toLowerCase()))
  const alreadyAdded = selected != null && snapshot.statuses.some((status) => status.message_id === selected.id)
  const rows = [...snapshot.statuses].sort((a, b) => Number(b.message_id === selected?.id) - Number(a.message_id === selected?.id) || a.message_id - b.message_id)
  return <>
    <button className="mode-btn scope-btn" type="button" title="Inspect standard MAVLink messages and configure telemetry rates for this target. This does not enable motor control." aria-expanded={open} aria-controls="motor-stream-panel" onClick={() => setOpen(true)}>Streams…</button>
    {open && <section ref={panelRef} className="stream-panel" id="motor-stream-panel" aria-label="MAVLink stream manager">
      <div className="stream-panel-heading">
        <div><strong>MAVLink streams</strong><div className="estop-note" data-testid="stream-target">
          {snapshot.target ? `${snapshot.endpoint} · PX4 ${snapshot.target.target_system_id}/${snapshot.target.target_component_id} · connection ${snapshot.target.connection_id}, generation ${snapshot.transport_generation}` : 'Connect the Motor Test PX4 target'}
        </div></div>
        <button className="mode-btn" type="button" title="Close the panel; saved stream requests continue on the current connection." aria-label="Close streams" onClick={() => setOpen(false)}>Close</button>
      </div>
      <div className="estop-note stream-notice">PX4 may clamp or ignore requests. Observed Hz requires received frames. ESC remains Emergency Stop.</div>
      {(error || snapshot.error) && <div role="alert" className="stream-error text-error">{error ?? snapshot.error}</div>}
      <div className="stream-columns">
        <div className="stream-search-pane">
          <input className="signal-search" aria-label="Search standard MAVLink messages" placeholder="Name, ID, field or uORB source" value={query} onChange={(event) => { setQuery(event.target.value); setResults([]); setSelected(null); setConfirmed(false) }} />
          {sourceMatches.length > 1 && <div className="estop-note text-warn" role="status">Shared uORB source: select one exact message ID from the results.</div>}
          <div className="stream-results" aria-label="Standard MAVLink messages">
            {results.map((message) => <button type="button" className={`stream-result mode-btn ${selected?.id === message.id ? 'active' : ''}`} key={message.id} onClick={() => { setSelected(message); setConfirmed(false) }}>
              <span>{message.name}</span><span className="mono text-muted">{message.id}</span>
            </button>)}
            {results.length === 0 && <div className="estop-note">No standard message matches. No verified standard mapping is available for this query. The installed firmware cannot expose arbitrary uORB topics through an interval request; select a documented standard message.</div>}
          </div>
        </div>
        <div className="stream-settings-pane">
          <div className="stream-selection">
            {selected ? <>
              <strong data-testid="stream-selected">{selected.name} · {selected.id}</strong>
              {selected.px4_mapping ? <div className="estop-note" data-testid="stream-uorb-mapping">
                <div>PX4 v1.17 uORB sources: {selected.px4_mapping.uorb_sources.map((source, index) => <span key={source}>{index > 0 && ', '}<code>{source}</code></span>)}</div>
                <p>{selected.px4_mapping.note}</p>
                <a href={selected.px4_mapping.source_url} target="_blank" rel="noopener noreferrer">PX4 v1.17 stream source ↗</a>
              </div> : <div className="estop-note">No verified uORB source mapping is recorded for this message.</div>}
              <details className="stream-fields"><summary>Fields and wire units ({selected.fields.length})</summary>
                {selected.fields.map((field) => <div key={field.name}><span>{field.name}</span> <code>{field.wire_type}</code> <span>{field.units ?? '—'}</span></div>)}
              </details>
              {selected.px4_support === 'unknown'
                ? <label className="stream-unknown text-warn"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />PX4 support unknown — request this standard message anyway</label>
                : <div className="estop-note">Documented in PX4 v1.17; availability depends on the installed firmware.</div>}
            </> : <div className="estop-note">Select a result to inspect its fields and request that exact message.</div>}
            {!alreadyAdded && <div className="stream-add-row">
              <label>Rate (Hz)<input className="phrase-input" aria-label="New stream rate (Hz)" type="number" step="any" value={rate} onChange={(event) => setRate(event.target.value)} /></label>
              <button className="mode-btn" type="button" title={!snapshot.ready ? 'Connect the target and wait for target traffic.' : !selected ? 'Select one exact message from the search results.' : !validRate ? 'Enter a finite positive frequency.' : selected.px4_support === 'unknown' && !confirmed ? 'Confirm unknown PX4 support before requesting this message.' : 'Request this exact standard message at the entered rate; verify actual receipt below.'} disabled={!snapshot.ready || !selected || !validRate || alreadyAdded || busy.has(selected.id) || (selected.px4_support === 'unknown' && !confirmed)} onClick={() => { if (selected && validRate) apply({ message_id: selected.id, rate: validRate, allow_unknown_support: confirmed }) }}>Add stream</button>
            </div>}
            {!alreadyAdded && !validRate && <div className="estop-note text-warn">Enter a finite positive rate with a representable interval.</div>}
            {alreadyAdded && <div className="estop-note">Already requested; edit its rate below.</div>}
          </div>
          <div className="stream-request-list" aria-label="Requested streams">
            {rows.map((status) => <StreamRow key={`${scopeKey(snapshot)}:${status.message_id}`} status={status} name={catalog[status.message_id]?.name ?? `Message ${status.message_id}`} ready={snapshot.ready} busy={busy.has(status.message_id)} onApply={apply} />)}
            {snapshot.statuses.length === 0 && <div className="estop-note">No requests on this connection.</div>}
          </div>
        </div>
      </div>
    </section>}
  </>
}

function StreamRow({ status, name, ready, busy, onApply }: {
  status: StreamStatus
  name: string
  ready: boolean
  busy: boolean
  onApply: (request: StreamRequest) => void
}) {
  const [draft, setDraft] = useState(status.requested_rate.kind === 'hertz' ? String(status.requested_rate.hz) : '')
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!busy && document.activeElement !== input.current && status.requested_rate.kind === 'hertz') setDraft(String(status.requested_rate.hz))
  }, [busy, status.requested_rate])
  const disabled = status.requested_rate.kind === 'disabled'
  const label = disabled ? (status.receive_state === 'receiving' ? 'Disable requested · still receiving' : status.receive_state === 'waiting' ? 'Disable requested · awaiting observation' : 'Disabled · no recent frames') : status.receive_state
  const send = (rate: StreamRate) => onApply({ message_id: status.message_id, rate, allow_unknown_support: true })
  return <article className="stream-row" data-testid={`stream-row-${status.message_id}`}>
    <div className="stream-row-heading"><strong>{name} · {status.message_id}</strong>
      <button className="mode-btn" type="button" title="Ask PX4 to stop this telemetry stream. Observed state shows whether frames actually stop." disabled={!ready || disabled || busy} aria-label={`Disable ${name}`} onClick={() => send({ kind: 'disabled' })}>Disable</button>
    </div>
    <div className="stream-row-data">
      <label>Requested Hz<input ref={input} className="phrase-input" aria-label={`Rate for ${name}`} type="number" step="any" value={draft} disabled={!ready} onChange={(event) => {
        setDraft(event.target.value)
        const rate = rateValue(event.target.value)
        if (rate && (busy || disabled || status.requested_rate.kind !== 'hertz' || rate.hz !== status.requested_rate.hz)) send(rate)
      }} /></label>
      <span>Observed <strong data-testid={`stream-hz-${status.message_id}`}>{status.observed_hz == null ? '—' : `${status.observed_hz.toFixed(1)} Hz`}</strong></span>
      <span className={status.receive_state === 'stale' && !disabled ? 'text-warn' : 'text-muted'} data-testid={`stream-state-${status.message_id}`}>{busy ? 'Requesting…' : label}</span>
    </div>
    <div className="estop-note">{status.last_seen_age_ms == null ? 'No frame received yet' : `Last frame ${(status.last_seen_age_ms / 1000).toFixed(1)} s ago`}{disabled ? ' · Edit Hz to enable' : ''}</div>
    {status.error && <div className="estop-note text-error">{status.error}</div>}
    {(status.error || (!disabled && status.receive_state === 'stale')) && <button className="mode-btn" type="button" disabled={!ready || busy} aria-label={`Reapply ${name}`} onClick={() => send(status.requested_rate)}>Reapply</button>}
  </article>
}
