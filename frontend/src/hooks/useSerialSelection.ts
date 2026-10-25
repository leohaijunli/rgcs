import { useEffect, useState } from 'react'
import { parseEndpoint } from '../desktop/endpoint'
import { startDeviceWatcher, useDevicesStore } from '../stores/devices'
import type { SerialPortRef } from '../generated-types/SerialPortRef'

/** Bind manual selection to the observed attachment, never just its OS name. */
export function useSerialSelection(endpoint: string) {
  const inventory = useDevicesStore((s) => s.inventory)
  const [selection, setSelection] = useState<{ endpoint: string; port: SerialPortRef | null; changed: boolean }>({ endpoint, port: null, changed: false })
  useEffect(() => {
    startDeviceWatcher()
    void useDevicesStore.getState().load()
  }, [])
  useEffect(() => {
    const draft = parseEndpoint(endpoint)
    const current = draft?.kind === 'serial' ? inventory?.ports.find((p) => p.port.device.port_name === draft.host)?.port : undefined
    setSelection((previous) => {
      if (previous.endpoint !== endpoint) return { endpoint, port: current ?? null, changed: false }
      if (previous.changed) return previous
      if (previous.port && previous.port.instance_id !== current?.instance_id) return { ...previous, changed: true }
      if (!previous.port && current) return { endpoint, port: current, changed: false }
      if (previous.port && current && JSON.stringify(previous.port.device) !== JSON.stringify(current.device)) return { ...previous, port: current }
      return previous
    })
  }, [endpoint, inventory])
  const reselect = () => {
    const draft = parseEndpoint(endpoint)
    const port = draft?.kind === 'serial' ? inventory?.ports.find((p) => p.port.device.port_name === draft.host)?.port : undefined
    setSelection({ endpoint, port: port ?? null, changed: false })
  }
  return { port: inventory ? selection.port : undefined, changed: selection.changed, reselect }
}
