//! Deterministic synthetic instrument stream for CI (W1, ADR-020).
//!
//! [`Simulator`] produces [`InstrumentSample`]s (and MAVLink
//! [`MessageEnvelope`]s) at configurable rates without any hardware. The
//! generator is fully deterministic: a small LCG drives the noise, so a test
//! that seeds the same [`SimConfig`] replays the same numbers.
//!
//! The defaults sit in the Victoria AOI (ADR-018) — a *geographic* location,
//! not sampled geomagnetic data. The magnetic model is a generic dipole
//! (inclination/declination are configurable, defaulting to 0° declination) so
//! no NRCan regional field data is embedded.

use std::time::Instant;

use mavlink::common::{
    GpsFixType as MavGpsFixType, MavMessage, GPS_RAW_INT_DATA, HIGHRES_IMU_DATA,
};

use crate::height::{Height, HeightDatum};
use crate::mavlink::{MavHeader, MessageEnvelope};
use crate::signals::GAUSS_TO_NANOTESLA;
use crate::telemetry::GpsFixType;

use super::sample::{GnssSample, InstrumentSample, MagInstance, MagSample};

/// Synthetic stream parameters. All rates are in hertz, angles in degrees,
/// field strength in nanotesla.
#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    pub mag_rate_hz: f64,
    pub gnss_rate_hz: f64,
    pub base_field_nt: f64,
    pub inclination_deg: f64,
    pub declination_deg: f64,
    pub start_lat_deg: f64,
    pub start_lon_deg: f64,
    pub start_altitude: Height,
    pub speed_m_s: f64,
    pub course_deg: f64,
    pub seed: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            mag_rate_hz: super::monitor::EXPECTED_MAG_RATE_HZ,
            gnss_rate_hz: 5.0,
            base_field_nt: 50_000.0,
            inclination_deg: 70.0,
            declination_deg: 0.0,
            // Victoria AOI centre (ADR-018): a location, not field data.
            start_lat_deg: 48.556_06,
            start_lon_deg: -123.381_36,
            start_altitude: Height::new(HeightDatum::AmslEgm96, 20.0),
            speed_m_s: 1.2,
            course_deg: 90.0,
            seed: 42,
        }
    }
}

/// Standard deviation of the injected magnetometer noise, in nanotesla.
const MAG_NOISE_NT: f64 = 8.0;
/// Standard deviation of the injected RTK position noise, in degrees
/// (≈1 cm).
const GNSS_NOISE_DEG: f64 = 1e-7;
/// Simulated RTK HDOP.
const SIM_HDOP: f64 = 0.7;
/// Simulated visible-satellite count.
const SIM_SATELLITES: u8 = 22;
/// WGS84 degrees → MAVLink `1e7` scaled integer.
const DEG_TO_E7: f64 = 1e7;
/// Metres → MAVLink millimetres.
const M_TO_MM: f64 = 1_000.0;
/// Metres/second → MAVLink cm/s.
const MPS_TO_CMS: f64 = 100.0;
/// Degrees → MAVLink centi-degrees.
const DEG_TO_CDEG: f64 = 100.0;

/// A tiny linear congruential generator (Numerical Recipes constants) so the
/// simulator has no external RNG dependency and is reproducible.
#[derive(Debug, Clone)]
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(2_654_435_761).wrapping_add(1))
    }

    fn next_unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Top 53 bits → [0, 1).
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

/// A deterministic synthetic instrument stream.
#[derive(Debug, Clone)]
pub struct Simulator {
    config: SimConfig,
    rng: Lcg,
    time_ms: f64,
    next_mag_ms: f64,
    next_gnss_ms: f64,
    last_gnss_ms: f64,
    travelled_m: f64,
    sequence: u8,
}

impl Simulator {
    /// Build a simulator from a config.
    pub fn new(config: SimConfig) -> Self {
        Self {
            rng: Lcg::new(config.seed),
            config,
            time_ms: 0.0,
            next_mag_ms: 0.0,
            next_gnss_ms: 0.0,
            last_gnss_ms: 0.0,
            travelled_m: 0.0,
            sequence: 0,
        }
    }

