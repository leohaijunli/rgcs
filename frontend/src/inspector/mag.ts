/** Pure source-identity helpers for Inspector magnetometer triples. */

export interface MagSignalIdentity {
  system_id: number
  component_id: number
  message_id: number
  field: string
}

export interface MagTraceLike {
  signal: MagSignalIdentity
}

export interface MagSourceIdentity {
  system_id: number
  component_id: number
  message_id: number
}

export interface MagComponents {
  source: MagSourceIdentity
  sourceKey: string
  x: number
  y: number
  z: number
}

/** Full MAVLink source identity without the field name. */
export function magSourceKey(source: MagSourceIdentity): string {
  return `${source.system_id}:${source.component_id}:${source.message_id}`
}

/** Must match `core::inspector::session::mag_total_trace_id`. */
export function magTotalTraceId(source: MagSourceIdentity): string {
  return `mag_total_cal:${magSourceKey(source)}`
}

/** Look up derived rings in group order without a shared fallback id. */
export function calibratedMagRings<T>(
  groups: readonly MagComponents[],
  rings: ReadonlyMap<string, T>,
): Array<T | undefined> {
  return groups.map((group) => rings.get(magTotalTraceId(group.source)))
}

/** Every complete x/y/z group, never mixing axes from different sources. */
export function magComponentGroups(traces: readonly MagTraceLike[]): MagComponents[] {
  const groups = new Map<string, {
    source: MagSourceIdentity
    x?: number
    y?: number
    z?: number
  }>()
  traces.forEach((trace, index) => {
    const field = trace.signal.field
    if (field !== 'xmag' && field !== 'ymag' && field !== 'zmag') return
    const source = {
      system_id: trace.signal.system_id,
      component_id: trace.signal.component_id,
      message_id: trace.signal.message_id,
    }
    const key = magSourceKey(source)
    let group = groups.get(key)
    if (!group) {
      group = { source }
      groups.set(key, group)
    }
    const axis = field[0] as 'x' | 'y' | 'z'
    if (group[axis] === undefined) group[axis] = index
  })
  return [...groups.entries()].flatMap(([sourceKey, group]) =>
    group.x !== undefined && group.y !== undefined && group.z !== undefined
      ? [{ source: group.source, sourceKey, x: group.x, y: group.y, z: group.z }]
      : [],
  )
}

/** Compact legend label; node ids stay visible for multi-vehicle traces. */
export function magSourceLabel(source: MagSourceIdentity, displayName?: string): string {
  const message = displayName ?? ({
    26: 'SCALED_IMU',
    105: 'HIGHRES_IMU',
    116: 'SCALED_IMU2',
    129: 'SCALED_IMU3',
  } as Record<number, string>)[source.message_id] ?? `msg ${source.message_id}`
  return `${message} ${source.system_id}:${source.component_id}`
}
