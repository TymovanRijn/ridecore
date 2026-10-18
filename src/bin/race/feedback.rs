//! Moment-to-moment feedback: the live objective card, celebratory and
//! warning toasts with sound, and effort effects (speed lines, a low-energy
//! vignette).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::i18n::{ordinal, t};
use ridecore::race::challenges::{ChallengeTracker, Event};
use ridecore::race::course::SegmentKind;
use ridecore::race::moments::{Moment, MomentTracker};
use ridecore::tf;

use crate::sound::{self, SoundBank};
use crate::{RaceRes, Screen};

const TOAST_LIFE_S: f64 = 3.2;
const GOOD: egui::Color32 = egui::Color32::from_rgb(90, 220, 120);
const GOLD: egui::Color32 = egui::Color32::from_rgb(255, 200, 40);
const BAD: egui::Color32 = egui::Color32::from_rgb(255, 90, 80);
const INFO: egui::Color32 = egui::Color32::from_rgb(120, 190, 255);

/// An optional sound and its volume to go with a toast.
type Sting<'a> = Option<(&'a Handle<AudioSource>, f32)>;

struct Toast {
    title: String,
    detail: String,
    color: egui::Color32,
    born: f64,
}

#[derive(Resource)]
pub struct Feedback {
    tracker: MomentTracker,
    pub challenges: ChallengeTracker,
    /// When XP was last earned, for the counter's pop.
    xp_at: f64,
    toasts: Vec<Toast>,
    /// Places gained and when, for the flash next to your position.
    pub position_flash: Option<(usize, f64)>,
}

impl Default for Feedback {
    fn default() -> Self {
        Self {
            tracker: MomentTracker::default(),
            challenges: ChallengeTracker::new(ridecore::fit::now_unix() as u64),
            xp_at: f64::MIN,
            toasts: Vec::new(),
            position_flash: None,
        }
    }
}

pub struct FeedbackPlugin;

impl Plugin for FeedbackPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Feedback>()
            .add_systems(OnEnter(Screen::Racing), |mut fb: ResMut<Feedback>| {
                *fb = Feedback::default()
            })
            .add_systems(
                Update,
                detect_moments
                    .after(crate::advance_race)
                    .run_if(in_state(Screen::Racing))
                    .run_if(crate::training::racing)
                    .run_if(crate::watch::riding),
            )
            .add_systems(
                EguiPrimaryContextPass,
                draw_feedback
                    .run_if(in_state(Screen::Racing))
                    .run_if(crate::training::racing)
                    .run_if(crate::watch::riding)
                    .run_if(crate::theme::ready),
            );
    }
}

fn jersey_name(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Sprint => t("groene trui", "green jersey"),
        SegmentKind::Kom => t("bolletjestrui", "polka-dot jersey"),
    }
}

