# LAN SITL server

Turn a Ubuntu machine on the local network into a headless PX4 SITL server,
and connect MagGCS from any other machine on the same LAN — **with no MagGCS
changes**: MagGCS keeps its usual `udpin:0.0.0.0:14550` connection.

```
+-------------------+        LAN (UDP broadcast 14550)      +------------------+
| Ubuntu server     | -------------------------------------> | MagGCS machine   |
| docker / px4 SITL | <------------------------------------- | udpin:0.0.0.0:   |
| (run_sitl_docker) |        replies to <server>:18570       |        14550     |
+-------------------+                                        +------------------+
```

## How it works (PX4 v1.17)

PX4's default SITL GCS link (`ROMFS/px4fmu_common/init.d-posix/px4-rc.mavlink`)
is `mavlink start -x -u 18570 -r 4000000 -f`: it **listens** on UDP 18570 and
unicasts telemetry to `127.0.0.1:14550` (the `-t` partner-IP default), so only a
same-host GCS sees it. Two facts make the LAN server possible without touching
PX4 parameters or the GCS:

1. PX4's posix `rcS` sources `. px4-rc.mavlink` from the **first match in
   `PATH`**, so a directory prepended to `PATH` can shadow the ROMFS file.
2. The `mavlink` module's `-p` flag broadcasts to the subnet (e.g.
   `192.168.1.255`) on the same remote port 14550 while still unicasting to
   `127.0.0.1` until a GCS connects (`src/modules/mavlink/mavlink_main.cpp`,
   `Mavlink::send_packet()`).

`scripts/sitl/px4-rc.mavlink` is a verbatim v1.17 copy with `-p` added to the
GCS instance. `scripts/sitl/run_sitl_docker.sh` mounts the script directory at
`/maggcs-sitl` and prepends it to `PATH` when started with
`PX4_GCS_BROADCAST=1` (v1.17 checkouts only; other versions fall back to the
stock localhost link with a warning).

Once a GCS answers the broadcast, PX4 stops broadcasting and unicasts to that
GCS — i.e. the standard link serves **one GCS at a time**. Connect the
intended MagGCS first; a second GCS will not see discovery traffic afterwards.

## Server setup (Ubuntu machine)

```bash
# 1. Docker
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER"   # then log out/in or `newgrp docker`

# 2. PX4 v1.17 source (first run builds ~10 min; restarts are fast)
git clone -b v1.17.0 --depth 1 \
  https://github.com/PX4/PX4-Autopilot.git ~/PX4-Autopilot-v1.17

# 3. Headless SITL with LAN broadcast (from a MagGCS checkout)
PX4_GCS_BROADCAST=1 scripts/sitl/run_sitl_docker.sh ~/PX4-Autopilot-v1.17

# Useful server-side checks
tail -f ~/PX4-Autopilot-v1.17/build.log          # SITL log
docker ps --filter name=px4-sitl                 # running?
ip -br addr                                      # server IP for the notes below
```

Signs of success in `build.log`:

```
INFO  [mavlink] mode: Normal, data rate: 4000000 B/s on udp port 18570 remote port 14550
INFO  [mavlink] using network interface <if>, IP: <server-ip>
INFO  [mavlink] and broadcast IP: <subnet>.255
```

## MagGCS side (unchanged)

* Connection stays `udpin:0.0.0.0:14550`.
* Allow **inbound UDP 14550** in the OS firewall (Windows prompts once).

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| No telemetry on the GCS machine | Wi-Fi **AP/client isolation** drops broadcast frames — use wired LAN or a router without client isolation. |
| MagGCS sees heartbeat, commands ignored | A GCS reply path problem: the GCS must be able to send UDP to `<server-ip>:18570` (outbound; normally fine). |
| `WARNING: ... is not v1.17*` in the script output | Broadcast override is v1.17-only (port layout differs per release). For other versions the link stays localhost-only. |
| Motor Test SITL source stays "no data yet" while the main window is connected | PX4 unicasts to its single GCS partner (the main link). In Motor Test → Forward choose **Use main link** (ADR-029); a second `udpout` to 18570 receives no telemetry. Alternatively run a separate offboard instance and use its own endpoint. |
| Deterministic alternative needed | Connect the GCS actively to the SITL listen port instead of relying on broadcast: `udp:<server-ip>:18570` (QGC-style). Requires a GCS connection string supporting active `udp:` — a config change, not a code change. |
| WSL2 GCS machine | Same notes as the single-host setup apply — see `docs/sitl-wsl-windows.md`. |

## Multiple PX4 versions on the server

Clone one checkout per version and give each its own container name; only run
one at a time per GCS link (they all use port 14550).

```bash
PX4_GCS_BROADCAST=1 PX4_CONTAINER=px4-sitl-v117 \
  PX4_DIR=~/PX4-Autopilot-v1.17 PX4_IMAGE=px4io/px4-dev:v1.17.0 \
  scripts/sitl/run_sitl_docker.sh
```

The image tag must match the source version's toolchain (e.g. older releases
fail on newer compilers). `PX4_GCS_BROADCAST` currently supports v1.17 only;
to extend it, copy the new version's `px4-rc.mavlink`, add `-p` to the GCS
instance, and extend the version guard in `run_sitl_docker.sh`.

## Verification status

* `bash -n` passes on both scripts; the override is byte-identical to the
  v1.17 original except the documented `-p` addition.
* Live check on the dev machine (host-run `bin/px4`, no docker): with the
  override shadowed in `PATH` the log shows the subnet broadcast address and
  the GCS instance's "MAVLink only on localhost" warning disappears, while
  same-host unicast to `127.0.0.1:14550` still arrives — so the existing
  single-host workflow is unaffected.
* **Field check on the real two-machine LAN still pending**: from a second
  machine, `udpin:0.0.0.0:14550` should receive broadcast heartbeats from
  `<server-ip>:18570` within ~10 s of SITL boot, and MagGCS should connect.
