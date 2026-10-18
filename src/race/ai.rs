//! Race tactics. Every rider has an `Intent`; a director per team assigns
//! intents from the race situation (gaps, distance left, terrain, energy),
//! and a control law turns the intent into pedal power each substep.

use super::course::SegmentKind;
use super::field::RiderKind;
use super::{Race, Rider, TeamOrder};
use crate::i18n::t;
use crate::race::course::Course;
use crate::rng::Rng;
use crate::tf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intent {
    /// Ride in the wheels, spending as little as possible.
    Sit,
    /// Work on the front of the group until the race clock reaches `until`.
    Pull {
        intensity: f32,
        until: f32,
    },
    /// Go clear at `intensity` × FTP.
    Attack {
        intensity: f32,
        until: f32,
    },
    /// In a breakaway: share turns with the other escapees.
    Break,
    /// All-out until the given race distance.
    Sprint {
        until_m: f64,
    },
    /// Ride in front of a teammate to shelter them; `slot` 0 is closest.
    Escort {
        rider: usize,
        slot: u8,
    },
    /// Ease up until the given teammate catches up.
    Wait {
        rider: usize,
    },
    /// Jump on a rival's attack and stay on their wheel as long as the legs
    /// allow, as favourites mark each other.
    Mark {
        rider: usize,
        until: f32,
    },
    Finished,
}

impl Intent {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sit => t("in het wiel", "on a wheel"),
            Self::Pull { .. } => t("kopwerk", "pulling"),
            Self::Attack { .. } => t("valt aan", "attacking"),
            Self::Break => t("in de vlucht", "in the break"),
            Self::Sprint { .. } => "sprint",
            Self::Escort { .. } => t("beschermt", "protecting"),
            Self::Wait { .. } => t("wacht", "waiting"),
            Self::Mark { .. } => t("springt mee", "following"),
            Self::Finished => t("binnen", "finished"),
        }
    }
}

/// A team's race plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Keep it together for the sprinter, lead out the finale.
    Sprint,
    /// Set tempo on climbs, the climber attacks late.
    Climb,
    /// Get a rider into the early breakaway; don't chase.
    Break,
}

impl Plan {
    pub fn for_kind(kind: RiderKind) -> Self {
        match kind {
            RiderKind::Climber => Self::Climb,
            _ => Self::Sprint,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sprint => "sprint",
            Self::Climb => t("klimmen", "climb"),
            Self::Break => t("vluchten", "breakaway"),
        }
    }
}

/// A bunch only sits up for a breakaway at least this far ahead; smaller
/// gaps (someone briefly off the front) don't change its pace.
const CALM_BEHIND_BREAK_S: f32 = 15.0;
/// Seconds between director decisions.
const DIRECTOR_INTERVAL: f32 = 2.0;
/// Spacing behind the wheel that riders aim for.
const FOLLOW_GAP_M: f32 = super::pack::MIN_GAP_M + 0.3;
/// Grade (percent, averaged ahead) above which the road counts as a climb.
const CLIMB_PCT: f32 = 3.5;
/// A chase rides this much faster than the break to keep it on its leash...
const CHASE_CLOSING_MPS: f32 = 0.6;
/// ...and at most this much faster to bring it back.
const MAX_CHASE_CLOSING_MPS: f32 = 2.5;
/// How long rivals stay on the player's wheel after marking an attack.
const PLAYER_ATTACK_S: f32 = 30.0;
/// Seconds after an attack before the next spontaneous one.
const ATTACK_LULL_S: f32 = 20.0;
/// Terrain, timing and tactics together make an attack at most this many
/// times likelier than on an ordinary stretch of road.
const MAX_ATTACK_BOOST: f32 = 8.0;
/// Sprint teams form their lead-out train inside this distance.
const LEADOUT_M: f64 = 3500.0;
/// Roughly how much faster than the bunch a sprint goes, m/s.
const SPRINT_GAIN_MPS: f32 = 4.0;
/// Chasers pace their turns to last a chase of a few minutes.
const CHASE_EFFORT_S: f32 = 180.0;
/// A break this big is a threat whoever is in it.
const BIG_BREAK: usize = 6;
/// The player riding this much under the group's pace gets passed...
const EASING_MARGIN_MPS: f32 = 0.3;
/// ...and this much over it is an attack most riders don't follow.
const ATTACK_MARGIN_MPS: f32 = 0.8;

/// Picks a team's plan and leader from its riders and the course.
pub fn choose_plan(
    rng: &mut Rng,
    members: &[Rider],
    course: &Course,
    first: usize,
) -> (Plan, usize) {
    let find = |kind| {
        members
            .iter()
            .position(|r| r.profile.kind == kind)
            .unwrap_or(0)
            + first
    };
    let lap_km = course.route.length_m() / 1000.0;
    let gain: f32 = course
        .route
        .points
        .windows(2)
        .map(|w| (w[1].1 - w[0].1).max(0.0))
        .sum();
    let hilly = gain / lap_km as f32 > 12.0;
    if hilly && rng.chance(0.6) {
        (Plan::Climb, find(RiderKind::Climber))
    } else if rng.chance(0.25) {
        (Plan::Break, find(RiderKind::Sprinter))
    } else {
        (Plan::Sprint, find(RiderKind::Sprinter))
    }
}

