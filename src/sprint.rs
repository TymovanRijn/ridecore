//! Sprint training: short all-out sprints to a line against a handful of
//! rivals, with full recovery in between. That is what raises peak and
//! short sprint power: efforts of 5–15 s at the very top of what you can
//! do, fresh, with 3–5 minutes of easy riding between them so every sprint
//! is a real one (with short rests it turns into repeated-sprint work).
//!
//! The durations come from a normal distribution (mean ± sd, clamped to
//! ±2 sd), so no two sprints are alike, and so do the rivals' watts. All
//! rivals ride your bike and your weight, so watts compare directly: a
//! rival at 800 W is beaten by riding more than 800 W to the line. A sprint
//! starts at a mark on the road, and the line is where a rival at the
//! sprint's mean watts gets in exactly the drawn duration. Rivals jump a
//! little before or after the mark: sometimes you have to react, sometimes
//! you can go first.

use serde::{Deserialize, Serialize};

use crate::i18n::t;
use crate::physics::{BikeParams, RiderState};
use crate::race::Race;
use crate::race::ai::{Intent, required_power};
use crate::rng::Rng;
use crate::training::best_average;

/// Shortest and longest sprint that can be drawn, seconds.
pub const MIN_S: f32 = 4.0;
pub const MAX_S: f32 = 45.0;
/// Countdown from the moment the start mark is set until you reach it.
pub const APPROACH_S: f32 = 15.0;
/// A rival gets to full power over this long after jumping.
const RISE_S: f32 = 1.5;
/// A rival's power at the jump, at its peak and at the end, as shares of
/// its sprint power before scaling (see `sprint_power`).
const JUMP: f32 = 0.35;
const TOP: f32 = 1.15;
const END: f32 = 0.85;
/// Between sprints the rivals ride round you at these gaps (ahead and
/// behind), and close in on their place no harder than this.
const SLOT_M: f32 = 2.2;
const REGROUP_MAX_W: f32 = 450.0;
/// A sprint nobody finishes (you stopped pedalling) ends after this long.
const GIVE_UP_S: f32 = 60.0;
/// Road kept free past the line, so a session never runs off the end.
const ROOM_M: f64 = 400.0;

/// The session as you set it up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SprintSettings {
    pub count: u32,
    /// Sprint duration, seconds: mean and standard deviation.
    pub mean_s: f32,
    pub sd_s: f32,
    /// Recovery from one line to the next start mark, seconds.
    pub rest_s: f32,
    pub warmup_s: f32,
    pub cooldown_s: f32,
    pub rivals: u32,
    /// The rivals' sprint power, watts: mean and standard deviation.
    pub rival_w: f32,
    pub rival_sd_w: f32,
    /// Route id of the road.
    pub route: String,
}

impl Default for SprintSettings {
    fn default() -> Self {
        Self {
            count: 8,
            mean_s: 12.0,
            sd_s: 3.0,
            rest_s: 240.0,
            warmup_s: 480.0,
            cooldown_s: 300.0,
            rivals: 5,
            rival_w: 750.0,
            rival_sd_w: 80.0,
            route: "kust-noordwijk".into(),
        }
    }
}

impl SprintSettings {
    /// The durations that can be drawn: mean ± 2 sd within the limits.
    pub fn duration_range(&self) -> (f32, f32) {
        let lo = (self.mean_s - 2.0 * self.sd_s).clamp(MIN_S, MAX_S);
        let hi = (self.mean_s + 2.0 * self.sd_s).clamp(lo, MAX_S);
        (lo, hi)
    }

    /// The rivals' watts that can be drawn.
    pub fn watts_range(&self) -> (f32, f32) {
        let lo = (self.rival_w - 2.0 * self.rival_sd_w).max(self.rival_w * 0.5);
        let hi = (self.rival_w + 2.0 * self.rival_sd_w).min(self.rival_w * 1.6);
        (lo, hi.max(lo))
    }

