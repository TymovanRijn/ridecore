//! The workout builder: stack blocks (steady, ramp, intervals, free ride)
//! in watts or % FTP, with a live profile, the numbers that matter, ready
//! made blocks, zone shortcuts and undo.

use bevy_egui::egui;

use ridecore::custom::{self, Block, CustomWorkout, Unit};
use ridecore::i18n::t;
use ridecore::tf;
use ridecore::training::Zone;
use ridecore::workout::Segment;

use crate::theme::{self, ACCENT, ACCENT_SOFT, MUTED, TEXT, card, display, stat};
use crate::training::{big_button, mmss, zone_color};

/// A workout being edited.
pub struct Builder {
    pub workout: CustomWorkout,
    /// Not saved yet: cancelling throws it away.
    pub is_new: bool,
    selected: Option<usize>,
    /// Scroll the list to the selected block on the next frame.
    reveal: bool,
    undo: Vec<(Unit, Vec<Block>)>,
    redo: Vec<(Unit, Vec<Block>)>,
}

pub enum Action {
    None,
    Cancel,
    Save,
}

const UNDO_DEPTH: usize = 100;

impl Builder {
    pub fn new(workout: CustomWorkout, is_new: bool) -> Self {
        let selected = (!workout.blocks.is_empty()).then_some(0);
        Self {
            workout,
            is_new,
            selected,
            reveal: false,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    fn snapshot(&self) -> (Unit, Vec<Block>) {
        (self.workout.unit, self.workout.blocks.clone())
    }

    fn remember(&mut self, before: (Unit, Vec<Block>)) {
        if self.undo.last() == Some(&before) {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, state: (Unit, Vec<Block>)) {
        self.workout.unit = state.0;
        self.workout.blocks = state.1;
        let n = self.workout.blocks.len();
        self.selected = self.selected.filter(|&i| i < n).or(n.checked_sub(1));
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
        }
    }

    /// Puts a block after the selected one (or at the end) and selects it.
    fn insert(&mut self, block: Block) {
        let at = self
            .selected
            .map(|i| i + 1)
            .unwrap_or(self.workout.blocks.len())
            .min(self.workout.blocks.len());
        self.remember(self.snapshot());
        self.workout.blocks.insert(at, block);
        self.selected = Some(at);
        self.reveal = true;
    }
}

/// A new block of `kind`, at sensible powers for this FTP.
fn fresh(w: &CustomWorkout, kind: usize, ftp: f32) -> Block {
    let p = |pct| w.from_percent(pct, ftp);
    match kind {
        0 => Block::Steady {
            secs: 300,
            power: p(75.0),
            cadence: None,
        },
        1 => Block::Ramp {
            secs: 300,
            from: p(55.0),
            to: p(85.0),
            cadence: None,
        },
        2 => Block::Intervals {
            reps: 5,
            on_secs: 60,
            on: p(110.0),
            off_secs: 60,
            off: p(55.0),
            cadence: None,
        },
        _ => Block::Free { secs: 300 },
    }
}

/// The whole builder, filling `ui`.
pub fn builder_ui(ui: &mut egui::Ui, b: &mut Builder, ftp: f32) -> Action {
    let before = b.snapshot();
    let mut touched = false;
    let mut action = Action::None;

    // Keyboard: undo/redo and delete, unless you're typing.
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing {
        let (undo, redo, delete) = ui.input(|i| {
            let cmd = i.modifiers.command;
            (
                cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if undo {
            b.undo();
        } else if redo {
            b.redo();
        } else if delete && let Some(i) = b.selected {
            b.remember(before.clone());
            b.workout.blocks.remove(i);
            let n = b.workout.blocks.len();
            b.selected = (n > 0).then(|| i.min(n - 1));
        }
    }

    let gap = 16.0;
    let right_w = (ui.available_width() * 0.32).clamp(300.0, 420.0);
    let left_w = ui.available_width() - right_w - gap - 10.0;
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.allocate_ui_with_layout(
            egui::vec2(left_w, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(left_w);
                top_card(ui, b, ftp, &mut touched);
                ui.add_space(8.0);
                block_list(ui, b, ftp, &mut touched);
            },
        );
        ui.allocate_ui_with_layout(
            egui::vec2(right_w, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(right_w);
                egui::ScrollArea::vertical()
                    .id_salt("builder-side")
                    .max_height(height - 150.0)
                    .show(ui, |ui| side_panel(ui, b, ftp));
                card(ui, "", |ui| {
                    let can_save = !b.workout.blocks.is_empty();
                    if big_button(
                        ui,
                        t("OPSLAAN", "SAVE"),
                        if can_save {
                            ACCENT
                        } else {
                            egui::Color32::from_gray(70)
                        },
                        ui.available_width(),
                    ) && can_save
                    {
                        action = Action::Save;
                    }
                    if !can_save {
                        ui.label(
                            egui::RichText::new(t(
                                "Voeg minstens één blok toe.",
                                "Add at least one block.",
                            ))
                            .color(MUTED),
                        );
                    }
                    ui.add_space(4.0);
                    let cancel = if b.is_new {
                        t("Annuleren", "Cancel")
                    } else {
                        t("Wijzigingen weggooien", "Discard changes")
                    };
                    if ui.button(cancel).clicked() {
                        action = Action::Cancel;
                    }
                });
            },
        );
    });

    if touched {
        b.remember(before);
    }
    action
}

/// Name, unit, undo, the live profile and the numbers.
fn top_card(ui: &mut egui::Ui, b: &mut Builder, ftp: f32, touched: &mut bool) {
    card(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut b.workout.name)
                    .font(display(30.0))
                    .hint_text(t("Naam van je training", "Name your workout"))
                    .desired_width(ui.available_width() - 330.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(
                    ui,
                    Icon::Redo,
                    t("Opnieuw (⌘⇧Z)", "Redo (⌘⇧Z)"),
                    !b.redo.is_empty(),
                )
                .clicked()
                {
                    b.redo();
                }
                if icon_button(
                    ui,
                    Icon::Undo,
                    t("Ongedaan maken (⌘Z)", "Undo (⌘Z)"),
                    !b.undo.is_empty(),
                )
                .clicked()
                {
                    b.undo();
                }
                ui.add_space(10.0);
                for (unit, label, tip) in [
                    (
                        Unit::Percent,
                        "% FTP",
                        t(
                            "Schaalt mee als je FTP verandert",
                            "Scales when your FTP changes",
                        ),
                    ),
                    (
                        Unit::Watts,
                        t("Watt", "Watts"),
                        t(
                            "Vaste wattages, ook als je FTP verandert",
                            "Fixed watts, even when your FTP changes",
                        ),
                    ),
                ] {
                    if ui
                        .selectable_label(b.workout.unit == unit, label)
                        .on_hover_text(tip)
                        .clicked()
                        && b.workout.unit != unit
                    {
                        b.workout.set_unit(unit, ftp);
                        *touched = true;
                    }
                }
            });
        });
        ui.add_space(4.0);
        if let Some(i) = graph(ui, &b.workout, b.selected, ftp) {
            b.selected = Some(i);
            b.reveal = true;
        }
        let s = custom::stats(&b.workout, ftp);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            stat(ui, mmss(s.secs as f32), t("duur", "duration"), TEXT);
            ui.add_space(14.0);
            stat(ui, format!("{:.0}", s.tss), "tss", ACCENT);
            ui.add_space(14.0);
            stat(ui, format!("{:.2}", s.intensity), "IF", TEXT);
            ui.add_space(14.0);
            stat(
                ui,
                format!("{:.0} W", s.avg_w),
                t("gemiddeld", "average"),
                TEXT,
            );
            ui.add_space(14.0);
            stat(ui, format!("{:.0} W", s.np_w), "NP", TEXT);
            ui.add_space(14.0);
            stat(ui, format!("{:.0}", s.kj), "kJ", TEXT);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(tf!("bij FTP {ftp:.0} W", "at FTP {ftp:.0} W"))
                        .small()
                        .color(MUTED),
                );
            });
        });
    });
}