/// Seconds before the line at which each rider type opens the sprint with
/// a full tank: sprinters can hold their top speed longest.
fn launch_s(kind: RiderKind) -> f32 {
    match kind {
        RiderKind::Sprinter => 14.0,
        RiderKind::Rouleur => 12.0,
        RiderKind::Domestique => 10.0,
        RiderKind::Climber => 9.0,
    }
}

/// Distance before the line at which a rider opens the sprint: a fixed time
/// at sprinting speed, shorter on tired legs, so a sprint from a slow bunch
/// starts closer to the line than one off a fast lead-out.
fn launch_m(r: &Rider) -> f64 {
    let legs = (0.4 + r.energy.fraction()).min(1.0);
    (launch_s(r.profile.kind) * legs * (r.speed + SPRINT_GAIN_MPS)) as f64
}

/// How big a gap (seconds) the bunch tolerates with `remaining_s` of racing
/// left: a steady chase takes back about a second every sixteen (a minute
/// per ten kilometers or so), and a break should be back about five minutes
/// before the line.
pub fn allowed_gap_s(remaining_s: f32) -> f32 {
    ((remaining_s - 300.0) / 16.0).clamp(5.0, 300.0)
}

/// Power a rider can hold for `t_rem` more seconds: critical power plus
/// whatever anaerobic capacity is left, spread over the effort.
pub fn sustainable_power(cp: f32, w_bal: f32, t_rem: f32) -> f32 {
    cp + w_bal / t_rem.max(30.0)
}

struct Situation {
    group_of: Vec<usize>,
    peloton: usize,
}

pub fn decide(race: &mut Race) {
    if race.groups.is_empty() {
        return;
    }
    let sit = Situation {
        group_of: {
            let mut g = vec![usize::MAX; race.riders.len()];
            for (gi, grp) in race.groups.iter().enumerate() {
                for &r in &grp.riders {
                    g[r] = gi;
                }
            }
            g
        },
        peloton: race.peloton(),
    };
    set_group_tempo(race);
    note_catches(race, &sit);
    pacing(race, &sit);
    expire_intents(race, &sit);
    if race.director_timer >= DIRECTOR_INTERVAL {
        race.director_timer = 0.0;
        for t in 0..race.teams.len() {
            if race.teams[t].human {
                human_team_orders(race, t, &sit);
            } else {
                direct_team(race, t, &sit);
            }
        }
    }
    attacks(race, &sit);
    player_attack(race, &sit);
    sprints(race, &sit);
}

/// Each group's front riders ride at a share of the median rider's FTP in
/// W/kg, so the bunch keeps a steady effort and its speed follows the road;
/// weaker riders suffer on climbs where drafting helps little. With a real
/// breakaway up the road (more than `CALM_BEHIND_BREAK_S`) the bunch eases
/// off, which lets breaks form; the race builds up and the finale is fast.
fn set_group_tempo(race: &mut Race) {
    let progress_of =
        |race: &Race, i: usize| 1.0 - (race.remaining_m(i) / race.course.total_m) as f32;
    let peloton = race.peloton();
    let tempos: Vec<f32> = race
        .groups
        .iter()
        .enumerate()
        .map(|(gi, g)| {
            let mut wkg: Vec<f32> = g
                .riders
                .iter()
                .map(|&i| race.riders[i].profile.wkg())
                .collect();
            wkg.sort_by(f32::total_cmp);
            let median = wkg[wkg.len() / 2];
            let progress = progress_of(race, g.riders[0]);
            let build_up = progress.clamp(0.0, 0.6) / 0.6 * 0.06;
            let finale = ((progress - 0.85) / 0.15).clamp(0.0, 1.0) * 0.1;
            let chasing = g
                .riders
                .iter()
                .any(|&i| matches!(race.riders[i].intent, Intent::Pull { .. }));
            let calm = gi == peloton && peloton > 0 && g.gap_s > CALM_BEHIND_BREAK_S && !chasing;
            let climbing = race
                .course
                .grade_ahead_pct(race.riders[g.riders[0]].dist, 300.0)
                > CLIMB_PCT;
            // A small lead group near the line: nobody wants to pull the
            // others to the finish, so they look at each other.
            let watching = gi == 0
                && g.riders.len() <= 10
                && race.remaining_m(g.riders[0]) < 6000.0
                && race.remaining_m(g.riders[0]) > 400.0;
            // Climbs are raced close to threshold; flats are steadier.
            let base = if climbing {
                0.9
            } else if watching {
                0.72
            } else if calm {
                0.78
            } else {
                0.84 + build_up
            };
            median * (base + finale)
        })
        .collect();
    for (g, t) in race.groups.iter_mut().zip(tempos) {
        g.tempo_wkg = t;
    }
}

/// Spots a breakaway being caught: the moment counter-attacks fly.
fn note_catches(race: &mut Race, sit: &Situation) {
    let break_up = sit.peloton > 0 && race.groups.first().is_some_and(|g| g.riders.len() < 12);
    if race.break_up && !break_up && race.started() {
        race.last_catch = race.time;
        race.log(t("De vlucht is ingelopen!", "The break has been caught!").into());
    }
    race.break_up = break_up;
}

