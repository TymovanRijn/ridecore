//! The far scenery: a ring of snow-capped mountains on the horizon, a wide
//! ground plane out to them, and slowly drifting clouds.

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

pub fn spawn(
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

    // Mountains well beyond the terrain, fading into the haze.
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

    // Clouds: a few puffs each, lit softly so they read as bright.
    let white = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 1.0, 1.0),
        emissive: LinearRgba::rgb(0.6, 0.62, 0.65),
        perceptual_roughness: 1.0,
        ..default()
    });
    for _ in 0..34 {
        let mut puff = Tris::default();
        for _ in 0..rng.range(3.0, 6.0) as usize {
            let o = Vec3::new(
                rng.range(-45.0, 45.0),
                rng.range(-6.0, 8.0),
                rng.range(-25.0, 25.0),
            );
            let s = rng.range(22.0, 42.0);
            props::blob(&mut puff, o, Vec3::new(s * 1.4, s * 0.55, s), [1.0; 4]);
        }
        let a = rng.range(0.0, TAU);
        let r = rng.range(0.0, radius + 600.0);
        let pos = Vec3::new(
            center.x + a.cos() * r,
            base_y + rng.range(240.0, 420.0),
            center.y + a.sin() * r,
        );
        commands.spawn((
            Cloud {
                speed: rng.range(2.0, 5.0),
                center,
                span: radius + 900.0,
            },
            Mesh3d(meshes.add(puff.mesh())),
            MeshMaterial3d(white.clone()),
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

/// Where the sun stands; the sun light shines the opposite way.
pub fn sun_direction() -> Vec3 {
    Vec3::new(0.45, 0.7, 0.35).normalize()
}

/// Sky color at horizon height, also used for the haze.
pub const HAZE: Color = Color::srgb(0.70, 0.79, 0.90);

/// The sky dome and sun travel with the camera.
#[derive(Component)]
pub struct SkyDome;

const DOME_R: f32 = 7000.0;

/// A gradient sky from deep blue overhead to pale haze at the horizon,
/// warmer around the sun, plus a glowing sun disk.
pub fn spawn_dome(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let sun = sun_direction();
    let zenith = LinearRgba::from(Color::srgb(0.22, 0.42, 0.80));
    let horizon = LinearRgba::from(HAZE);
    let glow = LinearRgba::rgb(1.0, 0.78, 0.5);
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
            c += glow * (s_dot.powi(6) * 0.25 + s_dot.powi(64) * 0.9);
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
    let disk = materials.add(StandardMaterial {
        // Far brighter than white, so the bloom gives it a halo.
        base_color: Color::linear_rgb(18.0, 15.0, 11.0),
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
