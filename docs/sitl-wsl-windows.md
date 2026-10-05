# SITL on WSL2, Ground Station on Windows

Run PX4 SITL inside WSL2 (Linux) and MagGCS on Windows. The two talk over
UDP — SITL streams telemetry to port 14550, the GCS binds
`udpin:0.0.0.0:14550` and sends commands back to 14540.

```
WSL2 (Linux)                     Windows
┌────────────────────┐           ┌─────────────────────┐
│ PX4 SITL           │   UDP     │ MagGCS desktop app   │
│ telemetry -> :14550│──────────▶│ bind udpin:0.0.0.0:14550
│ commands  <- :14540│◀──────────│                      │
└────────────────────┘           └─────────────────────┘
```

## 1. WSL2 mirrored networking (recommended)

Default WSL2 NAT makes host<->WSL UDP awkward (firewall gymnastics).
Mirrored mode shares the Windows host network stack so `localhost` works
both ways.

Add to `%UserProfile%\.wslconfig`:

```ini
[wsl2]
networkingMode=mirrored
```

Then in Windows:

```powershell
wsl --shutdown
wsl
```

Requires Windows 11 22H2+ and WSL 2.0+.

## 2. Run PX4 SITL in WSL

### Quick start (Docker, recommended)

```bash
# clone if needed
git clone --recursive https://github.com/PX4/PX4-Autopilot.git
# build (first run) or restart (later runs) SITL in Docker (jmavsim headless)
bash <repo>/scripts/sitl/run_sitl_docker.sh ~/PX4-Autopilot
tail -f ~/PX4-Autopilot/build.log   # wait for the "pxh>" prompt
```

### Direct build

```bash
cd PX4-Autopilot
make px4_sitl jmavsim          # or: HEADLESS=1 make px4_sitl jmavsim
```

## 3. Connect MagGCS on Windows

Open MagGCS -> settings -> Connection tab -> endpoint:

```
udpin:0.0.0.0:14550
```

Connect. You should see Mode/ARMED pills, RTK/GPS, and telemetry on the map.

## 4. Troubleshooting

- **No data received**: allow inbound UDP 14550 (and 14540 for command
  traffic) in Windows Defender Firewall (mirrored mode usually does not need
  this, NAT mode does).
- **NAT mode fallback**: find the WSL IP with `wsl hostname -I`, make SITL
  reachable on that IP, and bind the GCS accordingly.
- **Verify the SITL side first**: inside WSL run

  ```bash
  cd <repo> && cargo build --release --example sitl_monitor
  ./target/release/examples/sitl_monitor udpin:0.0.0.0:14550 60 3 1
  ```

  Zero heartbeat losses over 60 s means the SITL link is healthy; the
  Windows GCS then attaches to the same port.