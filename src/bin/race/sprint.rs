//! Sprint training mode: the drill from `ridecore::sprint` ridden in the 3D
//! world against a few rivals. The trainer rides the road in simulation
//! mode, so a sprint is whatever you put out: no ERG holding you back.
//! This keeps the drill going, puts up the start mark and the line, counts
//! you in, shows your power big, and scores every sprint and the session.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::fit;
use ridecore::i18n::{ordinal, t};
use ridecore::sprint::{Bests, Drill, Event, Phase, SprintLog, SprintResult, records};
use ridecore::tf;
use ridecore::training::normalized_power;

use crate::link::{InputSettings, PlayerInput, Trainer};
use crate::setup::SetupForm;
use crate::sound::{RATE, SoundBank, normalize, play, tone, wav};
use crate::theme::{ACCENT, ACCENT_SOFT, BAD, GOOD, LABEL, MUTED, TEXT, WARN, bold, display};
use crate::training::{big_button, glass, mmss, pill};
use crate::world::{ROAD_HALF, ROAD_LIFT, pose_v};
use crate::{RaceRes, Screen};

/// How long the result of a sprint stays up.
const RESULT_S: f32 = 9.0;
const SAY_S: f32 = 4.0;
/// The colour of sprinting.
const SPRINT: egui::Color32 = egui::Color32::from_rgb(255, 77, 94);

pub struct SprintPlugin;

impl Plugin for SprintPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, build_sounds)
            .add_systems(OnEnter(Screen::Setup), leave)
            .add_systems(
                Update,
                (keys, tick, place_arches)
                    .chain()
                    .after(crate::advance_race)
                    .run_if(in_state(Screen::Racing))
                    .run_if(resource_exists::<SprintSession>),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    hud.run_if(in_state(Screen::Racing)),
                    summary.run_if(in_state(Screen::Results)),
                )
                    .run_if(resource_exists::<SprintSession>)
                    .run_if(crate::theme::ready),
            );
    }
}

/// The sprint session being ridden.
#[derive(Resource)]
pub struct SprintSession {
    pub drill: Drill,
    /// Your records before this session, to spot new ones.
    before: Bests,
    menu: bool,
    say: Option<(String, String, egui::Color32, f32)>,
    /// Which sprint's result is up, and since when.
    shown: Option<(usize, f32)>,
    /// The countdown beep last played: (sprint, second).
    beeped: Option<(usize, i32)>,
    /// Real time, for the messages.
    clock: f32,
    /// Logged and saved; seconds since.
    done: Option<f32>,
    ended_early: bool,
}

impl SprintSession {
    pub fn new(drill: Drill, before: Bests) -> Self {
        Self {
            drill,
            before,
            menu: false,
            say: None,
            shown: None,
            beeped: None,
            clock: 0.0,
            done: None,
            ended_early: false,
        }
    }

    fn say(&mut self, title: impl Into<String>, sub: impl Into<String>, color: egui::Color32) {
        self.say = Some((title.into(), sub.into(), color, self.clock));
    }

    /// Test mode (keyboard) rides the drill by itself: easy in between,
    /// the keyboard watts (850 W unless set higher than 300) in the sprint.
    pub fn test_power(&self, res: &RaceRes, keyboard_w: f32) -> f32 {
        let sprint_w = if keyboard_w > 300.0 {
            keyboard_w
        } else {
            850.0
        };
        self.drill.test_power(&res.race, sprint_w)
    }
}

/// Session title, as on the card and in Strava.
fn title(drill: &Drill) -> String {
    let s = &drill.settings;
    if s.sd_s > 0.5 {
        tf!(
            "Sprints {} × {:.0} ± {:.0} s",
            "Sprints {} × {:.0} ± {:.0} s",
            s.count,
            s.mean_s,
            s.sd_s
        )
    } else {
        tf!(
            "Sprints {} × {:.0} s",
            "Sprints {} × {:.0} s",
            s.count,
            s.mean_s
        )
    }
}

#[derive(Resource)]
struct SprintSounds {
    beep: Handle<AudioSource>,
    go: Handle<AudioSource>,
    jump: Handle<AudioSource>,
}

fn build_sounds(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut add = |samples: Vec<f32>| {
        sources.add(AudioSource {
            bytes: std::sync::Arc::from(wav(&samples)),
        })
    };
    let mut beep = vec![0.0; (0.2 * RATE as f32) as usize];
    tone(&mut beep, 0.0, 880.0, 0.2, 0.7, 14.0);
    let mut go = vec![0.0; (0.7 * RATE as f32) as usize];
    tone(&mut go, 0.0, 1318.5, 0.7, 0.7, 4.0);
    tone(&mut go, 0.0, 659.3, 0.7, 0.4, 4.0);
    tone(&mut go, 0.0, 987.8, 0.7, 0.3, 4.0);
    let mut jump = vec![0.0; (0.3 * RATE as f32) as usize];
    tone(&mut jump, 0.0, 523.3, 0.12, 0.6, 20.0);
    tone(&mut jump, 0.1, 784.0, 0.2, 0.6, 14.0);
    commands.insert_resource(SprintSounds {
        beep: add(normalize(beep, 0.6)),
        go: add(normalize(go, 0.75)),
        jump: add(normalize(jump, 0.5)),
    });
}

fn leave(mut commands: Commands, arches: Query<Entity, With<Arch>>) {
    commands.remove_resource::<SprintSession>();
    for e in &arches {
        commands.entity(e).despawn();
    }
}

