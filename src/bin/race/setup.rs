//! Race setup screen: your profile, the course, and the field of teams,
//! over a dusk sky with the chosen course as a mountain silhouette.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use ridecore::custom::{self, CustomWorkout};
use ridecore::i18n::{self, Lang, t};
use ridecore::profile::Profile;
use ridecore::race::course::find_climbs;
use ridecore::race::field::{RiderKind, TeamSpec, rival_teams};
use ridecore::race::{PlayerSetup, Race, RaceConfig};
use ridecore::route::Route;
use ridecore::tf;
use ridecore::training::{Focus, Plan, library};

use crate::builder::{Action, Builder, builder_ui};
use crate::link::{InputSettings, Trainer};
use crate::theme::{self, ACCENT, ACCENT_SOFT, MUTED, TEXT, card, chip, display, stat};
use crate::world::Daylight;
use crate::{RaceRes, Screen};

const KINDS: [RiderKind; 4] = [
    RiderKind::Sprinter,
    RiderKind::Climber,
    RiderKind::Rouleur,
    RiderKind::Domestique,
];

/// What the start screen sets up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Race,
    Training,
    Sprint,
}

#[derive(Resource)]
pub struct SetupForm {
    pub mode: PlayMode,
    pub name: String,
    pub ftp: f32,
    pub kg: f32,
    pub height_cm: f32,
    pub kind: RiderKind,
    pub route: usize,
    pub laps: u32,
    pub daylight: Daylight,
    pub team_count: usize,
    pub riders_per_team: usize,
    /// Field strength as a multiple of your W/kg.
    pub difficulty: f32,
    pub teams: Vec<TeamSpec>,
    pub player_team: usize,
    /// Every course, loaded once.
    pub routes: Vec<Route>,
    /// Text filter for the route list.
    pub route_filter: String,
    /// Per route: number of climbs and a coarse elevation profile.
    infos: Vec<RouteInfo>,
    show_team_editor: bool,
    /// The workout library (built for the current FTP) and the pick.
    pub plans: Vec<Plan>,
    pub workout: usize,
    /// Ride the workout as a game instead of in ERG.
    pub game: Option<crate::games::GameKind>,
    focus_filter: Option<Focus>,
    /// Show only your own workouts.
    mine_only: bool,
    /// Workouts you built, and the one being edited.
    pub customs: Vec<CustomWorkout>,
    pub builder: Option<Builder>,
    /// A custom workout waiting for a second click to be deleted.
    confirm_delete: Option<String>,
    /// A short message under the workout (export, import).
    notice: Option<String>,
    /// Saved between sessions: profile and training log.
    pub profile: Profile,
    /// Catalog indices of the roads flat enough for sprints.
    sprint_roads: Vec<usize>,
}

struct RouteInfo {
    climbs: usize,
    hardest: &'static str,
    profile: Vec<f32>,
}

impl RouteInfo {
    fn of(route: &Route) -> Self {
        let climbs = find_climbs(route);
        let len = route.length_m();
        Self {
            climbs: climbs.len(),
            hardest: climbs
                .iter()
                .map(|c| c.category)
                .max()
                .map(|c| c.label())
                .unwrap_or(""),
            profile: (0..=48)
                .map(|k| route.elevation_at(len * k as f64 / 48.0))
                .collect(),
        }
    }
}

impl Default for SetupForm {
    fn default() -> Self {
        let mut profile = Profile::load();
        // The default name, in the language you ride in.
        if profile.name == "Jij" && i18n::english() {
            profile.name = "You".into();
        }
        let mut form = Self {
            mode: PlayMode::Race,
            name: profile.name.clone(),
            ftp: profile.ftp,
            kg: profile.kg,
            height_cm: profile.height_cm,
            kind: KINDS[(profile.role as usize).min(KINDS.len() - 1)],
            // The first course in the catalog: Alpe d'Huez (14 km).
            route: 0,
            laps: 2,
            daylight: Daylight::default(),
            team_count: 8,
            riders_per_team: 6,
            difficulty: 1.0,
            teams: Vec::new(),
            player_team: 4,
            routes: Route::catalog(),
            route_filter: String::new(),
            infos: Vec::new(),
            show_team_editor: false,
            plans: library(profile.ftp),
            workout: 0,
            game: None,
            focus_filter: None,
            mine_only: false,
            // Screenshot runs never see or touch your own workouts.
            customs: if std::env::var_os("RIDECORE_SHOTS").is_some() {
                Vec::new()
            } else {
                custom::load_all()
            },
            builder: None,
            confirm_delete: None,
            notice: None,
            sprint_roads: Vec::new(),
            profile,
        };
        form.infos = form.routes.iter().map(RouteInfo::of).collect();
        // `RIDECORE_MODE=training|sprint` opens on that tab (screenshots).
        if std::env::var("RIDECORE_MODE").is_ok_and(|m| m == "sprint") {
            form.mode = PlayMode::Sprint;
        }
        if std::env::var("RIDECORE_MODE").is_ok_and(|m| m == "training") {
            form.mode = PlayMode::Training;
            form.workout = 1;
        }
        // `RIDECORE_BUILDER=1` opens the builder on an example (screenshots).
        if std::env::var_os("RIDECORE_BUILDER").is_some() {
            form.mode = PlayMode::Training;
            let mut w = CustomWorkout::new("Dinsdag-intervallen");
            let tpls = custom::templates();
            for k in [0, 3, 10, 5, 1] {
                w.blocks.push(w.template_block(&tpls[k].block, form.ftp));
            }
            w.notes = "Rustig opbouwen, en de 4 × 4 vol houden op 95 rpm.".into();
            // `RIDECORE_BUILDER=list` shows it saved in the library instead.
            if std::env::var("RIDECORE_BUILDER").is_ok_and(|v| v == "list") {
                form.save_custom(w);
            } else {
                form.builder = Some(Builder::new(w, true));
            }
        }
        form.regenerate();
        form
    }
}

impl SetupForm {
    fn regenerate(&mut self) {
        self.teams = rival_teams(self.team_count, self.ftp / self.kg, self.difficulty);
        self.player_team = self.player_team.min(self.team_count - 1);
        let id = self.plans.get(self.workout).map(|p| p.id.clone());
        self.plans = library(self.ftp);
        self.plans
            .extend(self.customs.iter().map(|w| w.to_plan(self.ftp)));
        if let Some(id) = id {
            self.select_plan(&id);
        }
    }

    fn select_plan(&mut self, id: &str) {
        if let Some(i) = self.plans.iter().position(|p| p.id == id) {
            self.workout = i;
        }
    }

    fn write_customs(&mut self) {
        if std::env::var_os("RIDECORE_SHOTS").is_some() {
            return;
        }
        if let Err(e) = custom::save_all(&self.customs) {
            self.notice = Some(tf!("Opslaan mislukt: {e}", "Saving failed: {e}"));
        }
    }

    /// Adds or replaces a custom workout, saves, and selects it.
    pub fn save_custom(&mut self, w: CustomWorkout) {
        let id = w.id.clone();
        match self.customs.iter_mut().find(|c| c.id == id) {
            Some(c) => *c = w,
            None => self.customs.push(w),
        }
        self.write_customs();
        self.regenerate();
        self.select_plan(&id);
        self.mine_only = false;
        self.focus_filter = None;
    }

