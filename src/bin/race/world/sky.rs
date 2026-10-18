//! The sky and the light: the time of day (sun, sky colors, haze), the
//! far scenery (a ring of snow-capped mountains on the horizon unless the
//! route has real ones, and a ground plane out to them) and slowly drifting
//! clouds.

use std::f32::consts::TAU;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use ridecore::rng::Rng;

use super::props::{self, rgb};
use super::{Tris, WorldPart};

/// Clouds drift with the wind and wrap around the map.
#[derive(Component)]
pub struct Cloud {
    speed: f32,
    center: Vec2,
    span: f32,
}

/// Made-up mountains well beyond the terrain, fading into the haze, and a
/// ground plane out to them. Routes with real mountains on the horizon
/// don't need these.
pub fn mountains(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    center: Vec2,
    radius: f32,
    base_y: f32,
    rng: &mut Rng,
) {
    let colored = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        ..default()
    });
    let mut far = Tris::default();
    let ring = radius + 1400.0;
    for k in 0..64 {
        let a = k as f32 / 64.0 * TAU + rng.range(-0.04, 0.04);
        let r = ring + rng.range(0.0, 1300.0);
        let base = Vec3::new(
            center.x + a.cos() * r,
            base_y - 40.0,
            center.y + a.sin() * r,
        );
        let height = rng.range(380.0, 1100.0);
        let width = height * rng.range(1.1, 1.7);
        let rock = rng.range(0.38, 0.5);
        props::cone(
            &mut far,
            base,
            width,
            height,
            6,
            rgb(rock * 0.85, rock * 0.92, rock),
        );
        // Snow on the higher peaks.
        if height > 650.0 {
            let cap = 0.32;
            props::cone(
                &mut far,
                base + Vec3::Y * height * (1.0 - cap) + Vec3::Y * 0.5,
                width * cap * 1.02,
                height * cap,
                6,
                rgb(0.96, 0.97, 1.0),
            );
        }
    }
    // Ground from the terrain's edge out to the mountains.
    let outer = ring + 2600.0;
    // Below the terrain's lowest ground so it never shows through valleys
    // or lakes; it only fills the view beyond the terrain's edge.
    let y = base_y - 25.0;
    let inner = radius * 0.7;
    let plain = rgb(0.24, 0.38, 0.2);
    for k in 0..48 {
        let a0 = k as f32 / 48.0 * TAU;
        let a1 = (k + 1) as f32 / 48.0 * TAU;
        let p = |a: f32, r: f32| Vec3::new(center.x + a.cos() * r, y, center.y + a.sin() * r);
        far.quad(
            p(a0, inner),
            p(a0, outer),
            p(a1, outer),
            p(a1, inner),
            plain,
        );
    }
    commands.spawn((
        Mesh3d(meshes.add(far.mesh())),
        MeshMaterial3d(colored.clone()),
        Transform::default(),
        WorldPart,
        NotShadowCaster,
    ));
}

/// The material clouds share: white, glowing a little in the day's light.
pub fn cloud_material(
    materials: &mut Assets<StandardMaterial>,
    look: &Look,
) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::WHITE,
        emissive: look.cloud_glow,
        perceptual_roughness: 1.0,
        ..default()
    })
}

/// Slowly drifting cumulus over the course: heaps of rounded puffs on a
/// flat base, bright on top and grey underneath.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: Handle<StandardMaterial>,
    center: Vec2,
    radius: f32,
    base_y: f32,
    rng: &mut Rng,
) {
    for _ in 0..40 {
        let mut cloud = Tris::default();
        let size = rng.range(0.7, 1.4);
        let whole = Vec3::new(70.0, 30.0, 45.0) * size;
        let middle = Vec3::Y * whole.y * 0.5;
        for k in 0..rng.range(5.0, 9.0) as usize {
            // Puffs heap up towards the middle; the biggest ones first.
            let x = rng.range(-1.0, 1.0);
            let o = Vec3::new(
                x * whole.x * 0.65,
                rng.range(0.15, 0.6) * whole.y * (1.0 - x.abs() * 0.6),
                rng.range(-0.5, 0.5) * whole.z,
            );
            let r = rng.range(0.45, 0.65) * (1.0 - k as f32 * 0.05) * whole.y;
            super::trees::lump(
                &mut cloud,
                o,
                Vec3::new(r * 1.3, r, r * 1.15),
                middle,
                whole,
                8,
                5,
                [1.0; 4],
                rng,
            );
        }
        let a = rng.range(0.0, TAU);
        let r = rng.range(0.0, radius + 900.0);
        let pos = Vec3::new(
            center.x + a.cos() * r,
            base_y + rng.range(320.0, 650.0),
            center.y + a.sin() * r,
        );
        commands.spawn((
            Cloud {
                speed: rng.range(2.0, 5.0),
                center,
                span: radius + 1200.0,
            },
            Mesh3d(meshes.add(cloud.mesh())),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(pos),
            WorldPart,
            NotShadowCaster,
        ));
    }
}

