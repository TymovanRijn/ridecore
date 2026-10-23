//! Decides what to tell the trainer each tick: simulated grade (SIM mode) or
//! a power target (ERG mode). Pure logic, no I/O, so it is unit-testable.

use crate::ftms::ControlRequest;
use crate::physics::{BikeParams, G};

/// Tuning that mirrors Zwift's trainer control. Values read from Zwift's
/// defaults: grade lookahead 0.75 s (`g_FTMS_GradeLookaheadTime`), ERG off
/// below 20 W (`g_minPowerPauseThreshold`) after 5 s
/// (`g_minERGModeDisableTime`). Grades are resent on any change at FTMS
/// resolution (0.01%).
#[derive(Debug, Clone, Copy)]
pub struct ControlTuning {
    /// Seconds ahead on the route whose grade is sent (trainers lag).
    pub grade_lookahead_s: f32,
    /// Only resend a grade that moved at least this much (percent).
    pub min_grade_step_pct: f32,
    /// ERG is paused when power stays below this for `erg_pause_after_s`.
    pub erg_pause_below_w: f32,
    pub erg_pause_after_s: f32,
    /// ERG resumes once power is back above this fraction of the target.
    pub erg_resume_fraction: f32,
}

impl Default for ControlTuning {
    fn default() -> Self {
        Self {
            grade_lookahead_s: 0.75,
            min_grade_step_pct: 0.01,
            erg_pause_below_w: 20.0,
            erg_pause_after_s: 5.0,
            erg_resume_fraction: 0.5,
        }
    }
}

/// A virtual drivetrain: the rider shifts in-app instead of on the bike.
#[derive(Debug, Clone)]
pub struct VirtualGears {
    /// Gear ratios (chainring teeth / cog teeth), easiest first.
    pub ratios: Vec<f32>,
    pub index: usize,
    /// Ratio the bike is physically in on the KICKR CORE (e.g. chainring
    /// over the Zwift Cog). Learned from cadence and flywheel speed while
    /// riding, so any chainring works.
    pub physical_ratio: f32,
}

/// Wheel circumference the trainer's speed is based on (700x25c).
pub const WHEEL_CIRCUMFERENCE_M: f32 = 2.105;

impl Default for VirtualGears {
    fn default() -> Self {
        // Zwift's 24 virtual gears.
        let ratios = vec![
            0.75, 0.87, 0.99, 1.11, 1.23, 1.38, 1.53, 1.68, 1.86, 2.04, 2.22, 2.40, 2.61, 2.82,
            3.03, 3.24, 3.49, 3.74, 3.99, 4.24, 4.54, 4.84, 5.14, 5.49,
        ];
        Self {
            index: 11,
            ratios,
            physical_ratio: 34.0 / 17.0,
        }
    }
}

