//! In-race overlay, styled like a race broadcast: translucent panels, small
//! spaced capitals for labels and big condensed numbers.
//!
//! Riding: your numbers in a bar at the top with the race clock under it,
//! your energy and shelter, the race situation, team orders, the course
//! profile, the news and name tags. Watching: a live bar, a card for the
//! rider you follow, the whole field in race order (click a rider to
//! follow them) and the playback controls.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::i18n::t;
use ridecore::race::ai::Intent;
use ridecore::race::course::SegmentKind;
use ridecore::race::field::RiderKind;
use ridecore::race::{Race, TeamOrder};
use ridecore::tf;

use crate::camera::CameraRig;
use crate::link::{InputSettings, PlayerInput, Trainer};
use crate::riders::{GhostRider, RiderModel};
use crate::theme::{ACCENT, GOOD, MUTED, TEXT, display};
use crate::watch::{SPEEDS, Speed};
use crate::{RaceRes, Screen};

const ORDERS: [(KeyCode, TeamOrder); 5] = [
    (KeyCode::Digit1, TeamOrder::Free),
    (KeyCode::Digit2, TeamOrder::Tempo),
    (KeyCode::Digit3, TeamOrder::Chase),
    (KeyCode::Digit4, TeamOrder::LeadOut),
    (KeyCode::Digit5, TeamOrder::Protect),
];

const GLASS: egui::Color32 = egui::Color32::from_rgba_premultiplied(9, 11, 18, 200);
const EDGE: egui::Color32 = egui::Color32::from_rgba_premultiplied(40, 44, 56, 90);
const TRACK: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 64, 78, 160);
const LIVE: egui::Color32 = egui::Color32::from_rgb(230, 40, 50);
const BAD: egui::Color32 = egui::Color32::from_rgb(255, 95, 80);
const MARGIN: f32 = 16.0;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseMenu>()
            .add_systems(
                Update,
                race_keys
                    .run_if(in_state(Screen::Racing))
                    .run_if(crate::training::racing),
            )
            .add_systems(
                EguiPrimaryContextPass,
                hud.run_if(in_state(Screen::Racing))
                    .run_if(crate::training::racing)
                    .run_if(crate::theme::ready),
            );
    }
}

#[derive(Resource, Default)]
struct PauseMenu {
    open: bool,
}

fn race_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut res: ResMut<RaceRes>,
    mut menu: ResMut<PauseMenu>,
) {
    // Watching, the number keys set the speed instead.
    for (key, order) in ORDERS {
        if keys.just_pressed(key) && res.watch.is_none() {
            res.race.set_order(order);
        }
    }
    if keys.just_pressed(KeyCode::KeyP) {
        res.paused = !res.paused;
    }
    if keys.just_pressed(KeyCode::Escape) {
        menu.open = !menu.open;
        res.paused = menu.open;
    }
}

// ---- Building blocks -------------------------------------------------------

pub fn glass() -> egui::Frame {
    egui::Frame::new()
        .fill(GLASS)
        .stroke(egui::Stroke::new(1.0, EDGE))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(16, 12))
}

/// A label in small spaced capitals.
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(11.0)
        .extra_letter_spacing(1.4)
        .color(MUTED)
}

/// A big number with its unit and a label above.
fn metric(ui: &mut egui::Ui, label: &str, value: String, unit: &str, color: egui::Color32) {
    metric_sized(ui, label, value, unit, color, 34.0);
}

fn metric_sized(
    ui: &mut egui::Ui,
    label: &str,
    value: String,
    unit: &str,
    color: egui::Color32,
    size: f32,
) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(caps(label));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new(value).font(display(size)).color(color));
            if !unit.is_empty() {
                ui.label(egui::RichText::new(unit).size(13.0).color(MUTED));
            }
        });
    });
}

/// A thin rounded bar filled to `frac`.
fn bar(ui: &mut egui::Ui, frac: f32, width: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, TRACK);
    let fill = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width() * frac.clamp(0.0, 1.0), rect.height()),
    );
    p.rect_filled(fill, 3.0, color);
}

/// A labelled bar with its value on the right.
fn gauge(ui: &mut egui::Ui, label: &str, value: String, frac: f32, color: egui::Color32) {
    let width = 250.0;
    ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(caps(label));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(value).size(13.0).color(color));
            });
        });
    });
    bar(ui, frac, width, color);
}

/// A rounded pill button; `on` fills it with the accent.
fn pill(ui: &mut egui::Ui, text: &str, on: bool) -> bool {
    let text = egui::RichText::new(text).size(14.0).color(TEXT);
    let fill = if on {
        ACCENT
    } else {
        egui::Color32::from_rgba_premultiplied(40, 44, 58, 200)
    };
    ui.add(
        egui::Button::new(text)
            .fill(fill)
            .corner_radius(14.0)
            .min_size(egui::vec2(0.0, 28.0)),
    )
    .clicked()
}