/// Esc opens the menu (and pauses), P pauses.
fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut res: ResMut<RaceRes>,
    mut session: ResMut<SprintSession>,
) {
    if session.done.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        session.menu = !session.menu;
        res.paused = session.menu;
    }
    if keys.just_pressed(KeyCode::KeyP) && !session.menu {
        res.paused = !res.paused;
    }
}

#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    time: Res<Time>,
    mut res: ResMut<RaceRes>,
    input: Res<PlayerInput>,
    mut session: ResMut<SprintSession>,
    mut form: ResMut<SetupForm>,
    sounds: Res<SprintSounds>,
    bank: Res<SoundBank>,
    mut next: ResMut<NextState<Screen>>,
) {
    let dt = time.delta_secs().min(0.1);
    let s = &mut *session;
    s.clock += dt;
    if let Some(since) = s.done.as_mut() {
        *since += dt;
        // The rivals roll on with you while the results come up.
        s.drill.update(&mut res.race, dt, input.power);
        if *since > 5.0 {
            next.set(Screen::Results);
        }
        return;
    }
    if res.paused || !res.race.started() {
        return;
    }
    let n = s.drill.plan.len();
    for ev in s.drill.update(&mut res.race, dt, input.power) {
        match ev {
            Event::Approach(k) => {
                let secs = s.drill.plan[k].secs;
                s.say(
                    tf!("Sprint {} van {n}", "Sprint {} of {n}", k + 1),
                    tf!(
                        "{secs:.0} s · schakel 2–3 tandjes zwaarder",
                        "{secs:.0} s · shift up 2–3 gears",
                    ),
                    SPRINT,
                );
            }
            Event::Go => play(&mut commands, &sounds.go, 0.6),
            Event::Jump(i) => {
                // Only a jump before yours is news.
                if s.drill.phase == Phase::Approach {
                    let name = res.race.riders[i].profile.name.clone();
                    play(&mut commands, &sounds.jump, 0.5);
                    s.say(
                        tf!("{name} gaat vroeg!", "{name} goes early!"),
                        t("Spring mee!", "Go with it!"),
                        WARN,
                    );
                }
            }
            Event::Crossed(k) => {
                s.shown = Some((k, s.clock));
                s.say = None;
                let won = s.drill.results.get(k).is_some_and(SprintResult::won);
                play(
                    &mut commands,
                    if won { &bank.fanfare } else { &bank.chime },
                    if won { 0.7 } else { 0.55 },
                );
            }
            Event::CoolDown => s.say(
                t("Uitrijden", "Cool-down"),
                t(
                    "Alle sprints gedaan · rustig uitfietsen",
                    "All sprints done · spin it out easy",
                ),
                GOOD,
            ),
            Event::Done => finish(s, &mut res, &mut form, false),
            Event::Rewound => s.say(
                t("Nog een keer!", "Once more!"),
                t(
                    "Terug naar het begin van de weg",
                    "Back to the start of the road",
                ),
                ACCENT_SOFT,
            ),
        }
    }
    // 3-2-1 to the mark.
    if s.drill.phase == Phase::Approach
        && let Some(ttg) = s.drill.time_to_go(&res.race)
    {
        let second = ttg.ceil() as i32;
        let k = s.drill.current;
        if (1..=3).contains(&second) && s.beeped != Some((k, second)) {
            s.beeped = Some((k, second));
            play(&mut commands, &sounds.beep, 0.5);
        }
    }
}

/// Ends the session: logs it in the profile and saves the ride.
fn finish(s: &mut SprintSession, res: &mut RaceRes, form: &mut SetupForm, early: bool) {
    if s.done.is_some() {
        return;
    }
    s.done = Some(0.0);
    s.ended_early = early;
    s.drill.stop();
    let d = &s.drill;
    let bests = d.bests();
    let best = d.results.iter().max_by(|a, b| a.avg_w.total_cmp(&b.avg_w));
    let secs = d.watts.len() as f32;
    if d.results.is_empty() && secs < 60.0 {
        return;
    }
    let ftp = form.ftp;
    let np = normalized_power(&d.watts);
    let tss = secs * np * (np / ftp) / (ftp * 3600.0) * 100.0;
    let avg = d.watts.iter().sum::<f32>() / secs.max(1.0);
    let wins = d.wins() as u32;
    let ridden = d.results.len() as u32;
    let log = SprintLog {
        unix_time: fit::now_unix(),
        sprints: ridden,
        wins,
        bests,
        best_avg_w: best.map_or(0.0, |b| b.avg_w),
        best_avg_s: best.map_or(0.0, |b| b.ridden_s),
    };
    let name = title(d);
    if std::env::var_os("RIDECORE_SHOTS").is_none() && ridden > 0 {
        form.profile.sprint_log.push(log);
        form.log_session(ridecore::profile::Session {
            unix_time: fit::now_unix(),
            workout: name.clone(),
            secs: secs as u32,
            avg_w: avg,
            np_w: np,
            tss,
            kj: d.watts.iter().sum::<f32>() / 1000.0,
            nailed: wins,
            intervals: ridden,
            new_ftp: None,
        });
    }
    let mut badges = vec![
        tf!("Gewonnen {wins}/{ridden}", "Won {wins}/{ridden}"),
        format!("Peak {:.0} W", bests.peak_w),
        format!("5 s {:.0} W", bests.w5),
    ];
    if let Some(b) = best {
        badges.push(tf!(
            "Beste sprint {:.0} W in {:.1} s",
            "Best sprint {:.0} W for {:.1} s",
            b.avg_w,
            b.ridden_s
        ));
    }
    if res.samples.len() > 10 {
        let route = res.race.course.route.name.clone();
        res.saved = Some(crate::share::save_training(
            res,
            &name,
            badges,
            [255, 77, 94],
            &tf!(
                "sprinttraining tegen AI-renners op {route}",
                "sprint training against AI riders on {route}"
            ),
        ));
    }
}