/// The workout drawn block by block, colored by zone. Returns a clicked block.
fn graph(ui: &mut egui::Ui, w: &CustomWorkout, selected: Option<usize>, ftp: f32) -> Option<usize> {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 190.0),
        egui::Sense::click(),
    );
    let p = ui.painter_at(rect.expand(2.0));
    p.rect_filled(rect, 10.0, egui::Color32::from_black_alpha(70));
    let total = w.secs().max(1) as f32;
    if w.blocks.is_empty() {
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            t(
                "Voeg rechts je eerste blok toe →",
                "Add your first block on the right →",
            ),
            display(24.0),
            MUTED,
        );
        return None;
    }
    let plot = rect.shrink2(egui::vec2(8.0, 10.0));
    let plot = egui::Rect::from_min_max(plot.min, egui::pos2(plot.max.x, plot.max.y - 14.0));
    let top = w
        .segments(ftp)
        .iter()
        .filter_map(ridecore::training::peak)
        .fold(1.3f32, |a, b| a.max(b * 1.08));
    let x = |s: f32| plot.left() + s / total * plot.width();
    let y = |f: f32| plot.bottom() - (f / top).clamp(0.0, 1.0) * plot.height();

    let hover_x = resp.hover_pos().map(|h| h.x);
    let mut hovered = None;
    let mut t0 = 0.0;
    let mut spans = Vec::with_capacity(w.blocks.len());
    for (i, block) in w.blocks.iter().enumerate() {
        let t1 = t0 + block.secs() as f32;
        let (bx0, bx1) = (x(t0), x(t1));
        spans.push((bx0, bx1));
        if hover_x.is_some_and(|h| h >= bx0 && h < bx1) {
            hovered = Some(i);
        }
        let lit = selected == Some(i) || hovered == Some(i);
        if lit {
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(bx0, plot.top()),
                    egui::pos2(bx1, plot.bottom()),
                ),
                4.0,
                egui::Color32::from_white_alpha(if selected == Some(i) { 22 } else { 10 }),
            );
        }
        let mut s0 = t0;
        for seg in w.block_segments(block, ftp) {
            let s1 = s0 + seg.secs() as f32;
            let (a, bb, free) = match seg {
                Segment::Steady { power, .. } => (power, power, false),
                Segment::Ramp { from, to, .. } => (from, to, false),
                Segment::FreeRide { .. } => (0.6, 0.6, true),
            };
            let (ca, cb) = if free {
                (MUTED.gamma_multiply(0.5), MUTED.gamma_multiply(0.5))
            } else {
                (zone_color(Zone::of(a)), zone_color(Zone::of(bb)))
            };
            let dim = if selected.is_some() && selected != Some(i) {
                0.7
            } else {
                1.0
            };
            let mut m = egui::Mesh::default();
            let (xa, xb) = (x(s0), x(s1).max(x(s0) + 0.5));
            m.colored_vertex(egui::pos2(xa, y(a)), ca.gamma_multiply(dim));
            m.colored_vertex(egui::pos2(xb, y(bb)), cb.gamma_multiply(dim));
            m.colored_vertex(egui::pos2(xb, plot.bottom()), cb.gamma_multiply(dim * 0.75));
            m.colored_vertex(egui::pos2(xa, plot.bottom()), ca.gamma_multiply(dim * 0.75));
            m.add_triangle(0, 1, 2);
            m.add_triangle(0, 2, 3);
            p.add(m);
            s0 = s1;
        }
        if selected == Some(i) {
            p.rect_stroke(
                egui::Rect::from_min_max(
                    egui::pos2(bx0, plot.top()),
                    egui::pos2(bx1, plot.bottom()),
                ),
                4.0,
                egui::Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Outside,
            );
        }
        t0 = t1;
    }
    // FTP line.
    let fy = y(1.0);
    p.add(egui::Shape::dashed_line(
        &[egui::pos2(plot.left(), fy), egui::pos2(plot.right(), fy)],
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(110)),
        5.0,
        5.0,
    ));
    p.text(
        egui::pos2(plot.right() - 4.0, fy - 3.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("FTP {ftp:.0} W"),
        egui::FontId::proportional(11.0),
        egui::Color32::from_white_alpha(170),
    );
    // Minutes along the bottom.
    let step = [60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0]
        .into_iter()
        .find(|s| total / s <= 12.0)
        .unwrap_or(3600.0);
    let mut tick = step;
    while tick < total {
        let tx = x(tick);
        p.line_segment(
            [
                egui::pos2(tx, plot.bottom()),
                egui::pos2(tx, plot.bottom() + 4.0),
            ],
            egui::Stroke::new(1.0, MUTED),
        );
        p.text(
            egui::pos2(tx, plot.bottom() + 5.0),
            egui::Align2::CENTER_TOP,
            format!("{:.0}′", tick / 60.0),
            egui::FontId::proportional(10.0),
            MUTED,
        );
        tick += step;
    }
    if let Some(i) = hovered {
        resp.clone().on_hover_ui_at_pointer(|ui| {
            ui.label(describe(w, &w.blocks[i], ftp));
        });
    }
    if resp.clicked() {
        return hovered;
    }
    None
}

