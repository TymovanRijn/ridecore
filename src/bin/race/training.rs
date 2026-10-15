//! Training mode: a structured workout ridden in ERG on a real route. The
//! trainer holds the watts; this keeps the clock, shows the interval in big
//! type with the whole session as a timeline, coaches and cheers you
//! through it, and sums it up at the end.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::fit;
use ridecore::i18n::t;
use ridecore::tf;
use ridecore::training::{
    BlockScore, Focus, Plan, Zone, best_average, ftp_from_ramp, normalized_power, peak,
    zone_seconds,
};
use ridecore::workout::Segment;

use crate::games::{Game, GameKind, Sound as GameSound};
use crate::link::{PlayerInput, Trainer};
use crate::setup::SetupForm;
use crate::sound::{RATE, SoundBank, normalize, play, tone, wav};
use crate::theme::{self, ACCENT, ACCENT_SOFT, GOOD, MUTED, TEXT, WARN, display};
use crate::{RaceRes, Screen};

/// How long a coach message stays up.
const COACH_S: f32 = 4.5;
/// Intensity steps for the Click / +/− during a workout.
const BIAS_STEP: f32 = 0.025;
/// Blocks shorter than this are "short intervals": fewer messages.
const SHORT_S: u32 = 60;

pub struct TrainingPlugin;

impl Plugin for TrainingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, build_sounds)
            .add_systems(OnEnter(Screen::Setup), |mut commands: Commands| {
                commands.remove_resource::<Session>()
            })
            .add_systems(
                Update,
                (keys, tick)
                    .chain()
                    .after(crate::advance_race)
                    .run_if(in_state(Screen::Racing))
                    .run_if(resource_exists::<Session>),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    hud.run_if(in_state(Screen::Racing)),
                    summary.run_if(in_state(Screen::Results)),
                )
                    .run_if(resource_exists::<Session>)
                    .run_if(crate::theme::ready),
            );
    }
}

/// Run condition for the race-only screens.
pub fn racing(session: Option<Res<Session>>) -> bool {
    session.is_none()
}

/// The workout being ridden.
#[derive(Resource)]
pub struct Session {
    pub plan: Plan,
    pub ftp: f32,
    /// Seconds into the workout.
    pub elapsed: f32,
    /// Intensity: 1.0 is as planned.
    pub bias: f32,
    /// Per workout segment.
    scores: Vec<BlockScore>,
    /// One sample per second.
    watts: Vec<f32>,
    heart: Vec<u8>,
    cadence: Vec<f32>,
    /// Segment indices of the intervals.
    work: Vec<usize>,
    /// Workout time when it ended, and whether that was early.
    finished: Option<f32>,
    ended_early: bool,
    pub new_ftp: Option<f32>,
    ftp_saved: bool,
    menu: bool,
    coach: Option<Coach>,
    last_segment: Option<usize>,
    last_work_end: f32,
    beeped: Option<(usize, u32)>,
    /// Ramp test: seconds the rider has been short of the step.
    short_s: f32,
    since_end: f32,
    clock: f32,
    /// Ridden as a game instead of in ERG.
    pub game: Option<Game>,
}

struct Coach {
    title: String,
    sub: String,
    color: egui::Color32,
    at: f32,
}

impl Session {
    pub fn new(plan: Plan, ftp: f32) -> Self {
        let work = plan.work_blocks();
        let scores = vec![BlockScore::default(); plan.workout.segments.len()];
        Self {
            plan,
            ftp,
            elapsed: 0.0,
            bias: 1.0,
            scores,
            watts: Vec::new(),
            heart: Vec::new(),
            cadence: Vec::new(),
            work,
            finished: None,
            ended_early: false,
            new_ftp: None,
            ftp_saved: false,
            menu: false,
            coach: None,
            last_segment: None,
            last_work_end: -1000.0,
            beeped: None,
            short_s: 0.0,
            since_end: 0.0,
            clock: 0.0,
            game: None,
        }
    }

    /// Rides the workout as `kind`: no ERG, you hold the watts yourself.
    pub fn with_game(mut self, kind: Option<GameKind>) -> Self {
        self.game = kind.map(|k| {
            let secs: Vec<u32> = self.plan.workout.segments.iter().map(|s| s.secs()).collect();
            let hardest = self
                .plan
                .workout
                .segments
                .iter()
                .filter_map(peak)
                .fold(0.6f32, f32::max);
            Game::new(k, &secs, hardest * self.ftp)
        });
        self
    }

    /// The target `t` seconds into the workout, as `target_w` would give it.
    pub fn target_at_time(&self, t: f32) -> Option<f32> {
        let x = self.plan.workout.target_at(t, self.ftp.round() as u32)?;
        let w = x.watts.map_or(self.ftp * 0.6, |w| w as f32);
        Some((w * self.bias).max(50.0))
    }

    /// Jumps ahead (for testing), as if ridden at the targets.
    pub fn skip_to(&mut self, secs: f32) {
        let ftp = self.ftp.round() as u32;
        while self.elapsed < secs {
            let Some(t) = self.plan.workout.target_at(self.elapsed, ftp) else {
                break;
            };
            let w = t.watts.unwrap_or(150) as f32;
            self.scores[t.segment].add(1.0, w * 0.99, w);
            self.watts.push(w * 0.99);
            self.heart.push(0);
            self.cadence.push(90.0);
            self.last_segment = Some(t.segment);
            self.elapsed += 1.0;
        }
        if let Some(g) = self.game.as_mut() {
            g.skip_to(self.elapsed);
        }
    }

    /// The watts the trainer should hold now, or `None` once it's over.
    pub fn target_w(&self) -> Option<f32> {
        if self.finished.is_some() {
            return None;
        }
        // Free-ride blocks: an easy endurance pace.
        self.target_at_time(self.elapsed)
    }

    fn segment(&self) -> Option<(usize, f32)> {
        let t = self
            .plan
            .workout
            .target_at(self.elapsed, self.ftp.round() as u32)?;
        Some((t.segment, t.segment_remaining_s))
    }

    /// (k, n): this is interval k of n, counting from 1.
    fn interval(&self, seg: usize) -> Option<(usize, usize)> {
        let k = self.work.iter().position(|&s| s == seg)?;
        Some((k + 1, self.work.len()))
    }

