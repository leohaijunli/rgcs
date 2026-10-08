//! Pattern generators.
//!
//! Pure geometry: no MAVLink, no state. Every generator returns
//! [`PatternPlan`] in flyable order (increasing `seq`), with the line table
//! indexing those sequences.

use crate::height::Height;
use crate::plan::{GeoPoint, PlannedWaypoint};

use super::error::SurveyError;
use super::pattern::{
    CloverleafPattern, LineKind, PatternLine, PatternPlan, SurveyPattern, MAX_PATTERN_ITEMS,
};
use super::projection::{unit, LocalProjection};

/// MAV_CMD_DO_CHANGE_SPEED.
const CMD_DO_CHANGE_SPEED: u16 = 178;

/// A point in the local projection.
type Local = (f64, f64);

impl SurveyPattern {
    /// Generate the sweep (and tie lines, if requested).
    ///
    /// # Errors
    ///
    /// [`SurveyError`] if the polygon is not convex/simple, a spacing or
    /// azimuth is invalid, or the result exceeds [`MAX_PATTERN_ITEMS`].
    pub fn generate(&self) -> Result<PatternPlan, SurveyError> {
        validate_polygon(&self.polygon)?;
        validate_spacing(self.line_spacing_m)?;
        validate_azimuth(self.line_azimuth_deg)?;
        validate_lead(self.lead_in_m)?;
        validate_lead(self.lead_out_m)?;
        if let Some(spacing) = self.tie_spacing_m {
            validate_spacing(spacing)?;
            validate_azimuth(self.tie_azimuth_deg)?;
        }

        let mut waypoints = Vec::new();
        let mut lines = Vec::new();
        build_parallel_lines(
            self,
            self.line_azimuth_deg,
            self.line_spacing_m,
            LineKind::Survey,
            &mut waypoints,
            &mut lines,
        )?;
        if let Some(tie_spacing) = self.tie_spacing_m {
            build_parallel_lines(
                self,
                self.tie_azimuth_deg,
                tie_spacing,
                LineKind::Tie,
                &mut waypoints,
                &mut lines,
            )?;
        }
        finish(&mut waypoints, &mut lines, self.speed_mps, self.polygon[0])?;
        Ok(PatternPlan { waypoints, lines })
    }
}

impl CloverleafPattern {
    /// Generate the rose-curve cloverleaf.
    ///
    /// # Errors
    ///
    /// [`SurveyError`] if `petals` is odd or below 4, a size parameter is not
    /// positive/finite, or the result exceeds [`MAX_PATTERN_ITEMS`].
    pub fn generate(&self) -> Result<PatternPlan, SurveyError> {
        if !self.center.is_valid() {
            return Err(SurveyError::InvalidVertex {
                lat: self.center.latitude_deg,
                lon: self.center.longitude_deg,
            });
        }
        if !self.radius_m.is_finite() || self.radius_m <= 0.0 {
            return Err(SurveyError::InvalidParameter(format!(
                "radius must be positive and finite, got {}",
                self.radius_m
            )));
        }
        if self.petals < 4 || !self.petals.is_multiple_of(2) {
            return Err(SurveyError::InvalidParameter(format!(
                "petals must be even and >= 4, got {}",
                self.petals
            )));
        }
        if self.samples_per_petal == 0 {
            return Err(SurveyError::InvalidParameter(
                "samples_per_petal must be >= 1".into(),
            ));
        }
        if !self.altitude_amsl_m.is_finite() {
            return Err(SurveyError::InvalidParameter(
                "altitude must be finite".into(),
            ));
        }
        validate_azimuth(self.start_heading_deg)?;

        let projection = LocalProjection::new(self.center);
        let altitude = Height::new(crate::height::HeightDatum::AmslEgm96, self.altitude_amsl_m);
        // r(theta) = R * cos(k * theta) with k = petals / 2 is the 2k-petal rose.
        let k = f64::from(self.petals) / 2.0;
        let n = self.petals * self.samples_per_petal;
        let mut points: Vec<Local> = Vec::with_capacity(n as usize + 1);
        for i in 0..=n {
            let theta = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            let rho = self.radius_m * (k * theta).cos();
            let (x, y) = (rho * theta.cos(), rho * theta.sin());
            let (east, north) = super::projection::rotate(x, y, self.start_heading_deg);
            points.push((east, north));
        }

        let mut waypoints: Vec<PlannedWaypoint> = points
            .iter()
            .map(|&(east, north)| {
                let mut wp = PlannedWaypoint::waypoint(projection.to_geo(east, north), 0.0);
                wp.altitude = altitude;
                wp
            })
            .collect();
        let length_m: f64 = points
            .windows(2)
            .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
            .sum();
        let mut lines = vec![PatternLine {
            id: 1,
            kind: LineKind::Calibration,
            start_seq: 0,
            end_seq: (waypoints.len() - 1) as u16,
            length_m,
        }];
        finish(&mut waypoints, &mut lines, self.speed_mps, self.center)?;
        Ok(PatternPlan { waypoints, lines })
    }
}