/// Smart pacing: on a climb each rider works out what they can hold to the
/// top and won't go deeper to follow a wheel; in the finale the fast men
/// and leaders fight for position near the front.
fn pacing(race: &mut Race, sit: &Situation) {
    for i in 0..race.riders.len() {
        let r = &race.riders[i];
        if !is_bot(r) {
            continue;
        }
        let limit = match race.course.to_summit(r.dist) {
            Some(to_top) if to_top > 300.0 => {
                let t_rem = to_top as f32 / r.speed.max(2.5);
                sustainable_power(r.energy.cp, r.energy.balance, t_rem) * 1.03
            }
            _ => f32::MAX,
        };
        let g = sit.group_of[i];
        let place = race
            .groups
            .get(g)
            .and_then(|grp| grp.riders.iter().position(|&x| x == i))
            .unwrap_or(0);
        let leader = race.teams[r.team].leader == i;
        let fast = matches!(r.profile.kind, RiderKind::Sprinter | RiderKind::Rouleur);
        // Riders sent to the front to work move up the side of the bunch,
        // and in the finale a sprinter goes with their lead-out train.
        let train = matches!(r.intent, Intent::Pull { .. })
            || race.remaining_m(i) < LEADOUT_M && leader && race.teams[r.team].plan == Plan::Sprint;
        let move_up = race.remaining_m(i) < 4000.0
            && place > 10
            && r.energy.fraction() > 0.3
            && (fast || leader)
            || train;
        let r = &mut race.riders[i];
        r.limit_w = limit;
        r.move_up = move_up;
    }
}

fn is_bot(r: &Rider) -> bool {
    !r.human && !r.finished()
}

fn expire_intents(race: &mut Race, sit: &Situation) {
    let now = race.time;
    for i in 0..race.riders.len() {
        let g = sit.group_of[i];
        let r = &mut race.riders[i];
        if !is_bot(r) {
            continue;
        }
        r.intent = match r.intent {
            Intent::Pull { until, .. } if now > until => Intent::Sit,
            Intent::Attack { until, .. } if now > until => {
                if g < sit.peloton {
                    Intent::Break
                } else {
                    Intent::Sit
                }
            }
            Intent::Mark { until, .. } if now > until => {
                if g < sit.peloton {
                    Intent::Break
                } else {
                    Intent::Sit
                }
            }
            Intent::Break if g >= sit.peloton => Intent::Sit,
            Intent::Sprint { until_m } if r.dist >= until_m => Intent::Sit,
            other => other,
        };
        // Spent riders stop working.
        if r.energy.fraction() < 0.08
            && matches!(
                r.intent,
                Intent::Pull { .. } | Intent::Attack { .. } | Intent::Mark { .. }
            )
        {
            r.intent = Intent::Sit;
        }
    }
}

fn members(race: &Race, team: usize) -> Vec<usize> {
    (0..race.riders.len())
        .filter(|&i| race.riders[i].team == team && is_bot(&race.riders[i]))
        .collect()
}

