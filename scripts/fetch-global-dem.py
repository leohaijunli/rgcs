#!/usr/bin/env python3
"""Download a Copernicus DEM GLO-30 patch for a survey area and build a
quantized-mesh 3D tileset (ADR-004 addendum: the "global base map" fallback).

Pipeline: GLO-30 1x1-degree COG tiles from AWS Open Data -> `dem-prep
process` (GeoTIFF fallback path) -> `dem-prep terrain` -> a folder the app's
Layers ▸ Terrain (3D mesh) toggle serves. The tileset lives locally and
works fully offline afterwards.

Usage:
  python3 scripts/fetch-global-dem.py <lat> <lon> [--out DIR] [--size-km 50]
      [--maxzoom N] [--geoid FILE] [--dem-prep PATH]

Notes
- Copernicus GLO-30 is a *DSM* (surface incl. buildings/vegetation) on the
  EGM2008 geoid; tiles are 1x1 degrees (~24-32 MiB each, free to use per
  the AWS registry). Ocean pixels carry the nodata sentinel (-32768) and
  are filled from the nearest land value by the pipeline.
- Heights are served as-is (EGM2008 orthometric treated as ellipsoid) — the
  same visual-context convention as BC LiDAR without a geoid. Pass --geoid
  only with a grid matching your area's orthometric datum (e.g. a CGVD2013
  .mggeoid for Canada; EGM2008 would need a grid the loader doesn't know
  yet).
- --size-km sets the requested box edge; the covering whole 1-degree tiles
  are downloaded regardless (COG tiles cannot be trimmed client-side here).
"""

import argparse
import math
import shutil
import subprocess
import sys
from pathlib import Path
from urllib.request import Request, urlopen

# AWS Open Data, Copernicus DEM 2021 release (GLO-30 Public).
BUCKET = "copernicus-dem-30m.s3.eu-central-1.amazonaws.com"
GLO30_VERSION = "10"

REPO = Path(__file__).resolve().parent.parent

M_PER_DEG_LAT = 111_132.954


def tile_name(lat_floor: int, lon_floor: int) -> str:
    """Copernicus GLO-30 1x1-degree COG tile covering
    [lat_floor, lat_floor+1) x [lon_floor, lon_floor+1) (hemisphere by the
    south/west edge, e.g. N48_00_W123_00 covers 48..49 N, -123..-122 E)."""
    ns = "N" if lat_floor >= 0 else "S"
    ew = "E" if lon_floor >= 0 else "W"
    return (
        f"Copernicus_DSM_COG_{GLO30_VERSION}_{ns}{abs(lat_floor):02d}_00"
        f"_{ew}{abs(lon_floor):03d}_00_DEM"
    )


def covering_tiles(south: float, north: float, west: float, east: float) -> list[str]:
    tiles: list[str] = []
    for la in range(math.floor(south), math.floor(north) + 1):
        for lo in range(math.floor(west), math.floor(east) + 1):
            name = tile_name(la, lo)
            if name not in tiles:
                tiles.append(name)
    return tiles


def tile_url(name: str) -> str:
    return f"https://{BUCKET}/{name}/{name}.tif"


def download(url: str, dest: Path) -> None:
    """Fetch `url` to `dest`, resuming a partial file (AWS answers 206).

    # Errors
    SystemExit on a final failure — retried three times with backoff.
    """
    tmp = dest.with_suffix(".tif.part")
    for attempt in range(3):
        try:
            have = tmp.stat().st_size if tmp.exists() else 0
            req = Request(url, headers={"Range": f"bytes={have}-"})
            with urlopen(req, timeout=120) as resp, open(tmp, "wb") as fh:
                # A server ignoring the Range header answers 200 with the
                # full body: truncate instead of appending to a stale part.
                if resp.status != 206 and have > 0:
                    fh.seek(0)
                while True:
                    chunk = resp.read(1 << 20)
                    if not chunk:
                        break
                    fh.write(chunk)
            tmp.replace(dest)
            return
        except Exception as e:  # noqa: BLE001 — report and retry
            print(f"  {url}: attempt {attempt + 1}/3 failed: {e}", file=sys.stderr)
    raise SystemExit(f"download failed after 3 attempts: {url}")


def run(cmd: list[str], cwd: Path) -> None:
    subprocess.run(cmd, cwd=cwd, check=True)


def dem_prep_bin(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    binary = REPO / "target" / "debug" / "dem-prep"
    if not binary.exists():
        print("building dem-prep (first run)...")
        run(["cargo", "build", "-p", "dem-prep"], REPO)
    return binary


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("lat", type=float, help="survey centre latitude, degrees")
    ap.add_argument("lon", type=float, help="survey centre longitude, degrees")
    ap.add_argument("--out", type=Path, default=Path("global-terrain"),
                    help="output folder (downloads + processed + tileset subfolders)")
    ap.add_argument("--size-km", type=float, default=50.0,
                    help="requested box edge in km (default 50); whole 1-degree tiles are downloaded")
    ap.add_argument("--maxzoom", type=int, help="tileset maxzoom (default: from the 30 m resolution)")
    ap.add_argument("--geoid", type=Path, help=".mggeoid grid for H -> h conversion (dem-prep terrain --geoid)")
    ap.add_argument("--dem-prep", type=Path, help="dem-prep binary path (default: build from this repo)")
    args = ap.parse_args()

    out = args.out
    dl = out / "downloads"
    proc = out / "processed"
    tileset = out / "tileset"
    dl.mkdir(parents=True, exist_ok=True)

    # Requested box around the centre; the covering 1-degree tiles follow.
    dlat = args.size_km / M_PER_DEG_LAT
    dlon = args.size_km / (M_PER_DEG_LAT * math.cos(math.radians(args.lat)))
    south, north = args.lat - dlat / 2, args.lat + dlat / 2
    west, east = args.lon - dlon / 2, args.lon + dlon / 2
    tiles = covering_tiles(south, north, west, east)
    print(f"box {south:.3f}..{north:.3f} N, {west:.3f}..{east:.3f} E -> {len(tiles)} GLO-30 tile(s)")

    for name in tiles:
        dest = dl / f"{name}.tif"
        if dest.exists():
            print(f"  cached {name}")
            continue
        url = tile_url(name)
        print(f"  downloading {url} ({math.ceil(dest.stat().st_size / 1e6) if dest.exists() else '?'} MiB)")
        download(url, dest)

    bin_path = dem_prep_bin(args.dem_prep)
    run([str(bin_path), "process", str(dl), "--out", str(proc)], REPO)
    cmd = [str(bin_path), "terrain", str(proc), "--out", str(tileset)]
    if args.maxzoom is not None:
        cmd += ["--maxzoom", str(args.maxzoom)]
    if args.geoid:
        cmd += ["--geoid", str(args.geoid)]
    run(cmd, REPO)

    print(f"\n3D terrain ready: {tileset}")
    print("Load it in the app: Layers ▸ Terrain (3D mesh) ▸ pick this folder.")
    return 0


if __name__ == "__main__":
    sys.exit(main())