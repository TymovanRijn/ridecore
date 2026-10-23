//! The player's input: live trainer data (or keyboard watts in test mode),
//! and steering the trainer's resistance from the race.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use crossbeam_channel::Receiver;
use tokio::sync::mpsc::UnboundedSender;

use ridecore::ble::{self, TrainerEvent};
use ridecore::ftms::ControlRequest;
use ridecore::sensors::LiveData;
use ridecore::trainer::{
    ControlTuning, Mode, TRAINER_CRR, TRAINER_CW, TrainerController, VirtualGears,
    draft_relief_pct, shifts_ignored,
};

use crate::{RaceRes, Screen};
use ridecore::i18n::t;
use ridecore::tf;

/// Zwift sends simulation parameters at most every 0.75 s.
const CONTROL_INTERVAL: Duration = Duration::from_millis(500);
/// After a shift the new resistance goes out almost at once (a trainer that
/// shifts itself gets the gear on the very next frame, as in Zwift).
const SHIFT_INTERVAL: Duration = Duration::from_millis(120);
const NATIVE_SHIFT_INTERVAL: Duration = Duration::ZERO;

/// Watches runs of shifts, to notice when the trainer stops taking its
/// commands (the gears change on screen but the legs feel nothing).
#[derive(Default)]
struct ShiftCheck {
    /// Power and cadence over the last few seconds.
    recent: std::collections::VecDeque<(Instant, f32, f32)>,
    /// The run of shifts going on: the ratio, power and cadence before it.
    burst: Option<(f32, f32, f32)>,
}

impl ShiftCheck {
    fn record(&mut self, power: f32, cadence: f32) {
        let now = Instant::now();
        self.recent.push_back((now, power, cadence));
        while self
            .recent
            .front()
            .is_some_and(|&(t, ..)| now - t > Duration::from_secs(12))
        {
            self.recent.pop_front();
        }
    }

    /// Mean power and cadence between two moments.
    fn mean(&self, from: Instant, to: Instant) -> Option<(f32, f32)> {
        let s: Vec<_> = self
            .recent
            .iter()
            .filter(|&&(t, ..)| t >= from && t <= to)
            .collect();
        (s.len() >= 2).then(|| {
            let n = s.len() as f32;
            (
                s.iter().map(|x| x.1).sum::<f32>() / n,
                s.iter().map(|x| x.2).sum::<f32>() / n,
            )
        })
    }
}

#[derive(Resource)]
pub struct Trainer {
    events: Receiver<TrainerEvent>,
    commands: UnboundedSender<ControlRequest>,
    pub live: LiveData,
    pub status: String,
    pub name: Option<String>,
    pub controller: TrainerController,
    last_control: Instant,
    /// Connected Zwift Click / Play / Ride, if any.
    pub shifter: Option<String>,
    pub shifter_battery: Option<u8>,
    /// Counts every shift, so sound and HUD can react to new ones.
    pub shifts: u32,
    pub last_shift: Option<Instant>,
    shift_pending: bool,
    /// Shifts made without virtual gears (in a workout they set intensity).
    pub nudges: i32,
    /// The grade you feel right now (difficulty and shelter included),
    /// for the HUD.
    pub felt_grade: f32,
    /// The trainer shifts itself over Zwift's protocol (KICKR CORE Zwift
    /// One and friends); RideCore then only tells it the road and the gear.
    pub native_shifting: bool,
    /// What the trainer was told and what it reported, twice a second, for
    /// finding out what went wrong after a ride.
    log: Option<std::io::BufWriter<std::fs::File>>,
    log_start: Instant,
    check: ShiftCheck,
}

impl Trainer {
    fn send(&self, req: ControlRequest) {
        let _ = self.commands.send(req);
    }

