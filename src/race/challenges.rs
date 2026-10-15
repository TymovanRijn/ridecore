//! Short challenges during the race, so there is always something to go for:
//! jump on a rival's attack, put in a hard effort, shelter, move up, take a
//! turn on the front. Completing them earns XP; a streak multiplies it.
//! Sprint and KOM points and the finish place add XP too.

use super::Race;
use super::ai::Intent;
use super::course::{SEGMENT_POINTS, SegmentKind};
use crate::rng::Rng;

/// Quiet time between challenges.
const GAP_MIN_S: f32 = 70.0;
const GAP_MAX_S: f32 = 130.0;
/// No new challenge this close to the finish (the finale is the challenge).
const FINALE_M: f64 = 3000.0;
/// A rival attacking within this distance of you can be followed.
const ATTACK_REACH_M: f64 = 40.0;
/// Within this distance of the attacker you are on their wheel.
const ON_WHEEL_M: f64 = 8.0;
/// After an attack challenge, the next one waits at least this long.
const ATTACK_COOLDOWN_S: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// Get on the wheel of this attacking rival and stay there.
    FollowAttack { rider: usize },
    /// Ride above `watts` for `secs` in total.
    Effort { watts: f32, secs: f32 },
    /// Spend `secs` in a good slipstream.
    Shelter { secs: f32 },
    /// Reach this place (or better) in your group.
    MoveUp { place: usize },
    /// Spend `secs` on the front of your group.
    Pull { secs: f32 },
    /// Spin at or above `rpm` for `secs`.
    Cadence { rpm: f32, secs: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    pub xp: u32,
    /// Seconds left to complete it.
    pub time_left: f32,
    /// Accumulated seconds (for timed goals).
    held: f32,
    /// 0..1 for the bar.
    pub progress: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started(Challenge),
    Completed {
        title: String,
        xp: u32,
        streak: u32,
    },
    Failed {
        title: String,
    },
    /// XP for points, places and other achievements outside challenges.
    Bonus {
        what: String,
        xp: u32,
    },
}

#[derive(Debug, Clone)]
pub struct ChallengeTracker {
    rng: Rng,
    pub active: Option<Challenge>,
    next_in: f32,
    pub xp: u32,
    pub streak: u32,
    pub completed: u32,
    pub attempted: u32,
    attacking: Vec<bool>,
    segments_seen: Vec<usize>,
    finish_paid: bool,
}

