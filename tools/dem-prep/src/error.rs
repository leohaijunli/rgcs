//! Errors for the DEM preprocessing pipeline.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum DemError {
    Las(String),
    Projection(String),
    Raster(String),
    /// `dem.json` / tileset manifest (JSON) problems.
    Manifest(String),
    NoGroundPoints(PathBuf),
    /// Geoid-grid problems (missing grid, coverage, conversion).
    Geoid(String),
    /// Copernicus GLO-30 tile download problems (`fetch` subcommand).
    Fetch(String),
}

impl fmt::Display for DemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Las(m) => write!(f, "LAS error: {m}"),
            Self::Projection(m) => write!(f, "projection error: {m}"),
            Self::Raster(m) => write!(f, "raster error: {m}"),
            Self::Manifest(m) => write!(f, "manifest error: {m}"),
            Self::Geoid(m) => write!(f, "geoid error: {m}"),
            Self::Fetch(m) => write!(f, "fetch error: {m}"),
            Self::NoGroundPoints(p) => {
                write!(
                    f,
                    "no ground points in {} — cannot build a DTM",
                    p.display()
                )
            }
        }
    }
}

impl std::error::Error for DemError {}