fn separator(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, EDGE);
}

fn fmt_time(s: f32) -> String {
    let s = s.max(0.0) as u32;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn team_color(race: &Race, rider: usize) -> egui::Color32 {
    let [r, g, b] = race.teams[race.riders[rider].team].spec.color;
    egui::Color32::from_rgb(r, g, b)
}

/// Training zone of a power (Coggan's seven), with its color.
fn zone(power: f32, ftp: f32) -> (u8, egui::Color32) {
    let f = power / ftp.max(1.0);
    let (z, c) = match f {
        f if f < 0.55 => (1, (150, 158, 175)),
        f if f < 0.75 => (2, (70, 150, 255)),
        f if f < 0.90 => (3, (70, 215, 120)),
        f if f < 1.05 => (4, (255, 205, 60)),
        f if f < 1.20 => (5, (255, 140, 30)),
        f if f < 1.50 => (6, (255, 70, 60)),
        _ => (7, (190, 90, 255)),
    };
    (z, egui::Color32::from_rgb(c.0, c.1, c.2))
}

fn energy_color(frac: f32) -> egui::Color32 {
    if frac > 0.5 {
        GOOD
    } else if frac > 0.2 {
        ACCENT
    } else {
        BAD
    }
}

fn kind_short(kind: RiderKind) -> &'static str {
    match kind {
        RiderKind::Sprinter => "SPR",
        RiderKind::Climber => t("KLI", "CLB"),
        RiderKind::Rouleur => "ROU",
        RiderKind::Domestique => t("KNE", "DOM"),
    }
}

fn group_name(race: &Race, gi: usize) -> String {
    let peloton = race.peloton();
    if gi == peloton {
        "Peloton".into()
    } else if gi < peloton {
        if gi == 0 {
            t("Kopgroep", "Lead group").into()
        } else {
            tf!("Achtervolgers {gi}", "Chasers {gi}")
        }
    } else {
        tf!("Groep {}", "Group {}", gi + 1)
    }
}

/// How you're doing against your best ride: green ahead, red behind.
fn ghost_gap(gap: f32) -> egui::RichText {
    let (text, color) = if gap.abs() < 0.5 {
        (
            t("Gelijk met je beste rit", "Level with your best ride").to_string(),
            egui::Color32::from_rgb(170, 215, 255),
        )
    } else if gap < 0.0 {
        (
            tf!(
                "{:.1} s voor op je beste rit",
                "{:.1} s ahead of your best ride",
                -gap
            ),
            GOOD,
        )
    } else {
        (
            tf!(
                "{:.1} s achter je beste rit",
                "{:.1} s behind your best ride",
                gap
            ),
            BAD,
        )
    };
    egui::RichText::new(text).size(14.0).color(color)
}

/// What the watching controls asked for this frame.
enum Watching {
    Speed(Speed),
    Follow(usize),
    Group(i32),
    Director,
}

// ---- The overlay -----------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    mut res: ResMut<RaceRes>,
    input: Res<PlayerInput>,
    settings: Res<InputSettings>,
    trainer: Res<Trainer>,
    rig: Res<CameraRig>,
    mut menu: ResMut<PauseMenu>,
    feedback: Res<crate::feedback::Feedback>,
    time: Res<Time>,
    mut next: ResMut<NextState<Screen>>,
    camera: Query<(&Camera, &GlobalTransform)>,
    riders: Query<(&RiderModel, &GlobalTransform)>,
    ghost: Query<&GlobalTransform, With<GhostRider>>,
    mut shown: Local<Option<usize>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let mut chosen = None;
    let mut asked = None;
    {
        let race = &res.race;
        if res.watch.is_some() {
            asked = watch_overlay(ctx, &res, &rig, &mut shown);
        } else {
            ride_overlay(ctx, &res, &input, &settings, &trainer, &feedback, &time);
            chosen = orders(ctx, race, &rig);
        }
        ticker(ctx, race, res.watch.is_some());
        profile_strip(ctx, race);
        name_tags(
            ctx,
            race,
            res.watch.is_some(),
            &camera,
            &riders,
            ghost.single().ok(),
        );
    }

    if res.paused {
        egui::Area::new("pause".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                glass().inner_margin(24).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(t("GEPAUZEERD", "PAUSED"))
                                .font(display(40.0))
                                .color(TEXT),
                        );
                        ui.add_space(8.0);
                        if pill(ui, t("Hervatten", "Resume"), true) {
                            menu.open = false;
                            res.paused = false;
                        }
                        if menu.open
                            && pill(
                                ui,
                                if res.watch.is_some() {
                                    t(
                                        "Stoppen en terug naar het menu",
                                        "Stop and back to the menu",
                                    )
                                } else {
                                    t(
                                        "Opgeven en terug naar het menu",
                                        "Quit and back to the menu",
                                    )
                                },
                                false,
                            )
                        {
                            menu.open = false;
                            next.set(Screen::Setup);
                        }
                    });
                });
            });
    }
    if let Some(order) = chosen {
        res.race.set_order(order);
    }
    let res = &mut *res;
    if let (Some(watch), Some(asked)) = (res.watch.as_mut(), asked) {
        match asked {
            Watching::Speed(s) => watch.speed = s,
            Watching::Follow(i) => watch.follow(&mut res.race, i),
            Watching::Group(step) => watch.change_group(&mut res.race, step),
            Watching::Director => watch.director = !watch.director,
        }
    }
    Ok(())
}