    /// About how long the whole session takes.
    pub fn total_s(&self) -> f32 {
        let n = self.count.max(1) as f32;
        self.warmup_s + n * self.mean_s + (n - 1.0) * self.rest_s + self.cooldown_s
    }

    /// What sprints this long train.
    pub fn focus(&self) -> &'static str {
        match self.mean_s {
            s if s <= 8.0 => t(
                "Explosiviteit: de eerste meters en je topvermogen",
                "Explosiveness: the jump and your peak power",
            ),
            s if s <= 16.0 => t(
                "Sprintvermogen: topvermogen vasthouden tot de streep",
                "Sprint power: holding your peak to the line",
            ),
            s if s <= 30.0 => t(
                "Lange sprint: vermogen dat je 20–30 s volhoudt",
                "Long sprint: power you hold for 20–30 s",
            ),
            _ => t(
                "Anaerobe capaciteit: lang en diep gaan",
                "Anaerobic capacity: going long and deep",
            ),
        }
    }

    /// Shortest rest that keeps every sprint a full one.
    pub fn advised_rest_s(&self) -> f32 {
        (self.mean_s * 15.0).clamp(180.0, 360.0)
    }

    /// The sprints of one session.
    pub fn draw(&self, seed: u64) -> Vec<SprintSpec> {
        let mut rng = Rng::new(seed);
        let (lo, hi) = self.duration_range();
        let (wlo, whi) = self.watts_range();
        (0..self.count)
            .map(|_| SprintSpec {
                secs: rng.normal(self.mean_s, self.sd_s).clamp(lo, hi),
                rivals: (0..self.rivals)
                    .map(|_| RivalSprint {
                        watts: rng.normal(self.rival_w, self.rival_sd_w).clamp(wlo, whi),
                        // Most go at the mark, give or take; now and then
                        // someone goes early and you have to react.
                        jump_s: if rng.chance(0.15) {
                            rng.range(-3.0, -1.5)
                        } else {
                            rng.normal(0.4, 0.8).clamp(-1.5, 2.5)
                        },
                    })
                    .collect(),
            })
            .collect()
    }
}

/// One sprint: how long, and how each rival rides it.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintSpec {
    pub secs: f32,
    pub rivals: Vec<RivalSprint>,
}

