import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLinkStore } from '../../stores/link'
import { useMissionStore, degFromMavInt, type AltitudeMode } from '../../stores/mission'
import type { MissionItem } from '../../generated-types/MissionItem'

const MODES: AltitudeMode[] = ['relative', 'amsl', 'agl']

export default function PlanningPanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const connected = Boolean(link?.fc_alive)
  const items = useMissionStore((s) => s.items)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const busy = useMissionStore((s) => s.busy)
  const syncState = useMissionStore((s) => s.syncState)
  const lastEvent = useMissionStore((s) => s.lastEvent)
  const select = useMissionStore((s) => s.select)
  const setAltitudeMode = useMissionStore((s) => s.setAltitudeMode)
  const addWaypoint = useMissionStore((s) => s.addWaypoint)
  const removeWaypoint = useMissionStore((s) => s.removeWaypoint)
  const moveWaypoint = useMissionStore((s) => s.moveWaypoint)
  const upload = useMissionStore((s) => s.upload)
  const download = useMissionStore((s) => s.download)
  const clear = useMissionStore((s) => s.clear)

  const selected = selectedSeq !== null ? items.find((it) => it.seq === selectedSeq) ?? null : null
  const [dragSeq, setDragSeq] = useState<number | null>(null)

  const onDrop = (targetSeq: number) => {
    if (dragSeq === null || dragSeq === targetSeq) return
    moveWaypoint(dragSeq, targetSeq)
    setDragSeq(null)
  }

  return (
    <div className="flex h-full flex-col gap-2">
      <div className="panel rounded p-2">
        <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">{t('plan.altitude')}</div>
        <div className="flex rounded-md border border-line bg-canvas p-0.5 text-xs">
          {MODES.map((m) => (
            <button
              key={m}
              onClick={() => setAltitudeMode(m)}
              className={`flex-1 rounded px-1 py-1 transition-colors ${
                altitudeMode === m
                  ? 'bg-accent text-canvas'
                  : 'text-muted hover:text-ink'
              }`}
            >
              {t(`plan.mode.${m}`)}
            </button>
          ))}
        </div>
      </div>

      <div className="min-h-0 flex-1">
        <div className="mb-1.5 flex items-center justify-between px-1">
          <span className="text-xs uppercase tracking-wide text-muted">
            {t('plan.items')} · {items.length}
          </span>
        </div>
        {items.length === 0 ? (
          <div className="panel rounded p-3 text-xs text-muted">{t('plan.empty')}</div>
        ) : (
          <ul className="space-y-1">
            {items.map((it, idx) => (
              <li key={it.seq}>
                <div
                  draggable
                  onDragStart={() => setDragSeq(idx)}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={() => onDrop(idx)}
                  onClick={() => select(it.seq)}
                  className={`flex cursor-pointer items-center gap-2 rounded border px-2 py-1.5 text-sm transition-colors ${
                    it.seq === selectedSeq
                      ? 'border-accent bg-accent/10'
                      : 'border-line bg-panel hover:bg-canvas'
                  }`}
                >
                  <span className="mono w-6 shrink-0 text-right text-muted">{it.seq}</span>
                  <span className="min-w-0 flex-1 truncate">
                    {degFromMavInt(it.y).toFixed(6)}, {degFromMavInt(it.x).toFixed(6)}
                  </span>
                  <span className="mono shrink-0 text-muted">{it.z.toFixed(1)} m</span>
                  {it.current ? (
                    <span className="shrink-0 rounded bg-ok/20 px-1 text-[10px] text-ok">
                      {t('plan.current')}
                    </span>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      {selected ? <ItemEditor item={selected} /> : null}

      <div className="grid grid-cols-2 gap-1.5">
        <button
          onClick={() => addWaypoint(degFromMavInt(selected?.x ?? 48.6493), degFromMavInt(selected?.y ?? -123.3982), 50)}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas"
        >
          {t('plan.add')}
        </button>
        <button
          disabled={selected == null}
          onClick={() => selected != null && removeWaypoint(selected.seq)}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.remove')}
        </button>
      </div>

      <div className="grid grid-cols-3 gap-1.5">
        <button
          disabled={!connected || busy || items.length === 0}
          onClick={() => void upload()}
          className="rounded-md bg-accent px-2 py-1.5 text-sm text-canvas disabled:opacity-40"
        >
          {t('plan.upload')}
        </button>
        <button
          disabled={!connected || busy}
          onClick={() => void download()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.download')}
        </button>
        <button
          disabled={!connected || busy}
          onClick={() => void clear()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.clear')}
        </button>
      </div>

      {(busy || lastEvent) && (
        <div className="text-xs text-muted">
          {busy ? (
            <span className="text-accent">{t(`plan.status.${syncState}`)}</span>
          ) : lastEvent && lastEvent.kind === 'failed' ? (
            <span className="text-error">
              {t('plan.failed')}: {lastEvent.message ?? '—'}
            </span>
          ) : lastEvent && lastEvent.kind === 'progress' ? (
            <span>
              {t(`plan.op.${lastEvent.op}`)} {lastEvent.sent}/{lastEvent.total}
            </span>
          ) : (
            lastEvent && <span className="text-ok">{t(`plan.done.${lastEvent.op}`)}</span>
          )}
        </div>
      )}
    </div>
  )
}

function ItemEditor({ item }: { item: MissionItem }) {
  const { t } = useTranslation()
  const updateItem = useMissionStore((s) => s.updateItem)
  const setCurrent = useMissionStore((s) => s.setCurrent)

  return (
    <div className="panel rounded p-2 text-sm">
      <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">
        {t('plan.properties')} · WP {item.seq}
      </div>
      <div className="space-y-1.5">
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Lat</span>
          <input
            type="number"
            step="0.0000001"
            value={degFromMavInt(item.x).toFixed(7)}
            onChange={(e) => updateItem(item.seq, { x: Math.round(parseFloat(e.target.value) * 1e7) })}
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
        </label>
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Lon</span>
          <input
            type="number"
            step="0.0000001"
            value={degFromMavInt(item.y).toFixed(7)}
            onChange={(e) => updateItem(item.seq, { y: Math.round(parseFloat(e.target.value) * 1e7) })}
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
        </label>
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Alt</span>
          <input
            type="number"
            step="0.5"
            value={item.z}
            onChange={(e) => updateItem(item.seq, { z: parseFloat(e.target.value) })}
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
        </label>
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('plan.command')}</span>
          <span className="mono">{item.command === 16 ? 'NAV_WAYPOINT' : item.command}</span>
        </div>
        {item.current ? (
          <button
            onClick={() => void setCurrent(item.seq)}
            className="w-full rounded border border-line bg-panel px-2 py-1 text-xs hover:bg-canvas"
          >
            {t('plan.goTo')}
          </button>
        ) : null}
      </div>
    </div>
  )
}