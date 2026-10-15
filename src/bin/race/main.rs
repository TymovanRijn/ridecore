//! RideCore race mode: a single-player 3D road race on your smart trainer
//! against AI teams.

mod camera;
mod captions;
mod climb;
mod feedback;
mod games;
mod hud;
mod link;
mod results;
mod riders;
mod setup;
mod share;
mod sound;
mod theme;
mod training;
mod world;

use bevy::prelude::*;
use bevy::window::PresentMode;
use bevy_egui::EguiPlugin;

use ridecore::fit::{self, Sample};
use ridecore::race::Race;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Screen {
    #[default]
    Setup,
    Racing,
    Results,
}

/// The race being ridden, plus what is recorded for the FIT file.
#[derive(Resource)]
pub struct RaceRes {
    pub race: Race,
    pub paused: bool,
    pub samples: Vec<Sample>,
    /// The saved ride (FIT + stats card), once you've finished.
    pub saved: Option<Result<share::SavedRide, String>>,
    /// Seconds you spent well sheltered in the wheels.
    pub sheltered_s: u32,
    /// Seconds since the player crossed the line (results show after a pause).
    pub since_finish: f32,
    /// Challenge score: XP, challenges completed, challenges attempted.
    pub xp: (u32, u32, u32),
}

impl RaceRes {
    pub fn new(race: Race) -> Self {
        Self {
            race,
            paused: false,
            samples: Vec::new(),
            saved: None,
            sheltered_s: 0,
            since_finish: 0.0,
            xp: (0, 0, 0),
        }
    }
}

/// Sky and fog share one color so the horizon blends in.
pub const SKY: Color = world::HAZE;

fn main() {
    // Before anything builds text: the language from the saved profile.
    let profile = ridecore::profile::Profile::load();
    ridecore::i18n::set_lang(ridecore::i18n::startup_lang(&profile.lang));
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "RideCore".into(),
                resolution: (1440u32, 900u32).into(),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .init_state::<Screen>()
        .insert_resource(ClearColor(SKY))
        .init_resource::<share::StravaUi>()
        .add_plugins((
            link::LinkPlugin,
            setup::SetupPlugin,
            world::WorldPlugin,
            riders::RidersPlugin,
            camera::CameraPlugin,
            hud::HudPlugin,
            results::ResultsPlugin,
            sound::SoundPlugin,
            feedback::FeedbackPlugin,
            climb::ClimbPlugin,
            theme::ThemePlugin,
            captions::CaptionsPlugin,
            training::TrainingPlugin,
        ))
        .add_systems(
            Update,
            advance_race.run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
        )
        .add_systems(Update, screenshots)
        .add_systems(OnExit(Screen::Racing), save_unfinished)
        // Not on the setup screen, where F is typed into text fields.
        .add_systems(
            Update,
            toggle_fullscreen.run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
        )
        .run();
}

/// Steps the simulation with the frame time and the player's power, records
/// one FIT sample per second, and moves to the results after the finish.
pub fn advance_race(
    time: Res<Time>,
    power: Res<link::PlayerInput>,
    mut res: ResMut<RaceRes>,
    screen: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    if res.paused {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let res = &mut *res;
    res.race.step(dt, power.power, power.cadence);

    let player = res.race.player();
    if res.race.started() && !player.finished() {
        let second = res.race.time.floor() as u64;
        if res.samples.len() as u64 <= second {
            res.samples.push(Sample {
                unix_time: fit::now_unix(),
                distance_m: player.dist.max(0.0),
                speed_mps: player.speed,
                power_w: player.power.round() as u16,
                cadence_rpm: player.cadence.round().min(254.0) as u8,
                heart_rate: power.heart_rate,
                altitude_m: res.race.course.elevation_at(player.dist.max(0.0)),
                position: res.race.course.route.latlon_at(player.dist.max(0.0)),
            });
            if player.draft > 0.25 {
                res.sheltered_s += 1;
            }
        }
    }

    if player.finished() {
        res.since_finish += dt;
        // A moment after the line, so the finish XP is counted.
        if res.saved.is_none() && res.samples.len() > 10 && res.since_finish > 0.5 {
            res.saved = Some(share::save_ride(res));
        }
        if *screen.get() == Screen::Racing && res.since_finish > 6.0 {
            next.set(Screen::Results);
        }
    }
}

/// A race left before the finish is still saved (FIT and card), so every
/// ride ends up in ~/Documents/RideCore. Workouts save themselves.
fn save_unfinished(mut res: Option<ResMut<RaceRes>>, session: Option<Res<training::Session>>) {
    let Some(res) = res.as_deref_mut() else {
        return;
    };
    if session.is_none()
        && res.saved.is_none()
        && res.samples.len() >= 60
        && std::env::var_os("RIDECORE_SHOTS").is_none()
    {
        res.saved = Some(share::save_ride(res));
    }
}

/// F toggles full screen (handy on a TV).
fn toggle_fullscreen(keys: Res<ButtonInput<KeyCode>>, mut windows: Query<&mut Window>) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    use bevy::window::{MonitorSelection, WindowMode};
    for mut w in &mut windows {
        w.mode = match w.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        };
    }
}

