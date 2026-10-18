//! The game camera: smoothed views around the player (or, watching, the
//! rider followed). A director picks the shot by itself at logical moments
//! (you can't reach the laptop while riding); C switches manually for a
//! while, V turns the director off.
//!
//! The camera doesn't hang on the rider's every move: it follows a point on
//! the road at the rider's distance whose side-to-side position, height
//! and heading are eased, so shuffling in the bunch, kinks in the GPS
//! elevation and bends don't jolt the picture.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::view::Msaa;

use ridecore::i18n::t;
use ridecore::race::Race;
use ridecore::race::ai::Intent;

use crate::{RaceRes, SKY, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Chase,
    Close,
    Tv,
    Helicopter,
    LookBack,
}

impl View {
    fn next(self) -> Self {
        match self {
            Self::Chase => Self::Close,
            Self::Close => Self::Tv,
            Self::Tv => Self::Helicopter,
            Self::Helicopter => Self::LookBack,
            Self::LookBack => Self::Chase,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Chase => t("Achtervolg", "Chase"),
            Self::Close => t("Dichtbij", "Close"),
            Self::Tv => "TV",
            Self::Helicopter => t("Helikopter", "Helicopter"),
            Self::LookBack => t("Achterom", "Look back"),
        }
    }

    /// Camera offset and look-at offset in the rider's frame (right, up, back).
    fn offsets(self) -> (Vec3, Vec3) {
        match self {
            Self::Chase => (Vec3::new(0.0, 2.6, 7.0), Vec3::new(0.0, 1.0, -6.0)),
            Self::Close => (Vec3::new(0.6, 1.7, 3.2), Vec3::new(0.0, 1.1, -3.0)),
            Self::Tv => (Vec3::new(9.0, 2.2, -2.0), Vec3::new(0.0, 1.0, -1.0)),
            Self::Helicopter => (Vec3::new(-14.0, 22.0, 30.0), Vec3::new(0.0, 0.0, -18.0)),
            Self::LookBack => (Vec3::new(0.4, 2.0, -6.0), Vec3::new(0.0, 1.0, 8.0)),
        }
    }
}

#[derive(Resource)]
pub struct CameraRig {
    pub view: View,
    pos: Vec3,
    look: Vec3,
    initialized: bool,
    /// The director picks shots by itself.
    pub auto: bool,
    /// Seconds the director waits after a manual switch.
    manual_hold: f32,
    shot_time: f32,
    shot: usize,
    scene: Scene,
    /// View the camera was last placed for, to cut on a change.
    placed_for: Option<View>,
    /// The eased point followed: side-to-side position on the road, height
    /// and heading.
    lateral: f32,
    height: f32,
    heading: Vec3,
}

impl CameraRig {
    /// Jumps to the shot instead of flying there (a new rider to follow).
    pub fn cut(&mut self) {
        self.initialized = false;
    }
}

/// What's going on, for the director.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scene {
    Bunch,
    Climb,
    Descent,
    Break,
    Attack,
    Finale,
    Sprint,
}

impl Scene {
    /// Shots to cycle through, with how long each is held.
    fn shots(self) -> &'static [(View, f32)] {
        match self {
            Self::Bunch => &[
                (View::Chase, 25.0),
                (View::Tv, 8.0),
                (View::Chase, 20.0),
                (View::Helicopter, 9.0),
                (View::Chase, 20.0),
                (View::Close, 8.0),
            ],
            Self::Climb => &[
                (View::Chase, 18.0),
                (View::Helicopter, 10.0),
                (View::Close, 9.0),
                (View::Chase, 15.0),
                (View::Tv, 8.0),
            ],
            Self::Descent => &[(View::Close, 10.0), (View::Chase, 12.0), (View::Tv, 6.0)],
            Self::Break => &[
                (View::Chase, 18.0),
                (View::LookBack, 7.0),
                (View::Tv, 8.0),
                (View::Helicopter, 8.0),
            ],
            Self::Attack => &[(View::Close, 7.0), (View::LookBack, 5.0)],
            Self::Finale => &[(View::Chase, 1e9)],
            Self::Sprint => &[(View::Tv, 1e9)],
        }
    }

    /// Watching: long, calm shots, mostly from behind.
    fn watch_shots(self) -> &'static [(View, f32)] {
        match self {
            Self::Sprint => &[(View::Tv, 1e9)],
            Self::Finale => &[(View::Chase, 1e9)],
            Self::Attack => &[(View::Chase, 20.0), (View::Tv, 12.0)],
            Self::Climb => &[
                (View::Chase, 45.0),
                (View::Helicopter, 20.0),
                (View::Tv, 15.0),
            ],
            _ => &[
                (View::Chase, 50.0),
                (View::Helicopter, 18.0),
                (View::Chase, 40.0),
                (View::Tv, 15.0),
            ],
        }
    }

    /// Scenes that cut in at once instead of waiting for the shot to end.
    fn urgent(self) -> bool {
        matches!(self, Self::Attack | Self::Finale | Self::Sprint)
    }
}

