//! What the player should know right now: a live objective, and the
//! moments worth celebrating (or warning about) as they happen.

use super::Race;
use super::course::{SEGMENT_POINTS, SegmentKind};

#[derive(Debug, Clone, PartialEq)]
pub enum Moment {
    /// Places gained since the last check.
    PositionGain(usize),
    /// You crossed a sprint or KOM line in the scoring places.
    Segment {
        kind: SegmentKind,
        place: usize,
        points: u32,
    },
    JerseyTaken(SegmentKind),
    JerseyLost(SegmentKind),
    LastLap,
    /// You closed the gap to the group ahead.
    Bridged,
    /// Your group rode away from you.
    Dropped,
    /// You are clear in a breakaway.
    InBreak,
    /// W′ is almost empty.
    EmptyTank,
    Finish {
        place: usize,
    },
}

/// The one goal that matters most at this point of the race.
#[derive(Debug, Clone, PartialEq)]
pub struct Objective {
    pub title: String,
    pub detail: String,
    /// 0..1 progress for a bar, when it makes sense.
    pub progress: Option<f32>,
    /// Draws attention (sprint time).
    pub urgent: bool,
}

/// Groups further ahead than this aren't presented as a gap to close.
const CHASE_HORIZON_S: f32 = 45.0;
/// How long a positive streak of position changes is collected.
const CHECK_INTERVAL_S: f32 = 1.0;

impl Race {
    /// Your place: finishers by time, then everyone else by distance.
    pub fn position(&self, rider: usize) -> usize {
        if let Some(p) = self.finish.iter().position(|f| f.rider == rider) {
            return p + 1;
        }
        let me = self.riders[rider].dist;
        self.finish.len()
            + self
                .riders
                .iter()
                .filter(|r| !r.finished() && r.dist > me)
                .count()
            + 1
    }

    /// Place within your own group (1 = on the front).
    pub fn place_in_group(&self, rider: usize) -> usize {
        let g = self.group_of(rider);
        self.groups
            .get(g)
            .and_then(|g| g.riders.iter().position(|&r| r == rider))
            .map(|p| p + 1)
            .unwrap_or(1)
    }

    fn lap_of(&self, rider: usize) -> u32 {
        let lap = self.course.route.length_m();
        ((self.riders[rider].dist.max(0.0) / lap) as u32 + 1).min(self.course.laps)
    }

    fn leader(&self, kind: SegmentKind) -> Option<usize> {
        self.points(kind).first().map(|&(r, _)| r)
    }

    pub fn objective(&self) -> Option<Objective> {
        let me = self.player();
        if me.finished() || !self.started() {
            return None;
        }
        let left = self.remaining_m(self.human);
        let in_group = self.place_in_group(self.human);
        if left < 300.0 {
            return Some(Objective {
                title: "SPRINT!".into(),
                detail: format!("nog {:.0} m · P{} in je groep", left, in_group),
                progress: Some(1.0 - left as f32 / 300.0),
                urgent: true,
            });
        }
        if left < 3000.0 {
            return Some(Objective {
                title: format!("Finale · {:.1} km", left / 1000.0),
                detail: format!(
                    "Je zit P{} in je groep, P{} in de koers. Spaar je voor de laatste 300 m!",
                    in_group,
                    self.position(self.human)
                ),
                progress: Some(1.0 - left as f32 / 3000.0),
                urgent: false,
            });
        }
        if let Some(seg) = self.course.next_segment(me.dist) {
            let to_line = seg.end_m - me.dist;
            if to_line < 1500.0 && seg.end_m < self.course.total_m - 1.0 {
                let (name, verb) = match seg.kind {
                    SegmentKind::Sprint => ("Tussensprint", "Punten voor de eerste 5"),
                    SegmentKind::Kom => ("Bergsprint", "Bergpunten voor de eerste 5 boven"),
                };
                return Some(Objective {
                    title: format!("{name} over {:.0} m", to_line),
                    detail: format!("{verb} · je zit P{in_group} in je groep"),
                    progress: Some(1.0 - to_line as f32 / 1500.0),
                    urgent: false,
                });
            }
        }
        let g = self.group_of(self.human);
        let peloton = self.peloton();
        if g > 0 {
            let ahead = &self.groups[g - 1];
            let mine = &self.groups[g];
            let gap = mine.gap_s - ahead.gap_s;
            if gap < CHASE_HORIZON_S {
                let name = if g - 1 == 0 {
                    "Kopgroep"
                } else if g - 1 == peloton {
                    "Peloton"
                } else {
                    "Groep voor je"
                };
                return Some(Objective {
                    title: format!("{name} op {:.0} s", gap),
                    detail: format!(
                        "{} renners · haal ze terug en zit in het wiel!",
                        ahead.riders.len()
                    ),
                    progress: Some(1.0 - gap / CHASE_HORIZON_S),
                    urgent: false,
                });
            }
        }
        if g < peloton {
            let lead = self.groups[peloton].gap_s - self.groups[g].gap_s;
            return Some(Objective {
                title: format!("In de vlucht · +{:.0} s", lead),
                detail: "Deel het werk, maar houd W′ over voor de finale".into(),
                progress: None,
                urgent: false,
            });
        }
        None
    }
}

/// Watches the race and reports moments as they happen.
#[derive(Debug, Clone, Default)]
pub struct MomentTracker {
    timer: f32,
    last_position: Option<usize>,
    seen: Vec<usize>,
    leaders: [Option<usize>; 2],
    last_lap: u32,
    group: Vec<usize>,
    ahead: Vec<usize>,
    tank_warned: bool,
    break_announced: bool,
    finished: bool,
}

