// DEM surface resolution for the drape button and the altitude profile.
//
// The operator should not have to pick a folder when a DEM is already
// available somewhere. `ensureTile` (stores/terrain.ts) lists the possible
// sources in priority order and hands them to `firstSurface`, which returns
// the first tile path any of them can supply. This is kept pure so the
// ordering and fall-through contract are testable (scripts/check-surface.mjs).

/** A candidate DEM source: yields a tile path, or `null` when it has none to
 * offer (nothing remembered, no prepared session, no DEM beside the app). */
export type SurfaceSource = () => Promise<string | null>

/** Resolve the DEM tile path from the sources, in order. The first source to
 * yield a path wins and no later source runs. A source that returns `null` or
 * throws (no session yet, `survey_prepare_status` unreachable, a remembered
 * folder that is gone) falls through to the next. Never throws: `null` means
 * no source could supply a surface. */
export async function firstSurface(...sources: SurfaceSource[]): Promise<string | null> {
  for (const source of sources) {
    try {
      const path = await source()
      if (path) return path
    } catch {
      // A dead source is not fatal — try the next.
    }
  }
  return null
}