    /// Moves one virtual gear: +1 harder, -1 easier.
    pub fn shift(&mut self, dir: i8) {
        let Some(g) = self.controller.gears.as_mut() else {
            self.nudges += dir as i32;
            return;
        };
        let before = g.index;
        if self.check.burst.is_none() {
            let now = Instant::now();
            if let Some((p, c)) = self.check.mean(now - Duration::from_secs(3), now) {
                self.check.burst = Some((g.ratio(), p, c));
            }
        }
        if dir > 0 {
            g.shift_up();
        } else {
            g.shift_down();
        }
        if g.index != before {
            self.shifts += 1;
            self.last_shift = Some(Instant::now());
            self.shift_pending = true;
        }
    }
}

/// What the race uses as the player's effort this frame.
#[derive(Resource, Default)]
pub struct PlayerInput {
    pub power: f32,
    pub cadence: f32,
    pub heart_rate: Option<u8>,
    /// Power averaged over the last 3 seconds, for display (as Zwift's
    /// "3s power"); the race itself rides on `power`.
    pub power_3s: f32,
    recent: std::collections::VecDeque<(f32, f32)>,
}

impl PlayerInput {
    const DISPLAY_WINDOW_S: f32 = 3.0;

    /// Adds this frame's power (held for `dt` seconds) to the 3 s average.
    fn record(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        self.recent.push_back((dt, self.power));
        let mut span: f32 = self.recent.iter().map(|&(d, _)| d).sum();
        while span > Self::DISPLAY_WINDOW_S
            && let Some(&(d, _)) = self.recent.front()
            && span - d >= Self::DISPLAY_WINDOW_S
        {
            span -= d;
            self.recent.pop_front();
        }
        let sum: f32 = self.recent.iter().map(|&(d, p)| d * p).sum();
        self.power_3s = sum / span;
    }
}

#[derive(Resource)]
pub struct InputSettings {
    /// Ride with the keyboard instead of the trainer (for testing).
    pub keyboard: bool,
    pub keyboard_watts: f32,
    /// Trainer difficulty: how much of each climb you feel, 0..1.
    pub difficulty: f32,
    pub virtual_shifting: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            keyboard: false,
            keyboard_watts: 200.0,
            difficulty: 0.5,
            virtual_shifting: false,
        }
    }
}

pub struct LinkPlugin;

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = ble::spawn(tx);
        app.insert_resource(Trainer {
            events: rx,
            commands,
            live: LiveData::default(),
            status: t("Bluetooth starten…", "Starting Bluetooth…").into(),
            name: None,
            controller: TrainerController::new(ControlTuning::default()),
            last_control: Instant::now(),
            shifter: None,
            shifter_battery: None,
            shifts: 0,
            last_shift: None,
            shift_pending: false,
            nudges: 0,
            felt_grade: 0.0,
            native_shifting: false,
            log: open_log(),
            log_start: Instant::now(),
            check: ShiftCheck::default(),
        })
        .init_resource::<PlayerInput>()
        .init_resource::<InputSettings>()
        .add_systems(Update, (poll_trainer, update_input).chain())
        .add_systems(OnEnter(Screen::Racing), reset_controller)
        .add_systems(
            Update,
            (keyboard_controls, control_trainer).run_if(in_state(Screen::Racing)),
        )
        .add_systems(OnEnter(Screen::Results), flat_road);
    }
}

/// ~/Library/Logs/RideCore-trainer.csv, one per launch (the previous one
/// is kept as .prev.csv).
fn open_log() -> Option<std::io::BufWriter<std::fs::File>> {
    use std::io::Write;
    let dir = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("RideCore-trainer.csv");
    let _ = std::fs::rename(&path, dir.join("RideCore-trainer.prev.csv"));
    let mut w = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
    writeln!(
        w,
        "t_s,event,dist_m,road_pct,felt_pct,sent_pct,mode,gear,ratio,physical_ratio,\
         trainer_kmh,cadence,power_w,power_age_s,packets_per_s,draft,game_kmh"
    )
    .ok()?;
    Some(w)
}

impl Trainer {
    fn log_line(&mut self, line: std::fmt::Arguments) {
        use std::io::Write;
        let t = self.log_start.elapsed().as_secs_f32();
        if let Some(w) = self.log.as_mut() {
            let _ = writeln!(w, "{t:.2},{line}");
            let _ = w.flush();
        }
    }
}