impl VirtualGears {
    pub fn shift_up(&mut self) {
        self.index = (self.index + 1).min(self.ratios.len() - 1);
    }
    pub fn shift_down(&mut self) {
        self.index = self.index.saturating_sub(1);
    }
    /// Updates the physical ratio from a steady pedalling sample:
    /// flywheel speed = cadence × ratio × wheel circumference.
    pub fn observe(&mut self, cadence_rpm: f32, trainer_speed: f32, dt: f32) {
        if !(50.0..=130.0).contains(&cadence_rpm) || trainer_speed < 3.0 {
            return;
        }
        let measured = trainer_speed / (cadence_rpm / 60.0 * WHEEL_CIRCUMFERENCE_M);
        if (1.0..=5.0).contains(&measured) {
            let a = (dt / 20.0).clamp(0.0, 1.0);
            self.physical_ratio += (measured - self.physical_ratio) * a;
        }
    }
    pub fn ratio(&self) -> f32 {
        self.ratios[self.index]
    }
    /// How much faster the virtual bike goes than the flywheel for the same cadence.
    pub fn speed_factor(&self) -> f32 {
        self.ratio() / self.physical_ratio
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Resistance follows the road. `difficulty` scales positive grades, 0..=1.
    Sim {
        difficulty: f32,
    },
    Erg {
        target_w: u32,
    },
}

/// The values the KICKR applies itself in SIM mode (FTMS defaults we send).
pub const TRAINER_CRR: f32 = 0.004;
pub const TRAINER_CW: f32 = 0.51;

/// Grade (percent) equivalent to the air resistance saved by drafting, so
/// the trainer gets lighter in the wheels the way it does in Zwift.
pub fn draft_relief_pct(bike: &BikeParams, speed: f32, draft: f32) -> f32 {
    let saved = 0.5 * bike.rho * bike.cda * speed * speed * draft;
    saved / (bike.total_kg() * G) * 100.0
}

pub struct TrainerController {
    pub tuning: ControlTuning,
    pub gears: Option<VirtualGears>,
    /// The trainer shifts itself (Zwift protocol): it gets the real grade
    /// and the gear ratio, and works out the resistance on its own.
    pub native_gears: bool,
    last_gear_sent: Option<f32>,
    gear_was_last: bool,
    /// Flywheel speed averaged over a few seconds. The resistance changes
    /// the flywheel speed, so reacting to every wobble would make the two
    /// chase each other.
    smooth_speed: Option<f32>,
    last_grade_sent: Option<f32>,
    last_erg_sent: Option<u32>,
    low_power_for: f32,
    pub erg_paused: bool,
    /// Seconds since anything was sent: the target is repeated now and
    /// then, so one lost write can't leave the trainer on a stale grade.
    since_sent: f32,
}

/// Repeat an unchanged target this often (seconds).
const REFRESH_S: f32 = 2.0;

/// The grades a KICKR can simulate. A steeper descent (a −22% grade went
/// out at the bottom of the Ōtarumi) left the KICKR CORE ignoring every
/// command after it, gears and grade alike, until it was reconnected.
pub const MIN_GRADE_PCT: f32 = -10.0;
pub const MAX_GRADE_PCT: f32 = 20.0;

/// Whether a run of shifts at a steady cadence left the power where it
/// was: the trainer isn't taking its commands any more. Shifting a quarter
/// or more harder (or easier) normally moves the power by far more than
/// 10% in that direction within a few seconds.
pub fn shifts_ignored(
    ratio_before: f32,
    ratio_after: f32,
    (power_before, power_after): (f32, f32),
    (cadence_before, cadence_after): (f32, f32),
) -> bool {
    let big = (ratio_after / ratio_before - 1.0).abs() >= 0.25;
    let steady = cadence_before >= 60.0
        && cadence_after >= 60.0
        && (cadence_after / cadence_before - 1.0).abs() <= 0.12;
    if !big || !steady || power_before < 30.0 || power_after < 30.0 {
        return false;
    }
    let change = if ratio_after > ratio_before {
        power_after / power_before - 1.0
    } else {
        power_before / power_after - 1.0
    };
    change < 0.10
}

impl TrainerController {
    pub fn new(tuning: ControlTuning) -> Self {
        Self {
            tuning,
            gears: None,
            native_gears: false,
            last_gear_sent: None,
            gear_was_last: false,
            smooth_speed: None,
            last_grade_sent: None,
            last_erg_sent: None,
            low_power_for: 0.0,
            erg_paused: false,
            since_sent: 0.0,
        }
    }

    /// Forgets what was sent, so the gear and the grade go out again (after
    /// the trainer reconnected).
    pub fn resend(&mut self) {
        self.last_gear_sent = None;
        self.last_grade_sent = None;
        self.last_erg_sent = None;
    }