impl SprintSpec {
    pub fn mean_w(&self) -> f32 {
        let n = self.rivals.len().max(1) as f32;
        self.rivals.iter().map(|r| r.watts).sum::<f32>() / n
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RivalSprint {
    /// Average power from the jump over the sprint's duration.
    pub watts: f32,
    /// When the rival jumps, seconds after you pass the mark (negative:
    /// before you get there).
    pub jump_s: f32,
}

/// A rival's power `tau` seconds after jumping into a sprint of `secs`:
/// a quick rise to its peak, then fading. The shape is scaled so that it
/// averages `watts` over `secs`; past that it holds the fade's end.
pub fn sprint_power(watts: f32, secs: f32, tau: f32) -> f32 {
    let secs = secs.max(RISE_S + 0.5);
    let area = RISE_S * TOP * (JUMP + 1.0) / 2.0 + (secs - RISE_S) * (TOP + END) / 2.0;
    let k = secs / area;
    let shape = if tau < RISE_S {
        TOP * (JUMP + (1.0 - JUMP) * tau.max(0.0) / RISE_S)
    } else if tau < secs {
        TOP + (END - TOP) * (tau - RISE_S) / (secs - RISE_S)
    } else {
        END
    };
    watts * k * shape
}

/// Meters covered from `v0` in `secs` by a rider on `bike` sprinting at
/// `watts` (as `sprint_power` rides it), with `grade_at` giving the road.
pub fn sprint_distance(
    bike: &BikeParams,
    v0: f32,
    watts: f32,
    secs: f32,
    grade_at: impl Fn(f64) -> f32,
) -> f64 {
    let mut s = RiderState {
        speed: v0,
        distance: 0.0,
    };
    let h = 0.05;
    let mut tau = 0.0;
    while tau < secs {
        s.step(
            bike,
            sprint_power(watts, secs, tau),
            grade_at(s.distance),
            0.0,
            h,
        );
        tau += h;
    }
    s.distance
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Warmup,
    Rest,
    /// Counting down to the start mark.
    Approach,
    Sprint,
    CoolDown,
    Done,
}

/// What happened this update, for sounds and messages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The countdown to sprint `k` (from 0) begins.
    Approach(usize),
    /// You passed the start mark.
    Go,
    /// A rival (rider index) jumped.
    Jump(usize),
    /// You crossed the line of sprint `k`; its result is in `results`.
    Crossed(usize),
    CoolDown,
    Done,
    /// Everyone was put back down the road.
    Rewound,
}

/// How you rode one sprint.
#[derive(Debug, Clone, PartialEq)]
pub struct SprintResult {
    /// The drawn duration.
    pub secs: f32,
    /// Your effort, from your jump to the line.
    pub ridden_s: f32,
    pub avg_w: f32,
    pub peak_w: f32,
    pub best5_w: f32,
    /// From your jump to your peak.
    pub to_peak_s: f32,
    pub top_kmh: f32,
    /// 1 is a win; `field` is you and the rivals.
    pub place: usize,
    pub field: usize,
    /// Won by (positive) or lost to the winner by (negative), seconds.
    pub margin_s: f32,
    /// Per rival: rider index, sprint watts, and seconds behind you
    /// (negative: ahead of you) at the line.
    pub rivals: Vec<(usize, f32, f32)>,
}

impl SprintResult {
    pub fn won(&self) -> bool {
        self.place == 1
    }
}

/// A sprint session being ridden: drives the rivals in `race` and times
/// and scores your sprints.
#[derive(Debug, Clone)]
pub struct Drill {
    pub settings: SprintSettings,
    pub plan: Vec<SprintSpec>,
    pub ftp: f32,
    pub phase: Phase,
    /// The sprint being approached or ridden, or the next one.
    pub current: usize,
    /// Seconds into the session.
    pub clock: f32,
    /// When the warm-up, the rest or the cool-down ends.
    pub phase_until: f32,
    pub start_m: f64,
    pub line_m: f64,
    /// When you passed the start mark.
    go_at: Option<f32>,
    /// Rider indices of the rivals, in plan order.
    pub rivals: Vec<usize>,
    /// Per rival: when they jumped and when they crossed this sprint's line.
    jumped: Vec<Option<f32>>,
    crossed: Vec<Option<f32>>,
    /// Per rival: meters ahead of you (negative: behind) between sprints.
    slots: Vec<f32>,
    /// You this sprint: (clock, power, speed m/s).
    trace: Vec<(f32, f32, f32)>,
    pub results: Vec<SprintResult>,
    /// Your power, one value per second of the session.
    pub watts: Vec<f32>,
}

impl Drill {
    pub fn new(settings: SprintSettings, ftp: f32, seed: u64) -> Self {
        let plan = settings.draw(seed);
        Self {
            phase_until: settings.warmup_s,
            settings,
            plan,
            ftp,
            phase: Phase::Warmup,
            current: 0,
            clock: 0.0,
            start_m: 0.0,
            line_m: 0.0,
            go_at: None,
            rivals: Vec::new(),
            jumped: Vec::new(),
            crossed: Vec::new(),
            slots: Vec::new(),
            trace: Vec::new(),
            results: Vec::new(),
            watts: Vec::new(),
        }
    }

    /// Takes over the bots of `race` as rivals: on your bike and your
    /// weight, so watts compare directly, and under this drill's control.
    pub fn take_over(&mut self, race: &mut Race) {
        let me = race.human;
        let (bike, kg, height) = {
            let p = race.player();
            (p.bike, p.profile.kg, p.profile.height_cm)
        };
        self.rivals = (0..race.riders.len()).filter(|&i| i != me).collect();
        for &i in &self.rivals {
            let r = &mut race.riders[i];
            r.bike = bike;
            r.profile.kg = kg;
            r.profile.height_cm = height;
            r.intent = Intent::Sit;
        }
        // Alternately ahead and behind, closer in first.
        self.slots = (0..self.rivals.len())
            .map(|k| {
                let row = (k / 2 + 1) as f32 * SLOT_M;
                if k % 2 == 0 { row } else { -row }
            })
            .collect();
        self.jumped = vec![None; self.rivals.len()];
        self.crossed = vec![None; self.rivals.len()];
        race.script = Some(vec![0.0; race.riders.len()]);
    }

