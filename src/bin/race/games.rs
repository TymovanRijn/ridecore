//! Workout games: the same workout, but instead of chasing a watt number you
//! steer a hot-air balloon, bake pizzas or reel in fish. Your power against
//! the target is the controller. The trainer rides a flat road and you hit
//! the watts with the gears and your legs, so it is you, not ERG, that holds
//! them.
//!
//! Every game splits the workout into rounds (one per block, long blocks cut
//! into pieces of at most 2½ minutes) and judges each round by how long you
//! stayed within 5% of the target.

use bevy_egui::egui::{self, Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::theme::{ACCENT, GOOD, MUTED, TEXT, WARN, display};
use ridecore::i18n::t;
use ridecore::tf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameKind {
    Balloon,
    Pizza,
    Fishing,
}

impl GameKind {
    pub const ALL: [GameKind; 3] = [GameKind::Balloon, GameKind::Pizza, GameKind::Fishing];

    pub fn name(self) -> &'static str {
        match self {
            GameKind::Balloon => t("Ballonvaart", "Balloon ride"),
            GameKind::Pizza => t("Pizzabakker", "Pizza baker"),
            GameKind::Fishing => t("Vissen", "Fishing"),
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            GameKind::Balloon => t(
                "Je vermogen is je hoogte. Blijf in de luchtgang en pak de munten.",
                "Your power is your altitude. Stay in the corridor and grab the coins.",
            ),
            GameKind::Pizza => t(
                "Je vermogen is het vuur. Elke ronde een pizza: niet rauw, niet aangebrand.",
                "Your power is the fire. Every round is a pizza: not raw, not burnt.",
            ),
            GameKind::Fishing => t(
                "Je vermogen is de spanning op de lijn. Te slap en de vis zwemt weg.",
                "Your power is the tension on the line. Too slack and the fish swims off.",
            ),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "balloon" | "ballon" => Some(GameKind::Balloon),
            "pizza" => Some(GameKind::Pizza),
            "fishing" | "vissen" => Some(GameKind::Fishing),
            _ => None,
        }
    }

    fn score_label(self) -> &'static str {
        match self {
            GameKind::Balloon => t("munten", "coins"),
            GameKind::Pizza => t("pizza's", "pizzas"),
            GameKind::Fishing => t("vissen", "fish"),
        }
    }
}

/// Within this fraction of the target counts as "on target".
const TOL: f32 = 0.05;
/// …but never tighter than this, for easy blocks at low watts.
const TOL_MIN_W: f32 = 8.0;
const ROUND_MAX_S: f32 = 150.0;
const COIN_EVERY_S: f32 = 2.0;
const BANNER_S: f32 = 4.0;

pub fn band(target: f32) -> f32 {
    (target * TOL).max(TOL_MIN_W)
}

/// The share of a round you must ride on target to win it.
fn needed(len: f32) -> f32 {
    if len < 30.0 { 0.55 } else { 0.7 }
}

const FISH: [(&str, &str); 6] = [
    ("Voorn", "Roach"),
    ("Baars", "Perch"),
    ("Brasem", "Bream"),
    ("Karper", "Carp"),
    ("Snoek", "Pike"),
    ("Meerval", "Catfish"),
];

fn fish_name(i: usize) -> &'static str {
    let (nl, en) = FISH[i];
    t(nl, en)
}
const PIZZAS: [&str; 6] = [
    "Margherita",
    "Pepperoni",
    "Funghi",
    "Quattro Formaggi",
    "Hawaii",
    "Diavola",
];

/// Something to play a sound for.
pub enum Sound {
    Coin,
    Won,
    Lost,
}

#[derive(Clone, Copy, PartialEq)]
enum Coin {
    Open,
    Got(f32),
    Missed,
}

#[derive(Clone, Copy)]
struct Round {
    start: f32,
    end: f32,
}

#[derive(Clone, Copy)]
struct Outcome {
    good: bool,
    burnt: bool,
    variety: usize,
    size: f32,
}

struct Banner {
    title: String,
    sub: String,
    color: Color32,
    at: f32,
}

pub struct Game {
    pub kind: GameKind,
    rounds: Vec<Round>,
    round: usize,
    /// The scale for the balloon's height.
    max_w: f32,
    /// Your power, smoothed over about a second.
    power: f32,
    /// Your power against the target, smoothed slower: the oven's heat and
    /// the tension on the line.
    heat: f32,
    clock: f32,
    on: f32,
    over: f32,
    under: f32,
    /// 1 = the fish is far out, 0 = landed.
    reel: f32,
    coins: Vec<Coin>,
    next_coin: usize,
    streak: u32,
    pub best_streak: u32,
    pub score: u32,
    pub rounds_won: u32,
    shelf: Vec<Outcome>,
    banner: Option<Banner>,
}

impl Game {
    /// `segments` are the workout's block lengths, `max_w` its hardest target.
    pub fn new(kind: GameKind, segments: &[u32], max_w: f32) -> Self {
        let mut rounds = Vec::new();
        let mut start = 0.0;
        for &secs in segments {
            let len = secs as f32;
            let n = (len / ROUND_MAX_S).ceil().max(1.0);
            for k in 0..n as usize {
                rounds.push(Round {
                    start: start + len * k as f32 / n,
                    end: start + len * (k + 1) as f32 / n,
                });
            }
            start += len;
        }
        let coins = vec![Coin::Open; (start / COIN_EVERY_S) as usize];
        Self {
            kind,
            rounds,
            round: 0,
            max_w: max_w.max(100.0) * 1.3,
            power: 0.0,
            heat: 0.0,
            clock: 0.0,
            on: 0.0,
            over: 0.0,
            under: 0.0,
            reel: 1.0,
            coins,
            next_coin: 1,
            streak: 0,
            best_streak: 0,
            score: 0,
            rounds_won: 0,
            shelf: Vec::new(),
            banner: None,
        }
    }