    fn target_of(&self, seg: usize) -> f32 {
        peak(&self.plan.workout.segments[seg]).unwrap_or(0.6) * self.ftp * self.bias
    }

    fn total_s(&self) -> f32 {
        self.plan.secs() as f32
    }

    fn nailed(&self) -> (usize, usize) {
        let ridden: Vec<&BlockScore> = self
            .work
            .iter()
            .map(|&s| &self.scores[s])
            .filter(|s| s.secs > 1.0)
            .collect();
        (
            ridden.iter().filter(|s| s.nailed()).count(),
            self.work.len(),
        )
    }

    fn say(&mut self, title: impl Into<String>, sub: impl Into<String>, color: egui::Color32) {
        self.coach = Some(Coach {
            title: title.into(),
            sub: sub.into(),
            color,
            at: self.clock,
        });
    }

    fn power_3s(&self, now: f32) -> f32 {
        let n = self.watts.len();
        if n == 0 {
            return now;
        }
        let k = n.min(3);
        self.watts[n - k..].iter().sum::<f32>() / k as f32
    }
}

fn zone_color(z: Zone) -> egui::Color32 {
    let [r, g, b] = z.rgb();
    egui::Color32::from_rgb(r, g, b)
}

fn focus_color(f: Focus) -> egui::Color32 {
    let [r, g, b] = f.rgb();
    egui::Color32::from_rgb(r, g, b)
}

fn mmss(s: f32) -> String {
    let s = s.max(0.0).ceil() as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn duration_text(secs: u32) -> String {
    if secs >= 120 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m} min")
        } else {
            format!("{m}:{s:02} min")
        }
    } else {
        format!("{secs} s")
    }
}

#[derive(Resource)]
struct TrainingSounds {
    beep: Handle<AudioSource>,
    go: Handle<AudioSource>,
    coin: Handle<AudioSource>,
    miss: Handle<AudioSource>,
}

fn build_sounds(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut add = |samples: Vec<f32>| {
        sources.add(AudioSource {
            bytes: std::sync::Arc::from(wav(&samples)),
        })
    };
    let mut beep = vec![0.0; (0.2 * RATE as f32) as usize];
    tone(&mut beep, 0.0, 880.0, 0.2, 0.7, 14.0);
    let mut go = vec![0.0; (0.6 * RATE as f32) as usize];
    tone(&mut go, 0.0, 1318.5, 0.6, 0.7, 5.0);
    tone(&mut go, 0.0, 659.3, 0.6, 0.35, 5.0);
    let mut coin = vec![0.0; (0.25 * RATE as f32) as usize];
    tone(&mut coin, 0.0, 1568.0, 0.08, 0.6, 30.0);
    tone(&mut coin, 0.07, 2093.0, 0.18, 0.6, 18.0);
    let mut miss = vec![0.0; (0.5 * RATE as f32) as usize];
    tone(&mut miss, 0.0, 330.0, 0.2, 0.6, 8.0);
    tone(&mut miss, 0.18, 247.0, 0.32, 0.6, 6.0);
    commands.insert_resource(TrainingSounds {
        beep: add(normalize(beep, 0.6)),
        go: add(normalize(go, 0.7)),
        coin: add(normalize(coin, 0.5)),
        miss: add(normalize(miss, 0.6)),
    });
}

/// Esc opens the menu (and pauses), P pauses, the Click or +/− set the
/// intensity.
fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut res: ResMut<RaceRes>,
    mut session: ResMut<Session>,
    mut trainer: ResMut<Trainer>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        session.menu = !session.menu;
        res.paused = session.menu;
    }
    if keys.just_pressed(KeyCode::KeyP) && !session.menu {
        res.paused = !res.paused;
    }
    let nudges = std::mem::take(&mut trainer.nudges);
    if nudges != 0 && session.finished.is_none() {
        let s = &mut *session;
        s.bias = (s.bias + nudges as f32 * BIAS_STEP).clamp(0.7, 1.3);
        let text = tf!("Intensiteit {:.0}%", "Intensity {:.0}%", s.bias * 100.0);
        let sub = if s.bias > 1.0 {
            t("Zwaarder dan gepland — stoer!", "Harder than planned — brave!")
        } else if s.bias < 1.0 {
            t(
                "Lichter dan gepland: prima op een zware dag",
                "Lighter than planned: fine on a tough day",
            )
        } else {
            t("Precies zoals gepland", "Exactly as planned")
        };
        s.say(text, sub, ACCENT);
    }
}