    pub fn spec(&self) -> Option<&SprintSpec> {
        self.plan.get(self.current)
    }

    /// Seconds until you reach the start mark (counting down in the
    /// approach), then negative and counting since you passed it.
    pub fn time_to_go(&self, race: &Race) -> Option<f32> {
        let me = race.player();
        match self.phase {
            Phase::Approach => Some((self.start_m - me.dist) as f32 / me.speed.max(3.0)),
            Phase::Sprint => Some(-(self.clock - self.go_at.unwrap_or(self.clock))),
            _ => None,
        }
    }

    /// Meters to the line (in the approach and the sprint).
    pub fn to_line_m(&self, race: &Race) -> Option<f64> {
        matches!(self.phase, Phase::Approach | Phase::Sprint)
            .then(|| self.line_m - race.player().dist)
    }

    /// Your place right now among the riders in the sprint (1 = leading).
    pub fn live_place(&self, race: &Race) -> usize {
        let me = race.player().dist;
        1 + self
            .rivals
            .iter()
            .enumerate()
            .filter(|&(k, &i)| self.crossed[k].is_some() || race.riders[i].dist > me)
            .count()
    }

    /// Whether this rival has gone.
    pub fn has_jumped(&self, k: usize) -> bool {
        self.jumped.get(k).is_some_and(Option::is_some)
    }

    /// Seconds left of the warm-up, rest or cool-down.
    pub fn phase_left(&self) -> f32 {
        (self.phase_until - self.clock).max(0.0)
    }

    pub fn wins(&self) -> usize {
        self.results.iter().filter(|r| r.won()).count()
    }

    /// Steps the drill by `dt` with your `power`, after the race stepped,
    /// and sets what the rivals ride next.
    pub fn update(&mut self, race: &mut Race, dt: f32, power: f32) -> Vec<Event> {
        let mut events = Vec::new();
        if self.phase == Phase::Done {
            self.ride_rivals(race);
            return events;
        }
        self.clock += dt;
        if self.watts.len() < self.clock as usize {
            self.watts.push(power);
        }
        if matches!(self.phase, Phase::Approach | Phase::Sprint) {
            let me = race.player();
            self.trace.push((self.clock, power, me.speed));
        }
        match self.phase {
            Phase::Warmup | Phase::Rest => {
                self.keep_road(race, ROOM_M, &mut events);
                if self.clock >= self.phase_until {
                    self.approach(race, &mut events);
                }
            }
            Phase::Approach => {
                if race.player().dist >= self.start_m {
                    self.phase = Phase::Sprint;
                    self.go_at = Some(self.clock);
                    events.push(Event::Go);
                }
            }
            Phase::Sprint => {
                let me = race.player();
                let given_up = self.go_at.is_some_and(|g| self.clock - g > GIVE_UP_S);
                if me.dist >= self.line_m || given_up {
                    let over = (me.dist - self.line_m).max(0.0) as f32 / me.speed.max(1.0);
                    self.score(race, self.clock - over);
                    events.push(Event::Crossed(self.current));
                    self.current += 1;
                    if self.current >= self.plan.len() {
                        self.phase = Phase::CoolDown;
                        self.phase_until = self.clock + self.settings.cooldown_s;
                        events.push(Event::CoolDown);
                    } else {
                        self.phase = Phase::Rest;
                        self.phase_until =
                            self.clock + (self.settings.rest_s - APPROACH_S).max(20.0);
                    }
                }
            }
            Phase::CoolDown => {
                self.keep_road(race, ROOM_M, &mut events);
                if self.clock >= self.phase_until {
                    self.phase = Phase::Done;
                    events.push(Event::Done);
                }
            }
            Phase::Done => {}
        }
        self.jumps(race, &mut events);
        self.ride_rivals(race);
        events
    }

