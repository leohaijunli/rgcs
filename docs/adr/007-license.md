# ADR-007: License — Apache-2.0 or GPLv3

- Status: Draft
- Date: 2026-10-03
- Decision required during Phase 0.

## Options

- **Apache-2.0**: permissive, patent grant, easy for closed-source derivatives
  and commercial field teams; compatible with Tauri/React ecosystem norms.
- **GPLv3**: copyleft, stronger guarantee that modifications stay open;
  heavier for commercial adopters.

## Tension

- The project is open-source but targets field operators, many of whom are
  commercial. Copyleft may deter integration with proprietary payload
  software (magnetometry/geophysics post-processing).

## Recommendation (pending confirmation)

- Apache-2.0. Rationale: distribution goal (wide adoption side-by-side with
  QGC, which is GPLv3) and permissive attitude of the Rust/Tauri stack.
- `docs/` and test data may carry their own permissive terms.

## Consequences

- Once chosen, the `LICENSE` file and every crate's `license` field must
  reflect the decision; CI must check it.