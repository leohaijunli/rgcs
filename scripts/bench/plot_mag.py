#!/usr/bin/env python3
"""Plot a bench CSV (t_s,mag_total_nt header) to a standalone SVG file.

Stdlib only — no matplotlib on this box. Usage:
    python3 scripts/bench/plot_mag.py /tmp/opencode/bench_sine_mag.csv \
        [out.svg]
"""
import csv
import math
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    source = Path(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else source.with_suffix(".svg")
    rows: list[tuple[float, float]] = []
    with source.open() as handle:
        reader = csv.reader(handle)
        header = next(reader, None)
        if header != ["t_s", "mag_total_nt"]:
            print(f"unexpected header: {header}")
            return 1
        for line in reader:
            try:
                t, value = float(line[0]), float(line[1])
                if math.isfinite(t) and math.isfinite(value):
                    rows.append((t, value))
            except (IndexError, ValueError):
                continue
    if len(rows) < 2:
        print(f"not enough rows ({len(rows)})")
        return 1

    t0, t1 = rows[0][0], rows[-1][0]
    values = [v for _, v in rows]
    vmin, vmax = min(values), max(values)
    margin = 0.05 * (vmax - vmin) if vmax > vmin else 10.0
    y0, y1 = vmin - margin, vmax + margin

    W, H = 1280, 480
    pad = 60
    plot_w, plot_h = W - 2 * pad, H - 2 * pad

    def tx(t: float) -> float:
        return pad + (t - t0) / max(t1 - t0, 1e-9) * plot_w

    def ty(v: float) -> float:
        return pad + (y1 - v) / max(y1 - y0, 1e-9) * plot_h

    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
        'font-family="monospace" font-size="12">',
        f'<rect width="{W}" height="{H}" fill="#11151b"/>',
        f'<text x="{pad}" y="28" fill="#e6edf3">Total magnetic field — '
        f"M1+M3 sine 30 s bench ({len(rows)} samples @ {len(rows)/max(t1-t0, 1e-9):.0f} Hz)</text>",
    ]
    # horizontal grid lines (5)
    for i in range(6):
        v = y0 + (y1 - y0) * i / 5
        y = ty(v)
        parts.append(
            f'<line x1="{pad}" y1="{y:.1f}" x2="{W - pad}" y2="{y:.1f}" '
            f'stroke="#2a3138" stroke-width="1"/>'
        )
        parts.append(
            f'<text x="{pad - 8}" y="{y + 4:.1f}" fill="#9aa7b2" '
            f'text-anchor="end">{v:.0f}</text>'
        )
    # vertical time ticks every 5 s
    t_step = 5.0
    t = t0
    while t <= t1 + 1e-6:
        x = tx(t)
        parts.append(
            f'<line x1="{x:.1f}" y1="{pad}" x2="{x:.1f}" y2="{H - pad}" '
            f'stroke="#2a3138" stroke-width="1"/>'
        )
        parts.append(
            f'<text x="{x:.1f}" y="{H - pad + 18}" fill="#9aa7b2" '
            f'text-anchor="middle">{t:.0f}s</text>'
        )
        t += t_step
    # Initial CSV window; the original recording includes configuration reads.
    pre = [v for t, v in rows if t < 3.0]
    baseline = sum(pre) / max(len(pre), 1)
    d = "M" + " L".join(f"{tx(t):.2f},{ty(v):.2f}" for t, v in rows)
    parts.append(
        f'<path d="{d}" fill="none" stroke="#59c2ff" stroke-width="1.2"/>'
    )
    by = ty(baseline)
    parts.append(
        f'<line x1="{pad}" y1="{by:.1f}" x2="{W - pad}" y2="{by:.1f}" '
        f'stroke="#f0b429" stroke-width="1" stroke-dasharray="6 4"/>'
    )
    parts.append(
        f'<text x="{W - pad}" y="{by - 6:.1f}" fill="#f0b429" '
        f'text-anchor="end">baseline {baseline:.0f} nT</text>'
    )
    parts.append(
        f'<text x="{pad}" y="{H - pad + 36}" fill="#9aa7b2">'
        f"range {vmin:.0f}–{vmax:.0f} nT · swing {vmax - vmin:.0f} nT · "
        f"{source.name}</text>"
    )
    parts.append("</svg>")
    out.write_text("\n".join(parts), encoding="utf-8")
    print(f"wrote {out} ({len(rows)} points, baseline {baseline:.0f} nT, "
          f"swing {vmax - vmin:.0f} nT)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