/// The start mark (white) and the line (red) of the sprint coming up.
#[derive(Component)]
struct Arch {
    line: bool,
}

fn place_arches(
    mut commands: Commands,
    res: Res<RaceRes>,
    session: Res<SprintSession>,
    mut arches: Query<(&Arch, &mut Transform, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut placed: Local<Option<(usize, i64)>>,
) {
    let d = &session.drill;
    let show = matches!(d.phase, Phase::Approach | Phase::Sprint);
    if arches.is_empty() {
        spawn_arch(&mut commands, &mut meshes, &mut materials);
        return;
    }
    let key = (d.current, d.line_m as i64);
    for (arch, mut tf, mut vis) in &mut arches {
        *vis = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if show && *placed != Some(key) {
            let at = if arch.line { d.line_m } else { d.start_m };
            let (p, dir) = pose_v(&res.race.course, at, 0.0);
            *tf = Transform::from_translation(p).looking_to(dir, Vec3::Y);
        }
    }
    if show {
        *placed = Some(key);
    }
}

fn spawn_arch(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let post = meshes.add(Cylinder::new(0.22, 6.2));
    let banner = meshes.add(Cuboid::new(ROAD_HALF * 2.0 + 1.4, 1.3, 0.35));
    let stripe = meshes.add(Cuboid::new(ROAD_HALF * 2.0, 0.02, 0.7));
    let grey = materials.add(Color::srgb(0.85, 0.85, 0.88));
    for (line, color) in [
        (false, Color::srgb(0.95, 0.95, 0.95)),
        (true, Color::srgb(1.0, 0.3, 0.37)),
    ] {
        let mat = materials.add(StandardMaterial {
            base_color: color,
            emissive: color.to_linear() * 0.25,
            ..default()
        });
        commands
            .spawn((Arch { line }, Transform::default(), Visibility::Hidden))
            .with_children(|c| {
                for side in [-1.0, 1.0] {
                    c.spawn((
                        Mesh3d(post.clone()),
                        MeshMaterial3d(grey.clone()),
                        Transform::from_xyz(side * (ROAD_HALF + 0.6), 3.1, 0.0),
                    ));
                }
                c.spawn((
                    Mesh3d(banner.clone()),
                    MeshMaterial3d(mat.clone()),
                    Transform::from_xyz(0.0, 5.8, 0.0),
                ));
                c.spawn((
                    Mesh3d(stripe.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, ROAD_LIFT + 0.03, 0.0),
                    bevy::light::NotShadowCaster,
                ));
            });
    }
}

fn kmh(mps: f32) -> String {
    format!("{:.0}", mps * 3.6)
}

/// A label over a big number.
fn number(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    unit: &str,
    size: f32,
    color: egui::Color32,
) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .small()
                .color(MUTED)
                .extra_letter_spacing(1.0),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            bold(ui, &value, display(size), color);
            if !unit.is_empty() {
                ui.label(egui::RichText::new(unit).color(MUTED));
            }
        });
    });
}

