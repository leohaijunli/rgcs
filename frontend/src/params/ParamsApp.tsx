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

/** Fixed row height (compact tier, plan §5.2). */
const ROW_H = 26
/** Extra rows rendered above/below the viewport (virtualization slack). */
const OVERSCAN = 8

type Filter = 'all' | 'changed' | 'custom'

export function ParamsApp() {
  const tauri = isTauri()
  const store = useParamsStore
  const meta = store((s) => s.meta)
  const metaGroups = store((s) => s.metaGroups)
  const groups = store((s) => s.groups)
  const values = store((s) => s.values)
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
      if (filter === 'changed') {
        const v = values.get(n)
        const d = m.default
        if (!v || d == null) continue
        if (JSON.stringify(v.value) === JSON.stringify(d)) continue
      }
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
              ['changed', 'Changed'],
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
                      <span className="mono p-name">
                        {m.rebootRequired && <span className="p-reboot" title="Reboot required">⟲</span>}
                        {name}
                      </span>
                      <span className="mono p-value">
                        {v
                          ? JSON.stringify(v.value)
                          : rs === 'loading'
                            ? '…'
                            : rs === 'missing'
                              ? '—'
                              : '·'}
                      </span>
                      <span className="mono p-default">{m.default ?? '—'}</span>
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
                  <dd className="mono">
                    {selectedValue
                      ? JSON.stringify(selectedValue.value)
                      : readState.get(selected) === 'loading'
                        ? 'Fetching…'
                        : 'Select a row to fetch'}
                  </dd>
                </div>
                <div>
                  <dt>Default</dt>
                  <dd className="mono">{selectedMeta?.default ?? '—'}</dd>
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
              <div className="p-detail-actions">
                <button
                  className="p-btn"
                  onClick={() => void navigator.clipboard?.writeText(selected)}
                >
                  Copy name
                </button>
                {groups.length > 0 && (
                  <select
                    className="p-addsel"
                    title="Add to custom group"
                    value=""
                    onChange={(e) => {
                      if (e.target.value) {
                        store.getState().addToGroup(e.target.value, selected)
                        setNotice(`${selected} added to ${
                          groups.find((g) => g.id === e.target.value)?.name ?? ''
                        }`)
                      }
                    }}
                  >
                    <option value="">Add to group…</option>
                    {groups.map((g) => (
                      <option key={g.id} value={g.id}>
                        {g.name}
                      </option>
                    ))}
                  </select>
                )}
              </div>
            </>
          ) : (
            <div className="p-detail-empty">
              Select a parameter to fetch its value from the FC.
            </div>
          )}
        </aside>
      </div>
    </div>
  )
}
