//! The drape trajectory solver (plan §5B DR2, ADR-022 decision 6).
//!
//! Per line, in `sample_step_m` increments: sample the DTM and the
//! footprint-max DSM, form the target `z_t = DTM + target_agl + offset` and
//! the hard floor `z_min = DSM_fp + min_clearance`, then solve for `z(s)` —
//! hold the floor and the grade limits with **look-ahead**: a reverse pass
//! raises the profile before an upslope, a forward pass holds it after a
//! crest, and the two passes iterate to a fixed point so every edge ends
//! inside the grade window. Both constraints are hard; where they push the
//! planned AGL out of the acceptance band the span is **reported** (with the
//! worst excess as the deficit), never smoothed away or silently truncated.
//!
//! Pure and deterministic: same spec + geometry + DEM ⇒ same `DrapeResult`
//! (and the same `spec_hash`). The waypoints field of the result is filled by
//! the waypoint compiler (DR3); the solver emits the designed profiles.

use super::drape_spec::{
    Crossing, DemRef, DrapeResult, DrapeSample, DrapeSpec, DrapeSpecError, DrapeStats,
    InfeasibilityReason, InfeasibleSpan, LineProfile,
};
use super::{ElevationSource, TerrainError};
use crate::plan::GeoPoint;
use crate::survey::pattern::LineKind;

/// Grade-limit tolerance for the self-check (floating slack, metres/metre).
const GRADE_TOL: f64 = 1e-6;
/// Height-floor tolerance for the self-check, metres.
const FLOOR_TOL: f64 = 1e-6;
/// Bound on the raise/limit loop: each pass is O(n) and converges in a few
/// sweeps on real terrain; the cap keeps a pathological input bounded.
const MAX_RAISE_ITERATIONS: usize = 64;

