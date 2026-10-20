# ADR-007: License — Apache-2.0

- Status: Accepted
- Date: 2026-10-03
- Accepted: 2026-10-07 — Apache-2.0.

## Options

- **Apache-2.0**: permissive, patent grant, easy for closed-source derivatives
  and commercial field teams; compatible with Tauri/React ecosystem norms.
- **GPLv3**: copyleft, stronger guarantee that modifications stay open;
  heavier for commercial adopters.

## Tension

- The project is open-source but targets field operators, many of whom are
  commercial. Copyleft may deter integration with proprietary payload
  software (magnetometry/geophysics post-processing).

## Decision

- **Apache-2.0.** Rationale: the distribution goal (wide adoption side-by-side
  with QGC, which is GPLv3) and the permissive attitude of the Rust/Tauri
  stack. Apache-2.0 keeps the door open for commercial field teams and
  proprietary payload software.
- `docs/` and test data are covered by the same license unless a file states
  otherwise.

## Consequences

- The repository `LICENSE` holds the Apache-2.0 text; every crate inherits
  `license.workspace = true` (`Apache-2.0`).
- CI checks dependency licenses with `cargo deny` (`deny.toml`), rejecting
  copyleft/unknown licenses that would constrain distribution.
