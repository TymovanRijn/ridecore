//! Ride statistics and a shareable stats card (PNG) for after a ride.

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use image::{ImageBuffer, Rgba, RgbaImage};

use crate::fit::Sample;
use crate::i18n::{english, ordinal, t};
use crate::tf;

/// Summary numbers of one ride, from its 1 Hz samples.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RideStats {
    pub duration_s: u32,
    pub distance_m: f64,
    pub gain_m: f32,
    pub avg_power: f32,
    /// Normalized power: 30 s rolling average, 4th-power mean.
    pub norm_power: f32,
    pub max_power: u16,
    pub avg_speed_kmh: f32,
    pub avg_cadence: f32,
    pub avg_hr: Option<f32>,
    pub max_hr: Option<u8>,
}

impl RideStats {
    pub fn from_samples(s: &[Sample]) -> Self {
        if s.len() < 2 {
            return Self::default();
        }
        let duration_s = (s.len() - 1) as u32;
        let distance_m = s.last().map(|x| x.distance_m).unwrap_or(0.0) - s[0].distance_m;
        let gain_m = s
            .windows(2)
            .map(|w| (w[1].altitude_m - w[0].altitude_m).max(0.0))
            .sum();
        let power: Vec<f32> = s.iter().map(|x| x.power_w as f32).collect();
        let avg_power = power.iter().sum::<f32>() / power.len() as f32;
        let rolling: Vec<f32> = power
            .windows(30.min(power.len()))
            .map(|w| w.iter().sum::<f32>() / w.len() as f32)
            .collect();
        let norm_power =
            (rolling.iter().map(|p| p.powi(4)).sum::<f32>() / rolling.len() as f32).powf(0.25);
        let pedaling: Vec<f32> = s
            .iter()
            .filter(|x| x.cadence_rpm > 0)
            .map(|x| x.cadence_rpm as f32)
            .collect();
        let hrs: Vec<u8> = s.iter().filter_map(|x| x.heart_rate).collect();
        Self {
            duration_s,
            distance_m,
            gain_m,
            avg_power,
            norm_power,
            max_power: s.iter().map(|x| x.power_w).max().unwrap_or(0),
            avg_speed_kmh: (distance_m / duration_s.max(1) as f64 * 3.6) as f32,
            avg_cadence: if pedaling.is_empty() {
                0.0
            } else {
                pedaling.iter().sum::<f32>() / pedaling.len() as f32
            },
            avg_hr: (!hrs.is_empty())
                .then(|| hrs.iter().map(|&h| h as f32).sum::<f32>() / hrs.len() as f32),
            max_hr: hrs.iter().copied().max(),
        }
    }
}