fn poll_trainer(
    mut trainer: ResMut<Trainer>,
    mut settings: ResMut<InputSettings>,
    session: Option<Res<crate::training::Session>>,
) {
    let t = &mut *trainer;
    while let Ok(ev) = t.events.try_recv() {
        match &ev {
            TrainerEvent::Status(_)
            | TrainerEvent::Connected(_)
            | TrainerEvent::Disconnected
            | TrainerEvent::NativeShifting(_)
            | TrainerEvent::CommandFailed(..) => {
                let text = format!("{ev:?}").replace(',', ";");
                t.log_line(format_args!("{text}"));
            }
            _ => {}
        }
        match &ev {
            TrainerEvent::Status(s) => t.status = s.clone(),
            TrainerEvent::Connected(name) => {
                t.status = tf!("Verbonden met {name}", "Connected to {name}");
                t.name = Some(name.clone());
                // A fresh connection knows nothing yet: gear and grade
                // go out again.
                t.controller.resend();
                t.check = ShiftCheck::default();
            }
            TrainerEvent::NativeShifting(on) => {
                t.native_shifting = *on;
                t.controller.native_gears = *on;
                // Like Zwift: a trainer with virtual shifting always rides
                // on virtual gears (the Cog is a single speed).
                if *on {
                    if let Some(name) = &t.name {
                        t.status = tf!(
                            "Verbonden met {name} · schakelt zelf (Zwift-protocol)",
                            "Connected to {name} · shifts itself (Zwift protocol)"
                        );
                    }
                    settings.virtual_shifting = true;
                    if t.controller.gears.is_none()
                        && session.as_ref().is_none_or(|s| s.game.is_some())
                    {
                        t.controller.gears = Some(VirtualGears::default());
                    }
                }
            }
            TrainerEvent::Disconnected => {
                t.name = None;
                t.native_shifting = false;
                t.controller.native_gears = false;
                t.status = ridecore::i18n::t(
                    "Trainer losgekoppeld, opnieuw verbinden…",
                    "Trainer disconnected, reconnecting…",
                )
                .into();
            }
            TrainerEvent::CommandFailed(op, code) => {
                t.status = tf!(
                    "Trainer weigerde commando 0x{op:02X}: {code:?}",
                    "Trainer refused command 0x{op:02X}: {code:?}"
                );
            }
            TrainerEvent::ControllerConnected(name) => {
                t.shifter = Some(name.clone());
                // A shifter only makes sense with virtual gears.
                settings.virtual_shifting = true;
                if t.controller.gears.is_none() {
                    t.controller.gears = Some(VirtualGears::default());
                }
            }
            TrainerEvent::ControllerDisconnected => {
                t.shifter = None;
                t.shifter_battery = None;
            }
            TrainerEvent::ControllerBattery(b) => t.shifter_battery = Some(*b),
            TrainerEvent::Shift(dir) => t.shift(*dir),
            TrainerEvent::ControllerButton(id) => {
                t.status = tf!(
                    "Onbekende knop op je controller (0x{id:X})",
                    "Unknown button on your controller (0x{id:X})"
                );
            }
            _ => {}
        }
        t.live.apply(&ev);
    }
    t.live.expire();
}

fn update_input(
    trainer: Res<Trainer>,
    settings: Res<InputSettings>,
    session: Option<Res<crate::training::Session>>,
    sprints: Option<Res<crate::sprint::SprintSession>>,
    res: Option<Res<RaceRes>>,
    time: Res<Time>,
    mut input: ResMut<PlayerInput>,
) {
    // In test mode a workout rides itself, the way ERG would hold you,
    // and so do sprints.
    if settings.keyboard
        && let (Some(s), Some(res)) = (sprints.as_ref(), res.as_ref())
    {
        input.power = s.test_power(res, settings.keyboard_watts);
        input.cadence = if input.power > 600.0 { 112.0 } else { 90.0 };
        input.heart_rate = None;
    } else if settings.keyboard
        && let Some(w) = session.as_ref().and_then(|s| s.target_w())
    {
        let t = time.elapsed_secs();
        input.power = w + 4.0 * (t * 1.7).sin() + 2.0 * (t * 4.1).sin();
        input.cadence = 90.0;
        input.heart_rate = None;
    } else if settings.keyboard {
        input.power = settings.keyboard_watts;
        input.cadence = if settings.keyboard_watts > 0.0 {
            90.0
        } else {
            0.0
        };
        input.heart_rate = None;
    } else {
        input.power = trainer.live.power_w;
        input.cadence = trainer.live.cadence_rpm;
        input.heart_rate = trainer.live.heart_rate;
    }
    input.record(time.delta_secs());
}

