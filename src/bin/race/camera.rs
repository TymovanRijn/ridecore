//! The game camera: smoothed views around the player. A director picks the
//! shot by itself at logical moments (you can't reach the laptop while
//! riding); C switches manually for a while, V turns the director off.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::view::Msaa;

use ridecore::i18n::t;
use ridecore::race::Race;
use ridecore::race::ai::Intent;

use crate::riders::RiderModel;
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

    /// Scenes that cut in at once instead of waiting for the shot to end.
    fn urgent(self) -> bool {
        matches!(self, Self::Attack | Self::Finale | Self::Sprint)
    }
}

fn scene(race: &Race) -> Scene {
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
    if attack_near || me.power > 1.5 * me.profile.ftp {
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
            falloff: air(HAZY_M),
        },
    ));
}

/// How far you can see on most courses; real mountain ranges get clearer air.
pub const HAZY_M: f32 = 3800.0;
pub const CLEAR_M: f32 = 18_000.0;

/// Air: distant hills turn blue and fade into the haze, so the view
/// doesn't run on for ever.
pub fn air(visibility: f32) -> FogFalloff {
    FogFalloff::from_visibility_colors(
        visibility,
        Color::srgb(0.32, 0.45, 0.62),
        Color::srgb(0.78, 0.83, 0.9),
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
    let now = scene(&res.race);
    if now != rig.scene {
        // Calm scenes wait for the shot to run a few seconds; urgent ones cut.
        if now.urgent() || rig.shot_time > 4.0 {
            rig.scene = now;
            rig.shot = 0;
            rig.shot_time = 0.0;
            rig.view = now.shots()[0].0;
        }
        return;
    }
    let shots = rig.scene.shots();
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
    riders: Query<(&RiderModel, &Transform), Without<Camera3d>>,
    mut camera: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    let Some((_, player)) = riders.iter().find(|(m, _)| m.index == res.race.human) else {
        return;
    };
    let Ok((mut cam, mut projection)) = camera.single_mut() else {
        return;
    };
    // Results: a slow orbit around the player.
    let (offset, look_offset) = if *screen.get() == Screen::Results {
        let a = time.elapsed_secs() * 0.15;
        (
            Vec3::new(a.sin() * 8.0, 2.5, a.cos() * 8.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
    } else {
        rig.view.offsets()
    };
    // Follow heading but not pitch, so climbs don't tilt the horizon.
    let fwd = (player.rotation * Vec3::NEG_Z)
        .with_y(0.0)
        .normalize_or_zero();
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);
    let to_world = |o: Vec3| player.translation + right * o.x + Vec3::Y * o.y - fwd * o.z;
    let want_pos = to_world(offset);
    let want_look = to_world(look_offset);
    // Critically damped-ish smoothing, frame-rate independent.
    let k = 1.0 - (-time.delta_secs() * 8.0).exp();
    // A new shot is a cut, as on TV, not a long camera flight.
    if rig.placed_for != Some(rig.view) && *screen.get() == Screen::Racing {
        rig.placed_for = Some(rig.view);
        rig.initialized = false;
    }
    if !rig.initialized {
        rig.pos = want_pos;
        rig.look = want_look;
        rig.initialized = true;
    }
    let (pos, look) = (rig.pos.lerp(want_pos, k), rig.look.lerp(want_look, k * 1.4));
    rig.pos = pos;
    rig.look = look;
    *cam = Transform::from_translation(pos).looking_at(look, Vec3::Y);

    // Effort makes itself felt: the view widens with speed and when you go
    // deep, and the camera trembles slightly in an all-out effort.
    let me = res.race.player();
    let effort = if res.paused {
        0.0
    } else {
        ((me.power / me.profile.ftp - 1.3) / 0.7).clamp(0.0, 1.0)
    };
    let fast = ((me.speed * 3.6 - 30.0) / 40.0).clamp(0.0, 1.0);
    let t = time.elapsed_secs();
    cam.translation +=
        (right * (t * 41.0).sin() * 0.03 + Vec3::Y * (t * 53.0 + 1.0).sin() * 0.025) * effort;
    if let Projection::Perspective(p) = &mut *projection {
        let target = (52.0 + 8.0 * fast + 7.0 * effort).to_radians();
        p.fov += (target - p.fov) * (1.0 - (-time.delta_secs() * 3.0).exp());
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