    fn delete_custom(&mut self, id: &str) {
        self.customs.retain(|c| c.id != id);
        self.write_customs();
        self.workout = 0;
        self.regenerate();
    }

    /// Writes name, FTP, weight and role back to the profile on disk.
    pub fn save_profile(&mut self) {
        // Automated screenshot runs never touch the real profile.
        if std::env::var_os("RIDECORE_SHOTS").is_some() {
            return;
        }
        let p = &mut self.profile;
        p.name = self.name.clone();
        p.ftp = self.ftp;
        p.kg = self.kg;
        p.height_cm = self.height_cm;
        p.role = KINDS.iter().position(|&k| k == self.kind).unwrap_or(2) as u8;
        if let Err(e) = p.save() {
            eprintln!("profile not saved: {e}");
        }
    }

    /// Switches the language and rebuilds everything that holds text.
    fn set_lang(&mut self, lang: Lang) {
        if i18n::lang() == lang {
            return;
        }
        i18n::set_lang(lang);
        self.profile.lang = lang.code().into();
        let id = self.routes[self.route].id.clone();
        self.routes = Route::catalog();
        self.route = self.routes.iter().position(|r| r.id == id).unwrap_or(0);
        self.infos = self.routes.iter().map(RouteInfo::of).collect();
        if self.name == "Jij" || self.name == "You" {
            self.name = t("Jij", "You").into();
        }
        self.regenerate();
        self.save_profile();
    }

    pub fn log_session(&mut self, session: ridecore::profile::Session) {
        self.profile.sessions.push(session);
        self.save_profile();
    }

    pub fn set_ftp(&mut self, ftp: f32) {
        self.ftp = ftp;
        self.regenerate();
        self.save_profile();
    }

    /// A sprint session: you and the rivals, one rider a team, on the
    /// road picked for it, with enough laps for the whole session.
    fn sprint_config(&self) -> RaceConfig {
        let s = &self.profile.sprint;
        let mut cfg = self.config();
        let route = self
            .routes
            .iter()
            .find(|r| r.id == s.route)
            .or_else(|| self.routes.iter().find(|r| r.id == "flat-loop"))
            .unwrap_or(&self.routes[self.route])
            .clone();
        let needed = s.total_s() as f64 * 15.0;
        cfg.laps = if route.looped {
            (needed / route.length_m()).ceil().max(1.0) as u32 + 1
        } else {
            1
        };
        cfg.route = route;
        let mine = self.teams[self.player_team].clone();
        let rivals = rival_teams(
            ridecore::race::field::TEAM_PRESETS.len(),
            self.ftp / self.kg,
            1.0,
        )
        .into_iter()
        .filter(|t| t.name != mine.name)
        .take(s.rivals as usize)
        .collect::<Vec<_>>();
        cfg.teams = std::iter::once(mine).chain(rivals).collect();
        cfg.riders_per_team = 1;
        cfg.player_team = 0;
        cfg.countdown_s = 3.0;
        cfg
    }

    /// Starts the sprint session set up on the sprint tab.
    pub fn start_sprints(&mut self, commands: &mut Commands) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(7);
        let mut race = Race::new(&self.sprint_config());
        let mut drill = ridecore::sprint::Drill::new(self.profile.sprint.clone(), self.ftp, seed);
        drill.take_over(&mut race);
        let before = ridecore::sprint::records(&self.profile.sprint_log);
        commands.remove_resource::<crate::training::Session>();
        commands.insert_resource(crate::sprint::SprintSession::new(drill, before));
        let mut res = RaceRes::new(race);
        // No ghost of an old race riding along.
        res.best = None;
        commands.insert_resource(res);
    }

    /// A workout: you alone on the chosen road, with enough laps for it.
    fn training_config(&self) -> RaceConfig {
        let mut cfg = self.config();
        let route = &self.routes[self.route];
        let plan = &self.plans[self.workout];
        // Generous: 14 m/s for the whole session.
        let needed = plan.secs() as f64 * 14.0;
        cfg.laps = if route.looped {
            (needed / route.length_m()).ceil().max(1.0) as u32 + 1
        } else {
            1
        };
        let team = self.teams[self.player_team].clone();
        cfg.teams = vec![team];
        cfg.riders_per_team = 1;
        cfg.player_team = 0;
        cfg.countdown_s = 3.0;
        cfg
    }

    fn config(&self) -> RaceConfig {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        RaceConfig {
            seed,
            route: self.routes[self.route].clone(),
            laps: self.laps,
            teams: self.teams.clone(),
            riders_per_team: self.riders_per_team,
            player_team: self.player_team,
            player: PlayerSetup {
                name: self.name.clone(),
                kg: self.kg,
                height_cm: self.height_cm,
                ftp: self.ftp,
                kind: self.kind,
            },
            countdown_s: 10.0,
        }
    }
}

pub struct SetupPlugin;

impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SetupForm>()
            .add_systems(Startup, autostart)
            .add_systems(Update, import_dropped.run_if(in_state(Screen::Setup)))
            .add_systems(OnExit(Screen::Setup), pick_daylight)
            .add_systems(
                EguiPrimaryContextPass,
                setup_ui
                    .run_if(in_state(Screen::Setup))
                    .run_if(crate::theme::ready),
            );
    }
}

/// The ride takes the time of day picked on the course card
/// (`RIDECORE_DAYLIGHT` overrides it for test runs).
fn pick_daylight(form: Res<SetupForm>, mut daylight: ResMut<Daylight>) {
    *daylight = Daylight::from_env().unwrap_or(form.daylight);
}