fn medal(place: usize) -> egui::Color32 {
    match place {
        1 => egui::Color32::from_rgb(255, 200, 40),
        2 => egui::Color32::from_rgb(200, 205, 220),
        3 => egui::Color32::from_rgb(215, 130, 70),
        _ => MUTED,
    }
}

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    mut res: ResMut<RaceRes>,
    input: Res<PlayerInput>,
    trainer: Res<Trainer>,
    settings: Res<InputSettings>,
    mut session: ResMut<SprintSession>,
    mut form: ResMut<SetupForm>,
    mut next: ResMut<NextState<Screen>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    ctx.request_repaint();
    let s = &mut *session;
    let race = &res.race;
    let me = race.player();
    let d = &s.drill;
    let sprinting = matches!(d.phase, Phase::Approach | Phase::Sprint);

    // Your numbers, top left.
    egui::Area::new("s-me".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;
                    // In a sprint every second counts: the live number.
                    let w = if d.phase == Phase::Sprint {
                        input.power
                    } else {
                        input.power_3s
                    };
                    let color = if w >= d.spec().map_or(f32::MAX, |p| p.mean_w()) {
                        GOOD
                    } else if w > 1.5 * d.ftp {
                        ACCENT
                    } else {
                        TEXT
                    };
                    number(
                        ui,
                        t("Vermogen", "Power"),
                        format!("{w:.0}"),
                        "W",
                        46.0,
                        color,
                    );
                    number(
                        ui,
                        "Cadence",
                        format!("{:.0}", input.cadence),
                        "rpm",
                        30.0,
                        TEXT,
                    );
                    number(
                        ui,
                        t("Snelheid", "Speed"),
                        kmh(me.speed),
                        "km/h",
                        30.0,
                        TEXT,
                    );
                    if let Some(hr) = input.heart_rate.filter(|&h| h > 0) {
                        number(
                            ui,
                            t("Hartslag", "Heart"),
                            hr.to_string(),
                            "bpm",
                            30.0,
                            TEXT,
                        );
                    }
                    if let Some(g) = &trainer.controller.gears {
                        number(
                            ui,
                            t("Versnelling", "Gear"),
                            format!("{}", g.index + 1),
                            &format!("/{}", g.ratios.len()),
                            30.0,
                            ACCENT_SOFT,
                        );
                    }
                });
                if !settings.keyboard && trainer.name.is_none() {
                    ui.label(
                        egui::RichText::new(t(
                            "Trainer opnieuw verbinden…",
                            "Reconnecting the trainer…",
                        ))
                        .size(13.0)
                        .color(BAD),
                    );
                }
            });
        });

    // What's happening, top centre.
    egui::Area::new("s-phase".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            let frame = if sprinting {
                glass().stroke(egui::Stroke::new(2.0, SPRINT))
            } else {
                glass()
            };
            frame.show(ui, |ui| {
                ui.set_min_width(360.0);
                ui.vertical_centered(|ui| phase_card(ui, d, race));
            });
        });

    // The sprints so far, top right.
    egui::Area::new("s-list".into())
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .interactable(false)
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(250.0);
                ui.label(
                    egui::RichText::new(tf!("SPRINTS · {} GEWONNEN", "SPRINTS · {} WON", d.wins()))
                        .font(display(16.0))
                        .color(LABEL),
                );
                for (k, spec) in d.plan.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let now = k == d.current && d.phase != Phase::CoolDown;
                        let c = if now { SPRINT } else { MUTED };
                        ui.label(
                            egui::RichText::new(format!("{:>2}", k + 1))
                                .monospace()
                                .color(c),
                        );
                        ui.label(egui::RichText::new(format!("{:.0} s", spec.secs)).color(c));
                        if let Some(r) = d.results.get(k) {
                            ui.label(
                                egui::RichText::new(ordinal(r.place))
                                    .font(display(16.0))
                                    .color(medal(r.place)),
                            );
                            ui.label(format!("{:.0} W", r.avg_w));
                            ui.label(
                                egui::RichText::new(format!("{:.0}", r.peak_w))
                                    .small()
                                    .color(MUTED),
                            );
                        }
                    });
                }
            });
        });

    // In the sprint: everyone between the mark and the line.
    if sprinting {
        egui::Area::new("s-strip".into())
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
            .interactable(false)
            .show(ctx, |ui| {
                glass().show(ui, |ui| strip(ui, d, race));
            });
    }

    // The sprint just ridden.
    if let Some((k, at)) = s.shown {
        let age = s.clock - at;
        if age > RESULT_S {
            s.shown = None;
        } else if let Some(r) = d.results.get(k) {
            egui::Area::new("s-result".into())
                .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
                .interactable(false)
                .show(ctx, |ui| {
                    let c = medal(r.place);
                    glass().stroke(egui::Stroke::new(2.5, c)).show(ui, |ui| {
                        ui.set_min_width(420.0);
                        ui.vertical_centered(|ui| result_card(ui, r, race, &s.before, d));
                    });
                });
        }
    }

    // A message.
    if let Some((title, sub, color, at)) = &s.say {
        if s.clock - at > SAY_S || s.shown.is_some() {
            s.say = None;
        } else {
            egui::Area::new("s-say".into())
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 80.0])
                .interactable(false)
                .show(ctx, |ui| {
                    glass()
                        .stroke(egui::Stroke::new(2.0, *color))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                bold(ui, title, display(34.0), *color);
                                ui.label(egui::RichText::new(sub).size(17.0));
                            });
                        });
                });
        }
    }

    egui::Area::new("s-keys".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(t(
                    "Esc menu · P pauze · Click of +/− schakelen",
                    "Esc menu · P pause · Click or +/− to shift",
                ))
                .small()
                .color(MUTED),
            );
        });

    if res.paused && !s.menu && s.done.is_none() {
        egui::Area::new("s-paused".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                glass().show(ui, |ui| {
                    bold(ui, t("PAUZE", "PAUSED"), display(44.0), ACCENT_SOFT);
                    ui.label(t("P om verder te gaan", "P to carry on"));
                });
            });
    }

    if s.menu {
        let mut close = false;
        egui::Area::new("s-menu".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                glass().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
                    ui.set_width(320.0);
                    bold(ui, t("PAUZE", "PAUSED"), display(40.0), ACCENT_SOFT);
                    ui.add_space(10.0);
                    if big_button(ui, t("Verder sprinten", "Keep sprinting"), GOOD, 320.0) {
                        close = true;
                    }
                    if big_button(ui, t("Stoppen en opslaan", "Stop and save"), ACCENT, 320.0) {
                        finish(s, &mut res, &mut form, true);
                        s.menu = false;
                        res.paused = false;
                        res.stopped = true;
                        next.set(Screen::Results);
                    }
                    if big_button(
                        ui,
                        t("Stoppen zonder opslaan", "Quit without saving"),
                        egui::Color32::from_rgb(90, 96, 150),
                        320.0,
                    ) {
                        next.set(Screen::Setup);
                    }
                });
            });
        if close {
            s.menu = false;
            res.paused = false;
        }
    }
    Ok(())
}