fn direct_team(race: &mut Race, t: usize, sit: &Situation) {
    let now = race.time;
    let team = race.teams[t].clone();
    let leader = team.leader;
    let lg = sit.group_of[leader];
    let crew: Vec<usize> = members(race, t)
        .into_iter()
        .filter(|&i| i != leader)
        .collect();
    let in_front = |i: usize| sit.group_of[i] < sit.peloton;
    let team_ahead = crew.iter().any(|&i| in_front(i)) || (lg != usize::MAX && lg < sit.peloton);
    let remaining = race.remaining_m(leader);
    let progress = 1.0 - (remaining / race.course.total_m) as f32;
    let mut assign: Vec<(usize, Intent)> = Vec::new();

    // Keep a breakaway on a leash: the gap the bunch tolerates shrinks
    // towards the line, and chasers ride whatever it takes to bring it back
    // to that, so a break is caught late, as in a real race. Whichever group
    // the leader is in chases, unless a teammate is further up the road.
    let chasing = lg != usize::MAX && lg > 0 && !crew.iter().any(|&i| sit.group_of[i] < lg);
    let bunch_gap = race.groups.get(lg).map(|g| g.gap_s).unwrap_or(0.0);
    if chasing {
        let bunch_speed = race.groups[lg]
            .riders
            .first()
            .map(|&r| race.riders[r].speed)
            .unwrap_or(10.0)
            .max(5.0);
        // Sprinters' teams want the race together; the others mostly
        // leave the work to them.
        let keen = match team.plan {
            Plan::Sprint => 1.0,
            Plan::Climb => 1.5,
            Plan::Break => 2.5,
        };
        // Who is up the road matters as much as how far: a big break, or one
        // holding a rider as strong as our leader, gets a much shorter leash
        // than a couple of harmless escapees.
        let ahead: Vec<&Rider> = race.groups[..lg]
            .iter()
            .flat_map(|g| &g.riders)
            .map(|&i| &race.riders[i])
            .collect();
        let rival = ahead
            .iter()
            .any(|r| r.profile.wkg() >= race.riders[leader].profile.wkg());
        let threat = match (ahead.len(), rival) {
            (_, true) => 0.5,
            (n, false) if n >= BIG_BREAK => 0.7,
            (n, false) if n <= 2 => 1.2,
            _ => 1.0,
        };
        let allowed = allowed_gap_s(remaining as f32 / bunch_speed) * keen * threat;
        if bunch_gap > allowed {
            let head = &race.riders[race.groups[0].riders[0]];
            let grade = race.course.grade_pct_at(head.dist) / 100.0;
            // Close the excess within about a minute and a half, on top of
            // keeping pace with the shrinking allowance.
            let closing = (CHASE_CLOSING_MPS + (bunch_gap - allowed) * bunch_speed / 90.0)
                .min(MAX_CHASE_CLOSING_MPS);
            let v_target = head.speed + closing;
            let need = if bunch_gap > allowed * 2.0 { 3 } else { 2 };
            // Riders already on the job stay on it until they tire; fresh
            // ones near the front of the bunch take over from them. A chase
            // is ridden around threshold, so tired legs can still help (the
            // effort shrinks with what they have left).
            let bunch = &race.groups[lg].riders;
            let place = |i: usize| bunch.iter().position(|&x| x == i).unwrap_or(usize::MAX);
            let mut workers: Vec<(bool, usize, usize)> = crew
                .iter()
                .copied()
                .filter(|&i| sit.group_of[i] == lg)
                .filter_map(|i| {
                    let r = &race.riders[i];
                    let pulling = matches!(r.intent, Intent::Pull { .. });
                    let fresh = r.energy.fraction() > if pulling { 0.1 } else { 0.25 };
                    fresh.then_some((!pulling, place(i), i))
                })
                .collect();
            workers.sort();
            for &(_, _, i) in workers.iter().take(need) {
                let r = &race.riders[i];
                let solo = r.bike.resistance_n(v_target, grade, 0.0) * v_target
                    / (1.0 - r.bike.drivetrain_loss);
                // Hard turns, but ones the rider can repeat for a while.
                let repeatable = sustainable_power(r.energy.cp, r.energy.balance, CHASE_EFFORT_S);
                assign.push((
                    i,
                    Intent::Pull {
                        intensity: (solo.min(repeatable) / r.profile.ftp).clamp(0.85, 1.5),
                        until: now + DIRECTOR_INTERVAL + 1.0,
                    },
                ));
            }
        }
    }

    // Breakaway teams send a rider up the road early on.
    if team.plan == Plan::Break
        && !team_ahead
        && progress < 0.6
        && progress > 0.05
        && race.rng().chance(0.08)
        && let Some(&i) = crew.iter().find(|&&i| {
            let r = &race.riders[i];
            matches!(r.profile.kind, RiderKind::Rouleur | RiderKind::Domestique)
                && r.energy.fraction() > 0.8
                && r.intent == Intent::Sit
        })
    {
        let until = now + race.rng().range(35.0, 55.0);
        assign.push((
            i,
            Intent::Attack {
                intensity: 1.45,
                until,
            },
        ));
        let text = tf!(
            "{} ({}) probeert mee te springen",
            "{} ({}) tries to bridge across",
            race.riders[i].profile.name,
            team.spec.name
        );
        race.log(text);
    }

    if lg != usize::MAX {
        let leader_dist = race.riders[leader].dist;
        let climbing = race.course.grade_ahead_pct(leader_dist, 400.0) > CLIMB_PCT;

        // Climbers' teams set tempo uphill; the climber goes near the top.
        if team.plan == Plan::Climb && climbing && progress > 0.4 {
            if let Some(&i) = crew
                .iter()
                .find(|&&i| sit.group_of[i] == lg && race.riders[i].energy.fraction() > 0.3)
            {
                assign.push((
                    i,
                    Intent::Pull {
                        intensity: 0.97,
                        until: now + 10.0,
                    },
                ));
            }
            let to_top = to_summit(race, leader_dist).unwrap_or(f64::MAX);
            let r = &race.riders[leader];
            // Save the decisive attack for the second half of the race.
            let steep = race.course.grade_ahead_pct(leader_dist, 150.0) > 7.0;
            let ready = progress > 0.5
                && to_top < 1200.0
                && r.energy.fraction() > 0.55
                && r.intent == Intent::Sit;
            let name = r.profile.name.clone();
            let chance = if steep { 0.4 } else { 0.12 };
            if ready && race.rng().chance(chance) {
                let until = now + (to_top as f32 / 5.0).min(120.0);
                race.riders[leader].intent = Intent::Attack {
                    intensity: 1.25,
                    until,
                };
                let text = tf!(
                    "{name} ({}) demarreert op de klim!",
                    "{name} ({}) attacks on the climb!",
                    team.spec.name
                );
                race.log(text);
                mark_attack(race, leader, sit, until);
            }
        }

        // On the last climb every leader who can, goes: the punchers' finale.
        let r = &race.riders[leader];
        let can_go = on_final_climb(race, leader_dist)
            && team.plan != Plan::Climb
            && r.profile.kind != RiderKind::Sprinter
            && r.energy.fraction() > 0.5
            && r.intent == Intent::Sit
            && race.course.grade_ahead_pct(leader_dist, 150.0) > 6.0;
        let name = r.profile.name.clone();
        if can_go && race.rng().chance(0.15) {
            let to_top = to_summit(race, leader_dist).unwrap_or(500.0);
            let until = now + (to_top as f32 / 5.5).clamp(15.0, 90.0);
            race.riders[leader].intent = Intent::Attack {
                intensity: 1.3,
                until,
            };
            race.log(tf!(
                "{name} ({}) gaat op de laatste klim!",
                "{name} ({}) goes on the last climb!",
                team.spec.name
            ));
            mark_attack(race, leader, sit, until);
        }

        // Sprint teams line up a lead-out train in the final kilometers.
        let near_front = race.groups.get(lg).is_some_and(|g| g.gap_s < 15.0);
        if team.plan == Plan::Sprint && remaining < LEADOUT_M && near_front {
            // The last men of the train empty themselves.
            let intensity = if remaining > 2000.0 {
                1.0
            } else if remaining > 1000.0 {
                1.15
            } else {
                1.5
            };
            for &i in crew.iter().filter(|&&i| sit.group_of[i] == lg) {
                if race.riders[i].energy.fraction() > 0.1 {
                    assign.push((
                        i,
                        Intent::Pull {
                            intensity,
                            until: now + 3.0,
                        },
                    ));
                }
            }
        }

        // Contest intermediate sprints and KOMs with the right rider.
        if let Some(seg) = race.course.next_segment(leader_dist) {
            let to_line = seg.end_m - leader_dist;
            let r = &race.riders[leader];
            let wants = match seg.kind {
                SegmentKind::Sprint => team.plan == Plan::Sprint && to_line < 250.0,
                SegmentKind::Kom => team.plan == Plan::Climb && to_line < 300.0,
            };
            if wants
                && lg == 0
                && r.energy.fraction() > 0.5
                && seg.end_m < race.course.total_m - 1.0
            {
                assign.push((leader, Intent::Sprint { until_m: seg.end_m }));
            }
        }
    }

    for (i, intent) in assign {
        race.riders[i].intent = intent;
    }
}

