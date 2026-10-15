//! TV-style captions: the village you ride into and the climb you reach,
//! with its length and gradient, in a lower third.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::race::course::Course;

use crate::theme::{self, ACCENT, MUTED, TEXT, display};
use crate::{RaceRes, Screen};

/// How long a caption stays up.
const SHOW_S: f32 = 6.0;

struct Caption {
    /// Race distance at which it shows.
    at_m: f64,
    title: String,
    subtitle: String,
    climb: bool,
}

#[derive(Resource, Default)]
struct Captions {
    list: Vec<Caption>,
    next: usize,
    shown_at: Option<(usize, f32)>,
}

pub struct CaptionsPlugin;

impl Plugin for CaptionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Captions>()
            .add_systems(OnEnter(Screen::Racing), plan)
            .add_systems(
                EguiPrimaryContextPass,
                show.run_if(in_state(Screen::Racing))
                    .run_if(crate::theme::ready),
            );
    }
}

/// Villages and towns the road runs through (from the map) and every named
/// climb, in race order.
fn plan(res: Res<RaceRes>, mut captions: ResMut<Captions>) {
    let course = &res.race.course;
    let mut list = Vec::new();
    let lap = course.route.length_m();
    if let Some(geo) = course.route.geo.as_ref()
        && let Some(map) = geo.map()
    {
        // The road, sampled every 20 m, to find where it passes each place.
        let road: Vec<(f64, [f32; 2])> = (0..=(lap / 20.0) as usize)
            .map(|k| {
                let d = k as f64 * 20.0;
                let p = course.path.pose(&course.route, d, 0.0).pos;
                (d, [p[0], p[2]])
            })
            .collect();
        for place in &map.places {
            if !matches!(place.kind.as_str(), "village" | "town" | "city" | "suburb") {
                continue;
            }
            let Some((d, dist)) = road
                .iter()
                .map(|(d, p)| (*d, (p[0] - place.pos[0]).hypot(p[1] - place.pos[1])))
                .min_by(|a, b| a.1.total_cmp(&b.1))
            else {
                continue;
            };
            if dist > 350.0 {
                continue;
            }
            for l in 0..course.laps {
                list.push(Caption {
                    at_m: (d - 250.0).max(30.0) + lap * l as f64,
                    title: place.name.to_uppercase(),
                    subtitle: course.route.region.clone(),
                    climb: false,
                });
            }
        }
    }
    climbs(course, &mut list);
    list.sort_by(|a, b| a.at_m.total_cmp(&b.at_m));
    // Two captions too close together: keep the climb.
    let mut kept: Vec<Caption> = Vec::new();
    for c in list {
        match kept.last_mut() {
            Some(last) if c.at_m - last.at_m < 400.0 => {
                if c.climb && !last.climb {
                    *last = c;
                }
            }
            _ => kept.push(c),
        }
    }
    *captions = Captions {
        list: kept,
        next: 0,
        shown_at: None,
    };
}

fn climbs(course: &Course, list: &mut Vec<Caption>) {
    for c in &course.climbs {
        let Some(name) = &c.name else { continue };
        list.push(Caption {
            at_m: c.start_m,
            title: name.clone(),
            subtitle: format!(
                "{} · {:.1} km aan {:.1}% · max {:.0}%",
                c.category.label(),
                c.length_m() / 1000.0,
                c.avg_pct,
                c.max_pct
            ),
            climb: true,
        });
    }
}

fn show(
    mut contexts: EguiContexts,
    time: Res<Time>,
    res: Res<RaceRes>,
    mut captions: ResMut<Captions>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let d = res.race.player().dist;
    let now = time.elapsed_secs();
    // Skip what's already behind (e.g. a start further down the road).
    while captions
        .list
        .get(captions.next)
        .is_some_and(|c| d > c.at_m + 400.0)
    {
        captions.next += 1;
    }
    if let Some(c) = captions.list.get(captions.next)
        && d >= c.at_m
    {
        let k = captions.next;
        captions.shown_at = Some((k, now));
        captions.next += 1;
    }
    let Some((k, since)) = captions.shown_at else {
        return Ok(());
    };
    let age = now - since;
    if age > SHOW_S {
        captions.shown_at = None;
        return Ok(());
    }
    let c = &captions.list[k];
    // Slide in, hold, fade out.
    let fade = (age / 0.35).min(1.0) * ((SHOW_S - age) / 0.6).clamp(0.0, 1.0);
    let slide = (1.0 - (age / 0.35).min(1.0)).powi(2) * -60.0;
    egui::Area::new("caption".into())
        .anchor(egui::Align2::LEFT_BOTTOM, [16.0 + slide, -150.0])
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(12, 14, 20, 240))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(18, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (bar, _) =
                            ui.allocate_exact_size(egui::vec2(5.0, 48.0), egui::Sense::hover());
                        let color = if c.climb {
                            theme::grade_color(9.0)
                        } else {
                            ACCENT
                        };
                        ui.painter().rect_filled(bar, 2.0, color);
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&c.title)
                                        .font(display(30.0))
                                        .color(TEXT),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                            ui.add(
                                egui::Label::new(egui::RichText::new(&c.subtitle).color(MUTED))
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                    });
                });
        });
    Ok(())
}
