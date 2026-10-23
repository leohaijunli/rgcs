# param-meta

Trims the PX4 parameter metadata export for the MagGCS Parameters window.

```bash
python3 tools/param-meta/generate.py <px4-build>/parameters.json
# → frontend/public/param-meta/parameters.json (committed)
```

## Firmware and board identity

Use the firmware version read from the FC's `AUTOPILOT_VERSION` and the
intended firmware build target. PX4's export field `version: 1` is a **schema
version**, not a firmware version. Metadata also depends on the board/modules:
the SITL/general export omits the USB driver and `USB_MAV_MODE`.

The current bundle uses official **PX4 v1.17.0 / px4_fmu-v6x_default** metadata
(1912 parameters, including `USB_MAV_MODE`). Source:

<https://px4-travis.s3.amazonaws.com/Firmware/v1.17.0/px4_fmu-v6x_default/parameters.json.xz>

After downloading and decompressing that file, regenerate from the repository
root:

```bash
python3 tools/param-meta/generate.py /tmp/opencode/meta/px4_fmu-v6x_default.json \
  --firmware-version 1.17.0 \
  --target px4_fmu-v6x_default \
  --source-url https://px4-travis.s3.amazonaws.com/Firmware/v1.17.0/px4_fmu-v6x_default/parameters.json.xz
```

This writes both `parameters.json` and `manifest.json`. The manifest records
the firmware version, target, source URL, decompressed-source SHA-256, catalog
SHA-256 and parameter count. The generator rejects invalid/duplicate names.
The window reads the manifest instead of a hard-coded version and shows a
notice when the connected FC reports a different firmware version. Matching
version numbers alone do not prove that custom firmware has the same modules;
FC-reported parameters remain available even without bundled descriptions.

Rebuild the frontend **before** rebuilding the desktop binary to embed changed
metadata. See [Parameters verification](../../frontend/src/params/README.md).