#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    time: Res<Time>,
    mut res: ResMut<RaceRes>,
    input: Res<PlayerInput>,
    mut session: ResMut<Session>,
    mut form: ResMut<SetupForm>,
    sounds: Res<TrainingSounds>,
    bank: Res<SoundBank>,
    mut next: ResMut<NextState<Screen>>,
) {
    let dt = time.delta_secs().min(0.1);
    let s = &mut *session;
    s.clock += dt;
    if let Some(g) = s.game.as_mut() {
        g.animate(dt);
    }
    if let Some(_end) = s.finished {
        s.since_end += dt;
        if s.since_end > 5.0 {
            next.set(Screen::Results);
        }
        return;
    }
    if res.paused || !res.race.started() {
        return;
    }
    // A road that ends before the workout does: back to the start.
    let course = &res.race.course;
    if !course.route.looped && res.race.player().dist > course.total_m - 150.0 {
        let me = res.race.human;
        res.race.riders[me].dist = 30.0;
        s.say(
            t("Nog een keer!", "Once more!"),
            t("Terug naar de start van de route", "Back to the start of the route"),
            ACCENT_SOFT,
        );
    }

    let Some((seg, remaining)) = s.segment() else {
        finish(s, &mut res, &mut form, &mut commands, &bank, false);
        return;
    };
    let target = s.target_w().unwrap_or(0.0);
    s.elapsed += dt;
    s.scores[seg].add(dt, input.power, target);
    if let Some(g) = s.game.as_mut() {
        for sound in g.update(dt, s.elapsed, input.power, Some(target)) {
            match sound {
                GameSound::Coin => play(&mut commands, &sounds.coin, 0.25),
                GameSound::Won => play(&mut commands, &bank.chime, 0.5),
                GameSound::Lost => play(&mut commands, &sounds.miss, 0.45),
            }
        }
    }
    if s.watts.len() < s.elapsed as usize {
        s.watts.push(input.power);
        s.heart.push(input.heart_rate.unwrap_or(0));
        s.cadence.push(input.cadence);
    }

    let segments = s.plan.workout.segments.clone();
    let secs = segments[seg].secs();
    let zone = Zone::of(peak(&segments[seg]).unwrap_or(0.6));
    let short = secs < SHORT_S;

    // Entering a new block.
    if s.last_segment != Some(seg) {
        let prev = s.last_segment.replace(seg);
        if let Some(p) = prev
            && s.interval(p).is_some()
            && !s.plan.ramp_test
        {
            s.last_work_end = s.elapsed;
            let score = s.scores[p];
            let (k, n) = s.interval(p).expect("checked above");
            if s.game.is_none() && (segments[p].secs() >= SHORT_S || k == n) {
                play(&mut commands, &bank.chime, 0.55);
                let pct = (score.ratio() * 100.0).round();
                let left = n - k;
                let rest = match left {
                    0 => String::new(),
                    1 => t(" · nog 1 te gaan", " · 1 to go").into(),
                    _ => tf!(" · nog {left} te gaan", " · {left} to go"),
                };
                let sub = tf!("{pct:.0}% van het doel{rest}", "{pct:.0}% of target{rest}");
                if score.nailed() {
                    s.say(t("Gehaald!", "Nailed it!"), sub, GOOD);
                } else {
                    s.say(t("Doorbijten!", "Dig in!"), sub, WARN);
                }
            }
        }
        if s.plan.ramp_test && seg >= 2 {
            let w = s.target_of(seg);
            let sub = if Zone::of(w / s.ftp).is_work() {
                t("Houd vol zolang het lukt", "Hold on as long as you can")
            } else {
                t("Nog makkelijk: rustig doortrappen", "Still easy: keep spinning")
            };
            s.say(
                tf!("Stap {} · {w:.0} W", "Step {} · {w:.0} W", seg - 1),
                sub,
                zone_color(zone),
            );
        } else if let Some((k, n)) = s.interval(seg) {
            let new_set = s.elapsed - s.last_work_end > 45.0;
            let w = s.target_of(seg);
            if !short || new_set || k == n || k == n / 2 + 1 {
                let title = if k == n {
                    t("Laatste interval!", "Last interval!").to_string()
                } else if n >= 4 && k == n / 2 + 1 {
                    tf!("Halverwege · {k} / {n}", "Halfway · {k} / {n}")
                } else {
                    format!("Interval {k} / {n}")
                };
                let mut sub = tf!(
                    "{} · {} op {w:.0} W",
                    "{} · {} at {w:.0} W",
                    zone.label(),
                    duration_text(secs)
                );
                if let Some(c) = cadence_of(&segments[seg]) {
                    sub.push_str(&format!(" · {c} rpm"));
                }
                s.say(title, sub, zone_color(zone));
            }
            if !short || k == n || k % 3 == 0 {}
        } else if prev.is_some_and(|p| s.interval(p).is_some()) && !short_rest(secs) {
            let last = seg + 1 == segments.len();
            if last {
                s.say(
                    t("Uitrijden", "Cool-down"),
                    t("Goed gedaan! Rustig uitfietsen", "Well done! Spin it out easy"),
                    zone_color(Zone::Endurance),
                );
            } else {
                s.say(
                    t("Herstel", "Recovery"),
                    tf!(
                        "{} rustig doortrappen · drink wat",
                        "{} easy spinning · have a drink",
                        duration_text(secs)
                    ),
                    zone_color(Zone::Recovery),
                );
            }
        } else if prev.is_none() {
            s.say(
                s.plan.name,
                if s.game.is_some() {
                    t(
                        "Schakel naar je doelwattage · de trainer rijdt een vlakke weg",
                        "Shift to your target watts · the trainer rides a flat road",
                    )
                } else {
                    t(
                        "Rustig opbouwen · de trainer regelt de weerstand",
                        "Build up easy · the trainer sets the resistance",
                    )
                },
                focus_color(s.plan.focus),
            );
        }
    }

    // 3-2-1 into the next interval.
    let next_work = s.interval(seg + 1).is_some() && s.interval(seg).is_none();
    let next_short = segments.get(seg + 1).is_some_and(|n| n.secs() < SHORT_S);
    let major = !next_short || s.elapsed - s.last_work_end > 40.0;
    let second = remaining.ceil() as u32;
    if next_work && major && (1..=3).contains(&second) && s.beeped != Some((seg, second)) {
        s.beeped = Some((seg, second));
        play(&mut commands, &sounds.beep, 0.5);
    }
    if next_work && major && remaining <= dt + 0.02 && s.beeped != Some((seg, 0)) {
        s.beeped = Some((seg, 0));
        play(&mut commands, &sounds.go, 0.55);
    }

    // Ramp test: it ends when you can no longer hold the step.
    if s.plan.ramp_test && seg >= 2 {
        if s.power_3s(input.power) < target * 0.85 {
            s.short_s += dt;
        } else {
            s.short_s = (s.short_s - dt).max(0.0);
        }
        if s.short_s > 6.0 {
            finish(s, &mut res, &mut form, &mut commands, &bank, false);
        }
    }
}

fn short_rest(secs: u32) -> bool {
    secs < 30
}

fn cadence_of(s: &Segment) -> Option<u32> {
    match *s {
        Segment::Steady { cadence, .. } | Segment::Ramp { cadence, .. } => cadence,
        Segment::FreeRide { .. } => None,
    }
}