    /// Ends the session now (stopped early).
    pub fn stop(&mut self) {
        self.phase = Phase::Done;
    }

    /// Sets the start mark and the line of the next sprint.
    fn approach(&mut self, race: &mut Race, events: &mut Vec<Event>) {
        let Some(spec) = self.plan.get(self.current).cloned() else {
            return;
        };
        let me = race.player();
        let v0 = me.speed.max(6.0);
        let lead = (v0 * APPROACH_S) as f64;
        let course = &race.course;
        let sprint = sprint_distance(&me.bike, v0, spec.mean_w(), spec.secs, |d| {
            course.grade_pct_at(me.dist + lead + d) / 100.0
        });
        self.keep_road(race, lead + sprint + ROOM_M, events);
        let me = race.player();
        self.start_m = me.dist + lead;
        self.line_m = self.start_m + sprint;
        self.go_at = None;
        self.jumped.iter_mut().for_each(|j| *j = None);
        self.crossed.iter_mut().for_each(|c| *c = None);
        self.trace.clear();
        self.phase = Phase::Approach;
        events.push(Event::Approach(self.current));
    }

    /// Puts everyone back down the road when fewer than `need` meters are
    /// left: on a loop by whole laps, so nothing seems to move.
    fn keep_road(&mut self, race: &mut Race, need: f64, events: &mut Vec<Event>) {
        let course = &race.course;
        let d = race.player().dist;
        if d + need < course.total_m {
            return;
        }
        let lap = course.route.length_m();
        let by_laps = course.route.looped && course.laps > 1;
        let back = if by_laps {
            ((d - 100.0) / lap).floor().max(1.0) * lap
        } else {
            d - 100.0
        };
        if back <= 0.0 {
            return;
        }
        race.rewind(back);
        if !by_laps {
            events.push(Event::Rewound);
        }
    }

    fn jumps(&mut self, race: &Race, events: &mut Vec<Event>) {
        let Some(ttg) = self.time_to_go(race) else {
            return;
        };
        let Some(spec) = self.plan.get(self.current) else {
            return;
        };
        for (k, rival) in spec.rivals.iter().enumerate() {
            if self.jumped[k].is_none() && ttg <= -rival.jump_s {
                self.jumped[k] = Some(self.clock);
                events.push(Event::Jump(self.rivals[k]));
            }
        }
        for (k, &i) in self.rivals.iter().enumerate() {
            let r = &race.riders[i];
            if self.crossed[k].is_none() && r.dist >= self.line_m {
                let over = (r.dist - self.line_m) as f32 / r.speed.max(1.0);
                self.crossed[k] = Some(self.clock - over);
            }
        }
    }

    /// Sets each rival's power and intent for the next step.
    fn ride_rivals(&mut self, race: &mut Race) {
        let me = race.player().clone();
        let sprinting = matches!(self.phase, Phase::Approach | Phase::Sprint);
        let spec = self.plan.get(self.current).cloned();
        let mut powers = race.script.take().unwrap_or_default();
        powers.resize(race.riders.len(), 0.0);
        for (k, &i) in self.rivals.iter().enumerate() {
            let grade = race.course.grade_pct_at(race.riders[i].dist) / 100.0;
            let r = &mut race.riders[i];
            let jump = sprinting.then(|| self.jumped[k]).flatten();
            let racing = jump.is_some() && self.crossed[k].is_none();
            powers[i] = match (jump, &spec) {
                (Some(at), Some(spec)) if racing => {
                    r.intent = Intent::Sprint {
                        until_m: self.line_m,
                    };
                    sprint_power(spec.rivals[k].watts, spec.secs, self.clock - at)
                }
                _ => {
                    r.intent = Intent::Sit;
                    let gap = (me.dist + self.slots[k] as f64 - r.dist) as f32;
                    let v = (me.speed + (gap * 0.12).clamp(-2.5, 2.0)).max(0.0);
                    if me.speed < 0.5 && gap.abs() < 3.0 {
                        0.0
                    } else {
                        required_power(r, v, grade).min(REGROUP_MAX_W)
                    }
                }
            };
        }
        race.script = Some(powers);
    }