#[derive(Clone, Copy)]
enum Icon {
    Up,
    Down,
    Copy,
    Delete,
    Undo,
    Redo,
    Steady,
    Ramp,
    Intervals,
    Free,
}

/// Small line icons, drawn rather than taken from a font.
fn paint_icon(p: &egui::Painter, c: egui::Pos2, icon: Icon, color: egui::Color32) {
    use egui::{pos2, vec2};
    let s = egui::Stroke::new(1.8, color);
    let line = |pts: Vec<egui::Pos2>| egui::Shape::line(pts, s);
    match icon {
        Icon::Up => {
            p.add(line(vec![
                c + vec2(-5.0, 3.0),
                c + vec2(0.0, -3.0),
                c + vec2(5.0, 3.0),
            ]));
        }
        Icon::Down => {
            p.add(line(vec![
                c + vec2(-5.0, -3.0),
                c + vec2(0.0, 3.0),
                c + vec2(5.0, -3.0),
            ]));
        }
        Icon::Copy => {
            let r = egui::Rect::from_center_size(c + vec2(2.0, 2.0), vec2(9.0, 9.0));
            p.rect_stroke(r, 2.0, s, egui::StrokeKind::Middle);
            p.add(line(vec![
                c + vec2(-6.5, 2.5),
                c + vec2(-6.5, -6.5),
                c + vec2(2.5, -6.5),
            ]));
        }
        Icon::Delete => {
            p.line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], s);
            p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], s);
        }
        Icon::Undo | Icon::Redo => {
            let dir = if matches!(icon, Icon::Undo) {
                -1.0
            } else {
                1.0
            };
            // An arc over the top, with the arrowhead at its end.
            let pts: Vec<egui::Pos2> = (0..=10)
                .map(|k| {
                    let a = std::f32::consts::PI * k as f32 / 10.0;
                    c + vec2(-dir * 5.0 * a.cos(), 2.0 - 5.0 * a.sin())
                })
                .collect();
            let end = *pts.last().unwrap_or(&c);
            p.add(line(pts));
            p.add(line(vec![
                end + vec2(-dir * 3.5, -1.0),
                end + vec2(0.0, 3.0),
                end + vec2(dir * 3.0, -1.5),
            ]));
        }
        Icon::Steady => {
            p.rect_filled(
                egui::Rect::from_center_size(c + vec2(0.0, 2.0), vec2(16.0, 8.0)),
                1.5,
                color,
            );
        }
        Icon::Ramp => {
            p.add(egui::Shape::convex_polygon(
                vec![c + vec2(-8.0, 6.0), c + vec2(8.0, -6.0), c + vec2(8.0, 6.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Intervals => {
            for (k, h) in [10.0, 4.0, 10.0, 4.0].into_iter().enumerate() {
                let x = c.x - 7.5 + k as f32 * 4.5;
                p.rect_filled(
                    egui::Rect::from_min_max(pos2(x, c.y + 6.0 - h), pos2(x + 3.5, c.y + 6.0)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Free => {
            let pts: Vec<egui::Pos2> = (0..=16)
                .map(|k| {
                    let x = -8.0 + k as f32;
                    c + vec2(x, 2.5 * (x * 0.8).sin())
                })
                .collect();
            p.add(line(pts));
        }
    }
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, tip: &str, enabled: bool) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), sense);
    let bg = if enabled && resp.hovered() {
        egui::Color32::from_white_alpha(30)
    } else {
        egui::Color32::from_white_alpha(10)
    };
    ui.painter().rect_filled(rect, 6.0, bg);
    let color = if enabled {
        TEXT
    } else {
        MUTED.gamma_multiply(0.4)
    };
    paint_icon(ui.painter(), rect.center(), icon, color);
    resp.on_hover_text(tip)
}

/// A wide button with a drawn icon and a title.
fn add_button(ui: &mut egui::Ui, icon: Icon, text: &str, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 42.0), egui::Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        8.0,
        if resp.hovered() {
            egui::Color32::from_rgba_premultiplied(48, 54, 74, 240)
        } else {
            egui::Color32::from_rgba_premultiplied(30, 34, 48, 235)
        },
        egui::Stroke::new(1.0, theme::CARD_EDGE),
        egui::StrokeKind::Inside,
    );
    paint_icon(
        p,
        rect.left_center() + egui::vec2(22.0, 0.0),
        icon,
        ACCENT_SOFT,
    );
    p.text(
        rect.left_center() + egui::vec2(42.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        display(19.0),
        TEXT,
    );
    resp
}

/// "4 × (4:00 at 290 W / 3:00 at 140 W)".
fn describe(w: &CustomWorkout, b: &Block, ftp: f32) -> String {
    let pw = |v: f32| power_text(w, v, ftp);
    let d = |s: u32| mmss(s as f32);
    match *b {
        Block::Steady { secs, power, .. } => {
            format!("{} · {} · {}", b.kind_label(), d(secs), pw(power))
        }
        Block::Ramp { secs, from, to, .. } => {
            format!(
                "{} · {} · {} → {}",
                b.kind_label(),
                d(secs),
                pw(from),
                pw(to)
            )
        }
        Block::Intervals {
            reps,
            on_secs,
            on,
            off_secs,
            off,
            ..
        } => tf!(
            "{reps} × ({} op {} / {} op {})",
            "{reps} × ({} at {} / {} at {})",
            d(on_secs),
            pw(on),
            d(off_secs),
            pw(off)
        ),
        Block::Free { secs } => format!("{} · {}", b.kind_label(), d(secs)),
    }
}

/// "250 W", or "95% · 238 W".
fn power_text(w: &CustomWorkout, v: f32, ftp: f32) -> String {
    match w.unit {
        Unit::Watts => format!("{v:.0} W"),
        Unit::Percent => format!("{v:.0}% · {:.0} W", w.watts(v, ftp)),
    }
}

/// Every block as an editable row.
fn block_list(ui: &mut egui::Ui, b: &mut Builder, ftp: f32, touched: &mut bool) {
    enum Op {
        Up(usize),
        Down(usize),
        Copy(usize),
        Remove(usize),
    }
    let mut op = None;
    let n = b.workout.blocks.len();
    let reveal = std::mem::take(&mut b.reveal);
    egui::ScrollArea::vertical()
        .id_salt("builder-blocks")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if n == 0 {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(t(
                            "Nog leeg. Kies rechts een blok of een kant-en-klaar onderdeel.",
                            "Still empty. Pick a block or a ready-made part on the right.",
                        ))
                        .color(MUTED),
                    );
                });
            }
            for i in 0..n {
                let unit = b.workout.unit;
                let peak = b.workout.blocks[i].peak();
                let color = match peak {
                    Some(v) => zone_color(Zone::of(b.workout.fraction(v, ftp))),
                    None => MUTED,
                };
                let selected = b.selected == Some(i);
                let frame = egui::Frame::new()
                    .fill(if selected {
                        egui::Color32::from_rgba_premultiplied(44, 34, 26, 240)
                    } else {
                        egui::Color32::from_rgba_premultiplied(22, 26, 38, 230)
                    })
                    .stroke(egui::Stroke::new(
                        if selected { 1.5 } else { 1.0 },
                        if selected { ACCENT } else { theme::CARD_EDGE },
                    ))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 10,
                        top: 8,
                        bottom: 8,
                    });
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{}", i + 1))
                                    .font(display(20.0))
                                    .color(color),
                            );
                            ui.label(
                                egui::RichText::new(
                                    b.workout.blocks[i].kind_label().to_uppercase(),
                                )
                                .font(display(17.0))
                                .color(TEXT),
                            );
                            if let Some(v) = peak {
                                let z = Zone::of(b.workout.fraction(v, ftp));
                                ui.label(egui::RichText::new(z.label()).small().color(color));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(
                                        ui,
                                        Icon::Delete,
                                        t("Verwijderen", "Delete"),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        op = Some(Op::Remove(i));
                                    }
                                    if icon_button(
                                        ui,
                                        Icon::Copy,
                                        t("Dupliceren", "Duplicate"),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        op = Some(Op::Copy(i));
                                    }
                                    if icon_button(
                                        ui,
                                        Icon::Down,
                                        t("Omlaag", "Move down"),
                                        i + 1 < n,
                                    )
                                    .clicked()
                                    {
                                        op = Some(Op::Down(i));
                                    }
                                    if icon_button(ui, Icon::Up, t("Omhoog", "Move up"), i > 0)
                                        .clicked()
                                    {
                                        op = Some(Op::Up(i));
                                    }
                                    ui.label(
                                        egui::RichText::new(
                                            mmss(b.workout.blocks[i].secs() as f32),
                                        )
                                        .font(display(18.0))
                                        .color(ACCENT_SOFT),
                                    );
                                },
                            );
                        });
                        let w = &b.workout;
                        let mut block = w.blocks[i].clone();
                        let mut changed = block_fields(ui, &mut block, unit, w, ftp, touched);
                        if selected {
                            changed |= zone_chips(ui, &mut block, w, ftp, touched);
                        }
                        if changed {
                            b.workout.blocks[i] = block;
                        }
                    })
                    .response;
                // Strip in the zone color down the left edge.
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        resp.rect.min + egui::vec2(1.0, 8.0),
                        egui::vec2(4.0, resp.rect.height() - 16.0),
                    ),
                    2.0,
                    color,
                );
                if resp.interact(egui::Sense::click()).clicked() {
                    b.selected = Some(i);
                }
                if reveal && selected {
                    resp.scroll_to_me(Some(egui::Align::Center));
                }
                ui.add_space(6.0);
            }
        });
    if let Some(op) = op {
        *touched = true;
        let blocks = &mut b.workout.blocks;
        match op {
            Op::Up(i) => {
                blocks.swap(i, i - 1);
                b.selected = Some(i - 1);
            }
            Op::Down(i) => {
                blocks.swap(i, i + 1);
                b.selected = Some(i + 1);
            }
            Op::Copy(i) => {
                let copy = blocks[i].clone();
                blocks.insert(i + 1, copy);
                b.selected = Some(i + 1);
            }
            Op::Remove(i) => {
                blocks.remove(i);
                let n = blocks.len();
                b.selected = (n > 0).then(|| i.min(n - 1));
            }
        }
    }
}

