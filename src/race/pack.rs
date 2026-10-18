//! Pack dynamics: slipstreams, wheel choice, lateral steering, keeping
//! riders from overlapping, and splitting the field into groups.

use super::ai::Intent;
use super::{Group, ROAD_HALF_WIDTH, Race, Rider};

/// A slipstream is strongest within this distance behind a wheel...
const DRAFT_FULL_M: f32 = 1.5;
/// ...and gone beyond this.
const DRAFT_REACH_M: f32 = 10.0;
/// Drag reduction directly on one wheel.
const DRAFT_ONE_WHEEL: f32 = 0.33;
/// Lateral offset at which a wheel no longer shelters.
const DRAFT_WIDTH_M: f32 = 1.0;
/// Extra shelter per rider ahead deeper inside a bunch, capped: riders in
/// the middle of a peloton feel a fraction of the drag on its leader
/// (Blocken et al., 2018), more than any single wheel gives.
const PACK_BONUS_PER_RIDER: f32 = 0.025;
const PACK_BONUS_MAX: f32 = 0.27;
pub const DRAFT_MAX: f32 = 0.6;

/// Center-to-center spacing that stands for one bike length.
pub const MIN_GAP_M: f32 = 1.8;
/// Riders closer than this laterally are in the same line.
const BODY_WIDTH_M: f32 = 0.6;
/// Lateral steering speed in m/s.
const STEER_SPEED: f32 = 1.2;
/// A gap of more than this many seconds splits groups.
const GROUP_SPLIT_S: f32 = 1.0;
/// Gaps are timed at no less than this speed, so a slow start or a steep
/// climb doesn't split a bunch over a few meters.
const GROUP_SPEED_FLOOR: f32 = 8.0;
/// Seconds on the front before a rider without orders swings off...
pub const SWING_OFF_AFTER_S: f32 = 20.0;
/// Inside this distance to the line nobody swings off after a turn.
const NO_TURNS_M: f64 = 1500.0;
/// Turn length of riders pulling a chase or a lead-out.
const PULL_TURN_S: f32 = 40.0;
/// ...and how long they ease up for. If nobody comes through in that time
/// (e.g. a solo rider) they simply carry on.
const SWING_DURATION_S: f32 = 6.0;

pub fn update_draft(riders: &mut [Rider]) {
    let draft: Vec<f32> = (0..riders.len())
        .map(|i| {
            let me = &riders[i];
            let mut best: f32 = 0.0;
            let mut ahead_in_pack: u32 = 0;
            for (j, other) in riders.iter().enumerate() {
                let dd = (other.dist - me.dist) as f32;
                if j == i || dd <= 0.0 || dd > 20.0 {
                    continue;
                }
                let dy = (other.lateral - me.lateral).abs();
                if dy < 3.0 {
                    ahead_in_pack += 1;
                }
                if dd > DRAFT_REACH_M || dy > DRAFT_WIDTH_M {
                    continue;
                }
                let lon = if dd <= DRAFT_FULL_M {
                    1.0
                } else {
                    1.0 - (dd - DRAFT_FULL_M) / (DRAFT_REACH_M - DRAFT_FULL_M)
                };
                let lat = 1.0 - dy / DRAFT_WIDTH_M;
                best = best.max(DRAFT_ONE_WHEEL * lon * lat);
            }
            // The wheel itself is already counted in `best`.
            let others = ahead_in_pack.saturating_sub(1) as f32;
            let bonus = if best > 0.1 {
                (others * PACK_BONUS_PER_RIDER).min(PACK_BONUS_MAX)
            } else {
                0.0
            };
            (best + bonus).min(DRAFT_MAX)
        })
        .collect();
    for (r, d) in riders.iter_mut().zip(draft) {
        r.draft = d;
    }
}

/// A rider must be at least this far ahead to count as a wheel.
const MIN_WHEEL_AHEAD_M: f32 = 1.0;
/// A wheel more than this far behind its own wheel is losing contact.
const LOSING_CONTACT_M: f32 = 4.0;