/// Development shortcut: `RIDECORE_AUTOSTART=1` starts a keyboard-mode race
/// straight away, `RIDECORE_AUTOSTART=<watts>` also sets the keyboard watts.
fn autostart(
    mut form: ResMut<SetupForm>,
    mut settings: ResMut<InputSettings>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
) {
    let Ok(value) = std::env::var("RIDECORE_AUTOSTART") else {
        return;
    };
    settings.keyboard = true;
    // `RIDECORE_ROUTE=<id>` picks the course.
    if let Ok(id) = std::env::var("RIDECORE_ROUTE")
        && let Some(i) = form.routes.iter().position(|r| r.id == id)
    {
        form.route = i;
    }
    if let Ok(watts) = value.parse::<f32>()
        && watts > 1.0
    {
        settings.keyboard_watts = watts;
    }
    // `RIDECORE_SPRINTS=1` rides a sprint session instead, with
    // `RIDECORE_SPRINT_WARMUP=<s>` for a shorter warm-up,
    // `RIDECORE_SPRINT_COUNT=<n>` sprints and `RIDECORE_SPRINT_REST=<s>`
    // (no cool-down) for a short session.
    if std::env::var_os("RIDECORE_SPRINTS").is_some() {
        let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
        let s = &mut form.profile.sprint;
        if let Some(w) = env("RIDECORE_SPRINT_WARMUP") {
            s.warmup_s = w;
        }
        if let Some(c) = env("RIDECORE_SPRINT_COUNT") {
            s.count = c as u32;
        }
        if let Some(r) = env("RIDECORE_SPRINT_REST") {
            s.rest_s = r;
            s.cooldown_s = 0.0;
        }
        form.start_sprints(&mut commands);
        next.set(Screen::Racing);
        return;
    }
    // `RIDECORE_TRAINING=<workout id>` rides a workout instead, and
    // `RIDECORE_SKIP_S=<s>` starts it that far in.
    if let Ok(id) = std::env::var("RIDECORE_TRAINING")
        && let Some(i) = form.plans.iter().position(|p| p.id == id)
    {
        form.workout = i;
        // `RIDECORE_GAME=balloon|pizza|fishing` rides it as a game.
        form.game = std::env::var("RIDECORE_GAME")
            .ok()
            .and_then(|g| crate::games::GameKind::from_id(&g));
        let mut session =
            crate::training::Session::new(form.plans[i].clone(), form.ftp).with_game(form.game);
        if let Some(skip) = std::env::var("RIDECORE_SKIP_S")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            session.skip_to(skip);
        }
        commands.insert_resource(session);
        commands.insert_resource(RaceRes::new(Race::new(&form.training_config())));
        next.set(Screen::Racing);
        return;
    }
    let mut race = Race::new(&form.config());
    // `RIDECORE_SKIP_KM=<km>` starts the whole field further down the road.
    if let Some(skip) = std::env::var("RIDECORE_SKIP_KM")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        for r in &mut race.riders {
            r.dist += skip * 1000.0;
        }
    }
    // `RIDECORE_WATCH=1` watches the race instead of riding it.
    commands.insert_resource(if std::env::var_os("RIDECORE_WATCH").is_some() {
        RaceRes::watching(race)
    } else {
        RaceRes::new(race)
    });
    next.set(Screen::Racing);
}

fn setup_ui(
    mut contexts: EguiContexts,
    mut form: ResMut<SetupForm>,
    mut settings: ResMut<InputSettings>,
    trainer: Res<Trainer>,
    mut strava: ResMut<crate::share::StravaUi>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let t = ctx.input(|i| i.time) as f32;
    ctx.request_repaint();
    let screen = ctx.viewport_rect();
    backdrop(
        &ctx.layer_painter(egui::LayerId::background()),
        screen,
        &form,
        t,
    );

    let mut root = egui::Ui::new(
        ctx.clone(),
        "setup".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::new(egui::Order::Middle, "setup-ui".into()))
            .max_rect(screen.shrink2(egui::vec2(28.0, 20.0))),
    );
    let ui = &mut root;
    strava.poll();
    if let Some(lang) = header(ui, &trainer, &strava) {
        form.set_lang(lang);
    }
    ui.add_space(10.0);
    // The workout builder takes the whole screen under the title.
    if form.builder.is_some() {
        ui.label(
            egui::RichText::new(i18n::t("TRAININGSBOUWER", "WORKOUT BUILDER"))
                .font(display(26.0))
                .color(ACCENT),
        );
        ui.add_space(6.0);
        let ftp = form.ftp;
        let action = form
            .builder
            .as_mut()
            .map_or(Action::None, |b| builder_ui(ui, b, ftp));
        match action {
            Action::None => {}
            Action::Cancel => form.builder = None,
            Action::Save => {
                if let Some(b) = form.builder.take() {
                    form.notice = Some(tf!("“{}” opgeslagen", "“{}” saved", b.workout.name));
                    form.save_custom(b.workout);
                }
            }
        }
        return Ok(());
    }
    mode_tabs(ui, &mut form);
    ui.add_space(12.0);

    let gap = 16.0;
    // A few pixels spare for the scroll bars.
    let col = ((ui.available_width() - 2.0 * gap) / 3.0 - 10.0).max(280.0);
    let height = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        column(ui, "left", col, height, |ui| {
            rider_card(ui, &mut form);
            if form.mode == PlayMode::Training {
                week_card(ui, &form);
            }
            devices_card(ui, &mut settings, &trainer, &mut strava);
        });
        if form.mode == PlayMode::Sprint {
            if form.sprint_roads.is_empty() {
                form.sprint_roads = crate::sprint::flat_roads(&form.routes);
            }
            let roads = form.sprint_roads.clone();
            column(ui, "mid", col, height, |ui| {
                crate::sprint::settings_card(ui, &mut form, &roads)
            });
            column(ui, "right-sprint", col, height, |ui| {
                let can_start = settings.keyboard || trainer.name.is_some();
                if crate::sprint::start_card(ui, &mut form, can_start, t) {
                    form.save_profile();
                    form.start_sprints(&mut commands);
                    next.set(Screen::Racing);
                }
            });
            return;
        }
        if form.mode == PlayMode::Training {
            column(ui, "mid", col, height, |ui| library_card(ui, &mut form));
            // Like the race, the start button stays in view under the details.
            ui.allocate_ui_with_layout(
                egui::vec2(col, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(col);
                    egui::ScrollArea::vertical()
                        .id_salt("right-training")
                        .max_height((height - TRAINING_START_H).max(200.0))
                        .show(ui, |ui| training_card(ui, &mut form));
                    let can_start = settings.keyboard || trainer.name.is_some();
                    if training_start(ui, &mut form, can_start, t) {
                        form.save_profile();
                        let plan = form.plans[form.workout].clone();
                        commands.insert_resource(
                            crate::training::Session::new(plan, form.ftp).with_game(form.game),
                        );
                        commands.insert_resource(RaceRes::new(Race::new(&form.training_config())));
                        next.set(Screen::Racing);
                    }
                },
            );
            return;
        }
        column(ui, "mid", col, height, |ui| course_card(ui, &mut form));
        // The start card sits under the field, which scrolls, so the start
        // button is always in view.
        ui.allocate_ui_with_layout(
            egui::vec2(col, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(col);
                egui::ScrollArea::vertical()
                    .id_salt("right")
                    .max_height((height - START_CARD_H).max(200.0))
                    .show(ui, |ui| field_card(ui, &mut form));
                let can_start = settings.keyboard || trainer.name.is_some();
                if let Some(start) = start_card(ui, &form, can_start, t) {
                    form.save_profile();
                    commands.remove_resource::<crate::training::Session>();
                    let race = Race::new(&form.config());
                    commands.insert_resource(match start {
                        Start::Ride => RaceRes::new(race),
                        Start::Watch => RaceRes::watching(race),
                    });
                    next.set(Screen::Racing);
                }
            },
        );
    });
    Ok(())
}

/// Room kept for the start card under the field.
const START_CARD_H: f32 = 300.0;

fn column(ui: &mut egui::Ui, id: &str, width: f32, height: f32, body: impl FnOnce(&mut egui::Ui)) {
    // Columns stack top-down; inheriting the row's left-to-right layout
    // would lay every card out sideways.
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            ui.set_max_width(width);
            egui::ScrollArea::vertical()
                .id_salt(id)
                .max_height(height)
                .show(ui, body);
        },
    );
}