/// The editable numbers of one block. Returns true when something changed.
fn block_fields(
    ui: &mut egui::Ui,
    block: &mut Block,
    unit: Unit,
    w: &CustomWorkout,
    ftp: f32,
    touched: &mut bool,
) -> bool {
    let mut changed = false;
    let mut track = |r: egui::Response| {
        if r.drag_started() || r.gained_focus() {
            *touched = true;
        }
        changed |= r.changed();
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        match block {
            Block::Steady {
                secs,
                power,
                cadence,
            } => {
                track(field_label(ui, t("duur", "time")));
                track(duration(ui, secs));
                track(field_label(ui, t("vermogen", "power")));
                track(power_field(ui, power, unit));
                equivalent(ui, *power, w, ftp);
                track(cadence_field(ui, cadence));
            }
            Block::Ramp {
                secs,
                from,
                to,
                cadence,
            } => {
                track(field_label(ui, t("duur", "time")));
                track(duration(ui, secs));
                track(field_label(ui, t("van", "from")));
                track(power_field(ui, from, unit));
                track(field_label(ui, t("naar", "to")));
                track(power_field(ui, to, unit));
                equivalent(ui, from.max(*to), w, ftp);
                track(cadence_field(ui, cadence));
            }
            Block::Intervals {
                reps,
                on_secs,
                on,
                off_secs,
                off,
                cadence,
            } => {
                track(
                    ui.add(
                        egui::DragValue::new(reps)
                            .range(1..=60)
                            .speed(0.1)
                            .suffix(" ×"),
                    ),
                );
                track(field_label(ui, t("aan", "on")));
                track(duration(ui, on_secs));
                track(power_field(ui, on, unit));
                track(field_label(ui, t("rust", "off")));
                track(duration(ui, off_secs));
                track(power_field(ui, off, unit));
                equivalent(ui, *on, w, ftp);
                track(cadence_field(ui, cadence));
            }
            Block::Free { secs } => {
                track(field_label(ui, t("duur", "time")));
                track(duration(ui, secs));
                ui.label(
                    egui::RichText::new(t(
                        "geen doel: rij zoals je wilt",
                        "no target: ride as you like",
                    ))
                    .small()
                    .color(MUTED),
                );
            }
        }
    });
    changed
}