pub fn drift_clouds(time: Res<Time>, mut clouds: Query<(&Cloud, &mut Transform)>) {
    for (cloud, mut t) in &mut clouds {
        t.translation.x += cloud.speed * time.delta_secs();
        if t.translation.x > cloud.center.x + cloud.span {
            t.translation.x = cloud.center.x - cloud.span;
        }
    }
}

/// The time of day you ride at.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Daylight {
    /// Low sun, cool air and morning mist.
    Morning,
    #[default]
    Midday,
    /// Golden hour: a low, orange sun and long shadows.
    Evening,
}

/// Everything about the light that changes with the time of day.
pub struct Look {
    /// Where the sun stands; its light shines the opposite way.
    pub sun: Vec3,
    pub sun_color: Color,
    pub lux: f32,
    pub ambient: Color,
    pub ambient_brightness: f32,
    zenith: Color,
    /// Sky color at the horizon, also used for the haze.
    pub horizon: Color,
    /// Added around the sun: (color, how far it spreads over the sky).
    glow: (LinearRgba, f32),
    /// Haze colors: what distant land fades to, and the light it scatters.
    pub extinction: Color,
    pub inscattering: Color,
    /// Sunlit haze when looking towards the sun.
    pub fog_sun: Color,
    /// How clear the air is, as a share of a clear midday.
    pub clarity: f32,
    pub cloud_glow: LinearRgba,
}

impl Daylight {
    pub const ALL: [Daylight; 3] = [Daylight::Morning, Daylight::Midday, Daylight::Evening];

    /// `RIDECORE_DAYLIGHT=morning|midday|evening`, for test runs.
    pub fn from_env() -> Option<Self> {
        match std::env::var("RIDECORE_DAYLIGHT").ok()?.as_str() {
            "morning" => Some(Daylight::Morning),
            "midday" => Some(Daylight::Midday),
            "evening" => Some(Daylight::Evening),
            _ => None,
        }
    }

    pub fn look(self) -> Look {
        // Sun at an elevation and an azimuth, in radians.
        let sun = |el: f32, az: f32| Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
        match self {
            Daylight::Morning => Look {
                sun: sun(0.45, -0.7),
                sun_color: Color::srgb(1.0, 0.91, 0.77),
                lux: 9_000.0,
                ambient: Color::srgb(0.74, 0.82, 1.0),
                ambient_brightness: 520.0,
                zenith: Color::srgb(0.3, 0.5, 0.84),
                horizon: Color::srgb(0.83, 0.85, 0.89),
                glow: (LinearRgba::rgb(1.0, 0.7, 0.4), 0.18),
                extinction: Color::srgb(0.45, 0.52, 0.64),
                inscattering: Color::srgb(0.87, 0.87, 0.88),
                fog_sun: Color::srgba(1.0, 0.85, 0.6, 0.6),
                clarity: 0.6,
                cloud_glow: LinearRgba::rgb(0.55, 0.5, 0.47),
            },
            Daylight::Midday => Look {
                sun: Vec3::new(0.45, 0.7, 0.35).normalize(),
                sun_color: Color::srgb(1.0, 0.96, 0.88),
                lux: 9_000.0,
                ambient: Color::srgb(0.75, 0.85, 1.0),
                ambient_brightness: 450.0,
                zenith: Color::srgb(0.22, 0.42, 0.80),
                horizon: HAZE,
                glow: (LinearRgba::rgb(1.0, 0.78, 0.5), 0.0),
                extinction: Color::srgb(0.32, 0.45, 0.62),
                inscattering: Color::srgb(0.78, 0.83, 0.9),
                fog_sun: Color::srgba(1.0, 0.9, 0.7, 0.4),
                clarity: 1.0,
                cloud_glow: LinearRgba::rgb(0.5, 0.52, 0.55),
            },
            Daylight::Evening => Look {
                sun: sun(0.24, 2.3),
                sun_color: Color::srgb(1.0, 0.78, 0.55),
                lux: 7_500.0,
                ambient: Color::srgb(0.7, 0.7, 0.9),
                ambient_brightness: 400.0,
                zenith: Color::srgb(0.2, 0.33, 0.62),
                horizon: Color::srgb(0.9, 0.79, 0.7),
                glow: (LinearRgba::rgb(1.0, 0.55, 0.25), 0.35),
                extinction: Color::srgb(0.38, 0.4, 0.55),
                inscattering: Color::srgb(0.9, 0.8, 0.72),
                fog_sun: Color::srgba(1.0, 0.65, 0.35, 0.7),
                clarity: 0.85,
                cloud_glow: LinearRgba::rgb(0.6, 0.42, 0.34),
            },
        }
    }
}