/// Ends the workout: scores it, logs it in the profile, saves the ride.
fn finish(
    s: &mut Session,
    res: &mut RaceRes,
    form: &mut SetupForm,
    commands: &mut Commands,
    bank: &SoundBank,
    early: bool,
) {
    if s.finished.is_some() {
        return;
    }
    s.finished = Some(s.elapsed);
    s.ended_early = early;
    if s.plan.ramp_test && s.watts.len() > 300 {
        s.new_ftp = Some(ftp_from_ramp(best_average(&s.watts, 60)));
    }
    let (nailed, n) = s.nailed();
    let kj = s.watts.iter().sum::<f32>() / 1000.0;
    let np = normalized_power(&s.watts);
    let secs = s.watts.len() as f32;
    let intensity = np / s.ftp;
    let tss = secs * np * intensity / (s.ftp * 3600.0) * 100.0;
    let avg = s.watts.iter().sum::<f32>() / secs.max(1.0);
    form.log_session(ridecore::profile::Session {
        unix_time: fit::now_unix(),
        workout: s.plan.name.to_string(),
        secs: secs as u32,
        avg_w: avg,
        np_w: np,
        tss,
        kj,
        nailed: nailed as u32,
        intervals: n as u32,
        new_ftp: s.new_ftp,
    });
    if s.ended_early {
        s.say(
            t("Training gestopt", "Workout stopped"),
            tf!("{} gereden", "{} ridden", mmss(secs)),
            ACCENT_SOFT,
        );
    } else {
        play(commands, &bank.fanfare, 0.8);
        s.say(t("Training voltooid!", "Workout complete!"), s.plan.name, GOOD);
    }
    let mut badges = vec![format!("TSS {tss:.0}"), format!("{kj:.0} kJ")];
    if n > 0 {
        badges.insert(0, tf!("Intervallen {nailed}/{n}", "Intervals {nailed}/{n}"));
    }
    if let Some(f) = s.new_ftp {
        badges.insert(0, tf!("Nieuwe FTP {f:.0} W", "New FTP {f:.0} W"));
    }
    if res.samples.len() > 10 {
        res.saved = Some(crate::share::save_training(
            res,
            s.plan.name,
            badges,
            s.plan.focus.rgb(),
        ));
    }
}

/// The whole workout as blocks over time, colored by zone, with what you
/// actually rode drawn over it and a playhead.
fn timeline(p: &egui::Painter, rect: egui::Rect, s: &Session, ftp: f32, show_now: bool) {
    let segs = &s.plan.workout.segments;
    let total = s.total_s().max(1.0);
    let top = segs.iter().filter_map(peak).fold(1.25f32, f32::max) * s.bias.max(1.0) * 1.08;
    let x = |t: f32| rect.left() + t / total * rect.width();
    let y = |f: f32| rect.bottom() - (f / top).min(1.0) * rect.height();
    let now_x = x(s.elapsed);

    let draw = |p: &egui::Painter, alpha: f32| {
        let mut m = egui::Mesh::default();
        let mut t0 = 0.0;
        for seg in segs {
            let t1 = t0 + seg.secs() as f32;
            let (a, b) = match *seg {
                Segment::Steady { power, .. } => (power, power),
                Segment::Ramp { from, to, .. } => (from, to),
                Segment::FreeRide { .. } => (0.6, 0.6),
            };
            let (a, b) = (a * s.bias, b * s.bias);
            let ca = zone_color(Zone::of(a)).gamma_multiply(alpha);
            let cb = zone_color(Zone::of(b)).gamma_multiply(alpha);
            // A hairline gap between blocks keeps intervals readable.
            let gap = if rect.width() * seg.secs() as f32 / total > 4.0 {
                0.6
            } else {
                0.0
            };
            let (xa, xb) = (x(t0) + gap, x(t1) - gap);
            let i = m.vertices.len() as u32;
            m.colored_vertex(egui::pos2(xa, y(a)), ca);
            m.colored_vertex(egui::pos2(xb, y(b)), cb);
            m.colored_vertex(egui::pos2(xb, rect.bottom()), cb);
            m.colored_vertex(egui::pos2(xa, rect.bottom()), ca);
            m.add_triangle(i, i + 1, i + 2);
            m.add_triangle(i, i + 2, i + 3);
            t0 = t1;
        }
        p.add(m);
    };
    draw(p, if show_now { 0.32 } else { 0.95 });
    if show_now {
        let done = egui::Rect::from_min_max(rect.min, egui::pos2(now_x, rect.max.y));
        draw(&p.with_clip_rect(done.intersect(p.clip_rect())), 0.95);
    }
    // FTP line.
    let fy = y(1.0);
    p.extend(egui::Shape::dashed_line(
        &[egui::pos2(rect.left(), fy), egui::pos2(rect.right(), fy)],
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)),
        5.0,
        5.0,
    ));
    p.text(
        egui::pos2(rect.right() - 2.0, fy - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("FTP {ftp:.0}"),
        egui::FontId::proportional(11.0),
        egui::Color32::from_white_alpha(150),
    );
    // What you rode, smoothed over five seconds.
    if s.watts.len() > 2 {
        let w = &s.watts;
        let step = (w.len() / 900).max(1);
        let pts: Vec<egui::Pos2> = (0..w.len())
            .step_by(step)
            .map(|i| {
                let lo = i.saturating_sub(4);
                let avg = w[lo..=i].iter().sum::<f32>() / (i - lo + 1) as f32;
                egui::pos2(x(i as f32 + 0.5), y(avg / ftp))
            })
            .collect();
        p.line(pts, egui::Stroke::new(1.6, egui::Color32::WHITE));
    }
    if show_now {
        p.line_segment(
            [
                egui::pos2(now_x, rect.top() - 4.0),
                egui::pos2(now_x, rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
        p.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(now_x - 6.0, rect.top() - 10.0),
                egui::pos2(now_x + 6.0, rect.top() - 10.0),
                egui::pos2(now_x, rect.top() - 2.0),
            ],
            egui::Color32::WHITE,
            egui::Stroke::NONE,
        ));
    }
}

/// A small zone pill.
fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.22))
        .stroke(egui::Stroke::new(1.0, color))
        .corner_radius(20.0)
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text.to_uppercase())
                    .font(display(15.0))
                    .color(color)
                    .extra_letter_spacing(1.0),
            );
        });
}

fn glass() -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_premultiplied(12, 14, 22, 232))
        .stroke(egui::Stroke::new(1.0, theme::CARD_EDGE))
        .corner_radius(16.0)
        .inner_margin(egui::Margin::symmetric(18, 14))
}

