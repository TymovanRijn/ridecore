//! Rider models built from primitives, placed on the road every frame from
//! the simulation, with pedaling legs solved by two-bone IK, spinning wheels
//! and an out-of-the-saddle sprint.
//!
//! Model space: forward is −Z, up is +Y, right is +X.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::prelude::*;

use ridecore::race::ai::Intent;
use ridecore::race::course::SegmentKind;
use ridecore::race::field::RiderKind;
use ridecore::race::{Race, Rider};

use crate::{RaceRes, Screen};

const WHEEL_R: f32 = 0.34;
const REAR_AXLE: Vec3 = Vec3::new(0.0, WHEEL_R, 0.5);
const FRONT_AXLE: Vec3 = Vec3::new(0.0, WHEEL_R, -0.5);
const BB: Vec3 = Vec3::new(0.0, 0.30, 0.05);
const SADDLE: Vec3 = Vec3::new(0.0, 0.96, 0.24);
const BAR: Vec3 = Vec3::new(0.0, 1.0, -0.43);
const HEAD_TUBE: Vec3 = Vec3::new(0.0, 0.88, -0.38);
const CRANK: f32 = 0.17;
const THIGH: f32 = 0.45;
const SHIN: f32 = 0.45;
const HIP_X: f32 = 0.11;
/// How far the hips rise and move forward when standing on the pedals.
const STAND_OFFSET: Vec3 = Vec3::new(0.0, 0.14, -0.17);
/// Riders sit this far above the simulated road surface.
const ROAD_LIFT: f32 = 0.12;

#[derive(Component)]
pub struct RiderModel {
    pub index: usize,
    crank: f32,
    wheel: f32,
    stand: f32,
    /// Out of the saddle because of a hard effort (kept with hysteresis).
    surging: bool,
}

#[derive(Component)]
struct WheelSpin;

#[derive(Component)]
struct CrankSpin;

#[derive(Component)]
struct Body;

#[derive(Component)]
struct PlayerMarker;

/// Torso and helmet remember the team colors so leader's jerseys can be
/// put on and taken off again.
#[derive(Component)]
struct Torso(Handle<StandardMaterial>);

#[derive(Component)]
struct Helmet(Handle<StandardMaterial>);

#[derive(Component, Clone, Copy)]
enum LegPart {
    Thigh(f32),
    Shin(f32),
    Foot(f32),
}

pub struct RidersPlugin;

/// Rider transforms are final for this frame after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RidersPlaced;

impl Plugin for RidersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Racing), spawn_riders)
            .add_systems(OnEnter(Screen::Setup), despawn_riders)
            .add_systems(
                Update,
                (place_riders, animate_riders, bob_marker, dress_leaders)
                    .chain()
                    .after(crate::advance_race)
                    .in_set(RidersPlaced)
                    .run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
            );
    }
}

struct Palette {
    skin: Handle<StandardMaterial>,
    shorts: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
    tire: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
}

struct Shapes {
    tire: Handle<Mesh>,
    spoke: Handle<Mesh>,
    tube: Handle<Mesh>,
    torso: Handle<Mesh>,
    head: Handle<Mesh>,
    helmet: Handle<Mesh>,
    arm: Handle<Mesh>,
    thigh: Handle<Mesh>,
    shin: Handle<Mesh>,
    foot: Handle<Mesh>,
    crank: Handle<Mesh>,
    marker: Handle<Mesh>,
}

fn despawn_riders(mut commands: Commands, riders: Query<Entity, With<RiderModel>>) {
    for e in &riders {
        commands.entity(e).despawn();
    }
}