/// One click to put the selected block in a zone. For intervals it sets
/// the on power; a ramp keeps its slope and ends in the zone.
fn zone_chips(
    ui: &mut egui::Ui,
    block: &mut Block,
    w: &CustomWorkout,
    ftp: f32,
    touched: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
        ui.label(
            egui::RichText::new(t("zone:", "zone:"))
                .small()
                .color(MUTED),
        );
        for (zone, pct) in [
            (Zone::Recovery, 50.0),
            (Zone::Endurance, 68.0),
            (Zone::Tempo, 82.0),
            (Zone::SweetSpot, 90.0),
            (Zone::Threshold, 100.0),
            (Zone::Vo2max, 115.0),
            (Zone::Anaerobic, 135.0),
        ] {
            let c = zone_color(zone);
            let text = egui::RichText::new(format!("{} {pct:.0}%", zone.label()))
                .small()
                .color(c);
            if !ui
                .add(egui::Button::new(text).stroke(egui::Stroke::new(1.0, c.gamma_multiply(0.6))))
                .clicked()
            {
                continue;
            }
            let v = w.from_percent(pct, ftp);
            match block {
                Block::Steady { power, .. } => *power = v,
                Block::Ramp { from, to, .. } => {
                    let d = *to - *from;
                    *to = v;
                    *from = (v - d).max(0.0);
                }
                Block::Intervals { on, .. } => *on = v,
                Block::Free { secs } => {
                    *block = Block::Steady {
                        secs: *secs,
                        power: v,
                        cadence: None,
                    }
                }
            }
            *touched = true;
            changed = true;
        }
    });
    changed
}

