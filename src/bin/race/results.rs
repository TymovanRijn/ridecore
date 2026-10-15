//! Results: general classification, sprint and KOM points, best team.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::race::Race;
use ridecore::race::course::SegmentKind;

use crate::share::{self, StravaUi};
use crate::{RaceRes, Screen};

pub struct ResultsPlugin;

impl Plugin for ResultsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            results_ui
                .run_if(in_state(Screen::Results))
                .run_if(crate::training::racing)
                .run_if(crate::theme::ready),
        );
    }
}

fn gap_text(gap_s: f32) -> String {
    if gap_s < 0.5 {
        "z.t.".into()
    } else {
        let s = gap_s.round() as u32;
        format!("+{}:{:02}", s / 60, s % 60)
    }
}

fn color(race: &Race, rider: usize) -> egui::Color32 {
    let [r, g, b] = race.teams[race.riders[rider].team].spec.color;
    egui::Color32::from_rgb(r, g, b)
}

fn rider_row(ui: &mut egui::Ui, race: &Race, rank: usize, rider: usize, right: String) {
    let r = &race.riders[rider];
    let text_color = if r.human {
        egui::Color32::from_rgb(255, 140, 0)
    } else {
        ui.visuals().text_color()
    };
    ui.label(egui::RichText::new(format!("{rank}.")).color(text_color));
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 5.0, color(race, rider));
        ui.label(egui::RichText::new(&r.profile.name).color(text_color));
    });
    ui.label(egui::RichText::new(&race.teams[r.team].spec.name).weak());
    ui.label(egui::RichText::new(right).color(text_color));
    ui.end_row();
}

fn results_ui(
    mut contexts: EguiContexts,
    res: Res<RaceRes>,
    mut strava: ResMut<StravaUi>,
    mut next: ResMut<NextState<Screen>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let race = &res.race;
    egui::Window::new("Uitslag")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .default_width(900.0)
        .show(ctx, |ui| {
            if let Some(p) = race.finish.iter().position(|f| f.rider == race.human) {
                let f = &race.finish[p];
                ui.heading(
                    egui::RichText::new(format!(
                        "Je werd {}e van {} · {}",
                        p + 1,
                        race.riders.len(),
                        gap_text(f.gap_s)
                    ))
                    .size(26.0),
                );
            }
            let (xp, done, tried) = res.xp;
            ui.label(
                egui::RichText::new(format!("{xp} XP · {done} van {tried} uitdagingen gehaald"))
                    .size(20.0)
                    .strong()
                    .color(egui::Color32::from_rgb(255, 200, 40)),
            );
            share_panel(ui, &res, &mut strava);
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                cols[0].strong("Einduitslag");
                egui::ScrollArea::vertical()
                    .id_salt("gc")
                    .max_height(420.0)
                    .show(&mut cols[0], |ui| {
                        egui::Grid::new("gc")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for (k, f) in race.finish.iter().enumerate() {
                                    rider_row(ui, race, k + 1, f.rider, gap_text(f.gap_s));
                                }
                            });
                        let still = race.riders.len() - race.finish.len();
                        if still > 0 {
                            ui.label(
                                egui::RichText::new(format!("{still} renners nog onderweg…"))
                                    .weak(),
                            );
                        }
                    });

                for (title, kind) in [
                    ("Puntenklassement (sprints)", SegmentKind::Sprint),
                    ("Bergklassement", SegmentKind::Kom),
                ] {
                    let pts = race.points(kind);
                    if pts.is_empty() {
                        continue;
                    }
                    cols[1].strong(title);
                    egui::Grid::new(title)
                        .num_columns(4)
                        .striped(true)
                        .show(&mut cols[1], |ui| {
                            for (k, (rider, p)) in pts.iter().take(5).enumerate() {
                                rider_row(ui, race, k + 1, *rider, format!("{p} pt"));
                            }
                        });
                    cols[1].add_space(10.0);
                }

                cols[1].strong("Ploegenklassement (beste 3 tijden)");
                egui::Grid::new("teams")
                    .num_columns(2)
                    .striped(true)
                    .show(&mut cols[1], |ui| {
                        for (k, (team, time)) in team_ranking(race).iter().take(5).enumerate() {
                            ui.label(format!("{}. {}", k + 1, race.teams[*team].spec.name));
                            let s = *time as u32;
                            ui.label(format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60));
                            ui.end_row();
                        }
                    });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button(egui::RichText::new("Nieuwe wedstrijd").size(18.0))
                    .clicked()
                {
                    next.set(Screen::Setup);
                }
                ui.label(
                    egui::RichText::new("Je kunt uitfietsen terwijl de rest binnenkomt.").weak(),
                );
            });
        });
    Ok(())
}

/// Saved files, the stats card and the Strava upload.
pub fn share_panel(ui: &mut egui::Ui, res: &RaceRes, strava: &mut StravaUi) {
    strava.poll();
    match &res.saved {
        None => {
            ui.label("Rit opslaan…");
        }
        Some(Err(e)) => {
            ui.colored_label(egui::Color32::RED, e);
        }
        Some(Ok(ride)) => {
            let uploaded = strava.activity_url.lock().ok().and_then(|u| u.clone());
            ui.horizontal(|ui| {
                let can_upload = strava.connected.is_some() && !strava.busy() && uploaded.is_none();
                if ui
                    .add_enabled(
                        can_upload,
                        egui::Button::new(egui::RichText::new("Upload naar Strava").size(17.0)),
                    )
                    .clicked()
                {
                    strava.upload(ride);
                }
                if let Some(card) = &ride.card {
                    if ui.button("Open stats-kaart").clicked() {
                        share::open_file(card);
                    }
                    if ui.button("Kopieer kaart").clicked() && share::copy_image(card) {
                        strava.note("Kaart gekopieerd: plak hem als foto bij je Strava-rit.");
                    }
                }
                if let Some(url) = &uploaded {
                    ui.hyperlink_to("Bekijk op Strava", url);
                }
            });
            if strava.connected.is_none() {
                ui.label(
                    egui::RichText::new("Verbind Strava in het startscherm om te uploaden.").weak(),
                );
            }
            let status = strava.status();
            if !status.is_empty() {
                ui.label(status);
            }
            ui.label(
                egui::RichText::new(format!("Opgeslagen in {}", ride.fit.display()))
                    .weak()
                    .small(),
            );
        }
    }
}

/// Teams by the sum of their three fastest finish times.
fn team_ranking(race: &Race) -> Vec<(usize, f32)> {
    let mut out: Vec<(usize, f32)> = (0..race.teams.len())
        .filter_map(|t| {
            let times: Vec<f32> = race
                .finish
                .iter()
                .filter(|f| race.riders[f.rider].team == t)
                .map(|f| f.time)
                .take(3)
                .collect();
            (times.len() == 3).then(|| (t, times.iter().sum()))
        })
        .collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}