fn number(ui: &mut egui::Ui, value: String, label: &str, size: f32, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .small()
                .color(MUTED)
                .extra_letter_spacing(1.0),
        );
        ui.label(egui::RichText::new(value).font(display(size)).color(color));
    });
}

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    mut res: ResMut<RaceRes>,
    input: Res<PlayerInput>,
    trainer: Res<Trainer>,
    mut session: ResMut<Session>,
    mut form: ResMut<SetupForm>,
    mut commands: Commands,
    bank: Res<SoundBank>,
    mut next: ResMut<NextState<Screen>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let screen = ctx.viewport_rect();
    let s = &mut *session;
    let ftp = s.ftp;
    let seg = s.segment();
    let target = s.target_w();
    let power = input.power_3s;

    // A game fills the screen behind the workout's numbers.
    if let Some(g) = &s.game {
        let painter = ctx.layer_painter(egui::LayerId::background());
        let sess = &*s;
        g.draw(&painter, screen, s.elapsed, target, &|t| sess.target_at_time(t));
        egui::Area::new("t-game".into())
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
            .interactable(false)
            .show(ctx, |ui| {
                glass().show(ui, |ui| {
                    ui.set_width(220.0);
                    g.score_card(ui);
                });
            });
    }

    // Your numbers, top left.
    egui::Area::new("t-me".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(290.0);
                ui.horizontal(|ui| {
                    let color = match target {
                        Some(t) if (power - t).abs() <= t * 0.05 => GOOD,
                        Some(t) if power < t => WARN,
                        _ => TEXT,
                    };
                    number(ui, format!("{power:.0}"), "watt", 46.0, color);
                    ui.add_space(10.0);
                    let cad_hint = seg
                        .and_then(|(k, _)| cadence_of(&s.plan.workout.segments[k]))
                        .map(|c| tf!("rpm · doel {c}", "rpm · target {c}"))
                        .unwrap_or_else(|| "rpm".into());
                    number(ui, format!("{:.0}", input.cadence), &cad_hint, 46.0, TEXT);
                });
                ui.horizontal(|ui| {
                    if let Some(hr) = input.heart_rate {
                        number(
                            ui,
                            hr.to_string(),
                            t("hartslag", "heart rate"),
                            28.0,
                            egui::Color32::from_rgb(255, 95, 95),
                        );
                        ui.add_space(10.0);
                    }
                    let me = res.race.player();
                    number(ui, format!("{:.1}", me.speed * 3.6), t("km/u", "km/h"), 28.0, TEXT);
                    ui.add_space(10.0);
                    number(
                        ui,
                        format!("{:.2}", power / me.profile.kg),
                        "w/kg",
                        28.0,
                        TEXT,
                    );
                });
                if s.game.is_some()
                    && let Some(g) = &trainer.controller.gears
                {
                    ui.add_space(4.0);
                    pill(ui, &tf!("Versnelling {}", "Gear {}", g.index + 1), ACCENT);
                } else if (s.bias - 1.0).abs() > 0.001 {
                    ui.add_space(4.0);
                    pill(ui, &tf!("Intensiteit {:.0}%", "Intensity {:.0}%", s.bias * 100.0), ACCENT);
                }
                if trainer.controller.erg_paused {
                    ui.label(
                        egui::RichText::new(t(
                            "ERG gepauzeerd · trap door om verder te gaan",
                            "ERG paused · keep pedalling to carry on",
                        ))
                            .color(WARN),
                    );
                }
            });
        });

    // The interval, big, top centre.
    if let (Some((k, remaining)), Some(target)) = (seg, target) {
        let segs = &s.plan.workout.segments;
        let zone = Zone::of(peak(&segs[k]).unwrap_or(0.6));
        let zc = zone_color(zone);
        let secs = segs[k].secs() as f32;
        egui::Area::new("t-interval".into())
            .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
            .interactable(false)
            .show(ctx, |ui| {
                glass()
                    .stroke(egui::Stroke::new(1.5, zc.gamma_multiply(0.8)))
                    .show(ui, |ui| {
                        ui.set_width(470.0);
                        ui.horizontal(|ui| {
                            pill(ui, zone.label(), zc);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let text = match s.interval(k) {
                                        _ if s.plan.ramp_test && k >= 2 => {
                                            tf!("Stap {}", "Step {}", k - 1)
                                        }
                                        Some((i, n)) => format!("Interval {i} / {n}"),
                                        None if k < s.plan.warmup.max(1) => {
                                            t("Warming-up", "Warm-up").into()
                                        }
                                        None if k + 1 == segs.len() => {
                                            t("Uitrijden", "Cool-down").into()
                                        }
                                        None => t("Herstel", "Recovery").into(),
                                    };
                                    ui.label(
                                        egui::RichText::new(text).font(display(22.0)).color(TEXT),
                                    );
                                },
                            );
                        });
                        ui.columns(3, |cols| {
                            cols[0].vertical_centered(|ui| {
                                number(
                                    ui,
                                    format!("{target:.0}"),
                                    t("doel watt", "target watts"),
                                    46.0,
                                    zc,
                                );
                            });
                            cols[1].vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(mmss(remaining))
                                        .font(display(72.0))
                                        .color(TEXT),
                                );
                            });
                            cols[2].vertical_centered(|ui| {
                                let c = if (power - target).abs() <= target * 0.05 {
                                    GOOD
                                } else {
                                    WARN
                                };
                                number(ui, format!("{power:.0}"), t("jij", "you"), 46.0, c);
                            });
                        });
                        // Block progress.
                        let (bar, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 8.0),
                            egui::Sense::hover(),
                        );
                        let p = ui.painter();
                        p.rect_filled(bar, 4.0, egui::Color32::from_white_alpha(20));
                        let mut done = bar;
                        done.set_width(
                            bar.width() * (1.0 - remaining / secs.max(1.0)).clamp(0.0, 1.0),
                        );
                        p.rect_filled(done, 4.0, zc);
                        // What's next.
                        if let Some(n) = segs.get(k + 1) {
                            let nz = Zone::of(peak(n).unwrap_or(0.6));
                            let w = s.target_of(k + 1);
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(t("Hierna", "Next")).small().color(MUTED));
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(10.0, 10.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter()
                                    .circle_filled(dot.center(), 5.0, zone_color(nz));
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {w:.0} W",
                                        nz.label(),
                                        duration_text(n.secs())
                                    ))
                                    .color(ACCENT_SOFT),
                                );
                            });
                        }
                        if s.plan.ramp_test && s.watts.len() > 60 {
                            let best = best_average(&s.watts, 60);
                            ui.label(
                                egui::RichText::new(tf!(
                                    "Beste minuut {best:.0} W  ·  geschatte FTP {:.0} W",
                                    "Best minute {best:.0} W  ·  estimated FTP {:.0} W",
                                    ftp_from_ramp(best)
                                ))
                                .color(MUTED),
                            );
                        }
                    });
            });

        // 3-2-1 into an interval, huge in the middle.
        let into_work = s.interval(k).is_none() && s.interval(k + 1).is_some();
        if into_work && remaining <= 3.0 && !res.paused {
            let digit = remaining.ceil().max(1.0) as u32;
            let frac = remaining.fract();
            let nz = zone_color(Zone::of(peak(&segs[k + 1]).unwrap_or(1.0)));
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                "t-count".into(),
            ))
            .text(
                screen.center() - egui::vec2(0.0, 40.0),
                egui::Align2::CENTER_CENTER,
                digit.to_string(),
                display(150.0 + 40.0 * (1.0 - frac)),
                nz.gamma_multiply(0.4 + 0.6 * frac),
            );
        }
    }

    // Coach messages slide in under the interval card.
    if let Some(c) = &s.coach {
        let age = s.clock - c.at;
        if age < COACH_S {
            let fade = (age / 0.25).min(1.0) * ((COACH_S - age) / 0.5).clamp(0.0, 1.0);
            let slide = (1.0 - (age / 0.3).min(1.0)).powi(2) * 30.0;
            egui::Area::new("t-coach".into())
                .anchor(egui::Align2::CENTER_TOP, [0.0, 250.0 - slide])
                .interactable(false)
                .show(ctx, |ui| {
                    ui.set_opacity(fade);
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_premultiplied(10, 12, 18, 225))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::symmetric(22, 10))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&c.title)
                                            .font(display(40.0))
                                            .color(c.color),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&c.sub).color(TEXT))
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                        });
                });
        }
    }

    // The whole session along the bottom.
    let width = (screen.width() - 32.0).min(1500.0);
    egui::Area::new("t-timeline".into())
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -16.0])
        .interactable(false)
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(width - 36.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(s.plan.name)
                            .font(display(20.0))
                            .color(TEXT),
                    );
                    pill(ui, s.plan.focus.label(), focus_color(s.plan.focus));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let total = s.total_s();
                        ui.label(
                            egui::RichText::new(tf!(
                                "{} / {} · nog {}",
                                "{} / {} · {} left",
                                mmss(s.elapsed),
                                mmss(total),
                                mmss(total - s.elapsed)
                            ))
                            .font(display(20.0))
                            .color(ACCENT_SOFT),
                        );
                        if !s.work.is_empty() && !s.plan.ramp_test {
                            let (nailed, n) = s.nailed();
                            ui.label(
                                egui::RichText::new(tf!(
                                    "{nailed}/{n} gehaald  ·  ",
                                    "{nailed}/{n} nailed  ·  "
                                ))
                                    .color(MUTED),
                            );
                        }
                    });
                });
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 84.0),
                    egui::Sense::hover(),
                );
                timeline(
                    ui.painter(),
                    rect.shrink2(egui::vec2(0.0, 6.0)),
                    s,
                    ftp,
                    true,
                );
                let keys = if s.game.is_some() {
                    t(
                        "Click of +/− = schakelen  ·  P pauze  ·  Esc menu",
                        "Click or +/− = shift  ·  P pause  ·  Esc menu",
                    )
                } else {
                    t(
                        "Click of +/− = intensiteit  ·  P pauze  ·  Esc menu  ·  C camera",
                        "Click or +/− = intensity  ·  P pause  ·  Esc menu  ·  C camera",
                    )
                };
                ui.label(egui::RichText::new(keys).small().color(MUTED));
            });
        });

    if res.paused && !s.menu {
        egui::Area::new("t-paused".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .interactable(false)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(t("PAUZE", "PAUSED"))
                        .font(display(90.0))
                        .color(egui::Color32::WHITE),
                );
            });
    }

    // The menu.
    if s.menu {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, "t-dim".into()))
            .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
        egui::Area::new("t-menu".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                glass().show(ui, |ui| {
                    ui.set_width(360.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(t("GEPAUZEERD", "PAUSED")).font(display(40.0)));
                        ui.label(
                            egui::RichText::new(tf!(
                                "{} · {} van {}",
                                "{} · {} of {}",
                                s.plan.name,
                                mmss(s.elapsed),
                                mmss(s.total_s())
                            ))
                            .color(MUTED),
                        );
                        ui.add_space(10.0);
                        if big_button(ui, t("Doorgaan", "Continue"), ACCENT, 320.0) {
                            s.menu = false;
                            res.paused = false;
                        }
                        let stop = if s.plan.ramp_test {
                            t("Test afronden", "Finish test")
                        } else {
                            t("Training afronden", "Finish workout")
                        };
                        if big_button(ui, stop, egui::Color32::from_rgb(52, 60, 82), 320.0) {
                            s.menu = false;
                            res.paused = false;
                            finish(s, &mut res, &mut form, &mut commands, &bank, true);
                        }
                        if big_button(
                            ui,
                            t("Terug naar start", "Back to start"),
                            egui::Color32::from_rgb(40, 44, 58),
                            320.0,
                        ) {
                            next.set(Screen::Setup);
                        }
                    });
                });
            });
    }
    Ok(())
}