    /// Scores the sprint you just finished at `line_at` (clock).
    fn score(&mut self, race: &Race, line_at: f32) {
        let Some(spec) = self.plan.get(self.current) else {
            return;
        };
        let go = self.go_at.unwrap_or(line_at);
        // Your jump: back from the mark for as long as you were already
        // flat out.
        let hard = (1.5 * self.ftp).max(0.55 * spec.mean_w());
        let mut start = go;
        for &(t, p, _) in self.trace.iter().rev().filter(|s| s.0 <= go) {
            if p < hard || go - t > 6.0 {
                break;
            }
            start = t;
        }
        // Your power every 0.1 s from the jump to the line.
        let mut series = Vec::new();
        let mut j = 0;
        let mut t = start;
        let mut top = 0.0f32;
        while t <= line_at {
            while j + 1 < self.trace.len() && self.trace[j + 1].0 <= t {
                j += 1;
            }
            if let Some(&(_, p, v)) = self.trace.get(j) {
                series.push(p);
                top = top.max(v);
            }
            t += 0.1;
        }
        let n = series.len().max(1) as f32;
        let (peak_k, peak_w) = series
            .iter()
            .enumerate()
            .fold((0, 0.0f32), |a, (k, &p)| if p > a.1 { (k, p) } else { a });
        let best5_w = if series.len() >= 50 {
            best_average(&series, 50)
        } else {
            series.iter().sum::<f32>() / n
        };
        // Where every rival was when you crossed.
        let rivals: Vec<(usize, f32, f32)> = self
            .rivals
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                let behind = match self.crossed[k] {
                    Some(c) => c - line_at,
                    None => {
                        let r = &race.riders[i];
                        (self.line_m - r.dist).max(0.0) as f32 / r.speed.max(1.0)
                    }
                };
                (i, spec.rivals[k].watts, behind)
            })
            .collect();
        let ahead = rivals.iter().filter(|r| r.2 < 0.0).count();
        let margin_s = if ahead == 0 {
            rivals
                .iter()
                .map(|r| r.2)
                .fold(f32::MAX, f32::min)
                .min(99.0)
        } else {
            rivals.iter().map(|r| r.2).fold(0.0f32, f32::min)
        };
        self.results.push(SprintResult {
            secs: spec.secs,
            ridden_s: line_at - start,
            avg_w: series.iter().sum::<f32>() / n,
            peak_w,
            best5_w,
            to_peak_s: peak_k as f32 * 0.1,
            top_kmh: top * 3.6,
            place: ahead + 1,
            field: self.rivals.len() + 1,
            margin_s,
            rivals,
        });
    }

    /// Your best powers of the session: peak, then 5, 10, 15 and 30 s.
    pub fn bests(&self) -> Bests {
        Bests {
            peak_w: self.results.iter().map(|r| r.peak_w).fold(0.0, f32::max),
            w5: best_average(&self.watts, 5),
            w10: best_average(&self.watts, 10),
            w15: best_average(&self.watts, 15),
            w30: best_average(&self.watts, 30),
        }
    }

    /// What you'd ride in test mode (keyboard): easy between sprints, and
    /// `sprint_w` from a second before the mark to the line.
    pub fn test_power(&self, race: &Race, sprint_w: f32) -> f32 {
        match (self.phase, self.time_to_go(race)) {
            (Phase::Approach, Some(t)) if t < 1.0 => sprint_w,
            (Phase::Approach, _) => 0.75 * self.ftp,
            (Phase::Sprint, _) => sprint_w,
            _ => 0.6 * self.ftp,
        }
    }
}