/// The wheel each rider follows: the closest rider ahead in its group, with a
/// preference for the same line and for teammates (which forms team trains).
/// When that wheel is losing contact, riders go around it and follow the
/// rider ahead of it instead, so one dropped rider doesn't drag a line off.
pub fn wheels(riders: &[Rider], groups: &[Group]) -> Vec<Option<usize>> {
    let direct = nearest_wheels(riders, groups);
    (0..riders.len())
        .map(|i| {
            let w = direct[i]?;
            match direct[w] {
                Some(ww)
                    if !riders[i].human
                        && ((riders[ww].dist - riders[w].dist) as f32) > LOSING_CONTACT_M =>
                {
                    Some(ww)
                }
                _ => Some(w),
            }
        })
        .collect()
}

fn nearest_wheels(riders: &[Rider], groups: &[Group]) -> Vec<Option<usize>> {
    let mut group_of = vec![usize::MAX; riders.len()];
    for (g, grp) in groups.iter().enumerate() {
        for &r in &grp.riders {
            group_of[r] = g;
        }
    }
    (0..riders.len())
        .map(|i| {
            let me = &riders[i];
            riders
                .iter()
                .enumerate()
                // A rider swinging off the front is not a wheel to follow.
                .filter(|&(j, o)| {
                    j != i && group_of[j] == group_of[i] && !o.finished() && !swinging(o)
                })
                .filter_map(|(j, o)| {
                    // Someone level with you is beside you, not a wheel.
                    let dd = (o.dist - me.dist) as f32;
                    if dd <= MIN_WHEEL_AHEAD_M || dd > 40.0 {
                        return None;
                    }
                    let mut score = dd;
                    if (o.lateral - me.lateral).abs() > 1.5 {
                        score += 4.0;
                    }
                    if o.team == me.team && dd < 12.0 {
                        score -= 3.0;
                    }
                    // Riders latch on to their team's train.
                    if o.team == me.team && matches!(o.intent, Intent::Pull { .. }) {
                        score -= 10.0;
                    }
                    Some((j, score))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(j, _)| j)
        })
        .collect()
}

/// How long a rider stays on the front before swinging off: riders sharing
/// the work in a chase or a lead-out take short, hard turns.
fn turn_s(r: &Rider) -> Option<f32> {
    match r.intent {
        Intent::Sit | Intent::Break => Some(SWING_OFF_AFTER_S),
        Intent::Pull { .. } => Some(PULL_TURN_S),
        _ => None,
    }
}

pub fn swinging(r: &Rider) -> bool {
    turn_s(r).is_some_and(|turn| (turn..turn + SWING_DURATION_S).contains(&r.front_time))
}

/// Personal line preference so the bunch rides several abreast: riders sit
/// up to ±0.75 m beside the wheel they follow, keeping half its shelter.
fn lane_bias(id: usize) -> f32 {
    ((id * 37) % 7) as f32 * 0.25 - 0.75
}

/// Where to go to overtake a wheel: the side with more road.
fn pass_side(me: f32, wheel: f32) -> f32 {
    let left = wheel - 1.0;
    let right = wheel + 1.0;
    let prefer_left = me <= wheel;
    if (prefer_left && left > -ROAD_HALF_WIDTH) || right >= ROAD_HALF_WIDTH {
        left
    } else {
        right
    }
}

pub fn steer(race: &mut Race, wheels: &[Option<usize>], h: f32) {
    let targets: Vec<f32> = (0..race.riders.len())
        .map(|i| {
            let me = &race.riders[i];
            if let Intent::Escort { rider, .. } = me.intent {
                return race.riders[rider].lateral;
            }
            match wheels[i] {
                Some(w) => {
                    let wheel = &race.riders[w];
                    let dd = (wheel.dist - me.dist) as f32;
                    let train =
                        wheel.team == me.team && matches!(wheel.intent, Intent::Pull { .. });
                    let pushing = matches!(
                        me.intent,
                        Intent::Pull { .. } | Intent::Attack { .. } | Intent::Sprint { .. }
                    );
                    if me.human {
                        // The player can't steer, so they're put right on the
                        // wheel ahead and only pull out once they ride into it.
                        return if dd < MIN_GAP_M && me.speed > wheel.speed + 0.3 {
                            pass_side(me.lateral, wheel.lateral)
                        } else {
                            wheel.lateral
                        };
                    }
                    let faster = me.speed > wheel.speed + 0.4;
                    if dd < 6.0 && ((pushing && !train) || faster) {
                        pass_side(me.lateral, wheel.lateral)
                    } else {
                        wheel.lateral + lane_bias(me.id)
                    }
                }
                None if swinging(me) => {
                    // Swing off the front so the next rider comes through.
                    if me.lateral > 0.0 {
                        me.lateral - 1.8
                    } else {
                        me.lateral + 1.8
                    }
                }
                None => me.lateral,
            }
        })
        .collect();
    for (i, target) in targets.into_iter().enumerate() {
        let finale = race.remaining_m(i) < NO_TURNS_M;
        let r = &mut race.riders[i];
        let speed = if r.human {
            2.0 * STEER_SPEED
        } else {
            STEER_SPEED
        };
        let step = (target - r.lateral).clamp(-speed * h, speed * h);
        r.lateral = (r.lateral + step).clamp(-ROAD_HALF_WIDTH, ROAD_HALF_WIDTH);
        if wheels[i].is_none() && !r.finished() {
            // A turn that hurts is cut short. In the finale nobody takes
            // turns: riders on the front stay there until they're spent.
            let turn = turn_s(r).unwrap_or(SWING_OFF_AFTER_S);
            let tired = if finale {
                r.energy.fraction() < 0.1
            } else {
                r.front_time > 5.0 && r.front_time < turn && r.energy.fraction() < 0.5
            };
            r.front_time = if tired {
                turn
            } else if finale {
                0.0
            } else {
                r.front_time + h
            };
            if r.front_time > turn + SWING_DURATION_S {
                r.front_time = 0.0;
            }
        } else {
            r.front_time = 0.0;
        }
    }
}

/// Bots cannot ride through each other: a rider right behind another in the
/// same line can't go faster than it and is eased sideways towards open
/// road, so it can come past. The player passes through, as in Zwift, so
/// their power is never wasted, and so do riders moving up in the finale,
/// which stands for coming up the side of a bunch filling the road.
pub fn separate(riders: &mut [Rider], h: f32) {
    let mut order: Vec<usize> = (0..riders.len()).collect();
    order.sort_by(|&a, &b| riders[b].dist.total_cmp(&riders[a].dist));
    for a in 0..order.len() {
        let front = order[a];
        for &back in &order[a + 1..] {
            let dd = (riders[front].dist - riders[back].dist) as f32;
            if dd > MIN_GAP_M {
                break;
            }
            if riders[back].human
                || riders[front].human
                || riders[back].finished()
                || riders[back].move_up
            {
                continue;
            }
            let dy = riders[back].lateral - riders[front].lateral;
            if dy.abs() < BODY_WIDTH_M {
                riders[back].speed = riders[back].speed.min(riders[front].speed);
                let away = if dy.abs() > 0.01 {
                    dy.signum()
                } else if riders[back].lateral < 0.0 {
                    1.0
                } else {
                    -1.0
                };
                let room = if (riders[back].lateral + away * 0.5).abs() > ROAD_HALF_WIDTH {
                    -away
                } else {
                    away
                };
                riders[back].lateral = (riders[back].lateral + room * STEER_SPEED * h)
                    .clamp(-ROAD_HALF_WIDTH, ROAD_HALF_WIDTH);
            }
        }
    }
}

/// Splits unfinished riders into groups wherever a gap exceeds about a second.
pub fn find_groups(riders: &[Rider]) -> Vec<Group> {
    let mut order: Vec<usize> = (0..riders.len())
        .filter(|&i| !riders[i].finished())
        .collect();
    order.sort_by(|&a, &b| riders[b].dist.total_cmp(&riders[a].dist));
    let mut groups: Vec<Group> = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let split = k == 0 || {
            let prev = &riders[order[k - 1]];
            let gap_m = (prev.dist - riders[i].dist) as f32;
            gap_m / riders[i].speed.max(GROUP_SPEED_FLOOR) > GROUP_SPLIT_S
        };
        if split {
            groups.push(Group {
                riders: Vec::new(),
                gap_s: 0.0,
                tempo_wkg: 0.0,
            });
        }
        groups.last_mut().expect("pushed above").riders.push(i);
    }
    if let Some(head) = groups.first().map(|g| riders[g.riders[0]].dist) {
        for g in &mut groups {
            let r = &riders[g.riders[0]];
            g.gap_s = (head - r.dist) as f32 / r.speed.max(GROUP_SPEED_FLOOR);
        }
    }
    groups
}

