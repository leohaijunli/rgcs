// U15: the detached-map flag, projected from the core shared state so the
// main window and (a future) second view agree on whether the map lives in
// its own webview. The window itself is opened/closed by the shell commands
// (`map_open` / `map_close`); the flag follows the `map_window` events.

import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isTauri } from '../inspector/mock'

interface DetachedState {
  detached: boolean
  /** Set when the map window was closed/crashed while detached (main window
   * shows a "reopen" affordance rather than a silent empty panel). */
  notice: string | null
  open: () => Promise<void>
  bringBack: () => Promise<void>
}

export function useDetachedMap(): DetachedState {
  const [detached, setDetached] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    void invoke<boolean>('map_status')
      .then((open) => !disposed && setDetached(open))
      .catch(() => {})
    const un = listen<string>('map_window', (e) => {
      setDetached(e.payload === 'opened')
      if (e.payload === 'closed') setNotice('map re-embedded')
    })
    return () => {
      disposed = true
      void un.then((f) => f())
    }
  }, [])

  return {
    detached,
    notice,
    open: async () => {
      if (!isTauri()) return
      try {
        await invoke('map_open')
        setDetached(true)
        setNotice(null)
      } catch (e) {
        setNotice(String(e))
      }
    },
    bringBack: async () => {
      if (!isTauri()) return
      try {
        await invoke('map_close')
      } catch {
        /* already gone */
      }
      setDetached(false)
    },
  }
}