fn field_label(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(egui::RichText::new(text).small().color(MUTED))
}

/// Seconds, shown as m:ss. Type "5" for 5 minutes, "45s", or "1:30".
fn duration(ui: &mut egui::Ui, secs: &mut u32) -> egui::Response {
    ui.add(
        egui::DragValue::new(secs)
            .range(5..=4 * 3600)
            .speed(2.0)
            .custom_formatter(|v, _| mmss(v as f32))
            .custom_parser(parse_duration),
    )
    .on_hover_text(t(
        "Sleep, of typ: 5 = 5 min, 45s, 1:30",
        "Drag, or type: 5 = 5 min, 45s, 1:30",
    ))
}

fn parse_duration(text: &str) -> Option<f64> {
    let s = text.trim().to_lowercase().replace(',', ".");
    if s.contains(':') {
        let parts: Vec<f64> = s
            .split(':')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        return Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p));
    }
    if let Some(n) = s.strip_suffix("sec").or_else(|| s.strip_suffix('s')) {
        return n.trim().parse().ok();
    }
    if let Some(n) = s.strip_suffix("min").or_else(|| s.strip_suffix('m')) {
        return n.trim().parse::<f64>().ok().map(|m| m * 60.0);
    }
    if let Some(n) = s.strip_suffix('h').or_else(|| s.strip_suffix('u')) {
        return n.trim().parse::<f64>().ok().map(|h| h * 3600.0);
    }
    s.parse::<f64>().ok().map(|m| m * 60.0)
}