impl ChallengeTracker {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0xC4A1_1E46),
            active: None,
            next_in: 45.0,
            xp: 0,
            streak: 0,
            completed: 0,
            attempted: 0,
            attacking: Vec::new(),
            segments_seen: Vec::new(),
            finish_paid: false,
        }
    }

    /// Streak multiplier for the next completion: ×1, ×1.5, ×2.
    pub fn multiplier(&self) -> f32 {
        match self.streak {
            0 => 1.0,
            1 => 1.5,
            _ => 2.0,
        }
    }

    pub fn update(&mut self, race: &Race, dt: f32) -> Vec<Event> {
        let mut out = Vec::new();
        if !race.started() {
            return out;
        }
        self.pay_points(race, &mut out);
        let me = race.player();
        if me.finished() {
            self.active = None;
            return out;
        }
        let new_attack = self.new_attack(race);

        if let Some(mut c) = self.active.take() {
            c.time_left -= dt;
            match self.evaluate(race, &mut c, dt) {
                Some(true) => {
                    let xp = (c.xp as f32 * self.multiplier()).round() as u32;
                    self.xp += xp;
                    self.streak += 1;
                    self.completed += 1;
                    out.push(Event::Completed {
                        title: c.title.clone(),
                        xp,
                        streak: self.streak,
                    });
                    self.next_in = self.rng.range(GAP_MIN_S, GAP_MAX_S);
                }
                Some(false) => {
                    self.streak = 0;
                    out.push(Event::Failed {
                        title: c.title.clone(),
                    });
                    self.next_in = self.rng.range(GAP_MIN_S, GAP_MAX_S);
                }
                None => self.active = Some(c),
            }
        }

        // A rival's attack beats any other challenge, unless one is already
        // being chased.
        let following = matches!(
            self.active.as_ref().map(|c| c.kind),
            Some(Kind::FollowAttack { .. })
        );
        if let Some(rider) = new_attack
            && !following
            && race.remaining_m(race.human) > FINALE_M
        {
            self.active = None;
            let name = race.riders[rider].profile.name.clone();
            let c = Challenge {
                kind: Kind::FollowAttack { rider },
                title: "AANVAL! Spring mee".into(),
                detail: format!("{name} valt aan · zit 15 s in zijn wiel"),
                xp: 150,
                time_left: 40.0,
                held: 0.0,
                progress: 0.0,
            };
            self.attempted += 1;
            out.push(Event::Started(c.clone()));
            self.active = Some(c);
            self.next_in = self.next_in.max(ATTACK_COOLDOWN_S);
            return out;
        }

        if self.active.is_none() {
            self.next_in -= dt;
            if self.next_in <= 0.0 && race.remaining_m(race.human) > FINALE_M {
                if let Some(c) = self.pick(race) {
                    self.attempted += 1;
                    out.push(Event::Started(c.clone()));
                    self.active = Some(c);
                } else {
                    self.next_in = 10.0;
                }
            }
        }
        out
    }

    /// A rival near you who has just switched to attacking.
    fn new_attack(&mut self, race: &Race) -> Option<usize> {
        let me = race.player();
        self.attacking.resize(race.riders.len(), false);
        let mut found = None;
        for (i, r) in race.riders.iter().enumerate() {
            let now = matches!(r.intent, Intent::Attack { .. });
            if now
                && !self.attacking[i]
                && !r.human
                && r.team != me.team
                && (r.dist - me.dist).abs() < ATTACK_REACH_M
            {
                found = Some(i);
            }
            self.attacking[i] = now;
        }
        found
    }

    /// Some(true) when done, Some(false) when failed, None while running.
    fn evaluate(&self, race: &Race, c: &mut Challenge, dt: f32) -> Option<bool> {
        let me = race.player();
        let (done, target) = match c.kind {
            Kind::FollowAttack { rider } => {
                let a = &race.riders[rider];
                if (a.dist - me.dist) < ON_WHEEL_M {
                    c.held += dt;
                }
                (c.held >= 15.0, 15.0)
            }
            Kind::Effort { watts, secs } => {
                if me.power >= watts {
                    c.held += dt;
                }
                (c.held >= secs, secs)
            }
            Kind::Shelter { secs } => {
                if me.draft >= 0.25 {
                    c.held += dt;
                }
                (c.held >= secs, secs)
            }
            Kind::MoveUp { place } => {
                let now = race.place_in_group(race.human);
                (now <= place, 1.0)
            }
            Kind::Pull { secs } => {
                if race.place_in_group(race.human) == 1 {
                    c.held += dt;
                }
                (c.held >= secs, secs)
            }
            Kind::Cadence { rpm, secs } => {
                if me.cadence >= rpm {
                    c.held += dt;
                }
                (c.held >= secs, secs)
            }
        };
        c.progress = match c.kind {
            Kind::MoveUp { place } => {
                let now = race.place_in_group(race.human) as f32;
                (1.0 - (now - place as f32) / 20.0).clamp(0.0, 1.0)
            }
            _ => (c.held / target).clamp(0.0, 1.0),
        };
        if done {
            Some(true)
        } else if c.time_left <= 0.0 {
            Some(false)
        } else {
            None
        }
    }

    /// A challenge that fits the situation you're in.
    fn pick(&mut self, race: &Race) -> Option<Challenge> {
        let me = race.player();
        let ftp = me.profile.ftp;
        let group = race
            .groups
            .get(race.group_of(race.human))
            .map(|g| g.riders.len())
            .unwrap_or(1);
        let place = race.place_in_group(race.human);
        let mut options: Vec<Challenge> = Vec::new();
        let mk = |kind, title: &str, detail: String, xp, time_left| Challenge {
            kind,
            title: title.into(),
            detail,
            xp,
            time_left,
            held: 0.0,
            progress: 0.0,
        };
        let watts = (ftp * 1.2 / 10.0).round() * 10.0;
        options.push(mk(
            Kind::Effort { watts, secs: 15.0 },
            "Versnelling",
            format!("Trap 15 s boven {watts:.0} W"),
            80,
            30.0,
        ));
        options.push(mk(
            Kind::Cadence {
                rpm: 100.0,
                secs: 20.0,
            },
            "Souplesse",
            "Draai 20 s boven 100 rpm".into(),
            50,
            40.0,
        ));
        if group >= 6 {
            options.push(mk(
                Kind::Shelter { secs: 45.0 },
                "Spaar je benen",
                "Zit 45 s goed in het wiel".into(),
                60,
                75.0,
            ));
        }
        if group >= 12 && place > 8 {
            let target = (group / 4).clamp(3, 8);
            options.push(mk(
                Kind::MoveUp { place: target },
                "Schuif op",
                format!("Rij naar de top {target} van je groep"),
                90,
                60.0,
            ));
        }
        if (2..=12).contains(&group) || place <= 3 {
            options.push(mk(
                Kind::Pull { secs: 20.0 },
                "Kopwerk",
                "Trek 20 s op kop van je groep".into(),
                70,
                50.0,
            ));
        }
        let i = (self.rng.next_u64() % options.len() as u64) as usize;
        Some(options.swap_remove(i))
    }

    /// XP for sprint/KOM points and for the finish place.
    fn pay_points(&mut self, race: &Race, out: &mut Vec<Event>) {
        if self.segments_seen.len() != race.segment_results.len() {
            self.segments_seen = vec![0; race.segment_results.len()];
        }
        for (s, order) in race.segment_results.iter().enumerate() {
            for (k, &rider) in order.iter().enumerate().skip(self.segments_seen[s]) {
                if rider == race.human && k < SEGMENT_POINTS.len() {
                    let xp = SEGMENT_POINTS[k] * 15;
                    self.xp += xp;
                    out.push(Event::Bonus {
                        what: format!(
                            "{}e op de {}",
                            k + 1,
                            match race.course.segments[s].kind {
                                SegmentKind::Sprint => "tussensprint",
                                SegmentKind::Kom => "berg",
                            }
                        ),
                        xp,
                    });
                }
            }
            self.segments_seen[s] = order.len();
        }
        if !self.finish_paid
            && let Some(p) = race.finish.iter().position(|f| f.rider == race.human)
        {
            self.finish_paid = true;
            let field = race.riders.len();
            let xp =
                100 + (300 * (field - p - 1) / field.max(1)) as u32 + if p < 3 { 200 } else { 0 };
            self.xp += xp;
            out.push(Event::Bonus {
                what: format!("{}e plaats", p + 1),
                xp,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::field::{RiderKind, rival_teams};
    use crate::race::{PlayerSetup, RaceConfig};
    use crate::route::Route;

    fn race() -> Race {
        Race::new(&RaceConfig {
            seed: 9,
            route: Route::builtin()[0].clone(),
            laps: 2,
            teams: rival_teams(3, 3.6, 1.0),
            riders_per_team: 6,
            player_team: 2,
            player: PlayerSetup {
                name: "Jij".into(),
                kg: 75.0,
                height_cm: 180.0,
                ftp: 270.0,
                kind: RiderKind::Rouleur,
            },
            countdown_s: 0.0,
        })
    }

    #[test]
    fn a_rival_attack_starts_a_follow_challenge_that_pays_on_the_wheel() {
        let mut r = race();
        let mut t = ChallengeTracker::new(1);
        r.time = 10.0;
        t.update(&r, 0.1);
        let me = r.human;
        let rival = r
            .riders
            .iter()
            .position(|x| x.team != r.riders[me].team)
            .unwrap();
        r.riders[rival].dist = r.riders[me].dist + 5.0;
        r.riders[rival].intent = Intent::Attack {
            intensity: 1.3,
            until: 100.0,
        };
        let ev = t.update(&r, 0.1);
        assert!(matches!(&ev[0], Event::Started(c) if matches!(c.kind, Kind::FollowAttack { .. })));
        let mut done = None;
        for _ in 0..200 {
            for e in t.update(&r, 0.1) {
                if let Event::Completed { xp, .. } = e {
                    done = Some(xp);
                }
            }
        }
        assert_eq!(done, Some(150));
        assert_eq!(t.streak, 1);
        assert!((t.multiplier() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn an_unmet_challenge_fails_and_resets_the_streak() {
        let mut r = race();
        let mut t = ChallengeTracker::new(2);
        r.time = 10.0;
        t.streak = 2;
        t.active = Some(Challenge {
            kind: Kind::Effort {
                watts: 2000.0,
                secs: 15.0,
            },
            title: "Versnelling".into(),
            detail: String::new(),
            xp: 80,
            time_left: 1.0,
            held: 0.0,
            progress: 0.0,
        });
        let ev: Vec<Event> = (0..20).flat_map(|_| t.update(&r, 0.1)).collect();
        assert!(ev.iter().any(|e| matches!(e, Event::Failed { .. })));
        assert_eq!(t.streak, 0);
    }

    #[test]
    fn challenges_keep_coming_during_a_race() {
        let mut r = race();
        let mut t = ChallengeTracker::new(3);
        let mut started = 0;
        while r.time < 900.0 {
            r.step(0.25, 230.0, 90.0);
            for e in t.update(&r, 0.25) {
                if matches!(e, Event::Started(_)) {
                    started += 1;
                }
            }
        }
        assert!(started >= 5, "only {started} challenges in 15 minutes");
    }
}