fn spawn_riders(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    res: Res<RaceRes>,
) {
    let mat = |m: &mut Assets<StandardMaterial>, c: Color, rough: f32| {
        m.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: rough,
            ..default()
        })
    };
    let pal = Palette {
        skin: mat(&mut materials, Color::srgb(0.87, 0.68, 0.55), 0.8),
        shorts: mat(&mut materials, Color::srgb(0.06, 0.06, 0.07), 0.6),
        white: mat(&mut materials, Color::srgb(0.95, 0.95, 0.95), 0.6),
        tire: mat(&mut materials, Color::srgb(0.05, 0.05, 0.05), 0.9),
        metal: materials.add(StandardMaterial {
            base_color: Color::srgb(0.75, 0.75, 0.78),
            metallic: 0.8,
            perceptual_roughness: 0.35,
            ..default()
        }),
    };
    let shapes = Shapes {
        tire: meshes.add(Torus::new(WHEEL_R - 0.035, WHEEL_R)),
        spoke: meshes.add(Cuboid::new(0.012, WHEEL_R * 2.0 - 0.07, 0.012)),
        tube: meshes.add(Cylinder::new(0.022, 1.0)),
        torso: meshes.add(Capsule3d::new(0.125, 0.36)),
        head: meshes.add(Sphere::new(0.1)),
        helmet: meshes.add(
            Sphere::new(0.122)
                .mesh()
                .ico(2)
                .expect("valid ico subdivisions"),
        ),
        arm: meshes.add(Capsule3d::new(0.045, 0.42)),
        thigh: meshes.add(Capsule3d::new(0.075, THIGH - 0.1)),
        shin: meshes.add(Capsule3d::new(0.055, SHIN - 0.08)),
        foot: meshes.add(Cuboid::new(0.09, 0.07, 0.24)),
        crank: meshes.add(Cuboid::new(0.025, CRANK, 0.025)),
        marker: meshes.add(Cone::new(0.1, 0.2)),
    };
    let jersey = |m: &mut Assets<StandardMaterial>, c: Color| {
        m.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 0.55,
            ..default()
        })
    };
    commands.insert_resource(LeaderJerseys {
        green: jersey(&mut materials, Color::srgb(0.1, 0.75, 0.2)),
        white: jersey(&mut materials, Color::srgb(0.97, 0.97, 0.97)),
        red: jersey(&mut materials, Color::srgb(0.85, 0.1, 0.1)),
        worn: (None, None),
    });
    let marker_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.55, 0.0),
        emissive: LinearRgba::rgb(4.0, 1.6, 0.0),
        ..default()
    });

    let race = &res.race;
    let mut team_mats: Vec<(Handle<StandardMaterial>, Handle<StandardMaterial>)> = Vec::new();
    for t in &race.teams {
        let [r, g, b] = t.spec.color;
        let jersey = Color::srgb_u8(r, g, b);
        let frame = Color::srgb_u8(
            (r as f32 * 0.6) as u8,
            (g as f32 * 0.6) as u8,
            (b as f32 * 0.6) as u8,
        );
        team_mats.push((
            mat(&mut materials, jersey, 0.55),
            mat(&mut materials, frame, 0.3),
        ));
    }

    for (i, rider) in race.riders.iter().enumerate() {
        let (jersey, frame) = team_mats[rider.team].clone();
        let root = commands
            .spawn((
                RiderModel {
                    index: i,
                    crank: (i as f32 * 1.37) % TAU,
                    wheel: 0.0,
                    stand: 0.0,
                    surging: false,
                },
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        spawn_bike(&mut commands, root, &shapes, &pal, &frame);
        spawn_body(&mut commands, root, &shapes, &pal, &jersey);
        if rider.human {
            commands.spawn((
                PlayerMarker,
                Mesh3d(shapes.marker.clone()),
                MeshMaterial3d(marker_mat.clone()),
                Transform::from_xyz(0.0, 2.3, 0.0).with_rotation(Quat::from_rotation_x(PI)),
                ChildOf(root),
            ));
        }
    }
}

fn part(
    commands: &mut Commands,
    parent: Entity,
    mesh: &Handle<Mesh>,
    mat: &Handle<StandardMaterial>,
    t: Transform,
) -> Entity {
    commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            t,
            ChildOf(parent),
        ))
        .id()
}

/// A unit cylinder stretched between two points.
fn tube_between(a: Vec3, b: Vec3, thickness: f32) -> Transform {
    let d = b - a;
    Transform {
        translation: (a + b) / 2.0,
        rotation: Quat::from_rotation_arc(Vec3::Y, d.normalize()),
        scale: Vec3::new(thickness, d.length(), thickness),
    }
}