/// Riding: your numbers, the race strip, your energy and the situation.
fn ride_overlay(
    ctx: &egui::Context,
    res: &RaceRes,
    input: &PlayerInput,
    settings: &InputSettings,
    trainer: &Trainer,
    feedback: &crate::feedback::Feedback,
    time: &Time,
) {
    let race = &res.race;
    let me = race.player();

    // Your numbers, across the top.
    let top = egui::Area::new("numbers".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    // 3-second power, steady enough to read.
                    let watts = input.power_3s;
                    let (z, zc) = zone(watts, me.profile.ftp);
                    let cell = ui
                        .vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(caps(t("Vermogen", "Power")));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    egui::RichText::new(format!("{watts:.0}"))
                                        .font(display(52.0))
                                        .color(TEXT),
                                );
                                ui.label(egui::RichText::new("W").size(14.0).color(MUTED));
                            });
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.1} W/kg · Z{z}",
                                    watts / me.profile.kg
                                ))
                                .size(14.0)
                                .color(zc),
                            );
                        })
                        .response
                        .rect;
                    // The zone's color under the power.
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(cell.left(), cell.bottom() + 6.0),
                            egui::pos2(cell.right(), cell.bottom() + 9.0),
                        ),
                        1.5,
                        zc,
                    );
                    separator(ui, 64.0);
                    let avg = if res.samples.is_empty() {
                        0.0
                    } else {
                        res.samples.iter().map(|s| s.power_w as f32).sum::<f32>()
                            / res.samples.len() as f32
                    };
                    metric(ui, t("Gem.", "Avg"), format!("{avg:.0}"), "W", TEXT);
                    separator(ui, 64.0);
                    metric(
                        ui,
                        t("Snelheid", "Speed"),
                        format!("{:.1}", me.speed * 3.6),
                        "km/h",
                        TEXT,
                    );
                    separator(ui, 64.0);
                    metric(
                        ui,
                        t("Cadans", "Cadence"),
                        format!("{:.0}", input.cadence),
                        "rpm",
                        TEXT,
                    );
                    if let Some(hr) = input.heart_rate {
                        separator(ui, 64.0);
                        metric(
                            ui,
                            t("Hartslag", "Heart rate"),
                            hr.to_string(),
                            "bpm",
                            egui::Color32::from_rgb(255, 110, 110),
                        );
                    }
                });
            });
        })
        .response
        .rect;

    // The race under it: clock, distance, place, what's next.
    egui::Area::new("race".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, top.bottom() + 8.0])
        .show(ctx, |ui| {
            glass()
                .inner_margin(egui::Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if !race.started() {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "START OVER {:.0}",
                                    "START IN {:.0}",
                                    -race.time
                                ))
                                .font(display(34.0))
                                .color(ACCENT),
                            );
                            ui.label(
                                egui::RichText::new(t(
                                    "Klaar maken… begin te trappen bij 0!",
                                    "Get ready… start pedalling at 0!",
                                ))
                                .color(MUTED),
                            );
                            return;
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            race_strip(ui, race);
                            // Places just gained, in green.
                            let now = time.elapsed_secs_f64();
                            if let Some((gain, at)) = feedback.position_flash
                                && now - at < 2.5
                            {
                                let alpha = (1.0 - (now - at) as f32 / 2.5).clamp(0.0, 1.0);
                                ui.label(
                                    egui::RichText::new(format!("+{gain}"))
                                        .font(display(22.0))
                                        .color(GOOD.gamma_multiply(alpha)),
                                );
                            }
                        });
                        if let Some(gap) = res
                            .best
                            .as_ref()
                            .filter(|_| !me.finished())
                            .and_then(|b| b.gap(race.time, me.dist as f32))
                        {
                            ui.label(ghost_gap(gap));
                        }
                    });
                });
        });

    // Energy, shelter, the road and the trainer.
    egui::Area::new("me".into())
        .anchor(egui::Align2::LEFT_TOP, [MARGIN, MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let frac = me.energy.fraction();
                gauge(
                    ui,
                    t("Energie W′", "Energy W′"),
                    format!("{:.0}%", frac * 100.0),
                    frac,
                    energy_color(frac),
                );
                let draft = (me.draft / ridecore::race::pack::DRAFT_MAX).min(1.0);
                let sheltered = me.draft > 0.25;
                gauge(
                    ui,
                    t("Uit de wind", "Shelter"),
                    if sheltered {
                        t("in het wiel", "in the wheels").into()
                    } else {
                        t("in de wind", "in the wind").into()
                    },
                    draft,
                    if sheltered { GOOD } else { MUTED },
                );
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    let road = race.course.smooth_grade_pct(me.dist);
                    metric_sized(
                        ui,
                        t("Helling", "Grade"),
                        format!("{road:.1}"),
                        "%",
                        if road > 6.0 { BAD } else { TEXT },
                        28.0,
                    );
                    // Without virtual gears the trainer gives only part of a
                    // climb (the difficulty setting): say so.
                    if trainer.name.is_some()
                        && trainer.controller.gears.is_none()
                        && (trainer.felt_grade - road).abs() > 0.5
                    {
                        metric_sized(
                            ui,
                            "Trainer",
                            format!("{:.1}", trainer.felt_grade),
                            "%",
                            ACCENT,
                            28.0,
                        );
                    }
                    if let Some(g) = &trainer.controller.gears {
                        // The gear pops briefly after each shift.
                        let since = trainer
                            .last_shift
                            .map_or(1.0, |t| (t.elapsed().as_secs_f32() / 0.4).min(1.0));
                        metric_sized(
                            ui,
                            t("Versnelling", "Gear"),
                            format!("{}", g.index + 1),
                            &format!("/{}", g.ratios.len()),
                            egui::Color32::from_rgb(255, 255, (255.0 * since) as u8),
                            28.0 + 8.0 * (1.0 - since),
                        );
                        if trainer.shifter.is_some() {
                            ui.label(egui::RichText::new("Click").size(13.0).color(GOOD));
                        }
                        if since < 1.0 {
                            ui.ctx().request_repaint();
                        }
                    }
                });
                if settings.keyboard {
                    ui.label(
                        egui::RichText::new(t(
                            "Testmodus: pijl omhoog/omlaag",
                            "Test mode: arrow up/down",
                        ))
                        .size(13.0)
                        .color(egui::Color32::YELLOW),
                    );
                }
            });
        });

    // The groups on the road and the riders around you.
    egui::Area::new("groups".into())
        .anchor(egui::Align2::RIGHT_TOP, [-MARGIN, MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(290.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(caps(t("Koerssituatie", "Race situation")));
                let mine = race.group_of(race.human);
                for (gi, g) in race.groups.iter().enumerate().take(6) {
                    let mates = g
                        .riders
                        .iter()
                        .filter(|&&r| race.riders[r].team == me.team && !race.riders[r].human)
                        .count();
                    let mut name = format!("{} ({})", group_name(race, gi), g.riders.len());
                    if mates > 0 {
                        name += &format!("  ★{mates}");
                    }
                    let gap = if g.gap_s > 0.5 {
                        format!("+{}", fmt_time(g.gap_s))
                    } else {
                        String::new()
                    };
                    row(ui, gi == mine, None, &name, &gap);
                }
                ui.add_space(6.0);
                ui.label(caps(t("Om je heen", "Around you")));
                for (r, gap) in nearby(race, 6) {
                    let rider = &race.riders[r];
                    let mut name = rider.profile.name.clone();
                    if rider.team == me.team && !rider.human {
                        name += &format!(" · {}", rider.intent.label());
                    }
                    let gap = if rider.human {
                        "—".into()
                    } else {
                        format!("{gap:+.1}s")
                    };
                    row(ui, rider.human, Some(team_color(race, r)), &name, &gap);
                }
            });
        });
}

