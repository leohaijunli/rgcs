# ADR-015: Signal Inspector DSP lives in `core::dsp`

- Status: Accepted
- Date: 2026-10-08
- Supersedes: none. Related: ADR-001 (domain logic in `core`), ADR-002
  (ts-rs bindings), ADR-016 (inspector window + Channel).

## Decision

- The Signal Inspector's filtering and spectrum analysis are implemented in
  `crates/core/src/dsp` (Rust), not in a frontend Web Worker. The frontend only
  renders.
- Algorithms are self-describing: a `registry` of `AlgorithmDescriptor`s exposes
  `AlgorithmInfo { id, name, kind, params }` to the frontend over IPC, so a new
  algorithm is one Rust file + one registry entry and the UI parameter form is
  generated from `ParamSpec`s.
- A `Processor` is a single-sample stream filter (biquad LP/HP, moving average,
  detrend); an `Analyzer` is a windowed time-to-frequency transform (the FFT).
  A `Pipeline` chains processors. Both traits are `Send` so a per-trace instance
  can live on a tap task.
- The signal source is abstracted behind `SampleSource` (plan decision B) so a
  ULog replay can feed the inspector later.

## Rationale

- Domain logic in `core` is reusable by the headless server and Phase 4 QC
  (ADR-001), and testable with `cargo test` against numeric acceptance
  (SciPy golden vectors; the biquad meets −3.01 dB at fc, ~−40 dB/decade
  rolloff; the FFT locates a sine's peak within 1 %).
- Raw and filtered traces come from the same sample batch, so their time axes
  are aligned by construction.
- Self-description keeps the frontend free of per-algorithm knowledge.

## Consequences

- Changing a parameter is one IPC round-trip (milliseconds), acceptable for a
  plotting tool; `configure` hot-updates in place where possible.
- The first algorithms are `lpf2`, `hpf2`, `moving_average`, `detrend`, `fft`;
  the FFT is a dependency-free radix-2 implementation (no `realfft` dependency
  was added).
- Numeric acceptance numbers are proposed values; they are pinned by the tests
  in `core::dsp` and can be regenerated with `tools/gen_dsp_golden.py`.