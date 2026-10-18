//! In-race overlay: your numbers, the race situation, team orders, the
//! course profile, the news ticker and name tags.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::i18n::t;
use ridecore::race::ai::Intent;
use ridecore::race::course::SegmentKind;
use ridecore::race::{Race, TeamOrder};
use ridecore::tf;

use crate::camera::CameraRig;
use crate::link::{InputSettings, PlayerInput, Trainer};
use crate::riders::{GhostRider, RiderModel};
use crate::{RaceRes, Screen};

const ORDERS: [(KeyCode, TeamOrder); 5] = [
    (KeyCode::Digit1, TeamOrder::Free),
    (KeyCode::Digit2, TeamOrder::Tempo),
    (KeyCode::Digit3, TeamOrder::Chase),
    (KeyCode::Digit4, TeamOrder::LeadOut),
    (KeyCode::Digit5, TeamOrder::Protect),
];

const PANEL: egui::Color32 = egui::Color32::from_rgba_premultiplied(12, 14, 20, 236);
const ORANGE: egui::Color32 = egui::Color32::from_rgb(255, 140, 0);

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
    for (key, order) in ORDERS {
        if keys.just_pressed(key) {
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
            egui::Color32::from_rgb(80, 220, 120),
        )
    } else {
        (
            tf!(
                "{:.1} s achter je beste rit",
                "{:.1} s behind your best ride",
                gap
            ),
            egui::Color32::from_rgb(255, 110, 90),
        )
    };
    egui::RichText::new(text).size(18.0).strong().color(color)
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

fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(10.0)
        .inner_margin(12.0)
}

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
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let race = &res.race;
    let me = race.player();

    // Your numbers.
    egui::Area::new("me".into())
        .anchor(egui::Align2::LEFT_TOP, [16.0, 16.0])
        .show(ctx, |ui| {
            panel().show(ui, |ui| {
                ui.horizontal(|ui| {
                    // 3-second power, as Zwift can show it: steady enough to
                    // read. It pulses red well over threshold.
                    let watts = input.power_3s;
                    let over = watts / me.profile.ftp > 1.2;
                    let pulse = if over {
                        1.0 + 0.1 * (ui.input(|i| i.time) as f32 * 10.0).sin().abs()
                    } else {
                        1.0
                    };
                    big_sized(
                        ui,
                        "WATT",
                        format!("{watts:.0}"),
                        if over {
                            egui::Color32::from_rgb(255, 80, 60)
                        } else {
                            ORANGE
                        },
                        30.0 * pulse,
                    );
                    big(ui, "W/KG", format!("{:.1}", watts / me.profile.kg), ORANGE);
                    big(
                        ui,
                        "RPM",
                        format!("{:.0}", input.cadence),
                        egui::Color32::LIGHT_BLUE,
                    );
                    big(
                        ui,
                        "KM/H",
                        format!("{:.1}", me.speed * 3.6),
                        egui::Color32::WHITE,
                    );
                    if let Some(hr) = input.heart_rate {
                        big(
                            ui,
                            t("HART", "HR"),
                            hr.to_string(),
                            egui::Color32::from_rgb(255, 90, 90),
                        );
                    }
                });
                ui.add_space(4.0);
                let frac = me.energy.fraction();
                let color = if frac > 0.5 {
                    egui::Color32::from_rgb(80, 200, 120)
                } else if frac > 0.2 {
                    ORANGE
                } else {
                    egui::Color32::RED
                };
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(330.0)
                        .fill(color)
                        .text(format!("W' {:.0}%", frac * 100.0)),
                );
                ui.horizontal(|ui| {
                    // Shelter meter: glows green when you're well in the wheels.
                    let sheltered = me.draft > 0.25;
                    ui.add(
                        egui::ProgressBar::new((me.draft / 0.5).min(1.0))
                            .desired_width(110.0)
                            .fill(if sheltered {
                                egui::Color32::from_rgb(80, 220, 120)
                            } else {
                                egui::Color32::GRAY
                            })
                            .text(if sheltered {
                                t("In het wiel", "Sheltered")
                            } else {
                                "Wind"
                            }),
                    );
                    let road = race.course.smooth_grade_pct(me.dist);
                    ui.label(
                        egui::RichText::new(tf!("Helling {road:.1}%", "Grade {road:.1}%"))
                            .color(egui::Color32::LIGHT_GRAY),
                    );
                    // Without virtual gears the trainer gives only part of
                    // a climb (the difficulty setting): say so.
                    if trainer.name.is_some()
                        && trainer.controller.gears.is_none()
                        && (trainer.felt_grade - road).abs() > 0.5
                    {
                        ui.label(
                            egui::RichText::new(format!("trainer {:.1}%", trainer.felt_grade))
                                .color(ORANGE),
                        );
                    }
                    if let Some(g) = &trainer.controller.gears {
                        // The gear pops briefly after each shift.
                        let since = trainer
                            .last_shift
                            .map_or(1.0, |t| (t.elapsed().as_secs_f32() / 0.4).min(1.0));
                        let size = 18.0 + 10.0 * (1.0 - since);
                        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * since) as u8;
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(tf!("Versnelling {}", "Gear {}", g.index + 1))
                                    .size(size)
                                    .strong()
                                    .color(egui::Color32::from_rgb(255, 255, lerp(255, 0))),
                            );
                            ui.label(
                                egui::RichText::new(format!("/{}", g.ratios.len()))
                                    .color(egui::Color32::GRAY),
                            );
                            if trainer.shifter.is_some() {
                                ui.label(
                                    egui::RichText::new("Click")
                                        .color(egui::Color32::from_rgb(80, 220, 120)),
                                );
                            }
                        });
                        if since < 1.0 {
                            ctx.request_repaint();
                        }
                    }
                    if settings.keyboard {
                        ui.label(
                            egui::RichText::new(t(
                                "Testmodus: pijl omhoog/omlaag",
                                "Test mode: arrow up/down",
                            ))
                            .color(egui::Color32::YELLOW),
                        );
                    }
                });
            });
        });

    // Race clock, distance and position.
    egui::Area::new("race".into())
        .anchor(egui::Align2::CENTER_TOP, [0.0, 16.0])
        .show(ctx, |ui| {
            panel().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if !race.started() {
                        ui.label(
                            egui::RichText::new(tf!(
                                "Start over {:.0}",
                                "Start in {:.0}",
                                -race.time
                            ))
                            .size(40.0)
                            .strong()
                            .color(ORANGE),
                        );
                        ui.label(t(
                            "Klaar maken… begin te trappen bij 0!",
                            "Get ready… start pedalling at 0!",
                        ));
                    } else {
                        let left = race.remaining_m(race.human) / 1000.0;
                        let lap_len = race.course.route.length_m();
                        let lap = ((me.dist.max(0.0) / lap_len) as u32 + 1).min(race.course.laps);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(tf!(
                                    "{}   ·   {:.1} km te gaan   ·   P{}",
                                    "{}   ·   {:.1} km to go   ·   P{}",
                                    fmt_time(race.time),
                                    left,
                                    race.position(race.human)
                                ))
                                .size(24.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                            );
                            // Places just gained, in green.
                            let now = time.elapsed_secs_f64();
                            if let Some((gain, at)) = feedback.position_flash
                                && now - at < 2.5
                            {
                                let alpha = (1.0 - (now - at) as f32 / 2.5).clamp(0.0, 1.0);
                                ui.label(
                                    egui::RichText::new(format!("▲{gain}"))
                                        .size(24.0)
                                        .strong()
                                        .color(
                                            egui::Color32::from_rgb(80, 220, 120)
                                                .gamma_multiply(alpha),
                                        ),
                                );
                            }
                        });
                        let mut line = tf!("Ronde {lap}/{}", "Lap {lap}/{}", race.course.laps);
                        if let Some(seg) = race.course.next_segment(me.dist) {
                            let kind = if seg.kind == SegmentKind::Sprint {
                                "Sprint"
                            } else {
                                "KOM"
                            };
                            line += &tf!(
                                "   ·   {kind} over {:.1} km",
                                "   ·   {kind} in {:.1} km",
                                (seg.end_m - me.dist) / 1000.0
                            );
                        }
                        ui.label(egui::RichText::new(line).color(egui::Color32::LIGHT_GRAY));
                        if let Some(gap) = res
                            .best
                            .as_ref()
                            .filter(|_| !me.finished())
                            .and_then(|b| b.gap(race.time, me.dist as f32))
                        {
                            ui.label(ghost_gap(gap));
                        }
                    }
                });
            });
        });

    // Groups on the road.
    egui::Area::new("groups".into())
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .show(ctx, |ui| {
            panel().show(ui, |ui| {
                ui.set_width(300.0);
                ui.label(
                    egui::RichText::new(t("Koerssituatie", "Race situation"))
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                let mine = race.group_of(race.human);
                let peloton = race.peloton();
                for (gi, g) in race.groups.iter().enumerate().take(6) {
                    let name = if gi == peloton {
                        "Peloton".to_string()
                    } else if gi < peloton {
                        if gi == 0 {
                            t("Kopgroep", "Lead group").into()
                        } else {
                            tf!("Achtervolgers {gi}", "Chasers {gi}")
                        }
                    } else {
                        tf!("Groep {}", "Group {}", gi + 1)
                    };
                    let mates = g
                        .riders
                        .iter()
                        .filter(|&&r| race.riders[r].team == me.team && !race.riders[r].human)
                        .count();
                    let mut text = format!("{name} ({})", g.riders.len());
                    if g.gap_s > 0.5 {
                        text += &format!("  +{}", fmt_time(g.gap_s));
                    }
                    if mates > 0 {
                        text += &if mates == 1 {
                            tf!("  · {mates} ploegmaat", "  · {mates} teammate")
                        } else {
                            tf!("  · {mates} ploegmaten", "  · {mates} teammates")
                        };
                    }
                    let color = if gi == mine {
                        ORANGE
                    } else {
                        egui::Color32::LIGHT_GRAY
                    };
                    ui.label(
                        egui::RichText::new(if gi == mine {
                            format!("▶ {text}")
                        } else {
                            text
                        })
                        .color(color),
                    );
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(t("Om je heen", "Around you"))
                        .strong()
                        .color(egui::Color32::WHITE),
                );
                for (r, gap) in nearby(race, 6) {
                    let rider = &race.riders[r];
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 5.0, team_color(race, r));
                        let mut name = rider.profile.name.clone();
                        if rider.team == me.team && !rider.human {
                            name += &format!(" · {}", rider.intent.label());
                        }
                        let color = if rider.human {
                            ORANGE
                        } else {
                            egui::Color32::WHITE
                        };
                        ui.label(egui::RichText::new(name).color(color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let g = if rider.human {
                                "—".into()
                            } else {
                                format!("{:+.1}s", gap)
                            };
                            ui.label(egui::RichText::new(g).color(egui::Color32::LIGHT_GRAY));
                        });
                    });
                }
            });
        });

    // Team orders.
    let current = race.player_team().order;
    let mut chosen = None;
    egui::Area::new("orders".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0, -16.0])
        .show(ctx, |ui| {
            panel().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tf!(
                        "Ploegorders · {} ({})",
                        "Team orders · {} ({})",
                        race.player_team().spec.name,
                        race.player_team().plan.label()
                    ))
                    .strong()
                    .color(egui::Color32::WHITE),
                );
                ui.horizontal(|ui| {
                    for (k, (_, order)) in ORDERS.iter().enumerate() {
                        let label = format!("{} {}", k + 1, order.label());
                        if ui.selectable_label(current == *order, label).clicked() {
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
                    .small()
                    .color(egui::Color32::GRAY),
                );
            });
        });

    // News ticker.
    egui::Area::new("ticker".into())
        .anchor(egui::Align2::RIGHT_CENTER, [-16.0, 60.0])
        .show(ctx, |ui| {
            // A fixed width so messages wrap into lines, not single letters.
            ui.set_width(300.0);
            // Three at most, so they stay clear of the climb panel below.
            for e in race.events.iter().rev().take(3).rev() {
                let age = race.time - e.time;
                if age > 15.0 {
                    continue;
                }
                let alpha = (1.0 - age / 15.0).clamp(0.2, 1.0);
                egui::Frame::new()
                    .fill(PANEL.gamma_multiply(alpha))
                    .corner_radius(6.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&e.text)
                                .color(egui::Color32::WHITE.gamma_multiply(alpha)),
                        );
                    });
                ui.add_space(4.0);
            }
        });

    profile_strip(ctx, race);
    name_tags(ctx, race, &camera, &riders, ghost.single().ok());

    if res.paused {
        egui::Area::new("pause".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                panel().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(t("Gepauzeerd", "Paused"))
                                .size(32.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        if ui.button(t("Hervatten", "Resume")).clicked() {
                            menu.open = false;
                            res.paused = false;
                        }
                        if menu.open
                            && ui
                                .button(t(
                                    "Opgeven en terug naar het menu",
                                    "Quit and back to the menu",
                                ))
                                .clicked()
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
    Ok(())
}

fn big(ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32) {
    big_sized(ui, label, value, color, 30.0);
}

fn big_sized(ui: &mut egui::Ui, label: &str, value: String, color: egui::Color32, size: f32) {
    ui.vertical(|ui| {
        ui.set_min_width(62.0);
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(egui::Color32::GRAY),
        );
        ui.label(egui::RichText::new(value).size(size).strong().color(color));
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
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .show(ctx, |ui| {
            panel().show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, 70.0), egui::Sense::hover());
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
                        rect.bottom() - 6.0 - (e - lo) / span * (rect.height() - 18.0),
                    )
                };
                let mut pts: Vec<egui::Pos2> = elev
                    .iter()
                    .enumerate()
                    .map(|(k, &e)| to_screen(total * k as f64 / steps as f64, e))
                    .collect();
                pts.push(egui::pos2(rect.right(), rect.bottom()));
                pts.push(egui::pos2(rect.left(), rect.bottom()));
                let p = ui.painter();
                for w in pts[..pts.len() - 2].windows(2) {
                    p.add(egui::Shape::convex_polygon(
                        vec![
                            w[0],
                            w[1],
                            egui::pos2(w[1].x, rect.bottom()),
                            egui::pos2(w[0].x, rect.bottom()),
                        ],
                        egui::Color32::from_rgb(70, 120, 80),
                        egui::Stroke::NONE,
                    ));
                }
                p.line(
                    pts[..pts.len() - 2].to_vec(),
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(150, 220, 150)),
                );
                for seg in &race.course.segments {
                    let x = rect.left() + (seg.end_m / total) as f32 * rect.width();
                    let c = if seg.kind == SegmentKind::Sprint {
                        egui::Color32::from_rgb(40, 200, 80)
                    } else {
                        egui::Color32::from_rgb(230, 40, 40)
                    };
                    p.line_segment(
                        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                        egui::Stroke::new(1.5, c),
                    );
                }
                for r in race.riders.iter().filter(|r| !r.human) {
                    let d = r.dist.clamp(0.0, total);
                    p.circle_filled(
                        to_screen(d, race.course.elevation_at(d)) - egui::vec2(0.0, 4.0),
                        2.5,
                        team_color(race, r.id),
                    );
                }
                let me = race.player();
                let d = me.dist.clamp(0.0, total);
                p.circle_filled(
                    to_screen(d, race.course.elevation_at(d)) - egui::vec2(0.0, 4.0),
                    5.0,
                    ORANGE,
                );
            });
        });
}