/// The phase in big type: warm-up, recovery, the countdown, the sprint.
fn phase_card(ui: &mut egui::Ui, d: &Drill, race: &ridecore::race::Race) {
    let n = d.plan.len();
    let next_line = |ui: &mut egui::Ui| {
        if let Some(spec) = d.spec() {
            ui.label(
                egui::RichText::new(tf!(
                    "Hierna: sprint {} van {n} · {:.0} s",
                    "Next: sprint {} of {n} · {:.0} s",
                    d.current + 1,
                    spec.secs
                ))
                .size(16.0),
            );
        }
    };
    let easy = (0.55 * d.ftp).round();
    match d.phase {
        Phase::Warmup => {
            pill(ui, t("Opwarmen", "Warm-up"), ACCENT_SOFT);
            bold(ui, &mmss(d.phase_left()), display(52.0), TEXT);
            next_line(ui);
            ui.label(
                egui::RichText::new(t(
                    "Rustig opbouwen · doe halverwege twee korte versnellingen",
                    "Build up gradually · throw in two short spin-ups halfway",
                ))
                .small()
                .color(MUTED),
            );
        }
        Phase::Rest => {
            pill(ui, t("Herstel", "Recover"), GOOD);
            bold(ui, &mmss(d.phase_left()), display(52.0), TEXT);
            next_line(ui);
            ui.label(
                egui::RichText::new(tf!(
                    "Echt rustig: onder {easy:.0} W, zodat de volgende sprint weer volle bak is",
                    "Properly easy: under {easy:.0} W, so the next sprint is full gas again"
                ))
                .small()
                .color(MUTED),
            );
        }
        Phase::Approach => {
            let ttg = d.time_to_go(race).unwrap_or(0.0).max(0.0);
            pill(
                ui,
                &tf!("Sprint {} van {n}", "Sprint {} of {n}", d.current + 1),
                SPRINT,
            );
            bold(ui, &format!("{:.0}", ttg.ceil()), display(72.0), SPRINT);
            if let Some(spec) = d.spec() {
                let m = d.to_line_m(race).unwrap_or(0.0) - (d.start_m - race.player().dist);
                ui.label(
                    egui::RichText::new(tf!(
                        "{:.0} s · {m:.0} m van de witte boog tot de rode lijn",
                        "{:.0} s · {m:.0} m from the white arch to the red line",
                        spec.secs
                    ))
                    .size(16.0),
                );
                ui.label(
                    egui::RichText::new(tf!(
                        "Rivalen rond {:.0} W · schakel nu zwaarder",
                        "Rivals at around {:.0} W · shift up now",
                        spec.mean_w()
                    ))
                    .small()
                    .color(MUTED),
                );
            }
        }
        Phase::Sprint => {
            let left = d.to_line_m(race).unwrap_or(0.0).max(0.0);
            let since = -d.time_to_go(race).unwrap_or(0.0);
            if since < 1.2 {
                bold(ui, t("GO!", "GO!"), display(78.0), SPRINT);
            } else {
                bold(ui, &format!("{left:.0} m"), display(64.0), SPRINT);
            }
            let place = d.live_place(race);
            ui.label(
                egui::RichText::new(tf!(
                    "{} · {since:.1} s",
                    "{} · {since:.1} s",
                    ordinal(place)
                ))
                .font(display(26.0))
                .color(medal(place)),
            );
        }
        Phase::CoolDown => {
            pill(ui, t("Uitrijden", "Cool-down"), GOOD);
            bold(ui, &mmss(d.phase_left()), display(52.0), TEXT);
            ui.label(
                egui::RichText::new(tf!("{} van de {n} gewonnen", "{} of {n} won", d.wins()))
                    .size(16.0),
            );
        }
        Phase::Done => {
            bold(ui, t("Klaar!", "Done!"), display(52.0), GOOD);
        }
    }
}

/// The road from the start mark to the line, with every rider on it.
fn strip(ui: &mut egui::Ui, d: &Drill, race: &ridecore::race::Race) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(620.0, 64.0), egui::Sense::hover());
    let p = ui.painter();
    let from = d.start_m - 30.0;
    let to = d.line_m + 10.0;
    let x = |m: f64| rect.left() + ((m - from) / (to - from)).clamp(0.0, 1.0) as f32 * rect.width();
    let road = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.center().y - 6.0),
        egui::pos2(rect.right(), rect.center().y + 6.0),
    );
    p.rect_filled(road, 6.0, egui::Color32::from_white_alpha(30));
    for (m, c) in [(d.start_m, egui::Color32::WHITE), (d.line_m, SPRINT)] {
        p.line_segment(
            [
                egui::pos2(x(m), rect.top() + 6.0),
                egui::pos2(x(m), rect.bottom() - 6.0),
            ],
            egui::Stroke::new(3.0, c),
        );
    }
    for (k, &i) in d.rivals.iter().enumerate() {
        let r = &race.riders[i];
        let [cr, cg, cb] = race.teams[r.team].spec.color;
        let pos = egui::pos2(
            x(r.dist),
            rect.center().y + if k % 2 == 0 { -12.0 } else { 12.0 },
        );
        p.circle(
            pos,
            7.0,
            egui::Color32::from_rgb(cr, cg, cb),
            egui::Stroke::new(1.5, crate::theme::INK),
        );
        if d.has_jumped(k) && d.phase == Phase::Approach {
            p.text(
                pos + egui::vec2(0.0, -14.0),
                egui::Align2::CENTER_BOTTOM,
                "!",
                display(16.0),
                WARN,
            );
        }
    }
    let me = race.player();
    p.circle(
        egui::pos2(x(me.dist), rect.center().y),
        10.0,
        ACCENT,
        egui::Stroke::new(2.5, egui::Color32::WHITE),
    );
}

