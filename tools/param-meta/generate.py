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
on modules); the window's "缺项处理" (§6) hides metadata the FC does not
report.

Usage:
  python3 tools/param-meta/generate.py <px4-build>/parameters.json [out.json]

The output is committed: the app bundles it as a static asset
(frontend/public/param-meta/) and the window loads it at startup.
"""

import json
import sys
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
)


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    src = Path(sys.argv[1])
    out = Path(
        sys.argv[2]
        if len(sys.argv) > 2
        else Path(__file__).resolve().parents[2]
        / "frontend/public/param-meta/parameters.json"
    )

    doc = json.loads(src.read_text())
    params = doc.get("parameters", [])
    trimmed = []
    for p in params:
        entry = {k: p[k] for k in KEEP if k in p}
        trimmed.append(entry)

    trimmed.sort(key=lambda e: e["name"])

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(trimmed, indent=1, sort_keys=False) + "\n")
    print(f"{len(trimmed)} parameters → {out} ({out.stat().st_size // 1024} KiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())