/// Prepend the speed change (if any), shift the line table, and check the cap.
fn finish(
    waypoints: &mut Vec<PlannedWaypoint>,
    lines: &mut [PatternLine],
    speed_mps: Option<f32>,
    fallback: GeoPoint,
) -> Result<(), SurveyError> {
    if let Some(speed) = speed_mps {
        let reference = waypoints.first().cloned().unwrap_or_else(|| {
            let mut wp = PlannedWaypoint::waypoint(fallback, 0.0);
            wp.altitude = Height::new(crate::height::HeightDatum::AmslEgm96, 0.0);
            wp
        });
        // The command item carries no coordinate: it compiles to
        // `MAV_FRAME_MISSION`, so its altitude stays zero rather than inheriting
        // the survey height (issues.md #34).
        let mut item = PlannedWaypoint::waypoint(reference.position, 0.0);
        item.command = CMD_DO_CHANGE_SPEED;
        item.params = [1.0, speed, -1.0, 0.0]; // groundspeed, m/s, no throttle
        waypoints.insert(0, item);
        for line in lines.iter_mut() {
            line.start_seq += 1;
            line.end_seq += 1;
        }
    }
    if waypoints.len() > MAX_PATTERN_ITEMS {
        return Err(SurveyError::TooManyWaypoints(
            waypoints.len(),
            MAX_PATTERN_ITEMS,
        ));
    }
    Ok(())
}

/// Clip parallel lines at `spacing_m`/`azimuth_deg` to the polygon and append
/// them to `waypoints`/`lines`.
fn build_parallel_lines(
    pattern: &SurveyPattern,
    azimuth_deg: f64,
    spacing_m: f64,
    kind: LineKind,
    waypoints: &mut Vec<PlannedWaypoint>,
    lines: &mut Vec<PatternLine>,
) -> Result<(), SurveyError> {
    let projection = LocalProjection::new(centroid(&pattern.polygon));
    let mut poly: Vec<Local> = pattern
        .polygon
        .iter()
        .map(|&p| projection.to_local(p))
        .collect();
    ensure_ccw(&mut poly);

    let dir = unit(azimuth_deg);
    let across = unit(azimuth_deg + 90.0);
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &(x, y) in &poly {
        let d = x * across.0 + y * across.1;
        min = min.min(d);
        max = max.max(d);
    }

    let altitude = Height::new(
        crate::height::HeightDatum::AmslEgm96,
        pattern.altitude_amsl_m,
    );
    let mut offset = min;
    let mut index = 0usize;
    while offset <= max + 1e-9 {
        let origin = (across.0 * offset, across.1 * offset);
        if let Some((t0, t1)) = clip_to_convex(&poly, origin, dir) {
            let mut start = (origin.0 + dir.0 * t0, origin.1 + dir.1 * t0);
            let mut end = (origin.0 + dir.0 * t1, origin.1 + dir.1 * t1);
            // Lead-in/out extend the line along its own direction.
            start = (
                start.0 - dir.0 * pattern.lead_in_m,
                start.1 - dir.1 * pattern.lead_in_m,
            );
            end = (
                end.0 + dir.0 * pattern.lead_out_m,
                end.1 + dir.1 * pattern.lead_out_m,
            );
            if pattern.alternate && index % 2 == 1 {
                std::mem::swap(&mut start, &mut end);
            }
            let length_m = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
            let start_seq = waypoints.len();
            for &(east, north) in &[start, end] {
                let mut wp = PlannedWaypoint::waypoint(projection.to_geo(east, north), 0.0);
                wp.altitude = altitude;
                waypoints.push(wp);
            }
            let id = lines.iter().filter(|l| l.kind == kind).count() as u32 + 1;
            lines.push(PatternLine {
                id,
                kind,
                start_seq: start_seq as u16,
                end_seq: (start_seq + 1) as u16,
                length_m,
            });
            index += 1;
        }
        offset += spacing_m;
    }
    Ok(())
}

