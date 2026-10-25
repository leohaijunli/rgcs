# Standard MAVLink stream catalog — MR-18a

`streams` exposes the compiled `common` dialect's message metadata:

```rust
pub fn search_standard_streams(query: &str) -> Vec<StandardStream>;
pub fn resolve_standard_stream(selection: &str) -> Result<StandardStream, StreamLookupError>;
pub fn px4_stream_mapping(message_id: u32) -> Option<Px4StreamMapping>;
pub fn resolve_uorb_stream(topic: &str) -> Result<StandardStream, StreamLookupError>;
```

`StandardStream` contains the numeric ID, exact message name, field names,
primitive/array wire types, optional wire units, `Px4StreamSupport` and optional
`Px4StreamMapping { uorb_sources, source_url, note }`.
The current locked `mavlink 0.17.1` catalog has **229 messages / 1883 fields**,
including inherited `standard.xml` / `minimal.xml` messages and extension fields.
Fields follow XML declaration order; the catalog does not assign byte offsets.
The XML marker type `uint8_t_mavlink_version` is shown as its wire type `uint8_t`.

Search is ASCII case-insensitive and trims surrounding whitespace. Exact
message names and decimal IDs rank first, then exact verified uORB sources,
name prefixes, name substrings,
exact field names and field substrings. Ties use numeric ID order. Blank search
returns the whole catalog. Resolution requires an exact message name, ID or
unique verified source alias:
unknown selections return `UnknownMessage`; ambiguous partial queries return
`AmbiguousSelection` with candidate IDs. Even a unique partial match requires
an explicit exact selection.

Examples:

- `BATTERY_STATUS` or `147` → ID 147; `current_battery` is `int16_t`, `cA`;
  `voltages` is `uint16_t[10]`, `mV` and `voltages_ext` is `uint16_t[4]`, `mV`.
- `SCALED_IMU` or `26` → ID 26.
- `HIGHRES_IMU` or `105` → ID 105.
- `current_battery` searches the field but cannot resolve to a single message.

### Firmware support metadata

