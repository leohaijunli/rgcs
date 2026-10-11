// Parameters window — metadata-first design (operator request 2026-10-11):
// the list renders immediately from the bundled PX4 metadata; the current
// FC value is fetched lazily when a row is selected (params_read), and
// "Pull all" remains an explicit action for snapshot/diff workflows.
//
// Layout (plan §5.1, tightened — no blank placeholders): compact three-pane
// grid; the left pane holds PX4 groups and the operator's custom groups
// (add/rename/note/delete, kept separate from the PX4 tree).

import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isTauri } from '../inspector/mock'
import type { ParamEntry } from '../generated-types/ParamEntry'
import { useParamsStore, type ParamMeta, type ParamsIpcEvent } from './store'

/** The bundled metadata's target PX4 version (regenerate → bump). */
const META_VERSION = '1.17.0'

/** Enum option lookup: the description of value `v`, if the metadata has one. */
function enumDesc(
  meta: { values?: Array<{ value: number; description: string }> } | undefined,
  v: number,
): string | undefined {
  return meta?.values?.find((o) => o.value === v)?.description
}

/** Physical-meaning value text (operator request 2026-10-11): enum shows
 * `value · description`, numeric appends the metadata unit. */
function fmtValue(
  meta: { values?: Array<{ value: number; description: string }>; units?: string; decimalPlaces?: number } | undefined,
  v: number | null,
): string {
  if (v == null || !Number.isFinite(v)) return '\u2014'
  const d = enumDesc(meta, v)
  if (d) return `${v} \u00b7 ${d}`
  const dp = meta?.decimalPlaces
  const num = dp != null ? v.toFixed(dp) : String(v)
  return meta?.units ? `${num} ${meta.units}` : num
}

/** Default cell with the same physical formatting. */
function fmtDefault(meta: ParamMeta | undefined): string {
  return fmtValue(meta, meta?.default ?? null)
}

/** Fixed row height (compact tier, plan §5.2). */
const ROW_H = 26
/** Extra rows rendered above/below the viewport (virtualization slack). */
const OVERSCAN = 8

type Filter = 'all' | 'custom'