fn detect_moments(
    mut commands: Commands,
    time: Res<Time>,
    mut res: ResMut<RaceRes>,
    bank: Res<SoundBank>,
    mut fb: ResMut<Feedback>,
) {
    if res.paused {
        return;
    }
    let now = time.elapsed_secs_f64();
    let moments = fb.tracker.update(&res.race, time.delta_secs());
    for m in moments {
        let (title, detail, color, sting): (String, String, egui::Color32, Sting) = match m {
            Moment::PositionGain(n) => {
                let total = fb
                    .position_flash
                    .filter(|(_, t)| now - t < 2.0)
                    .map(|(g, _)| g)
                    .unwrap_or(0)
                    + n;
                fb.position_flash = Some((total, now));
                continue;
            }
            Moment::Segment {
                kind,
                place,
                points,
            } => {
                let what = if kind == SegmentKind::Sprint {
                    t("TUSSENSPRINT", "SPRINT")
                } else {
                    "KOM"
                };
                let title = if place == 1 {
                    tf!("{what} GEWONNEN!", "{what} WON!")
                } else {
                    tf!(
                        "{} bij de {}",
                        "{} at the {}",
                        ordinal(place),
                        what.to_lowercase()
                    )
                };
                (
                    title,
                    tf!("+{points} punten", "+{points} points"),
                    if place == 1 { GOLD } else { GOOD },
                    Some((&bank.chime, 0.7)),
                )
            }
            Moment::JerseyTaken(kind) => (
                tf!("Je pakt de {}!", "You take the {}!", jersey_name(kind)),
                t("Leider in het klassement", "Leading the classification").into(),
                GOLD,
                Some((&bank.fanfare, 0.6)),
            ),
            Moment::JerseyLost(kind) => (
                tf!("{} kwijt", "{} lost", jersey_name(kind)),
                t(
                    "Pak hem terug bij de volgende kans",
                    "Take it back at the next chance",
                )
                .into(),
                egui::Color32::LIGHT_GRAY,
                None,
            ),
            Moment::LastLap => (
                t("LAATSTE RONDE", "LAST LAP").into(),
                t("Nu telt het!", "This is where it counts!").into(),
                GOLD,
                Some((&bank.bell, 0.8)),
            ),
            Moment::Bridged => (
                t("Aangesloten!", "Bridged across!").into(),
                t("Zit in het wiel en herstel", "Sit on a wheel and recover").into(),
                GOOD,
                Some((&bank.chime, 0.4)),
            ),
            Moment::Dropped => (
                t("Gelost", "Dropped").into(),
                t(
                    "Rij je eigen tempo of zet alles op alles",
                    "Ride your own pace or go all in",
                )
                .into(),
                BAD,
                None,
            ),
            Moment::InBreak => (
                t("Je zit in de vlucht!", "You're in the break!").into(),
                t("Het peloton laat je gaan", "The peloton lets you go").into(),
                INFO,
                Some((&bank.chime, 0.5)),
            ),
            Moment::EmptyTank => (
                t("W′ bijna leeg!", "W′ almost empty!").into(),
                t("Zoek een wiel en herstel", "Find a wheel and recover").into(),
                BAD,
                None,
            ),
            Moment::Finish { place } => {
                let (title, sting) = match place {
                    1 => (t("OVERWINNING!", "VICTORY!").to_string(), &bank.fanfare),
                    2 | 3 => (format!("PODIUM! {}", ordinal(place)), &bank.fanfare),
                    4..=10 => (format!("Top 10! {}", ordinal(place)), &bank.chime),
                    _ => (
                        tf!("Binnen als {}", "Finished {}", ordinal(place)),
                        &bank.chime,
                    ),
                };
                (
                    title,
                    t("Goed gereden!", "Well ridden!").into(),
                    if place <= 3 { GOLD } else { GOOD },
                    Some((sting, 0.8)),
                )
            }
        };
        if let Some((s, v)) = sting {
            sound::play(&mut commands, s, v);
        }
        fb.toasts.push(Toast {
            title,
            detail,
            color,
            born: now,
        });
    }
    let events = fb.challenges.update(&res.race, time.delta_secs());
    for e in events {
        let (title, detail, color, sting): (String, String, egui::Color32, Sting) = match e {
            Event::Started(c) => (
                c.title.clone(),
                format!("{} · +{} XP", c.detail, c.xp),
                INFO,
                Some((&bank.bell, 0.35)),
            ),
            Event::Completed { title, xp, streak } => {
                fb.xp_at = now;
                let streak = if streak >= 2 {
                    tf!(" · reeks x{streak}!", " · streak x{streak}!")
                } else {
                    String::new()
                };
                (
                    tf!("{title} gelukt!", "{title} done!"),
                    format!("+{xp} XP{streak}"),
                    GOLD,
                    Some((&bank.chime, 0.7)),
                )
            }
            Event::Failed { title } => (
                tf!("{title} gemist", "{title} missed"),
                t(
                    "Reeks verloren · volgende kans komt eraan",
                    "Streak lost · another chance is coming",
                )
                .into(),
                egui::Color32::LIGHT_GRAY,
                None,
            ),
            Event::Bonus { what, xp } => {
                fb.xp_at = now;
                // Shown with the segment/finish toast already; just pop the counter.
                let _ = (what, xp);
                continue;
            }
        };
        if let Some((s, v)) = sting {
            sound::play(&mut commands, s, v);
        }
        fb.toasts.push(Toast {
            title,
            detail,
            color,
            born: now,
        });
    }
    let ch = &fb.challenges;
    res.xp = (ch.xp, ch.completed, ch.attempted);
    fb.toasts.retain(|t| now - t.born < TOAST_LIFE_S);
}