fn power_field(ui: &mut egui::Ui, v: &mut f32, unit: Unit) -> egui::Response {
    match unit {
        Unit::Watts => ui.add(
            egui::DragValue::new(v)
                .range(0.0..=2000.0)
                .speed(1.0)
                .suffix(" W")
                .max_decimals(0),
        ),
        Unit::Percent => ui.add(
            egui::DragValue::new(v)
                .range(0.0..=400.0)
                .speed(0.5)
                .suffix("%")
                .max_decimals(0),
        ),
    }
}

/// The power in the other unit, quietly.
fn equivalent(ui: &mut egui::Ui, v: f32, w: &CustomWorkout, ftp: f32) {
    let text = match w.unit {
        Unit::Watts => format!("= {:.0}% FTP", w.fraction(v, ftp) * 100.0),
        Unit::Percent => format!("= {:.0} W", w.watts(v, ftp)),
    };
    ui.label(egui::RichText::new(text).small().color(MUTED));
}

/// An optional cadence target.
fn cadence_field(ui: &mut egui::Ui, cadence: &mut Option<u32>) -> egui::Response {
    let mut on = cadence.is_some();
    let mut r = ui
        .checkbox(&mut on, "rpm")
        .on_hover_text(t("Cadansdoel", "Cadence target"));
    if r.changed() {
        *cadence = on.then_some(90);
    }
    if let Some(c) = cadence {
        r = r.union(ui.add(egui::DragValue::new(c).range(50..=140)));
    }
    r
}

