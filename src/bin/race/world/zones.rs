//! Landscape zones along a lap: each stretch of road gets its own character
//! (village, farmland, forest, meadow, lake, rocky climb).

use ridecore::race::course::Course;
use ridecore::rng::Rng;

use super::props::{Rgba, rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Village,
    Farmland,
    Forest,
    Meadow,
    Lake,
    Rocky,
}

pub struct ZoneMap {
    /// (end of stretch in lap meters, zone, seed for that stretch)
    stretches: Vec<(f64, Zone, u64)>,
    lap_m: f64,
    looped: bool,
}

/// The finish town reaches this far on either side of the line.
const TOWN_M: f64 = 450.0;

impl ZoneMap {
    /// `lakes`: whether generated lakes may appear (not on real terrain).
    pub fn new(course: &Course, seed: u64, lakes: bool) -> Self {
        let mut rng = Rng::new(seed);
        let lap_m = course.route.length_m();
        let looped = course.route.looped;
        let mut stretches = Vec::new();
        let mut d = if looped { TOWN_M } else { 0.0 };
        if looped {
            stretches.push((TOWN_M, Zone::Village, rng.next_u64()));
        }
        let mut last = Zone::Village;
        // A looped course has its town around the line at 0; a point-to-point
        // one ends in a town at the summit.
        let end = lap_m - TOWN_M;
        while d < end {
            let len = rng.range(500.0, 1200.0) as f64;
            let stop = (d + len).min(end);
            let climb = course.route.elevation_at(stop) - course.route.elevation_at(d);
            let steep = climb / (stop - d).max(1.0) as f32 > 0.04;
            let zone = if steep {
                if rng.chance(0.5) {
                    Zone::Rocky
                } else {
                    Zone::Forest
                }
            } else {
                loop {
                    let z = *rng.pick(&[
                        Zone::Farmland,
                        Zone::Forest,
                        Zone::Meadow,
                        Zone::Lake,
                        Zone::Farmland,
                        Zone::Meadow,
                    ]);
                    if z != last && (lakes || z != Zone::Lake) {
                        break z;
                    }
                }
            };
            stretches.push((stop, zone, rng.next_u64()));
            last = zone;
            d = stop;
        }
        stretches.push((lap_m + 1.0, Zone::Village, rng.next_u64()));
        Self {
            stretches,
            lap_m,
            looped,
        }
    }

    /// Real routes: zones from the map instead of chance, decided every
    /// 100 m by `zone_of` and merged into stretches.
    pub fn adopt(&mut self, mut zone_of: impl FnMut(f64) -> Zone) {
        let step = 100.0;
        let mut stretches: Vec<(f64, Zone, u64)> = Vec::new();
        let mut d = 0.0;
        while d < self.lap_m {
            let zone = zone_of(d + step / 2.0);
            let seed = self.stretch(d).2;
            match stretches.last_mut() {
                Some(last) if last.1 == zone => last.0 = d + step,
                _ => stretches.push((d + step, zone, seed)),
            }
            d += step;
        }
        if let Some(last) = stretches.last_mut() {
            last.0 = self.lap_m + 1.0;
        }
        self.stretches = stretches;
    }

    fn lap_d(&self, d: f64) -> f64 {
        if self.looped {
            d.rem_euclid(self.lap_m)
        } else {
            d.clamp(0.0, self.lap_m)
        }
    }

    pub fn at(&self, d: f64) -> Zone {
        self.stretch(d).1
    }

    /// Zone and the stretch's own seed (for consistent field layouts).
    pub fn stretch(&self, d: f64) -> (f64, Zone, u64) {
        let d = self.lap_d(d);
        *self
            .stretches
            .iter()
            .find(|s| d < s.0)
            .unwrap_or(self.stretches.last().expect("at least one stretch"))
    }
}

fn hash2(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed;
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 29)
}

/// Ground color for farmland: a rotated patchwork of fields with crop rows.
pub fn field_color(x: f32, z: f32, seed: u64) -> Rgba {
    let angle = (seed % 628) as f32 / 100.0;
    let (s, c) = angle.sin_cos();
    let (u, v) = (x * c - z * s, x * s + z * c);
    let (cu, cv) = ((u / 70.0).floor() as i32, (v / 48.0).floor() as i32);
    let crops: [Rgba; 6] = [
        rgb(0.83, 0.72, 0.36),
        rgb(0.36, 0.6, 0.25),
        rgb(0.45, 0.33, 0.22),
        rgb(0.88, 0.8, 0.22),
        rgb(0.42, 0.64, 0.3),
        rgb(0.62, 0.66, 0.3),
    ];
    let base = crops[(hash2(cu, cv, seed) % crops.len() as u64) as usize];
    let rows = 1.0 + 0.05 * (u * 0.9).sin();
    [base[0] * rows, base[1] * rows, base[2] * rows, 1.0]
}