fn spawn_bike(
    commands: &mut Commands,
    root: Entity,
    s: &Shapes,
    pal: &Palette,
    frame: &Handle<StandardMaterial>,
) {
    for axle in [REAR_AXLE, FRONT_AXLE] {
        let pivot = commands
            .spawn((
                WheelSpin,
                Transform::from_translation(axle),
                Visibility::default(),
                ChildOf(root),
            ))
            .id();
        part(
            commands,
            pivot,
            &s.tire,
            &pal.tire,
            Transform::from_rotation(Quat::from_rotation_z(FRAC_PI_2)),
        );
        for k in 0..3 {
            part(
                commands,
                pivot,
                &s.spoke,
                &pal.metal,
                Transform::from_rotation(Quat::from_rotation_x(k as f32 * PI / 3.0)),
            );
        }
    }
    let seat_top = SADDLE - Vec3::Y * 0.04;
    for (a, b, w) in [
        (REAR_AXLE, BB, 1.0),
        (REAR_AXLE, seat_top, 0.8),
        (BB, seat_top, 1.3),
        (seat_top, HEAD_TUBE, 1.2),
        (BB, HEAD_TUBE, 1.5),
        (HEAD_TUBE, FRONT_AXLE, 1.0),
        (HEAD_TUBE, BAR, 1.0),
    ] {
        part(commands, root, &s.tube, frame, tube_between(a, b, w));
    }
    // Handlebar and saddle.
    part(
        commands,
        root,
        &s.tube,
        &pal.tire,
        Transform {
            translation: BAR,
            rotation: Quat::from_rotation_z(FRAC_PI_2),
            scale: Vec3::new(1.0, 0.42, 1.0),
        },
    );
    part(
        commands,
        root,
        &s.foot,
        &pal.tire,
        Transform::from_translation(SADDLE).with_scale(Vec3::new(1.4, 0.6, 1.1)),
    );
    let crank = commands
        .spawn((
            CrankSpin,
            Transform::from_translation(BB),
            Visibility::default(),
            ChildOf(root),
        ))
        .id();
    // At crank angle 0 the right pedal is at the top and the left at the bottom.
    for side in [-1.0f32, 1.0] {
        part(
            commands,
            crank,
            &s.crank,
            &pal.metal,
            Transform::from_xyz(side * 0.08, side * CRANK / 2.0, 0.0),
        );
    }
}

fn spawn_body(
    commands: &mut Commands,
    root: Entity,
    s: &Shapes,
    pal: &Palette,
    jersey: &Handle<StandardMaterial>,
) {
    let body = commands
        .spawn((
            Body,
            Transform::default(),
            Visibility::default(),
            ChildOf(root),
        ))
        .id();
    let shoulder = Vec3::new(0.0, 1.31, -0.2);
    let torso = part(
        commands,
        body,
        &s.torso,
        jersey,
        tube_between(SADDLE + Vec3::Y * 0.05, shoulder, 1.0).with_scale(Vec3::ONE),
    );
    commands.entity(torso).insert(Torso(jersey.clone()));
    let head = shoulder + Vec3::new(0.0, 0.16, -0.12);
    part(
        commands,
        body,
        &s.head,
        &pal.skin,
        Transform::from_translation(head),
    );
    let helmet = part(
        commands,
        body,
        &s.helmet,
        jersey,
        Transform::from_translation(head + Vec3::new(0.0, 0.035, 0.01))
            .with_scale(Vec3::new(0.95, 0.8, 1.2)),
    );
    commands.entity(helmet).insert(Helmet(jersey.clone()));
    for side in [-1.0f32, 1.0] {
        let sh = shoulder + Vec3::X * side * 0.17;
        let hand = BAR + Vec3::X * side * 0.2;
        part(
            commands,
            body,
            &s.arm,
            jersey,
            tube_between(sh, hand, 1.0).with_scale(Vec3::ONE),
        );
    }
    for side in [-1.0f32, 1.0] {
        let thigh = commands
            .spawn((
                LegPart::Thigh(side),
                Transform::default(),
                Visibility::default(),
                ChildOf(root),
            ))
            .id();
        part(
            commands,
            thigh,
            &s.thigh,
            &pal.shorts,
            Transform::from_xyz(0.0, -THIGH / 2.0, 0.0),
        );
        let shin = commands
            .spawn((
                LegPart::Shin(side),
                Transform::default(),
                Visibility::default(),
                ChildOf(root),
            ))
            .id();
        part(
            commands,
            shin,
            &s.shin,
            &pal.skin,
            Transform::from_xyz(0.0, -SHIN / 2.0, 0.0),
        );
        part(
            commands,
            shin,
            &s.shin,
            &pal.white,
            Transform::from_xyz(0.0, -SHIN * 0.78, 0.0).with_scale(Vec3::new(1.08, 0.3, 1.08)),
        );
        commands.spawn((
            LegPart::Foot(side),
            Mesh3d(s.foot.clone()),
            MeshMaterial3d(pal.white.clone()),
            Transform::default(),
            ChildOf(root),
        ));
    }
}