/// Dusk sky, two ranges of distant mountains, and the chosen course's real
/// profile as the big silhouette in front.
fn backdrop(p: &egui::Painter, r: egui::Rect, form: &SetupForm, t: f32) {
    let mut sky = egui::Mesh::default();
    let top = egui::Color32::from_rgb(8, 10, 22);
    let mid = egui::Color32::from_rgb(38, 30, 66);
    let low = egui::Color32::from_rgb(170, 80, 60);
    let bands = [(0.0, top), (0.55, mid), (0.82, low), (1.0, low)];
    for w in bands.windows(2) {
        let (y0, c0) = w[0];
        let (y1, c1) = w[1];
        let i = sky.vertices.len() as u32;
        let ya = r.top() + r.height() * y0;
        let yb = r.top() + r.height() * y1;
        sky.colored_vertex(egui::pos2(r.left(), ya), c0);
        sky.colored_vertex(egui::pos2(r.right(), ya), c0);
        sky.colored_vertex(egui::pos2(r.right(), yb), c1);
        sky.colored_vertex(egui::pos2(r.left(), yb), c1);
        sky.add_triangle(i, i + 1, i + 2);
        sky.add_triangle(i, i + 2, i + 3);
    }
    p.add(sky);
    // A low sun glowing on the horizon.
    let sun = egui::pos2(r.left() + r.width() * 0.72, r.top() + r.height() * 0.74);
    for k in 0..10 {
        let a = 0.035 * (10 - k) as f32 / 10.0;
        p.circle_filled(
            sun,
            40.0 + k as f32 * 26.0,
            egui::Color32::from_rgb(255, 170, 90).gamma_multiply(a),
        );
    }
    p.circle_filled(sun, 34.0, egui::Color32::from_rgb(255, 205, 140));

    // Distant ranges, drifting slowly.
    let range = |seed: f32, base: f32, amp: f32, color: egui::Color32, drift: f32| {
        let n = 90;
        let mut pts = Vec::with_capacity(n + 3);
        for k in 0..=n {
            let x = k as f32 / n as f32;
            let u = x * 7.0 + seed + t * drift;
            let h = (u.sin() * 0.5 + (u * 2.3 + 1.7).sin() * 0.3 + (u * 5.1).sin() * 0.12) * amp;
            pts.push(egui::pos2(
                r.left() + x * r.width(),
                r.top() + r.height() * base - h,
            ));
        }
        fill_down(p, &pts, r.bottom(), color);
    };
    range(0.0, 0.70, 60.0, egui::Color32::from_rgb(52, 40, 78), 0.01);
    range(3.0, 0.78, 45.0, egui::Color32::from_rgb(30, 26, 52), 0.02);

    // The selected course, huge, in front.
    if let Some(info) = form.infos.get(form.route) {
        let (lo, hi) = info
            .profile
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &e| (a.min(e), b.max(e)));
        let span = (hi - lo).max(150.0);
        let n = info.profile.len() - 1;
        let pts: Vec<egui::Pos2> = info
            .profile
            .iter()
            .enumerate()
            .map(|(k, &e)| {
                egui::pos2(
                    r.left() + k as f32 / n as f32 * r.width(),
                    r.bottom() - 30.0 - (e - lo) / span * r.height() * 0.32,
                )
            })
            .collect();
        fill_down(p, &pts, r.bottom(), egui::Color32::from_rgb(14, 14, 28));
        p.line(pts, egui::Stroke::new(2.0, ACCENT.gamma_multiply(0.55)));
    }
}

fn fill_down(p: &egui::Painter, pts: &[egui::Pos2], bottom: f32, color: egui::Color32) {
    let mut m = egui::Mesh::default();
    for w in pts.windows(2) {
        let i = m.vertices.len() as u32;
        m.colored_vertex(w[0], color);
        m.colored_vertex(w[1], color);
        m.colored_vertex(egui::pos2(w[1].x, bottom), color);
        m.colored_vertex(egui::pos2(w[0].x, bottom), color);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    p.add(m);
}

/// Title, device chips and the language switch; returns a picked language.
fn header(ui: &mut egui::Ui, trainer: &Trainer, strava: &crate::share::StravaUi) -> Option<Lang> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(egui::RichText::new("RIDE").font(display(56.0)).color(TEXT));
                ui.label(
                    egui::RichText::new("CORE")
                        .font(display(56.0))
                        .color(ACCENT),
                );
            });
            ui.label(
                egui::RichText::new(t(
                    "Wegwedstrijden tegen AI-ploegen · trainingen die je sneller maken · op je eigen trainer",
                    "Road races against AI teams · workouts that make you faster · on your own trainer",
                ))
                .color(MUTED),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Language switch: the other language, in its own words.
            for lang in Lang::ALL.into_iter().rev() {
                let on = i18n::lang() == lang;
                if ui
                    .selectable_label(on, egui::RichText::new(lang.code().to_uppercase()).strong())
                    .on_hover_text(lang.name())
                    .clicked()
                {
                    picked = Some(lang);
                }
            }
            ui.add_space(8.0);
            let strava_text = match &strava.connected {
                Some(who) => format!("Strava · {who}"),
                None => t("Strava niet gekoppeld", "Strava not connected").into(),
            };
            chip(ui, &strava_text, strava.connected.is_some());
            let click = match (&trainer.shifter, trainer.shifter_battery) {
                (Some(_), Some(b)) => format!("Zwift Click · {b}%"),
                (Some(_), None) => "Zwift Click".into(),
                (None, _) => t("Click: druk op een knop", "Click: press a button").into(),
            };
            chip(ui, &click, trainer.shifter.is_some());
            let kickr = match &trainer.name {
                Some(n) => n.clone(),
                None => t("Trainer zoeken…", "Looking for trainer…").into(),
            };
            chip(ui, &kickr, trainer.name.is_some());
        });
    });
    picked
}

fn rider_card(ui: &mut egui::Ui, form: &mut SetupForm) {
    card(ui, t("Renner", "Rider"), |ui| {
        let mut regen = false;
        ui.horizontal(|ui| {
            let [r, g, b] = form
                .teams
                .get(form.player_team)
                .map(|t| t.color)
                .unwrap_or([255, 122, 26]);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(54.0, 54.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 26.0, egui::Color32::from_rgb(r, g, b));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                form.name
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string(),
                display(28.0),
                egui::Color32::WHITE,
            );
            ui.vertical(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut form.name)
                        .font(display(24.0))
                        .desired_width(200.0)
                        .frame(egui::Frame::NONE),
                );
                let team = form
                    .teams
                    .get(form.player_team)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                ui.label(egui::RichText::new(team).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                stat(ui, format!("{:.2}", form.ftp / form.kg), "W/kg", ACCENT);
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (label, value, range, suffix, speed) in [
                ("FTP", &mut form.ftp, 80.0..=600.0, " W", 1.0),
                (
                    t("Gewicht", "Weight"),
                    &mut form.kg,
                    35.0..=160.0,
                    " kg",
                    0.1,
                ),
            ] {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(label).small().color(MUTED));
                    regen |= ui
                        .add(
                            egui::DragValue::new(value)
                                .range(range)
                                .speed(speed)
                                .suffix(suffix),
                        )
                        .changed();
                });
            }
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(t("Lengte", "Height"))
                        .small()
                        .color(MUTED),
                );
                ui.add(
                    egui::DragValue::new(&mut form.height_cm)
                        .range(140.0..=215.0)
                        .suffix(" cm"),
                );
            });
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(t("Rol in de ploeg", "Role in the team"))
                .small()
                .color(MUTED),
        );
        ui.horizontal(|ui| {
            for k in KINDS {
                if ui.selectable_label(form.kind == k, k.label()).clicked() {
                    form.kind = k;
                }
            }
        });
        if regen {
            form.regenerate();
        }
    });
}

