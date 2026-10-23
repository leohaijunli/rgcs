// Parameters window — metadata-first design (operator request 2026-10-11):
// the list renders immediately from the bundled PX4 metadata; the current
// FC value is fetched lazily when a row is selected (params_read), and
// "Pull all" remains an explicit action for snapshot/diff workflows.
//
// Layout (plan §5.1, tightened — no blank placeholders): compact three-pane
// grid; the left pane holds PX4 groups and the operator's custom groups
// (add/rename/note/delete, kept separate from the PX4 tree).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { ChevronsLeft, ChevronsRight } from 'lucide-react'
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

/** Fixed row height fallback (compact tier, plan §5.2). The live height is
 * read from `--p-row-h` so Field mode can enlarge rows (Params-5). */
const ROW_H = 26
/** Extra rows rendered above/below the viewport (virtualization slack). */
const OVERSCAN = 8

type Filter = 'all' | 'custom'

/** Pane geometry (Params-3). Values mirror the previous fixed columns
 * (190 px group pane, 230 px editor) so an untouched layout looks unchanged. */
interface LayoutPrefs {
  version: 1
  leftW: number
  rightW: number
  leftCollapsed: boolean
  rightCollapsed: boolean
}

const LAYOUT_KEY = 'maggcs.params.layout'
/** A pane never narrows past this, so its tabs/search stay usable. */
const MIN_PANE_W = 140
/** A pane never widens past this, so the table keeps most of the width. */
const MAX_PANE_W = 420
/** The table keeps at least this width so the `--p-param-cols` minimums fit. */
const MIN_CENTER_W = 460

const RAIL_W = 24
const SPLITTER_W = 4
/** Keyboard nudge per arrow press on a splitter. */
const PANE_STEP = 8

const DEFAULT_LAYOUT: LayoutPrefs = {
  version: 1,
  leftW: 180,
  rightW: 220,
  leftCollapsed: false,
  rightCollapsed: false,
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n))
}

function readLayout(): LayoutPrefs {
  try {
    const raw = localStorage.getItem(LAYOUT_KEY)
    if (!raw) return DEFAULT_LAYOUT
    const p = JSON.parse(raw) as Partial<LayoutPrefs>
    return {
      version: 1,
      leftW: typeof p.leftW === 'number' ? p.leftW : DEFAULT_LAYOUT.leftW,
      rightW: typeof p.rightW === 'number' ? p.rightW : DEFAULT_LAYOUT.rightW,
      leftCollapsed: p.leftCollapsed === true,
      rightCollapsed: p.rightCollapsed === true,
    }
  } catch {
    return DEFAULT_LAYOUT
  }
}

function saveLayout(p: LayoutPrefs): void {
  try {
    localStorage.setItem(LAYOUT_KEY, JSON.stringify(p))
  } catch {
    /* private mode */
  }
}

