//! LAS/LAZ point reading (BC LiDAR tiles).

use std::path::Path;

use crate::error::DemError;

/// One point reduced to what gridding needs.
#[derive(Debug, Clone, Copy)]
pub struct LasPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// ASPRS classification (2 = ground).
    pub class: u8,
}

/// ASPRS ground classification.
pub const CLASS_GROUND: u8 = 2;

/// Streaming summary of a tile: header facts + projection VLR.
pub struct TileInfo {
    pub version: (u8, u8),
    pub count: u64,
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
    pub min_z: f64,
    pub max_z: f64,
    /// The `LASF_Projection` VLR payload (WKT or GeoTIFF keys), if any.
    pub projection: Option<String>,
}

/// Read the header facts without touching the points.
///
/// # Errors
///
/// [`DemError::Las`] on a malformed file.
pub fn probe(path: &Path) -> Result<TileInfo, DemError> {
    let reader = las::Reader::from_path(path)
        .map_err(|e| DemError::Las(format!("{}: {e}", path.display())))?;
    let h = reader.header();
    let projection = h
        .vlrs()
        .iter()
        .find(|v| v.user_id == "LASF_Projection")
        .map(|v| String::from_utf8_lossy(&v.data).to_string());
    Ok(TileInfo {
        version: (h.version().major, h.version().minor),
        count: h.number_of_points(),
        min_x: h.bounds().min.x,
        max_x: h.bounds().max.x,
        min_y: h.bounds().min.y,
        max_y: h.bounds().max.y,
        min_z: h.bounds().min.z,
        max_z: h.bounds().max.z,
        projection,
    })
}

/// Visit every point once (streaming, LAZ-decoded).
///
/// # Errors
///
/// [`DemError::Las`] on a decode failure mid-stream.
pub fn for_each_point(path: &Path, mut f: impl FnMut(LasPoint)) -> Result<(), DemError> {
    let mut reader = las::Reader::from_path(path)
        .map_err(|e| DemError::Las(format!("{}: {e}", path.display())))?;
    // Stream in chunks: 45M-point tiles do not fit one Vec comfortably.
    loop {
        let chunk = reader
            .read_points(1_000_000)
            .map_err(|e| DemError::Las(format!("{}: {e}", path.display())))?;
        if chunk.points().len() == 0 {
            break;
        }
        for p in chunk.points() {
            let p = p.map_err(|e| DemError::Las(format!("{}: {e}", path.display())))?;
            f(LasPoint {
                x: p.x,
                y: p.y,
                z: p.z,
                class: u8::from(p.classification),
            });
        }
    }
    Ok(())
}