    /// The config this simulator was built from.
    pub fn config(&self) -> SimConfig {
        self.config
    }

    /// Advance the simulated clock by `dt_ms` and return every sample produced
    /// in that interval.
    pub fn step(&mut self, dt_ms: f64) -> Vec<InstrumentSample> {
        let mag_period_ms = 1000.0 / self.config.mag_rate_hz;
        let gnss_period_ms = 1000.0 / self.config.gnss_rate_hz;
        self.time_ms += dt_ms;
        let mut mags = Vec::new();
        while self.next_mag_ms <= self.time_ms {
            let t = self.next_mag_ms;
            let sample = self.mag_sample(t);
            mags.push((t, sample));
            self.next_mag_ms += mag_period_ms;
        }
        let mut gnss = Vec::new();
        while self.next_gnss_ms <= self.time_ms {
            let t = self.next_gnss_ms;
            self.travelled_m += self.config.speed_m_s * ((t - self.last_gnss_ms) / 1000.0);
            self.last_gnss_ms = t;
            gnss.push((t, self.gnss_sample(t)));
            self.next_gnss_ms += gnss_period_ms;
        }
        // Interleave both streams on the simulated time axis.
        mags.append(&mut gnss);
        mags.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        mags.into_iter().map(|(_, s)| s).collect()
    }

    /// Like [`Simulator::step`], but wrapped as MAVLink envelopes — the shape a
    /// [`crate::instruments::InstrumentSource`] consumes.
    pub fn step_envelopes(&mut self, dt_ms: f64) -> Vec<MessageEnvelope> {
        self.step(dt_ms)
            .into_iter()
            .map(|sample| self.envelope(sample))
            .collect()
    }

    fn mag_sample(&mut self, t_ms: f64) -> InstrumentSample {
        let incl = self.config.inclination_deg.to_radians();
        let decl = self.config.declination_deg.to_radians();
        let total = self.config.base_field_nt;
        let x = total * incl.cos() * decl.cos() + self.noise(MAG_NOISE_NT);
        let y = total * incl.cos() * decl.sin() + self.noise(MAG_NOISE_NT);
        let z = total * incl.sin() + self.noise(MAG_NOISE_NT);
        InstrumentSample::Mag(MagSample {
            t_ms,
            instance: MagInstance::primary(),
            x_nt: x,
            y_nt: y,
            z_nt: z,
            total_nt: (x * x + y * y + z * z).sqrt(),
        })
    }

    fn gnss_sample(&mut self, t_ms: f64) -> InstrumentSample {
        let north = self.travelled_m * self.config.course_deg.to_radians().cos();
        let east = self.travelled_m * self.config.course_deg.to_radians().sin();
        // Small-angle metres → degrees around the start latitude.
        let lat = self.config.start_lat_deg + north / 111_320.0 + self.noise(GNSS_NOISE_DEG);
        let lon = self.config.start_lon_deg
            + east / (111_320.0 * self.config.start_lat_deg.to_radians().cos())
            + self.noise(GNSS_NOISE_DEG);
        InstrumentSample::Gnss(GnssSample {
            t_ms,
            fix: GpsFixType::RtkFixed,
            latitude_deg: lat,
            longitude_deg: lon,
            altitude: self.config.start_altitude,
            hdop: SIM_HDOP,
            satellites_visible: SIM_SATELLITES,
            velocity_m_s: self.config.speed_m_s,
            course_over_ground_deg: self.config.course_deg,
        })
    }

    fn noise(&mut self, scale: f64) -> f64 {
        (self.rng.next_unit() * 2.0 - 1.0) * scale
    }