/// Clock, distance to go, place, lap and the next sprint or climb.
fn race_strip(ui: &mut egui::Ui, race: &Race) {
    let me = race.player();
    let item = |ui: &mut egui::Ui, label: &str, value: String| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(caps(label));
            ui.label(egui::RichText::new(value).font(display(22.0)).color(TEXT));
        });
    };
    item(ui, t("Tijd", "Time"), fmt_time(race.time));
    item(
        ui,
        t("Te gaan", "To go"),
        format!("{:.1} km", race.remaining_m(race.human) / 1000.0),
    );
    item(
        ui,
        t("Plaats", "Place"),
        format!("{}/{}", race.position(race.human), race.riders.len()),
    );
    if race.course.laps > 1 {
        let lap_len = race.course.route.length_m();
        let lap = ((me.dist.max(0.0) / lap_len) as u32 + 1).min(race.course.laps);
        item(ui, t("Ronde", "Lap"), format!("{lap}/{}", race.course.laps));
    }
    if let Some(seg) = race.course.next_segment(me.dist) {
        let kind = if seg.kind == SegmentKind::Sprint {
            "Sprint"
        } else {
            "KOM"
        };
        item(
            ui,
            kind,
            format!("{:.1} km", (seg.end_m - me.dist) / 1000.0),
        );
    }
}