fn draw_feedback(
    mut contexts: EguiContexts,
    time: Res<Time>,
    res: Res<RaceRes>,
    fb: Res<Feedback>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let race = &res.race;
    let now = time.elapsed_secs_f64();
    let t = time.elapsed_secs();

    // The live objective, under the race clock.
    if let Some(o) = race.objective() {
        egui::Area::new("objective".into())
            // Under the race strip.
            .anchor(egui::Align2::CENTER_TOP, [0.0, 196.0])
            .show(ctx, |ui| {
                crate::hud::see_through(ui);
                let pulse = if o.urgent {
                    0.75 + 0.25 * (t * 8.0).sin().abs()
                } else {
                    1.0
                };
                let accent = if o.urgent {
                    BAD.gamma_multiply(pulse)
                } else {
                    egui::Color32::from_rgb(255, 140, 0)
                };
                crate::hud::glass()
                    .stroke(egui::Stroke::new(1.5, accent))
                    .show(ui, |ui| {
                        // A fixed width so the text can't collapse to one
                        // letter per line.
                        ui.set_width(340.0);
                        ui.vertical_centered(|ui| {
                            let size = if o.urgent {
                                30.0 * pulse.max(0.9)
                            } else {
                                22.0
                            };
                            ui.label(
                                egui::RichText::new(o.title.to_uppercase())
                                    .font(crate::theme::display(size))
                                    .color(accent),
                            );
                            ui.label(
                                egui::RichText::new(&o.detail).color(egui::Color32::LIGHT_GRAY),
                            );
                            if let Some(p) = o.progress {
                                ui.add(
                                    egui::ProgressBar::new(p.clamp(0.0, 1.0))
                                        .desired_width(320.0)
                                        .desired_height(6.0)
                                        .fill(accent),
                                );
                            }
                        });
                    });
            });
    }

    // The running challenge and your XP, on the left.
    egui::Area::new("challenge".into())
        .anchor(egui::Align2::LEFT_CENTER, [16.0, 40.0])
        .interactable(false)
        .show(ctx, |ui| {
            crate::hud::see_through(ui);
            let ch = &fb.challenges;
            let pop = 1.0 + 0.4 * (1.0 - ((now - fb.xp_at) as f32 / 0.5).clamp(0.0, 1.0));
            crate::hud::glass().show(ui, |ui| {
                ui.set_width(250.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} XP", ch.xp))
                            .font(crate::theme::display(26.0 * pop))
                            .color(GOLD),
                    );
                    if ch.streak > 0 {
                        ui.label(
                            egui::RichText::new(tf!(
                                "reeks x{:.1}",
                                "streak x{:.1}",
                                ch.multiplier()
                            ))
                            .color(GOOD),
                        );
                    }
                });
                if let Some(c) = &ch.active {
                    let attack = c.title.starts_with("AANVAL") || c.title.starts_with("ATTACK");
                    let accent = if attack {
                        BAD.gamma_multiply(0.75 + 0.25 * (t * 8.0).sin().abs())
                    } else {
                        INFO
                    };
                    ui.label(
                        egui::RichText::new(c.title.to_uppercase())
                            .font(crate::theme::display(20.0))
                            .color(accent),
                    );
                    ui.label(egui::RichText::new(&c.detail).color(egui::Color32::LIGHT_GRAY));
                    ui.add(
                        egui::ProgressBar::new(c.progress)
                            .desired_width(250.0)
                            .desired_height(8.0)
                            .fill(accent),
                    );
                    ui.label(
                        egui::RichText::new(tf!(
                            "nog {:.0} s · +{:.0} XP",
                            "{:.0} s left · +{:.0} XP",
                            c.time_left.max(0.0),
                            c.xp as f32 * ch.multiplier()
                        ))
                        .weak(),
                    );
                } else {
                    ui.label(crate::hud::caps(&tf!(
                        "Uitdagingen {}/{}",
                        "Challenges {}/{}",
                        ch.completed,
                        ch.attempted
                    )));
                }
            });
        });

    // Toasts: pop in, hold, fade out.
    egui::Area::new("toasts".into())
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -150.0])
        .interactable(false)
        .show(ctx, |ui| {
            crate::hud::see_through(ui);
            ui.vertical_centered(|ui| {
                for toast in fb.toasts.iter().rev().take(3) {
                    let age = (now - toast.born) as f32;
                    let pop = 1.0 + 0.35 * (1.0 - (age / 0.25).min(1.0)).powi(2);
                    let alpha = (1.0 - ((age - (TOAST_LIFE_S as f32 - 0.6)) / 0.6).max(0.0))
                        .clamp(0.0, 1.0);
                    // Never wrap: an unsized area would break every letter.
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&toast.title)
                                .size(38.0 * pop)
                                .strong()
                                .color(toast.color.gamma_multiply(alpha)),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&toast.detail)
                                .size(18.0)
                                .color(egui::Color32::WHITE.gamma_multiply(alpha)),
                        )
                        .wrap_mode(egui::TextWrapMode::Extend),
                    );
                    ui.add_space(10.0);
                }
            });
        });

    let me = race.player();
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        "effects".into(),
    ));

    // Speed lines when you're going all-out or very fast.
    let effort = ((me.power / me.profile.ftp - 1.3) / 0.7).clamp(0.0, 1.0);
    let speed = ((me.speed * 3.6 - 50.0) / 20.0).clamp(0.0, 1.0);
    let intensity = effort.max(speed);
    if intensity > 0.0 && !res.paused {
        let c = screen.center();
        for k in 0..36 {
            let seed = k as f32 * 12.9898;
            let angle = seed.sin() * 1000.0 % std::f32::consts::TAU;
            let phase = (t * 2.5 + (seed.cos() * 43.0).fract()).fract();
            let dir = egui::vec2(angle.cos(), angle.sin());
            let reach = screen.width().max(screen.height()) * 0.6;
            let a = c + dir * reach * (0.45 + phase * 0.5);
            let b = a + dir * reach * 0.18;
            let alpha = intensity * (1.0 - phase) * 0.55;
            painter.line_segment(
                [a, b],
                egui::Stroke::new(2.0, egui::Color32::WHITE.gamma_multiply(alpha)),
            );
        }
    }

    // Red heartbeat vignette when W′ is nearly empty.
    let low = ((0.2 - me.energy.fraction()) / 0.2).clamp(0.0, 1.0);
    if low > 0.0 && !res.paused {
        let beat = 0.6 + 0.4 * (t * 7.0).sin().abs();
        for k in 0..14 {
            let inset = k as f32 * 6.0;
            let alpha = low * beat * 0.09 * (1.0 - k as f32 / 14.0);
            painter.rect_stroke(
                screen.shrink(inset),
                0.0,
                egui::Stroke::new(7.0, BAD.gamma_multiply(alpha)),
                egui::StrokeKind::Inside,
            );
        }
    }
    Ok(())
}
