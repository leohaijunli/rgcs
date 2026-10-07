// Structured MAVLink endpoint model for the settings form (DEVELOPMENT_PLAN
// Phase 0 addendum, task 0.8). Pure functions — no React, no Tauri.
//
// The wire format is the address string the core `Endpoint` type round-trips:
//   udpin:<addr>:<port>   udpout:<addr>:<port>   tcpin/udpout/tcpout:...
//   serial:<port>:<baudrate>

export type EndpointKind = 'udpin' | 'udpout' | 'tcpin' | 'tcpout' | 'serial'

export interface EndpointDraft {
  kind: EndpointKind
  /** Host address for UDP/TCP, device path for serial. */
  host: string
  /** Port for UDP/TCP; the serial baud rate is held in `baud`. */
  port: string
  baud: string
}

export const DEFAULT_ENDPOINT = 'udpin:0.0.0.0:14550'

export const DEFAULT_DRAFT: EndpointDraft = {
  kind: 'udpin',
  host: '0.0.0.0',
  port: '14550',
  baud: '115200',
}

export interface EndpointPreset {
  id: string
  kind: EndpointKind
  host: string
  port: string
  baud: string
}

/** Common field setups; labels come from `settings.endpoint.preset.<id>`. */
export const ENDPOINT_PRESETS: EndpointPreset[] = [
  { id: 'sitl', kind: 'udpin', host: '0.0.0.0', port: '14550', baud: '115200' },
  { id: 'qgcForward', kind: 'udpout', host: '127.0.0.1', port: '14551', baud: '115200' },
  { id: 'serialTelemetry', kind: 'serial', host: '/dev/ttyACM0', port: '14550', baud: '115200' },
]

export const BAUD_RATES = ['57600', '115200', '230400', '460800', '921600']

export function formatEndpoint(d: EndpointDraft): string {
  if (d.kind === 'serial') return `serial:${d.host.trim()}:${d.baud.trim()}`
  return `${d.kind}:${d.host.trim()}:${d.port.trim()}`
}

function parsePort(value: string): number | null {
  if (!/^\d+$/.test(value.trim())) return null
  const n = Number(value)
  return n >= 1 && n <= 65535 ? n : null
}

export function draftToPreset(p: EndpointPreset): EndpointDraft {
  return { kind: p.kind, host: p.host, port: p.port, baud: p.baud }
}

/** Parse an address string back into a draft, or `null` if unrecognised. */
export function parseEndpoint(value: string): EndpointDraft | null {
  const [kind, ...rest] = value.split(':')
  if (kind === 'serial') {
    const port = rest.join(':')
    const cut = port.lastIndexOf(':')
    if (cut <= 0) return null
    return { kind: 'serial', host: port.slice(0, cut), port: '14550', baud: port.slice(cut + 1) }
  }
  if (kind === 'udpin' || kind === 'udpout' || kind === 'tcpin' || kind === 'tcpout') {
    const addr = rest.join(':')
    const cut = addr.lastIndexOf(':')
    if (cut <= 0) return null
    return { kind, host: addr.slice(0, cut), port: addr.slice(cut + 1), baud: '115200' }
  }
  return null
}

export type EndpointFieldError = 'host' | 'port' | 'baud' | null

/** Which field (if any) is invalid, for inline form validation. */
export function endpointError(d: EndpointDraft): EndpointFieldError {
  if (d.host.trim() === '') return 'host'
  if (d.kind === 'serial') {
    const baud = Number(d.baud)
    if (!/^\d+$/.test(d.baud.trim()) || baud <= 0) return 'baud'
    return null
  }
  if (parsePort(d.port) === null) return 'port'
  if (!/^[A-Za-z0-9.\-:[\]_]+$/.test(d.host.trim())) return 'host'
  return null
}