fn reset_controller(
    mut trainer: ResMut<Trainer>,
    settings: Res<InputSettings>,
    session: Option<Res<crate::training::Session>>,
) {
    trainer.controller = TrainerController::new(ControlTuning::default());
    trainer.controller.native_gears = trainer.native_shifting;
    // In ERG the trainer sets the load; the shifter sets the intensity. A
    // workout game has no ERG, so it always rides on the gears.
    let game = session.as_ref().is_some_and(|s| s.game.is_some());
    trainer.controller.gears =
        ((settings.virtual_shifting && session.is_none()) || game).then(VirtualGears::default);
    trainer.nudges = 0;
    trainer.last_control = Instant::now();
}

/// ↑/↓ change keyboard watts (Shift for big steps); +/- shift virtual gears.
fn keyboard_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut settings: ResMut<InputSettings>,
    mut trainer: ResMut<Trainer>,
) {
    let step = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        50.0
    } else {
        10.0
    };
    if settings.keyboard {
        if keys.just_pressed(KeyCode::ArrowUp) {
            settings.keyboard_watts = (settings.keyboard_watts + step).min(2000.0);
        }
        if keys.just_pressed(KeyCode::ArrowDown) {
            settings.keyboard_watts = (settings.keyboard_watts - step).max(0.0);
        }
    }
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        trainer.shift(1);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        trainer.shift(-1);
    }
}