/// Best powers over a session, or over all of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bests {
    pub peak_w: f32,
    pub w5: f32,
    pub w10: f32,
    pub w15: f32,
    pub w30: f32,
}

impl Bests {
    pub fn max(self, o: Bests) -> Bests {
        Bests {
            peak_w: self.peak_w.max(o.peak_w),
            w5: self.w5.max(o.w5),
            w10: self.w10.max(o.w10),
            w15: self.w15.max(o.w15),
            w30: self.w30.max(o.w30),
        }
    }

    /// (label, this, before) for each value; `before` 0 means no record yet.
    pub fn rows(self, before: Bests) -> [(&'static str, f32, f32); 5] {
        [
            ("Peak", self.peak_w, before.peak_w),
            ("5 s", self.w5, before.w5),
            ("10 s", self.w10, before.w10),
            ("15 s", self.w15, before.w15),
            ("30 s", self.w30, before.w30),
        ]
    }
}

/// One sprint session in the profile's log.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SprintLog {
    pub unix_time: u64,
    pub sprints: u32,
    pub wins: u32,
    pub bests: Bests,
    /// Your best average over a whole sprint, and how long it was.
    pub best_avg_w: f32,
    pub best_avg_s: f32,
}

/// Your records over every logged session.
pub fn records(log: &[SprintLog]) -> Bests {
    log.iter().fold(Bests::default(), |a, l| a.max(l.bests))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::field::{RiderKind, rival_teams};
    use crate::race::{PlayerSetup, RaceConfig};
    use crate::route::Route;

    fn race(rivals: usize) -> Race {
        let mut teams = rival_teams(rivals + 1, 3.5, 1.0);
        teams.truncate(rivals + 1);
        let mut race = Race::new(&RaceConfig {
            seed: 3,
            route: Route::builtin()
                .into_iter()
                .find(|r| r.id == "flat-loop")
                .unwrap(),
            laps: 2,
            teams,
            riders_per_team: 1,
            player_team: 0,
            player: PlayerSetup {
                name: "Me".into(),
                kg: 75.0,
                height_cm: 182.0,
                ftp: 250.0,
                kind: RiderKind::Sprinter,
            },
            countdown_s: 0.0,
        });
        race.time = 0.0;
        race
    }

    fn quick(rival_w: f32) -> SprintSettings {
        SprintSettings {
            count: 3,
            warmup_s: 40.0,
            rest_s: 60.0,
            rival_w,
            rival_sd_w: 30.0,
            ..SprintSettings::default()
        }
    }

    /// Rides a whole session with the test-mode power; returns the drill.
    fn ride(settings: SprintSettings, sprint_w: f32) -> Drill {
        let mut race = race(settings.rivals as usize);
        let mut drill = Drill::new(settings, 250.0, 9);
        drill.take_over(&mut race);
        let dt = 1.0 / 30.0;
        let mut power = 150.0;
        while drill.phase != Phase::Done && drill.clock < 3600.0 {
            race.step(dt, power, 90.0);
            drill.update(&mut race, dt, power);
            power = drill.test_power(&race, sprint_w);
        }
        drill
    }

    #[test]
    fn durations_and_watts_follow_the_settings() {
        let s = SprintSettings {
            count: 400,
            ..SprintSettings::default()
        };
        let plan = s.draw(1);
        let secs: Vec<f32> = plan.iter().map(|p| p.secs).collect();
        let mean = secs.iter().sum::<f32>() / secs.len() as f32;
        assert!((mean - 12.0).abs() < 0.5, "mean {mean}");
        let (lo, hi) = s.duration_range();
        assert!(secs.iter().all(|&x| (lo..=hi).contains(&x)));
        assert!(secs.iter().any(|&x| x < 10.0) && secs.iter().any(|&x| x > 14.0));
        let w: Vec<f32> = plan
            .iter()
            .flat_map(|p| p.rivals.iter().map(|r| r.watts))
            .collect();
        let wm = w.iter().sum::<f32>() / w.len() as f32;
        assert!((wm - 750.0).abs() < 10.0, "rival mean {wm}");
        // A tight setting can't draw a sprint shorter than the minimum.
        let short = SprintSettings {
            mean_s: 5.0,
            sd_s: 4.0,
            ..s
        };
        assert!(short.draw(2).iter().all(|p| p.secs >= MIN_S));
    }

    #[test]
    fn a_rival_averages_its_watts_over_the_sprint() {
        for secs in [5.0, 12.0, 30.0] {
            let n = 10_000;
            let avg = (0..n)
                .map(|k| sprint_power(800.0, secs, secs * (k as f32 + 0.5) / n as f32))
                .sum::<f32>()
                / n as f32;
            assert!((avg - 800.0).abs() < 2.0, "{secs} s: {avg}");
            assert!(sprint_power(800.0, secs, 2.0) > 850.0);
        }
    }

    #[test]
    fn the_line_is_where_an_average_rival_gets_in_the_drawn_time() {
        let bike = BikeParams::default();
        let d = sprint_distance(&bike, 10.0, 800.0, 12.0, |_| 0.0);
        // About 55 km/h at the end from 36 km/h: some 160–200 m.
        assert!((150.0..210.0).contains(&d), "{d} m");
        assert!(sprint_distance(&bike, 10.0, 800.0, 12.0, |_| 0.04) < d);
    }

    #[test]
    fn a_session_is_ridden_through_and_scored() {
        let drill = ride(quick(750.0), 900.0);
        assert_eq!(drill.results.len(), 3);
        for r in &drill.results {
            // Your effort lasts about as long as the sprint was drawn.
            assert!(
                (r.ridden_s - r.secs).abs() < 0.35 * r.secs + 1.5,
                "ridden {} of {}",
                r.ridden_s,
                r.secs
            );
            assert!((r.avg_w - 900.0).abs() < 20.0, "avg {}", r.avg_w);
            assert!(r.top_kmh > 45.0, "top {}", r.top_kmh);
            assert_eq!(r.field, 6);
        }
        // Faster than every rival, you win most of them.
        assert!(drill.wins() >= 2, "wins {}", drill.wins());
        assert!(drill.bests().w5 >= 880.0);
    }

    #[test]
    fn rivals_beat_you_when_you_ride_less_than_them() {
        let drill = ride(quick(750.0), 560.0);
        assert_eq!(drill.results.len(), 3);
        assert_eq!(drill.wins(), 0);
        assert!(
            drill
                .results
                .iter()
                .all(|r| r.place >= 4 && r.margin_s < 0.0)
        );
    }

    #[test]
    fn rivals_ride_round_you_between_sprints() {
        let mut race = race(5);
        let settings = SprintSettings {
            warmup_s: 300.0,
            ..quick(750.0)
        };
        let mut drill = Drill::new(settings, 250.0, 4);
        drill.take_over(&mut race);
        let dt = 0.1;
        for _ in 0..600 {
            race.step(dt, 180.0, 90.0);
            drill.update(&mut race, dt, 180.0);
        }
        assert_eq!(drill.phase, Phase::Warmup);
        let me = race.player().dist;
        for &i in &drill.rivals {
            let gap = (race.riders[i].dist - me).abs();
            assert!(gap < 12.0, "rival {i} is {gap} m away");
        }
    }

    #[test]
    fn a_short_road_starts_over_before_it_runs_out() {
        let mut race = race(3);
        let total = race.course.total_m;
        for r in &mut race.riders {
            r.dist += total - 300.0;
        }
        let mut drill = Drill::new(quick(700.0), 250.0, 2);
        drill.take_over(&mut race);
        race.step(0.1, 200.0, 90.0);
        drill.update(&mut race, 0.1, 200.0);
        assert!(race.player().dist < total - 1000.0);
        // Everyone moved together.
        let me = race.player().dist;
        assert!(race.riders.iter().all(|r| (r.dist - me).abs() < 20.0));
    }
}
