//! Climb mode, like a bike computer's ClimbPro: on (or just before) a climb,
//! its profile colored by gradient with you on it, and what's left to the top.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::i18n::t;
use ridecore::race::course::Climb;
use ridecore::tf;

use crate::theme::grade_color;
use crate::{RaceRes, Screen};

/// The panel appears this far before a climb starts.
const SHOW_AHEAD_M: f64 = 1000.0;

pub struct ClimbPlugin;

impl Plugin for ClimbPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            climb_panel
                .run_if(in_state(Screen::Racing))
                .run_if(crate::training::racing)
                .run_if(crate::theme::ready),
        );
    }
}

fn climb_panel(mut contexts: EguiContexts, res: Res<RaceRes>) -> Result {
    let ctx = contexts.ctx_mut()?;
    let race = &res.race;
    let me = race.player();
    if me.finished() || !race.started() {
        return Ok(());
    }
    let d = me.dist.max(0.0);
    let Some(climb) = race.course.climb_near(d, SHOW_AHEAD_M) else {
        return Ok(());
    };
    let index = race
        .course
        .climbs
        .iter()
        .position(|c| c == climb)
        .unwrap_or(0);
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.42).min(700.0);
    egui::Area::new("climbpro".into())
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -122.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(12, 14, 20, 242))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(width);
                    header(ui, race, climb, index, d);
                    profile(ui, race, climb, d, width);
                });
        });
    Ok(())
}

fn stat(ui: &mut egui::Ui, value: String, label: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(value).size(24.0).strong().color(color));
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(egui::Color32::GRAY),
        );
    });
}

fn header(ui: &mut egui::Ui, race: &ridecore::race::Race, climb: &Climb, index: usize, d: f64) {
    let course = &race.course;
    let what = match &climb.name {
        Some(name) => format!("{name} · {}", climb.category.label()),
        None => tf!(
            "{} · klim {}/{}",
            "{} · climb {}/{}",
            climb.category.label(),
            index + 1,
            course.climbs.len()
        ),
    };
    let title = format!(
        "{what} · {:.1} km · {:.1}% · +{:.0} m",
        climb.length_m() / 1000.0,
        climb.avg_pct,
        climb.gain_m
    );
    ui.label(
        egui::RichText::new(title)
            .strong()
            .color(egui::Color32::WHITE),
    );
    if d < climb.start_m {
        ui.horizontal(|ui| {
            stat(
                ui,
                format!("{:.0} m", climb.start_m - d),
                t("TOT DE VOET", "TO THE FOOT"),
                egui::Color32::WHITE,
            );
            ui.add_space(18.0);
            stat(
                ui,
                format!("{:.0}%", climb.max_pct),
                t("STEILSTE 100 M", "STEEPEST 100 M"),
                grade_color(climb.max_pct),
            );
        });
        return;
    }
    let left = climb.end_m - d;
    let up = course.elevation_at(climb.end_m) - course.elevation_at(d);
    let avg_left = if left > 1.0 {
        up / left as f32 * 100.0
    } else {
        0.0
    };
    let now = course.grade_ahead_pct(d, 50.0);
    ui.horizontal(|ui| {
        stat(
            ui,
            format!("{:.2} km", left / 1000.0),
            t("NOG TE GAAN", "TO GO"),
            egui::Color32::WHITE,
        );
        ui.add_space(18.0);
        stat(
            ui,
            format!("{:.0} m", up.max(0.0)),
            t("NOG OMHOOG", "STILL TO CLIMB"),
            egui::Color32::WHITE,
        );
        ui.add_space(18.0);
        stat(
            ui,
            format!("{avg_left:.1}%"),
            t("GEM. RESTANT", "AVG REMAINING"),
            grade_color(avg_left),
        );
        ui.add_space(18.0);
        stat(ui, format!("{now:.1}%"), t("NU", "NOW"), grade_color(now));
    });
}

/// The climb's profile in gradient-colored bars, the part you've ridden
/// dimmed, and a marker where you are.
fn profile(ui: &mut egui::Ui, race: &ridecore::race::Race, climb: &Climb, d: f64, width: f32) {
    let course = &race.course;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 66.0), egui::Sense::hover());
    let p = ui.painter();
    let (start, end) = (climb.start_m, climb.end_m);
    let length = end - start;
    let lo = course.elevation_at(start);
    let span = (course.elevation_at(end) - lo).max(10.0);
    let x_of = |x: f64| rect.left() + ((x - start) / length) as f32 * rect.width();
    let y_of = |e: f32| rect.bottom() - (e - lo) / span * (rect.height() - 6.0);
    let bars = 60;
    let step = length / bars as f64;
    let pts: Vec<egui::Pos2> = (0..=bars)
        .map(|k| {
            let x = start + step * k as f64;
            egui::pos2(x_of(x), y_of(course.elevation_at(x)))
        })
        .collect();
    crate::theme::profile_fill(p, &pts, rect.bottom(), |k| {
        let a = start + step * k as f64;
        let pct = (course.elevation_at(a + step) - course.elevation_at(a)) / step as f32 * 100.0;
        let color = grade_color(pct);
        if a + step < d {
            color.gamma_multiply(0.45)
        } else {
            color
        }
    });
    // Kilometer ticks.
    let mut km = 1000.0;
    while km < length {
        let x = x_of(start + km);
        p.line_segment(
            [
                egui::pos2(x, rect.bottom() - 4.0),
                egui::pos2(x, rect.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::WHITE.gamma_multiply(0.6)),
        );
        km += 1000.0;
    }
    if d >= start {
        let pos = egui::pos2(x_of(d.min(end)), y_of(course.elevation_at(d.min(end))));
        p.circle_filled(pos, 6.0, egui::Color32::WHITE);
        p.circle_stroke(pos, 6.0, egui::Stroke::new(2.0, egui::Color32::BLACK));
    }
}