/// Sends the grade a second ahead, scaled by difficulty on climbs and made
/// lighter by the shelter you are getting from the wheels in front.
fn control_trainer(
    mut trainer: ResMut<Trainer>,
    settings: Res<InputSettings>,
    input: Res<PlayerInput>,
    res: Res<RaceRes>,
    session: Option<Res<crate::training::Session>>,
) {
    let interval = if trainer.shift_pending && trainer.native_shifting {
        NATIVE_SHIFT_INTERVAL
    } else if trainer.shift_pending {
        SHIFT_INTERVAL
    } else {
        CONTROL_INTERVAL
    };
    if settings.keyboard || trainer.name.is_none() || trainer.last_control.elapsed() < interval {
        return;
    }
    let dt = trainer.last_control.elapsed().as_secs_f32();
    trainer.last_control = Instant::now();
    trainer.shift_pending = false;
    let race = &res.race;
    let me = race.player();
    // Watching a race: the trainer stays flat.
    let target = if race.started() && !res.paused && res.watch.is_none() {
        let lookahead = trainer.controller.tuning.grade_lookahead_s;
        let road = race
            .course
            .smooth_grade_pct(me.dist + (me.speed * lookahead) as f64);
        // Like Zwift: with virtual shifting the climb is felt in full and
        // the gears make it rideable; difficulty only applies without gears.
        let difficulty = if trainer.controller.gears.is_some() {
            1.0
        } else {
            settings.difficulty
        };
        let scaled = if road > 0.0 { road * difficulty } else { road };
        scaled - draft_relief_pct(&me.bike, me.speed, me.draft)
    } else {
        0.0
    };
    let speed = trainer.live.trainer_speed_mps;
    let cadence = trainer.live.cadence_rpm;
    let bike = me.bike;
    let t = &mut *trainer;
    if let Some(g) = t.controller.gears.as_mut() {
        g.observe(cadence, speed, dt);
    }
    // A workout holds its watts in ERG; between sessions the road is flat.
    // A workout game rides a flat road on the gears: you hold the watts.
    let game = session.as_ref().is_some_and(|s| s.game.is_some());
    let mode = match session.as_ref().map(|s| s.target_w()) {
        Some(_) if game => Mode::Sim { difficulty: 1.0 },
        Some(Some(w)) if !res.paused && race.started() => Mode::Erg {
            target_w: w.round() as u32,
        },
        Some(_) => Mode::Sim { difficulty: 0.0 },
        None => Mode::Sim { difficulty: 1.0 },
    };
    let target = if session.is_some() { 0.0 } else { target };
    t.felt_grade = target;
    let mut sent = t
        .controller
        .tick(mode, target, input.power, speed, &bike, dt);
    if let Some(req) = sent {
        t.send(req);
        // After a shift the grade follows straight away.
        if matches!(req, ControlRequest::SetGear { .. })
            && let Some(next) = t
                .controller
                .tick(mode, target, input.power, speed, &bike, 0.0)
        {
            t.send(next);
            sent = Some(next);
        }
    }
    let kind = if t.native_shifting { "zwift-" } else { "" };
    let (mode_name, sent_pct) = match (mode, sent) {
        (Mode::Erg { target_w }, _) => (format!("{kind}erg{target_w}"), String::new()),
        (_, Some(ControlRequest::SetSimulation { grade_pct, .. })) => {
            (format!("{kind}sim"), format!("{grade_pct:.2}"))
        }
        _ => (format!("{kind}sim"), String::new()),
    };
    let (gear, ratio, physical) = t
        .controller
        .gears
        .as_ref()
        .map(|g| (g.index as i32 + 1, g.ratio(), g.physical_ratio))
        .unwrap_or((0, 0.0, 0.0));
    let road_now = race.course.smooth_grade_pct(me.dist);
    check_shifts(t, input.power, cadence, mode, race.started() && !res.paused);
    let age = t.live.power_age_s().unwrap_or(-1.0);
    let pps = t.live.packets_per_s();
    t.log_line(format_args!(
        "tick,{:.0},{road_now:.2},{target:.2},{sent_pct},{mode_name},{gear},{ratio:.2},{physical:.2},\
         {:.1},{cadence:.0},{:.0},{age:.2},{pps:.1},{:.2},{:.1}",
        me.dist,
        speed * 3.6,
        input.power,
        me.draft,
        me.speed * 3.6,
    ));
}

/// A few seconds after a run of shifts, checks the legs felt it; if the
/// trainer ignored it (it stopped taking commands, as a KICKR CORE did after
/// a too steep descent), reconnects to it.
fn check_shifts(t: &mut Trainer, power: f32, cadence: f32, mode: Mode, riding: bool) {
    t.check.record(power, cadence);
    let Some(last) = t.last_shift else { return };
    if t.check.burst.is_none() || last.elapsed() < Duration::from_millis(4500) {
        return;
    }
    let Some((ratio_before, p0, c0)) = t.check.burst.take() else {
        return;
    };
    let after = t.check.mean(
        last + Duration::from_millis(1500),
        last + Duration::from_millis(4500),
    );
    let Some(ratio_after) = t.controller.gears.as_ref().map(VirtualGears::ratio) else {
        return;
    };
    let Some((p1, c1)) = after else { return };
    if t.native_shifting
        && riding
        && matches!(mode, Mode::Sim { .. })
        && shifts_ignored(ratio_before, ratio_after, (p0, p1), (c0, c1))
    {
        t.log_line(format_args!(
            "ignored shifts {ratio_before:.2}->{ratio_after:.2} power {p0:.0}->{p1:.0}: reconnecting"
        ));
        t.status = ridecore::i18n::t(
            "Trainer negeerde het schakelen: opnieuw verbinden…",
            "Trainer ignored the shifts: reconnecting…",
        )
        .into();
        t.send(ControlRequest::Reconnect);
    }
}

fn flat_road(trainer: Res<Trainer>) {
    if trainer.name.is_some() {
        trainer.send(ControlRequest::SetSimulation {
            wind_mps: 0.0,
            grade_pct: 0.0,
            crr: TRAINER_CRR,
            cw: TRAINER_CW,
        });
    }
}
