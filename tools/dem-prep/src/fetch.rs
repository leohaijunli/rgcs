//! Copernicus DEM GLO-30 download (ADR-004 addendum, the "global fallback"):
//! given a survey box, download the covering 1° × 1° COG tiles from the AWS
//! Open Data bucket into a folder, ready for `dem-prep process`.
//!
//! Downloads shell out to `curl` (present on every dev/operator machine we
//! target) with `-C -` resume — no HTTP-client dependency, matching the
//! tool's lean dependency set. Tile naming mirrors the AWS registry: a tile
//! covers `[lat, lat+1) × [lon, lon+1)` and is keyed by its south-west
//! corner, e.g. `Copernicus_DSM_COG_10_N48_00_W123_00_DEM` covers
//! 48..49 N, -123..-122 E.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::DemError;

/// AWS Open Data, Copernicus DEM 2021 release (GLO-30 Public).
const BUCKET: &str = "copernicus-dem-30m.s3.eu-central-1.amazonaws.com";
/// The 2021 release tag in the tile key.
const GLO30_VERSION: &str = "10";

/// A covering GLO-30 tile: its key fragment and the geographic placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GloTile {
    /// Tile key fragment, e.g. `Copernicus_DSM_COG_10_N48_00_W123_00_DEM`.
    pub name: String,
    /// South-west corner (the tile covers `[lat, lat+1) × [lon, lon+1)`).
    pub lat_floor: i32,
    pub lon_floor: i32,
}

/// The GLO-30 tile whose south-west corner is `(lat_floor, lon_floor)`,
/// hemisphere taken from the sign of each corner.
///
/// # Panics
///
/// Never; formatting only.
#[must_use]
pub fn tile_name(lat_floor: i32, lon_floor: i32) -> String {
    let ns = if lat_floor >= 0 { 'N' } else { 'S' };
    let ew = if lon_floor >= 0 { 'E' } else { 'W' };
    format!(
        "Copernicus_DSM_COG_{GLO30_VERSION}_{ns}{:02}_00_{ew}{:03}_00_DEM",
        lat_floor.abs(),
        lon_floor.abs()
    )
}

/// The 1° × 1° GLO-30 tiles covering `box` (half-open in the north/east, so
/// a box edge on a tile boundary does not pull in the neighbour).
#[must_use]
pub fn covering_tiles(south: f64, north: f64, west: f64, east: f64) -> Vec<GloTile> {
    // Degenerate/inverted boxes yield nothing rather than panicking.
    if !(south < north && west < east) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut lat = south.floor() as i32;
    while f64::from(lat) < north {
        let mut lon = west.floor() as i32;
        while f64::from(lon) < east {
            out.push(GloTile {
                name: tile_name(lat, lon),
                lat_floor: lat,
                lon_floor: lon,
            });
            lon += 1;
        }
        lat += 1;
    }
    out
}

/// The COG download URL of a tile.
#[must_use]
pub fn tile_url(tile: &GloTile) -> String {
    format!("https://{BUCKET}/{0}/{0}.tif", tile.name)
}

/// Download one tile into `dest` with `curl -C -` (resume a `.part` file).
/// Retries up to three times; a final failure is a [`DemError::Fetch`].
///
/// # Errors
///
/// [`DemError::Fetch`] after the retries fail, or on a curl spawn problem.
pub fn download_tile(url: &str, dest: &Path) -> Result<(), DemError> {
    let tmp = dest.with_extension("tif.part");
    let mut last = String::new();
    for attempt in 1..=3 {
        // `-C -` resumes the partial file; `--retry` covers transient
        // network errors inside one curl run.
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--continue-at",
                "-",
                "--retry",
                "2",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(&tmp)
            .arg(url)
            .status();
        match status {
            Ok(s) if s.success() => {
                std::fs::rename(&tmp, dest)
                    .map_err(|e| DemError::Fetch(format!("rename {}: {e}", tmp.display())))?;
                return Ok(());
            }
            Ok(s) => last = format!("curl exit {s}"),
            Err(e) => last = format!("curl spawn: {e}"),
        }
        let _ = attempt;
    }
    Err(DemError::Fetch(format!(
        "download failed after 3 attempts ({last}): {url}"
    )))
}

/// Download every covering tile for the box into `dir`; existing files are
/// kept (the cache). Returns the tile paths in order.
///
/// # Errors
///
/// [`DemError::Fetch`] on a download failure, [`DemError::Raster`] on a
/// directory-creation problem.
pub fn fetch_box(
    south: f64,
    north: f64,
    west: f64,
    east: f64,
    dir: &Path,
) -> Result<Vec<PathBuf>, DemError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| DemError::Raster(format!("create {}: {e}", dir.display())))?;
    let mut paths = Vec::new();
    for tile in covering_tiles(south, north, west, east) {
        let dest = dir.join(format!("{}.tif", tile.name));
        if !dest.is_file() {
            download_tile(&tile_url(&tile), &dest)?;
        }
        paths.push(dest);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_names_follow_the_aws_key_layout() {
        // Sidney: 48..49 N, -124..-123 E (W124 because floor(-123.4) = -124).
        assert_eq!(
            tile_name(48, -124),
            "Copernicus_DSM_COG_10_N48_00_W124_00_DEM"
        );
        // Zero padding: 3° → E003, 48° → N48.
        assert_eq!(tile_name(3, 3), "Copernicus_DSM_COG_10_N03_00_E003_00_DEM");
        // Southern hemisphere.
        assert_eq!(
            tile_name(-34, 151),
            "Copernicus_DSM_COG_10_S34_00_E151_00_DEM"
        );
    }

    #[test]
    fn covering_tiles_are_the_whole_degrees_intersecting_the_box() {
        // A box inside one tile yields exactly that tile.
        let t = covering_tiles(48.60, 48.70, -123.47, -123.33);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, tile_name(48, -124));
        // A box spanning two tiles in each direction yields 4 (order: rows
        // of increasing lat, each west→east).
        let t = covering_tiles(48.9, 49.1, -123.1, -122.9);
        assert_eq!(
            t.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            vec![
                tile_name(48, -124),
                tile_name(48, -123),
                tile_name(49, -124),
                tile_name(49, -123),
            ]
        );
        // Half-open: an east edge exactly on a boundary must not pull the
        // next tile (49.0 exactly → only the 48..49 row).
        let t = covering_tiles(48.9, 49.0, -123.1, -123.0);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn degenerate_boxes_yield_no_tiles() {
        assert!(covering_tiles(48.0, 48.0, -123.0, -122.0).is_empty());
        assert!(covering_tiles(49.0, 48.0, -123.0, -122.0).is_empty());
        assert!(covering_tiles(48.0, 49.0, -122.0, -123.0).is_empty());
    }

    #[test]
    fn urls_point_at_the_open_data_bucket() {
        let t = GloTile {
            name: tile_name(48, -124),
            lat_floor: 48,
            lon_floor: -124,
        };
        assert_eq!(
            tile_url(&t),
            "https://copernicus-dem-30m.s3.eu-central-1.amazonaws.com/\
             Copernicus_DSM_COG_10_N48_00_W124_00_DEM/\
             Copernicus_DSM_COG_10_N48_00_W124_00_DEM.tif"
        );
    }
}
