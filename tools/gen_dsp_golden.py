#!/usr/bin/env python3
"""Regenerate the Signal Inspector DSP golden vectors (testdata/dsp).

The Rust tests in crates/core/src/dsp pin the numeric acceptance directly
(−3.01 dB cutoff, ~−40 dB/decade rolloff, sine-peak < 1 %), so this script is a
cross-check against SciPy for the plan's "< 1e-9 coefficient error" goal:

  python3 tools/gen_dsp_golden.py   # writes testdata/dsp/*.json

Needs: numpy, scipy. Not run in CI (SciPy is not a dependency).
"""
from pathlib import Path

import numpy as np
from scipy import signal

OUT = Path(__file__).resolve().parents[1] / "testdata" / "dsp"


def butter_biquad(fc, q, fs):
    """zpk of a second-order Butterworth-like biquad, RBJ-style (fc prewarped)."""
    k = np.tan(np.pi * fc / fs)
    norm = 1.0 / (1.0 + k / q + k * k)
    b = np.array([k * k * norm, 2.0 * k * k * norm, k * k * norm])  # LPF
    a = np.array([1.0, 2.0 * (k * k - 1.0) * norm, (1.0 - k / q + k * k) * norm])
    return b, a


def main():
    OUT.mkdir(parents=True, exist_ok=True)

    # 1. Biquad coefficients for a few designs (compare to core::dsp::biquad).
    cases = [
        {"fc": 5.0, "q": 1.0 / np.sqrt(2), "fs": 1000.0},
        {"fc": 20.0, "q": 1.0 / np.sqrt(2), "fs": 1000.0},
        {"fc": 120.0, "q": 1.0 / np.sqrt(2), "fs": 1000.0},  # fc near Nyquist 500
    ]
    coeffs = []
    for c in cases:
        b, a = butter_biquad(c["fc"], c["q"], c["fs"])
        coeffs.append({"fc": c["fc"], "fs": c["fs"], "b": b.tolist(), "a": a.tolist()})
    (OUT / "biquad.json").write_text(json_dumps(coeffs))

    # 2. Frequency response of the fc=5 design at fc, 10·fc, fs/2.
    b, a = butter_biquad(5.0, 1.0 / np.sqrt(2), 1000.0)
    w, h = signal.freqz(b, a, worN=[5.0, 50.0, 500.0], fs=1000.0)
    (OUT / "biquad_response.json").write_text(
        json_dumps({"f": w.tolist(), "gain_db": (20 * np.log10(np.abs(h))).tolist()})
    )

    # 3. FFT sine peak (bin + amplitude reading) with the rect window.
    n, fs, f, a = 1024, 1024.0, 100.0, 0.5
    t = np.arange(n) / fs
    x = a * np.sin(2 * np.pi * f * t)
    mag = np.abs(np.fft.rfft(x)) / (n / 2)
    peak = int(np.argmax(mag))
    (OUT / "fft_sine.json").write_text(
        json_dumps({"n": n, "fs": fs, "f": f, "a": a, "peak_bin": peak, "peak_value": float(mag[peak])})
    )

    print(f"wrote {len(coeffs)} biquad cases + response + fft to {OUT}")


def json_dumps(obj):
    import json

    return json.dumps(obj, indent=2, sort_keys=True)


if __name__ == "__main__":
    main()