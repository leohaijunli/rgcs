# ADR-022: Drape trajectory design, waypoint simplification and monitoring matching

- Status: Accepted (2026-10-18; data model per plan §5B **DR1**; the algorithm
  decision below is executed by **DR2**)
- Date: 2026-10-18
- Relates to: plan `docs/plans/lifecycle-survey-hrdem-plan.md` §5B
  (**DR1–DR6**); ADR-004 (terrain sources), ADR-005 (terrain following — the
  per-waypoint interim this supersedes), ADR-006 (height datums), ADR-018
  (HRDEM), ADR-019 (project model)
- Supersedes: the interim per-waypoint drape (`core::terrain::drape` +
  `mission/meshDrape.ts`) as the *design-grade* algorithm. **That path was
  removed on 2026-10-20 (A9)** — the solver pipeline is the only
  implementation (see Consequences).

## Decision

1. **Priorities when constraints conflict** (§5B principle 2):
   `DSM clearance > climb/descent grade limits > AGL target`. Conflicts are
   **reported** (`infeasible` spans + `max_deficit`), never silently truncated
   or smoothed away.
2. **One shared result.** `DrapeResult` (hash of the `DrapeSpec` + DEM ref)
   is the single baseline for the upload read-back (`itemsHash`), the
   preflight Drape group (DR4), the monitor (DR5) and the archive (DR7).
3. **Required parameter, no default.** `min_clearance_dsm_m` must be entered
   by the operator; an unset clearance refuses generation with an
   explanation (`DrapeSpecError::MissingClearance`). Thresholds otherwise come
   from configuration, never built-in guesses.
4. **`vertical_offset` is explicit and provenance-tracked**
   (`origin: operator | dem_bias_estimate`, plus a note). It shifts the planned
   height, **not** the safety clearance floor, and never landing targets.
5. **Heights are never bare `f64` at datum boundaries** (AGENTS.md): the spec
   works in the DEM datum; the AMSL conversion stays in `core::height`.
6. **Algorithm (implemented in DR2, `core::terrain::drape_solver`)**: per
   line, sample DTM/DSM at `sample_step`; target `z_t = DTM + target_agl +
   offset`, floor `z_min = DSM_fp + min_clearance`; initialise
   `z = max(z_t, z_min)` — the floor holds from the start — then propagate
   the grade window to a fixed point: a forward pass raises samples that
   dropped too fast from their predecessor, a reverse pass raises samples
   sitting too low before a high one (the **look-ahead**: the climb starts
   before the upslope). Together the two passes bound every edge to
   `−grade_down ≤ dz/ds ≤ grade_up`; optional smoothing runs between passes
   and the constraints re-apply after it. The floor and the grade limits are
   therefore **hard** — what gets reported is the consequence: every span
   where the planned AGL leaves the acceptance band (`agl_band`), classified
   by its binder (terrain out-climbing the limits → `ClimbRate` /
   `DescentRate`; the floor itself above the band → `Clearance`), with the
   worst excess over the band as the deficit. Implemented per the plan's
   recommended ① (explainable, O(n) per pass, table-driven synthetic-terrain
   tests) — ② constrained QP was not needed.
7. **Waypoint compilation (implemented in DR3,
   `core::terrain::drape_compile`)**: Douglas–Peucker variant on the (s, z)
   curve whose split metric is the **vertical** deviation from the chord (the
   quantity DR5 reports as compilation error — a perpendicular distance would
   understate it), tolerance `wp_tol_m`. The floor is re-checked at every
   original sample after simplification; violated samples are re-inserted and
   the check repeats, so simplification can reduce the item count but never
   the clearance. Over the mission-item limit the compile **refuses** with a
   `wp_tol` suggestion that was verified to fit — never a silent relaxation.
   One `DO_CHANGE_SPEED` per line (param semantics *verify*, plan issue 13);
   every compiled waypoint carries `s` / line id / kind for DR5 matching and
   DR7 aggregation; the worst vertical deviation is stored in
   `DrapeStats.compile_deviation_max_m`. Compiled heights carry the DEM's
   datum (`DemRef.datum`, captured from the sampled heights).
8. **Monitoring matching (DR5)**: monotone projection `s*` onto the compiled
   polyline with hysteresis; `e_agl` is measured against the **DTM** target,
   `ez` against the compiled polyline; the two are reported separately so
   compilation error never masquerades as flight error.

## Data model (DR1)

`core/src/terrain/drape_spec.rs` — `DrapeSpec` (target AGL, band, required DSM
clearance, position margin, DSM footprint, climb/descent rates, ground speed,
sample step, smoothing, run-in, turn policy, waypoint tolerance, vertical
offset), `DrapeResult` (per-line profiles, waypoints, infeasible spans,
crossings, stats, `spec_hash`, `DemRef`), all exposed to the frontend via
ts-rs. `spec_hash` = SHA-256 of the canonical JSON (version included).

## Consequences

- The interim per-waypoint drape had gained `vertical_offset_m` (applied to
  the AGL target only) and took the operator-entered clearance instead of the
  hardcoded 5 m — a bridge until DR2, not the final algorithm. **The bridge
  was removed on 2026-10-20 (A9, operator decisions: no mesh fallback, land
  on the DEM surface, delete the old engine)**: `core::terrain::drape` and
  the `terrain_drape` command are deleted, `mission/meshDrape.ts` is deleted,
  and `drape_preview` (solve → compile; compiled NAV heights converted to the
  stored `AMSL_EGM96` datum through the real H7 chain, fail-closed; landing
  items returned at the DEM surface) returns the route the plan stores via
  the pure frontend merge `mission/drapeApply.ts`. The monitor baseline keeps
  `result.compiled` in the DEM datum — the anchored fix's frame.
- U8 presets carry `min_clearance_dsm_m` and `vertical_offset_m` (additive,
  optional, backwards compatible — no config version bump).
- DR2/DR3/DR5 implement against these types without re-modelling.