/// One list row: an optional color mark, a name, a value on the right;
/// highlighted when `on`. Returns the row's response (clickable).
fn row(
    ui: &mut egui::Ui,
    on: bool,
    mark: Option<egui::Color32>,
    name: &str,
    value: &str,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 5.0, ACCENT.gamma_multiply(0.28));
    } else if resp.hovered() {
        p.rect_filled(rect, 5.0, egui::Color32::from_white_alpha(14));
    }
    let mut x = rect.left() + 6.0;
    if let Some(c) = mark {
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 6.0), egui::vec2(4.0, 12.0)),
            2.0,
            c,
        );
        x += 12.0;
    }
    let color = if on { egui::Color32::WHITE } else { TEXT };
    p.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(14.0),
        color,
    );
    p.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        value,
        egui::FontId::proportional(13.0),
        MUTED,
    );
    resp
}

/// Team orders, as pills along the bottom.
fn orders(ctx: &egui::Context, race: &Race, rig: &CameraRig) -> Option<TeamOrder> {
    let current = race.player_team().order;
    let mut chosen = None;
    egui::Area::new("orders".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [MARGIN, -MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.label(caps(&tf!(
                    "Ploegorders · {} ({})",
                    "Team orders · {} ({})",
                    race.player_team().spec.name,
                    race.player_team().plan.label()
                )));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (k, (_, order)) in ORDERS.iter().enumerate() {
                        if pill(
                            ui,
                            &format!("{}  {}", k + 1, order.label()),
                            current == *order,
                        ) {
                            chosen = Some(*order);
                        }
                    }
                });
                ui.label(
                    egui::RichText::new(tf!(
                        "C camera ({}{})  ·  V auto aan/uit  ·  P pauze  ·  Esc menu",
                        "C camera ({}{})  ·  V auto on/off  ·  P pause  ·  Esc menu",
                        rig.view.label(),
                        if rig.auto { " · auto" } else { "" }
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
            });
        });
    chosen
}

