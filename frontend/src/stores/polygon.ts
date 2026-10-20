// Draft survey-boundary polygon for the polygon map tool (WS-G G2).
//
// Vertices are clicked on the map while the tool is open; closing the polygon
// (double-click or clicking the first vertex) fixes the boundary that the sweep
// is generated against. The draft persists across tool switches in the planning
// view so the operator can come back and keep editing; `reset` (Esc) drops it.

import { create } from 'zustand'
import type { GeoPoint } from '../generated-types/GeoPoint'

interface PolygonState {
  /** Boundary vertices in click order (open while drawing, closed when done). */
  vertices: GeoPoint[]
  /** True once the boundary is closed and ready to generate from. */
  closed: boolean
  /** Append a vertex (ignored while closed). */
  addVertex: (v: GeoPoint) => void
  /** Move one vertex (drag) by index. */
  moveVertex: (seq: number, v: GeoPoint) => void
  /** Remove the vertex at `seq`. */
  removeVertex: (seq: number) => void
  /** Close the boundary (requires at least 3 vertices). */
  close: () => void
  /** Drop the draft and go back to drawing. */
  reset: () => void
}

export const usePolygonStore = create<PolygonState>((set) => ({
  vertices: [],
  closed: false,
  addVertex: (v) =>
    set((s) => (s.closed ? s : { vertices: [...s.vertices, v] })),
  moveVertex: (seq, v) =>
    set((s) => ({
      vertices: s.vertices.map((p, i) => (i === seq ? v : p)),
    })),
  removeVertex: (seq) =>
    set((s) => ({
      vertices: s.vertices.filter((_p, i) => i !== seq),
      closed: s.vertices.length - 1 < 3 ? false : s.closed,
    })),
  close: () =>
    set((s) => (s.vertices.length >= 3 ? { closed: true } : s)),
  reset: () => set({ vertices: [], closed: false }),
}))