/// How the sprint you just rode went.
fn result_card(
    ui: &mut egui::Ui,
    r: &SprintResult,
    race: &ridecore::race::Race,
    before: &Bests,
    d: &Drill,
) {
    let (title, color) = if r.won() {
        (
            tf!(
                "GEWONNEN · {:.2} s voor",
                "WON · by {:.2} s",
                r.margin_s.min(9.99)
            ),
            medal(1),
        )
    } else {
        (
            tf!(
                "{} · {:.2} s achter",
                "{} · {:.2} s down",
                ordinal(r.place).to_uppercase(),
                -r.margin_s
            ),
            medal(r.place),
        )
    };
    bold(ui, &title, display(36.0), color);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        number(
            ui,
            t("Gemiddeld", "Average"),
            format!("{:.0}", r.avg_w),
            "W",
            40.0,
            TEXT,
        );
        let pb = before.peak_w > 0.0 && r.peak_w > before.peak_w;
        number(
            ui,
            if pb { "Peak · PR!" } else { "Peak" },
            format!("{:.0}", r.peak_w),
            "W",
            40.0,
            if pb { GOOD } else { TEXT },
        );
        number(
            ui,
            t("Duur", "Time"),
            format!("{:.1}", r.ridden_s),
            "s",
            40.0,
            TEXT,
        );
        number(ui, "Top", format!("{:.0}", r.top_kmh), "km/h", 40.0, TEXT);
    });
    // Who beat you, or who you beat, and on what watts.
    let mut rivals = r.rivals.clone();
    rivals.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut line = Vec::new();
    for (i, w, gap) in rivals.iter().take(3) {
        let name = &race.riders[*i].profile.name;
        let first = name.split_whitespace().next().unwrap_or(name);
        line.push(if *gap < 0.0 {
            format!("{first} {w:.0} W (−{:.2} s)", -gap)
        } else {
            format!("{first} {w:.0} W (+{gap:.2} s)")
        });
    }
    ui.label(egui::RichText::new(line.join(" · ")).small().color(MUTED));
    let left = d.plan.len() - d.results.len();
    if left > 0 {
        ui.label(
            egui::RichText::new(tf!(
                "Nu herstellen · nog {left} te gaan",
                "Recover now · {left} to go"
            ))
            .size(15.0)
            .color(GOOD),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn summary(
    mut contexts: EguiContexts,
    res: Res<RaceRes>,
    session: Res<SprintSession>,
    form: Res<SetupForm>,
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
    let s = &*session;
    let d = &s.drill;
    let bests = d.bests();
    let width = (screen.width() - 80.0).min(980.0);
    egui::Area::new("s-summary".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            glass().inner_margin(egui::Margin::same(26)).show(ui, |ui| {
                ui.set_width(width);
                let (head, color) = if s.ended_early {
                    (t("SPRINTS GESTOPT", "SPRINTS STOPPED"), ACCENT_SOFT)
                } else {
                    (t("SPRINTS KLAAR", "SPRINTS DONE"), GOOD)
                };
                ui.label(egui::RichText::new(head).font(display(50.0)).color(color));
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {}",
                        title(d),
                        res.race.course.route.name
                    ))
                    .font(display(22.0)),
                );
                ui.label(
                    egui::RichText::new(d.settings.focus())
                        .color(MUTED),
                );
                ui.add_space(10.0);

                // The records: this session against everything before it.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 26.0;
                    number(
                        ui,
                        t("Gewonnen", "Won"),
                        format!("{}/{}", d.wins(), d.results.len()),
                        "",
                        40.0,
                        medal(1),
                    );
                    for (label, now, before) in bests.rows(s.before) {
                        let pb = now > 0.0 && now > before;
                        number(
                            ui,
                            &if pb && before > 0.0 {
                                format!("{label} · PR!")
                            } else {
                                label.to_string()
                            },
                            format!("{now:.0}"),
                            "W",
                            40.0,
                            if pb && before > 0.0 { GOOD } else { TEXT },
                        );
                    }
                });
                ui.add_space(10.0);
                if !d.results.is_empty() {
                    sprint_bars(ui, d);
                    ui.add_space(8.0);
                }
                let all = records(&form.profile.sprint_log);
                if all.peak_w > 0.0 {
                    ui.label(
                        egui::RichText::new(tf!(
                            "Je records: peak {:.0} W · 5 s {:.0} W · 10 s {:.0} W · 30 s {:.0} W",
                            "Your records: peak {:.0} W · 5 s {:.0} W · 10 s {:.0} W · 30 s {:.0} W",
                            all.peak_w,
                            all.w5,
                            all.w10,
                            all.w30
                        ))
                        .color(MUTED),
                    );
                }
                ui.add_space(10.0);
                if res.saved.is_some() {
                    crate::results::share_panel(ui, &res, &mut strava);
                }
                ui.add_space(10.0);
                if big_button(ui, t("Terug naar start", "Back to start"), ACCENT, 260.0) {
                    next.set(Screen::Setup);
                }
            });
        });
    Ok(())
}

