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
        // A concave polygon splits the line into several segments; each becomes
        // its own PatternLine, so the tie/survey readout and the seq table stay
        // per flown line even across a notch (B1).
        for (t0, t1) in clip_to_polygon(&poly, origin, dir) {
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

/// Clip the infinite line `origin + t * dir` to a simple polygon (convex or
/// concave), returning the `[t0, t1]` intervals of the parts inside it — a
/// concave polygon can split the line into several disjoint segments.
fn clip_to_polygon(poly: &[Local], origin: Local, dir: Local) -> Vec<(f64, f64)> {
    // Collect the line parameter at every edge crossing (the standard
    // parametric intersection: `t` along the line, `s` along the edge).
    let mut ts: Vec<f64> = Vec::new();
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let edge = (b.0 - a.0, b.1 - a.1);
        let denom = dir.0 * edge.1 - dir.1 * edge.0; // cross(dir, edge)
        if denom.abs() < 1e-12 {
            continue; // line parallel to the edge: no crossing to record
        }
        let w = (a.0 - origin.0, a.1 - origin.1);
        let t = (w.0 * edge.1 - w.1 * edge.0) / denom;
        let s = (w.0 * dir.1 - w.1 * dir.0) / denom;
        // A sweep line exactly on an edge crosses its corners with `s` a
        // float-error away from 0/1; accept a small margin and let the
        // midpoint test decide (a near-miss corner ends up outside anyway).
        if (-S_EPS..=1.0 + S_EPS).contains(&s) {
            ts.push(t);
        }
    }
    ts.sort_by(f64::total_cmp);
    // Merge crossings at the same point (a line through a vertex hits two
    // edges); otherwise the midpoint test sees a zero-length interval.
    let mut unique: Vec<f64> = Vec::with_capacity(ts.len());
    for t in ts {
        if unique
            .last()
            .is_none_or(|&last| (t - last).abs() > 1e-9 * t.abs().max(1.0))
        {
            unique.push(t);
        }
    }
    // The line alternates outside/inside at every crossing: an interval whose
    // midpoint is inside the polygon is a clipped segment (even-odd rule).
    let mut segments = Vec::new();
    for w in unique.windows(2) {
        let mid = (w[0] + w[1]) / 2.0;
        let p = (origin.0 + dir.0 * mid, origin.1 + dir.1 * mid);
        if point_in_polygon(poly, p) {
            segments.push((w[0], w[1]));
        }
    }
    segments
}

/// Even-odd point-in-polygon test (ray toward +x). Assumes a simple polygon.
///
/// A point exactly on an edge counts as inside, so a sweep line coincident with
/// the polygon boundary is flown (matching the previous convex clipper), which
/// keeps boundary lines in the plan.
fn point_in_polygon(poly: &[Local], p: Local) -> bool {
    // Distance from `p` to each segment; an on-edge point is inside.
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let ab = (b.0 - a.0, b.1 - a.1);
        let len2 = ab.0 * ab.0 + ab.1 * ab.1;
        if len2 == 0.0 {
            continue; // repeated consecutive vertex: nothing to measure
        }
        let t = (((p.0 - a.0) * ab.0 + (p.1 - a.1) * ab.1) / len2).clamp(0.0, 1.0);
        let proj = (a.0 + t * ab.0 - p.0, a.1 + t * ab.1 - p.1);
        if proj.0 * proj.0 + proj.1 * proj.1 <= ON_EDGE_EPS_M2 {
            return true;
        }
    }
    let mut inside = false;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        // Count only edges that straddle the point's y (vertices are tested
        // with the strict `>` so a point exactly on one side is unambiguous).
        if (a.1 > p.1) != (b.1 > p.1) {
            let x_cross = a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1);
            if p.0 < x_cross {
                inside = !inside;
            }
        }
    }
    inside
}

/// On-edge tolerance for [`point_in_polygon`], in metres squared (~1 µm).
const ON_EDGE_EPS_M2: f64 = 1e-12;

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
    // Work in (lat, lon) as (x, y): orientation signs are scale-invariant, so
    // the self-intersection and degenerate checks are valid in degrees.
    let pts: Vec<Local> = polygon
        .iter()
        .map(|p| (p.latitude_deg, p.longitude_deg))
        .collect();

    // Reject degenerate (all-collinear) polygons: they enclose no area. The
    // largest consecutive cross product bounds the "thickness" of the ring.
    let mut max_cross = 0.0f64;
    let mut max_len = 0.0f64;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        let c = pts[(i + 2) % pts.len()];
        max_cross = max_cross.max(orient(a, b, c).abs());
        max_len = max_len.max((b.0 - a.0).hypot(b.1 - a.1));
    }
    if max_cross <= max_len * max_len * 1e-12 {
        return Err(SurveyError::DegeneratePolygon);
    }

    // Simplicity: no two non-adjacent edges may cross or touch. Adjacent edges
    // (i,i+1) share a vertex by construction, as do the closing pair (n-1, 0).
    for i in 0..pts.len() {
        for j in i + 1..pts.len() {
            if j == i + 1 || (i == 0 && j == pts.len() - 1) {
                continue;
            }
            if segments_intersect(
                pts[i],
                pts[(i + 1) % pts.len()],
                pts[j],
                pts[(j + 1) % pts.len()],
            ) {
                return Err(SurveyError::SelfIntersectingPolygon);
            }
        }
    }
    Ok(())
}

/// Signed area of the triangle `a-b-c` (twice the oriented area).
fn orient(a: Local, b: Local, c: Local) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Collinearity/touch tolerance in degrees (≈ 1e-10 m at survey latitudes).
const ORIENT_EPS: f64 = 1e-12;

/// Edge-parameter margin for a crossing to count (see `clip_to_polygon`).
const S_EPS: f64 = 1e-9;

/// Whether `c` lies within the axis-aligned box of `a`..`b` (with tolerance).
fn on_segment(a: Local, b: Local, c: Local) -> bool {
    c.0 >= a.0.min(b.0) - ORIENT_EPS
        && c.0 <= a.0.max(b.0) + ORIENT_EPS
        && c.1 >= a.1.min(b.1) - ORIENT_EPS
        && c.1 <= a.1.max(b.1) + ORIENT_EPS
}

/// Whether segments `a-b` and `c-d` intersect (properly or at an endpoint).
fn segments_intersect(a: Local, b: Local, c: Local, d: Local) -> bool {
    let o1 = orient(a, b, c);
    let o2 = orient(a, b, d);
    let o3 = orient(c, d, a);
    let o4 = orient(c, d, b);
    let proper = (o1 > ORIENT_EPS && o2 < -ORIENT_EPS) || (o1 < -ORIENT_EPS && o2 > ORIENT_EPS);
    if proper && ((o3 > ORIENT_EPS && o4 < -ORIENT_EPS) || (o3 < -ORIENT_EPS && o4 > ORIENT_EPS)) {
        return true; // proper crossing
    }
    // Touching/collinear cases (near-collinear counts as touching).
    if o1.abs() <= ORIENT_EPS && on_segment(a, b, c) {
        return true;
    }
    if o2.abs() <= ORIENT_EPS && on_segment(a, b, d) {
        return true;
    }
    if o3.abs() <= ORIENT_EPS && on_segment(c, d, a) {
        return true;
    }
    if o4.abs() <= ORIENT_EPS && on_segment(c, d, b) {
        return true;
    }
    false
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
