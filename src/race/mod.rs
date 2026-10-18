//! Single-player road race: the player and AI riders share one physics
//! model, drafting, W′ energy and team tactics.
//!
//! `Race::step` is deterministic for a given seed and inputs, so the renderer
//! can run it every frame and tests can run whole races headless.

pub mod ai;
pub mod challenges;
pub mod course;
pub mod energy;
pub mod field;
pub mod moments;
pub mod pack;

use crate::i18n::t;
use crate::physics::{BikeParams, RiderState};
use crate::rng::Rng;
use crate::route::Route;
use crate::tf;

use ai::{Intent, Plan};
use course::{Course, SEGMENT_POINTS};
use energy::Energy;
use field::{Profile, RiderKind, TeamSpec};

/// Riders keep to ±this many meters from the road center.
pub const ROAD_HALF_WIDTH: f32 = 3.6;
/// Simulation substep; small enough for stable pack interactions.
const MAX_SUBSTEP: f32 = 0.05;
/// How often riders and team directors re-plan.
const AI_INTERVAL: f32 = 0.5;
/// Riders at the front of a group look this far up the road for a gap to close.
const BRIDGE_REACH_M: f32 = 250.0;
/// Riders crossing the line within this many seconds share a finish time.
const SAME_TIME_GAP_S: f32 = 1.0;

#[derive(Debug, Clone)]
pub struct Rider {
    pub id: usize,
    pub team: usize,
    pub profile: Profile,
    pub bike: BikeParams,
    pub energy: Energy,
    pub human: bool,
    /// Race distance covered in meters.
    pub dist: f64,
    /// Meters right of the road center.
    pub lateral: f32,
    pub speed: f32,
    /// Change of speed over the last substep, m/s².
    pub accel: f32,
    pub power: f32,
    pub cadence: f32,
    /// Current drag reduction from drafting, 0..1.
    pub draft: f32,
    pub intent: Intent,
    pub finish_time: Option<f32>,
    /// Seconds spent leading this group (drives rotation).
    pub front_time: f32,
    /// Most power this rider can hold to the top of the current climb; above
    /// it they let the wheel go and ride their own tempo.
    pub limit_w: f32,
    /// Wants to move up in the bunch (finale positioning); such riders come
    /// up the side of the bunch instead of being boxed in.
    pub move_up: bool,
}

impl Rider {
    pub fn finished(&self) -> bool {
        self.finish_time.is_some()
    }
}

/// Orders the player can give their teammates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamOrder {
    /// Teammates ride their own race in the bunch.
    Free,
    /// Two teammates set a steady pace on the front of your group.
    Tempo,
    /// All teammates in your group chase hard.
    Chase,
    /// Teammates form a lead-out train in front of you.
    LeadOut,
    /// Teammates shelter you and wait for you if you are dropped.
    Protect,
}

