//! Watch mode: sit back while the whole field races. Every rider is a bot
//! (a bot takes your slot too). The camera stays on the rider you choose,
//! whatever happens around them: click a rider in the race order, Tab
//! moves to the next group, ↑/↓ to the rider ahead or behind. A TV director (D) can pick the riders instead. On auto speed the
//! quiet stretches run fast-forward and the action plays at real speed.

use bevy::prelude::*;

use ridecore::race::Race;
use ridecore::race::ai::Intent;

use crate::camera::CameraRig;
use crate::{RaceRes, Screen};

/// Fixed playback speeds, on keys 1 to 4.
pub const SPEEDS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
/// Auto speed while nothing much happens.
const CRUISE: f32 = 4.0;
/// The finale, played at real speed, starts this far from the line.
const FINALE_M: f64 = 5000.0;
/// The director holds a shot this long (race seconds)…
const SHOT_S: f32 = 45.0;
/// …and cuts to an attack only after this long on a rider.
const ATTACK_CUT_S: f32 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Speed {
    /// Fast through the quiet parts, real speed for the action.
    Auto,
    Fixed(f32),
}

pub struct Watch {
    pub speed: Speed,
    /// Playback rate now: it eases towards the speed asked for, so the
    /// action doesn't lurch when auto speed changes.
    rate: f32,
    /// The director picks who to follow. Off, the camera stays on the rider
    /// chosen last.
    pub director: bool,
    /// Race seconds on the rider being followed.
    on_rider_s: f32,
    /// The rider the camera was last placed on.
    shown: Option<usize>,
}

impl Default for Watch {
    fn default() -> Self {
        Self {
            speed: Speed::Auto,
            rate: 1.0,
            director: false,
            on_rider_s: 0.0,
            shown: None,
        }
    }
}

impl Watch {
    /// Race seconds per real second right now.
    pub fn rate(&self) -> f32 {
        self.rate
    }

    fn target_rate(&self, race: &Race) -> f32 {
        match self.speed {
            Speed::Fixed(x) => x,
            Speed::Auto if action(race) => 1.0,
            Speed::Auto => CRUISE,
        }
    }

    /// Follows `rider` from now on (a choice of yours: the director stops).
    pub fn follow(&mut self, race: &mut Race, rider: usize) {
        self.director = false;
        race.watch(rider);
    }

    /// Follows a rider in the group `step` groups back down the road
    /// (negative: up the road).
    pub fn change_group(&mut self, race: &mut Race, step: i32) {
        let n = race.groups.len() as i32;
        if n == 0 {
            return;
        }
        let g = (race.group_of(race.human) as i32 + step).rem_euclid(n) as usize;
        let rider = in_group(race, g);
        self.follow(race, rider);
    }

    /// Follows the rider next behind (`step` 1) or ahead (-1) on the road.
    pub fn change_rider(&mut self, race: &mut Race, step: i32) {
        let mut order: Vec<usize> = (0..race.riders.len())
            .filter(|&i| !race.riders[i].finished())
            .collect();
        order.sort_by(|&a, &b| race.riders[b].dist.total_cmp(&race.riders[a].dist));
        let Some(k) = order.iter().position(|&i| i == race.human) else {
            return;
        };
        let k = (k as i32 + step).clamp(0, order.len() as i32 - 1) as usize;
        self.follow(race, order[k]);
    }