    /// Grade (percent) the trainer should simulate. Without virtual gears it is
    /// the road grade scaled by difficulty; descents are passed through unscaled
    /// so they still feel like descents.
    pub fn sim_grade_pct(
        &self,
        road_grade_pct: f32,
        difficulty: f32,
        bike: &BikeParams,
        trainer_speed: f32,
    ) -> f32 {
        let scaled = if road_grade_pct > 0.0 {
            road_grade_pct * difficulty
        } else {
            road_grade_pct
        };
        let Some(gears) = self.gears.as_ref().filter(|_| !self.native_gears) else {
            return scaled.clamp(MIN_GRADE_PCT, MAX_GRADE_PCT);
        };
        // Virtual shifting: the rider must feel the force of the virtual bike at
        // virtual speed, transferred through the gear ratio onto the flywheel.
        // Solve trainer_force(sent_grade, v_t) == road_force(v_virtual) * k.
        let k = gears.speed_factor();
        let v_t = trainer_speed.max(1.0);
        let v_virtual = v_t * k;
        let road_force = bike.resistance_n(v_virtual, scaled / 100.0, 0.0);
        let m = bike.total_kg();
        let wanted = road_force * k;
        let aero_t = 0.5 * TRAINER_CW * v_t * v_t;
        let sin = ((wanted - aero_t) / (m * G) - TRAINER_CRR).clamp(-0.2, 0.2);
        (sin.asin().tan() * 100.0).clamp(MIN_GRADE_PCT, MAX_GRADE_PCT)
    }

    /// Returns the request to send this tick, if anything changed enough.
    pub fn tick(
        &mut self,
        mode: Mode,
        road_grade_ahead_pct: f32,
        power_w: f32,
        trainer_speed: f32,
        bike: &BikeParams,
        dt: f32,
    ) -> Option<ControlRequest> {
        self.since_sent += dt;
        let a = (dt / 3.0).clamp(0.0, 1.0);
        let smooth = self
            .smooth_speed
            .map_or(trainer_speed, |v| v + (trainer_speed - v) * a);
        self.smooth_speed = Some(smooth);
        let trainer_speed = smooth;
        let stale = self.since_sent >= REFRESH_S;
        let req = self.decide(
            mode,
            road_grade_ahead_pct,
            power_w,
            trainer_speed,
            bike,
            dt,
            stale,
        );
        if req.is_some() {
            self.since_sent = 0.0;
        }
        req
    }

    /// The grade the trainer was last told (SIM mode), for display and logs.
    pub fn last_grade(&self) -> Option<f32> {
        self.last_grade_sent
    }

    #[allow(clippy::too_many_arguments)]
    fn decide(
        &mut self,
        mode: Mode,
        road_grade_ahead_pct: f32,
        power_w: f32,
        trainer_speed: f32,
        bike: &BikeParams,
        dt: f32,
        stale: bool,
    ) -> Option<ControlRequest> {
        // A shift goes out before anything else; when repeating, gear and
        // grade take turns.
        if self.native_gears
            && let Some(ratio) = self.gears.as_ref().map(VirtualGears::ratio)
            && (self.last_gear_sent != Some(ratio) || (stale && !self.gear_was_last))
        {
            self.last_gear_sent = Some(ratio);
            self.gear_was_last = true;
            return Some(ControlRequest::SetGear {
                ratio,
                rider_kg: bike.rider_kg,
                bike_kg: bike.bike_kg,
            });
        }
        self.gear_was_last = false;
        match mode {
            Mode::Sim { difficulty } => {
                self.last_erg_sent = None;
                self.erg_paused = false;
                let grade =
                    self.sim_grade_pct(road_grade_ahead_pct, difficulty, bike, trainer_speed);
                let changed = stale
                    || self
                        .last_grade_sent
                        .is_none_or(|last| (last - grade).abs() >= self.tuning.min_grade_step_pct);
                changed.then(|| {
                    self.last_grade_sent = Some(grade);
                    ControlRequest::SetSimulation {
                        wind_mps: 0.0,
                        grade_pct: grade,
                        crr: TRAINER_CRR,
                        cw: TRAINER_CW,
                    }
                })
            }
            Mode::Erg { target_w } => {
                self.last_grade_sent = None;
                self.update_erg_pause(power_w, target_w as f32, dt);
                if self.erg_paused {
                    // Hand the rider a flat road so they can spin back up.
                    return self
                        .last_erg_sent
                        .take()
                        .map(|_| ControlRequest::SetSimulation {
                            wind_mps: 0.0,
                            grade_pct: 0.0,
                            crr: TRAINER_CRR,
                            cw: TRAINER_CW,
                        });
                }
                (stale || self.last_erg_sent != Some(target_w)).then(|| {
                    self.last_erg_sent = Some(target_w);
                    ControlRequest::SetTargetPower {
                        watts: target_w.min(i16::MAX as u32) as i16,
                    }
                })
            }
        }
    }

