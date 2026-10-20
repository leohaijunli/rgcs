# ADR-021: Flight recorder — raw-frame tee, QGC tlog format, crash-safe writes

- Status: Accepted (2026-10-15)
- Date: 2026-10-15
- Relates to: plan `docs/plans/lifecycle-survey-hrdem-plan.md` §5B WP **A4**
  (and §5B WP **W2**, which extends this recorder); ADR-011 (link-state model),
  ADR-020 (walk survey — walk recording extends this same recorder)
- Resolves: plan §9 open item 6 (tlog format vs QGC)

## Decision

1. **Capture strategy — recording-scoped raw-frame tee (option B).** The
   `mavlink::connection` worker reads with `recv_raw()` and parses the message
   from the raw frame, so it holds the **exact wire bytes**. It publishes a
   `RawFrame { header, bytes, received_at, utc_us }` on a `broadcast` channel
   **only when `receiver_count() > 0`** — a recorder subscribes while a session
   is open, so an idle GCS pays **zero** extra cost. We do **not** re-serialize
   decoded messages: the tlog is byte-exact by construction and can carry frames
   the current dialect cannot represent (e.g. PX4 v1.17 out-of-range enums),
   which are skipped only for *decoding*, never for *recording*.
2. **tlog format (QGC-aligned).** Each record is a **8-byte little-endian
   `u64` timestamp in microseconds since the Unix epoch**, immediately followed
   by the **raw MAVLink packet including its start sign** (`0xFE`/`0xFD`). This
   matches QGC's documented `.tlog` layout: QGC writes the timestamp via
   `memcpy` of a `uint64_t` (host byte order — little-endian on every target
   platform) followed by the packet. The reader re-frames by scanning the
   self-delimiting MAVLink packets rather than trusting a fixed stride, so QGC
   files and MagGCS files read identically.
3. **Non-blocking, crash-safe writes.** A dedicated writer thread consumes a
   **bounded** queue (`Mutex<VecDeque>` + `Condvar`). The producer enqueues and
   returns immediately; when the queue is full it **drops the oldest record and
   increments a counter** (surfaced as an alert), so a slow or stalled disk can
   never add latency to the MAVLink hub. A `BufWriter` is flushed every
   `500 ms`, and `flush + fsync` runs on stop. `session.json` is written to a
   temporary file and renamed (atomic). A crash therefore loses at most the data
   since the last flush, and the file stays parseable up to that point
   (a truncated trailing record is ignored by the reader).
4. **Session layout.** Under the app data directory:
   `recordings/session-<started_utc_us>/` containing
   `raw/session.tlog` (raw frames), `events.jsonl` (application events), and
   `session.json` (the manifest: version, start/close time, endpoint, target
   system/component, app version, frame/event/dropped counts, relative paths).
5. **Events.** `events.jsonl` is one JSON object per line: `{ "ts_us", "event",
   ... }` where `event` is a tagged `RecordEvent` (session opened/closed, alert,
   command, mode change, mission sync, force-arm, line started/ended, marker,
   quality, note). Event **sources** (command service, mode changes, mission
   sync, survey line markers) are wired by their owning WPs (M1/W4/L5); until
   then they can be logged via the `recorder_log_event` command.
6. **Forced-arm tlog marker (L5).** A forced-arm authorization writes three
   correlated records with the same UTC timestamp: the `session.json`
   `forced_arm` object, a `force_arm_audit` JSONL event, and one synthetic
   MAVLink 2 `STATUSTEXT` (severity `NOTICE`, text
   `MAGGCS FORCE ARM: <reason> (<n> items)`) in the tlog. The marker carries
   the **GCS** system/component ids, never the FC's, so a replay shows it as a
   GCS annotation rather than something the flight controller sent. The event
   and manifest writes are durable **before** the arm command can be sent; the
   marker is enqueued after them, so losing it under queue pressure or a crash
   cannot lose the authorization record. A synthetic marker is the only
   exception to the raw inbound bytes rule in decision 1; all captured FC
   frames remain byte-exact.
7. **Lifecycle.** The recorder is a `ManagedService` (`Stage::Recorder`),
   stopped **before the link** so the final frames are captured. On link swap or
   disconnect the session is flushed in the background; a start failure is
   reported as a single Warn alert and is **non-fatal** to the connection.

## Context

The plan requires each flight/walk to have a lossless tlog plus an event log and
a manifest, with no impact on the hub under disk stalls. Two capture strategies
were considered:

- **A — decode-and-re-serialize** into the tlog. Simple, but the round-trip is
  not guaranteed byte-identical (extension fields, trailing-zero elision,
  signature handling) and cannot represent undecodable frames.
- **B — raw-frame tee** (chosen). The connection already owns the raw frame;
  duplicating it under a refcount gate is cheap and exactly what QGC logs.

## Consequences

- `mavlink::connection` gains a raw tap (`ConnectionHandle::subscribe_raw`)
  and switches its receive path to `recv_raw()` + parse. Decode behavior for the
  rest of the app is unchanged (undecodable-but-valid frames are skipped once).
- The recorder's tlog is the **source of truth**; the walk CSV (W2) is a
  derived view, not the primary record (also because PX4 SD/ULog is the walk
  full-rate record — see ADR-020).
- **Deferred:** a hard disk-size cap/rotation is **not** in this WP and moves to
  **M4** (archive); for now a stalled/full disk is handled by drop-oldest +
  alert, and sessions are append-only.
- Timestamp for now is the host system clock in microseconds; mapping to
  GNSS/PX4 time (`SYSTEM_TIME`) is the M8 concern.

## Verification

- Core `recorder` unit tests (7): tlog byte-exact round-trip and truncated-tail
  tolerance; all `RecordEvent` variants round-trip and truncated-final-line
  tolerance; the queue drops oldest and its push never blocks on a full queue;
  write path round-trip with manifest counters; forced-arm audit writes the
  manifest, JSONL event and a decodable GCS `STATUSTEXT` marker with the same
  timestamp, and a failed audit blocks arming without leaving a manifest entry.
- Connection integration test: a UDP loopback send asserts the raw tap delivers
  the **exact** bytes (and the header sequence).
- `cargo test -p maggcs-core --lib` (526 passed), `cargo test -p maggcs-app`
  (47 passed), `cargo clippy`/`cargo fmt` clean,
  `cargo check --workspace --all-targets`, `npm run typecheck` clean.