fn human_team_orders(race: &mut Race, t: usize, sit: &Situation) {
    let now = race.time;
    let human = race.human;
    let hg = sit.group_of[human];
    let order = race.teams[t].order;
    for i in members(race, t) {
        let r = &race.riders[i];
        let with_me = sit.group_of[i] == hg;
        let fresh = r.energy.fraction() > 0.1;
        let intent = match order {
            TeamOrder::Free => match r.intent {
                Intent::Escort { .. } | Intent::Wait { .. } | Intent::Pull { .. } => Intent::Sit,
                other => other,
            },
            TeamOrder::Tempo | TeamOrder::Chase | TeamOrder::LeadOut if !(with_me && fresh) => {
                Intent::Sit
            }
            TeamOrder::Tempo => Intent::Pull {
                intensity: 0.88,
                until: now + 3.0,
            },
            TeamOrder::Chase => Intent::Pull {
                intensity: 1.0,
                until: now + 3.0,
            },
            TeamOrder::LeadOut => Intent::Pull {
                intensity: 1.12,
                until: now + 3.0,
            },
            TeamOrder::Protect if with_me => Intent::Escort {
                rider: human,
                slot: 0,
            },
            TeamOrder::Protect if sit.group_of[i] < hg && fresh => Intent::Wait { rider: human },
            TeamOrder::Protect => Intent::Sit,
        };
        race.riders[i].intent = intent;
    }
    if order == TeamOrder::Protect {
        // Escorts line up one behind the other in front of the player.
        let mut slot = 0u8;
        for i in members(race, t) {
            if let Intent::Escort { rider, .. } = race.riders[i].intent {
                race.riders[i].intent = Intent::Escort { rider, slot };
                slot += 1;
            }
        }
    }
    if order == TeamOrder::Tempo {
        // Only the two freshest teammates set tempo.
        let mut pullers: Vec<usize> = members(race, t)
            .into_iter()
            .filter(|&i| matches!(race.riders[i].intent, Intent::Pull { .. }))
            .collect();
        pullers.sort_by(|&a, &b| {
            race.riders[b]
                .energy
                .balance
                .total_cmp(&race.riders[a].energy.balance)
        });
        for &i in pullers.iter().skip(2) {
            race.riders[i].intent = Intent::Sit;
        }
    }
}

/// Spontaneous attacks and riders jumping across to them.
fn attacks(race: &mut Race, sit: &Situation) {
    let now = race.time;
    let break_exists = sit.peloton > 0;
    for i in 0..race.riders.len() {
        let r = &race.riders[i];
        let remaining = race.remaining_m(i);
        // Late on, breakaway companions turn on each other.
        let may = r.intent == Intent::Sit || r.intent == Intent::Break && remaining < 12_000.0;
        let fresh_enough = if remaining < 8000.0 { 0.45 } else { 0.75 };
        if !is_bot(r) || !may || r.energy.fraction() < fresh_enough {
            continue;
        }
        let team = &race.teams[r.team];
        if team.human || team.leader == i && team.plan == Plan::Sprint {
            continue;
        }
        let progress = 1.0 - (race.remaining_m(i) / race.course.total_m) as f32;
        let phase = if progress < 0.08 {
            0.0
        } else if progress > 0.85 {
            2.5
        } else if break_exists {
            0.35
        } else {
            1.0
        };
        let grade = race.course.grade_ahead_pct(r.dist, 300.0);
        let steep = race.course.grade_ahead_pct(r.dist, 120.0);
        let terrain = if grade < -2.0 {
            0.0 // nobody attacks downhill
        } else if grade > 4.0 {
            let kind = match r.profile.kind {
                RiderKind::Climber => 3.0,
                RiderKind::Sprinter => 0.2,
                _ => 1.0,
            };
            // Attacks come where it's steepest.
            kind * if steep > 8.0 { 2.0 } else { 1.0 }
        } else {
            1.0
        };
        // The final climb is where the race is decided.
        let decisive = if on_final_climb(race, r.dist) {
            3.0
        } else {
            1.0
        };
        // A small group near the line: riders watch each other, then someone
        // goes before the sprinters can.
        let group = &race.groups[sit.group_of[i]];
        let cat_and_mouse = if group.riders.len() <= 10 && remaining < 6000.0 {
            if r.profile.kind == RiderKind::Sprinter {
                0.3
            } else {
                4.0
            }
        } else {
            1.0
        };
        // Right after a catch the bunch is vulnerable: counter-attacks.
        let counter = if now - race.last_catch < 40.0 {
            4.0
        } else {
            1.0
        };
        // Someone to attack: a rider alone has nobody to drop. And after an
        // attack the race needs a moment before the next one flies.
        if group.riders.len() < 2 || now - race.last_attack < ATTACK_LULL_S {
            continue;
        }
        let moment = f32::min(
            phase * terrain * counter * decisive * cat_and_mouse,
            MAX_ATTACK_BOOST,
        );
        let p = r.profile.aggression * 0.00015 * moment;
        if !race.rng().chance(p) {
            continue;
        }
        race.last_attack = now;
        // Uphill, where speed comes from power against gravity alone, a
        // smaller surge is enough to go clear and a climber can't afford
        // to empty the tank.
        let intensity = if grade > 4.0 {
            race.rng().range(1.2, 1.4)
        } else {
            race.rng().range(1.35, 1.6)
        };
        let until = now + race.rng().range(20.0, 45.0);
        race.riders[i].intent = Intent::Attack { intensity, until };
        let name = race.riders[i].profile.name.clone();
        let team_name = race.teams[race.riders[i].team].spec.name.clone();
        race.log(tf!(
            "Aanval van {name} ({team_name})!",
            "Attack by {name} ({team_name})!"
        ));

        // Riders of the same calibre jump on it.
        mark_attack(race, i, sit, until);
    }
}