    pub fn summary(&self) -> String {
        match self.kind {
            GameKind::Balloon => tf!(
                "{} munten · langste reeks {}",
                "{} coins · longest streak {}",
                self.score,
                self.best_streak
            ),
            GameKind::Pizza => tf!(
                "{} van {} pizza's geserveerd",
                "{} of {} pizzas served",
                self.rounds_won,
                self.shelf.len()
            ),
            GameKind::Fishing => tf!(
                "{} van {} vissen gevangen",
                "{} of {} fish caught",
                self.rounds_won,
                self.shelf.len()
            ),
        }
    }

    /// Jumps ahead (for testing) without judging the rounds skipped.
    pub fn skip_to(&mut self, elapsed: f32) {
        while self
            .rounds
            .get(self.round)
            .is_some_and(|r| r.end <= elapsed)
        {
            self.round += 1;
        }
        self.next_coin = self.next_coin.max((elapsed / COIN_EVERY_S).ceil() as usize);
    }

    /// Lets the animations run while paused or between rounds.
    pub fn animate(&mut self, dt: f32) {
        self.clock += dt;
    }

    /// One frame of riding: `elapsed` is the workout clock after this frame.
    pub fn update(&mut self, dt: f32, elapsed: f32, power: f32, target: Option<f32>) -> Vec<Sound> {
        let mut sounds = Vec::new();
        self.power += (power - self.power) * (1.0 - (-dt / 1.0).exp());
        if let Some(target) = target.filter(|t| *t > 1.0) {
            self.ride(dt, elapsed, target, &mut sounds);
        }
        while let Some(&r) = self.rounds.get(self.round)
            && elapsed >= r.end
        {
            sounds.push(self.end_round(r));
            self.round += 1;
            self.on = 0.0;
            self.over = 0.0;
            self.under = 0.0;
            self.reel = 1.0;
        }
        sounds
    }

    fn ride(&mut self, dt: f32, elapsed: f32, target: f32, sounds: &mut Vec<Sound>) {
        let ratio = self.power / target;
        self.heat += (ratio - self.heat) * (1.0 - (-dt / 1.5).exp());
        let tol = band(target) / target;
        let level = match self.kind {
            GameKind::Balloon => ratio,
            _ => self.heat,
        };
        let on = (level - 1.0).abs() <= tol;
        if on {
            self.on += dt;
        } else if level > 1.0 {
            self.over += dt;
        } else {
            self.under += dt;
        }
        if let Some(r) = self.rounds.get(self.round) {
            let len = (r.end - r.start).max(1.0);
            // On target the fish comes in exactly by the end of the round;
            // a slack line lets it swim off again.
            if on {
                self.reel -= dt / len;
            } else if level < 1.0 {
                self.reel += 0.5 * dt / len;
            }
            self.reel = self.reel.clamp(0.0, 1.0);
        }

        while self.next_coin < self.coins.len() && self.next_coin as f32 * COIN_EVERY_S <= elapsed {
            let got = (self.power - target).abs() <= band(target);
            self.coins[self.next_coin] = if got {
                Coin::Got(self.clock)
            } else {
                Coin::Missed
            };
            if self.kind == GameKind::Balloon {
                if got {
                    self.score += 1;
                    self.streak += 1;
                    self.best_streak = self.best_streak.max(self.streak);
                    sounds.push(Sound::Coin);
                } else {
                    self.streak = 0;
                }
            }
            self.next_coin += 1;
        }
    }

    fn end_round(&mut self, r: Round) -> Sound {
        let len = (r.end - r.start).max(1.0);
        let share = self.on / len;
        let won = match self.kind {
            GameKind::Fishing => 1.0 - self.reel >= needed(len),
            _ => share >= needed(len),
        };
        let burnt = !won && self.over > self.under;
        let variety = self.shelf.len() * 7 % 6;
        let pct = (share * 100.0).round();
        let hint = if self.over > self.under {
            t(
                "Te veel vermogen: schakel lichter",
                "Too much power: shift easier",
            )
        } else {
            t(
                "Te weinig vermogen: schakel zwaarder",
                "Too little power: shift harder",
            )
        };
        let (title, sub) = match (self.kind, won) {
            (GameKind::Balloon, true) => {
                let first = (r.start / COIN_EVERY_S).ceil() as usize;
                let last = ((r.end / COIN_EVERY_S).ceil() as usize).min(self.coins.len());
                let got = self.coins[first.min(last)..last]
                    .iter()
                    .filter(|c| matches!(c, Coin::Got(_)))
                    .count();
                (
                    t("Mooi gevlogen!", "Nice flying!").to_string(),
                    tf!(
                        "{got} munten in deze ronde · {pct:.0}% in de luchtgang",
                        "{got} coins this round · {pct:.0}% in the corridor"
                    ),
                )
            }
            (GameKind::Balloon, false) => (
                t("Uit koers geraakt", "Blown off course").into(),
                hint.to_string(),
            ),
            (GameKind::Pizza, true) => (
                if share > 0.9 {
                    tf!("{} perfect!", "{} perfect!", PIZZAS[variety])
                } else {
                    tf!("{} is klaar!", "{} is ready!", PIZZAS[variety])
                },
                tf!(
                    "{pct:.0}% op de juiste temperatuur",
                    "{pct:.0}% at the right temperature"
                ),
            ),
            (GameKind::Pizza, false) if burnt => (
                t("Aangebrand!", "Burnt!").into(),
                t(
                    "Te heet: schakel lichter of trap rustiger",
                    "Too hot: shift easier or pedal softer",
                )
                .into(),
            ),
            (GameKind::Pizza, false) => (
                t("Half rauw…", "Half raw…").into(),
                t(
                    "Te koud: schakel zwaarder of trap sneller",
                    "Too cold: shift harder or pedal faster",
                )
                .into(),
            ),
            (GameKind::Fishing, true) => {
                let fish = fish_of(len, variety);
                (
                    tf!("{} gevangen!", "{} caught!", fish_name(fish)),
                    tf!(
                        "{} cm · {pct:.0}% strak aan de lijn",
                        "{} cm · {pct:.0}% tight on the line",
                        fish_cm(len, variety)
                    ),
                )
            }
            (GameKind::Fishing, false) => (t("Ontsnapt!", "Got away!").into(), hint.to_string()),
        };
        if won {
            self.rounds_won += 1;
            if self.kind != GameKind::Balloon {
                self.score += 1;
            }
        }
        self.shelf.push(Outcome {
            good: won,
            burnt,
            variety,
            size: len,
        });
        self.banner = Some(Banner {
            title,
            sub,
            color: if won { GOOD } else { WARN },
            at: self.clock,
        });
        if won { Sound::Won } else { Sound::Lost }
    }