fn scene(race: &Race, watching: bool) -> Scene {
    let me = race.player();
    let d = me.dist.max(0.0);
    let left = race.remaining_m(race.human);
    if left < 350.0 {
        return Scene::Sprint;
    }
    if left < 1500.0 {
        return Scene::Finale;
    }
    let attack_near = race.riders.iter().any(|r| {
        !r.human && matches!(r.intent, Intent::Attack { .. }) && (r.dist - me.dist).abs() < 40.0
    });
    // Watching, a bot's every surge isn't an attack worth a cut.
    let attacking = matches!(me.intent, Intent::Attack { .. });
    if !watching && (attack_near || me.power > 1.5 * me.profile.ftp) || watching && attacking {
        return Scene::Attack;
    }
    let g = race.group_of(race.human);
    let peloton = race.peloton();
    if g < peloton && race.groups[peloton].gap_s - race.groups[g].gap_s > 8.0 {
        return Scene::Break;
    }
    let grade = race.course.grade_ahead_pct(d, 200.0);
    if race.course.climb_near(d, 0.0).is_some() && grade > 3.0 {
        return Scene::Climb;
    }
    if grade < -4.0 {
        return Scene::Descent;
    }
    Scene::Bunch
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CameraRig {
            view: View::Chase,
            pos: Vec3::ZERO,
            look: Vec3::ZERO,
            initialized: false,
            auto: true,
            manual_hold: 0.0,
            shot_time: 0.0,
            shot: 0,
            scene: Scene::Bunch,
            placed_for: None,
            lateral: 0.0,
            height: 0.0,
            heading: Vec3::NEG_Z,
        })
        .add_systems(Startup, spawn_camera)
        .add_systems(OnEnter(Screen::Racing), |mut rig: ResMut<CameraRig>| {
            rig.initialized = false
        })
        .add_systems(
            Update,
            (
                switch_view,
                direct.run_if(in_state(Screen::Racing)),
                follow_player,
            )
                .chain()
                .after(crate::riders::RidersPlaced)
                .run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
        );
    }
}

/// Test runs with the screen locked (macOS draws no windows then) render
/// into this image instead: `RIDECORE_OFFSCREEN=1`.
#[derive(Resource)]
pub struct Offscreen(pub Handle<Image>);

fn spawn_camera(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    use bevy::camera::Hdr;
    use bevy::render::view::{ColorGrading, ColorGradingGlobal};
    let target = offscreen_target(&mut commands, &mut images);
    commands.spawn((
        Camera3d::default(),
        Msaa::Sample4,
        // HDR with a filmic curve and a touch more color, like a broadcast.
        // (No bloom: on Apple M1 it turns the whole frame black.)
        Hdr,
        ColorGrading {
            global: ColorGradingGlobal {
                post_saturation: 1.12,
                ..default()
            },
            ..default()
        },
        // Far enough to see the mountains on the horizon.
        Projection::Perspective(PerspectiveProjection {
            far: 60_000.0,
            ..default()
        }),
        Transform::from_xyz(0.0, 30.0, 60.0).looking_at(Vec3::ZERO, Vec3::Y),
        target,
        DistanceFog {
            color: SKY,
            directional_light_color: Color::srgba(1.0, 0.9, 0.7, 0.4),
            directional_light_exponent: 20.0,
            falloff: air(HAZY_M, &crate::world::Daylight::Midday.look()),
        },
    ));
}

/// How far you can see on most courses; real mountain ranges get clearer air.
pub const HAZY_M: f32 = 3800.0;
pub const CLEAR_M: f32 = 18_000.0;

/// Air: distant hills turn blue and fade into the haze, so the view
/// doesn't run on for ever. Mist and evening haze shorten the view.
pub fn air(visibility: f32, look: &crate::world::sky::Look) -> FogFalloff {
    FogFalloff::from_visibility_colors(
        visibility * look.clarity,
        look.extinction,
        look.inscattering,
    )
}

/// After a manual switch, the director leaves the camera alone this long.
const MANUAL_HOLD_S: f32 = 45.0;

fn switch_view(keys: Res<ButtonInput<KeyCode>>, mut rig: ResMut<CameraRig>) {
    if keys.just_pressed(KeyCode::KeyC) {
        rig.view = rig.view.next();
        rig.manual_hold = MANUAL_HOLD_S;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        rig.auto = !rig.auto;
        rig.manual_hold = 0.0;
    }
}