/// World pose of a rider: on the road, pitched with the grade. Before the
/// start of a point-to-point course the grid extends back along the road.
fn rider_transform(race: &Race, dist: f64, lateral: f32) -> Transform {
    let course = &race.course;
    let (d, back) = if !course.route.looped && dist < 0.0 {
        (0.0, -dist as f32)
    } else {
        (dist, 0.0)
    };
    let p = course.path.pose(&course.route, d, lateral);
    let dir = Vec3::new(p.dir[0], 0.0, p.dir[1]);
    let grade = course.grade_pct_at(d) / 100.0;
    let pos = Vec3::from_array(p.pos) - dir * back + Vec3::Y * ROAD_LIFT;
    let forward = Vec3::new(dir.x, grade, dir.z).normalize();
    Transform::from_translation(pos).looking_to(forward, Vec3::Y)
}

fn place_riders(
    res: Res<RaceRes>,
    mut riders: Query<(&mut RiderModel, &mut Transform)>,
    time: Res<Time>,
) {
    let race = &res.race;
    let dt = if res.paused { 0.0 } else { time.delta_secs() };
    for (mut model, mut tf) in &mut riders {
        let r = &race.riders[model.index];
        model.crank = (model.crank + r.cadence / 60.0 * TAU * dt) % TAU;
        model.wheel = (model.wheel + r.speed / WHEEL_R * dt) % TAU;
        let wants = wants_to_stand(
            r,
            race.course.grade_pct_at(r.dist),
            race.time,
            &mut model.surging,
        );
        let target = if wants && r.cadence > 0.0 { 1.0 } else { 0.0 };
        model.stand += (target - model.stand) * (dt * 4.0).min(1.0);

        let mut t = rider_transform(race, r.dist, r.lateral);
        // Out of the saddle the bike rocks with each pedal stroke.
        let rock = model.crank.sin() * 0.11 * model.stand;
        t.rotation *= Quat::from_rotation_z(rock);
        *tf = t;
    }
}

// The animated parts are disjoint sets of entities.
type WheelFilter = (
    With<WheelSpin>,
    Without<CrankSpin>,
    Without<Body>,
    Without<LegPart>,
);
type CrankFilter = (With<CrankSpin>, Without<Body>, Without<LegPart>);

fn animate_riders(
    riders: Query<(&RiderModel, &Children)>,
    mut wheels: Query<&mut Transform, WheelFilter>,
    mut cranks: Query<&mut Transform, CrankFilter>,
    mut bodies: Query<&mut Transform, (With<Body>, Without<LegPart>)>,
    mut legs: Query<(&LegPart, &mut Transform)>,
) {
    for (model, children) in &riders {
        let hip_base = SADDLE + Vec3::Y * 0.02;
        let lift = STAND_OFFSET * model.stand;
        for &c in children {
            if let Ok(mut t) = wheels.get_mut(c) {
                t.rotation = Quat::from_rotation_x(-model.wheel);
            } else if let Ok(mut t) = cranks.get_mut(c) {
                t.rotation = Quat::from_rotation_x(-model.crank);
            } else if let Ok(mut t) = bodies.get_mut(c) {
                t.translation = lift;
                // Standing riders lean forward a little more.
                t.rotation = Quat::from_rotation_x(-0.12 * model.stand);
            } else if let Ok((part, mut t)) = legs.get_mut(c) {
                let side = match *part {
                    LegPart::Thigh(s) | LegPart::Shin(s) | LegPart::Foot(s) => s,
                };
                let phase = model.crank + if side < 0.0 { PI } else { 0.0 };
                let pedal = BB + Vec3::new(side * 0.13, CRANK * phase.cos(), -CRANK * phase.sin());
                let hip = hip_base + lift + Vec3::X * side * HIP_X;
                let (knee, _) = solve_leg(hip, pedal);
                *t = match *part {
                    LegPart::Thigh(_) => {
                        Transform::from_translation(hip).with_rotation(bone_rotation(hip, knee))
                    }
                    LegPart::Shin(_) => {
                        Transform::from_translation(knee).with_rotation(bone_rotation(knee, pedal))
                    }
                    LegPart::Foot(_) => {
                        Transform::from_translation(pedal + Vec3::new(0.0, 0.03, -0.03))
                    }
                };
            }
        }
    }
}