/// Watching: the live bar, the rider followed, the field in race order and
/// the playback controls. Returns what the controls asked for.
fn watch_overlay(
    ctx: &egui::Context,
    res: &RaceRes,
    rig: &CameraRig,
    shown: &mut Option<usize>,
) -> Option<Watching> {
    let race = &res.race;
    let watch = res.watch.as_ref()?;
    let me = race.player();
    let mut asked = None;

    // Live bar.
    egui::Area::new("live".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, MARGIN])
        .show(ctx, |ui| {
            glass()
                .inner_margin(egui::Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(52.0, 24.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 5.0, LIVE);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "LIVE",
                            display(17.0),
                            egui::Color32::WHITE,
                        );
                        if race.started() {
                            let head = race
                                .groups
                                .first()
                                .and_then(|g| g.riders.first())
                                .copied()
                                .unwrap_or(race.human);
                            ui.label(
                                egui::RichText::new(fmt_time(race.time))
                                    .font(display(24.0))
                                    .color(TEXT),
                            );
                            ui.label(caps(t("Kop van de koers", "Race leader")));
                            ui.label(
                                egui::RichText::new(tf!(
                                    "nog {:.1} km",
                                    "{:.1} km to go",
                                    race.remaining_m(head) / 1000.0
                                ))
                                .font(display(24.0))
                                .color(TEXT),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "START OVER {:.0}",
                                    "START IN {:.0}",
                                    -race.time
                                ))
                                .font(display(24.0))
                                .color(ACCENT),
                            );
                        }
                        ui.label(
                            egui::RichText::new(format!("{:.0}×", watch.rate().round()))
                                .font(display(24.0))
                                .color(ACCENT),
                        );
                    });
                });
        });

    // The rider followed.
    egui::Area::new("followed".into())
        .anchor(egui::Align2::LEFT_TOP, [MARGIN, MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(330.0);
                let top = ui.cursor().min;
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(caps(&format!(
                            "P{} · {}",
                            race.position(race.human),
                            race.teams[me.team].spec.name
                        )));
                        ui.label(
                            egui::RichText::new(me.profile.name.to_uppercase())
                                .font(display(30.0))
                                .color(TEXT),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                me.profile.kind.label(),
                                me.intent.label()
                            ))
                            .size(14.0)
                            .color(ACCENT),
                        );
                    });
                });
                // The team's color down the side.
                let bottom = ui.cursor().min.y;
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(top, egui::pos2(top.x + 4.0, bottom - 6.0)),
                    2.0,
                    team_color(race, race.human),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    let (_, zc) = zone(me.power, me.profile.ftp);
                    metric(ui, "Watt", format!("{:.0}", me.power), "", zc);
                    metric(
                        ui,
                        "W/kg",
                        format!("{:.1}", me.power / me.profile.kg),
                        "",
                        TEXT,
                    );
                    metric(ui, "km/h", format!("{:.1}", me.speed * 3.6), "", TEXT);
                    metric(ui, "rpm", format!("{:.0}", me.cadence), "", TEXT);
                });
                ui.add_space(4.0);
                let frac = me.energy.fraction();
                gauge(
                    ui,
                    t("Energie W′", "Energy W′"),
                    format!("{:.0}%", frac * 100.0),
                    frac,
                    energy_color(frac),
                );
                ui.horizontal(|ui| {
                    ui.label(caps(&format!(
                        "{} {:.1}%",
                        t("Helling", "Grade"),
                        race.course.smooth_grade_pct(me.dist)
                    )));
                    ui.label(caps(&format!(
                        "FTP {:.1} W/kg",
                        me.profile.ftp / me.profile.kg
                    )));
                    if me.draft > 0.25 {
                        ui.label(caps(t("in het wiel", "in the wheels")));
                    }
                });
            });
        });

    // The field in race order: finishers first, then each group on the
    // road. Click a rider to follow them.
    let screen = ctx.content_rect();
    egui::Area::new("order".into())
        .anchor(egui::Align2::RIGHT_TOP, [-MARGIN, MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.set_width(300.0);
                ui.label(caps(t(
                    "Koersverloop · klik om te volgen",
                    "Race order · click to follow",
                )));
                ui.add_space(2.0);
                // The rider followed stays in view, unless you're browsing
                // the list with the mouse.
                let changed = *shown != Some(race.human);
                *shown = Some(race.human);
                let browsing = ui.ui_contains_pointer();
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 330.0).max(200.0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let mut place = 0;
                        let mut rider_row = |ui: &mut egui::Ui, i: usize, value: String| {
                            place += 1;
                            let r = &race.riders[i];
                            let mut name = format!("{place:>2}.  {}", r.profile.name);
                            if matches!(r.intent, Intent::Attack { .. } | Intent::Sprint { .. }) {
                                name += "  ⚡";
                            }
                            let resp = row(
                                ui,
                                i == race.human,
                                Some(team_color(race, i)),
                                &name,
                                &value,
                            );
                            let hidden = !ui.clip_rect().contains_rect(resp.rect);
                            if i == race.human && (changed || hidden && !browsing) {
                                resp.scroll_to_me(Some(egui::Align::Center));
                            }
                            if resp.clicked() {
                                asked = Some(Watching::Follow(i));
                            }
                        };
                        if !race.finish.is_empty() {
                            ui.add_space(4.0);
                            ui.label(caps(t("Binnen", "Finished")));
                            for f in &race.finish {
                                let gap = if f.gap_s < 0.5 {
                                    fmt_time(f.time)
                                } else {
                                    format!("+{}", fmt_time(f.gap_s))
                                };
                                rider_row(ui, f.rider, gap);
                            }
                        }
                        for (gi, g) in race.groups.iter().enumerate() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(caps(&format!(
                                    "{} · {}",
                                    group_name(race, gi),
                                    g.riders.len()
                                )));
                                if g.gap_s > 0.5 {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "+{}",
                                                    fmt_time(g.gap_s)
                                                ))
                                                .size(13.0)
                                                .color(ACCENT),
                                            );
                                        },
                                    );
                                }
                            });
                            for &i in &g.riders {
                                rider_row(ui, i, kind_short(race.riders[i].profile.kind).into());
                            }
                        }
                    });
            });
        });

    // Playback: speed, groups, the director.
    egui::Area::new("playback".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [MARGIN, -MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if pill(ui, "Auto", watch.speed == Speed::Auto) {
                        asked = Some(Watching::Speed(Speed::Auto));
                    }
                    for x in SPEEDS {
                        if pill(ui, &format!("{x:.0}×"), watch.speed == Speed::Fixed(x)) {
                            asked = Some(Watching::Speed(Speed::Fixed(x)));
                        }
                    }
                    separator(ui, 24.0);
                    if pill(ui, t("◀ Groep", "◀ Group"), false) {
                        asked = Some(Watching::Group(-1));
                    }
                    if pill(ui, t("Groep ▶", "Group ▶"), false) {
                        asked = Some(Watching::Group(1));
                    }
                    separator(ui, 24.0);
                    if pill(ui, t("Regie", "Director"), watch.director)
                    {
                        asked = Some(Watching::Director);
                    }
                });
                ui.label(
                    egui::RichText::new(tf!(
                        "Pijltjes renner  ·  Tab groep  ·  D regie  ·  0–4 snelheid  ·  C camera ({})  ·  P pauze  ·  Esc menu",
                        "Arrows rider  ·  Tab group  ·  D director  ·  0–4 speed  ·  C camera ({})  ·  P pause  ·  Esc menu",
                        rig.view.label()
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
            });
        });
    asked
}

