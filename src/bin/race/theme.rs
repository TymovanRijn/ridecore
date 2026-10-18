//! The look of every screen, cartoony like the 3D world: chunky Lilita One
//! for titles and numbers and rounded Varela Round for text (both under
//! the SIL Open Font License, in assets/fonts), indigo panels with a thick
//! edge and a hard drop shadow like a sticker, yellow labels and an orange
//! accent, and helpers for cards and big numbers.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 122, 26);
pub const ACCENT_SOFT: egui::Color32 = egui::Color32::from_rgb(255, 210, 63);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(255, 252, 245);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(178, 184, 232);
pub const GOOD: egui::Color32 = egui::Color32::from_rgb(61, 220, 132);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(255, 210, 63);
pub const BAD: egui::Color32 = egui::Color32::from_rgb(255, 77, 94);
/// Labels.
pub const LABEL: egui::Color32 = egui::Color32::from_rgb(255, 210, 63);
pub const CARD: egui::Color32 = egui::Color32::from_rgb(38, 40, 98);
pub const CARD_EDGE: egui::Color32 = egui::Color32::from_rgb(86, 92, 186);
/// Outlines and drop shadows.
pub const INK: egui::Color32 = egui::Color32::from_rgb(18, 16, 46);

/// The sticker shadow under panels.
pub fn drop_shadow() -> egui::Shadow {
    egui::Shadow {
        offset: [0, 5],
        blur: 0,
        spread: 0,
        color: INK.gamma_multiply(0.75),
    }
}

/// Text with a hard shadow under it, for big numbers and titles.
pub fn bold(
    ui: &mut egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let drop = (galley.size().y * 0.06).clamp(1.5, 3.5);
    let p = ui.painter();
    p.galley_with_override_text_color(rect.min + egui::vec2(0.0, drop), galley.clone(), INK);
    p.galley(rect.min, galley, color);
    resp
}

/// The chunky face for titles and big numbers.
pub fn display(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("display".into()))
}

pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Installed>()
            .add_systems(EguiPrimaryContextPass, install);
    }
}

/// Frames since the fonts were handed to egui; they apply from the next one.
#[derive(Resource, Default)]
pub struct Installed(u8);

/// Run condition for every UI system: egui panics on a font family it
/// doesn't know yet, so screens wait until the fonts are live.
pub fn ready(installed: Res<Installed>) -> bool {
    installed.0 >= 2
}

fn install(mut contexts: EguiContexts, mut installed: ResMut<Installed>) -> Result {
    if installed.0 > 0 {
        installed.0 = installed.0.saturating_add(1).min(2);
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(
            name.into(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    };
    add(
        "varela",
        include_bytes!("../../../assets/fonts/VarelaRound-Regular.ttf"),
    );
    add(
        "lilita",
        include_bytes!("../../../assets/fonts/LilitaOne-Regular.ttf"),
    );
    // Ours first; egui's own fonts stay behind for symbols they lack.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "varela".into());
    let mut condensed = vec!["lilita".to_string()];
    condensed.extend(
        fonts.families[&egui::FontFamily::Proportional]
            .iter()
            .cloned(),
    );
    fonts
        .families
        .insert(egui::FontFamily::Name("display".into()), condensed);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);

    ctx.all_styles_mut(|s| {
        use egui::{FontFamily::Proportional, TextStyle};
        s.text_styles = [
            (TextStyle::Small, egui::FontId::new(12.0, Proportional)),
            (TextStyle::Body, egui::FontId::new(16.0, Proportional)),
            (TextStyle::Button, egui::FontId::new(16.0, Proportional)),
            (TextStyle::Monospace, egui::FontId::monospace(14.0)),
            (TextStyle::Heading, display(30.0)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.slider_width = 200.0;
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = egui::Color32::TRANSPARENT;
        v.window_fill = CARD;
        v.window_stroke = egui::Stroke::new(3.0, CARD_EDGE);
        v.window_corner_radius = 18.0.into();
        v.window_shadow = drop_shadow();
        v.extreme_bg_color = egui::Color32::from_rgb(24, 24, 66);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.85);
        v.selection.stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
        v.hyperlink_color = ACCENT_SOFT;
        v.slider_trailing_fill = true;
        for (w, fill) in [
            (
                &mut v.widgets.inactive,
                egui::Color32::from_rgb(58, 62, 140),
            ),
            (&mut v.widgets.hovered, egui::Color32::from_rgb(76, 82, 170)),
            (&mut v.widgets.active, ACCENT),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = 12.0.into();
            w.bg_stroke = egui::Stroke::new(2.0, INK.gamma_multiply(0.6));
        }
        v.widgets.hovered.bg_stroke = egui::Stroke::new(2.0, ACCENT_SOFT);
        v.widgets.noninteractive.corner_radius = 12.0.into();
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(2.0, CARD_EDGE);
    });
    installed.0 = 1;
    Ok(())
}

/// A glassy card with a small caps title.
pub fn card<R>(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let r = egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(3.0, CARD_EDGE))
        .corner_radius(18.0)
        .shadow(drop_shadow())
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    egui::RichText::new(title.to_uppercase())
                        .font(display(17.0))
                        .color(LABEL)
                        .extra_letter_spacing(0.6),
                );
                ui.add_space(4.0);
            }
            body(ui)
        })
        .inner;
    ui.add_space(12.0);
    r
}

/// A big number with a small label under it.
pub fn stat(ui: &mut egui::Ui, value: impl Into<String>, label: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        bold(ui, &value.into(), display(34.0), color);
        ui.label(
            egui::RichText::new(label.to_uppercase())
                .small()
                .color(MUTED),
        );
    });
}

/// A rounded status pill with a colored dot.
pub fn chip(ui: &mut egui::Ui, text: &str, ok: bool) {
    let dot = if ok { GOOD } else { WARN };
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_premultiplied(20, 24, 36, 200))
        .stroke(egui::Stroke::new(1.0, dot.gamma_multiply(0.5)))
        .corner_radius(20.0)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 5.0, dot);
                ui.label(egui::RichText::new(text).size(14.0));
            });
        });
}

/// Gradient colors for climbs, as bike computers use them.
pub fn grade_color(pct: f32) -> egui::Color32 {
    match pct {
        p if p < 3.0 => egui::Color32::from_rgb(90, 190, 90),
        p if p < 6.0 => egui::Color32::from_rgb(235, 210, 60),
        p if p < 9.0 => egui::Color32::from_rgb(245, 140, 40),
        p if p < 12.0 => egui::Color32::from_rgb(220, 50, 40),
        _ => egui::Color32::from_rgb(200, 50, 140),
    }
}

/// A filled elevation profile, one colored slice per segment. Drawn as a
/// plain mesh: egui's feathered polygons smear long streaks across the
/// screen when a flat slice is only a pixel high.
pub fn profile_fill(
    p: &egui::Painter,
    pts: &[egui::Pos2],
    bottom: f32,
    color_of: impl Fn(usize) -> egui::Color32,
) {
    let mut m = egui::Mesh::default();
    for (k, w) in pts.windows(2).enumerate() {
        let c = color_of(k);
        let i = m.vertices.len() as u32;
        m.colored_vertex(w[0], c);
        m.colored_vertex(w[1], c);
        m.colored_vertex(egui::pos2(w[1].x, bottom), c);
        m.colored_vertex(egui::pos2(w[0].x, bottom), c);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    p.add(m);
}