/// When a rider gets out of the saddle: hard efforts (attacks, sprints, big
/// turns; for you above 1.4× FTP until you drop under 1.2×), steep ramps at
/// low cadence or by riding style, and a short stretch every few minutes.
fn wants_to_stand(r: &Rider, grade_pct: f32, time: f32, surging: &mut bool) -> bool {
    let ftp = r.profile.ftp;
    *surging = if *surging {
        r.power > 1.2 * ftp
    } else {
        r.power > 1.4 * ftp
    };
    let effort = match r.intent {
        Intent::Sprint { .. } | Intent::Attack { .. } => true,
        Intent::Pull { intensity, .. } => intensity > 1.1,
        _ => false,
    };
    if *surging || effort {
        return true;
    }
    // Each rider gets their own rhythm so the bunch doesn't move in unison.
    let phase = r.id as f32 * 37.3;
    if grade_pct > 6.0 {
        let liking = match r.profile.kind {
            RiderKind::Climber => 0.55,
            RiderKind::Sprinter => 0.15,
            _ => 0.3,
        };
        let rhythm = ((time + phase) / 9.0).sin() * 0.5 + 0.5;
        if r.cadence < 75.0 || rhythm < liking {
            return true;
        }
    }
    // Stretch the legs for about 4 s every 3–5 minutes.
    let period = 180.0 + (r.id % 7) as f32 * 20.0;
    (time + phase).rem_euclid(period) < 4.0
}

/// Two-bone IK in the bike's side plane: returns the knee position, bent
/// forward, for a hip and a pedal.
fn solve_leg(hip: Vec3, pedal: Vec3) -> (Vec3, f32) {
    // 2D coordinates: u = forward (−Z), v = up (+Y).
    let (pu, pv) = (-(pedal.z - hip.z), pedal.y - hip.y);
    let d = (pu * pu + pv * pv)
        .sqrt()
        .clamp((THIGH - SHIN).abs() + 1e-3, THIGH + SHIN - 1e-3);
    let base = pv.atan2(pu);
    let a = ((THIGH * THIGH + d * d - SHIN * SHIN) / (2.0 * THIGH * d))
        .clamp(-1.0, 1.0)
        .acos();
    let ang = base + a;
    let knee = Vec3::new(
        hip.x + (pedal.x - hip.x) * 0.5,
        hip.y + THIGH * ang.sin(),
        hip.z - THIGH * ang.cos(),
    );
    (knee, ang)
}

/// Rotation turning a bone that hangs along −Y to point from `a` to `b`.
fn bone_rotation(a: Vec3, b: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::NEG_Y, (b - a).normalize())
}

/// Leader's jersey materials and who wears them, reset for every race.
#[derive(Resource)]
struct LeaderJerseys {
    green: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    worn: (Option<usize>, Option<usize>),
}

/// The points leader rides in green, the mountains leader in white with a
/// red helmet.
fn dress_leaders(
    res: Res<RaceRes>,
    mut jerseys: ResMut<LeaderJerseys>,
    riders: Query<(&RiderModel, &Children)>,
    bodies: Query<&Children, With<Body>>,
    mut torsos: Query<(&Torso, &mut MeshMaterial3d<StandardMaterial>), Without<Helmet>>,
    mut helmets: Query<(&Helmet, &mut MeshMaterial3d<StandardMaterial>), Without<Torso>>,
) {
    let race = &res.race;
    let leaders = (
        race.points(SegmentKind::Sprint).first().map(|p| p.0),
        race.points(SegmentKind::Kom).first().map(|p| p.0),
    );
    if leaders == jerseys.worn {
        return;
    }
    jerseys.worn = leaders;
    for (model, children) in &riders {
        let green_jersey = leaders.0 == Some(model.index);
        let polka = leaders.1 == Some(model.index) && !green_jersey;
        for body in children.iter().filter_map(|c| bodies.get(c).ok()) {
            for c in body.iter() {
                if let Ok((team, mut mat)) = torsos.get_mut(c) {
                    mat.0 = if green_jersey {
                        jerseys.green.clone()
                    } else if polka {
                        jerseys.white.clone()
                    } else {
                        team.0.clone()
                    };
                }
                if let Ok((team, mut mat)) = helmets.get_mut(c) {
                    mat.0 = if polka {
                        jerseys.red.clone()
                    } else {
                        team.0.clone()
                    };
                }
            }
        }
    }
}

fn bob_marker(time: Res<Time>, mut markers: Query<&mut Transform, With<PlayerMarker>>) {
    for mut t in &mut markers {
        t.translation.y = 2.3 + (time.elapsed_secs() * 3.0).sin() * 0.06;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leg_ik_keeps_bone_lengths_and_bends_the_knee_forward() {
        let hip = SADDLE;
        for k in 0..12 {
            let phase = k as f32 / 12.0 * TAU;
            let pedal = BB + Vec3::new(0.0, CRANK * phase.cos(), -CRANK * phase.sin());
            let (knee, _) = solve_leg(hip, pedal);
            assert!(((knee - hip).length() - THIGH).abs() < 1e-3);
            assert!(((pedal - knee).length() - SHIN).abs() < 2e-3);
            assert!(knee.z < hip.z, "knee should be ahead of the hip");
        }
    }
}