/// Names over teammates and riders right around you.
fn name_tags(
    ctx: &egui::Context,
    race: &Race,
    camera: &Query<(&Camera, &GlobalTransform)>,
    riders: &Query<(&RiderModel, &GlobalTransform)>,
    ghost: Option<&GlobalTransform>,
) {
    let Ok((cam, cam_tf)) = camera.single() else {
        return;
    };
    let tag = |text: String, at: Vec3, color: egui::Color32| {
        if (at - cam_tf.translation()).length() > 80.0 {
            return;
        }
        let Ok(p) = cam.world_to_viewport(cam_tf, at) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, "tags".into()));
        let galley =
            painter.layout_no_wrap(text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
        let bg = egui::Rect::from_center_size(
            egui::pos2(p.x, p.y),
            galley.size() + egui::vec2(10.0, 4.0),
        );
        painter.rect_filled(bg, 4.0, color);
        painter.galley(bg.min + egui::vec2(5.0, 2.0), galley, egui::Color32::WHITE);
    };
    if let Some(g) = ghost {
        tag(
            t("Je beste rit", "Your best").into(),
            g.translation() + Vec3::Y * 2.0,
            egui::Color32::from_rgba_unmultiplied(90, 150, 210, 190),
        );
    }
    let me = race.player();
    // Teammates always; otherwise only the few riders right around you.
    let mut close: Vec<usize> = race
        .riders
        .iter()
        .filter(|r| !r.human && !r.finished())
        .map(|r| r.id)
        .collect();
    close.sort_by(|&a, &b| {
        (race.riders[a].dist - me.dist)
            .abs()
            .total_cmp(&(race.riders[b].dist - me.dist).abs())
    });
    close.truncate(6);
    for (model, tf) in riders {
        let r = &race.riders[model.index];
        let mate = r.team == me.team;
        if r.human || r.finished() || !(mate || close.contains(&r.id)) {
            continue;
        }
        let mut text = r.profile.name.clone();
        if mate {
            text = format!("★ {text}");
        }
        if matches!(r.intent, Intent::Attack { .. } | Intent::Sprint { .. }) {
            text += " ⚡";
        }
        tag(
            text,
            tf.translation() + Vec3::Y * 2.0,
            team_color(race, r.id).gamma_multiply(0.75),
        );
    }
}
