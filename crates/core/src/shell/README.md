# Owned primary MAVLink shell

Rust-derived shell DTOs bind the native window's Runtime client session, target,
operation and individual request ID. Native composition opens a 24 h maximum
Shell lease on the existing primary connection; it does not open another port.

Commands are one printable line, at most 1024 UTF-8 bytes. Reviewed read-only
commands allow safe arguments; compound/substitution characters require exact
`RUN`. The full line plus newline is sent across 70-byte SERIAL_CONTROL frames.
Write deadlines include queue/lock waiting. A separate priority-close channel
retires queued/ongoing writes; session request history keeps at most 4096 IDs
without re-admitting evicted identities.

Output is operation/target/sequence scoped. Native output is bounded/nonblocking;
the frontend retains 64 KiB characters and marks gaps. Closing/owner expiry
releases only the recorded generation, before primary transport shutdown.
Missing release receipts retain uncertainty and resource claims. Transport write
receipts and shell close do not prove arbitrary shell command execution/rollback.

Verification: `cargo test -p maggcs-core --lib shell`; production native PTY,
priority-close and lifecycle fixtures in `console_operation_tests.rs`; browser
acceptance via `npm --prefix frontend run check:shell-logs-acceptance`.