fn devices_card(
    ui: &mut egui::Ui,
    settings: &mut InputSettings,
    trainer: &Trainer,
    strava: &mut crate::share::StravaUi,
) {
    card(ui, t("Trainer & apparaten", "Trainer & devices"), |ui| {
        ui.label(egui::RichText::new(&trainer.status).color(MUTED));
        ui.checkbox(
            &mut settings.virtual_shifting,
            t(
                "Virtueel schakelen (Zwift Click of +/−)",
                "Virtual shifting (Zwift Click or +/−)",
            ),
        );
        ui.horizontal(|ui| {
            ui.label(t("Moeilijkheid", "Difficulty"));
            let gears = settings.virtual_shifting;
            ui.add_enabled(
                !gears,
                egui::Slider::new(&mut settings.difficulty, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
        });
        if settings.virtual_shifting {
            ui.label(
                egui::RichText::new(t(
                    "Met virtueel schakelen voel je 100% van de helling, net als in Zwift.",
                    "With virtual shifting you feel 100% of the gradient, just like in Zwift.",
                ))
                .small()
                .color(MUTED),
            );
        }
        ui.checkbox(
            &mut settings.keyboard,
            t(
                "Testmodus: rijden met het toetsenbord",
                "Test mode: ride with the keyboard",
            ),
        );
        if strava.connected.is_none() {
            ui.add_space(6.0);
            egui::CollapsingHeader::new(t("Strava koppelen", "Connect Strava"))
                .default_open(false)
                .show(ui, |ui| crate::share::settings_ui(ui, strava));
        }
    });
}

fn course_card(ui: &mut egui::Ui, form: &mut SetupForm) {
    card(ui, t("Parcours", "Course"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut form.route_filter)
                .hint_text(t(
                    "Zoek: Alpe, Italië, Dolomieten…",
                    "Search: Alpe, Italy, Dolomites…",
                ))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        let filter = form.route_filter.to_lowercase();
        let tile_w = (ui.available_width() - 10.0) / 2.0;
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("tiles")
            .max_height(196.0)
            .show(ui, |ui| {
                let shown: Vec<usize> = (0..form.routes.len())
                    .filter(|&i| {
                        let r = &form.routes[i];
                        filter.is_empty()
                            || format!("{} {}", r.name, r.region)
                                .to_lowercase()
                                .contains(&filter)
                    })
                    .collect();
                for pair in shown.chunks(2) {
                    ui.horizontal(|ui| {
                        for &i in pair {
                            if route_tile(ui, form, i, tile_w) {
                                pick = Some(i);
                            }
                        }
                    });
                }
            });
        if let Some(i) = pick {
            form.route = i;
        }

        ui.add_space(10.0);
        let route = &form.routes[form.route];
        let info = &form.infos[form.route];
        ui.label(egui::RichText::new(&route.name).font(display(26.0)));
        ui.label(egui::RichText::new(&route.region).color(MUTED));
        big_profile(ui, route);
        ui.horizontal(|ui| {
            stat(ui, format!("{:.1}", route.length_m() / 1000.0), "km", TEXT);
            ui.add_space(8.0);
            stat(
                ui,
                format!("{:.0}", route.gain_m()),
                t("hoogtemeters", "elevation m"),
                TEXT,
            );
            ui.add_space(8.0);
            stat(
                ui,
                format!("{:.0}%", route.max_grade_pct()),
                "max",
                theme::grade_color(route.max_grade_pct()),
            );
            ui.add_space(8.0);
            stat(ui, info.climbs.to_string(), t("klimmen", "climbs"), TEXT);
        });
        let (looped, lap_km) = (route.looped, route.length_m() / 1000.0);
        if looped {
            ui.horizontal(|ui| {
                ui.label(t("Ronden", "Laps"));
                ui.add(egui::DragValue::new(&mut form.laps).range(1..=20));
                ui.label(
                    egui::RichText::new(format!("= {:.1} km", lap_km * form.laps as f64))
                        .color(MUTED),
                );
            });
        } else {
            ui.label(
                egui::RichText::new(t(
                    "Van start naar finish op de echte weg",
                    "From start to finish on the real road",
                ))
                .color(MUTED),
            );
        }
        ui.horizontal(|ui| {
            ui.label(t("Tijdstip", "Time of day"));
            for d in Daylight::ALL {
                let name = match d {
                    Daylight::Morning => t("Ochtend", "Morning"),
                    Daylight::Midday => t("Middag", "Midday"),
                    Daylight::Evening => t("Avond", "Evening"),
                };
                if ui.selectable_label(form.daylight == d, name).clicked() {
                    form.daylight = d;
                }
            }
        });
    });
}

/// One course as a clickable tile with its mini profile.
fn route_tile(ui: &mut egui::Ui, form: &SetupForm, i: usize, width: f32) -> bool {
    let route = &form.routes[i];
    let info = &form.infos[i];
    let selected = form.route == i;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 86.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if selected {
        egui::Color32::from_rgba_premultiplied(70, 34, 10, 235)
    } else if resp.hovered() {
        egui::Color32::from_rgba_premultiplied(36, 42, 60, 235)
    } else {
        egui::Color32::from_rgba_premultiplied(24, 28, 42, 225)
    };
    p.rect(
        rect,
        10.0,
        fill,
        egui::Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected { ACCENT } else { theme::CARD_EDGE },
        ),
        egui::StrokeKind::Inside,
    );
    // Mini profile along the bottom.
    let (lo, hi) = info
        .profile
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &e| (a.min(e), b.max(e)));
    let span = (hi - lo).max(60.0);
    let n = info.profile.len() - 1;
    let area = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 34.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 6.0),
    );
    let len = route.length_m() as f32;
    let pts: Vec<egui::Pos2> = info
        .profile
        .iter()
        .enumerate()
        .map(|(k, &e)| {
            egui::pos2(
                area.left() + k as f32 / n as f32 * area.width(),
                area.bottom() - (e - lo) / span * area.height(),
            )
        })
        .collect();
    theme::profile_fill(p, &pts, area.bottom(), |k| {
        let grade = (info.profile[k + 1] - info.profile[k]) / (len / n as f32) * 100.0;
        theme::grade_color(grade).gamma_multiply(0.85)
    });
    let name = elide(&route.name, (width / 9.0) as usize);
    p.text(
        rect.left_top() + egui::vec2(10.0, 6.0),
        egui::Align2::LEFT_TOP,
        name,
        display(19.0),
        TEXT,
    );
    let mut line = format!(
        "{:.1} km · +{:.0} m",
        route.length_m() / 1000.0,
        route.gain_m()
    );
    if !info.hardest.is_empty() {
        line.push_str(&format!(" · {}", info.hardest));
    }
    p.text(
        rect.left_top() + egui::vec2(10.0, 29.0),
        egui::Align2::LEFT_TOP,
        line,
        egui::FontId::proportional(13.0),
        if selected { ACCENT_SOFT } else { MUTED },
    );
    ui.add_space(0.0);
    resp.clicked()
}

fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

/// The selected course's elevation profile, colored by gradient.
fn big_profile(ui: &mut egui::Ui, route: &Route) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 84.0), egui::Sense::hover());
    let len = route.length_m();
    let steps = 140;
    let elev: Vec<f32> = (0..=steps)
        .map(|k| route.elevation_at(len * k as f64 / steps as f64))
        .collect();
    let (lo, hi) = elev
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &e| (a.min(e), b.max(e)));
    let span = (hi - lo).max(40.0);
    let at = |k: usize, e: f32| {
        egui::pos2(
            rect.left() + k as f32 / steps as f32 * rect.width(),
            rect.bottom() - 4.0 - (e - lo) / span * (rect.height() - 18.0),
        )
    };
    let p = ui.painter();
    let pts: Vec<egui::Pos2> = (0..=steps).map(|k| at(k, elev[k])).collect();
    theme::profile_fill(p, &pts, rect.bottom(), |k| {
        theme::grade_color((elev[k + 1] - elev[k]) / (len as f32 / steps as f32) * 100.0)
    });
    for (text, y, align) in [
        (
            format!("{hi:.0} m"),
            rect.top() + 2.0,
            egui::Align2::LEFT_TOP,
        ),
        (
            format!("{lo:.0} m"),
            rect.bottom() - 2.0,
            egui::Align2::LEFT_BOTTOM,
        ),
    ] {
        p.text(
            egui::pos2(rect.left() + 4.0, y),
            align,
            text,
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE.gamma_multiply(0.8),
        );
    }
}

fn field_card(ui: &mut egui::Ui, form: &mut SetupForm) {
    card(ui, t("Deelnemersveld", "The field"), |ui| {
        let mut regen = false;
        let strength = match form.difficulty {
            d if d < 0.85 => t("Rustig", "Easy"),
            d if d < 0.97 => t("Gelijkwaardig", "Even"),
            d if d < 1.08 => t("Pittig", "Tough"),
            d if d < 1.2 => t("Zwaar", "Hard"),
            _ => "WorldTour",
        };
        ui.horizontal(|ui| {
            ui.label(t("Sterkte", "Strength"));
            regen |= ui
                .add(egui::Slider::new(&mut form.difficulty, 0.6..=1.4).show_value(false))
                .changed();
            ui.label(
                egui::RichText::new(format!("{strength} · {:.0}%", form.difficulty * 100.0))
                    .color(ACCENT_SOFT),
            );
        });
        ui.horizontal(|ui| {
            ui.label(t("Ploegen", "Teams"));
            regen |= ui
                .add(egui::DragValue::new(&mut form.team_count).range(3..=11))
                .changed();
            ui.add_space(10.0);
            ui.label(t("Renners per ploeg", "Riders per team"));
            ui.add(egui::DragValue::new(&mut form.riders_per_team).range(4..=8));
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(t("Kies je ploeg", "Pick your team"))
                .small()
                .color(MUTED),
        );
        let max_wkg = form.teams.iter().map(|t| t.mean_wkg).fold(0.1, f32::max);
        let mut pick = None;
        for (i, team) in form.teams.iter().enumerate() {
            let selected = form.player_team == i;
            let (rect, resp) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
            let p = ui.painter();
            if selected || resp.hovered() {
                p.rect_filled(
                    rect,
                    8.0,
                    if selected {
                        ACCENT.gamma_multiply(0.25)
                    } else {
                        egui::Color32::from_white_alpha(10)
                    },
                );
            }
            let [r, g, b] = team.color;
            let jersey = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 18.0, rect.center().y),
                egui::vec2(18.0, 20.0),
            );
            p.rect_filled(jersey, 4.0, egui::Color32::from_rgb(r, g, b));
            p.text(
                egui::pos2(rect.left() + 38.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &team.name,
                egui::FontId::proportional(15.0),
                if selected { egui::Color32::WHITE } else { TEXT },
            );
            // Strength bar.
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 150.0, rect.center().y - 4.0),
                egui::vec2(90.0, 8.0),
            );
            p.rect_filled(bar, 4.0, egui::Color32::from_white_alpha(18));
            let mut fill = bar;
            fill.set_width(bar.width() * (team.mean_wkg / max_wkg).clamp(0.05, 1.0));
            p.rect_filled(fill, 4.0, egui::Color32::from_rgb(r, g, b));
            p.text(
                egui::pos2(rect.right() - 6.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{:.2}", team.mean_wkg),
                display(17.0),
                MUTED,
            );
            if resp.clicked() {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            form.player_team = i;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(t("Opnieuw verdelen", "Reshuffle")).clicked() {
                regen = true;
            }
            ui.toggle_value(
                &mut form.show_team_editor,
                t("Ploegen aanpassen", "Edit teams"),
            );
        });
        if form.show_team_editor {
            egui::Grid::new("teams-edit")
                .num_columns(3)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    for t in form.teams.iter_mut() {
                        ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(140.0));
                        ui.add(
                            egui::DragValue::new(&mut t.mean_wkg)
                                .range(1.5..=7.0)
                                .speed(0.01)
                                .suffix(" W/kg"),
                        );
                        ui.add(
                            egui::DragValue::new(&mut t.std_wkg)
                                .range(0.0..=1.0)
                                .speed(0.01)
                                .prefix("± "),
                        );
                        ui.end_row();
                    }
                });
        }
        if regen {
            form.regenerate();
        }
    });
}

enum Start {
    Ride,
    /// Sit back and watch the bots race it.
    Watch,
}

/// Race summary, the big start button and one to watch the race instead
/// (no trainer needed). Returns the one pressed.
fn start_card(ui: &mut egui::Ui, form: &SetupForm, can_start: bool, t: f32) -> Option<Start> {
    let route = &form.routes[form.route];
    let laps = if route.looped { form.laps } else { 1 };
    let km = route.length_m() / 1000.0 * laps as f64;
    let riders = form.team_count * form.riders_per_team;
    card(ui, "", |ui| {
        ui.horizontal(|ui| {
            stat(ui, format!("{km:.1}"), "km", TEXT);
            ui.add_space(8.0);
            stat(
                ui,
                format!("{:.0}", route.gain_m() * laps as f32),
                i18n::t("hoogtemeters", "elevation m"),
                TEXT,
            );
            ui.add_space(8.0);
            stat(ui, riders.to_string(), i18n::t("renners", "riders"), TEXT);
        });
        ui.add_space(8.0);
        let ride = start_button_inner(ui, i18n::t("START WEDSTRIJD", "START RACE"), can_start, t);
        ui.add_space(6.0);
        let watch = ui
            .add_sized(
                [ui.available_width(), 32.0],
                egui::Button::new(
                    egui::RichText::new(i18n::t("▶  Bekijk deze koers", "▶  Watch this race"))
                        .size(17.0),
                ),
            )
            .on_hover_text(i18n::t(
                "Leun achterover: de bots rijden de koers, de camera volgt de actie",
                "Sit back: the bots race it and the camera follows the action",
            ))
            .clicked();
        if ride {
            Some(Start::Ride)
        } else if watch {
            Some(Start::Watch)
        } else {
            None
        }
    })
}