    /// Zwift-style protection against the ERG "spiral of death": when the
    /// rider stops pedaling the trainer would otherwise lock up.
    fn update_erg_pause(&mut self, power_w: f32, target_w: f32, dt: f32) {
        if self.erg_paused {
            if power_w >= target_w * self.tuning.erg_resume_fraction {
                self.erg_paused = false;
                self.low_power_for = 0.0;
            }
            return;
        }
        if power_w < self.tuning.erg_pause_below_w {
            self.low_power_for += dt;
            if self.low_power_for >= self.tuning.erg_pause_after_s {
                self.erg_paused = true;
            }
        } else {
            self.low_power_for = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_stay_within_what_the_kickr_can_simulate() {
        let mut c = TrainerController::new(ControlTuning::default());
        c.gears = Some(VirtualGears::default());
        c.native_gears = true;
        assert_eq!(
            c.sim_grade_pct(-21.8, 1.0, &BikeParams::default(), 15.0),
            -10.0
        );
        assert_eq!(
            c.sim_grade_pct(24.0, 1.0, &BikeParams::default(), 3.0),
            20.0
        );
        assert_eq!(
            c.sim_grade_pct(-6.0, 1.0, &BikeParams::default(), 15.0),
            -6.0
        );
    }

    #[test]
    fn ignored_shifts_are_told_from_working_ones() {
        // From the ride where the KICKR stopped listening (trainer log).
        // Working: 1.53 → 2.22 took the power from 146 to 534 W, 2.04 →
        // 1.53 eased it from 231 to 199 W.
        assert!(!shifts_ignored(1.53, 2.22, (146.0, 534.0), (115.0, 108.0)));
        assert!(!shifts_ignored(2.04, 1.53, (231.0, 199.0), (99.0, 107.0)));
        // Ignored: gear 24 down to gear 1 and back up at ~98 rpm.
        assert!(shifts_ignored(5.49, 0.75, (162.0, 180.0), (100.0, 97.0)));
        assert!(shifts_ignored(0.75, 2.22, (167.0, 180.0), (99.0, 97.0)));
        // Small shifts, coasting and sprints that change the cadence say
        // nothing.
        assert!(!shifts_ignored(2.22, 2.40, (200.0, 200.0), (90.0, 90.0)));
        assert!(!shifts_ignored(2.22, 3.03, (200.0, 200.0), (0.0, 0.0)));
        assert!(!shifts_ignored(2.22, 3.03, (200.0, 200.0), (90.0, 120.0)));
    }

    fn bike() -> BikeParams {
        BikeParams::default()
    }

    #[test]
    fn drafting_at_speed_feels_like_about_one_percent_easier() {
        let relief = draft_relief_pct(&bike(), 40.0 / 3.6, 0.35);
        assert!((0.8..1.4).contains(&relief), "{relief}");
        assert_eq!(draft_relief_pct(&bike(), 11.0, 0.0), 0.0);
    }

    #[test]
    fn sim_scales_climbs_by_difficulty_but_not_descents() {
        let c = TrainerController::new(ControlTuning::default());
        assert_eq!(c.sim_grade_pct(8.0, 0.5, &bike(), 5.0), 4.0);
        assert_eq!(c.sim_grade_pct(-4.0, 0.5, &bike(), 5.0), -4.0);
    }

    #[test]
    fn sim_only_resends_meaningful_grade_changes() {
        let mut c = TrainerController::new(ControlTuning::default());
        let m = Mode::Sim { difficulty: 1.0 };
        assert!(c.tick(m, 2.0, 200.0, 8.0, &bike(), 0.25).is_some());
        // Below FTMS resolution (0.01 %) nothing is resent.
        assert!(c.tick(m, 2.004, 200.0, 8.0, &bike(), 0.25).is_none());
        assert!(c.tick(m, 2.05, 200.0, 8.0, &bike(), 0.25).is_some());
    }

    #[test]
    fn an_unchanged_grade_is_repeated_every_two_seconds() {
        let mut c = TrainerController::new(ControlTuning::default());
        let m = Mode::Sim { difficulty: 1.0 };
        assert!(c.tick(m, 3.0, 200.0, 8.0, &bike(), 0.5).is_some());
        for _ in 0..3 {
            assert!(c.tick(m, 3.0, 200.0, 8.0, &bike(), 0.5).is_none());
        }
        assert!(c.tick(m, 3.0, 200.0, 8.0, &bike(), 0.5).is_some());
    }

    #[test]
    fn physical_ratio_is_learned_from_cadence() {
        let mut g = VirtualGears::default();
        // 50 x 14 on the Cog at 90 rpm.
        let real = 50.0 / 14.0;
        let v = 90.0 / 60.0 * real * WHEEL_CIRCUMFERENCE_M;
        for _ in 0..600 {
            g.observe(90.0, v, 0.75);
        }
        assert!(
            (g.physical_ratio - real).abs() < 0.02,
            "{}",
            g.physical_ratio
        );
        // Coasting or noise is ignored.
        g.observe(0.0, v, 0.75);
        g.observe(90.0, 0.5, 0.75);
        assert!((g.physical_ratio - real).abs() < 0.02);
    }

    #[test]
    fn harder_gear_means_more_trainer_resistance() {
        let mut c = TrainerController::new(ControlTuning::default());
        c.gears = Some(VirtualGears::default());
        let mid = c.sim_grade_pct(2.0, 1.0, &bike(), 7.0);
        c.gears.as_mut().unwrap().shift_up();
        c.gears.as_mut().unwrap().shift_up();
        let harder = c.sim_grade_pct(2.0, 1.0, &bike(), 7.0);
        assert!(harder > mid, "{harder} <= {mid}");
    }

    #[test]
    fn a_trainer_that_shifts_itself_gets_the_real_grade_and_each_shift_at_once() {
        let mut c = TrainerController::new(ControlTuning::default());
        c.gears = Some(VirtualGears::default());
        c.native_gears = true;
        let m = Mode::Sim { difficulty: 1.0 };
        assert!(matches!(
            c.tick(m, 6.0, 200.0, 12.0, &bike(), 0.5),
            Some(ControlRequest::SetGear { ratio, .. }) if ratio == 2.40
        ));
        assert!(matches!(
            c.tick(m, 6.0, 200.0, 12.0, &bike(), 0.5),
            Some(ControlRequest::SetSimulation { grade_pct, .. }) if grade_pct == 6.0
        ));
        assert_eq!(c.tick(m, 6.0, 200.0, 12.0, &bike(), 0.1), None);
        c.gears.as_mut().unwrap().shift_up();
        assert!(matches!(
            c.tick(m, 6.0, 200.0, 12.0, &bike(), 0.01),
            Some(ControlRequest::SetGear { ratio, .. }) if ratio == 2.61
        ));
        // The flywheel speed no longer changes what is sent.
        assert_eq!(c.sim_grade_pct(6.0, 1.0, &bike(), 3.0), 6.0);
    }

    #[test]
    fn erg_pauses_when_rider_stops_and_resumes_when_pedaling() {
        let mut c = TrainerController::new(ControlTuning::default());
        let m = Mode::Erg { target_w: 200 };
        assert_eq!(
            c.tick(m, 0.0, 200.0, 8.0, &bike(), 0.5),
            Some(ControlRequest::SetTargetPower { watts: 200 })
        );
        // Zwift turns ERG off after 5 s below 20 W.
        for _ in 0..9 {
            c.tick(m, 0.0, 0.0, 2.0, &bike(), 0.5);
        }
        assert!(!c.erg_paused);
        c.tick(m, 0.0, 0.0, 2.0, &bike(), 0.5);
        assert!(c.erg_paused);
        // Resuming re-sends the target because the trainer was put in SIM mode.
        assert_eq!(
            c.tick(m, 0.0, 150.0, 6.0, &bike(), 0.5),
            Some(ControlRequest::SetTargetPower { watts: 200 })
        );
        assert!(!c.erg_paused);
    }
}
