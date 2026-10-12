#!/usr/bin/env python3
"""Generate a synthetic ground-classified LAS tile with hill relief.

Purpose: visual verification screenshots for the 3D terrain toggle (plan B
task E). The committed BC test DEM only spans -1.8..19 m, so no camera can
show visible relief from it; this script produces a deterministic ~660 m
ridge over 4x2.8 km in UTM zone 10N (the zone `dem-prep process` assumes).

The output feeds the real pipeline: dem-prep process -> dem-prep terrain ->
quantized-mesh tileset. No Python ships in the app; this is an offline
fixture generator only.

Usage:
    python3 scripts/make_hill_las.py <out.las> [--lat 49.35] [--lon -123.15]
        [--width 4000] [--height 2800] [--res 4]
"""

import argparse
import math
import os
import struct
import sys

WGS84_A = 6378137.0
WGS84_F = 1.0 / 298.257223563
UTM_K0 = 0.9996

GROUND_CLASS = 2
LAS_HEADER_SIZE = 227
POINT_RECORD_SIZE = 20  # point data format 0
SCALE = 0.01  # metres per count for x/y/z


def utm_forward(lat_deg: float, lon_deg: float, zone: int) -> tuple[float, float]:
    """WGS84 geographic -> UTM (Snyder/USGS series, sub-metre for fixtures)."""
    e2 = WGS84_F * (2.0 - WGS84_F)
    ep2 = e2 / (1.0 - e2)
    lon0 = math.radians((zone - 1) * 6.0 - 180.0 + 3.0)
    lat = math.radians(lat_deg)
    lon = math.radians(lon_deg)

    n = WGS84_A / math.sqrt(1.0 - e2 * math.sin(lat) ** 2)
    t = math.tan(lat) ** 2
    c = ep2 * math.cos(lat) ** 2
    aa = (lon - lon0) * math.cos(lat)
    m = WGS84_A * (
        (1.0 - e2 / 4.0 - 3.0 * e2**2 / 64.0 - 5.0 * e2**3 / 256.0) * lat
        - (3.0 * e2 / 8.0 + 3.0 * e2**2 / 32.0 + 45.0 * e2**3 / 1024.0) * math.sin(2.0 * lat)
        + (15.0 * e2**2 / 256.0 + 45.0 * e2**3 / 1024.0) * math.sin(4.0 * lat)
        - (35.0 * e2**3 / 3072.0) * math.sin(6.0 * lat)
    )
    easting = (
        UTM_K0
        * n
        * (
            aa
            + (1.0 - t + c) * aa**3 / 6.0
            + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * aa**5 / 120.0
        )
        + 500_000.0
    )
    northing = UTM_K0 * (
        m
        + n
        * math.tan(lat)
        * (
            aa**2 / 2.0
            + (5.0 - t + 9.0 * c + 4.0 * c * c) * aa**4 / 24.0
            + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * aa**6 / 720.0
        )
    )
    return easting, northing


def hill_height(u: float, v: float) -> float:
    """Deterministic relief in metres: one main ridge + two satellite peaks."""
    def blob(cu: float, cv: float, su: float, sv: float, amp: float) -> float:
        return amp * math.exp(-(((u - cu) / su) ** 2 + ((v - cv) / sv) ** 2) / 2.0)

    z = 30.0
    z += blob(1400.0, 1500.0, 520.0, 660.0, 620.0)  # main massif
    z += blob(3100.0, 900.0, 360.0, 300.0, 430.0)  # east peak
    z += blob(800.0, 2350.0, 470.0, 320.0, 340.0)  # SW shoulder
    z += 10.0 * math.sin(u / 95.0) * math.cos(v / 130.0)  # texture
    return max(5.0, z)


def build_las(
    path: str, lat: float, lon: float, width_m: int, height_m: int, res_m: int
) -> None:
    zone = int((lon + 180.0) // 6.0) + 1
    dlat = height_m / 111_132.0
    dlon = width_m / (111_319.5 * math.cos(math.radians(lat)))
    corners = [
        utm_forward(lat, lon, zone),
        utm_forward(lat + dlat, lon, zone),
        utm_forward(lat, lon + dlon, zone),
        utm_forward(lat + dlat, lon + dlon, zone),
    ]
    minx = min(x for x, _ in corners)
    maxx = max(x for x, _ in corners)
    miny = min(y for _, y in corners)
    maxy = max(y for _, y in corners)

    width = math.ceil((maxx - minx) / res_m)
    height = math.ceil((maxy - miny) / res_m)

    heights: list[float] = []
    for iy in range(height):
        v = (iy + 0.5) * res_m
        for ix in range(width):
            u = (ix + 0.5) * res_m
            heights.append(hill_height(u, v))

    xoff = math.floor(minx)
    yoff = math.floor(miny)
    zoff = 0.0
    inv = 1.0 / SCALE

    point = struct.Struct("<iiiHBBbBH")
    bitfield = 0b00001001  # return 1 of 1
    body = bytearray()
    for iy in range(height):
        for ix in range(width):
            cx = minx + (ix + 0.5) * res_m
            cy = miny + (iy + 0.5) * res_m
            cz = heights[iy * width + ix]
            body += point.pack(
                round((cx - xoff) * inv),
                round((cy - yoff) * inv),
                round((cz - zoff) * inv),
                0,
                bitfield,
                GROUND_CLASS,
                0,
                0,
                0,
            )
    assert len(body) == width * height * POINT_RECORD_SIZE

    n = width * height
    day_of_year = int(os.popen("date +%j").read().strip() or "1")
    year = int(os.popen("date +%Y").read().strip() or "2026")
    header = struct.pack(
        "<4sHHIHH8sBB32s32sHHHIIBHI5I" + "12d",
        b"LASF",
        0,
        0,
        0,
        0,
        0,
        b"\x00" * 8,
        1,
        2,
        b"make_hill_las".ljust(32, b"\x00"),
        b"maggcs-demo-fixture".ljust(32, b"\x00"),
        day_of_year,
        year,
        LAS_HEADER_SIZE,
        LAS_HEADER_SIZE,
        0,
        0,
        POINT_RECORD_SIZE,
        n,
        0,
        0,
        0,
        0,
        0,
        SCALE,
        SCALE,
        SCALE,
        float(xoff),
        float(yoff),
        float(zoff),
        minx + (width - 0.5) * res_m,
        minx + 0.5 * res_m,
        miny + (height - 0.5) * res_m,
        miny + 0.5 * res_m,
        max(heights),
        min(heights),
    )
    assert len(header) == LAS_HEADER_SIZE, len(header)

    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "wb") as fh:
        fh.write(header)
        fh.write(body)

    print(
        f"{path}: {n} ground pts, {width}x{height} cells @ {res_m} m, "
        f"UTM{zone}N {minx:.0f}..{maxx:.0f} E {miny:.0f}..{maxy:.0f} N, "
        f"z {min(heights):.1f}..{max(heights):.1f} m"
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("out", help="output .las path")
    ap.add_argument("--lat", type=float, default=49.35)
    ap.add_argument("--lon", type=float, default=-123.15)
    ap.add_argument("--width", type=int, default=4000, help="metres east")
    ap.add_argument("--height", type=int, default=2800, help="metres north")
    ap.add_argument("--res", type=int, default=4, help="metres per grid cell")
    args = ap.parse_args()
    build_las(args.out, args.lat, args.lon, args.width, args.height, args.res)
    return 0


if __name__ == "__main__":
    sys.exit(main())
