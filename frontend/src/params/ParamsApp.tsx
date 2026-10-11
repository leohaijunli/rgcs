// Parameters window — PM3 skeleton (parameters plan §4.3/§5.1): the window
// shell with pull/progress and the event stream; the full virtualized table
// (PM5) replaces the placeholder list.

import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isTauri } from '../inspector/mock'
import type { ParamEntry } from '../generated-types/ParamEntry'

type ParamsEvent =
  | { kind: 'pull_started'; total: number }
  | { kind: 'batch'; entries: ParamEntry[] }
  | { kind: 'progress'; received: number; total: number }
  | { kind: 'pull_done'; received: number; total: number }
  | { kind: 'external_change'; name: string }

export function ParamsApp() {
  const tauri = isTauri()
  const [entries, setEntries] = useState<ParamEntry[]>([])
  const [progress, setProgress] = useState<{ received: number; total: number } | null>(null)
  const [pulling, setPulling] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<ParamsEvent>('params_event', (e) => {
          if (disposed) return
          const ev = e.payload
          if (ev.kind === 'pull_started') {
            setEntries([])
            setProgress({ received: 0, total: ev.total })
            setPulling(true)
          } else if (ev.kind === 'batch') {
            setEntries((prev) => [...prev, ...ev.entries])
          } else if (ev.kind === 'progress') {
            setProgress({ received: ev.received, total: ev.total })
          } else if (ev.kind === 'pull_done') {
            setProgress({ received: ev.received, total: ev.total })
            setPulling(false)
          } else if (ev.kind === 'external_change') {
            setNotice(`External change: ${ev.name}`)
          }
        })
      } catch {
        /* events unavailable: poll commands still work */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

  const pull = async () => {
    if (!tauri) return
    setNotice(null)
    try {
      setPulling(true)
      await invoke('params_pull')
    } catch (e) {
      setPulling(false)
      setNotice(String(e))
    }
  }

  const cancel = async () => {
    if (!tauri) return
    try {
      await invoke('params_cancel')
    } catch {
      /* already idle */
    }
  }

  return (
    <div className="params-grid">
      <header className="params-toolbar">
        <strong style={{ fontSize: 13 }}>Parameters</strong>
        <button className="params-btn" onClick={() => void pull()} disabled={pulling}>
          {pulling ? `Pulling ${progress ? `${progress.received}/${progress.total}` : '…'}` : 'Pull all'}
        </button>
        {pulling && (
          <button className="params-btn" onClick={() => void cancel()}>
            Cancel
          </button>
        )}
        {notice && <span className="params-status">{notice}</span>}
        <span className="params-status" style={{ marginLeft: 'auto' }}>
          {entries.length} cached · {tauri ? 'linked' : 'mock (no link)'}
        </span>
      </header>
      <main style={{ overflowY: 'auto', padding: 10 }}>
        {entries.length === 0 ? (
          <div className="params-status">
            {pulling ? 'Receiving parameters…' : 'No parameters yet — press Pull all.'}
          </div>
        ) : (
          <table style={{ width: '100%', fontSize: 12, borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ textAlign: 'left', color: 'var(--mg-muted)' }}>
                <th>#</th>
                <th>Name</th>
                <th>Type</th>
                <th>Value</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.index} style={{ borderBottom: '1px solid var(--mg-border)' }}>
                  <td style={{ color: 'var(--mg-muted)' }}>{e.index}</td>
                  <td className="mono">{e.name}</td>
                  <td>{e.ptype}</td>
                  <td className="mono">{JSON.stringify(e.value)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </main>
    </div>
  )
}