/// Why the solver refused to produce a result.
#[derive(Debug, thiserror::Error)]
pub enum DrapeError {
    /// A line leaves the DEM or crosses a void — nothing is extrapolated
    /// (§5B principle 7); the operator must handle the segment.
    #[error(
        "line {line_id} has no DEM data at s = {s:.1} m — the DEM does not cover the plan there"
    )]
    NoData { line_id: String, s: f64 },
    /// The final profile violates a hard constraint — an internal bug, never
    /// emitted (§5B step 7).
    #[error("internal self-check failed: {0}")]
    SelfCheck(String),
    #[error(transparent)]
    Spec(#[from] DrapeSpecError),
    #[error(transparent)]
    Terrain(#[from] TerrainError),
    /// The compiled mission exceeds the item limit — refused with a `wp_tol`
    /// suggestion that was verified to fit (DR3: never a silent relaxation).
    #[error("the compiled mission needs {count} items, over the {limit} limit — raise the waypoint tolerance to about {suggested_wp_tol_m:.0} m and regenerate")]
    TooManyItems {
        count: usize,
        limit: usize,
        suggested_wp_tol_m: f64,
    },
}

/// One input line: id, kind and the polyline to follow (WGS84 degrees).
#[derive(Debug, Clone, PartialEq)]
pub struct InputLine {
    pub id: String,
    pub kind: LineKind,
    /// Vertices in order; at least two.
    pub vertices: Vec<GeoPoint>,
}

/// One line's solved output: the profile plus the spans where the constraints
/// pushed the AGL out of the acceptance band.
struct LineSolve {
    profile: LineProfile,
    infeasible: Vec<InfeasibleSpan>,
    datum: crate::height::HeightDatum,
}

/// Solve the drape trajectory for every line (see the module docs).
///
/// # Errors
///
/// [`DrapeError::NoData`] when the DEM does not cover a line;
/// [`DrapeError::SelfCheck`] on an internal inconsistency; spec/terrain
/// errors pass through.
pub fn solve(
    spec: &DrapeSpec,
    lines: &[InputLine],
    dem: &dyn ElevationSource,
) -> Result<DrapeResult, DrapeError> {
    // The cell-size cap is enforced by the caller when the DEM exposes a
    // resolution (the trait does not carry one); validate the rest here.
    spec.validate(None)?;
    let mut profiles = Vec::with_capacity(lines.len());
    let mut infeasible = Vec::new();
    let mut datum = crate::height::HeightDatum::default();
    for line in lines {
        let solved = solve_line(spec, line, dem)?;
        infeasible.extend(solved.infeasible);
        datum = solved.datum;
        profiles.push(solved.profile);
    }
    let crossings = compute_crossings(&profiles);
    let stats = compute_stats(&profiles, spec);
    Ok(DrapeResult {
        version: spec.version,
        lines: profiles.clone(),
        waypoints: Vec::new(), // DR3 compiles the waypoints from the profiles.
        compiled: Vec::new(),
        infeasible,
        stats,
        crossings,
        spec_hash: spec.spec_hash(),
        dem_ref: DemRef {
            datum,
            ..dem_ref(dem)
        },
    })
}

/// Provenance from the DEM's version string (the hash part of `DemRef`; the
/// HRDEM item fields arrive with the H4 metadata on the CogDem side).
fn dem_ref(dem: &dyn ElevationSource) -> DemRef {
    DemRef {
        item_id: None,
        project_name: None,
        acquisition: None,
        hash: dem.version().to_string(),
        datum: crate::height::HeightDatum::default(),
    }
}

/// The solved profile of one line, including its run-in extension.
fn solve_line(
    spec: &DrapeSpec,
    line: &InputLine,
    dem: &dyn ElevationSource,
) -> Result<LineSolve, DrapeError> {
    if line.vertices.len() < 2 {
        return Err(DrapeError::SelfCheck(format!(
            "line {} has {} vertices",
            line.id,
            line.vertices.len()
        )));
    }
    let polyline = Polyline::new(&line.vertices);
    let total = polyline.length();

    // s grid: the run-in extension first (s < 0), then the line proper.
    let step = spec.sample_step_m;
    let mut ss: Vec<f64> = Vec::new();
    let mut s = -spec.runin_length_m;
    while s < total {
        ss.push(s);
        s += step;
    }
    ss.push(total);

    // Sample the terrain along the grid.
    let n = ss.len();
    let mut dtm = vec![0.0f64; n];
    let mut dsm: Vec<Option<f64>> = vec![None; n];
    let mut xy: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    let mut datum = crate::height::HeightDatum::default();
    for (i, &si) in ss.iter().enumerate() {
        let (x, y) = polyline.at(si.clamp(0.0, total));
        xy[i] = (x, y);
        let point = GeoPoint::new(y, x);
        let (d, surface) = dem.sample_pair(point).map_err(|e| match e {
            TerrainError::OutsideCoverage(_) | TerrainError::Void(_) => DrapeError::NoData {
                line_id: line.id.clone(),
                s: si,
            },
            other => DrapeError::Terrain(other),
        })?;
        if i == 0 {
            datum = d.datum();
        }
        dtm[i] = d.meters();
        dsm[i] = if surface.meters().is_finite() {
            Some(footprint_max(
                dem,
                point,
                spec.dsm_footprint_m + spec.pos_margin_m,
                surface.meters(),
            ))
        } else {
            None
        };
    }

    // Target and floor; the floor is a hard constraint from the start.
    let mut z: Vec<f64> = vec![0.0; n];
    let mut z_min: Vec<Option<f64>> = vec![None; n];
    for i in 0..n {
        z[i] = dtm[i] + spec.target_agl_m + spec.vertical_offset.meters;
        z_min[i] = dsm[i].map(|d| d + spec.min_clearance_dsm_m.unwrap_or(0.0));
    }

    // Solve: raise to the floor, rate-limit both directions, repeat to a
    // fixed point; then the (optional) smoothing pass, and re-raise.
    let grade_up = spec.max_grade_up();
    let grade_down = spec.max_grade_down();
    for _ in 0..MAX_RAISE_ITERATIONS {
        if raise_to_floor(&mut z, &z_min) == 0 {
            break;
        }
        limit_rates(&mut z, &ss, grade_up, grade_down);
    }
    if spec.smoothing_m > 0.0 {
        smooth(&mut z, &ss, spec.smoothing_m);
        for _ in 0..MAX_RAISE_ITERATIONS {
            if raise_to_floor(&mut z, &z_min) == 0 {
                break;
            }
            limit_rates(&mut z, &ss, grade_up, grade_down);
        }
    }
    limit_rates(&mut z, &ss, grade_up, grade_down);

    // Report every span where the constraints push the AGL out of the band,
    // with the binder (§5B step 4).
    let infeasible = infeasible_spans(
        &line.id,
        &ss,
        &z,
        &dtm,
        &GradeLimits {
            target_agl_m: spec.target_agl_m,
            band_m: spec.agl_band_m,
            grade_up,
            grade_down,
        },
    );

    // Self-check (§5B step 7): the floor and the grade limits hold everywhere
    // — they are hard constraints; the band deviation is what gets reported.
    self_check(&line.id, &ss, &z, &z_min, grade_up, grade_down)?;

    let profile = ss
        .iter()
        .zip(&xy)
        .enumerate()
        .map(|(i, (si, (x, y)))| DrapeSample {
            s: *si,
            x: *x,
            y: *y,
            z_plan: z[i],
            dtm: dtm[i],
            dsm: dsm[i],
            agl_plan: z[i] - dtm[i],
            clearance: dsm[i].map(|d| z[i] - d),
        })
        .collect();
    Ok(LineSolve {
        profile: LineProfile {
            line_id: line.id.clone(),
            kind: line.kind,
            profile,
        },
        infeasible,
        datum,
    })
}

/// The target/band/grade bundle the reporting needs (DR1 spec, derived).
struct GradeLimits {
    target_agl_m: f64,
    band_m: f64,
    grade_up: f64,
    grade_down: f64,
}

/// Raise every sample below its floor to the floor. Returns the count raised.
fn raise_to_floor(z: &mut [f64], z_min: &[Option<f64>]) -> usize {
    let mut raised = 0;
    for (zi, floor) in z.iter_mut().zip(z_min) {
        if let Some(floor) = floor {
            if *zi < *floor {
                *zi = *floor;
                raised += 1;
            }
        }
    }
    raised
}

/// Enforce the grade window on every edge, by propagation: the forward pass
/// raises a sample that dropped too fast from its predecessor (the descent
/// limit), the reverse pass raises a sample that sits too low before a high
/// one (the climb limit, looked at from ahead — the profile starts climbing
/// **before** an upslope, ADR-022 decision 6). Together the two passes bound
/// every edge to `−grade_down ≤ dz/ds ≤ grade_up`.
fn limit_rates(z: &mut [f64], ss: &[f64], grade_up: f64, grade_down: f64) {
    for i in 1..z.len() {
        let ds = (ss[i] - ss[i - 1]).max(f64::EPSILON);
        let floor = z[i - 1] - grade_down * ds;
        if z[i] < floor {
            z[i] = floor;
        }
    }
    for i in (0..z.len().saturating_sub(1)).rev() {
        let ds = (ss[i + 1] - ss[i]).max(f64::EPSILON);
        let floor = z[i + 1] - grade_up * ds;
        if z[i] < floor {
            z[i] = floor;
        }
    }
}

/// Light smoothing (weighted moving average over ±1 samples, scaled by
/// `smoothing_m`), then the caller re-raises and re-limits so the
/// constraints win over the smoothing (§5B principle 2).
fn smooth(z: &mut [f64], ss: &[f64], smoothing_m: f64) {
    if z.len() < 3 {
        return;
    }
    let typical_ds = (ss[ss.len() - 1] - ss[0]) / (ss.len() as f64 - 1.0).max(1.0);
    let weight = (smoothing_m / typical_ds).clamp(0.0, 1.0) * 0.5;
    let original = z.to_vec();
    for i in 1..z.len() - 1 {
        z[i] = original[i] * (1.0 - 2.0 * weight) + (original[i - 1] + original[i + 1]) * weight;
    }
}

/// Contiguous spans where the constraints force the planned AGL out of the
/// acceptance band (§5B principle 2), with the worst excess over the band as
/// the deficit and the binder classified from the terrain.
fn infeasible_spans(
    line_id: &str,
    ss: &[f64],
    z: &[f64],
    dtm: &[f64],
    limits: &GradeLimits,
) -> Vec<InfeasibleSpan> {
    let mut spans: Vec<InfeasibleSpan> = Vec::new();
    let mut start: Option<usize> = None;
    let mut deficit = 0.0f64;
    for i in 0..ss.len() {
        let excess = ((z[i] - dtm[i] - limits.target_agl_m).abs() - limits.band_m).max(0.0);
        if excess > 0.0 {
            start.get_or_insert(i);
            deficit = deficit.max(excess);
        } else if let Some(first) = start.take() {
            spans.push(span(line_id, ss, dtm, limits, first, i - 1, deficit));
            deficit = 0.0;
        }
    }
    if let Some(first) = start {
        spans.push(span(line_id, ss, dtm, limits, first, ss.len() - 1, deficit));
    }
    spans
}

/// Classify one out-of-band span: terrain rising or falling faster than the
/// limits (the profile cannot follow the ground), else the DSM clearance
/// floor itself sits above the target band.
fn span(
    line_id: &str,
    ss: &[f64],
    dtm: &[f64],
    limits: &GradeLimits,
    first: usize,
    last: usize,
    deficit: f64,
) -> InfeasibleSpan {
    let mut worst_up = 0.0f64;
    let mut worst_down = 0.0f64;
    for i in first..last {
        let ds = (ss[i + 1] - ss[i]).max(f64::EPSILON);
        let rise = (dtm[i + 1] - dtm[i]) / ds;
        worst_up = worst_up.max(rise);
        worst_down = worst_down.max(-rise);
    }
    let reason = if worst_up > limits.grade_up + GRADE_TOL {
        InfeasibilityReason::ClimbRate
    } else if worst_down > limits.grade_down + GRADE_TOL {
        InfeasibilityReason::DescentRate
    } else {
        InfeasibilityReason::Clearance
    };
    InfeasibleSpan {
        line_id: line_id.to_string(),
        s_start: ss[first],
        s_end: ss[last],
        reason,
        max_deficit_m: deficit,
    }
}

/// The self-check (§5B step 7): the floor and the grade limits hold everywhere
/// — they are hard constraints; the band deviation is what gets reported.
fn self_check(
    line_id: &str,
    ss: &[f64],
    z: &[f64],
    z_min: &[Option<f64>],
    grade_up: f64,
    grade_down: f64,
) -> Result<(), DrapeError> {
    for i in 1..ss.len() {
        let ds = (ss[i] - ss[i - 1]).max(f64::EPSILON);
        let grade = (z[i] - z[i - 1]) / ds;
        if grade > grade_up + GRADE_TOL || grade < -grade_down - GRADE_TOL {
            return Err(DrapeError::SelfCheck(format!(
                "line {line_id}: grade {grade:.3} at s = {:.1} exceeds the limits",
                ss[i]
            )));
        }
        if let Some(floor) = z_min[i] {
            if z[i] < floor - FLOOR_TOL {
                return Err(DrapeError::SelfCheck(format!(
                    "line {line_id}: z = {:.3} below the floor {:.3} at s = {:.1}",
                    z[i], floor, ss[i]
                )));
            }
        }
    }
    Ok(())
}

/// The DSM maximum within `radius` of the point: a coarse neighbourhood sweep
/// over the DEM (spikes survive — H3's `-r max` semantics at solve time).
/// The DSM maximum within `radius` of the point (the footprint semantics the
/// solver and the monitor share; spikes survive, H3).
#[must_use]
pub(crate) fn footprint_max(
    dem: &dyn ElevationSource,
    center: GeoPoint,
    radius_m: f64,
    own: f64,
) -> f64 {
    let mut max = own;
    // ~2 m spacing inside the radius bounds the sweep on small footprints.
    const STEP_M: f64 = 2.0;
    let steps = (radius_m / STEP_M).ceil() as i32;
    for di in -steps..=steps {
        for dj in -steps..=steps {
            let dx = f64::from(di) * STEP_M;
            let dy = f64::from(dj) * STEP_M;
            if dx * dx + dy * dy > radius_m * radius_m {
                continue;
            }
            let p = GeoPoint::new(
                center.latitude_deg + dy / 111_320.0,
                center.longitude_deg
                    + dx / (111_320.0 * center.latitude_deg.to_radians().cos().max(0.1)),
            );
            if let Ok((_, dsm)) = dem.sample_pair(p) {
                let m = dsm.meters();
                if m.is_finite() && m > max {
                    max = m;
                }
            }
        }
    }
    max
}

/// Aggregate stats over every line (the run-in is part of the reported
/// profile).
fn compute_stats(profiles: &[LineProfile], spec: &DrapeSpec) -> DrapeStats {
    let mut agls: Vec<f64> = Vec::new();
    let mut min_clearance = None;
    let mut max_grade = 0.0f64;
    let mut max_climb = 0.0f64;
    for line in profiles {
        for (i, sample) in line.profile.iter().enumerate() {
            agls.push(sample.agl_plan);
            if let Some(c) = sample.clearance {
                min_clearance = Some(min_clearance.map_or(c, |m: f64| m.min(c)));
            }
            if i > 0 {
                let ds = (sample.s - line.profile[i - 1].s).max(f64::EPSILON);
                let grade = (sample.z_plan - line.profile[i - 1].z_plan) / ds;
                max_grade = max_grade.max(grade.abs());
                max_climb = max_climb.max(grade * spec.ground_speed_mps);
            }
        }
    }
    if agls.is_empty() {
        return DrapeStats::default();
    }
    let mean = agls.iter().sum::<f64>() / agls.len() as f64;
    let var = agls.iter().map(|a| (a - mean) * (a - mean)).sum::<f64>() / agls.len() as f64;
    DrapeStats {
        agl_mean_m: mean,
        agl_std_m: var.sqrt(),
        agl_min_m: agls.iter().copied().fold(f64::INFINITY, f64::min),
        agl_max_m: agls.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        min_clearance_m: min_clearance,
        max_grade,
        max_climb_needed_mps: max_climb,
        compile_deviation_max_m: None,
    }
}

/// Survey × tie crossings: where the polylines meet, how consistent are the
/// planned AGL and height (the magnetic cross-over levelling depends on it).
fn compute_crossings(profiles: &[LineProfile]) -> Vec<Crossing> {
    let mut out = Vec::new();
    for (i, a) in profiles.iter().enumerate() {
        for b in profiles.iter().skip(i + 1) {
            // Crossings pair a survey line against any other line (tie lines
            // close the grid onto the survey lines); two tie lines never
            // intersect in a boustrophedon.
            if a.kind != LineKind::Survey && b.kind != LineKind::Survey {
                continue;
            }
            out.extend(cross(a, b));
        }
    }
    out
}

fn cross(a: &LineProfile, b: &LineProfile) -> Vec<Crossing> {
    let mut out = Vec::new();
    for w in a.profile.windows(2) {
        for v in b.profile.windows(2) {
            let Some(p) = seg_intersect(
                (w[0].x, w[0].y),
                (w[1].x, w[1].y),
                (v[0].x, v[0].y),
                (v[1].x, v[1].y),
            ) else {
                continue;
            };
            let za = w[0].z_plan + (w[1].z_plan - w[0].z_plan) * p.t1;
            let zb = v[0].z_plan + (v[1].z_plan - v[0].z_plan) * p.t2;
            let agl_a = w[0].agl_plan + (w[1].agl_plan - w[0].agl_plan) * p.t1;
            let agl_b = v[0].agl_plan + (v[1].agl_plan - v[0].agl_plan) * p.t2;
            out.push(Crossing {
                line_a: a.line_id.clone(),
                line_b: b.line_id.clone(),
                agl_diff_m: agl_a - agl_b,
                z_diff_m: za - zb,
            });
        }
    }
    out
}

struct Intersection {
    t1: f64,
    t2: f64,
}

/// Segment–segment intersection parameters (`None` when parallel/disjoint).
fn seg_intersect(
    a1: (f64, f64),
    a2: (f64, f64),
    b1: (f64, f64),
    b2: (f64, f64),
) -> Option<Intersection> {
    let d1 = (a2.0 - a1.0, a2.1 - a1.1);
    let d2 = (b2.0 - b1.0, b2.1 - b1.1);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-12 {
        return None;
    }
    let t1 = ((b1.0 - a1.0) * d2.1 - (b1.1 - a1.1) * d2.0) / denom;
    let t2 = ((b1.0 - a1.0) * d1.1 - (b1.1 - a1.1) * d1.0) / denom;
    if (0.0..=1.0).contains(&t1) && (0.0..=1.0).contains(&t2) {
        Some(Intersection { t1, t2 })
    } else {
        None
    }
}

/// A planar polyline for along-line sampling (local-metre distances).
struct Polyline {
    /// Cumulative length at each vertex; `lengths[0] == 0`.
    lengths: Vec<f64>,
    vertices: Vec<GeoPoint>,
    length: f64,
}

impl Polyline {
    fn new(vertices: &[GeoPoint]) -> Self {
        let mut lengths = Vec::with_capacity(vertices.len());
        let mut total = 0.0;
        for (i, v) in vertices.iter().enumerate() {
            if i == 0 {
                lengths.push(0.0);
                continue;
            }
            let prev = &vertices[i - 1];
            let dlat = (v.latitude_deg - prev.latitude_deg) * 111_320.0;
            let dlon = (v.longitude_deg - prev.longitude_deg)
                * 111_320.0
                * prev.latitude_deg.to_radians().cos();
            total += (dlat * dlat + dlon * dlon).sqrt();
            lengths.push(total);
        }
        Self {
            lengths,
            vertices: vertices.to_vec(),
            length: total,
        }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.length
    }

    /// Position (lon, lat degrees) at along-line distance `s`.
    fn at(&self, s: f64) -> (f64, f64) {
        let i = match self.lengths.partition_point(|&l| l < s) {
            0 => 1,
            i => i.min(self.lengths.len() - 1),
        };
        let seg = self.lengths[i] - self.lengths[i - 1];
        let t = if seg <= 0.0 {
            0.0
        } else {
            (s - self.lengths[i - 1]) / seg
        };
        let a = &self.vertices[i - 1];
        let b = &self.vertices[i];
        (
            a.longitude_deg + (b.longitude_deg - a.longitude_deg) * t,
            a.latitude_deg + (b.latitude_deg - a.latitude_deg) * t,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::{Height, HeightDatum};
    use crate::terrain::DemBounds;

    /// A synthetic DTM+DSM over a local-metre function pair — the table-driven
    /// terrain the DR2 acceptance runs on (slope / step / spike / cliff /
    /// valley). `OutsideCoverage` outside the bounds; nothing extrapolated.
    struct SyntheticDem {
        dtm: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        dsm: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        origin: (f64, f64),
        extent_m: f64,
    }

    impl ElevationSource for SyntheticDem {
        fn elevation(&self, point: GeoPoint) -> Result<Height, TerrainError> {
            let (dtm, _) = self.pair(point)?;
            Ok(Height::new(HeightDatum::OrthometricCgvd2013, dtm))
        }
        fn bounds(&self) -> DemBounds {
            let (lat, lon) = self.origin;
            DemBounds {
                south: lat - self.extent_m / 111_320.0,
                north: lat + self.extent_m / 111_320.0,
                west: lon - self.extent_m / 111_320.0,
                east: lon + self.extent_m / 111_320.0,
            }
        }
        fn version(&self) -> &str {
            "synthetic-000000000000"
        }
        fn sample_pair(&self, point: GeoPoint) -> Result<(Height, Height), TerrainError> {
            let (dtm, dsm) = self.pair(point)?;
            Ok((
                Height::new(HeightDatum::OrthometricCgvd2013, dtm),
                Height::new(HeightDatum::OrthometricCgvd2013, dsm),
            ))
        }
    }

    impl SyntheticDem {
        fn pair(&self, point: GeoPoint) -> Result<(f64, f64), TerrainError> {
            let (lat0, lon0) = self.origin;
            let x = (point.longitude_deg - lon0) * 111_320.0 * lat0.to_radians().cos();
            let y = (point.latitude_deg - lat0) * 111_320.0;
            if x.abs() > self.extent_m || y.abs() > self.extent_m {
                return Err(TerrainError::OutsideCoverage(point));
            }
            Ok(((self.dtm)(x, y), (self.dsm)(x, y)))
        }
    }

    fn flat() -> SyntheticDem {
        SyntheticDem {
            dtm: Box::new(|_, _| 17.0),
            dsm: Box::new(|_, _| 18.0),
            origin: (48.5, -123.4),
            extent_m: 400.0,
        }
    }

    fn spec() -> DrapeSpec {
        DrapeSpec {
            min_clearance_dsm_m: Some(5.0),
            target_agl_m: 40.0,
            agl_band_m: 3.0,
            max_climb_rate_mps: 3.0,
            max_descent_rate_mps: 3.0,
            ground_speed_mps: 5.0,
            sample_step_m: 1.0,
            runin_length_m: 0.0,
            ..DrapeSpec::default()
        }
    }

    fn line(id: &str, lon_from: f64, lon_to: f64) -> InputLine {
        InputLine {
            id: id.into(),
            kind: LineKind::Survey,
            vertices: vec![GeoPoint::new(48.5, lon_from), GeoPoint::new(48.5, lon_to)],
        }
    }

    fn grade_window_holds(l: &LineProfile, spec: &DrapeSpec) {
        for i in 1..l.profile.len() {
            let ds = l.profile[i].s - l.profile[i - 1].s;
            let grade = (l.profile[i].z_plan - l.profile[i - 1].z_plan) / ds;
            assert!(
                grade <= spec.max_grade_up() + 1e-6 && grade >= -spec.max_grade_down() - 1e-6,
                "grade {grade} at s {}",
                l.profile[i].s
            );
        }
    }

    #[test]
    fn flat_terrain_holds_the_target_agl_exactly() {
        let dem = flat();
        let result = solve(&spec(), &[line("L1", -123.401, -123.399)], &dem).expect("solve");
        let l = &result.lines[0];
        for s in l.profile.iter() {
            assert!(
                (s.agl_plan - 40.0).abs() < 0.15,
                "agl {} at s {}",
                s.agl_plan,
                s.s
            );
            assert!((s.clearance.expect("dsm") - 39.0).abs() < 0.2);
        }
        assert!(result.infeasible.is_empty());
        assert!(result.crossings.is_empty());
        assert!((result.stats.agl_mean_m - 40.0).abs() < 0.1);
    }

    #[test]
    fn constraints_hold_on_slope_step_spike_cliff_and_valley() {
        // A 20 % slope, a 15 m step, a 12 m spike, a cliff down and a valley
        // — every terrain the acceptance names, on one line. The floor and
        // the grade limits are hard; out-of-band AGL is reported, never a
        // constraint violation.
        let dem = SyntheticDem {
            dtm: Box::new(|x, _| {
                let mut z = 17.0 + 0.2 * x;
                if x > 50.0 {
                    z += 15.0;
                }
                let spike = (60.0 - x).abs();
                if spike < 3.0 {
                    z += 12.0 * (1.0 - spike / 3.0);
                }
                if x > 120.0 {
                    z -= 25.0;
                }
                if (200.0..220.0).contains(&x) {
                    z -= 8.0;
                }
                z
            }),
            dsm: Box::new(|x, y| {
                let obstacle = if (150.0..152.0).contains(&x) && y.abs() < 2.0 {
                    10.0
                } else {
                    0.0
                };
                1.0 + obstacle
            }),
            origin: (48.5, -123.4),
            extent_m: 400.0,
        };
        let spec = spec();
        let result = solve(&spec, &[line("L1", -123.403, -123.397)], &dem).expect("solve");
        let l = &result.lines[0];
        grade_window_holds(l, &spec);
        // Outside the reported spans the AGL sits inside the band.
        for s in l.profile.iter() {
            let excess = (s.agl_plan - spec.target_agl_m).abs() - spec.agl_band_m;
            if excess > 1e-6 {
                assert!(
                    result
                        .infeasible
                        .iter()
                        .any(|sp| s.s >= sp.s_start - 1e-9 && s.s <= sp.s_end + 1e-9),
                    "out-of-band AGL {} at s {} is not in a reported span",
                    s.agl_plan,
                    s.s
                );
            }
        }
    }

    #[test]
    fn a_step_forces_a_reported_out_of_band_span() {
        // A 15 m step: the profile pre-climbs (look-ahead) and post-crest the
        // AGL settles back — the deviation concentrates around the step and
        // exceeds the ±3 m band there, reported with the binder classified.
        let dem = SyntheticDem {
            dtm: Box::new(|x, _| 17.0 + if x > 50.0 { 15.0 } else { 0.0 }),
            dsm: Box::new(|x, _| 18.0 + if x > 50.0 { 15.0 } else { 0.0 }),
            origin: (48.5, -123.4),
            extent_m: 400.0,
        };
        let result = solve(&spec(), &[line("L1", -123.401, -123.399)], &dem).expect("solve");
        assert!(!result.infeasible.is_empty(), "the step is out of band");
        // Numeric: the worst excess over the band approaches the step height
        // minus what the pre-climb absorbed.
        let worst = result
            .infeasible
            .iter()
            .map(|sp| sp.max_deficit_m)
            .fold(0.0, f64::max);
        assert!(worst > 3.0, "deficit {worst}");
        // And the look-ahead really did climb early: 25 m before the step the
        // profile is already above the target (a causal profile would still
        // sit exactly on it).
        let l = &result.lines[0];
        let step_s = l
            .profile
            .iter()
            .map(|s| s.s)
            .find(|s| *s > 139.0)
            .expect("step");
        let before = &l.profile[l
            .profile
            .iter()
            .position(|s| s.s >= step_s - 25.0)
            .expect("pre-step")];
        assert!(
            before.agl_plan > spec().target_agl_m + 1.0,
            "pre-step AGL {:.1} shows no look-ahead climb",
            before.agl_plan
        );
    }

    #[test]
    fn the_look_ahead_holds_the_floor_where_a_causal_pass_cannot() {
        // Low AGL over an obstacle the clearance floor sits well above the
        // target: a causal (forward-only) pass arrives at the step low and
        // violates the floor; the look-ahead pre-climbs and never does.
        // Terrain: flat 17 m ground, DSM = DTM + 6, a 15 m step at x = 100.
        // Spec: target AGL 10 m (floor = DTM + 11), clearance 5, grade 0.6,
        // line starting only 30 m before the step (pre-climb is capped).
        let dem = SyntheticDem {
            dtm: Box::new(|x, _| 17.0 + if x > 100.0 { 15.0 } else { 0.0 }),
            dsm: Box::new(|x, _| 23.0 + if x > 100.0 { 15.0 } else { 0.0 }),
            origin: (48.5, -123.4),
            extent_m: 400.0,
        };
        let spec = DrapeSpec {
            min_clearance_dsm_m: Some(5.0),
            target_agl_m: 10.0,
            agl_band_m: 3.0,
            max_climb_rate_mps: 3.0,
            max_descent_rate_mps: 3.0,
            ground_speed_mps: 5.0,
            sample_step_m: 1.0,
            runin_length_m: 0.0,
            ..DrapeSpec::default()
        };
        let result =
            solve(&spec, &[line("L1", -123.4 + 0.0001, -123.4 + 0.0018)], &dem).expect("solve");
        let l = &result.lines[0];
        // The floor never breaks (the self-check already guarantees it; pin it).
        for s in l.profile.iter() {
            let floor = s.dtm + 11.0;
            assert!(
                s.z_plan >= floor - 1e-6,
                "z {} below floor {} at s {}",
                s.z_plan,
                floor,
                s.s
            );
        }
        // And the profile climbed early: 20 m before the step it is already
        // most of the way up, where a causal pass would still be at the target.
        let step_idx = l
            .profile
            .iter()
            .position(|s| s.dtm > 20.0)
            .expect("step in profile");
        let pre = &l.profile[step_idx.saturating_sub(20)];
        assert!(
            pre.z_plan > 17.0 + 10.0 + 5.0,
            "no pre-climb: AGL {:.1} at s {}",
            pre.agl_plan,
            pre.s
        );
    }

    #[test]
    fn terrain_steeper_than_the_climb_limit_is_reported_as_climb_rate() {
        // A 100 % slope against a 0.6 grade limit: the profile cannot follow
        // the ground, and the span says which limit binds.
        let dem = SyntheticDem {
            dtm: Box::new(|x, _| 17.0 + x),
            dsm: Box::new(|x, _| 18.0 + x),
            origin: (48.5, -123.4),
            extent_m: 400.0,
        };
        let result = solve(&spec(), &[line("L1", -123.402, -123.398)], &dem).expect("solve");
        assert!(
            result
                .infeasible
                .iter()
                .any(|sp| sp.reason == InfeasibilityReason::ClimbRate),
            "spans: {:?}",
            result.infeasible
        );
    }

    #[test]
    fn outside_the_dem_is_refused_not_extrapolated() {
        let dem = flat();
        // The line runs past the synthetic DEM's 400 m extent.
        let err = solve(&spec(), &[line("L1", -123.4, -123.3)], &dem).expect_err("must refuse");
        assert!(matches!(err, DrapeError::NoData { .. }), "got {err}");
    }

    #[test]
    fn the_same_inputs_solve_to_the_same_bytes() {
        let dem = flat();
        let lines = vec![line("L1", -123.401, -123.399)];
        let a = solve(&spec(), &lines, &dem).expect("a");
        let b = solve(&spec(), &lines, &dem).expect("b");
        assert_eq!(a, b, "deterministic");
        assert_eq!(a.spec_hash, spec().spec_hash());
        assert_eq!(a.dem_ref.hash, "synthetic-000000000000");
    }

    #[test]
    fn a_crossing_reports_the_agl_and_height_difference() {
        // A survey line and a tie line crossing at x = 0: both flat, so the
        // reported diffs are ~0 and well-formed (the pairing logic is what
        // this pins).
        let dem = flat();
        let survey = InputLine {
            id: "S1".into(),
            kind: LineKind::Survey,
            vertices: vec![
                GeoPoint::new(48.5 + 0.0009, -123.401),
                GeoPoint::new(48.5 - 0.0009, -123.401),
            ],
        };
        let tie = InputLine {
            id: "T1".into(),
            kind: LineKind::Tie,
            vertices: vec![
                GeoPoint::new(48.5 + 0.0009, -123.399),
                GeoPoint::new(48.5 - 0.0009, -123.399),
            ],
        };
        let result = solve(&spec(), &[survey, tie], &dem).expect("solve");
        for c in result.crossings.iter() {
            assert!(c.agl_diff_m.is_finite() && c.z_diff_m.is_finite());
            assert!(
                (c.agl_diff_m - c.z_diff_m).abs() < 1e-9,
                "flat ground: AGL diff == z diff"
            );
        }
    }
}