impl TeamOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Free => t("Vrij rijden", "Ride free"),
            Self::Tempo => t("Tempo rijden", "Set tempo"),
            Self::Chase => t("Achtervolgen", "Chase"),
            Self::LeadOut => "Lead-out",
            Self::Protect => t("Bescherm mij", "Protect me"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Team {
    pub spec: TeamSpec,
    pub plan: Plan,
    /// Rider index of the team leader.
    pub leader: usize,
    pub human: bool,
    pub order: TeamOrder,
}

/// Riders within about a second of each other.
#[derive(Debug, Clone)]
pub struct Group {
    /// Rider indices, front to back.
    pub riders: Vec<usize>,
    /// Seconds behind the first group.
    pub gap_s: f32,
    /// Power (W/kg) the riders leading this group ride at; speed then
    /// follows the terrain, as in a real bunch.
    pub tempo_wkg: f32,
}

#[derive(Debug, Clone)]
pub struct RaceEvent {
    pub time: f32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct PlayerSetup {
    pub name: String,
    pub kg: f32,
    pub height_cm: f32,
    pub ftp: f32,
    /// The role you take in your team; you replace that rider.
    pub kind: RiderKind,
}

#[derive(Debug, Clone)]
pub struct RaceConfig {
    pub seed: u64,
    pub route: Route,
    pub laps: u32,
    pub teams: Vec<TeamSpec>,
    pub riders_per_team: usize,
    /// Index into `teams` of the team you ride for.
    pub player_team: usize,
    pub player: PlayerSetup,
    /// Seconds of countdown before the start.
    pub countdown_s: f32,
}

#[derive(Debug, Clone)]
pub struct FinishLine {
    pub rider: usize,
    pub time: f32,
    /// Seconds behind the winner, after same-time grouping.
    pub gap_s: f32,
}

pub struct Race {
    pub course: Course,
    pub riders: Vec<Rider>,
    pub teams: Vec<Team>,
    pub groups: Vec<Group>,
    /// Race clock in seconds; negative during the countdown.
    pub time: f32,
    pub events: Vec<RaceEvent>,
    /// Per course segment: rider indices in crossing order.
    pub segment_results: Vec<Vec<usize>>,
    pub finish: Vec<FinishLine>,
    pub human: usize,
    rng: Rng,
    ai_timer: f32,
    pub(crate) director_timer: f32,
    /// Whether a breakaway was up the road at the last AI tick.
    pub(crate) break_up: bool,
    /// Race time when the last breakaway was caught (counter-attack window).
    pub(crate) last_catch: f32,
    /// Race time of the last spontaneous attack.
    pub(crate) last_attack: f32,
    /// Rivals marking the player's last attack stay on it until then.
    pub(crate) player_marked_until: f32,
}

/// Spread of a rider's threshold on the day, as a share of it.
pub const DAY_FORM_SD: f32 = 0.025;

impl Race {
    pub fn new(cfg: &RaceConfig) -> Self {
        let mut rng = Rng::new(cfg.seed);
        let course = Course::new(cfg.route.clone(), cfg.laps);
        let mut riders = Vec::new();
        let mut teams = Vec::new();
        let mut human = 0;

        for (t, spec) in cfg.teams.iter().enumerate() {
            let mut profiles = field::sample_team(&mut rng, spec, cfg.riders_per_team);
            let is_player_team = t == cfg.player_team;
            let mut player_slot = None;
            if is_player_team {
                let slot = profiles
                    .iter()
                    .position(|p| p.kind == cfg.player.kind)
                    .unwrap_or(0);
                let p = &cfg.player;
                profiles[slot] = Profile {
                    name: p.name.clone(),
                    kind: p.kind,
                    kg: p.kg,
                    height_cm: p.height_cm,
                    ftp: p.ftp,
                    peak: p.kg * 15.0,
                    w_prime: 20_000.0,
                    aggression: 0.0,
                };
                player_slot = Some(slot);
            }
            let first = riders.len();
            for (k, mut profile) in profiles.into_iter().enumerate() {
                let is_human = player_slot == Some(k);
                if !is_human {
                    // Nobody is the same rider every day: a few percent
                    // either way, which the team knows when it picks its
                    // leader.
                    profile.ftp *= rng.normal(1.0, DAY_FORM_SD).clamp(0.93, 1.07);
                }
                if is_human {
                    human = riders.len();
                }
                riders.push(Rider {
                    id: riders.len(),
                    team: t,
                    bike: profile.bike(),
                    energy: profile.energy(),
                    profile,
                    human: is_human,
                    dist: 0.0,
                    lateral: 0.0,
                    speed: 0.0,
                    accel: 0.0,
                    power: 0.0,
                    cadence: 0.0,
                    draft: 0.0,
                    intent: Intent::Sit,
                    finish_time: None,
                    front_time: 0.0,
                    limit_w: f32::MAX,
                    move_up: false,
                });
            }
            let members = first..riders.len();
            let (plan, leader) = if let Some(slot) = player_slot {
                (Plan::for_kind(cfg.player.kind), first + slot)
            } else {
                ai::choose_plan(&mut rng, &riders[members.clone()], &course, first)
            };
            teams.push(Team {
                spec: spec.clone(),
                plan,
                leader,
                human: is_player_team,
                order: TeamOrder::Free,
            });
        }

        place_on_grid(&mut riders, &mut rng);
        let segment_results = vec![Vec::new(); course.segments.len()];
        let mut race = Self {
            course,
            riders,
            teams,
            groups: Vec::new(),
            time: -cfg.countdown_s,
            events: Vec::new(),
            segment_results,
            finish: Vec::new(),
            human,
            rng,
            ai_timer: 0.0,
            director_timer: 0.0,
            break_up: false,
            last_catch: -1_000.0,
            last_attack: -1_000.0,
            player_marked_until: -1_000.0,
        };
        race.groups = pack::find_groups(&race.riders);
        race
    }

    pub fn player(&self) -> &Rider {
        &self.riders[self.human]
    }

    pub fn player_team(&self) -> &Team {
        &self.teams[self.riders[self.human].team]
    }

    pub fn set_order(&mut self, order: TeamOrder) {
        let t = self.riders[self.human].team;
        if self.teams[t].order != order {
            self.teams[t].order = order;
            self.log(tf!("Teamorder: {}", "Team order: {}", order.label()));
        }
    }

    pub fn started(&self) -> bool {
        self.time >= 0.0
    }

    /// Everyone has crossed the line, or the player finished long ago.
    pub fn over(&self) -> bool {
        let player_done = self.riders[self.human]
            .finish_time
            .is_some_and(|t| self.time - t > 180.0);
        self.riders.iter().all(Rider::finished) || player_done
    }

    pub fn group_of(&self, rider: usize) -> usize {
        self.groups
            .iter()
            .position(|g| g.riders.contains(&rider))
            .unwrap_or(0)
    }

    /// Index of the biggest group, i.e. the peloton.
    pub fn peloton(&self) -> usize {
        self.groups
            .iter()
            .enumerate()
            .max_by_key(|(i, g)| (g.riders.len(), usize::MAX - i))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    pub(crate) fn log(&mut self, text: String) {
        self.events.push(RaceEvent {
            time: self.time.max(0.0),
            text,
        });
    }

    pub(crate) fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }

    /// Advances the race by `dt` seconds with the player's measured power.
    pub fn step(&mut self, dt: f32, player_power: f32, player_cadence: f32) {
        if !self.started() {
            self.time = (self.time + dt).min(0.0);
            return;
        }
        let n = (dt / MAX_SUBSTEP).ceil().max(1.0) as usize;
        let h = dt / n as f32;
        for _ in 0..n {
            self.substep(h, player_power, player_cadence);
        }
    }

    fn substep(&mut self, h: f32, player_power: f32, player_cadence: f32) {
        self.time += h;
        self.ai_timer += h;
        self.director_timer += h;
        if self.ai_timer >= AI_INTERVAL {
            self.ai_timer = 0.0;
            self.groups = pack::find_groups(&self.riders);
            ai::decide(self);
        }

        pack::update_draft(&mut self.riders);
        let neighbors = pack::wheels(&self.riders, &self.groups);
        let ahead = pack::nearest_ahead(&self.riders, BRIDGE_REACH_M);
        for i in 0..self.riders.len() {
            let power = if self.riders[i].human {
                player_power.max(0.0)
            } else {
                ai::power(self, i, neighbors[i], ahead[i])
            };
            let grade = self.course.grade_pct_at(self.riders[i].dist) / 100.0;
            let r = &mut self.riders[i];
            r.power = power;
            if r.human {
                r.cadence = player_cadence;
            } else {
                r.cadence = ai::cadence(r, grade);
            }
            r.energy.step(power, h);
            let mut state = RiderState {
                speed: r.speed,
                distance: r.dist,
            };
            state.step(&r.bike, power, grade, r.draft, h);
            let prev = r.dist;
            r.accel = (state.speed - r.speed) / h;
            r.speed = state.speed;
            r.dist = state.distance;
            self.check_lines(i, prev);
        }
        pack::steer(self, &neighbors, h);
        pack::separate(&mut self.riders, h);
    }

    /// Records segment and finish crossings between `prev` and the current distance.
    fn check_lines(&mut self, i: usize, prev: f64) {
        let d = self.riders[i].dist;
        for (s, seg) in self.course.segments.iter().enumerate() {
            if prev < seg.end_m && d >= seg.end_m {
                self.segment_results[s].push(i);
                if self.segment_results[s].len() == 1 {
                    let name = self.riders[i].profile.name.clone();
                    let text = tf!("{} wint {}", "{} wins the {}", name, seg.name);
                    self.events.push(RaceEvent {
                        time: self.time,
                        text,
                    });
                }
            }
        }
        let total = self.course.total_m;
        if prev < total && d >= total && self.riders[i].finish_time.is_none() {
            // Interpolate the exact crossing time inside this substep.
            let speed = self.riders[i].speed.max(0.1);
            let t = self.time - ((d - total) as f32 / speed);
            self.riders[i].finish_time = Some(t);
            self.riders[i].intent = Intent::Finished;
            self.push_finish(i, t);
        }
    }

    fn push_finish(&mut self, rider: usize, time: f32) {
        if self.finish.is_empty() {
            let name = self.riders[rider].profile.name.clone();
            let team = self.teams[self.riders[rider].team].spec.name.clone();
            self.log(tf!(
                "🏆 {name} ({team}) wint de wedstrijd!",
                "🏆 {name} ({team}) wins the race!"
            ));
        }
        let at = self.finish.partition_point(|f| f.time <= time);
        self.finish.insert(
            at,
            FinishLine {
                rider,
                time,
                gap_s: 0.0,
            },
        );
        // Riders within a second of the one ahead get that rider's time.
        let winner = self.finish[0].time;
        for k in 1..self.finish.len() {
            let (prev_time, prev_gap) = (self.finish[k - 1].time, self.finish[k - 1].gap_s);
            let f = &mut self.finish[k];
            f.gap_s = if f.time - prev_time < SAME_TIME_GAP_S {
                prev_gap
            } else {
                f.time - winner
            };
        }
    }

    /// Points per rider index from all sprint and KOM segments.
    pub fn points(&self, kind: course::SegmentKind) -> Vec<(usize, u32)> {
        let mut pts: Vec<(usize, u32)> = Vec::new();
        for (seg, order) in self.course.segments.iter().zip(&self.segment_results) {
            if seg.kind != kind {
                continue;
            }
            for (&rider, &p) in order.iter().zip(SEGMENT_POINTS.iter()) {
                match pts.iter_mut().find(|(r, _)| *r == rider) {
                    Some(e) => e.1 += p,
                    None => pts.push((rider, p)),
                }
            }
        }
        pts.sort_by_key(|&(_, p)| std::cmp::Reverse(p));
        pts
    }

    /// Meters left to the finish for `rider`.
    pub fn remaining_m(&self, rider: usize) -> f64 {
        (self.course.total_m - self.riders[rider].dist).max(0.0)
    }
}

/// Start pens: rows of six, 2.5 meters apart, in shuffled order.
fn place_on_grid(riders: &mut [Rider], rng: &mut Rng) {
    let mut order: Vec<usize> = (0..riders.len()).collect();
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    for (slot, &i) in order.iter().enumerate() {
        let row = slot / 6;
        let col = slot % 6;
        riders[i].dist = -(row as f64) * 2.5;
        riders[i].lateral = -2.5 + col as f32 * 1.0;
    }
}

#[cfg(test)]
mod tests;
