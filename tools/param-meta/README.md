# param-meta

Trims the PX4 parameter metadata export for the MagGCS Parameters window.

```bash
python3 tools/param-meta/generate.py <px4-build>/parameters.json
# → frontend/public/param-meta/parameters.json (committed)
```

Regenerate when the target PX4 version changes; the window shows a mismatch
notice when the FC's `AUTOPILOT_VERSION` differs from `meta.version`.
