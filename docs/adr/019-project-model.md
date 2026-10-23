# ADR-019: Project / survey / session model and storage

- Status: Accepted (2026-10-18)
- Date: 2026-10-18
- Relates to: plan `docs/plans/lifecycle-survey-hrdem-plan.md` §5A WP **PJ1**
  (and **PJ2**, **PJ4**); ADR-018 (HRDEM), ADR-002 (frontend types), ADR-004
  (terrain sources), ADR-006 (height datums)
- Supersedes: the assumption that all operator settings live in the single
  application `config.json` (A6). Application-level settings stay there;
  **project-level** settings move under the project.

## Decision

1. **Local-first project directory.** A project is a directory on disk with no
   cloud dependency. Its layout:

   ```
   <project>/
     project.json                     # schema_version, name, CRS, datum, areas, settings
     resources/
       dem/                           # DEM fetches, keyed by AOI hash
       calibration/                   # mag/other calibration files
       presets/                       # planning presets
     surveys/<survey-id>/
       survey.json                    # kind, area, route, config, status
       sessions/<ts>/
         session.json                 # status, operator, start/end
         ...                          # raw/, events.jsonl, manifest.json, qc.json (M3/M4)
     .lock                            # exclusive open guard (see 4)
   ```

2. **Three levels.** `Project` → `Survey` → `Session`. One project holds many
   surveys (e.g. repeat measurements of the same area) that share project-level
   resources (DEM cache, calibration, presets). Each survey runs independently;
   each flight is one session.

3. **Per-file schema version + forward migration.** Every file carries a
   `version: u32`. A file with a version **newer** than the build supports is an
   error (never silently downgraded). Older versions migrate in place on load.
   `#[serde(default)]` fills missing fields from `Default`, so a partial file
   still loads and one bad field does not lose the rest.

4. **Atomic writes and single-open lock.** Every write goes to a sibling
   `<file>.tmp`, is flushed, then renamed over the target, so a crash mid-write
   cannot leave a truncated file; a leftover/stale temp is ignored on load. The
   project directory is locked against a second process with an **advisory file
   lock** (`fs2` on `.lock`) so two GCS instances cannot corrupt one project;
   the OS releases the lock on process death (no stale-lock cleanup needed).

5. **Corruption policy.** An unreadable/permission error on a file surfaces as
   `ProjectError::Io`; a parse failure surfaces as `ProjectError::Corrupt`. The
   caller keeps the last good in-memory state and raises a **Warn** — a corrupt
   file must never stop the app from starting (same policy as A6).

6. **Settings split.** `config.json` (A6) keeps **application-level** settings
   only: MAVLink endpoint, auto-connect, initial position, model orientation,
   alert volume, safety thresholds. **Project-level** settings (DEM directory,
   calibration path, planning presets) move into `Project::settings`. When a
   project is created, these are seeded from the current app config so existing
   installs carry over; the app config drops them.

7. **IDs are slugs.** A survey/session id is a filesystem-safe slug
   (`A-Za-z0-9._-`, non-empty) and is its directory name. Anything else is
   rejected before touching the disk.

## Rationale

- **Why a directory, not a single archive:** DEM fetches are large (tens of MB–
  GB) and must be cached per AOI; keeping them as files beside the survey is the
  H1/H9 cache. An archive would force repacking for every flight.
- **Why three levels:** the plan keeps repeat measurements (surveys) and their
  individual flights (sessions) distinct; collapsing them would lose per-flight
  raw logs and QC.
- **Why `fs2` (advisory lock):** it is released by the OS when the holding
  process exits, so a crash cannot leave a stale lock — a defect of pid-file
  schemes that would otherwise need cross-platform liveness probes.
- **Why per-file versions:** `project.json`, `survey.json` and `session.json`
  evolve independently; one global version would force needless migrations.

## Consequences

- `core::project` is the single writer of the project tree; features (H9
  survey-prep, M3/M4 manifest/QC, PJ3 history) read and write through
  `ProjectStore`, never by ad-hoc path joins.
- The 3D/2D DEM auto-resolution gains a first-class source: the **open
  project's** `resources/dem` (ahead of the remembered folder / `$MAGCS_DEM_DIR`
  fallbacks).
- PJ2 (`SurveyKind` + profile registry) replaces the `Survey::kind` string with
  the typed enum; PJ4 (project archive/import) zips this same layout.
- The A6 `config.json` loses `plan.presets`; a migration seeds them into the
  project on first open (or keeps them until a project exists).

## Addendum (2026-10-18): PJ1 scope

PJ1 implements the model, the store, atomic writes, the lock, migration and the
A6 settings split. It does **not** implement manifest/QC/archive (M3/M4/PJ4) or
the profile registry (PJ2); those consume the model when they unfreeze.

Implementation notes:

- **Timestamps are epoch microseconds** (`u64`), the same convention as
  `core::recorder::now_utc_us` — one time format across the repo (no RFC3339
  formatter, no new dependency).
- **A6 transition**: `config.json` keeps `plan.presets` for now (the presets UI
  still binds to it); `project_create` seeds the project's `ProjectSettings`
  from it. Removing the app-config copy is a frontend WP, so the presets UI
  never breaks in between.
- **Lock**: `fs2` advisory exclusive lock on `<root>/.lock`, held by the store
  for its whole lifetime; `AppState.project` keeps the single open store.

## Addendum (2026-10-22): creation location, optional project, and deletion

1. **The project is optional at runtime.** StartPage offers *Continue without a
   project*; the workspace runs with no project open, and the TopBar's
   `No project` pill reopens the project picker. Closing a project returns to
   StartPage. Nothing outside `core::project` requires an open project.

2. **Create form asks for a name only.** `ProjectWizard` is shared by StartPage
   and `ProjectHome`. CRS and vertical datum use the defaults `EPSG:4326` /
   `cgvd2013`; the operator never edits them at creation. By default the project
   is created at `<app data>/projects/<slug-of-name>` (a free `-2`, `-3`, …
   suffix on collision), with an explicit *Change location…* for any other
   directory. `project_create` keeps its `(path, name, crs, vertical_datum)`
   signature; the frontend supplies the constants and resolves the path.

3. **Deletion is explicit and validated.** `ProjectStore::delete(root)` validates
   that `<root>/project.json` parses as a `Project` and refuses a path without a
   final component (a filesystem root); a missing/invalid manifest is
   `ProjectError::NotAProject`. `project_delete` is refused while a project is
   open, and the frontend additionally refuses while the vehicle is armed (A1).
   Recents offer two distinct actions: *Remove from list* (localStorage only,
   never touches disk) and *Delete from disk* (typed project-name confirmation).
   Removal is `remove_dir_all` and irreversible — there is no trash integration.