#[allow(clippy::too_many_arguments)]
/// F12 saves a screenshot to ~/Documents/RideCore/screenshots. For
/// development, `RIDECORE_SHOTS=<dir>` captures a series in different camera
/// views during an autostarted race and then quits.
fn screenshots(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    res: Option<Res<RaceRes>>,
    mut rig: ResMut<camera::CameraRig>,
    mut taken: Local<usize>,
    mut frames: Local<Vec<f32>>,
    offscreen: Option<Res<camera::Offscreen>>,
    mut exit: MessageWriter<AppExit>,
    time: Res<Time>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    if keys.just_pressed(KeyCode::F12) {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Documents/RideCore/screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("ridecore-{}.png", fit::now_unix()));
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    let Ok(dir) = std::env::var("RIDECORE_SHOTS") else {
        return;
    };
    let shot = || match &offscreen {
        Some(o) => Screenshot::image(o.0.clone()),
        None => Screenshot::primary_window(),
    };
    // Without a race: the setup screen, once it has settled.
    let Some(res) = res else {
        let t = time.elapsed_secs();
        if *taken == 0 && t > 4.0 {
            commands
                .spawn(shot())
                .observe(save_to_disk(format!("{dir}/setup.png")));
            *taken = 1;
        } else if *taken == 1 && t > 6.0 {
            exit.write(AppExit::Success);
        }
        return;
    };
    const PLAN: [(f32, camera::View); 6] = [
        (-8.0, camera::View::Chase),
        (4.0, camera::View::Chase),
        (8.0, camera::View::Close),
        (12.0, camera::View::Tv),
        (16.0, camera::View::Helicopter),
        (20.0, camera::View::LookBack),
    ];
    if let Some(&(at, view)) = PLAN.get(*taken) {
        if res.race.time >= at - 2.0 {
            rig.view = view;
        }
        if res.race.time >= at {
            let path = format!("{dir}/shot-{}.png", *taken);
            commands.spawn(shot()).observe(save_to_disk(path));
            *taken += 1;
        }
    } else if res.race.time > PLAN[PLAN.len() - 1].0 + 3.0 {
        report_frames(&frames);
        exit.write(AppExit::Success);
    }
    if res.race.started() {
        frames.push(time.delta_secs() * 1000.0);
    }
}

/// Frame times of a screenshot run, to keep an eye on smoothness.
fn report_frames(frames: &[f32]) {
    if frames.is_empty() {
        return;
    }
    let mut sorted = frames.to_vec();
    sorted.sort_by(f32::total_cmp);
    let avg = sorted.iter().sum::<f32>() / sorted.len() as f32;
    let p95 = sorted[sorted.len() * 95 / 100];
    let worst = sorted[sorted.len() - 1];
    eprintln!(
        "frames: {} · avg {avg:.1} ms ({:.0} fps) · p95 {p95:.1} ms · worst {worst:.1} ms",
        sorted.len(),
        1000.0 / avg
    );
}
