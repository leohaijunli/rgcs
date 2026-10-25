# Parameters window

The table is metadata-first: the bundled board catalog renders without a link,
and selecting a row lazily reads its current FC value. **Pull all** is an
explicit snapshot action; opening/reconnecting the window does not start a
full parameter download.

## Value presentation

- **Current value** and **Default** have separate table columns. Their headers
  align with the same right edge as the values, so a current value does not
  visually sit under the next column's header.
- The table keeps at least 474 px of content width (column minimums, gaps and
  padding), plus the live scrollbar gutter when resizing the side panes.
- Selecting a parameter places its current FC value and metadata default at
  the top of the detail pane, before the firmware description. Both wrap in
  full, including long enum meanings and numeric text. Compact table cells
  also expose their full formatted text on hover.
- Selecting another row returns the detail pane to its value summary, even
  after scrolling through a long description.

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

From `frontend/`, with Vite and Playwright installed (examples use port 5199):

```bash
npm run typecheck
npm run check:params-connection -- http://localhost:5199/params.html screenshots/params/connection
npm run check:params -- http://localhost:5199/params.html screenshots/params
```

`PLAYWRIGHT_MODULE=/path/to/playwright` can select an external Playwright
installation. The connection check exercises the real React page with an
isolated IPC stub: scalar version replies, tagged integer/float values,
external updates, search after deep scrolling, repeated same-endpoint
reconnects, stale read/version replies, unknown parameters, and opening the
window before the FC connects. It also checks catalog count/uniqueness/hash,
separate current/default values (including zero and missing defaults), full
long-value wrapping at 900 × 600 and with a 140 px editor, and resetting the
detail scroll position on selection.
The selected parameter, displayed row value and editor draft are observed in
one eventual assertion. Recorded 120/150/250 ms reply delays exercise initial
connection, reconnect and numeric reads, avoiding a transient old draft after
the row has updated (MR-15). Two concurrent repeat runs passed on 2026-10-24;
late replies cannot overwrite either the recovered row or its draft.
The layout check verifies column widths, origins and actual value-text
alignment within 1 CSS px at 1366, 1920 and 900 px widths, horizontal overflow
at the maximum pane width, pane controls, virtualization and Field-mode rows.

### Screenshots (isolated IPC)

- [USB_MAV_MODE displayed](../../screenshots/params/connection/params-live-usb.png)
- [Delayed numeric reply with synchronized editor](../../screenshots/params/connection/params-delayed-float.png)
- [Disconnected](../../screenshots/params/connection/params-disconnected.png)
- [Recovered on the same endpoint](../../screenshots/params/connection/params-reconnected.png)
- [Window opened before connection](../../screenshots/params/connection/params-late-connect.png)
- [Full current/default meanings at 900 × 600](../../screenshots/params/connection/params-long-values-900.png)
- [Full meanings with a 140 px editor](../../screenshots/params/connection/params-long-values-narrow.png)

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
   **Current value** and **Default** columns line up with their respective
   headers, and both values appear at the top of the detail pane.
3. Keep the window open while unplugging/replugging USB. Confirm the
   disconnected state and automatic version/selected-value recovery after
   the heartbeat returns. Also check an explicit Disconnect → Connect.
4. Open Parameters before connecting the FC, select a row, then connect.
   Confirm recovery without closing or reloading the parameter window.
5. Search `GPS_UBX_MODE` or `RTL_TYPE`, select it, and resize to 900 × 600.
   Narrow the editor; confirm both values remain visible above the long
   description and their meanings wrap in full. Scroll the detail pane down,
   then select another parameter and confirm its value summary is back on top.

The automated serial test reopens the transport; physically unplugging USB in
the native desktop window is the manual review step above.