/// A date like "13 oktober 2026" from Unix seconds (UTC).
pub fn date_nl(unix: u64) -> String {
    let (day, month, year) = civil(unix);
    const MONTHS: [&str; 12] = [
        "januari",
        "februari",
        "maart",
        "april",
        "mei",
        "juni",
        "juli",
        "augustus",
        "september",
        "oktober",
        "november",
        "december",
    ];
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// The date in the current language: "13 oktober 2026" or "13 October 2026".
pub fn date(unix: u64) -> String {
    if !english() {
        return date_nl(unix);
    }
    let (day, month, year) = civil(unix);
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

/// Day, month (1-12) and year from Unix seconds (UTC).
fn civil(unix: u64) -> (i64, i64, i64) {
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (day, month, year)
}

pub fn fmt_duration(s: u32) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Everything shown on a stats card.
pub struct CardInput<'a> {
    pub title: String,
    pub subtitle: String,
    /// (place, field size) for races.
    pub place: Option<(usize, usize)>,
    pub stats: RideStats,
    pub rider_kg: f32,
    /// Short highlights such as jerseys and points.
    pub badges: Vec<String>,
    pub samples: &'a [Sample],
    pub accent: [u8; 3],
}

const W: u32 = 1080;
const H: u32 = 1350;

/// Renders the card as PNG bytes.
pub fn render(input: &CardInput) -> Vec<u8> {
    let font = FontArc::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT)
        .expect("embedded font is valid");
    let mut img: RgbaImage = ImageBuffer::from_fn(W, H, |_, y| {
        let t = y as f32 / H as f32;
        Rgba([
            lerp(16.0, 30.0, t) as u8,
            lerp(19.0, 35.0, t) as u8,
            lerp(28.0, 52.0, t) as u8,
            255,
        ])
    });
    let accent = Rgba([input.accent[0], input.accent[1], input.accent[2], 255]);
    let orange = Rgba([255, 140, 0, 255]);
    let white = Rgba([245, 245, 245, 255]);
    let grey = Rgba([150, 158, 175, 255]);

    // Accent band along the top.
    fill(&mut img, 0, 0, W, 14, accent);
    text(
        &mut img,
        &font,
        t("RIDECORE · WEDSTRIJD", "RIDECORE · RACE"),
        64.0,
        70.0,
        30.0,
        orange,
        false,
    );
    text(
        &mut img,
        &font,
        &input.title,
        64.0,
        150.0,
        66.0,
        white,
        true,
    );
    text(
        &mut img,
        &font,
        &input.subtitle,
        64.0,
        200.0,
        30.0,
        grey,
        false,
    );

    // The result.
    let mut y = 360.0;
    match input.place {
        Some((1, _)) => {
            text(
                &mut img,
                &font,
                t("OVERWINNING!", "VICTORY!"),
                64.0,
                y,
                120.0,
                Rgba([255, 205, 40, 255]),
                true,
            );
        }
        Some((p, n)) => {
            let big = ordinal(p);
            text(&mut img, &font, &big, 64.0, y, 170.0, orange, true);
            let w = width(&font, &big, 170.0);
            text(
                &mut img,
                &font,
                &tf!("van {n}", "of {n}"),
                64.0 + w + 24.0,
                y,
                48.0,
                grey,
                false,
            );
        }
        None => {
            text(
                &mut img,
                &font,
                &format!("{:.1} km", input.stats.distance_m / 1000.0),
                64.0,
                y,
                140.0,
                orange,
                true,
            );
        }
    }

    // Stats grid: three columns.
    let s = &input.stats;
    let mut cells = vec![
        (t("AFSTAND", "DISTANCE"), format!("{:.1} km", s.distance_m / 1000.0)),
        (t("TIJD", "TIME"), fmt_duration(s.duration_s)),
        (t("HOOGTEMETERS", "ELEVATION"), format!("{:.0} m", s.gain_m)),
        (t("GEM. VERMOGEN", "AVG POWER"), format!("{:.0} W", s.avg_power)),
        ("NP", format!("{:.0} W", s.norm_power)),
        (
            "W/KG",
            format!("{:.2}", s.avg_power / input.rider_kg.max(1.0)),
        ),
        (t("MAX VERMOGEN", "MAX POWER"), format!("{} W", s.max_power)),
        (
            t("GEM. SNELHEID", "AVG SPEED"),
            tf!("{:.1} km/u", "{:.1} km/h", s.avg_speed_kmh),
        ),
    ];
    cells.push(match s.avg_hr {
        Some(hr) => (t("GEM. HARTSLAG", "AVG HEART RATE"), format!("{hr:.0} bpm")),
        None => (t("GEM. CADANS", "AVG CADENCE"), format!("{:.0} rpm", s.avg_cadence)),
    });
    y += 90.0;
    for (k, (label, value)) in cells.iter().enumerate() {
        let (col, row) = (k % 3, k / 3);
        let x = 64.0 + col as f32 * 330.0;
        let cy = y + row as f32 * 150.0;
        fill(&mut img, x as u32 - 4, cy as u32 + 8, 6, 100, accent);
        text(
            &mut img,
            &font,
            label,
            x + 18.0,
            cy + 40.0,
            24.0,
            grey,
            false,
        );
        text(
            &mut img,
            &font,
            value,
            x + 18.0,
            cy + 98.0,
            50.0,
            white,
            true,
        );
    }

    // Highlights.
    let mut bx = 64.0;
    let by = y + 3.0 * 150.0 + 40.0;
    for badge in &input.badges {
        let w = width(&font, badge, 28.0) + 36.0;
        if bx + w > (W - 64) as f32 {
            break;
        }
        rounded(
            &mut img,
            bx as u32,
            by as u32,
            w as u32,
            52,
            Rgba([255, 255, 255, 28]),
        );
        text(
            &mut img,
            &font,
            badge,
            bx + 18.0,
            by + 36.0,
            28.0,
            white,
            false,
        );
        bx += w + 14.0;
    }

    // Elevation profile with the power line over it.
    // Power in white over the elevation profile in the accent color.
    profile(
        &mut img,
        input.samples,
        64,
        1010,
        W - 128,
        230,
        accent,
        Rgba([255, 255, 255, 190]),
    );
    text(
        &mut img,
        &font,
        t(
            "Gereden op RideCore · echte route, echte hoogtemeters",
            "Ridden on RideCore · real route, real climbing",
        ),
        64.0,
        (H - 40) as f32,
        24.0,
        grey,
        false,
    );

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("PNG encoding to memory cannot fail");
    png
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn blend(img: &mut RgbaImage, x: u32, y: u32, c: Rgba<u8>, coverage: f32) {
    if x >= img.width() || y >= img.height() {
        return;
    }
    let a = coverage * c[3] as f32 / 255.0;
    let p = img.get_pixel_mut(x, y);
    for k in 0..3 {
        p[k] = (p[k] as f32 * (1.0 - a) + c[k] as f32 * a) as u8;
    }
}

fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            blend(img, xx, yy, c, 1.0);
        }
    }
}