    fn round_progress(&self, elapsed: f32) -> (usize, f32, f32) {
        match self.rounds.get(self.round) {
            Some(r) => (
                self.round,
                ((elapsed - r.start) / (r.end - r.start).max(1.0)).clamp(0.0, 1.0),
                r.end - r.start,
            ),
            None => (self.round, 1.0, 1.0),
        }
    }

    /// The score, top right.
    pub fn score_card(&self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(self.kind.name().to_uppercase())
                    .font(display(18.0))
                    .color(ACCENT),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(self.score.to_string())
                        .font(display(52.0))
                        .color(TEXT),
                );
                ui.label(
                    egui::RichText::new(self.kind.score_label())
                        .font(display(20.0))
                        .color(MUTED),
                );
            });
            let line = match self.kind {
                GameKind::Balloon => tf!(
                    "reeks {} · beste {}",
                    "streak {} · best {}",
                    self.streak,
                    self.best_streak
                ),
                _ => {
                    let lost = self.shelf.len() as u32 - self.rounds_won;
                    let word = if self.kind == GameKind::Pizza {
                        t("mislukt", "failed")
                    } else {
                        t("ontsnapt", "got away")
                    };
                    tf!(
                        "{lost} {word} · ronde {}",
                        "{lost} {word} · round {}",
                        (self.round + 1).min(self.rounds.len())
                    )
                }
            };
            ui.label(egui::RichText::new(line).color(MUTED));
        });
    }

    /// Paints the game over the whole screen; the workout HUD sits on top.
    pub fn draw(
        &self,
        p: &Painter,
        screen: Rect,
        elapsed: f32,
        target: Option<f32>,
        target_at: &dyn Fn(f32) -> Option<f32>,
    ) {
        let top = screen.top() + 290.0;
        let bottom = (screen.bottom() - 190.0).max(top + 220.0);
        let area = Rect::from_min_max(
            pos2(screen.left() + 40.0, top),
            pos2(screen.right() - 40.0, bottom),
        );
        match self.kind {
            GameKind::Balloon => self.draw_balloon(p, screen, area, elapsed, target, target_at),
            GameKind::Pizza => self.draw_pizza(p, screen, area, elapsed, target),
            GameKind::Fishing => self.draw_fishing(p, screen, area, elapsed, target),
        }
        if let Some(b) = &self.banner {
            let age = self.clock - b.at;
            if age < BANNER_S {
                let fade = (age / 0.2).min(1.0) * ((BANNER_S - age) / 0.6).clamp(0.0, 1.0);
                let pop = 1.0 + 0.25 * (1.0 - (age / 0.25).min(1.0)).powi(2);
                // Low in the play area: the coach's messages use the top.
                let at = pos2(area.center().x, area.bottom() - 70.0);
                let bg = Rect::from_center_size(at + vec2(0.0, 18.0), vec2(560.0, 96.0));
                p.rect_filled(bg, 16.0, Color32::from_black_alpha((170.0 * fade) as u8));
                p.text(
                    at,
                    egui::Align2::CENTER_CENTER,
                    &b.title,
                    display(44.0 * pop),
                    b.color.gamma_multiply(fade),
                );
                p.text(
                    at + vec2(0.0, 42.0),
                    egui::Align2::CENTER_CENTER,
                    &b.sub,
                    egui::FontId::proportional(18.0),
                    TEXT.gamma_multiply(fade),
                );
            }
        }
    }

    fn draw_balloon(
        &self,
        p: &Painter,
        screen: Rect,
        area: Rect,
        elapsed: f32,
        target: Option<f32>,
        target_at: &dyn Fn(f32) -> Option<f32>,
    ) {
        vgradient(
            p,
            screen,
            Color32::from_rgb(52, 110, 196),
            Color32::from_rgb(255, 196, 150),
        );
        p.circle_filled(
            pos2(screen.right() - 220.0, screen.top() + 330.0),
            70.0,
            Color32::from_rgba_unmultiplied(255, 236, 170, 200),
        );
        // Clouds drift past at three depths.
        for i in 0..9 {
            let depth = 0.4 + (i % 3) as f32 * 0.3;
            let span = screen.width() + 400.0;
            let x = screen.right() + 200.0 - ((i as f32 * 523.0 + elapsed * 18.0 * depth) % span);
            let y = screen.top() + 120.0 + (i as f32 * 211.0 % (screen.height() * 0.6));
            cloud(p, pos2(x, y), 26.0 * depth + 14.0, (150.0 * depth) as u8);
        }
        // Rolling hills.
        for (layer, color, speed, h) in [
            (0, Color32::from_rgb(96, 150, 92), 6.0, 0.16),
            (1, Color32::from_rgb(64, 120, 70), 14.0, 0.09),
        ] {
            let r = screen.width() * 0.35;
            for k in -1..5 {
                let x =
                    screen.left() + (k as f32 * r * 1.3 - elapsed * speed).rem_euclid(r * 6.5) - r;
                let y = screen.bottom() + r * (1.0 - h * 3.0) + layer as f32 * 30.0;
                p.circle_filled(pos2(x, y), r, color);
            }
        }

        let px_s = area.width() / 42.0;
        let bx = area.left() + area.width() * 0.28;
        let x_of = |t: f32| bx + (t - elapsed) * px_s;
        let y_of = |w: f32| area.bottom() - (w / self.max_w).clamp(0.0, 1.08) * area.height();

        // The corridor: the workout, ahead of you.
        let step = 0.5;
        let mut upper = Vec::new();
        let mut lower = Vec::new();
        let flush = |upper: &mut Vec<Pos2>, lower: &mut Vec<Pos2>| {
            for line in [&mut *upper, &mut *lower] {
                if line.len() > 1 {
                    p.add(Shape::line(
                        std::mem::take(line),
                        Stroke::new(2.5, Color32::from_white_alpha(170)),
                    ));
                }
                line.clear();
            }
        };
        let mut t = (elapsed - 12.0).max(0.0);
        while t < elapsed + 30.0 {
            match target_at(t) {
                Some(w) => {
                    let b = band(w);
                    let (x0, x1) = (x_of(t), x_of(t + step));
                    let r = Rect::from_min_max(pos2(x0, y_of(w + b)), pos2(x1, y_of(w - b)));
                    p.rect_filled(r, 0.0, Color32::from_white_alpha(55));
                    upper.push(pos2(x0, y_of(w + b)));
                    lower.push(pos2(x0, y_of(w - b)));
                }
                None => flush(&mut upper, &mut lower),
            }
            t += step;
        }
        flush(&mut upper, &mut lower);

        // Coins along the middle of the corridor.
        let first = ((elapsed - 12.0).max(0.0) / COIN_EVERY_S) as usize;
        let last = (((elapsed + 30.0) / COIN_EVERY_S) as usize).min(self.coins.len());
        for k in first.max(1)..last {
            let t = k as f32 * COIN_EVERY_S;
            let Some(w) = target_at(t) else { continue };
            let at = pos2(x_of(t), y_of(w));
            match self.coins[k] {
                Coin::Open => {
                    let spin = ((self.clock * 3.0 + k as f32).sin()).abs().max(0.25);
                    p.add(Shape::ellipse_filled(
                        at,
                        vec2(10.0 * spin, 10.0),
                        Color32::from_rgb(214, 160, 30),
                    ));
                    p.add(Shape::ellipse_filled(
                        at,
                        vec2(7.0 * spin, 7.0),
                        Color32::from_rgb(255, 214, 80),
                    ));
                }
                Coin::Got(when) => {
                    let age = self.clock - when;
                    if age < 0.6 {
                        let a = 1.0 - age / 0.6;
                        p.circle_stroke(
                            at,
                            10.0 + 30.0 * age,
                            Stroke::new(3.0, Color32::from_rgb(255, 214, 80).gamma_multiply(a)),
                        );
                        p.text(
                            at - vec2(0.0, 20.0 + 40.0 * age),
                            egui::Align2::CENTER_CENTER,
                            "+1",
                            display(24.0),
                            Color32::WHITE.gamma_multiply(a),
                        );
                    }
                }
                Coin::Missed => {
                    p.circle_filled(at, 7.0, Color32::from_black_alpha(50));
                }
            }
        }

        // The balloon.
        let bob = (self.clock * 1.3).sin() * 4.0;
        let by = y_of(self.power).clamp(area.top() - 40.0, area.bottom()) + bob;
        let env = pos2(bx, by - 46.0);
        let rope = Color32::from_rgb(90, 60, 40);
        for dx in [-12.0, 12.0] {
            p.line_segment(
                [env + vec2(dx * 1.6, 22.0), pos2(bx + dx * 0.7, by + 4.0)],
                Stroke::new(1.5, rope),
            );
        }
        p.add(Shape::ellipse_filled(env, vec2(34.0, 40.0), ACCENT));
        p.add(Shape::ellipse_filled(
            env,
            vec2(19.0, 40.0),
            Color32::from_rgb(255, 214, 120),
        ));
        p.add(Shape::ellipse_filled(env, vec2(6.0, 40.0), ACCENT));
        p.add(Shape::ellipse_filled(
            env + vec2(-14.0, -16.0),
            vec2(6.0, 10.0),
            Color32::from_white_alpha(90),
        ));
        p.rect_filled(
            Rect::from_center_size(pos2(bx, by + 10.0), vec2(22.0, 16.0)),
            3.0,
            Color32::from_rgb(140, 92, 52),
        );
        if let Some(t) = target {
            let b = band(t);
            if self.power > t + b {
                let f = (self.clock * 20.0).sin() * 3.0;
                p.add(Shape::convex_polygon(
                    vec![
                        pos2(bx - 6.0, by - 2.0),
                        pos2(bx, by - 22.0 - f),
                        pos2(bx + 6.0, by - 2.0),
                    ],
                    Color32::from_rgb(255, 170, 40),
                    Stroke::NONE,
                ));
            }
            let hint = if self.power > t + b {
                Some((
                    ridecore::i18n::t("Lager! Minder vermogen", "Lower! Less power"),
                    WARN,
                ))
            } else if self.power < t - b {
                Some((
                    ridecore::i18n::t("Hoger! Meer vermogen", "Higher! More power"),
                    WARN,
                ))
            } else {
                None
            };
            if let Some((text, color)) = hint {
                p.text(
                    pos2(bx + 52.0, by - 46.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    display(22.0),
                    color,
                );
            }
        }
    }

    fn draw_pizza(&self, p: &Painter, screen: Rect, area: Rect, elapsed: f32, target: Option<f32>) {
        vgradient(
            p,
            screen,
            Color32::from_rgb(92, 44, 34),
            Color32::from_rgb(36, 20, 18),
        );
        // Bricks.
        let bh = 34.0;
        let mut row = 0;
        let mut y = screen.top();
        while y < screen.bottom() {
            let off = if row % 2 == 0 { 0.0 } else { 40.0 };
            p.line_segment(
                [pos2(screen.left(), y), pos2(screen.right(), y)],
                Stroke::new(2.0, Color32::from_black_alpha(60)),
            );
            let mut x = screen.left() - off;
            while x < screen.right() {
                p.line_segment(
                    [pos2(x, y), pos2(x, y + bh)],
                    Stroke::new(2.0, Color32::from_black_alpha(60)),
                );
                x += 80.0;
            }
            y += bh;
            row += 1;
        }

        let r = (area.height() * 0.4).min(area.width() * 0.2).max(90.0);
        let c = pos2(area.center().x, area.top() + area.height() * 0.42);
        let heat = self.heat.clamp(0.0, 1.6);
        // The oven: a stone dome with a glowing mouth.
        p.circle_filled(c, r * 1.12, Color32::from_rgb(140, 124, 110));
        p.circle_stroke(
            c,
            r * 1.12,
            Stroke::new(6.0, Color32::from_rgb(105, 92, 80)),
        );
        let mouth = c + vec2(0.0, r * 0.25);
        p.circle_filled(mouth, r * 0.7, Color32::from_rgb(22, 12, 10));
        let glow = (heat / 1.2).clamp(0.0, 1.0);
        for k in 0..4 {
            p.circle_filled(
                mouth,
                r * (0.66 - k as f32 * 0.12),
                Color32::from_rgba_unmultiplied(255, 120 + 30 * k, 30, (40.0 * glow) as u8),
            );
        }
        // Flames along the back, taller and whiter as it gets hotter.
        for k in 0..7 {
            let x = mouth.x + (k as f32 - 3.0) * r * 0.16;
            let flicker = (self.clock * (7.0 + k as f32) + k as f32).sin() * 0.15 + 1.0;
            let h = r * 0.45 * heat * flicker;
            let base = mouth.y + r * 0.1;
            let color = if heat > 1.0 + TOL {
                Color32::from_rgb(255, 240, 200)
            } else if heat > 1.0 - TOL {
                Color32::from_rgb(255, 190, 60)
            } else {
                Color32::from_rgb(220, 80, 30)
            };
            p.add(Shape::convex_polygon(
                vec![
                    pos2(x - r * 0.07, base),
                    pos2(x, base - h),
                    pos2(x + r * 0.07, base),
                ],
                color.gamma_multiply(0.9),
                Stroke::NONE,
            ));
        }
        // The counter in front of the oven.
        p.rect_filled(
            Rect::from_min_max(
                pos2(screen.left(), mouth.y + r * 0.5),
                pos2(screen.right(), screen.bottom()),
            ),
            0.0,
            Color32::from_rgb(58, 40, 30),
        );
        p.line_segment(
            [
                pos2(screen.left(), mouth.y + r * 0.5),
                pos2(screen.right(), mouth.y + r * 0.5),
            ],
            Stroke::new(4.0, Color32::from_rgb(120, 86, 60)),
        );

        // The pizza on its peel.
        let (round, progress, len) = self.round_progress(elapsed);
        let variety = self.shelf.len() * 7 % 6;
        let done = (self.on / len.max(1.0)).clamp(0.0, 1.0);
        let burn = (self.over / len.max(1.0)).clamp(0.0, 1.0);
        let pc = mouth + vec2(0.0, r * 0.38);
        p.rect_filled(
            Rect::from_center_size(pc + vec2(0.0, r * 0.62), vec2(r * 0.16, r * 0.5)),
            4.0,
            Color32::from_rgb(170, 120, 70),
        );
        pizza(p, pc, r * 0.42, variety, done, burn, round as u32);

        // The thermometer.
        let tx = (c.x + r * 1.45).min(area.right() - 30.0);
        let tube = Rect::from_min_max(
            pos2(tx - 14.0, area.top() + 20.0),
            pos2(tx + 14.0, area.bottom() - 20.0),
        );
        p.rect_filled(tube, 14.0, Color32::from_black_alpha(140));
        let y_of = |h: f32| tube.bottom() - ((h - 0.6) / 0.8).clamp(0.0, 1.0) * tube.height();
        let tol = target.map_or(TOL, |t| band(t) / t);
        p.rect_filled(
            Rect::from_min_max(
                pos2(tube.left(), y_of(1.0 + tol)),
                pos2(tube.right(), y_of(1.0 - tol)),
            ),
            0.0,
            GOOD.gamma_multiply(0.55),
        );
        let fill = Rect::from_min_max(
            pos2(tube.left() + 6.0, y_of(heat)),
            pos2(tube.right() - 6.0, tube.bottom() - 4.0),
        );
        let hot = if (heat - 1.0).abs() <= tol {
            GOOD
        } else if heat > 1.0 {
            Color32::from_rgb(255, 80, 50)
        } else {
            Color32::from_rgb(120, 170, 255)
        };
        p.rect_filled(fill, 8.0, hot);
        p.text(
            pos2(tube.center().x, tube.top() - 14.0),
            egui::Align2::CENTER_CENTER,
            format!("{:.0}°", 250.0 * heat),
            display(26.0),
            TEXT,
        );
        let label = if (heat - 1.0).abs() <= tol {
            t("Perfect vuur", "Perfect fire")
        } else if heat > 1.0 {
            t("Te heet!", "Too hot!")
        } else {
            t("Te koud", "Too cold")
        };
        p.text(
            pos2(tube.left() - 12.0, y_of(heat)),
            egui::Align2::RIGHT_CENTER,
            label,
            display(22.0),
            hot,
        );

        // The order ticket.
        let ticket = Rect::from_min_size(pos2(area.left(), area.top() + 20.0), vec2(230.0, 120.0));
        p.rect_filled(ticket, 6.0, Color32::from_rgb(246, 238, 220));
        p.text(
            ticket.left_top() + vec2(14.0, 12.0),
            egui::Align2::LEFT_TOP,
            tf!("Bestelling #{}", "Order #{}", round + 1),
            display(20.0),
            Color32::from_rgb(60, 40, 30),
        );
        p.text(
            ticket.left_top() + vec2(14.0, 40.0),
            egui::Align2::LEFT_TOP,
            PIZZAS[variety],
            display(26.0),
            Color32::from_rgb(170, 40, 30),
        );
        let bar = Rect::from_min_size(ticket.left_top() + vec2(14.0, 82.0), vec2(202.0, 12.0));
        p.rect_filled(bar, 6.0, Color32::from_black_alpha(40));
        p.rect_filled(
            Rect::from_min_size(bar.min, vec2(bar.width() * progress, bar.height())),
            6.0,
            Color32::from_rgb(200, 120, 40),
        );
        p.text(
            ticket.left_top() + vec2(14.0, 100.0),
            egui::Align2::LEFT_TOP,
            tf!("gaar: {:.0}%", "done: {:.0}%", done / needed(len) * 100.0),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(90, 70, 60),
        );

        // Served pizzas, under the ticket.
        for (i, o) in self.shelf.iter().rev().take(15).enumerate() {
            let at = ticket.left_bottom()
                + vec2(22.0 + (i % 5) as f32 * 46.0, 34.0 + (i / 5) as f32 * 46.0);
            if o.good {
                pizza(p, at, 18.0, o.variety, 1.0, 0.0, i as u32);
            } else if o.burnt {
                p.circle_filled(at, 18.0, Color32::from_rgb(40, 30, 26));
            } else {
                p.circle_filled(at, 18.0, Color32::from_rgb(235, 220, 180));
            }
        }
    }

    fn draw_fishing(
        &self,
        p: &Painter,
        screen: Rect,
        area: Rect,
        elapsed: f32,
        target: Option<f32>,
    ) {
        let water = area.top() + area.height() * 0.18;
        vgradient(
            p,
            Rect::from_min_max(screen.min, pos2(screen.right(), water)),
            Color32::from_rgb(120, 180, 230),
            Color32::from_rgb(210, 230, 240),
        );
        // Far shore with trees.
        p.rect_filled(
            Rect::from_min_max(
                pos2(screen.left(), water - 26.0),
                pos2(screen.right(), water),
            ),
            0.0,
            Color32::from_rgb(70, 120, 80),
        );
        for k in 0..30 {
            let x = screen.left() + k as f32 * screen.width() / 29.0;
            let h = 30.0 + (k * 37 % 23) as f32;
            p.add(Shape::convex_polygon(
                vec![
                    pos2(x - 16.0, water - 20.0),
                    pos2(x, water - 20.0 - h),
                    pos2(x + 16.0, water - 20.0),
                ],
                Color32::from_rgb(40, 90, 60),
                Stroke::NONE,
            ));
        }
        vgradient(
            p,
            Rect::from_min_max(pos2(screen.left(), water), screen.max),
            Color32::from_rgb(40, 110, 150),
            Color32::from_rgb(8, 30, 56),
        );
        for k in 0..14 {
            let y = water + 12.0 + k as f32 * 26.0;
            let pts: Vec<Pos2> = (0..=48)
                .map(|i| {
                    let x = screen.left() + i as f32 * screen.width() / 48.0;
                    pos2(x, y + (x * 0.02 + self.clock * 1.5 + k as f32).sin() * 3.0)
                })
                .collect();
            p.add(Shape::line(
                pts,
                Stroke::new(1.5, Color32::from_white_alpha(18)),
            ));
        }

        // The jetty and the angler.
        let jetty = area.left() + 150.0;
        p.rect_filled(
            Rect::from_min_max(pos2(screen.left(), water - 14.0), pos2(jetty, water + 4.0)),
            0.0,
            Color32::from_rgb(120, 84, 54),
        );
        for x in [jetty - 20.0, jetty - 90.0] {
            p.rect_filled(
                Rect::from_min_max(pos2(x - 5.0, water), pos2(x + 5.0, water + 60.0)),
                0.0,
                Color32::from_rgb(90, 62, 40),
            );
        }
        let me = pos2(jetty - 50.0, water - 14.0);
        p.rect_filled(
            Rect::from_center_size(me - vec2(0.0, 26.0), vec2(22.0, 36.0)),
            6.0,
            ACCENT,
        );
        p.circle_filled(me - vec2(0.0, 54.0), 11.0, Color32::from_rgb(240, 200, 170));
        p.rect_filled(
            Rect::from_center_size(me - vec2(0.0, 63.0), vec2(26.0, 6.0)),
            2.0,
            Color32::from_rgb(40, 60, 90),
        );

        let tension = self.heat.clamp(0.0, 1.6);
        let tol = target.map_or(TOL, |t| band(t) / t);
        let bend = ((tension - 0.6) * 60.0).clamp(0.0, 70.0);
        let tip = pos2(jetty + 110.0, water - 120.0 + bend);
        p.add(Shape::line(
            vec![
                me - vec2(-6.0, 36.0),
                pos2(jetty + 50.0, water - 100.0 + bend * 0.4),
                tip,
            ],
            Stroke::new(4.0, Color32::from_rgb(60, 40, 30)),
        ));

        // The fish.
        let (round, _progress, len) = self.round_progress(elapsed);
        let variety = self.shelf.len() * 7 % 6;
        let kind = fish_of(len, variety);
        let size = 26.0 + kind as f32 * 7.0;
        let reach = (area.right() - 200.0 - tip.x).max(100.0);
        let fight = if tension < 1.0 - tol { 2.0 } else { 1.0 };
        let fx = tip.x + 80.0 + self.reel * reach + (self.clock * 2.3).sin() * 18.0 * fight;
        let fy = water + area.height() * 0.45 + (self.clock * 1.7).sin() * 22.0 * fight;
        let fish_at = pos2(fx, fy);
        let body = Color32::from_rgb(90 + kind as u8 * 20, 150 - kind as u8 * 10, 110);
        let wag = (self.clock * 9.0 * fight).sin() * 6.0;
        p.add(Shape::convex_polygon(
            vec![
                fish_at + vec2(size * 0.8, 0.0),
                fish_at + vec2(size * 1.4, -size * 0.5 + wag),
                fish_at + vec2(size * 1.4, size * 0.5 + wag),
            ],
            body.gamma_multiply(0.8),
            Stroke::NONE,
        ));
        p.add(Shape::ellipse_filled(fish_at, vec2(size, size * 0.5), body));
        p.circle_filled(
            fish_at + vec2(-size * 0.6, -size * 0.12),
            3.5,
            Color32::WHITE,
        );
        p.circle_filled(
            fish_at + vec2(-size * 0.62, -size * 0.12),
            1.8,
            Color32::BLACK,
        );

        // The line: slack sags, too tight goes red and trembles.
        let mouth = fish_at - vec2(size, 0.0);
        let line_color = if (tension - 1.0).abs() <= tol {
            GOOD
        } else if tension > 1.0 {
            Color32::from_rgb(255, 70, 50)
        } else {
            WARN
        };
        let sag = ((1.0 - tol - tension).max(0.0) * 400.0).min(160.0);
        let shake = if tension > 1.0 + tol {
            (self.clock * 60.0).sin() * 3.0
        } else {
            0.0
        };
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let f = i as f32 / 24.0;
                let mut q = tip + (mouth - tip) * f;
                q.y += sag * 4.0 * f * (1.0 - f) + shake * (f * std::f32::consts::PI).sin();
                q
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(2.0, line_color)));

        // Tension gauge.
        let g = pos2(area.left() + 110.0, water + 190.0);
        let rad = 80.0;
        let ang = |v: f32| std::f32::consts::PI * (1.0 - ((v - 0.6) / 0.8).clamp(0.0, 1.0));
        let arc = |from: f32, to: f32, color: Color32, w: f32| {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = ang(from + (to - from) * i as f32 / 20.0);
                    g + vec2(a.cos(), -a.sin()) * rad
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(w, color)));
        };
        p.circle_filled(g, rad + 18.0, Color32::from_black_alpha(90));
        arc(0.6, 1.0 - tol, WARN, 10.0);
        arc(1.0 - tol, 1.0 + tol, GOOD, 14.0);
        arc(1.0 + tol, 1.4, Color32::from_rgb(255, 70, 50), 10.0);
        let a = ang(tension);
        p.line_segment(
            [g, g + vec2(a.cos(), -a.sin()) * (rad - 6.0)],
            Stroke::new(4.0, TEXT),
        );
        p.circle_filled(g, 6.0, TEXT);
        let word = if (tension - 1.0).abs() <= tol {
            t("Strak!", "Tight!")
        } else if tension > 1.0 {
            t("Lijn kraakt!", "Line's creaking!")
        } else {
            t("Te slap", "Too slack")
        };
        p.text(
            g + vec2(0.0, 26.0),
            egui::Align2::CENTER_CENTER,
            word,
            display(24.0),
            line_color,
        );
        p.text(
            g + vec2(0.0, 52.0),
            egui::Align2::CENTER_CENTER,
            tf!(
                "{} · nog {:.0} m",
                "{} · {:.0} m to go",
                fish_name(kind),
                self.reel * 30.0
            ),
            egui::FontId::proportional(15.0),
            TEXT,
        );

        // The catch so far.
        let bucket = pos2(area.left() + 260.0, area.bottom() - 30.0);
        for (i, o) in self.shelf.iter().rev().take(10).enumerate() {
            let at = bucket + vec2(i as f32 * 46.0, 0.0);
            let k = fish_of(o.size, o.variety);
            let c = if o.good {
                Color32::from_rgb(90 + k as u8 * 20, 150 - k as u8 * 10, 110)
            } else {
                Color32::from_white_alpha(30)
            };
            p.add(Shape::ellipse_filled(at, vec2(16.0, 8.0), c));
            p.add(Shape::convex_polygon(
                vec![
                    at + vec2(14.0, 0.0),
                    at + vec2(22.0, -7.0),
                    at + vec2(22.0, 7.0),
                ],
                c,
                Stroke::NONE,
            ));
        }
        let _ = round;
    }
}