/// The player going clear off the front of their group is an attack like
/// any other: rivals of the same calibre may mark it, the rest let them go.
fn player_attack(race: &mut Race, sit: &Situation) {
    let h = race.human;
    let p = &race.riders[h];
    let Some(group) = race.groups.get(sit.group_of[h]) else {
        return;
    };
    if p.finished() || race.time < race.player_marked_until || group.riders.first() != Some(&h) {
        return;
    }
    let grade = race.course.grade_pct_at(p.dist) / 100.0;
    let pace = p.bike.steady_speed(group.tempo_wkg * p.profile.kg, grade);
    if p.speed > pace + ATTACK_MARGIN_MPS && p.power > 1.2 * p.profile.ftp {
        let until = race.time + PLAYER_ATTACK_S;
        race.player_marked_until = until;
        mark_attack(race, h, sit, until);
    }
}

/// Rivals respond to an attack the way favourites do in a real finale: the
/// other leaders in the group who are about as strong jump on the wheel
/// (marking), the rest ride their own pace and let the directors decide.
fn mark_attack(race: &mut Race, attacker: usize, sit: &Situation, until: f32) {
    let g = sit.group_of[attacker];
    let a = &race.riders[attacker];
    let a_wkg = a.profile.wkg();
    let a_team = a.team;
    let important = race.teams[a_team].leader == attacker || a_wkg > race.groups[g].tempo_wkg * 1.2;
    let mut responders: Vec<usize> = (0..race.riders.len())
        .filter(|&j| {
            let o = &race.riders[j];
            is_bot(o)
                && j != attacker
                && sit.group_of[j] == g
                && o.team != a_team
                && !race.teams[o.team].human
                && matches!(o.intent, Intent::Sit | Intent::Break)
                && o.energy.fraction() > 0.35
                && o.profile.wkg() > a_wkg * 0.93
                // Leaders mark leaders; anyone strong may mark a big name.
                && (race.teams[o.team].leader == j || important && o.profile.aggression > 0.5)
        })
        .collect();
    // The freshest react first.
    responders.sort_by(|&x, &y| {
        race.riders[y]
            .energy
            .balance
            .total_cmp(&race.riders[x].energy.balance)
    });
    let mut names = Vec::new();
    for &j in responders.iter().take(3) {
        // Not everyone reacts at once: some hesitate and lose the wheel.
        if !race.rng().chance(0.75) {
            continue;
        }
        race.riders[j].intent = Intent::Mark {
            rider: attacker,
            until: until + 5.0,
        };
        names.push(race.riders[j].profile.name.clone());
    }
    if !names.is_empty() {
        let verb = if names.len() > 1 {
            t("springen", "go")
        } else {
            t("springt", "goes")
        };
        race.log(tf!(
            "{} {verb} mee!",
            "{} {verb} with it!",
            names.join(t(" en ", " and "))
        ));
    }
}

/// Whether the rider is on the decisive climb: the hardest one in the
/// second half of the race (La Roche-aux-Faucons in Liège), where the
/// favourites play their cards.
fn on_final_climb(race: &Race, d: f64) -> bool {
    let half = race.course.total_m * 0.45;
    let score = |c: &&super::course::Climb| c.length_m() as f32 * c.avg_pct;
    race.course
        .climbs
        .iter()
        .filter(|c| c.end_m > half)
        .max_by(|a, b| score(a).total_cmp(&score(b)))
        .is_some_and(|c| d >= c.start_m && d < c.end_m)
}

/// Meters to the top of the climb the rider is on.
fn to_summit(race: &Race, d: f64) -> Option<f64> {
    race.course
        .climbs
        .iter()
        .find(|c| d >= c.start_m && d < c.end_m)
        .map(|c| c.end_m - d)
}

/// Everyone near the front opens their sprint in the last few hundred meters.
fn sprints(race: &mut Race, sit: &Situation) {
    let total = race.course.total_m;
    for i in 0..race.riders.len() {
        let r = &race.riders[i];
        // Lead-out men keep pulling instead of sprinting.
        if !is_bot(r) || matches!(r.intent, Intent::Sprint { .. } | Intent::Pull { .. }) {
            continue;
        }
        let front = race
            .groups
            .get(sit.group_of[i])
            .is_some_and(|g| g.gap_s < 3.0);
        if front && race.remaining_m(i) < launch_m(r) && r.energy.fraction() > 0.05 {
            race.riders[i].intent = Intent::Sprint {
                until_m: total + 50.0,
            };
        }
    }
}