fn kind_index(kind: SegmentKind) -> usize {
    match kind {
        SegmentKind::Sprint => 0,
        SegmentKind::Kom => 1,
    }
}

impl MomentTracker {
    pub fn update(&mut self, race: &Race, dt: f32) -> Vec<Moment> {
        let mut out = Vec::new();
        if !race.started() {
            return out;
        }
        let me = race.human;
        if self.seen.len() != race.segment_results.len() {
            self.seen = vec![0; race.segment_results.len()];
        }

        // Your crossings of sprint and KOM lines.
        for (s, order) in race.segment_results.iter().enumerate() {
            for (k, &rider) in order.iter().enumerate().skip(self.seen[s]) {
                if rider == me && k < SEGMENT_POINTS.len() {
                    out.push(Moment::Segment {
                        kind: race.course.segments[s].kind,
                        place: k + 1,
                        points: SEGMENT_POINTS[k],
                    });
                }
            }
            self.seen[s] = order.len();
        }
        for kind in [SegmentKind::Sprint, SegmentKind::Kom] {
            let now = race.leader(kind);
            let before = self.leaders[kind_index(kind)];
            if now == Some(me) && before != Some(me) {
                out.push(Moment::JerseyTaken(kind));
            } else if before == Some(me) && now != Some(me) {
                out.push(Moment::JerseyLost(kind));
            }
            self.leaders[kind_index(kind)] = now;
        }

        if let Some(p) = race.finish.iter().position(|f| f.rider == me) {
            if !self.finished {
                self.finished = true;
                out.push(Moment::Finish { place: p + 1 });
            }
            return out;
        }

        let lap = race.lap_of(me);
        if race.course.laps > 1 && lap == race.course.laps && self.last_lap < lap {
            out.push(Moment::LastLap);
        }
        self.last_lap = lap;

        let fraction = race.player().energy.fraction();
        if fraction < 0.15 && !self.tank_warned {
            self.tank_warned = true;
            out.push(Moment::EmptyTank);
        } else if fraction > 0.4 {
            self.tank_warned = false;
        }

        self.timer += dt;
        if self.timer < CHECK_INTERVAL_S {
            return out;
        }
        self.timer = 0.0;

        let position = race.position(me);
        if let Some(last) = self.last_position
            && position < last
            && race.time > 20.0
        {
            out.push(Moment::PositionGain(last - position));
        }
        self.last_position = Some(position);

        // Group changes: did riders that were ahead become your group, or did
        // your group ride away from you?
        let g = race.group_of(me);
        let group: Vec<usize> = race
            .groups
            .get(g)
            .map(|grp| grp.riders.iter().copied().filter(|&r| r != me).collect())
            .unwrap_or_default();
        let ahead: Vec<usize> = race
            .groups
            .iter()
            .take(g)
            .flat_map(|grp| grp.riders.iter().copied())
            .collect();
        if race.time > 30.0 {
            let joined = group.iter().filter(|r| self.ahead.contains(r)).count();
            let left_me = self.group.iter().filter(|r| ahead.contains(r)).count();
            if joined >= 2 {
                out.push(Moment::Bridged);
            } else if left_me >= 3 {
                out.push(Moment::Dropped);
            }
        }
        self.group = group;
        self.ahead = ahead;

        let peloton = race.peloton();
        let clear = g < peloton && race.groups[peloton].gap_s - race.groups[g].gap_s > 10.0;
        if clear && !self.break_announced {
            self.break_announced = true;
            out.push(Moment::InBreak);
        } else if g >= peloton {
            self.break_announced = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::race::field::{RiderKind, rival_teams};
    use crate::race::{PlayerSetup, Race, RaceConfig};
    use crate::route::Route;

    use super::*;

    fn race(laps: u32) -> Race {
        Race::new(&RaceConfig {
            seed: 3,
            route: Route::builtin()[0].clone(),
            laps,
            teams: rival_teams(3, 3.6, 1.0),
            riders_per_team: 4,
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
    fn sprint_objective_in_the_last_300_m() {
        let mut r = race(1);
        r.time = 1.0;
        let me = r.human;
        r.riders[me].dist = r.course.total_m - 200.0;
        let o = r.objective().unwrap();
        assert_eq!(o.title, "SPRINT!");
        assert!(o.urgent);
    }

    #[test]
    fn scoring_a_sprint_and_taking_the_jersey_are_reported() {
        let mut r = race(2);
        let mut t = MomentTracker::default();
        r.time = 5.0;
        t.update(&r, 0.1);
        let sprint = r
            .course
            .segments
            .iter()
            .position(|s| s.kind == SegmentKind::Sprint)
            .unwrap();
        r.segment_results[sprint] = vec![r.human];
        let moments = t.update(&r, 0.1);
        assert!(moments.contains(&Moment::Segment {
            kind: SegmentKind::Sprint,
            place: 1,
            points: 10
        }));
        assert!(moments.contains(&Moment::JerseyTaken(SegmentKind::Sprint)));
        // Reported once only.
        assert!(t.update(&r, 0.1).is_empty());
    }

    #[test]
    fn last_lap_is_announced_once() {
        let mut r = race(3);
        let mut t = MomentTracker::default();
        r.time = 5.0;
        let me = r.human;
        r.riders[me].dist = r.course.route.length_m() * 2.0 + 10.0;
        assert!(t.update(&r, 0.1).contains(&Moment::LastLap));
        assert!(!t.update(&r, 0.1).contains(&Moment::LastLap));
    }
}