/// The latest news, fading out.
fn ticker(ctx: &egui::Context, race: &Race, watching: bool) {
    // Watching, the race order fills the right, so the news goes left
    // under the rider's card.
    let (corner, offset) = if watching {
        (egui::Align2::LEFT_TOP, [MARGIN, 270.0])
    } else {
        (egui::Align2::RIGHT_CENTER, [-MARGIN, 60.0])
    };
    egui::Area::new("ticker".into())
        .anchor(corner, offset)
        .show(ctx, |ui| {
            // A fixed width so messages wrap into lines, not single letters.
            ui.set_width(300.0);
            // Three at most, so they stay clear of the panels below.
            for e in race.events.iter().rev().take(3).rev() {
                let age = race.time - e.time;
                if age > 15.0 {
                    continue;
                }
                let alpha = (1.0 - age / 15.0).clamp(0.2, 1.0);
                glass()
                    .fill(GLASS.gamma_multiply(alpha))
                    .stroke(egui::Stroke::NONE)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .corner_radius(8.0)
                    .show(ui, |ui| {
                        ui.set_width(276.0);
                        ui.label(
                            egui::RichText::new(&e.text)
                                .size(14.0)
                                .color(TEXT.gamma_multiply(alpha)),
                        );
                    });
                ui.add_space(4.0);
            }
        });
}

/// The riders closest to you with their time gap (negative = ahead).
fn nearby(race: &Race, n: usize) -> Vec<(usize, f32)> {
    let me = race.player();
    let mut all: Vec<(usize, f32)> = race
        .riders
        .iter()
        .filter(|r| !r.finished() || r.human)
        .map(|r| (r.id, ((me.dist - r.dist) as f32) / me.speed.max(3.0)))
        .collect();
    all.sort_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
    all.truncate(n);
    all.sort_by(|a, b| a.1.total_cmp(&b.1));
    all
}

/// The whole race's elevation with every rider as a dot.
fn profile_strip(ctx: &egui::Context, race: &Race) {
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.42).min(700.0);
    egui::Area::new("profile".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, [-MARGIN, -MARGIN])
        .show(ctx, |ui| {
            glass().show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::hover());
                let total = race.course.total_m;
                let steps = 160;
                let elev: Vec<f32> = (0..=steps)
                    .map(|k| race.course.elevation_at(total * k as f64 / steps as f64))
                    .collect();
                let (lo, hi) = elev
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), &e| (a.min(e), b.max(e)));
                let span = (hi - lo).max(20.0);
                let to_screen = |d: f64, e: f32| {
                    egui::pos2(
                        rect.left() + (d / total) as f32 * rect.width(),
                        rect.bottom() - 4.0 - (e - lo) / span * (rect.height() - 16.0),
                    )
                };
                let pts: Vec<egui::Pos2> = elev
                    .iter()
                    .enumerate()
                    .map(|(k, &e)| to_screen(total * k as f64 / steps as f64, e))
                    .collect();
                let p = ui.painter();
                for w in pts.windows(2) {
                    p.add(egui::Shape::convex_polygon(
                        vec![
                            w[0],
                            w[1],
                            egui::pos2(w[1].x, rect.bottom()),
                            egui::pos2(w[0].x, rect.bottom()),
                        ],
                        egui::Color32::from_rgba_premultiplied(60, 70, 90, 150),
                        egui::Stroke::NONE,
                    ));
                }
                p.line(pts, egui::Stroke::new(1.5, TEXT.gamma_multiply(0.7)));
                for seg in &race.course.segments {
                    let x = rect.left() + (seg.end_m / total) as f32 * rect.width();
                    let c = if seg.kind == SegmentKind::Sprint {
                        GOOD
                    } else {
                        BAD
                    };
                    p.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.0, c.gamma_multiply(0.8)),
                    );
                }
                for r in race.riders.iter().filter(|r| r.id != race.human) {
                    let d = r.dist.clamp(0.0, total);
                    p.circle_filled(
                        to_screen(d, race.course.elevation_at(d)) - egui::vec2(0.0, 3.0),
                        2.5,
                        team_color(race, r.id),
                    );
                }
                let me = race.player();
                let d = me.dist.clamp(0.0, total);
                let at = to_screen(d, race.course.elevation_at(d)) - egui::vec2(0.0, 3.0);
                p.circle_filled(at, 5.5, egui::Color32::WHITE);
                p.circle_filled(at, 4.0, ACCENT);
            });
        });
}