/** Draggable pane divider (Params-3), shared by the group pane and the editor. */
function PaneSplitter({
  side,
  getWidth,
  onResize,
  onReset,
}: {
  side: 'left' | 'right'
  getWidth: () => number
  onResize: (px: number) => void
  onReset: () => void
}) {
  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault()
    const startX = e.clientX
    const startW = getWidth()
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - startX
      onResize(side === 'left' ? startW + dx : startW - dx)
    }
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }
  const label = side === 'left' ? 'Resize group pane' : 'Resize editor'
  return (
    <div
      className={`p-split ${side}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      title={`${label} · double-click to reset`}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onDoubleClick={onReset}
      onKeyDown={(e) => {
        const step = e.key === 'ArrowRight' ? PANE_STEP : e.key === 'ArrowLeft' ? -PANE_STEP : 0
        if (step === 0) return
        e.preventDefault()
        onResize(getWidth() + (side === 'left' ? step : -step))
      }}
    />
  )
}

/** Narrow strip shown in place of a collapsed pane; its button reopens it. */
function PaneRail({ side, title, onOpen }: { side: 'left' | 'right'; title: string; onOpen: () => void }) {
  const Icon = side === 'left' ? ChevronsRight : ChevronsLeft
  return (
    <div className={`p-rail ${side}`}>
      <button className="rail-toggle" title={title} aria-label={title} onClick={onOpen}>
        <Icon size={16} />
      </button>
    </div>
  )
}

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
  /** Scrollbar gutter width, so the fixed header can match the body's content
   * box (WP3a: header and rows must share column origins within 1 px). */
  const [gutter, setGutter] = useState(0)
  /** Live data-row height, read from `--p-row-h` (Params-5): Field mode
   * enlarges it, and the virtualizer must track that, not a fixed 26. */
  const [rowH, setRowH] = useState(ROW_H)

  /** Pane geometry (Params-3): resizable/collapsible group pane + editor. */
  const initialLayout = useMemo(readLayout, [])
  const [leftW, setLeftW] = useState(initialLayout.leftW)
  const [rightW, setRightW] = useState(initialLayout.rightW)
  const [leftCollapsed, setLeftCollapsed] = useState(initialLayout.leftCollapsed)
  const [rightCollapsed, setRightCollapsed] = useState(initialLayout.rightCollapsed)
  const leftWRef = useRef(leftW)
  leftWRef.current = leftW
  const rightWRef = useRef(rightW)
  rightWRef.current = rightW
  const leftCollapsedRef = useRef(leftCollapsed)
  leftCollapsedRef.current = leftCollapsed
  const rightCollapsedRef = useRef(rightCollapsed)
  rightCollapsedRef.current = rightCollapsed

  /** Largest width a pane may take while the table keeps MIN_CENTER_W. */
  const maxPane = useCallback((otherW: number, otherCollapsed: boolean) => {
    const other = otherCollapsed ? RAIL_W : otherW
    const avail = window.innerWidth - MIN_CENTER_W - other - 2 * SPLITTER_W
    return clamp(avail, MIN_PANE_W, MAX_PANE_W)
  }, [])
  const resizeLeft = useCallback(
    (px: number) => setLeftW(clamp(px, MIN_PANE_W, maxPane(rightWRef.current, rightCollapsedRef.current))),
    [maxPane],
  )
  const resizeRight = useCallback(
    (px: number) => setRightW(clamp(px, MIN_PANE_W, maxPane(leftWRef.current, leftCollapsedRef.current))),
    [maxPane],
  )

  // Persist the pane layout after every change.
  useEffect(() => {
    saveLayout({ version: 1, leftW, rightW, leftCollapsed, rightCollapsed })
  }, [leftW, rightW, leftCollapsed, rightCollapsed])

  // Re-clamp on window resize so the table never drops below MIN_CENTER_W.
  useEffect(() => {
    const onResize = () => {
      resizeLeft(leftWRef.current)
      resizeRight(rightWRef.current)
    }
    window.addEventListener('resize', onResize)
    onResize()
    return () => window.removeEventListener('resize', onResize)
  }, [resizeLeft, resizeRight])

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
  const first = Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN)
  const visibleCount = Math.ceil(viewH / rowH) + OVERSCAN * 2
  const slice = rows.slice(first, first + visibleCount)

  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const sync = () => {
      setViewH(el.clientHeight)
      // offsetWidth - clientWidth is the reserved scrollbar gutter (0 for
      // overlay scrollbars, so header and body stay consistent either way).
      setGutter(el.offsetWidth - el.clientWidth)
    }
    const ro = new ResizeObserver(sync)
    ro.observe(el)
    sync()
    return () => ro.disconnect()
  }, [])

  // Track the declared row height (`--p-row-h`). Field mode toggles the
  // `field-mode` class on <html>, so observe that too (Params-5).
  useEffect(() => {
    const read = () => {
      const v = parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue('--p-row-h'),
      )
      setRowH(Number.isFinite(v) && v > 0 ? v : ROW_H)
    }
    read()
    const mo = new MutationObserver(read)
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
    return () => mo.disconnect()
  }, [])

  /** Keep a row within the viewport after keyboard navigation. */
  const scrollRowIntoView = (index: number) => {
    const el = scrollRef.current
    if (!el) return
    const top = index * rowH
    const bottom = top + rowH
    if (top < el.scrollTop) el.scrollTop = top
    else if (bottom > el.scrollTop + el.clientHeight) el.scrollTop = bottom - el.clientHeight
  }

  // A group/pane switch changes the row set: snap back to the top, or a
  // deep scroll position renders past the end of a short group and the
  // list looks like it never switched.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0
    setScrollTop(0)
  }, [activeGroup, groupPane])

  const selectedMeta = selected ? meta.get(selected) : undefined
  const selectedValue = selected ? values.get(selected) : undefined

  /** Keyboard navigation over the virtualized list (Params-5). */
  const selectedIndex = selected ? rows.indexOf(selected) : -1
  const onListKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (rows.length === 0) return
    const page = Math.max(1, Math.floor(viewH / rowH))
    let next = selectedIndex
    switch (e.key) {
      case 'ArrowDown':
        next = selectedIndex < 0 ? 0 : selectedIndex + 1
        break
      case 'ArrowUp':
        next = selectedIndex < 0 ? rows.length - 1 : selectedIndex - 1
        break
      case 'PageDown':
        next = (selectedIndex < 0 ? 0 : selectedIndex) + page
        break
      case 'PageUp':
        next = (selectedIndex < 0 ? 0 : selectedIndex) - page
        break
      case 'Home':
        next = 0
        break
      case 'End':
        next = rows.length - 1
        break
      default:
        return
    }
    e.preventDefault()
    const idx = clamp(next, 0, rows.length - 1)
    selectRow(rows[idx])
    scrollRowIntoView(idx)
  }

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
          title="Browse the built-in PX4 parameter groups"
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
          title="Show the groups you built (separate from the PX4 tree)"
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
          My groups
        </button>
        <button
          className="p-pane-collapse"
          title="Hide group pane"
          aria-label="Hide group pane"
          onClick={() => setLeftCollapsed(true)}
        >
          <ChevronsLeft size={14} />
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

  const gridStyle = {
    '--p-left': `${leftCollapsed ? RAIL_W : leftW}px`,
    '--p-left-split': leftCollapsed ? '0px' : `${SPLITTER_W}px`,
    '--p-right': `${rightCollapsed ? RAIL_W : rightW}px`,
    '--p-right-split': rightCollapsed ? '0px' : `${SPLITTER_W}px`,
  } as React.CSSProperties

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
        <div className="p-filter" role="group" aria-label="Membership filter">
          {(
            [
              ['all', 'All', 'Show every parameter'],
              ['custom', 'In groups', 'Show only parameters that are members of one of your groups'],
            ] as Array<[Filter, string, string]>
          ).map(([id, label, title]) => (
            <button
              key={id}
              className={`p-tab ${filter === id ? 'active' : ''}`}
              title={title}
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
      <div className="p-main" style={gridStyle}>
        {leftCollapsed ? (
          <PaneRail side="left" title="Show group pane" onOpen={() => setLeftCollapsed(false)} />
        ) : (
          <>
            {leftPane}
            <PaneSplitter
              side="left"
              getWidth={() => leftWRef.current}
              onResize={resizeLeft}
              onReset={() => resizeLeft(DEFAULT_LAYOUT.leftW)}
            />
          </>
        )}

        <div className="p-tablewrap">
          <div className="p-thead" style={{ paddingRight: 8 + gutter }}>
            <span>Name</span>
            <span>Value</span>
            <span>Default</span>
            <span>Description</span>
          </div>
          <div
            className="p-scroll"
            ref={scrollRef}
            role="listbox"
            aria-label="Parameters"
            tabIndex={0}
            aria-activedescendant={
              selectedIndex >= first && selectedIndex < first + slice.length
                ? `p-opt-${selectedIndex}`
                : undefined
            }
            onKeyDown={onListKeyDown}
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
              <div
                role="presentation"
                style={{ height: rows.length * rowH, position: 'relative' }}
              >
                {slice.map((name, i) => {
                  const m = meta.get(name)!
                  const v = values.get(name)
                  const rs = readState.get(name)
                  const isSel = selected === name
                  // Full cell text as a title so a clipped value is still
                  // reachable on hover; the detail pane shows it on selection.
                  const valueText = v
                    ? fmtValue(m, typeof v.value === 'number' ? v.value : null)
                    : rs === 'loading'
                      ? '…'
                      : rs === 'missing'
                        ? '—'
                        : '·'
                  const defaultText = fmtDefault(m)
                  return (
                    <div
                      key={name}
                      id={`p-opt-${first + i}`}
                      role="option"
                      aria-selected={isSel}
                      className={`p-row ${isSel ? 'sel' : ''}`}
                      style={{ top: (first + i) * rowH, height: rowH }}
                      onClick={() => {
                        selectRow(name)
                        scrollRef.current?.focus()
                      }}
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
                      <span className="p-value" title={v ? valueText : undefined}>
                        {valueText}
                      </span>
                      <span className="p-default" title={defaultText || undefined}>
                        {defaultText}
                      </span>
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

        {rightCollapsed ? (
          <PaneRail side="right" title="Show editor" onOpen={() => setRightCollapsed(false)} />
        ) : (
          <>
            <PaneSplitter
              side="right"
              getWidth={() => rightWRef.current}
              onResize={resizeRight}
              onReset={() => resizeRight(DEFAULT_LAYOUT.rightW)}
            />
            <aside className="p-detail">
              <div className="p-detail-head">
                <button
                  className="p-pane-collapse"
                  title="Hide editor"
                  aria-label="Hide editor"
                  onClick={() => setRightCollapsed(true)}
                >
                  <ChevronsRight size={14} />
                </button>
              </div>
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
                  ⟲ Changing this parameter requires an FC restart to
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
          </>
        )}
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