/// A wide painted button.
pub fn big_button(ui: &mut egui::Ui, text: &str, color: egui::Color32, width: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 48.0), egui::Sense::click());
    let fill = if resp.hovered() {
        color.gamma_multiply(1.2)
    } else {
        color
    };
    ui.painter().rect_filled(rect, 12.0, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        display(24.0),
        egui::Color32::WHITE,
    );
    ui.add_space(4.0);
    resp.clicked()
}

#[allow(clippy::too_many_arguments)]
fn summary(
    mut contexts: EguiContexts,
    res: Res<RaceRes>,
    mut session: ResMut<Session>,
    mut form: ResMut<SetupForm>,
    mut strava: ResMut<crate::share::StravaUi>,
    mut next: ResMut<NextState<Screen>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let screen = ctx.viewport_rect();
    ctx.layer_painter(egui::LayerId::background()).rect_filled(
        screen,
        0.0,
        egui::Color32::from_black_alpha(150),
    );
    let s = &mut *session;
    let ftp = s.ftp;
    let secs = s.watts.len() as f32;
    let avg = s.watts.iter().sum::<f32>() / secs.max(1.0);
    let np = normalized_power(&s.watts);
    let tss = secs * np * (np / ftp) / (ftp * 3600.0) * 100.0;
    let kj = s.watts.iter().sum::<f32>() / 1000.0;
    let hr: Vec<f32> = s
        .heart
        .iter()
        .filter(|&&h| h > 0)
        .map(|&h| h as f32)
        .collect();
    let width = (screen.width() - 80.0).min(980.0);

    egui::Area::new("t-summary".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            glass().inner_margin(egui::Margin::same(26)).show(ui, |ui| {
                ui.set_width(width);
                let complete = !s.ended_early;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let title = match (s.plan.ramp_test, complete) {
                            (true, _) => t("TEST AFGEROND", "TEST DONE"),
                            (false, true) => t("TRAINING VOLTOOID", "WORKOUT COMPLETE"),
                            (false, false) => t("TRAINING GESTOPT", "WORKOUT STOPPED"),
                        };
                        ui.label(
                            egui::RichText::new(title)
                                .font(display(50.0))
                                .color(if complete { GOOD } else { ACCENT_SOFT }),
                        );
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(s.plan.name).font(display(24.0)));
                            pill(ui, s.plan.focus.label(), focus_color(s.plan.focus));
                        });
                        if let Some(g) = &s.game {
                            ui.label(
                                egui::RichText::new(format!("{} · {}", g.kind.name(), g.summary()))
                                    .font(display(22.0))
                                    .color(ACCENT),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                        let now = fit::now_unix();
                        let streak = form.profile.streak_days(now);
                        let week = form.profile.week_tss(now);
                        ui.add_space(6.0);
                        {
                            ui.label(
                                egui::RichText::new(if streak == 1 {
                                    tf!("{streak} dag op rij", "{streak} day streak")
                                } else {
                                    tf!("{streak} dagen op rij", "{streak} day streak")
                                })
                                .font(display(26.0))
                                .color(ACCENT),
                            );
                            ui.label(
                                egui::RichText::new(tf!(
                                    "{week:.0} TSS deze week",
                                    "{week:.0} TSS this week"
                                ))
                                    .color(MUTED),
                            );
                        }
                    });
                });
                ui.add_space(10.0);

                // A ramp test's verdict.
                if let Some(new) = s.new_ftp {
                    let delta = new - ftp;
                    glass()
                        .stroke(egui::Stroke::new(1.5, focus_color(Focus::Test)))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                number(
                                    ui,
                                    format!("{new:.0} W"),
                                    t("jouw nieuwe ftp", "your new ftp"),
                                    56.0,
                                    TEXT,
                                );
                                ui.add_space(20.0);
                                let (sign, c) = if delta >= 0.0 {
                                    ("+", GOOD)
                                } else {
                                    ("", WARN)
                                };
                                number(
                                    ui,
                                    format!("{sign}{delta:.0} W"),
                                    t("verschil", "change"),
                                    40.0,
                                    c,
                                );
                                ui.add_space(20.0);
                                number(
                                    ui,
                                    format!("{:.2}", new / form.kg),
                                    "w/kg",
                                    40.0,
                                    ACCENT_SOFT,
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if s.ftp_saved {
                                            ui.label(
                                                egui::RichText::new(t("Opgeslagen ✔", "Saved ✔"))
                                                    .font(display(22.0))
                                                    .color(GOOD),
                                            );
                                        } else if big_button(
                                            ui,
                                            t("Gebruik als mijn FTP", "Use as my FTP"),
                                            ACCENT,
                                            260.0,
                                        ) {
                                            form.set_ftp(new);
                                            s.ftp_saved = true;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(8.0);
                }

                ui.horizontal(|ui| {
                    let gap = 26.0;
                    number(ui, mmss(secs), t("tijd", "time"), 38.0, TEXT);
                    ui.add_space(gap);
                    number(ui, format!("{avg:.0} W"), t("gemiddeld", "average"), 38.0, TEXT);
                    ui.add_space(gap);
                    number(ui, format!("{np:.0} W"), t("genormaliseerd", "normalized"), 38.0, TEXT);
                    ui.add_space(gap);
                    number(ui, format!("{tss:.0}"), "tss", 38.0, ACCENT);
                    ui.add_space(gap);
                    number(ui, format!("{kj:.0}"), "kJ", 38.0, TEXT);
                    if !hr.is_empty() {
                        ui.add_space(gap);
                        let mean = hr.iter().sum::<f32>() / hr.len() as f32;
                        number(
                            ui,
                            format!("{mean:.0}"),
                            t("gem. hartslag", "avg heart rate"),
                            38.0,
                            egui::Color32::from_rgb(255, 95, 95),
                        );
                    }
                });
                ui.add_space(8.0);

                // Every interval as a dot: held, short of it, or not ridden.
                if !s.work.is_empty() && !s.plan.ramp_test {
                    let (nailed, n) = s.nailed();
                    ui.label(
                        egui::RichText::new(tf!(
                            "{nailed} van de {n} intervallen gehaald",
                            "{nailed} of {n} intervals nailed"
                        ))
                            .font(display(20.0)),
                    );
                    let per_row = ((ui.available_width() / 22.0) as usize).max(1);
                    for row in s.work.chunks(per_row) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            for &k in row {
                                let b = s.scores[k];
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(16.0, 16.0),
                                    egui::Sense::hover(),
                                );
                                let c = if b.secs < 1.0 {
                                    egui::Color32::from_white_alpha(30)
                                } else if b.nailed() {
                                    GOOD
                                } else {
                                    WARN
                                };
                                ui.painter().circle_filled(r.center(), 7.0, c);
                            }
                        });
                    }
                    ui.add_space(6.0);
                }

                // Time in each zone.
                let zones = zone_seconds(&s.watts, ftp);
                let (bar, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 18.0),
                    egui::Sense::hover(),
                );
                let total: u32 = zones.iter().sum::<u32>().max(1);
                let mut x = bar.left();
                for (z, &t) in Zone::ALL.iter().zip(&zones) {
                    let w = bar.width() * t as f32 / total as f32;
                    if w > 0.5 {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(x, bar.top()),
                                egui::vec2(w, bar.height()),
                            ),
                            3.0,
                            zone_color(*z),
                        );
                    }
                    x += w;
                }
                ui.horizontal_wrapped(|ui| {
                    for (z, &t) in Zone::ALL.iter().zip(&zones) {
                        if t == 0 {
                            continue;
                        }
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot.center(), 5.0, zone_color(*z));
                        ui.label(
                            egui::RichText::new(format!("{} {}", z.label(), mmss(t as f32)))
                                .small()
                                .color(MUTED),
                        );
                    }
                });
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 110.0),
                    egui::Sense::hover(),
                );
                timeline(ui.painter(), rect, s, ftp, false);
                ui.add_space(10.0);
                if res.saved.is_some() {
                    crate::results::share_panel(ui, &res, &mut strava);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if big_button(ui, t("Terug naar start", "Back to start"), ACCENT, 260.0) {
                        next.set(Screen::Setup);
                    }
                });
            });
        });
    Ok(())
}