fn rounded(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    let r = h as f32 / 2.0;
    for yy in 0..h {
        for xx in 0..w {
            let dx = if (xx as f32) < r {
                r - xx as f32
            } else if xx as f32 > w as f32 - r {
                xx as f32 - (w as f32 - r)
            } else {
                0.0
            };
            let dy = yy as f32 - r + 0.5;
            if dx * dx + dy * dy <= r * r || dx == 0.0 {
                blend(img, x + xx, y + yy, c, 1.0);
            }
        }
    }
}

fn width(font: &FontArc, s: &str, size: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    s.chars().map(|c| scaled.h_advance(font.glyph_id(c))).sum()
}

/// Draws text with its baseline at `y`; `bold` thickens the light font.
#[allow(clippy::too_many_arguments)]
fn text(
    img: &mut RgbaImage,
    font: &FontArc,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    c: Rgba<u8>,
    bold: bool,
) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let offsets: &[f32] = if bold {
        &[0.0, size * 0.025, size * 0.05]
    } else {
        &[0.0]
    };
    for &dx in offsets {
        let mut pen = x + dx;
        for ch in s.chars() {
            let id = font.glyph_id(ch);
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen, y));
            if let Some(outline) = font.outline_glyph(glyph) {
                let b = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px >= 0 && py >= 0 {
                        blend(img, px as u32, py as u32, c, cov);
                    }
                });
            }
            pen += scaled.h_advance(id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn profile(
    img: &mut RgbaImage,
    s: &[Sample],
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    fill_c: Rgba<u8>,
    line: Rgba<u8>,
) {
    if s.len() < 2 {
        return;
    }
    let (lo, hi) = s.iter().fold((f32::MAX, f32::MIN), |(a, b), p| {
        (a.min(p.altitude_m), b.max(p.altitude_m))
    });
    let span = (hi - lo).max(20.0);
    let max_p = s.iter().map(|p| p.power_w).max().unwrap_or(1).max(1) as f32;
    let col = |k: u32| &s[(k as usize * (s.len() - 1)) / (w as usize - 1)];
    let mut last_py = None;
    for k in 0..w {
        let p = col(k);
        let top = y + h - ((p.altitude_m - lo) / span * (h as f32 * 0.85)) as u32;
        for yy in top..y + h {
            let fade = 0.25 + 0.55 * (1.0 - (yy - top) as f32 / h as f32);
            blend(img, x + k, yy, fill_c, fade);
        }
        // Power, smoothed over a few neighbors, as a line on top.
        let n = 6usize;
        let i = (k as usize * (s.len() - 1)) / (w as usize - 1);
        let window = &s[i.saturating_sub(n)..(i + n).min(s.len())];
        let pw = window.iter().map(|q| q.power_w as f32).sum::<f32>() / window.len() as f32;
        let py = y + h - (pw / max_p * h as f32 * 0.95) as u32;
        let (a, b) = match last_py {
            Some(l) => (py.min(l), py.max(l)),
            None => (py, py),
        };
        for yy in a..=b {
            blend(img, x + k, yy, line, 0.9);
            blend(img, x + k, yy + 1, line, 0.6);
        }
        last_py = Some(py);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Sample> {
        (0..600)
            .map(|i| Sample {
                unix_time: 1_760_000_000 + i,
                distance_m: i as f64 * 9.0,
                speed_mps: 9.0,
                power_w: if i % 60 < 30 { 300 } else { 100 },
                cadence_rpm: 90,
                heart_rate: Some(150),
                altitude_m: 100.0 + i as f32 * 0.1,
                position: None,
            })
            .collect()
    }

    #[test]
    fn dates_in_dutch() {
        assert_eq!(date_nl(0), "1 januari 1970");
        assert_eq!(date_nl(1_791_878_400), "13 oktober 2026");
        assert_eq!(date_nl(951_782_400), "29 februari 2000");
    }

    #[test]
    fn stats_from_samples() {
        let s = RideStats::from_samples(&samples());
        assert_eq!(s.duration_s, 599);
        assert!((s.avg_power - 200.0).abs() < 1.0);
        // Hard/easy blocks: normalized power is above the average.
        assert!(
            s.norm_power > s.avg_power,
            "{} vs {}",
            s.norm_power,
            s.avg_power
        );
        assert_eq!(s.max_power, 300);
        assert!((s.gain_m - 59.9).abs() < 0.2);
        assert_eq!(s.avg_hr, Some(150.0));
    }

    #[test]
    fn card_renders_a_png() {
        let samples = samples();
        let png = render(&CardInput {
            title: "Alpe d'Huez".into(),
            subtitle: "Isère, Frankrijk · 13 oktober 2026".into(),
            place: Some((3, 48)),
            stats: RideStats::from_samples(&samples),
            rider_kg: 75.0,
            badges: vec!["Bergpunten 10".into(), "In het wiel 58%".into()],
            samples: &samples,
            accent: [255, 120, 0],
        });
        assert_eq!(&png[1..4], b"PNG");
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (W, H));
    }
}