/// Every sprint as a bar of your average watts, with your peak as a tick
/// and the winning rival's watts as a line, coloured by your place.
fn sprint_bars(ui: &mut egui::Ui, d: &Drill) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 170.0),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let top = d
        .results
        .iter()
        .map(|r| r.peak_w.max(r.avg_w))
        .chain(d.plan.iter().flat_map(|s| s.rivals.iter().map(|r| r.watts)))
        .fold(400.0f32, f32::max)
        * 1.1;
    let n = d.plan.len().max(1);
    let slot = rect.width() / n as f32;
    let base = rect.bottom() - 34.0;
    let h = base - rect.top() - 4.0;
    let y = |w: f32| base - (w / top).min(1.0) * h;
    for (k, spec) in d.plan.iter().enumerate() {
        let cx = rect.left() + slot * (k as f32 + 0.5);
        let bw = (slot * 0.55).min(46.0);
        let label = |text: String, at: f32, c: egui::Color32| {
            p.text(
                egui::pos2(cx, at),
                egui::Align2::CENTER_TOP,
                text,
                egui::FontId::proportional(12.0),
                c,
            );
        };
        label(format!("{:.0} s", spec.secs), base + 4.0, MUTED);
        let Some(r) = d.results.get(k) else {
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(cx - bw / 2.0, base - 3.0),
                    egui::pos2(cx + bw / 2.0, base),
                ),
                2.0,
                egui::Color32::from_white_alpha(30),
            );
            continue;
        };
        let c = medal(r.place);
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(cx - bw / 2.0, y(r.avg_w)),
                egui::pos2(cx + bw / 2.0, base),
            ),
            4.0,
            c.gamma_multiply(0.85),
        );
        p.line_segment(
            [
                egui::pos2(cx - bw / 2.0, y(r.peak_w)),
                egui::pos2(cx + bw / 2.0, y(r.peak_w)),
            ],
            egui::Stroke::new(2.0, TEXT),
        );
        // The rival who got there first, or the best one you beat.
        let best_rival = r
            .rivals
            .iter()
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|x| x.1);
        if let Some(w) = best_rival {
            p.line_segment(
                [
                    egui::pos2(cx - bw / 2.0 - 5.0, y(w)),
                    egui::pos2(cx + bw / 2.0 + 5.0, y(w)),
                ],
                egui::Stroke::new(2.0, SPRINT),
            );
        }
        p.text(
            egui::pos2(cx, y(r.avg_w) - 3.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.0}", r.avg_w),
            display(15.0),
            TEXT,
        );
        label(ordinal(r.place), base + 18.0, c);
    }
    ui.label(
        egui::RichText::new(t(
            "Balk: je gemiddelde · witte streep: je piek · rode streep: de snelste rivaal",
            "Bar: your average · white tick: your peak · red line: the fastest rival",
        ))
        .small()
        .color(MUTED),
    );
}

/// Roads flat enough to sprint on: (index into the catalog, name).
pub fn flat_roads(routes: &[ridecore::route::Route]) -> Vec<usize> {
    (0..routes.len())
        .filter(|&i| {
            let r = &routes[i];
            let len = r.length_m();
            let mut gain = 0.0;
            let mut prev = r.elevation_at(0.0);
            let mut d = 100.0;
            while d <= len {
                let e = r.elevation_at(d);
                gain += (e - prev).max(0.0);
                prev = e;
                d += 100.0;
            }
            // Under 2% average climbing: a flat or rolling road.
            (gain as f64) / len < 0.02
        })
        .collect()
}

/// The setup screen's card for the session: how many sprints, how long,
/// the rest, the rivals and the road.
pub fn settings_card(ui: &mut egui::Ui, form: &mut SetupForm, roads: &[usize]) {
    crate::theme::card(ui, t("Sprintsessie", "Sprint session"), |ui| {
        let s = &mut form.profile.sprint;
        let row = |ui: &mut egui::Ui, label: &str, slider: egui::Slider| {
            ui.label(egui::RichText::new(label).small().color(MUTED));
            ui.add(slider);
        };
        row(
            ui,
            t("Aantal sprints", "Number of sprints"),
            egui::Slider::new(&mut s.count, 2..=16),
        );
        row(
            ui,
            t("Duur gemiddeld", "Duration, mean"),
            egui::Slider::new(&mut s.mean_s, 5.0..=40.0)
                .step_by(1.0)
                .suffix(" s"),
        );
        row(
            ui,
            t("Duur spreiding (sd)", "Duration, spread (sd)"),
            egui::Slider::new(&mut s.sd_s, 0.0..=10.0)
                .step_by(0.5)
                .suffix(" s"),
        );
        duration_curve(ui, s);
        row(
            ui,
            t("Herstel tussen sprints", "Recovery between sprints"),
            egui::Slider::new(&mut s.rest_s, 60.0..=480.0)
                .step_by(15.0)
                .custom_formatter(|v, _| mmss(v as f32)),
        );
        let advised = s.advised_rest_s();
        if s.rest_s + 1.0 < advised {
            ui.label(
                egui::RichText::new(tf!(
                    "Kort herstel: dan train je herhaald sprinten meer dan je topvermogen. Voor volle sprints: {} of meer.",
                    "Short rest: that trains repeated sprints more than peak power. For full sprints: {} or more.",
                    mmss(advised)
                ))
                .small()
                .color(WARN),
            );
        }
        row(
            ui,
            t("Opwarmen", "Warm-up"),
            egui::Slider::new(&mut s.warmup_s, 0.0..=900.0)
                .step_by(60.0)
                .custom_formatter(|v, _| mmss(v as f32)),
        );
        row(
            ui,
            t("Uitrijden", "Cool-down"),
            egui::Slider::new(&mut s.cooldown_s, 0.0..=900.0)
                .step_by(60.0)
                .custom_formatter(|v, _| mmss(v as f32)),
        );
        ui.add_space(4.0);
        ui.separator();
        row(
            ui,
            t("Rivalen", "Rivals"),
            egui::Slider::new(&mut s.rivals, 1..=9),
        );
        row(
            ui,
            t("Hun vermogen gemiddeld", "Their power, mean"),
            egui::Slider::new(&mut s.rival_w, 300.0..=1600.0)
                .step_by(10.0)
                .suffix(" W"),
        );
        row(
            ui,
            t("Hun spreiding (sd)", "Their spread (sd)"),
            egui::Slider::new(&mut s.rival_sd_w, 0.0..=250.0)
                .step_by(5.0)
                .suffix(" W"),
        );
        ui.label(
            egui::RichText::new(t(
                "Ze rijden jouw fiets en gewicht: wie meer watt trapt tot de streep, wint (de slipstream telt mee).",
                "They ride your bike and weight: more watts to the line wins (the draft counts too).",
            ))
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(t("Weg", "Road")).color(MUTED));
            let current = form
                .routes
                .iter()
                .find(|r| r.id == s.route)
                .map_or_else(|| "—".to_string(), |r| r.name.clone());
            egui::ComboBox::from_id_salt("sprint-road")
                .selected_text(current)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for &i in roads {
                        let r = &form.routes[i];
                        let label = format!("{} · {:.0} km", r.name, r.length_m() / 1000.0);
                        if ui.selectable_label(r.id == s.route, label).clicked() {
                            s.route = r.id.clone();
                        }
                    }
                });
        });
    });
}