fn fish_of(len: f32, variety: usize) -> usize {
    ((len / ROUND_MAX_S * 4.0) as usize + variety % 2).min(FISH.len() - 1)
}

fn fish_cm(len: f32, variety: usize) -> u32 {
    (18.0 + fish_of(len, variety) as f32 * 14.0 + (variety * 5 % 9) as f32) as u32
}

fn vgradient(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(Shape::mesh(mesh));
}

fn cloud(p: &Painter, at: Pos2, size: f32, alpha: u8) {
    let c = Color32::from_white_alpha(alpha);
    p.circle_filled(at, size, c);
    p.circle_filled(at + vec2(size * 0.9, size * 0.2), size * 0.8, c);
    p.circle_filled(at + vec2(-size * 0.9, size * 0.25), size * 0.7, c);
    p.circle_filled(at + vec2(size * 0.2, -size * 0.5), size * 0.75, c);
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A pizza: the crust browns as it bakes, burn shows as dark spots.
fn pizza(p: &Painter, c: Pos2, r: f32, variety: usize, done: f32, burn: f32, seed: u32) {
    let crust = lerp(
        Color32::from_rgb(238, 222, 180),
        Color32::from_rgb(214, 148, 64),
        done,
    );
    p.circle_filled(c, r, crust);
    p.circle_filled(c, r * 0.84, Color32::from_rgb(196, 52, 30));
    let cheese = lerp(
        Color32::from_rgb(250, 240, 200),
        Color32::from_rgb(246, 200, 90),
        done,
    );
    for k in 0..9 {
        let a = k as f32 * 2.4 + seed as f32;
        let d = r * 0.45 * ((k % 3) as f32 * 0.4 + 0.3);
        p.circle_filled(c + vec2(a.cos(), a.sin()) * d, r * 0.24, cheese);
    }
    let (topping, size) = match variety {
        1 | 5 => (Color32::from_rgb(150, 30, 30), 0.13),
        2 => (Color32::from_rgb(170, 150, 130), 0.1),
        4 => (Color32::from_rgb(250, 220, 90), 0.09),
        3 => (Color32::from_rgb(255, 250, 230), 0.1),
        _ => (Color32::from_rgb(40, 140, 60), 0.08),
    };
    for k in 0..7 {
        let a = k as f32 * 0.9 + 0.5;
        let d = r * (0.25 + 0.4 * ((k * 3 % 5) as f32 / 5.0));
        p.circle_filled(c + vec2(a.cos(), a.sin()) * d, r * size, topping);
    }
    let spots = (burn * 30.0) as usize;
    for k in 0..spots.min(24) {
        let a = k as f32 * 2.7 + 1.0;
        let d = r * (0.2 + 0.75 * ((k * 7 % 10) as f32 / 10.0));
        p.circle_filled(
            c + vec2(a.cos(), a.sin()) * d,
            r * 0.07,
            Color32::from_rgba_unmultiplied(30, 20, 15, 220),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_blocks_are_cut_into_rounds() {
        let g = Game::new(GameKind::Pizza, &[600, 30, 120], 300.0);
        // 600 s → 4 rounds of 150 s, then 30 s and 120 s.
        assert_eq!(g.rounds.len(), 6);
        assert!((g.rounds[3].end - 600.0).abs() < 1e-3);
        assert!((g.rounds[5].end - 750.0).abs() < 1e-3);
    }

    fn ride(kind: GameKind, power: f32) -> Game {
        let mut g = Game::new(kind, &[60], 250.0);
        g.power = power;
        g.heat = power / 250.0;
        let dt = 0.1;
        let mut t = 0.0;
        while t < 61.0 {
            t += dt;
            g.update(dt, t, power, (t < 60.0).then_some(250.0));
        }
        g
    }

    #[test]
    fn on_target_wins_every_game() {
        for kind in GameKind::ALL {
            let g = ride(kind, 250.0);
            assert_eq!(g.rounds_won, 1, "{kind:?}");
        }
        assert_eq!(ride(GameKind::Balloon, 250.0).score, 29);
    }

    #[test]
    fn too_hot_burns_and_too_weak_loses_the_fish() {
        let hot = ride(GameKind::Pizza, 300.0);
        assert_eq!(hot.rounds_won, 0);
        assert!(hot.shelf[0].burnt);
        let weak = ride(GameKind::Fishing, 200.0);
        assert_eq!(weak.rounds_won, 0);
        assert_eq!(ride(GameKind::Balloon, 200.0).score, 0);
    }
}