    fn envelope(&mut self, sample: InstrumentSample) -> MessageEnvelope {
        let message = match sample {
            InstrumentSample::Mag(m) => MavMessage::HIGHRES_IMU(HIGHRES_IMU_DATA {
                time_usec: (m.t_ms * 1000.0) as u64,
                xmag: (m.x_nt / GAUSS_TO_NANOTESLA) as f32,
                ymag: (m.y_nt / GAUSS_TO_NANOTESLA) as f32,
                zmag: (m.z_nt / GAUSS_TO_NANOTESLA) as f32,
                ..Default::default()
            }),
            InstrumentSample::Gnss(g) => MavMessage::GPS_RAW_INT(GPS_RAW_INT_DATA {
                time_usec: (g.t_ms * 1000.0) as u64,
                lat: (g.latitude_deg * DEG_TO_E7).round() as i32,
                lon: (g.longitude_deg * DEG_TO_E7).round() as i32,
                alt: (g.altitude.meters() * M_TO_MM).round() as i32,
                eph: (g.hdop * MPS_TO_CMS).round() as u16,
                epv: (g.hdop * MPS_TO_CMS).round() as u16,
                vel: (g.velocity_m_s * MPS_TO_CMS).round() as u16,
                cog: (g.course_over_ground_deg * DEG_TO_CDEG).round() as u16,
                fix_type: MavGpsFixType::GPS_FIX_TYPE_RTK_FIXED,
                satellites_visible: g.satellites_visible,
                ..Default::default()
            }),
        };
        let header = MavHeader {
            system_id: 1,
            component_id: 1,
            sequence: self.sequence,
        };
        self.sequence = self.sequence.wrapping_add(1);
        MessageEnvelope {
            header,
            message,
            received_at: Instant::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_a_plausible_walk() {
        let c = SimConfig::default();
        assert!(c.mag_rate_hz > 0.0 && c.gnss_rate_hz > 0.0);
        assert!(c.base_field_nt > 20_000.0 && c.base_field_nt < 70_000.0);
    }

    #[test]
    fn one_second_yields_the_configured_rates() {
        let mut sim = Simulator::new(SimConfig::default());
        let mag = (0..100)
            .flat_map(|_| sim.step(10.0))
            .filter(|s| matches!(s, InstrumentSample::Mag(_)))
            .count();
        let mut sim = Simulator::new(SimConfig::default());
        let gnss = (0..100)
            .flat_map(|_| sim.step(10.0))
            .filter(|s| matches!(s, InstrumentSample::Gnss(_)))
            .count();
        assert!((mag as i64 - 100).abs() <= 1, "mag rate, got {mag}");
        assert!((gnss as i64 - 5).abs() <= 1, "gnss rate, got {gnss}");
    }

    #[test]
    fn field_magnitude_matches_the_base_field() {
        let mut sim = Simulator::new(SimConfig::default());
        let samples = sim.step(100.0);
        let mag = samples
            .iter()
            .find_map(|s| match s {
                InstrumentSample::Mag(m) => Some(*m),
                _ => None,
            })
            .expect("a mag sample");
        // Noise is ±8 nT on components; the total is within a few nT.
        assert!(
            (mag.total_nt - 50_000.0).abs() < 40.0,
            "total {}",
            mag.total_nt
        );
    }

    #[test]
    fn the_same_seed_replays_the_same_numbers() {
        let mut a = Simulator::new(SimConfig::default());
        let mut b = Simulator::new(SimConfig::default());
        for _ in 0..50 {
            let sa = a.step(10.0);
            let sb = b.step(10.0);
            for (x, y) in sa.iter().zip(sb.iter()) {
                match (x, y) {
                    (InstrumentSample::Mag(p), InstrumentSample::Mag(q)) => {
                        assert_eq!(p.x_nt.to_bits(), q.x_nt.to_bits());
                    }
                    (InstrumentSample::Gnss(p), InstrumentSample::Gnss(q)) => {
                        assert_eq!(p.latitude_deg.to_bits(), q.latitude_deg.to_bits());
                    }
                    _ => panic!("kind mismatch"),
                }
            }
        }
    }
}