/// Adding blocks, ready-made parts, zone shortcuts and notes.
fn side_panel(ui: &mut egui::Ui, b: &mut Builder, ftp: f32) {
    card(ui, t("Blok toevoegen", "Add a block"), |ui| {
        let kinds = [
            (
                Icon::Steady,
                t("Blok", "Block"),
                t("Constant vermogen", "Steady power"),
            ),
            (
                Icon::Ramp,
                t("Oplopend", "Ramp"),
                t(
                    "Van het ene naar het andere vermogen",
                    "From one power to another",
                ),
            ),
            (
                Icon::Intervals,
                t("Intervallen", "Intervals"),
                t("Herhalingen van aan en rust", "Repeats of on and off"),
            ),
            (
                Icon::Free,
                t("Vrij", "Free"),
                t("Zonder doel, je rijdt zelf", "No target, you ride"),
            ),
        ];
        let w = (ui.available_width() - 10.0) / 2.0;
        for row in kinds.chunks(2).enumerate() {
            ui.horizontal(|ui| {
                for (k, (icon, name, tip)) in row.1.iter().enumerate() {
                    let kind = row.0 * 2 + k;
                    if add_button(ui, *icon, name, w).on_hover_text(*tip).clicked() {
                        let block = fresh(&b.workout, kind, ftp);
                        b.insert(block);
                    }
                }
            });
        }
    });
    ui.add_space(6.0);
    card(ui, t("Kant-en-klaar", "Ready-made"), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for tpl in custom::templates() {
                let block = b.workout.template_block(&tpl.block, ftp);
                if ui
                    .button(tpl.name)
                    .on_hover_text(describe(&b.workout, &block, ftp))
                    .clicked()
                {
                    b.insert(block);
                }
            }
        });
    });
    ui.add_space(6.0);
    card(ui, t("Notities", "Notes"), |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut b.workout.notes)
                .hint_text(t(
                    "Waar let je op? Dit zie je bij de training.",
                    "What to focus on? You'll see this with the workout.",
                ))
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.label(
            egui::RichText::new(t(
                "Tip: ⌘Z maakt ongedaan, Delete verwijdert het gekozen blok, en je kunt een .zwo-bestand op het venster slepen om het te importeren.",
                "Tip: ⌘Z undoes, Delete removes the selected block, and you can drop a .zwo file on the window to import it.",
            ))
            .small()
            .color(MUTED),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_goes_after_the_selection_and_undo_brings_it_back() {
        let mut b = Builder::new(CustomWorkout::new("x"), true);
        b.insert(fresh(&b.workout, 0, 250.0));
        b.insert(fresh(&b.workout, 2, 250.0));
        b.selected = Some(0);
        b.insert(fresh(&b.workout, 3, 250.0));
        let kinds: Vec<&str> = b.workout.blocks.iter().map(Block::kind_label).collect();
        assert_eq!(kinds.len(), 3);
        assert!(matches!(b.workout.blocks[1], Block::Free { .. }));
        assert_eq!(b.selected, Some(1));
        b.undo();
        assert_eq!(b.workout.blocks.len(), 2);
        b.redo();
        assert_eq!(b.workout.blocks.len(), 3);
        b.undo();
        b.undo();
        b.undo();
        assert!(b.workout.blocks.is_empty());
        assert_eq!(b.selected, None);
    }

    #[test]
    fn durations_parse_the_way_people_type_them() {
        assert_eq!(parse_duration("5"), Some(300.0));
        assert_eq!(parse_duration("45s"), Some(45.0));
        assert_eq!(parse_duration("1:30"), Some(90.0));
        assert_eq!(parse_duration("1:00:00"), Some(3600.0));
        assert_eq!(parse_duration("2,5 min"), Some(150.0));
        assert_eq!(parse_duration("1h"), Some(3600.0));
        assert_eq!(parse_duration("abc"), None);
    }
}