/// One workout as a tile on the start screen. Returns true when clicked.
pub fn plan_tile(ui: &mut egui::Ui, plan: &Plan, selected: bool) -> bool {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 96.0), egui::Sense::click());
    let p = ui.painter();
    let fc = focus_color(plan.focus);
    let fill = if selected {
        egui::Color32::from_rgba_premultiplied(40, 30, 22, 235)
    } else if resp.hovered() {
        egui::Color32::from_rgba_premultiplied(36, 42, 60, 235)
    } else {
        egui::Color32::from_rgba_premultiplied(24, 28, 42, 225)
    };
    p.rect(
        rect,
        12.0,
        fill,
        egui::Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected { ACCENT } else { theme::CARD_EDGE },
        ),
        egui::StrokeKind::Inside,
    );
    p.rect_filled(
        egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 12.0), egui::vec2(4.0, 30.0)),
        2.0,
        fc,
    );
    p.text(
        rect.left_top() + egui::vec2(14.0, 8.0),
        egui::Align2::LEFT_TOP,
        plan.name,
        display(21.0),
        TEXT,
    );
    p.text(
        rect.right_top() + egui::vec2(-12.0, 8.0),
        egui::Align2::RIGHT_TOP,
        format!("{} min", (plan.secs() as f32 / 60.0).round()),
        display(21.0),
        if selected { ACCENT } else { ACCENT_SOFT },
    );
    let line = format!("{} · TSS {:.0}", plan.focus.label(), plan.tss());
    p.text(
        rect.left_top() + egui::vec2(14.0, 33.0),
        egui::Align2::LEFT_TOP,
        line,
        egui::FontId::proportional(13.0),
        fc,
    );
    // Mini workout profile.
    let area = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 12.0, rect.bottom() - 36.0),
        egui::pos2(rect.right() - 12.0, rect.bottom() - 8.0),
    );
    mini_blocks(p, area, plan);
    resp.clicked()
}