/// The big pulsing start button.
pub fn start_button(ui: &mut egui::Ui, text: &str, can_start: bool, t: f32) -> bool {
    start_button_inner(ui, text, can_start, t)
}

fn start_button_inner(ui: &mut egui::Ui, text: &str, can_start: bool, t: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 70.0),
        if can_start {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let p = ui.painter();
    let pulse = 0.5 + 0.5 * (t * 2.2).sin();
    let color = if can_start {
        ACCENT
    } else {
        egui::Color32::from_gray(70)
    };
    if can_start {
        for k in 1..6 {
            p.rect_filled(
                rect.expand(k as f32 * 3.0),
                14.0 + k as f32 * 3.0,
                ACCENT.gamma_multiply(0.05 * pulse * (6 - k) as f32 / 5.0),
            );
        }
    }
    let fill = if resp.hovered() {
        color.gamma_multiply(1.15)
    } else {
        color
    };
    p.rect_filled(rect, 14.0, fill);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        display(34.0),
        egui::Color32::WHITE,
    );
    if !can_start {
        ui.label(
            egui::RichText::new(i18n::t(
                "Wacht op je trainer, of zet de testmodus aan.",
                "Waiting for your trainer, or switch on test mode.",
            ))
            .color(MUTED),
        );
    }
    resp.clicked()
}

/// Race or train: two big tabs under the title.
fn mode_tabs(ui: &mut egui::Ui, form: &mut SetupForm) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (mode, title, sub) in [
            (
                PlayMode::Race,
                t("WEDSTRIJD", "RACE"),
                t("Koers tegen AI-ploegen", "Race against AI teams"),
            ),
            (
                PlayMode::Training,
                t("TRAINING", "WORKOUT"),
                t(
                    "Intervallen in ERG of als spel",
                    "Intervals in ERG or as a game",
                ),
            ),
            (
                PlayMode::Sprint,
                "SPRINTS",
                t("Sprinten tegen rivalen", "Sprint against rivals"),
            ),
        ] {
            let selected = form.mode == mode;
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(250.0, 54.0), egui::Sense::click());
            let p = ui.painter();
            let fill = if selected {
                ACCENT
            } else if resp.hovered() {
                egui::Color32::from_rgba_premultiplied(40, 46, 66, 235)
            } else {
                egui::Color32::from_rgba_premultiplied(20, 24, 36, 225)
            };
            p.rect(
                rect,
                14.0,
                fill,
                egui::Stroke::new(1.0, if selected { ACCENT } else { theme::CARD_EDGE }),
                egui::StrokeKind::Inside,
            );
            p.text(
                rect.left_center() + egui::vec2(18.0, -8.0),
                egui::Align2::LEFT_CENTER,
                title,
                display(24.0),
                egui::Color32::WHITE,
            );
            p.text(
                rect.left_center() + egui::vec2(18.0, 13.0),
                egui::Align2::LEFT_CENTER,
                sub,
                egui::FontId::proportional(13.0),
                if selected {
                    egui::Color32::from_rgb(255, 230, 210)
                } else {
                    MUTED
                },
            );
            if resp.clicked() {
                form.mode = mode;
            }
        }
    });
}

/// The workout library, filterable by what you want to improve.
fn library_card(ui: &mut egui::Ui, form: &mut SetupForm) {
    card(ui, t("Trainingen", "Workouts"), |ui| {
        if ui
            .add_sized(
                [ui.available_width(), 38.0],
                egui::Button::new(
                    egui::RichText::new(t("+  NIEUWE TRAINING BOUWEN", "+  BUILD A NEW WORKOUT"))
                        .font(display(19.0))
                        .color(egui::Color32::WHITE),
                )
                .fill(ACCENT.gamma_multiply(0.85)),
            )
            .clicked()
        {
            form.builder = Some(Builder::new(
                CustomWorkout::new(t("Mijn training", "My workout")),
                true,
            ));
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if !form.customs.is_empty()
                && ui
                    .selectable_label(form.mine_only, t("★ Eigen", "★ Mine"))
                    .clicked()
            {
                form.mine_only = !form.mine_only;
            }
            for (focus, label) in [
                (None, t("Alles", "All")),
                (Some(Focus::Vo2max), "VO2max"),
                (Some(Focus::Threshold), "FTP"),
                (Some(Focus::SweetSpot), "Sweet spot"),
                (Some(Focus::Endurance), t("Basis", "Base")),
                (Some(Focus::Test), "Test"),
            ] {
                if ui
                    .selectable_label(!form.mine_only && form.focus_filter == focus, label)
                    .clicked()
                {
                    form.focus_filter = focus;
                    form.mine_only = false;
                }
            }
        });
        ui.add_space(6.0);
        let mut pick = None;
        // Your own workouts first, newest on top.
        let n = form.plans.len();
        let mine = n - form.customs.len().min(n);
        let order = (mine..n).rev().chain(0..mine);
        for i in order {
            let plan = &form.plans[i];
            let own = i >= mine;
            if form.mine_only && !own {
                continue;
            }
            let shown = form.mine_only
                || match form.focus_filter {
                    None => true,
                    Some(Focus::Endurance) => {
                        matches!(plan.focus, Focus::Endurance | Focus::Recovery)
                    }
                    Some(Focus::Threshold) => {
                        matches!(plan.focus, Focus::Threshold | Focus::SweetSpot)
                    }
                    Some(f) => plan.focus == f,
                };
            if shown && crate::training::plan_tile(ui, plan, form.workout == i) {
                pick = Some(i);
            }
        }
        if let Some(i) = pick {
            form.workout = i;
        }
    });
}

/// The chosen workout, where to ride it, and the start button.
/// Room kept for the route choice and start button under the workout.
const TRAINING_START_H: f32 = 262.0;

/// Your streak, this week's training load and your FTP.
fn week_card(ui: &mut egui::Ui, form: &SetupForm) {
    let now = ridecore::fit::now_unix();
    let streak = form.profile.streak_days(now);
    let week = form.profile.week_tss(now);
    card(ui, t("Jouw week", "Your week"), |ui| {
        ui.horizontal(|ui| {
            stat(
                ui,
                streak.to_string(),
                t("dagen op rij", "day streak"),
                ACCENT,
            );
            ui.add_space(14.0);
            stat(
                ui,
                format!("{week:.0}"),
                t("tss deze week", "tss this week"),
                TEXT,
            );
            ui.add_space(14.0);
            stat(
                ui,
                form.profile.sessions.len().to_string(),
                t("trainingen", "workouts"),
                TEXT,
            );
        });
        match form.profile.sessions.last() {
            Some(last) => ui.label(
                egui::RichText::new(tf!(
                    "Laatst: {} · {}",
                    "Last: {} · {}",
                    last.workout,
                    ridecore::card::date(last.unix_time)
                ))
                .color(MUTED),
            ),
            None => ui.label(
                egui::RichText::new(t(
                    "Nog geen trainingen: begin met de Kennismaking of een ramptest.",
                    "No workouts yet: start with the First ride or a ramp test.",
                ))
                .color(MUTED),
            ),
        };
    });
}

