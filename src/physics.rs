//! Rider speed simulation: turns measured power into virtual speed on a road
//! with a given grade, using the standard cycling force balance.

pub const G: f32 = 9.80665;

/// Drag coefficient applied to the frontal area below (inferred, not read
/// directly from Zwift: 0.7 sits in the same physics routine and gives a
/// typical road CdA of about 0.33 m²).
pub const CD: f32 = 0.7;

/// Rider plus bike frontal area in m² from height and weight. This is the
/// formula Zwift's bike physics uses (observed in its BikeEntity code):
/// A = 0.003014024 · height_cm^0.655 · kg^0.441 − 0.1159.
pub fn frontal_area_m2(height_cm: f32, kg: f32) -> f32 {
    0.003_014_024 * height_cm.powf(0.655) * kg.powf(0.441) - 0.1159
}

#[derive(Debug, Clone, Copy)]
pub struct BikeParams {
    pub rider_kg: f32,
    pub bike_kg: f32,
    /// Drag area in m².
    pub cda: f32,
    pub crr: f32,
    /// Air density in kg/m³.
    pub rho: f32,
    /// Fraction of pedal power lost in the drivetrain.
    pub drivetrain_loss: f32,
    /// Extra "virtual mass" from wheel rotational inertia, in kg.
    pub wheel_inertia_kg: f32,
}

impl Default for BikeParams {
    fn default() -> Self {
        Self {
            rider_kg: 75.0,
            bike_kg: 8.0,
            cda: 0.32,
            crr: 0.004,
            rho: 1.225,
            drivetrain_loss: 0.02,
            // Two road wheels, I ≈ 0.08 kg·m² each, radius 0.336 m: I / r² per wheel.
            wheel_inertia_kg: 2.0 * 0.08 / (0.336 * 0.336),
        }
    }
}

impl BikeParams {
    pub fn total_kg(&self) -> f32 {
        self.rider_kg + self.bike_kg
    }

    /// Resistive force at `speed` (m/s) on `grade` (rise/run), with an
    /// optional drag reduction factor (0 = none, e.g. 0.3 when drafting).
    pub fn resistance_n(&self, speed: f32, grade: f32, drag_reduction: f32) -> f32 {
        let angle = grade.atan();
        let m = self.total_kg();
        let gravity = m * G * angle.sin();
        let rolling = self.crr * m * G * angle.cos();
        let aero = 0.5 * self.rho * self.cda * (1.0 - drag_reduction) * speed * speed;
        gravity + rolling + aero
    }

    /// Steady-state speed for constant power on a constant grade (bisection).
    pub fn steady_speed(&self, power_w: f32, grade: f32) -> f32 {
        let wheel_power = power_w * (1.0 - self.drivetrain_loss);
        let (mut lo, mut hi) = (0.0f32, 40.0f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.resistance_n(mid, grade, 0.0) * mid < wheel_power {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

/// Integrates the rider's speed over time.
#[derive(Debug, Clone, Copy, Default)]
pub struct RiderState {
    pub speed: f32,
    pub distance: f64,
}

/// Below this speed power is applied as if moving at it, so a standing start
/// doesn't produce infinite force.
const MIN_PROPULSION_SPEED: f32 = 1.0;

impl RiderState {
    pub fn step(&mut self, p: &BikeParams, power_w: f32, grade: f32, drag_reduction: f32, dt: f32) {
        let wheel_power = power_w.max(0.0) * (1.0 - p.drivetrain_loss);
        let propulsion = wheel_power / self.speed.max(MIN_PROPULSION_SPEED);
        let resistance = p.resistance_n(self.speed, grade, drag_reduction);
        let accel = (propulsion - resistance) / (p.total_kg() + p.wheel_inertia_kg);
        self.speed = (self.speed + accel * dt).max(0.0);
        self.distance += (self.speed * dt) as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zwift_frontal_area_for_its_default_rider() {
        // Zwift's default rider is 177 cm and 73 kg.
        let a = frontal_area_m2(177.0, 73.0);
        assert!((a - 0.477).abs() < 0.002, "{a}");
        assert!(frontal_area_m2(190.0, 85.0) > a);
    }

    #[test]
    fn flat_200w_is_a_plausible_speed() {
        let v = BikeParams::default().steady_speed(200.0, 0.0) * 3.6;
        assert!((32.0..36.0).contains(&v), "{v} km/h");
    }

    #[test]
    fn climbing_is_slower_and_descending_faster() {
        let p = BikeParams::default();
        let flat = p.steady_speed(250.0, 0.0);
        assert!(p.steady_speed(250.0, 0.08) < flat * 0.5);
        assert!(p.steady_speed(250.0, -0.05) > flat);
    }

    #[test]
    fn integration_converges_to_steady_speed() {
        let p = BikeParams::default();
        let mut s = RiderState::default();
        for _ in 0..(120 * 20) {
            s.step(&p, 250.0, 0.03, 0.0, 0.05);
        }
        let expected = p.steady_speed(250.0, 0.03);
        assert!(
            (s.speed - expected).abs() < 0.05,
            "{} vs {}",
            s.speed,
            expected
        );
    }

    #[test]
    fn drafting_increases_speed() {
        let p = BikeParams::default();
        let solo = p.resistance_n(11.0, 0.0, 0.0);
        let draft = p.resistance_n(11.0, 0.0, 0.3);
        assert!(draft < solo);
    }
}