/// A workout's blocks, small, colored by zone.
pub fn mini_blocks(p: &egui::Painter, rect: egui::Rect, plan: &Plan) {
    let segs = &plan.workout.segments;
    let total = plan.secs().max(1) as f32;
    let top = segs.iter().filter_map(peak).fold(1.25f32, f32::max);
    let mut m = egui::Mesh::default();
    let mut t0 = 0.0;
    for seg in segs {
        let t1 = t0 + seg.secs() as f32;
        let (a, b) = match *seg {
            Segment::Steady { power, .. } => (power, power),
            Segment::Ramp { from, to, .. } => (from, to),
            Segment::FreeRide { .. } => (0.6, 0.6),
        };
        let x0 = rect.left() + t0 / total * rect.width();
        let x1 = rect.left() + t1 / total * rect.width();
        let ya = rect.bottom() - a / top * rect.height();
        let yb = rect.bottom() - b / top * rect.height();
        let (ca, cb) = (zone_color(Zone::of(a)), zone_color(Zone::of(b)));
        let i = m.vertices.len() as u32;
        m.colored_vertex(egui::pos2(x0, ya), ca);
        m.colored_vertex(egui::pos2(x1, yb), cb);
        m.colored_vertex(egui::pos2(x1, rect.bottom()), cb);
        m.colored_vertex(egui::pos2(x0, rect.bottom()), ca);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
        t0 = t1;
    }
    p.add(m);
}

/// The selected workout in full, for the start screen.
pub fn plan_detail(ui: &mut egui::Ui, plan: &Plan, ftp: f32) {
    ui.label(egui::RichText::new(plan.name).font(display(32.0)));
    ui.horizontal(|ui| {
        pill(ui, plan.focus.label(), focus_color(plan.focus));
        ui.label(
            egui::RichText::new(format!(
                "{} min · TSS {:.0}",
                (plan.secs() as f32 / 60.0).round(),
                plan.tss()
            ))
            .color(ACCENT_SOFT),
        );
        let n = plan.work_blocks().len();
        if n > 0 && !plan.ramp_test {
            ui.label(egui::RichText::new(tf!("· {n} blokken", "· {n} blocks")).color(MUTED));
        }
    });
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 120.0),
        egui::Sense::hover(),
    );
    let s = Session::new(plan.clone(), ftp);
    timeline(ui.painter(), rect, &s, ftp, false);
    // Watts for the main block, so you know what's coming.
    if let Some(&k) = plan
        .work_blocks()
        .iter()
        .find(|&&k| k >= 5)
        .or(plan.work_blocks().first())
    {
        let seg = &plan.workout.segments[k];
        let w = peak(seg).unwrap_or(0.0) * ftp;
        ui.label(
            egui::RichText::new(tf!(
                "Hoofdblokken: {} op {w:.0} W ({:.0}% FTP)",
                "Main blocks: {} at {w:.0} W ({:.0}% FTP)",
                duration_text(seg.secs()),
                peak(seg).unwrap_or(0.0) * 100.0
            ))
            .color(TEXT),
        );
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(plan.summary).color(ACCENT_SOFT));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(t("WAAROM WERKT DIT?", "WHY DOES THIS WORK?"))
            .font(display(15.0))
            .color(MUTED)
            .extra_letter_spacing(1.2),
    );
    ui.label(egui::RichText::new(plan.why).color(TEXT));
    if !plan.source.is_empty() {
        ui.label(
            egui::RichText::new(plan.source)
                .small()
                .italics()
                .color(MUTED),
        );
    }
}