    /// The director's pick: who to follow now, if it's time for a new shot.
    fn direct(&self, race: &Race) -> Option<usize> {
        let current = race.human;
        if race.riders[current].finished() || race.groups.is_empty() {
            return None;
        }
        // An attack, once the shot has run a while.
        let attacking = |i: usize| matches!(race.riders[i].intent, Intent::Attack { .. });
        if self.on_rider_s > ATTACK_CUT_S
            && !attacking(current)
            && let Some(a) = (0..race.riders.len())
                .filter(|&i| attacking(i))
                .max_by(|&a, &b| race.riders[a].dist.total_cmp(&race.riders[b].dist))
        {
            return Some(a);
        }
        if self.on_rider_s < SHOT_S {
            return None;
        }
        // In turn the front of the race and the bunch; the front in the
        // finale.
        let head = race.groups[0].riders[0];
        let g = race.group_of(current);
        let peloton = race.peloton();
        let next = if race.remaining_m(head) < FINALE_M || g != 0 {
            0
        } else {
            peloton
        };
        (next != g).then(|| in_group(race, next))
    }
}

/// A rider a few places back in group `g`, so the shot from behind shows
/// the front of the group.
pub fn in_group(race: &Race, g: usize) -> usize {
    let riders = &race.groups[g].riders;
    riders[(riders.len() / 4).min(3)]
}

/// Something worth seeing at real speed: an attack, a sprint, the finale.
fn action(race: &Race) -> bool {
    if !race.started() || !race.finish.is_empty() {
        return true;
    }
    let Some(&head) = race.groups.first().and_then(|g| g.riders.first()) else {
        return true;
    };
    race.remaining_m(head) < FINALE_M
        || race
            .riders
            .iter()
            .any(|r| matches!(r.intent, Intent::Attack { .. } | Intent::Sprint { .. }))
}

/// Not watching: the systems for a ridden race run.
pub fn riding(res: Option<Res<RaceRes>>) -> bool {
    res.is_none_or(|r| r.watch.is_none())
}

fn watching(res: Option<Res<RaceRes>>) -> bool {
    res.is_some_and(|r| r.watch.is_some())
}

pub struct WatchPlugin;

impl Plugin for WatchPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (keys, direct)
                .chain()
                .before(crate::advance_race)
                .run_if(in_state(Screen::Racing))
                .run_if(watching),
        );
    }
}

/// Tab (Shift+Tab) the next group back (up the road), ↑/↓ the rider ahead
/// or behind, D the director, 0 auto speed and 1 to 4 fixed speeds.
fn keys(keys: Res<ButtonInput<KeyCode>>, mut res: ResMut<RaceRes>) {
    let res = &mut *res;
    let Some(watch) = res.watch.as_mut() else {
        return;
    };
    let race = &mut res.race;
    if keys.just_pressed(KeyCode::Tab) {
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        watch.change_group(race, if shift { -1 } else { 1 });
    }
    // Up and down the race order, as listed.
    if keys.just_pressed(KeyCode::ArrowDown) {
        watch.change_rider(race, 1);
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        watch.change_rider(race, -1);
    }
    if keys.just_pressed(KeyCode::KeyD) {
        watch.director = !watch.director;
    }
    if keys.just_pressed(KeyCode::Digit0) {
        watch.speed = Speed::Auto;
    }
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
    ];
    for (key, speed) in digits.into_iter().zip(SPEEDS) {
        if keys.just_pressed(key) {
            watch.speed = Speed::Fixed(speed);
        }
    }
}

/// Eases the playback rate, lets the director pick when it's on, and cuts
/// the camera to a newly chosen rider.
fn direct(time: Res<Time>, mut res: ResMut<RaceRes>, mut rig: ResMut<CameraRig>) {
    let res = &mut *res;
    let Some(watch) = res.watch.as_mut() else {
        return;
    };
    let race = &mut res.race;
    if res.paused {
        return;
    }
    let dt = time.delta_secs();
    let target = watch.target_rate(race);
    watch.rate += (target - watch.rate) * (1.0 - (-dt * 2.5).exp());
    watch.on_rider_s += dt * watch.rate;
    if watch.director
        && let Some(rider) = watch.direct(race)
    {
        race.watch(rider);
    }
    if watch.shown != Some(race.human) {
        watch.shown = Some(race.human);
        watch.on_rider_s = 0.0;
        rig.cut();
    }
}