/// Riders close a speed difference over about this many seconds.
const CATCH_UP_S: f32 = 2.5;

/// Power needed to reach `v_target` quickly from the rider's current state.
fn required_power(r: &Rider, v_target: f32, grade: f32) -> f32 {
    let b = &r.bike;
    let accel = ((v_target - r.speed) / CATCH_UP_S).clamp(-0.8, 0.8);
    let force =
        b.resistance_n(r.speed, grade, r.draft) + (b.total_kg() + b.wheel_inertia_kg) * accel;
    (force * r.speed.max(1.0) / (1.0 - b.drivetrain_loss)).max(0.0)
}

/// Follows a wheel. Riders react to the wheel speeding up or slowing down,
/// not only to the gap it leaves, so a change of pace at the front passes
/// down a line of riders instead of growing into a yo-yo at the back.
fn hold_wheel(r: &Rider, wheel: &Rider, grade: f32) -> f32 {
    let gap = (wheel.dist - r.dist) as f32;
    let v_target =
        wheel.speed + wheel.accel * CATCH_UP_S + ((gap - FOLLOW_GAP_M) * 0.5).clamp(-2.0, 3.0);
    required_power(r, v_target, grade)
}

/// Pace of a rider leading a group that nobody is driving: the group's tempo
/// in W/kg, a little harder uphill and easy downhill, limited to what this
/// rider can push for a short turn.
fn front_pace(r: &Rider, grade: f32, tempo_wkg: f32) -> f32 {
    let ftp = r.profile.ftp;
    if super::pack::swinging(r) {
        return 0.6 * ftp;
    }
    let terrain = if grade > 0.03 {
        1.05
    } else if grade < -0.02 {
        0.45
    } else {
        1.0
    };
    (tempo_wkg * r.profile.kg * terrain).min(1.15 * ftp)
}

/// Pace of a rider on the front of a group. On the decisive climb the
/// favourites stop riding the group's tempo: whoever is on the front rides
/// what they can hold to the top, and the climb sorts the riders by
/// strength.
fn own_pace(race: &Race, r: &Rider, grade: f32, tempo_wkg: f32) -> f32 {
    let tempo = front_pace(r, grade, tempo_wkg);
    if on_final_climb(race, r.dist) && r.limit_w < f32::MAX && !super::pack::swinging(r) {
        tempo.max(r.limit_w * 0.97)
    } else {
        tempo
    }
}

/// Nobody but the group's tempo setters decides its pace. A bot on the wheel
/// of the player, of an attacker or of someone marking one rides round them
/// when they ease below the group's tempo, and lets them go when they go
/// clear: the riders who mark the move were picked when it went (see
/// `mark_attack`), and the chase is up to the team directors. An attacker's
/// teammates never tow the bunch after them: they block.
fn behind_surge(r: &Rider, wheel: &Rider, grade: f32, tempo_wkg: f32, marks: bool) -> f32 {
    let pace_v = r.bike.steady_speed(front_pace(r, grade, tempo_wkg), grade);
    if wheel.speed < pace_v - EASING_MARGIN_MPS {
        return required_power(r, pace_v, grade);
    }
    let hold = hold_wheel(r, wheel, grade);
    if wheel.speed > pace_v + ATTACK_MARGIN_MPS && !marks {
        hold.min(required_power(r, pace_v + ATTACK_MARGIN_MPS, grade))
    } else {
        hold
    }
}

/// Whether the leader of a group should close the gap to `target` up the
/// road: yes when the group ahead is an accidental split (riders who just
/// rolled clear, e.g. after an intermediate sprint), which a bunch shuts
/// down at once; no when it is a real move (an attack, a breakaway, riders
/// marking one), which only team directors decide to chase.
fn should_bridge(race: &Race, target: usize) -> bool {
    let committed = |i: usize| {
        matches!(
            race.riders[i].intent,
            Intent::Attack { .. } | Intent::Break | Intent::Mark { .. } | Intent::Sprint { .. }
        )
    };
    match race.groups.iter().find(|g| g.riders.contains(&target)) {
        Some(g) => !g.riders.iter().any(|&i| committed(i)),
        None => !committed(target),
    }
}

/// Effort to close a gap to `target`: as hard as a short chase allows.
fn bridge(r: &Rider, target: &Rider, grade: f32) -> f32 {
    let gap = (target.dist - r.dist) as f32;
    let v_target = target.speed + ((gap - FOLLOW_GAP_M) * 0.25).clamp(0.0, 2.5);
    required_power(r, v_target, grade).min(1.2 * r.profile.ftp)
}