/// The director: holds each shot for its time and moves on to the next one
/// that fits the scene; urgent scenes (attack, finale, sprint) cut in at once.
fn direct(time: Res<Time>, res: Res<RaceRes>, mut rig: ResMut<CameraRig>) {
    if !rig.auto || res.paused || !res.race.started() {
        return;
    }
    let dt = time.delta_secs();
    if rig.manual_hold > 0.0 {
        rig.manual_hold -= dt;
        return;
    }
    rig.shot_time += dt;
    let now = scene(&res.race, res.watch.is_some());
    if now != rig.scene {
        // Calm scenes wait for the shot to run a few seconds; urgent ones cut.
        if now.urgent() || rig.shot_time > 4.0 {
            rig.scene = now;
            rig.shot = 0;
            rig.shot_time = 0.0;
            rig.view = if res.watch.is_some() {
                now.watch_shots()[0].0
            } else {
                now.shots()[0].0
            };
        }
        return;
    }
    let shots = if res.watch.is_some() {
        rig.scene.watch_shots()
    } else {
        rig.scene.shots()
    };
    let (_, hold) = shots[rig.shot % shots.len()];
    if rig.shot_time > hold {
        rig.shot = (rig.shot + 1) % shots.len();
        rig.shot_time = 0.0;
        rig.view = shots[rig.shot].0;
    }
}

fn follow_player(
    time: Res<Time>,
    res: Res<RaceRes>,
    screen: Res<State<Screen>>,
    mut rig: ResMut<CameraRig>,
    mut camera: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    let Ok((mut cam, mut projection)) = camera.single_mut() else {
        return;
    };
    let race = &res.race;
    let me = race.player();
    let dt = time.delta_secs();
    let ease = |rate: f32| 1.0 - (-dt * rate).exp();
    // A new shot or a new rider is a cut, as on TV, not a long camera flight.
    if rig.placed_for != Some(rig.view) && *screen.get() == Screen::Racing {
        rig.placed_for = Some(rig.view);
        rig.initialized = false;
    }

    // The point followed: on the road at the rider's distance, with eased
    // side-to-side position and height, heading along the road around it.
    let at = |d: f64, lateral: f32| crate::riders::rider_transform(race, d, lateral).translation;
    let height = (at(me.dist - 10.0, 0.0).y + at(me.dist, 0.0).y + at(me.dist + 10.0, 0.0).y) / 3.0;
    let along = (at(me.dist + 8.0, 0.0) - at(me.dist - 8.0, 0.0))
        .with_y(0.0)
        .normalize_or(rig.heading);
    if !rig.initialized {
        rig.lateral = me.lateral;
        rig.height = height;
        rig.heading = along;
    }
    rig.lateral += (me.lateral - rig.lateral) * ease(1.2);
    rig.height += (height - rig.height) * ease(2.0);
    rig.heading = rig.heading.lerp(along, ease(2.5)).normalize_or(along);
    let anchor = at(me.dist, rig.lateral).with_y(rig.height);
    let fwd = rig.heading;
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);

    // Results: a slow orbit around the rider.
    let (offset, look_offset) = if *screen.get() == Screen::Results {
        let a = time.elapsed_secs() * 0.15;
        (
            Vec3::new(a.sin() * 8.0, 2.5, a.cos() * 8.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
    } else {
        rig.view.offsets()
    };
    let to_frame = |o: Vec3| right * o.x + Vec3::Y * o.y - fwd * o.z;
    // A slow drift, like a camera on a motorbike, so the shot isn't welded on.
    let t = time.elapsed_secs();
    let drift = right * (t * 0.31).sin() * 0.18 + Vec3::Y * (t * 0.23 + 1.0).sin() * 0.1;
    let want_pos = to_frame(offset) + drift;
    let want_look = to_frame(look_offset);
    // The offsets are eased relative to the followed point, so the camera
    // keeps up with the rider at any speed and only bends and view changes
    // are smoothed.
    if !rig.initialized {
        rig.pos = want_pos;
        rig.look = want_look;
        rig.initialized = true;
    }
    rig.pos = rig.pos.lerp(want_pos, ease(2.2));
    rig.look = rig.look.lerp(want_look, ease(3.0));
    *cam = Transform::from_translation(anchor + rig.pos).looking_at(anchor + rig.look, Vec3::Y);

    // Riding, effort makes itself felt: the view widens with speed and when
    // you go deep, and trembles slightly in an all-out effort. Watching,
    // the picture stays calm.
    let effort = if res.paused || res.watch.is_some() {
        0.0
    } else {
        ((me.power / me.profile.ftp - 1.3) / 0.7).clamp(0.0, 1.0)
    };
    let fast = ((me.speed * 3.6 - 30.0) / 40.0).clamp(0.0, 1.0);
    cam.translation +=
        (right * (t * 41.0).sin() * 0.03 + Vec3::Y * (t * 53.0 + 1.0).sin() * 0.025) * effort;
    if let Projection::Perspective(p) = &mut *projection {
        let target = (52.0 + 8.0 * fast + 7.0 * effort).to_radians();
        p.fov += (target - p.fov) * ease(1.5);
    }
}

fn offscreen_target(commands: &mut Commands, images: &mut Assets<Image>) -> RenderTarget {
    use bevy::render::render_resource::{TextureFormat, TextureUsages};
    if std::env::var_os("RIDECORE_OFFSCREEN").is_none() {
        return RenderTarget::default();
    }
    let mut image = Image::new_target_texture(1600, 900, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.insert_resource(Offscreen(handle.clone()));
    RenderTarget::Image(handle.into())
}