/// The nearest unfinished rider ahead of each rider within `reach` meters,
/// in any group (used to bridge small gaps).
pub fn nearest_ahead(riders: &[Rider], reach: f32) -> Vec<Option<usize>> {
    let mut order: Vec<usize> = (0..riders.len())
        .filter(|&i| !riders[i].finished())
        .collect();
    order.sort_by(|&a, &b| riders[b].dist.total_cmp(&riders[a].dist));
    let mut out = vec![None; riders.len()];
    for k in 1..order.len() {
        let (front, me) = (order[k - 1], order[k]);
        if ((riders[front].dist - riders[me].dist) as f32) <= reach {
            out[me] = Some(front);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::field::{Profile, RiderKind};

    fn rider(id: usize, dist: f64, lateral: f32) -> Rider {
        let profile = Profile {
            name: format!("R{id}"),
            kind: RiderKind::Domestique,
            kg: 70.0,
            height_cm: 178.0,
            ftp: 280.0,
            peak: 1000.0,
            w_prime: 20_000.0,
            aggression: 0.5,
        };
        Rider {
            id,
            team: id,
            bike: profile.bike(),
            energy: profile.energy(),
            profile,
            human: false,
            dist,
            lateral,
            speed: 11.0,
            accel: 0.0,
            power: 250.0,
            cadence: 90.0,
            draft: 0.0,
            intent: Intent::Sit,
            finish_time: None,
            front_time: 0.0,
            limit_w: f32::MAX,
            move_up: false,
        }
    }

    #[test]
    fn draft_is_strongest_close_behind_and_fades() {
        let mut rs = vec![
            rider(0, 100.0, 0.0),
            rider(1, 98.5, 0.0),
            rider(2, 90.0, 0.0),
            rider(3, 98.5, 2.0),
        ];
        update_draft(&mut rs);
        assert_eq!(rs[0].draft, 0.0);
        assert!((rs[1].draft - 0.33).abs() < 0.01, "{}", rs[1].draft);
        assert!(rs[2].draft > 0.0 && rs[2].draft < 0.1, "{}", rs[2].draft);
        assert_eq!(rs[3].draft, 0.0, "two meters to the side gives no shelter");
    }

    #[test]
    fn deep_in_the_bunch_shelters_more() {
        let mut rs: Vec<Rider> = (0..12)
            .map(|i| rider(i, 100.0 - (i / 3) as f64 * 2.0, (i % 3) as f32 * 0.8))
            .collect();
        update_draft(&mut rs);
        assert!(
            rs[11].draft > rs[3].draft,
            "{} vs {}",
            rs[11].draft,
            rs[3].draft
        );
    }

    #[test]
    fn groups_split_on_time_gaps() {
        let rs = vec![
            rider(0, 1000.0, 0.0),
            rider(1, 998.0, 0.0),
            rider(2, 900.0, 0.0),
            rider(3, 897.0, 0.0),
        ];
        let g = find_groups(&rs);
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].riders, vec![2, 3]);
        assert!((g[1].gap_s - 100.0 / 11.0).abs() < 0.01);
    }

    #[test]
    fn a_blocked_bot_slows_and_moves_aside() {
        let mut rs = vec![rider(0, 100.0, 0.0), rider(1, 99.0, 0.1)];
        rs[1].speed = 13.0;
        separate(&mut rs, 0.1);
        assert_eq!(rs[1].speed, 11.0);
        assert_eq!(rs[1].dist, 99.0, "no teleporting");
        assert!(rs[1].lateral > 0.1, "eased towards open road");
    }
}