/// For a sprint team's leader in the finale: the wheel to ride. That's the
/// nearest teammate ahead in their lead-out train or, with no train of their
/// own, a rival sprinter who has one: sprinters without a team surf the
/// strongest train.
fn train_ahead(race: &Race, i: usize) -> Option<usize> {
    let r = &race.riders[i];
    let team = &race.teams[r.team];
    if team.leader != i || team.plan != Plan::Sprint || race.remaining_m(i) >= LEADOUT_M {
        return None;
    }
    let g = race.groups.iter().find(|g| g.riders.contains(&i))?;
    let own_train = |s: usize| {
        let leader = &race.riders[s];
        g.riders
            .iter()
            .copied()
            .filter(|&j| {
                let o = &race.riders[j];
                o.team == leader.team
                    && matches!(o.intent, Intent::Pull { .. })
                    && o.dist > leader.dist
            })
            .min_by(|&a, &b| race.riders[a].dist.total_cmp(&race.riders[b].dist))
    };
    own_train(i).or_else(|| {
        g.riders
            .iter()
            .copied()
            .filter(|&j| {
                let t = &race.teams[race.riders[j].team];
                j != i && t.leader == j && t.plan == Plan::Sprint && !race.riders[j].human
            })
            .filter(|&j| own_train(j).is_some())
            .max_by(|&a, &b| race.riders[a].dist.total_cmp(&race.riders[b].dist))
    })
}

/// Pedal power for a bot this substep.
pub fn power(race: &Race, i: usize, wheel: Option<usize>, ahead: Option<usize>) -> f32 {
    let r = &race.riders[i];
    let ftp = r.profile.ftp;
    let grade = race.course.grade_pct_at(r.dist) / 100.0;
    let follow = |w: usize| hold_wheel(r, &race.riders[w], grade);
    let tempo = race
        .groups
        .iter()
        .find(|g| g.riders.contains(&i))
        .map(|g| g.tempo_wkg)
        .unwrap_or(3.0);
    let p = match r.intent {
        Intent::Finished => 0.4 * ftp,
        // A sprinter rides on the last man of their team's lead-out train.
        Intent::Sit if let Some(last) = train_ahead(race, i) => follow(last),
        Intent::Sit => match (wheel, ahead) {
            // Moving up: a little extra to come past riders in the finale.
            (Some(w), _) if race.riders[w].human => {
                // Late in the race the favourites don't let you ride away.
                let progress = 1.0 - (race.remaining_m(i) / race.course.total_m) as f32;
                let marks =
                    race.teams[r.team].leader == i && progress > 0.55 && r.energy.fraction() > 0.3;
                behind_surge(r, &race.riders[w], grade, tempo, marks)
            }
            // Nobody tows the bunch after an attacker or the riders
            // marking them.
            (Some(w), _)
                if matches!(
                    race.riders[w].intent,
                    Intent::Attack { .. } | Intent::Mark { .. }
                ) =>
            {
                behind_surge(r, &race.riders[w], grade, tempo, false)
            }
            (Some(w), _) if r.move_up => follow(w) + 0.08 * ftp,
            (Some(w), _) => follow(w),
            (None, Some(a)) if should_bridge(race, a) => {
                bridge(r, &race.riders[a], grade).max(own_pace(race, r, grade, tempo))
            }
            (None, _) => own_pace(race, r, grade, tempo),
        },
        Intent::Pull { intensity, .. } => match wheel {
            // Turns are shared: off the front, ease up and drop back...
            _ if super::pack::swinging(r) => 0.6 * ftp,
            // ...wait in the line of riders doing the work...
            Some(w) if matches!(race.riders[w].intent, Intent::Pull { .. }) => follow(w),
            // ...take over from whoever is on the front...
            Some(w) if race.groups.iter().any(|g| g.riders.first() == Some(&w)) => intensity * ftp,
            // ...or move up through the bunch to join it; in the finale
            // lead-out trains fight their way up the side of the bunch.
            Some(w) if race.remaining_m(i) < LEADOUT_M => follow(w) + 0.3 * ftp,
            Some(w) => follow(w) + 0.2 * ftp,
            None => intensity * ftp,
        },
        Intent::Attack { intensity, .. } => intensity * ftp,
        Intent::Break => match wheel {
            Some(w) => follow(w),
            None if super::pack::swinging(r) => 0.6 * ftp,
            None => 1.02 * ftp,
        },
        Intent::Sprint { .. } => r.profile.peak,
        Intent::Escort { rider, slot } => {
            let target = &race.riders[rider];
            let ahead = (r.dist - target.dist) as f32;
            let wanted = FOLLOW_GAP_M * (slot as f32 + 1.0);
            let v_target = target.speed + ((wanted - ahead) * 0.5).clamp(-2.0, 2.0);
            required_power(r, v_target, grade)
        }
        Intent::Wait { .. } => 0.35 * ftp,
        Intent::Mark { rider, .. } => {
            let target = &race.riders[rider];
            if wheel == Some(rider) {
                follow(rider)
            } else {
                // Close the gap at once: that's what marking is.
                let gap = (target.dist - r.dist) as f32;
                let v_target = target.speed + ((gap - FOLLOW_GAP_M) * 0.4).clamp(-1.0, 3.0);
                required_power(r, v_target, grade)
            }
        }
    };
    // On climbs, nobody but sprinters and attackers rides beyond what they
    // can hold to the top: they let the wheel go instead of blowing up.
    let paced = match r.intent {
        Intent::Sprint { .. } | Intent::Attack { .. } | Intent::Mark { .. } => p,
        _ => p.min(r.limit_w),
    };
    paced.clamp(0.0, r.energy.max_power(r.profile.peak))
}

/// Animation cadence for a bot.
pub fn cadence(r: &Rider, grade: f32) -> f32 {
    if r.power < 15.0 {
        return 0.0;
    }
    let personal = (r.id % 5) as f32 * 1.5;
    match r.intent {
        Intent::Sprint { .. } => 108.0 + personal,
        _ if grade > 0.04 => 80.0 + personal,
        _ => 88.0 + personal,
    }
}