export function ParamsApp() {
  const tauri = isTauri()
  const store = useParamsStore
  const meta = store((s) => s.meta)
  const metaGroups = store((s) => s.metaGroups)
  const groups = store((s) => s.groups)
  const values = store((s) => s.values)
  const fcVersion = store((s) => s.fcVersion)
  const progress = store((s) => s.progress)
  const pulling = store((s) => s.pulling)
  const staged = store((s) => s.staged)

  const [selected, setSelected] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<Filter>('all')
  const [activeGroup, setActiveGroup] = useState<string | null>(null)
  /** 'px4' | 'custom' — which pane the group selection belongs to. */
  const [groupPane, setGroupPane] = useState<'px4' | 'custom'>('px4')
  const [leftTab, setLeftTab] = useState<'px4' | 'custom'>('px4')
  const [notice, setNotice] = useState<string | null>(null)
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  /** Value fetch state per name: 'idle' | 'loading' | 'done' | 'missing'. */
  const [readState, setReadState] = useState<
    Map<string, 'loading' | 'done' | 'missing'>
  >(new Map())
  /** Right-click menu on a row name: { name, x, y }. */
  const [ctxMenu, setCtxMenu] = useState<{ name: string; x: number; y: number } | null>(null)
  /** The detail editor draft (string so the input stays controllable). */
  const [draft, setDraft] = useState('')
  const [draftError, setDraftError] = useState<string | null>(null)
  /** Applying the staged tray via params_set (in flight). */
  const [applying, setApplying] = useState(false)
  /** Successfully applied names whose metadata says a reboot is required. */
  const [rebootNeeded, setRebootNeeded] = useState<string[]>([])
  const [rebooting, setRebooting] = useState(false)
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [viewH, setViewH] = useState(400)

  // Load the bundled metadata once (works without a link).
  useEffect(() => {
    let disposed = false
    void (async () => {
      try {
        const res = await fetch('/param-meta/parameters.json')
        const list = (await res.json()) as ParamMeta[]
        if (!disposed) store.getState().setMeta(list)
      } catch (e) {
        setNotice(`Metadata failed to load: ${String(e)}`)
      }
    })()
    return () => {
      disposed = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Ask the FC for its firmware version (mismatch notice below).
  useEffect(() => {
    if (!tauri) return
    void invoke<{ fcVersion: string | null }>('params_fc_version' as never)
      .then((r) => {
        if (r?.fcVersion) store.setState({ fcVersion: r.fcVersion })
      })
      .catch(() => {})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri])

  // IPC events: pull batches/progress + external changes.
  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<ParamsIpcEvent>('params_event', (e) => {
          if (disposed) return
          store.getState().onEvent(e.payload)
        })
      } catch {
        /* no events in browser mode */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri])

  // Reset the draft whenever the selection changes.
  useEffect(() => {
    const v = selected ? values.get(selected) : undefined
    setDraft(v ? JSON.stringify(v.value) : '')
    setDraftError(null)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected])

  // Lazy value fetch on selection.
  const selectRow = (name: string) => {
    setSelected(name)
    if (!tauri) return
    if (values.has(name) || readState.get(name) === 'loading') return
    setReadState((prev) => new Map(prev).set(name, 'loading'))
    void (async () => {
      try {
        const entry = await invoke<ParamEntry | null>('params_read', { name })
        setReadState((prev) => new Map(prev).set(name, entry ? 'done' : 'missing'))
        if (entry) store.setState((s) => ({ values: new Map(s.values).set(name, entry) }))
      } catch (e) {
        setReadState((prev) => new Map(prev).set(name, 'missing'))
        setNotice(String(e))
      }
    })()
  }

  /** Validate the draft against the metadata; returns the numeric value or
   * an error string. */
  const validateDraft = (): number | string => {
    if (!selectedMeta) return ''
    if (selectedMeta.values && selectedMeta.values.length > 0) {
      const n = Number(draft)
      if (!selectedMeta.values.some((o) => o.value === n))
        return `Not an enum value (${selectedMeta.values.map((o) => o.value).join(', ')})`
      return n
    }
    if (selectedMeta.type === 'Float32') {
      const f = Number(draft)
      if (!Number.isFinite(f)) return 'Not a number'
      if (selectedMeta.min != null && f < selectedMeta.min) return `Min ${selectedMeta.min}`
      if (selectedMeta.max != null && f > selectedMeta.max) return `Max ${selectedMeta.max}`
      return f
    }
    const n = Number(draft)
    if (!Number.isInteger(n)) return 'Not an integer'
    if (selectedMeta.min != null && n < selectedMeta.min) return `Min ${selectedMeta.min}`
    if (selectedMeta.max != null && n > selectedMeta.max) return `Max ${selectedMeta.max}`
    return n
  }

  const stageDraft = () => {
    if (!selected) return
    const v = validateDraft()
    if (typeof v === 'string') {
      setDraftError(v)
      return
    }
    setDraftError(null)
    store.setState((s) => ({ staged: new Map(s.staged).set(selected, v) }))
  }

  const applyStaged = async () => {
    if (!tauri || staged.size === 0) return
    setApplying(true)
    setNotice(null)
    const errors: string[] = []
    const done: string[] = []
    for (const [name, v] of staged) {
      try {
        const m = meta.get(name)
        // The pulled ParamEntry is the authoritative type (u8…real32 — the
        // ts-rs literal union matches serde's rename_all); PX4 metadata only
        // distinguishes Float32/Int32, so it is just the never-read fallback.
        const ptype = values.get(name)?.ptype ?? (m?.type === 'Float32' ? 'real32' : 'i32')
        const num = Number(v)
        const value = ptype === 'real32' ? { real: num } : { int: num }
        const echo = await invoke<{ status: string }>('params_set', { name, value, ptype })
        if (echo.status === 'clamped') errors.push(`${name}: clamped`)
        else if (echo.status === 'timeout') errors.push(`${name}: timeout`)
        else done.push(name)
      } catch (e) {
        errors.push(`${name}: ${String(e)}`)
      }
    }
    // Remove the successfully applied names from the tray.
    store.setState((s) => {
      const staged = new Map(s.staged)
      for (const n of done) staged.delete(n)
      return { staged }
    })
    setApplying(false)
    const rebootList = done.filter((n) => meta.get(n)?.rebootRequired === true)
    if (rebootList.length > 0) setRebootNeeded(rebootList)
    setNotice(
      errors.length > 0
        ? errors.join(' · ')
        : `${done.length} parameter(s) applied`,
    )
  }

  const rebootFc = async () => {
    if (!tauri) return
    setRebooting(true)
    try {
      const result = await invoke<string>('params_reboot')
      if (result === 'accepted')
        setNotice('Reboot accepted — the link will drop; re-pull after reconnect.')
      else if (result === 'denied')
        // SITL denies PREFLIGHT_REBOOT_SHUTDOWN (a NuttX-only code path).
        setNotice('FC rejected the reboot (denied) — SITL does not support reboot')
      else setNotice(`Reboot result: ${result}`)
    } catch (e) {
      setNotice(String(e))
    } finally {
      setRebooting(false)
    }
  }

  const pullAll = async () => {
    if (!tauri) return
    setNotice(null)
    try {
      await invoke('params_pull')
    } catch (e) {
      setNotice(String(e))
    }
  }
  const cancelPull = async () => {
    if (!tauri) return
    await invoke('params_cancel').catch(() => {})
  }

  /** The filtered, sorted rows for the table. */
  const rows = useMemo(() => {
    const q = query.trim().toLowerCase()
    let names: string[]
    if (activeGroup) {
      names =
        groupPane === 'custom'
          ? (groups.find((g) => g.id === activeGroup)?.members ?? [])
          : (metaGroups.get(activeGroup) ?? [])
    } else {
      names = [...meta.keys()]
    }
    const out: string[] = []
    for (const n of names) {
      const m = meta.get(n)
      if (!m) continue
      if (q && !n.toLowerCase().includes(q) && !(m.shortDesc ?? '').toLowerCase().includes(q))
        continue
      if (filter === 'custom') {
        if (!groups.some((g) => g.members.includes(n))) continue
      }
      out.push(n)
    }
    out.sort()
    return out
  }, [meta, metaGroups, groups, values, activeGroup, groupPane, query, filter])

  // Virtualization window.
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN)
  const visibleCount = Math.ceil(viewH / ROW_H) + OVERSCAN * 2
  const slice = rows.slice(first, first + visibleCount)

  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const ro = new ResizeObserver(() => setViewH(el.clientHeight))
    ro.observe(el)
    setViewH(el.clientHeight)
    return () => ro.disconnect()
  }, [])

  // A group/pane switch changes the row set: snap back to the top, or a
  // deep scroll position renders past the end of a short group and the
  // list looks like it never switched.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0
    setScrollTop(0)
  }, [activeGroup, groupPane])

  const selectedMeta = selected ? meta.get(selected) : undefined
  const selectedValue = selected ? values.get(selected) : undefined

  const groupCounts = useMemo(() => {
    const counts = new Map<string, number>()
    for (const g of groups) counts.set(g.id, g.members.length)
    return counts
  }, [groups])

  // ---- left pane: PX4 groups / custom groups -------------------------------
  const leftPane = (
    <div className="p-left">
      <div className="p-left-tabs">
        <button
          className={`p-tab ${leftTab === 'px4' ? 'active' : ''}`}
          onClick={() => {
            setLeftTab('px4')
            // Coming from a custom group: back to All (a custom selection
            // has no meaning in the PX4 tree).
            if (groupPane === 'custom') {
              setActiveGroup(null)
              setGroupPane('px4')
            }
          }}
        >
          PX4
        </button>
        <button
          className={`p-tab ${leftTab === 'custom' ? 'active' : ''}`}
          onClick={() => {
            setLeftTab('custom')
            // Coming from a PX4 view: jump to the first custom group (or
            // clear, when none exist) so the pane and the list agree.
            if (groupPane !== 'custom') {
              setActiveGroup(groups.length > 0 ? groups[0].id : null)
              setGroupPane('custom')
            }
          }}
        >
          Custom
        </button>
      </div>
      <div className="p-left-scroll">
        {leftTab === 'px4' ? (
          <>
            <div
              className={`p-group ${activeGroup === null ? 'active' : ''}`}
              onClick={() => {
                setActiveGroup(null)
                setGroupPane('px4')
              }}
            >
              All parameters <span className="p-count">{meta.size}</span>
            </div>
            {[...metaGroups.keys()].sort().map((g) => (
              <div
                key={g}
                className={`p-group ${groupPane === 'px4' && activeGroup === g ? 'active' : ''}`}
                onClick={() => {
                  setActiveGroup(g)
                  setGroupPane('px4')
                }}
              >
                {g} <span className="p-count">{metaGroups.get(g)?.length}</span>
              </div>
            ))}
          </>
        ) : (
          <>
            <button
              className="p-new-group"
              onClick={() => store.getState().addGroup(`Group ${groups.length + 1}`, '')}
            >
              + New group
            </button>
            {groups.map((g) => (
              <div key={g.id} className={`p-cgroup ${activeGroup === g.id ? 'active' : ''}`}>
                <div
                  className="p-cgroup-head"
                  onClick={() => {
                    setActiveGroup(g.id)
                    setGroupPane('custom')
                  }}
                >
                  <input
                    className="p-cgroup-name"
                    value={g.name}
                    title="Group name"
                    onChange={(e) => store.getState().renameGroup(g.id, e.target.value)}
                    onClick={(e) => e.stopPropagation()}
                  />
                  <button
                    className="p-cgroup-del"
                    title="Delete group"
                    onClick={(e) => {
                      e.stopPropagation()
                      setConfirmDelete(g.id)
                    }}
                  >
                    ×
                  </button>
                </div>
                <input
                  className="p-cgroup-note"
                  value={g.note}
                  placeholder="Note…"
                  title="Group note"
                  onChange={(e) => store.getState().setGroupNote(g.id, e.target.value)}
                />
                <div className="p-cgroup-info" title="Members">
                  {groupCounts.get(g.id) ?? 0} members
                </div>
                {confirmDelete === g.id && (
                  <div className="p-cgroup-confirm">
                    <span>Delete?</span>
                    <button
                      onClick={() => {
                        store.getState().deleteGroup(g.id)
                        setConfirmDelete(null)
                        if (activeGroup === g.id) setActiveGroup(null)
                      }}
                    >
                      Delete
                    </button>
                    <button onClick={() => setConfirmDelete(null)}>Keep</button>
                  </div>
                )}
              </div>
            ))}
            {groups.length === 0 && (
              <div className="p-hint">
                Custom groups hold the parameters you actually touch — new
                group, then add members from the table's + button.
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )

  return (
    <div className="p-grid">
      <header className="p-toolbar">
        <strong className="p-title">Parameters</strong>
        <button className="p-btn" onClick={() => void pullAll()} disabled={pulling}>
          {pulling
            ? `Pulling ${progress ? `${progress.received}/${progress.total}` : '…'}`
            : 'Pull all'}
        </button>
        {pulling && (
          <button className="p-btn" onClick={() => void cancelPull()}>
            Cancel
          </button>
        )}
        <input
          className="p-search"
          placeholder="Search name or description…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <span className="p-hitcount">
          {rows.length}/{meta.size}
        </span>
        <div className="p-filter">
          {(
            [
              ['all', 'All'],
              ['custom', 'Custom'],
            ] as Array<[Filter, string]>
          ).map(([id, label]) => (
            <button
              key={id}
              className={`p-tab ${filter === id ? 'active' : ''}`}
              onClick={() => setFilter(id)}
            >
              {label}
            </button>
          ))}
        </div>
        <span className="p-status">
          {notice ?? `${values.size} values${staged.size ? ` · ${staged.size} staged` : ''}`}
        </span>
      </header>

      {fcVersion != null && !fcVersion.startsWith(META_VERSION) && (
        <div className="p-version-note">
          FC runs PX4 {fcVersion} but the bundled metadata is from PX4 {META_VERSION} —
          descriptions may be outdated; unknown parameters stay editable.
        </div>
      )}
      {fcVersion != null && fcVersion.startsWith(META_VERSION) && (
        <div className="p-version-ok mono">PX4 {fcVersion}</div>
      )}
      {rebootNeeded.length > 0 && (
        <div className="p-reboot-banner">
          <span>
            ⟲ Reboot required for: <span className="mono">{rebootNeeded.join(', ')}</span>
          </span>
          <div className="p-reboot-banner-actions">
            <button
              className="p-btn p-reboot-go"
              onClick={() => void rebootFc()}
              disabled={rebooting || !tauri}
            >
              {rebooting ? 'Rebooting…' : 'Reboot FC'}
            </button>
            <button className="p-btn" onClick={() => setRebootNeeded([])}>
              Dismiss
            </button>
          </div>
        </div>
      )}
      <div className="p-main">
        {leftPane}

        <div className="p-tablewrap">
          <div className="p-thead">
            <span>Name</span>
            <span>Value</span>
            <span>Default</span>
            <span>Description</span>
          </div>
          <div
            className="p-scroll"
            ref={scrollRef}
            onScroll={(e) => setScrollTop((e.target as HTMLDivElement).scrollTop)}
          >
            {rows.length === 0 ? (
              <div className="p-empty">
                {pulling
                  ? 'Receiving parameters…'
                  : query
                    ? `No match for "${query}" — clear the search.`
                    : 'No parameters in this group.'}
              </div>
            ) : (
              <div style={{ height: rows.length * ROW_H, position: 'relative' }}>
                {slice.map((name, i) => {
                  const m = meta.get(name)!
                  const v = values.get(name)
                  const rs = readState.get(name)
                  const isSel = selected === name
                  return (
                    <div
                      key={name}
        className={`p-row ${isSel ? 'sel' : ''}`}
                      style={{ position: 'absolute', top: (first + i) * ROW_H, height: ROW_H }}
                      onClick={() => selectRow(name)}
                    >
                      <span
                        className="mono p-name"
                        onContextMenu={(e) => {
                          e.preventDefault()
                          setCtxMenu({ name, x: e.clientX, y: e.clientY })
                        }}
                      >
                        {m.rebootRequired && <span className="p-reboot" title="Reboot required">⟲</span>}
                        {name}
                      </span>
                      <span className="p-value">
                        {v
                          ? fmtValue(m, typeof v.value === 'number' ? v.value : null)
                          : rs === 'loading'
                            ? '…'
                            : rs === 'missing'
                              ? '—'
                              : '·'}
                      </span>
                      <span className="p-default">{fmtDefault(m)}</span>
                      <span className="p-desc" title={m.longDesc ?? m.shortDesc ?? ''}>
                        {m.shortDesc ?? ''}
                      </span>
                    </div>
                  )
                })}
              </div>
            )}
          </div>
        </div>

        <aside className="p-detail">
          {selected ? (
            <>
              <div className="p-detail-title mono">{selected}</div>
              <div className="p-detail-desc">
                {selectedMeta?.longDesc ?? selectedMeta?.shortDesc ?? 'No description.'}
              </div>
              <dl className="p-detail-list">
                <div>
                  <dt>Value</dt>
                  <dd>
                    {selectedValue
                      ? fmtValue(selectedMeta, typeof selectedValue.value === 'number' ? selectedValue.value : null)
                      : readState.get(selected) === 'loading'
                        ? 'Fetching…'
                        : 'Select a row to fetch'}
                  </dd>
                </div>
                {selectedMeta?.units && (
                  <div>
                    <dt>Unit</dt>
                    <dd>{selectedMeta.units}</dd>
                  </div>
                )}
                <div>
                  <dt>Default</dt>
                  <dd>{fmtDefault(selectedMeta)}</dd>
                </div>
                <div>
                  <dt>Type</dt>
                  <dd>{selectedMeta?.type ?? '—'}</dd>
                </div>
                {(selectedMeta?.min != null || selectedMeta?.max != null) && (
                  <div>
                    <dt>Range</dt>
                    <dd className="mono">
                      {selectedMeta?.min ?? '−∞'} … {selectedMeta?.max ?? '∞'}
                    </dd>
                  </div>
                )}
                <div>
                  <dt>Group</dt>
                  <dd>{selectedMeta?.group ?? '—'}</dd>
                </div>
                <div>
                  <dt>Reboot</dt>
                  <dd>{selectedMeta?.rebootRequired ? 'Required' : 'No'}</dd>
                </div>
              </dl>
              {selectedMeta?.values && selectedMeta.values.length > 0 && (
                <div className="p-detail-enum">
                  <div className="p-detail-h">Enum values</div>
                  {selectedMeta.values.map((o) => (
                    <div key={o.value} className="p-enum-row">
                      <span className="mono">{o.value}</span>
                      <span>{o.description}</span>
                    </div>
                  ))}
                </div>
              )}
              {selectedMeta?.rebootRequired && (
                <div className="p-reboot-note">
                  \u27f2 Changing this parameter requires an FC restart to
                  take effect.
                </div>
              )}
              <div className="p-detail-h">Edit</div>
              {selectedMeta?.values && selectedMeta.values.length > 0 ? (
                <select
                  className="p-edit-select"
                  value={draft}
                  onChange={(e) => {
                    setDraft(e.target.value)
                    setDraftError(null)
                  }}
                >
                  <option value="">—</option>
                  {selectedMeta.values.map((o) => (
                    <option key={o.value} value={String(o.value)}>
                      {o.value} · {o.description}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  className={`p-edit-input ${draftError ? 'p-edit-error' : ''}`}
                  value={draft}
                  inputMode={selectedMeta?.type === 'Float32' ? 'decimal' : 'numeric'}
                  onChange={(e) => {
                    setDraft(e.target.value)
                    setDraftError(null)
                  }}
                  onKeyDown={(e) => e.key === 'Enter' && stageDraft()}
                  placeholder={String(selectedMeta?.default ?? '')}
                  aria-label="New value"
                />
              )}
              {draftError && <div className="p-edit-error-text">{draftError}</div>}
              <div className="p-detail-actions">
                <button
                  className="p-btn"
                  onClick={stageDraft}
                  disabled={!tauri || draft === ''}
                  title="Stage the new value (applied from the tray below)"
                >
                  Stage
                </button>
                <button
                  className="p-btn"
                  onClick={() => {
                    setDraft(selectedMeta?.default != null ? String(selectedMeta.default) : '')
                    setDraftError(null)
                  }}
                  disabled={selectedMeta?.default == null}
                  title="Reset the draft to the default value"
                >
                  Reset
                </button>
                <button
                  className="p-btn"
                  onClick={() => void navigator.clipboard?.writeText(selected)}
                >
                  Copy
                </button>
              </div>
            </>
          ) : (
            <div className="p-detail-empty">
              Select a parameter to edit it. Right-click a name to add it
              to a group.
            </div>
          )}
        </aside>
      </div>

      {staged.size > 0 && (
        <div className="p-tray">
          <div className="p-tray-list">
            {[...staged.entries()].map(([name, v]) => (
              <span key={name} className="p-tray-item">
                <span className="mono">{name}</span>
                <span className="p-tray-arrow">→</span>
                <span className="mono">{v}</span>
                <button
                  className="p-tray-del"
                  title="Remove from tray"
                  onClick={() =>
                    store.setState((s) => {
                      const staged = new Map(s.staged)
                      staged.delete(name)
                      return { staged }
                    })
                  }
                >
                  ×
                </button>
              </span>
            ))}
          </div>
          <button
            className="p-btn p-tray-apply"
            onClick={() => void applyStaged()}
            disabled={applying || !tauri}
          >
            {applying ? 'Applying…' : `Apply ${staged.size}`}
          </button>
        </div>
      )}

      {ctxMenu && (
        <>
          <div className="p-ctx-backdrop" onClick={() => setCtxMenu(null)} onContextMenu={(e) => { e.preventDefault(); setCtxMenu(null) }} />
          <div className="p-ctx" style={{ left: ctxMenu.x, top: ctxMenu.y }}>
            <button
              className="p-ctx-item"
              onClick={() => {
                store.getState().addGroup(`${ctxMenu.name}`, '')
                // Add to the newest group (just created last).
                const gs = store.getState().groups
                const g = gs[gs.length - 1]
                if (g) store.getState().addToGroup(g.id, ctxMenu.name)
                setCtxMenu(null)
                setLeftTab('custom')
              }}
            >
              New group from “{ctxMenu.name}”
            </button>
            {groups.length > 0 && <div className="p-ctx-sep" />}
            {groups.map((g) => (
              <button
                key={g.id}
                className="p-ctx-item"
                onClick={() => {
                  store.getState().addToGroup(g.id, ctxMenu.name)
                  setCtxMenu(null)
                }}
              >
                Add to “{g.name}”
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