/// Names over riders: teammates and the few right around you (watching:
/// around the rider followed, who gets an accented tag). Nearer tags win
/// where they would overlap, so the bunch doesn't turn into a pile of
/// labels.
fn name_tags(
    ctx: &egui::Context,
    race: &Race,
    watching: bool,
    camera: &Query<(&Camera, &GlobalTransform)>,
    riders: &Query<(&RiderModel, &GlobalTransform)>,
    ghost: Option<&GlobalTransform>,
) {
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, "tags".into()));
    let mut taken: Vec<egui::Rect> = Vec::new();
    let mut tag = |text: String, at: Vec3, color: egui::Color32, outline: bool| {
        if (at - cam_tf.translation()).length() > 80.0 {
            return;
        }
        let Ok(p) = cam.world_to_viewport(cam_tf, at) else {
            return;
        };
        let galley =
            painter.layout_no_wrap(text, egui::FontId::proportional(12.5), egui::Color32::WHITE);
        let bg = egui::Rect::from_center_size(
            egui::pos2(p.x, p.y),
            galley.size() + egui::vec2(12.0, 5.0),
        );
        if taken.iter().any(|r| r.expand(2.0).intersects(bg)) {
            return;
        }
        taken.push(bg);
        painter.rect_filled(bg, 4.0, GLASS);
        painter.rect_filled(
            egui::Rect::from_min_size(bg.min, egui::vec2(3.0, bg.height())),
            egui::CornerRadius {
                nw: 4,
                sw: 4,
                ..Default::default()
            },
            color,
        );
        if outline {
            painter.rect_stroke(
                bg,
                4.0,
                egui::Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Outside,
            );
        }
        painter.galley(bg.min + egui::vec2(7.0, 2.5), galley, egui::Color32::WHITE);
    };
    let me = race.player();
    // Nearest first, so they win the space.
    let mut models: Vec<(usize, Vec3)> = riders
        .iter()
        .map(|(m, tf)| (m.index, tf.translation()))
        .collect();
    models.sort_by(|a, b| {
        (a.1 - cam_tf.translation())
            .length()
            .total_cmp(&(b.1 - cam_tf.translation()).length())
    });
    if watching && let Some(&(_, at)) = models.iter().find(|(i, _)| *i == race.human) {
        tag(
            me.profile.name.clone(),
            at + Vec3::Y * 2.1,
            team_color(race, race.human),
            true,
        );
    }
    if let Some(g) = ghost {
        tag(
            t("Je beste rit", "Your best").into(),
            g.translation() + Vec3::Y * 2.0,
            egui::Color32::from_rgb(90, 150, 210),
            false,
        );
    }
    let mut close: Vec<usize> = race
        .riders
        .iter()
        .filter(|r| r.id != race.human && !r.finished())
        .map(|r| r.id)
        .collect();
    close.sort_by(|&a, &b| {
        (race.riders[a].dist - me.dist)
            .abs()
            .total_cmp(&(race.riders[b].dist - me.dist).abs())
    });
    close.truncate(6);
    for (i, at) in models {
        let r = &race.riders[i];
        let mate = !watching && r.team == me.team;
        if i == race.human || r.finished() || !(mate || close.contains(&i)) {
            continue;
        }
        let mut text = r.profile.name.clone();
        if mate {
            text = format!("★ {text}");
        }
        if matches!(r.intent, Intent::Attack { .. } | Intent::Sprint { .. }) {
            text += " ⚡";
        }
        tag(text, at + Vec3::Y * 2.0, team_color(race, i), false);
    }
}