/// The selected workout in full.
fn training_card(ui: &mut egui::Ui, form: &mut SetupForm) {
    let plan = form.plans[form.workout.min(form.plans.len() - 1)].clone();
    card(ui, t("Training", "Workout"), |ui| {
        crate::training::plan_detail(ui, &plan, form.ftp);
        ui.add_space(8.0);
        plan_actions(ui, form, &plan);
        if let Some(note) = &form.notice {
            ui.label(egui::RichText::new(note).small().color(ACCENT_SOFT));
        }
    });
}

/// Edit, copy, export or delete your own workout; or turn a library
/// workout into one of your own.
fn plan_actions(ui: &mut egui::Ui, form: &mut SetupForm, plan: &ridecore::training::Plan) {
    let ftp = form.ftp;
    let Some(k) = form.customs.iter().position(|c| c.id == plan.id) else {
        if !plan.ramp_test
            && ui
                .button(t("Aanpassen als eigen training", "Customise as your own"))
                .on_hover_text(t(
                    "Maakt een kopie die je in de bouwer kunt aanpassen.",
                    "Makes a copy you can change in the builder.",
                ))
                .clicked()
        {
            let mut w = CustomWorkout::from_segments(
                &tf!("{} (eigen)", "{} (mine)", plan.name),
                &plan.workout.segments,
            );
            w.notes = plan.summary.clone();
            form.builder = Some(Builder::new(w, true));
        }
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        if ui.button(t("Bewerken", "Edit")).clicked() {
            form.builder = Some(Builder::new(form.customs[k].clone(), false));
            form.notice = None;
        }
        if ui.button(t("Dupliceren", "Duplicate")).clicked() {
            let mut copy =
                CustomWorkout::new(&tf!("{} (kopie)", "{} (copy)", form.customs[k].name));
            copy.unit = form.customs[k].unit;
            copy.blocks = form.customs[k].blocks.clone();
            copy.notes = form.customs[k].notes.clone();
            form.notice = Some(t("Kopie gemaakt", "Copy made").into());
            form.save_custom(copy);
            return;
        }
        if ui
            .button(t("Exporteer .zwo", "Export .zwo"))
            .on_hover_text(t(
                "Een Zwift-bestand, ook te gebruiken in andere apps.",
                "A Zwift file, works in other apps too.",
            ))
            .clicked()
        {
            form.notice = Some(match custom::export(&form.customs[k], ftp) {
                Ok(path) => {
                    crate::share::reveal(&path);
                    tf!("Opgeslagen: {}", "Saved: {}", path.display())
                }
                Err(e) => tf!("Exporteren mislukt: {e}", "Export failed: {e}"),
            });
        }
        let id = form.customs[k].id.clone();
        let armed = form.confirm_delete.as_deref() == Some(id.as_str());
        let label = if armed {
            t("Zeker weten? Klik nog eens", "Sure? Click again")
        } else {
            t("Verwijderen", "Delete")
        };
        if ui
            .button(egui::RichText::new(label).color(if armed {
                egui::Color32::from_rgb(255, 110, 90)
            } else {
                MUTED
            }))
            .clicked()
        {
            if armed {
                form.confirm_delete = None;
                form.notice = None;
                form.delete_custom(&id);
            } else {
                form.confirm_delete = Some(id);
            }
        }
    });
}

/// A `.zwo` file dropped on the window becomes one of your workouts.
fn import_dropped(
    mut drops: MessageReader<bevy::window::FileDragAndDrop>,
    mut form: ResMut<SetupForm>,
) {
    for d in drops.read() {
        let bevy::window::FileDragAndDrop::DroppedFile { path_buf, .. } = d else {
            continue;
        };
        if !path_buf
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zwo"))
        {
            form.notice = Some(
                t(
                    "Alleen .zwo-bestanden kunnen worden geïmporteerd.",
                    "Only .zwo files can be imported.",
                )
                .into(),
            );
            continue;
        }
        let stem = path_buf
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let read = std::fs::read_to_string(path_buf)
            .map_err(anyhow::Error::from)
            .and_then(|xml| CustomWorkout::from_zwo(&xml, &stem));
        form.mode = PlayMode::Training;
        match read {
            Ok(w) => {
                form.notice = Some(tf!("Geïmporteerd: {}", "Imported: {}", w.name));
                form.save_custom(w);
            }
            Err(e) => {
                form.notice = Some(tf!("Importeren mislukt: {e:#}", "Import failed: {e:#}"));
            }
        }
    }
}

/// Where to ride it, and the start button. Returns true when pressed.
fn training_start(ui: &mut egui::Ui, form: &mut SetupForm, can_start: bool, t: f32) -> bool {
    card(ui, "", |ui| {
        // How to ride it: on a road in ERG, or as a game.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            if ui
                .selectable_label(form.game.is_none(), "Route (ERG)")
                .clicked()
            {
                form.game = None;
            }
            for kind in crate::games::GameKind::ALL {
                if ui
                    .selectable_label(form.game == Some(kind), kind.name())
                    .clicked()
                {
                    form.game = Some(kind);
                }
            }
        });
        if let Some(kind) = form.game {
            ui.label(egui::RichText::new(kind.blurb()).color(TEXT));
            ui.label(
                egui::RichText::new(i18n::t(
                    "Geen ERG: de trainer rijdt een vlakke weg en jij haalt het doel met \
                     je versnellingen. Click of +/− = schakelen.",
                    "No ERG: the trainer rides a flat road and you hit the target with \
                     your gears. Click or +/− = shift.",
                ))
                .small()
                .color(MUTED),
            );
            ui.add_space(4.0);
            return start_button_inner(ui, i18n::t("START SPEL", "START GAME"), can_start, t);
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(i18n::t("Omgeving", "Scenery")).color(MUTED));
            let current = form.routes[form.route].name.clone();
            egui::ComboBox::from_id_salt("training-route")
                .selected_text(current)
                .width(ui.available_width() - 8.0)
                .height(320.0)
                .show_ui(ui, |ui| {
                    for i in 0..form.routes.len() {
                        let label = format!(
                            "{} · {:.0} km",
                            form.routes[i].name,
                            form.routes[i].length_m() / 1000.0
                        );
                        ui.selectable_value(&mut form.route, i, label);
                    }
                });
        });
        ui.label(
            egui::RichText::new(i18n::t(
                "De trainer houdt het vermogen vast (ERG). Click of +/− = intensiteit.",
                "The trainer holds the power (ERG). Click or +/− = intensity.",
            ))
            .small()
            .color(MUTED),
        );
        ui.add_space(4.0);
        start_button_inner(ui, i18n::t("START TRAINING", "START WORKOUT"), can_start, t)
    })
}