/// Clip the infinite line `origin + t * dir` to a convex CCW polygon, returning
/// the `[t_min, t_max]` interval inside it.
fn clip_to_convex(poly: &[Local], origin: Local, dir: Local) -> Option<(f64, f64)> {
    let (mut t_min, mut t_max) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let edge = (b.0 - a.0, b.1 - a.1);
        // Left normal: the inside of a CCW polygon.
        let normal = (-edge.1, edge.0);
        let c = normal.0 * (origin.0 - a.0) + normal.1 * (origin.1 - a.1);
        let denom = normal.0 * dir.0 + normal.1 * dir.1;
        if denom.abs() < 1e-12 {
            if c < -1e-9 {
                return None;
            }
            continue;
        }
        let t = -c / denom;
        if denom > 0.0 {
            t_min = t_min.max(t);
        } else {
            t_max = t_max.min(t);
        }
    }
    if t_min.is_finite() && t_max.is_finite() && t_min <= t_max {
        Some((t_min, t_max))
    } else {
        None
    }
}

/// Force a local polygon into counter-clockwise order.
fn ensure_ccw(poly: &mut [Local]) {
    let mut area = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        area += a.0 * b.1 - b.0 * a.1;
    }
    if area < 0.0 {
        poly.reverse();
    }
}

fn centroid(polygon: &[GeoPoint]) -> GeoPoint {
    let n = polygon.len() as f64;
    let lat = polygon.iter().map(|p| p.latitude_deg).sum::<f64>() / n;
    let lon = polygon.iter().map(|p| p.longitude_deg).sum::<f64>() / n;
    GeoPoint::new(lat, lon)
}

fn validate_polygon(polygon: &[GeoPoint]) -> Result<(), SurveyError> {
    if polygon.len() < 3 {
        return Err(SurveyError::PolygonTooFew(polygon.len()));
    }
    for p in polygon {
        if !p.is_valid() {
            return Err(SurveyError::InvalidVertex {
                lat: p.latitude_deg,
                lon: p.longitude_deg,
            });
        }
    }
    // Convexity: every consecutive cross product has the same sign.
    let mut sign = 0i32;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let c = polygon[(i + 2) % polygon.len()];
        let ab = (
            b.latitude_deg - a.latitude_deg,
            b.longitude_deg - a.longitude_deg,
        );
        let bc = (
            c.latitude_deg - b.latitude_deg,
            c.longitude_deg - b.longitude_deg,
        );
        let cross = ab.0 * bc.1 - ab.1 * bc.0;
        if cross.abs() < 1e-15 {
            continue; // collinear edges are allowed
        }
        let s = if cross > 0.0 { 1 } else { -1 };
        if sign == 0 {
            sign = s;
        } else if sign != s {
            return Err(SurveyError::NonConvexPolygon);
        }
    }
    if sign == 0 {
        return Err(SurveyError::NonConvexPolygon);
    }
    Ok(())
}

fn validate_spacing(spacing_m: f64) -> Result<(), SurveyError> {
    if !spacing_m.is_finite() || spacing_m <= 0.0 {
        return Err(SurveyError::InvalidSpacing(spacing_m));
    }
    Ok(())
}

fn validate_azimuth(azimuth_deg: f64) -> Result<(), SurveyError> {
    if !azimuth_deg.is_finite() {
        return Err(SurveyError::InvalidAzimuth(azimuth_deg));
    }
    Ok(())
}

fn validate_lead(lead_m: f64) -> Result<(), SurveyError> {
    if !lead_m.is_finite() || lead_m < 0.0 {
        return Err(SurveyError::InvalidLead(lead_m));
    }
    Ok(())
}