/// Sky color at horizon height on a clear midday, also the haze before a
/// race starts.
pub const HAZE: Color = Color::srgb(0.70, 0.79, 0.90);

/// The sky dome and sun travel with the camera.
#[derive(Component)]
pub struct SkyDome;

/// Beyond the farthest mountains.
const DOME_R: f32 = 45_000.0;

/// The midday sky until a race picks its time of day.
pub fn spawn_dome(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    dome(
        &mut commands,
        &mut meshes,
        &mut materials,
        &Daylight::Midday.look(),
    );
}

/// Sets the race's time of day: sun, ambient light, haze and sky.
#[allow(clippy::too_many_arguments)]
pub fn apply_daylight(
    daylight: Res<Daylight>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform)>,
    mut fog: Query<&mut DistanceFog>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    domes: Query<Entity, With<SkyDome>>,
) {
    let look = daylight.look();
    for (mut light, mut t) in &mut suns {
        light.color = look.sun_color;
        light.illuminance = look.lux;
        *t = Transform::default().looking_at(-look.sun, Vec3::Y);
    }
    for mut f in &mut fog {
        f.color = look.horizon;
        f.directional_light_color = look.fog_sun;
    }
    clear.0 = look.horizon;
    ambient.color = look.ambient;
    ambient.brightness = look.ambient_brightness;
    for d in &domes {
        commands.entity(d).despawn();
    }
    dome(&mut commands, &mut meshes, &mut materials, &look);
}

/// A gradient sky from deep blue overhead to pale haze at the horizon,
/// warmer around the sun, plus a glowing sun disk.
fn dome(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    look: &Look,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let sun = look.sun;
    let zenith = LinearRgba::from(look.zenith);
    let horizon = LinearRgba::from(look.horizon);
    let (glow, spread) = look.glow;
    let (rings, segs) = (48, 96);
    let mut pos = Vec::new();
    let mut col = Vec::new();
    for r in 0..=rings {
        // From straight down to straight up.
        let el = -0.3 + (r as f32 / rings as f32) * (std::f32::consts::FRAC_PI_2 + 0.3);
        for s in 0..=segs {
            let az = s as f32 / segs as f32 * TAU;
            let dir = Vec3::new(el.cos() * az.cos(), el.sin(), el.cos() * az.sin());
            let up = dir.y.max(0.0);
            let t = up.powf(0.45);
            let mut c = horizon * (1.0 - t) + zenith * t;
            let s_dot = dir.dot(sun).max(0.0);
            // A low sun warms a wide band of the sky near the horizon.
            let band = s_dot.powi(2) * (1.0 - t) * spread;
            c += glow * (band + s_dot.powi(6) * 0.25 + s_dot.powi(64) * 0.9);
            pos.push((dir * DOME_R).to_array());
            col.push(c.to_f32_array());
        }
    }
    let mut idx = Vec::new();
    let w = segs + 1;
    for r in 0..rings {
        for s in 0..segs {
            let (a, b) = (r * w + s, r * w + s + 1);
            let (c, d) = (a + w, b + w);
            idx.extend([a as u32, c as u32, b as u32, b as u32, c as u32, d as u32]);
        }
    }
    let dome = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
    .with_inserted_indices(Indices::U32(idx));
    let sky = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        fog_enabled: false,
        cull_mode: None,
        ..default()
    });
    let disk_color = LinearRgba::from(look.sun_color) * 16.0;
    let disk = materials.add(StandardMaterial {
        // Far brighter than white, so it glows through the haze.
        base_color: disk_color.into(),
        unlit: true,
        fog_enabled: false,
        ..default()
    });
    commands
        .spawn((
            SkyDome,
            Transform::default(),
            Visibility::default(),
            NotShadowCaster,
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(meshes.add(dome)),
                MeshMaterial3d(sky),
                NotShadowCaster,
            ));
            p.spawn((
                Mesh3d(meshes.add(Sphere::new(DOME_R * 0.012))),
                MeshMaterial3d(disk),
                Transform::from_translation(sun * DOME_R * 0.95),
                NotShadowCaster,
            ));
        });
}

pub fn follow_camera(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut dome: Query<&mut Transform, With<SkyDome>>,
) {
    let (Ok(cam), Ok(mut t)) = (camera.single(), dome.single_mut()) else {
        return;
    };
    t.translation = cam.translation();
}