/// The durations that can come up: a bell curve over the drawn range.
fn duration_curve(ui: &mut egui::Ui, s: &ridecore::sprint::SprintSettings) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::hover());
    let p = ui.painter();
    let (lo, hi) = s.duration_range();
    let (a, b) = (0.0f32, 50.0f32);
    let x = |v: f32| rect.left() + (v - a) / (b - a) * rect.width();
    let base = rect.bottom() - 14.0;
    let sd = s.sd_s.max(0.3);
    let pts: Vec<egui::Pos2> = (0..=120)
        .map(|k| {
            let v = lo + (hi - lo) * k as f32 / 120.0;
            let z = (v - s.mean_s) / sd;
            egui::pos2(x(v), base - (-0.5 * z * z).exp() * 28.0)
        })
        .collect();
    crate::theme::profile_fill(p, &pts, base, |_| SPRINT.gamma_multiply(0.6));
    p.line_segment(
        [
            egui::pos2(rect.left(), base),
            egui::pos2(rect.right(), base),
        ],
        egui::Stroke::new(1.0, MUTED),
    );
    for v in [lo, s.mean_s, hi] {
        p.text(
            egui::pos2(x(v), base + 1.0),
            egui::Align2::CENTER_TOP,
            format!("{v:.0} s"),
            egui::FontId::proportional(11.0),
            if v == s.mean_s { TEXT } else { MUTED },
        );
    }
}

/// Your sprint records and recent sessions, and the start button.
/// Returns true when start is pressed.
pub fn start_card(ui: &mut egui::Ui, form: &mut SetupForm, can_start: bool, t_now: f32) -> bool {
    let rec = records(&form.profile.sprint_log);
    crate::theme::card(ui, t("Je sprintrecords", "Your sprint records"), |ui| {
        if rec.peak_w <= 0.0 {
            ui.label(
                egui::RichText::new(t(
                    "Nog geen sprints gereden: na je eerste sessie staan hier je beste peak, 5, 10, 15 en 30 s.",
                    "No sprints yet: after your first session your best peak, 5, 10, 15 and 30 s show here.",
                ))
                .color(MUTED),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for (label, w, _) in rec.rows(Bests::default()) {
                    crate::theme::stat(ui, format!("{w:.0}"), label, ACCENT);
                }
            });
            ui.add_space(4.0);
            for log in form.profile.sprint_log.iter().rev().take(4) {
                ui.label(
                    egui::RichText::new(tf!(
                        "{} · {}/{} gewonnen · 5 s {:.0} W · 10 s {:.0} W",
                        "{} · {}/{} won · 5 s {:.0} W · 10 s {:.0} W",
                        ridecore::card::date(log.unix_time),
                        log.wins,
                        log.sprints,
                        log.bests.w5,
                        log.bests.w10
                    ))
                    .small()
                    .color(MUTED),
                );
            }
            if rec.w10 > 0.0 {
                let target = ((rec.w10 * 1.03) / 10.0).round() * 10.0;
                if ui
                    .button(tf!(
                        "Rivalen op je 10 s-record +3% ({target:.0} W)",
                        "Rivals at your 10 s record +3% ({target:.0} W)"
                    ))
                    .clicked()
                {
                    form.profile.sprint.rival_w = target;
                }
            }
        }
    });
    let s = &form.profile.sprint;
    crate::theme::card(ui, "", |ui| {
        ui.label(
            egui::RichText::new(s.focus())
                .font(display(19.0))
                .color(ACCENT_SOFT),
        );
        ui.label(
            egui::RichText::new(tf!(
                "Ongeveer {} · {} sprints van {:.0}–{:.0} s · rivalen {:.0}–{:.0} W",
                "About {} · {} sprints of {:.0}–{:.0} s · rivals {:.0}–{:.0} W",
                mmss(s.total_s()),
                s.count,
                s.duration_range().0,
                s.duration_range().1,
                s.watts_range().0,
                s.watts_range().1
            ))
            .color(TEXT),
        );
        ui.label(
            egui::RichText::new(t(
                "Geen ERG: de trainer rijdt de weg, dus een sprint is wat jij trapt. Schakel zwaarder voor de start en ga vol tot de streep.",
                "No ERG: the trainer rides the road, so a sprint is whatever you put out. Shift up before the mark and go full gas to the line.",
            ))
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        crate::setup::start_button(ui, t("START SPRINTS", "START SPRINTS"), can_start, t_now)
    })
}