`DocumentedV1_17` identifies message names with stream definitions registered in
[PX4 v1.17's stream registry](https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/mavlink_messages.cpp).
Some are conditional on board, dialect or build configuration. The flag does
not mean a connected vehicle is currently sending the message or can honor a
particular rate. Other catalog entries use `Unknown`; the catalog does not infer
support from the ability to decode a message. `stream_control` and the app
service provide live rate state.

### Verified PX4 uORB sources — MR-19

Source-reviewed against PX4 v1.17.0's matching `streams/<MESSAGE>.hpp`:

| Message | Verified uORB inputs |
| --- | --- |
| GPS_RAW_INT (24) | `sensor_gps` (instance 0) |
| SCALED_IMU (26) | `vehicle_imu`, `vehicle_imu_status`, `sensor_mag` (instance 0) |
| SERVO_OUTPUT_RAW (36) | `actuator_outputs` (instances 0/1, separate PX4 stream names/ports) |
| HIGHRES_IMU (105) | `vehicle_imu`, `sensor_selection`, `vehicle_magnetometer`, `vehicle_air_data`, `differential_pressure`, `estimator_sensor_bias`, `estimator_selector_status` |
| BATTERY_STATUS (147) | `battery_status` (updated instances) |
| ESC_STATUS (291) | `esc_status` (first-index batches) |

Each descriptor links to the pinned source file and explains field selection,
scaling/instance behavior. A source alias is not a complete topic transport.
`vehicle_imu` returns `AmbiguousSelection` with IDs `[26, 105]`; an unverified
topic returns `NoStandardMapping`. Standard names such as lowercase
`scaled_imu`/`highres_imu` still resolve as message names; they are not uORB
topics. [MR-20 firmware observations](../../../../docs/verification/mr20-standard-streams-20261025.md)
record actual rates, disable/stale/reconnect behavior and unavailable sources.

### Reproducible generation and verification

The committed `standard_stream_catalog.rs` is generated from the **resolved
direct MAVLink dependency of `maggcs-core`**, selected through locked Cargo
metadata. It includes the source version and SHA-256 of each input XML file.
Desktop builds use this Rust table; Python is only a development/CI tool.

From the workspace root:

```bash
python3 scripts/generate-stream-catalog.py
python3 scripts/generate-stream-catalog.py --check
cargo test -p maggcs-core --lib mavlink::streams
```

Use `--offline` when all required Cargo metadata is already cached. A dependency
or schema change makes `--check` fail until the table is deliberately regenerated.
The release CI runs this check. Unit tests also compare every catalog ID, name
and generated base-field CRC_EXTRA with the compiled decoder, then cover exact
selection/ranking, inherited messages, array/extension metadata, field search,
ambiguous/unknown queries, verified-source precedence, unmapped-topic refusal
and firmware support labels. The MR-19 catalog selection has 7 passing tests.

## Stream requests and arrival statistics — MR-18b

`stream_control` validates `StreamRequest { message_id, rate,
allow_unknown_support }` and computes `StreamStatus` from received-frame times.
`StreamRate` serializes as `{ "kind": "hertz", "hz": 20.0 }` or
`{ "kind": "disabled" }`. `stream_interval_us()` maps finite positive Hz to a
representable positive `COMMAND_LONG` f32 microsecond interval; Disabled maps to
`-1`. Invalid/unknown IDs, unrepresentable rates and unconfirmed unknown PX4
support are typed errors before a transport write.

`StreamTracker` keeps one request per exact ID and at most 256 arrival times.
Observed Hz needs two fresh frames. Its window is at least 5 s or four requested
periods; stale is older than `max(2 s, three requested periods)`. Slow requests
therefore do not appear stale merely because they are below 1 Hz. Rate changes,
reconnect and known receive gaps reset the history. Disabled requests still
report any frames that arrive, rather than assuming the firmware stopped them.

`ConnectionHandle::handle_id()` is monotonic and not reused. `generation()`
increments on every successful transport open; `connected_since()` and the
Connected event's `generation`/`started_at` identify that boundary.
`send_in_generation()` has a deadline and transport guard checked again by the
connection worker before writing. Old-generation, expired or cancelled guarded
queue entries cannot migrate to a newly connected transport.

Verification:

```bash
cargo test -p maggcs-core --lib mavlink
cargo test -p maggcs-core --test connection_integration
```

MR-18b recorded 32 MAVLink library tests and 7 connection integration tests.
These cover the catalog, actual interval boundaries, numeric arrival statistics,
slow/stale/disabled behavior and queued generation guards. See the desktop
[stream adapter](../../../app-tauri/README.md#dedicated-target-stream-manager)
for persistence and reconnect coordination.

## Serial ownership and passive FC discovery

`spawn_leased_serial_connection(config, lease)` retains a reservation through
manual transport opening/reopening and checks attachment identity around opens.
`shutdown_and_wait` joins actual transport retirement; signalling Stop alone
does not release a lease. Startup success now includes a nonzero transport
generation and the primed event stream.

`serial::probe_fc_serial` observes CRC-valid MAVLink without transmitting, uses
two distinct heartbeat sequences and checks autopilot/component/vehicle roles,
MAVLink 2, target ambiguity and local-ID conflicts. Automatic attachment starts
with PX4 multirotors. `ProbedFcSerial::connect` transfers the same live transport,
pins target IDs and seeds heartbeat evidence. A managed transport failure ends
that worker; the native supervisor performs fresh discovery for the next handle.

```bash
cargo test -p maggcs-core --lib mavlink::serial
cargo test -p maggcs-core --test connection_integration
```

Wire fixtures verify zero probe writes, CRC/sequence/version/role rejection,
ambiguity, cancellation and same-transport handoff. Physical serial/radio tests
are in the [hardware checklist](../../../../docs/verification/serial-rtk-hardware-checklist.md).
