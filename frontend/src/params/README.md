# Parameters window

The table is metadata-first: the bundled board catalog renders without a link,
and selecting a row lazily reads its current FC value. **Pull all** is an
explicit snapshot action; opening/reconnecting the window does not start a
full parameter download.

## Connection and values

- `params_fc_version` returns `string | null` (Rust `Option<String>`), not an
  object. The page subscribes to link events before reading the initial link
  snapshot, and retries an unanswered version query while the FC is alive.
- Connection availability follows the live FC heartbeat, independently of
  firmware-version availability. A live FC awaiting its version is labelled
  **FC connected**; a previously live FC that drops is **FC disconnected**.
- Every link transition invalidates cached UI values and queued reads. The
  retained selection is read again after recovery, including on the same
  serial endpoint. Delayed replies from an older connection epoch are ignored.
- Row reads are serialized because the parameter protocol supports one
  transaction at a time. The Rust service survives transport reconnects and
  invalidates its cache, firmware identity, partial batches and pending reads.
  The event forwarder continues after a lagged receiver.
- Integer/float replies use `{int: number}` / `{real: number}` on the JSON wire.
  They are decoded for the table, detail panel and numeric/enum editor. External
  changes update the displayed value. All supported MAVLink integer types fit
  exactly in JavaScript numbers.
- Search/filter changes reset the virtualized table's scroll position. Unknown
  FC-reported parameters are merged into the list; an exact 1–16-character
  standard parameter name can also be read directly from the empty search
  result. Such rows use the FC's reported type without invented descriptions.

## Catalog

The bundle contains **1912 parameters** from official PX4 v1.17.0
`px4_fmu-v6x_default`, including the hardware-specific `USB_MAV_MODE`.
`public/param-meta/manifest.json` records its provenance and hashes and drives
the version mismatch notice. See the [generator](../../../tools/param-meta/README.md).

## Automated verification

From the repository root:

```bash
cargo test -p maggcs-core --test params_service_integration
cargo test -p maggcs-app params_service --lib
```

From `frontend/`, with Vite serving port 1420 and Playwright installed:

```bash
npm run typecheck
npm run check:params-connection -- http://127.0.0.1:1420/params.html screenshots/params/connection
npm run check:params -- http://127.0.0.1:1420/params.html /tmp/opencode/params-layout
```

`PLAYWRIGHT_MODULE=/path/to/playwright` can select an external Playwright
installation. The connection check exercises the real React page with an
isolated IPC stub: scalar version replies, tagged integer/float values,
external updates, search after deep scrolling, repeated same-endpoint
reconnects, stale read/version replies, unknown parameters, and opening the
window before the FC connects. It also checks catalog count/uniqueness/hash.
The layout check verifies column alignment within 1 CSS px at 1366, 1920 and
900 px widths, pane controls, virtual scrolling and Field-mode row height.

### Screenshots (isolated IPC)

- [USB_MAV_MODE displayed](../../screenshots/params/connection/params-live-usb.png)
- [Disconnected](../../screenshots/params/connection/params-disconnected.png)
- [Recovered on the same endpoint](../../screenshots/params/connection/params-reconnected.png)
- [Window opened before connection](../../screenshots/params/connection/params-late-connect.png)

## Read-only hardware acceptance

Close any app using the serial port, then run from the repository root:

```bash
MAGGCS_BENCH=1 MAGGCS_BENCH_UART=/dev/ttyACM0 \
  cargo test -p maggcs-core --test params_service_integration \
  serial_parameter_reads_survive_reopening -- --ignored --nocapture
```

Verified on 2026-10-23: PX4 **1.17.0**, `USB_MAV_MODE = 0` (`I32`). Two explicit
serial close/reopen cycles retained the same parameter service and both
successfully read the version and value again (approximately 2 seconds each).
The test sends version/parameter reads and does not change parameter values.

### Native desktop review

1. Start the rebuilt `target/release/maggcs-app`, connect the primary FC and
   open Tools → Parameters. Confirm the version badge.
2. Scroll down, then search `usb_mav_mode`. Select the result; confirm the
   current value and enum meaning, separately from its metadata default.
3. Keep the window open while unplugging/replugging USB. Confirm the
   disconnected state and automatic version/selected-value recovery after
   the heartbeat returns. Also check an explicit Disconnect → Connect.
4. Open Parameters before connecting the FC, select a row, then connect.
   Confirm recovery without closing or reloading the parameter window.

The automated serial test reopens the transport; physically unplugging USB in
the native desktop window is the manual review step above.
