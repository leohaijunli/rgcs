#!/usr/bin/env python3
"""Trim PX4's parameters.json for the MagGCS parameter window (PM4).

Input : <px4-build>/parameters.json (from `make parameters_xml` /
        airframe metadata generation) — the full PX4 export.
Output: frontend/public/param-meta/parameters.json — every parameter the
        window needs, nothing it doesn't:

  name            MAVLink parameter id (16-byte wire limit)
  shortDesc       one-line description (the table column)
  longDesc        full description (the detail panel)
  group           PX4 group (the left tree, e.g. "Commander")
  category        "Standard" | "System" (kept for completeness)
  type            Int32 | Float32 | … (the wire type)
  default         default value
  min/max         range when PX4 publishes one
  rebootRequired  ⟲ marker
  values          enum options [{value, description}] when present
  bitmask         bitmask options when present
  increment      step when present

Unstable/development categories are kept (the FC may expose them depending
on modules). Export the intended board target: SITL metadata omits hardware
drivers such as cdcacm_autostart and its USB_MAV_MODE parameter.

Usage:
  python3 tools/param-meta/generate.py <px4-build>/parameters.json [out.json]

Pass --firmware-version, --target and --source-url to also write the manifest
used by the window's version guard. PX4's JSON "version" is the metadata
schema version, not the firmware version.

The output is committed: the app bundles it as a static asset
(frontend/public/param-meta/) and the window loads it at startup.
"""

import argparse
import hashlib
import json
from pathlib import Path

# Keys the window consumes; anything else in the PX4 export is dropped.
KEEP = (
    "name",
    "shortDesc",
    "longDesc",
    "group",
    "category",
    "type",
    "default",
    "min",
    "max",
    "increment",
    "rebootRequired",
    "values",
    "bitmask",
    "units",
    "decimalPlaces",
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", nargs="?", type=Path, default=(
        Path(__file__).resolve().parents[2] / "frontend/public/param-meta/parameters.json"
    ))
    parser.add_argument("--firmware-version")
    parser.add_argument("--target")
    parser.add_argument("--source-url")
    args = parser.parse_args()
    identity = (args.firmware_version, args.target, args.source_url)
    if any(identity) and not all(identity):
        parser.error("--firmware-version, --target and --source-url must be provided together")
    source_bytes = args.source.read_bytes()
    doc = json.loads(source_bytes)
    params = doc.get("parameters")
    if not isinstance(params, list) or not params:
        parser.error("source must contain a non-empty PX4 parameters array")
    trimmed = []
    names = set()
    for p in params:
        entry = {k: p[k] for k in KEEP if k in p}
        name = entry.get("name", "")
        if not isinstance(name, str) or not 1 <= len(name) <= 16 or not name.isascii() or "\0" in name:
            parser.error(f"invalid MAVLink parameter name: {name!r}")
        if name in names:
            parser.error(f"duplicate parameter: {name}")
        names.add(name)
        trimmed.append(entry)

    trimmed.sort(key=lambda e: e["name"])

    out = args.output
    out.parent.mkdir(parents=True, exist_ok=True)
    catalog_bytes = (json.dumps(trimmed, indent=1, sort_keys=False) + "\n").encode("utf-8")
    out.write_bytes(catalog_bytes)
    if all(identity):
        manifest = {
            "schemaVersion": 1,
            "firmwareVersion": args.firmware_version,
            "target": args.target,
            "parameterCount": len(trimmed),
            "sourceUrl": args.source_url,
            "sourceJsonSha256": hashlib.sha256(source_bytes).hexdigest(),
            "catalogSha256": hashlib.sha256(catalog_bytes).hexdigest(),
        }
        manifest_path = out.parent / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        print(f"manifest → {manifest_path}")
    print(f"{len(trimmed)} parameters → {out} ({len(catalog_bytes) // 1024} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
