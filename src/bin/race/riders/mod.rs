//! Rider models built from lofted shapes (see `shapes`), placed on the road every frame from
//! the simulation, with pedaling legs solved by two-bone IK, spinning wheels
//! and an out-of-the-saddle sprint.
//!
//! Model space: forward is −Z, up is +Y, right is +X.

use std::f32::consts::{PI, TAU};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use ridecore::race::ai::Intent;
use ridecore::race::course::SegmentKind;
use ridecore::race::field::RiderKind;
use ridecore::race::{Race, Rider};

use crate::{RaceRes, Screen};

mod shapes;

const WHEEL_R: f32 = 0.34;
const REAR_AXLE: Vec3 = Vec3::new(0.0, WHEEL_R, 0.5);
const FRONT_AXLE: Vec3 = Vec3::new(0.0, WHEEL_R, -0.5);
const BB: Vec3 = Vec3::new(0.0, 0.30, 0.05);
const SADDLE: Vec3 = Vec3::new(0.0, 0.96, 0.24);
/// Middle of the handlebar's tops.
const BAR: Vec3 = Vec3::new(0.0, 0.94, -0.47);
/// Where the right hand rests on the brake hood (mirrored for the left).
const HOOD: Vec3 = Vec3::new(0.205, 0.985, -0.56);
const CRANK: f32 = 0.17;
const THIGH: f32 = 0.45;
const SHIN: f32 = 0.45;
const HIP_X: f32 = 0.11;
const UPPER_ARM: f32 = 0.29;
const FOREARM: f32 = 0.27;
/// Seat of the rider, where the spine starts.
const HIP: Vec3 = Vec3::new(0.0, 0.98, 0.24);
/// Base of the neck: the back leans forward about 40°.
const SHOULDER: Vec3 = Vec3::new(0.0, 1.34, -0.19);
/// The right shoulder joint (mirrored for the left).
const ARM_ROOT: Vec3 = Vec3::new(0.165, 1.3, -0.16);
const HEAD: Vec3 = Vec3::new(0.0, 1.475, -0.31);
/// How far the hips rise and move forward when standing on the pedals.
const STAND_OFFSET: Vec3 = Vec3::new(0.0, 0.14, -0.17);
/// Riders sit this far above the simulated road surface.
const ROAD_LIFT: f32 = 0.12;

#[derive(Component)]
pub struct RiderModel {
    pub index: usize,
    /// Out of the saddle because of a hard effort (kept with hysteresis).
    surging: bool,
}

/// Where a rider (or the ghost) is in their pedal stroke.
#[derive(Component, Default)]
struct Pedaling {
    crank: f32,
    wheel: f32,
    /// 0 seated, 1 standing on the pedals.
    stand: f32,
}

/// Your best ride on this course, riding along translucent.
#[derive(Component)]
pub struct GhostRider;

#[derive(Component)]
struct WheelSpin;

#[derive(Component)]
struct CrankSpin;

#[derive(Component)]
struct Body;

#[derive(Component)]
struct PlayerMarker;

/// A part of a rider's kit that changes for a leader's jersey, with the
/// team's version to go back to.
#[derive(Component)]
struct Kit {
    rider: usize,
    part: KitPart,
}

enum KitPart {
    Torso(Handle<Mesh>),
    Sleeve(Handle<StandardMaterial>),
    Helmet(Handle<StandardMaterial>),
}

/// Bones posed every frame; the value is the side (−1 left, 1 right).
#[derive(Component, Clone, Copy)]
enum Limb {
    Thigh(f32),
    Shin(f32),
    Foot(f32),
    UpperArm(f32),
    Forearm(f32),
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
                (
                    place_riders,
                    place_ghost,
                    animate_riders,
                    bob_marker,
                    dress_leaders,
                    ghost_casts_no_shadow,
                )
                    .chain()
                    .after(crate::advance_race)
                    .in_set(RidersPlaced)
                    .run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
            );
    }
}

/// Materials shared by every rider.
struct Palette {
    /// White, for meshes that carry their colors per vertex.
    cloth: Handle<StandardMaterial>,
    /// The same for the smoother bike parts.
    parts: Handle<StandardMaterial>,
    /// Light to dark skin tones.
    skin: [Handle<StandardMaterial>; 4],
}

struct Shapes {
    wheel: Handle<Mesh>,
    frame: Handle<Mesh>,
    fittings: Handle<Mesh>,
    cranks: Handle<Mesh>,
    head: Handle<Mesh>,
    helmet: Handle<Mesh>,
    sleeve: Handle<Mesh>,
    upper_arm: Handle<Mesh>,
    forearm: Handle<Mesh>,
    thigh: Handle<Mesh>,
    shin: Handle<Mesh>,
    sock: Handle<Mesh>,
    shoe: Handle<Mesh>,
    marker: Handle<Mesh>,
}

/// What one team wears and rides.
struct TeamKit {
    torso: Handle<Mesh>,
    jersey: Handle<StandardMaterial>,
    frame: Handle<StandardMaterial>,
}

type RiderOrGhost = (With<RiderModel>, With<GhostRider>);

fn despawn_riders(mut commands: Commands, riders: Query<Entity, Or<RiderOrGhost>>) {
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
        cloth: mat(&mut materials, Color::WHITE, 0.62),
        parts: mat(&mut materials, Color::WHITE, 0.4),
        skin: [
            (0.87, 0.68, 0.55),
            (0.8, 0.6, 0.46),
            (0.64, 0.45, 0.32),
            (0.42, 0.29, 0.2),
        ]
        .map(|(r, g, b)| mat(&mut materials, Color::srgb(r, g, b), 0.7)),
    };
    let shapes = Shapes {
        wheel: meshes.add(shapes::wheel()),
        frame: meshes.add(shapes::frame()),
        fittings: meshes.add(shapes::fittings()),
        cranks: meshes.add(shapes::cranks()),
        head: meshes.add(shapes::head()),
        helmet: meshes.add(shapes::helmet()),
        sleeve: meshes.add(shapes::sleeve()),
        upper_arm: meshes.add(shapes::upper_arm()),
        forearm: meshes.add(shapes::forearm()),
        thigh: meshes.add(shapes::thigh()),
        shin: meshes.add(shapes::shin()),
        sock: meshes.add(shapes::sock()),
        shoe: meshes.add(shapes::shoe()),
        marker: meshes.add(Cone::new(0.1, 0.2)),
    };
    let jersey = |m: &mut Assets<StandardMaterial>, c: Color| {
        m.add(StandardMaterial {
            base_color: c,
            perceptual_roughness: 0.55,
            ..default()
        })
    };
    let green = Color::srgb(0.1, 0.75, 0.2);
    let white = Color::srgb(0.97, 0.97, 0.97);
    let red = Color::srgb(0.85, 0.1, 0.1);
    commands.insert_resource(LeaderJerseys {
        green_torso: meshes.add(shapes::torso(
            green.to_linear(),
            white.to_linear(),
            shapes::Design::Plain,
        )),
        dots_torso: meshes.add(shapes::torso(
            white.to_linear(),
            red.to_linear(),
            shapes::Design::Dots,
        )),
        green: jersey(&mut materials, green),
        white: jersey(&mut materials, white),
        red: jersey(&mut materials, red),
        worn: (None, None),
    });
    let marker_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.55, 0.0),
        emissive: LinearRgba::rgb(4.0, 1.6, 0.0),
        ..default()
    });

    let race = &res.race;
    let kits: Vec<TeamKit> = race
        .teams
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let [r, g, b] = t.spec.color;
            let main = Color::srgb_u8(r, g, b);
            let dark = |c: u8| (c as f32 * 0.6) as u8;
            TeamKit {
                torso: meshes.add(shapes::torso(
                    main.to_linear(),
                    shapes::accent(main.to_linear()),
                    shapes::Design::for_team(i),
                )),
                jersey: jersey(&mut materials, main),
                frame: materials.add(StandardMaterial {
                    base_color: Color::srgb_u8(dark(r), dark(g), dark(b)),
                    perceptual_roughness: 0.35,
                    clearcoat: 0.8,
                    clearcoat_perceptual_roughness: 0.2,
                    ..default()
                }),
            }
        })
        .collect();

    for (i, rider) in race.riders.iter().enumerate() {
        let kit = &kits[rider.team];
        let root = commands
            .spawn((
                RiderModel {
                    index: i,
                    surging: false,
                },
                Pedaling {
                    crank: (i as f32 * 1.37) % TAU,
                    ..default()
                },
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        spawn_bike(&mut commands, root, &shapes, &pal, kit);
        // Mostly lighter skin, like the European peloton, with some variety.
        let skin = &pal.skin[[0, 0, 0, 0, 0, 1, 1, 1, 2, 3][(i * 7 + 3) % 10]];
        spawn_body(&mut commands, root, i, &shapes, &pal, skin, kit);
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

    if res.best.is_some() {
        let ghost = materials.add(StandardMaterial {
            base_color: Color::srgba(0.8, 0.93, 1.0, 0.45),
            emissive: LinearRgba::rgb(0.35, 0.6, 0.9),
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        let pal = Palette {
            cloth: ghost.clone(),
            parts: ghost.clone(),
            skin: std::array::from_fn(|_| ghost.clone()),
        };
        let kit = TeamKit {
            torso: meshes.add(shapes::torso(
                LinearRgba::WHITE,
                LinearRgba::WHITE,
                shapes::Design::Plain,
            )),
            jersey: ghost.clone(),
            frame: ghost,
        };
        let root = commands
            .spawn((
                GhostRider,
                Pedaling::default(),
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        spawn_bike(&mut commands, root, &shapes, &pal, &kit);
        spawn_body(
            &mut commands,
            root,
            usize::MAX,
            &shapes,
            &pal,
            &pal.cloth,
            &kit,
        );
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

/// An empty entity to hang parts on.
fn pivot(commands: &mut Commands, parent: Entity, t: Transform, tag: impl Bundle) -> Entity {
    commands
        .spawn((tag, t, Visibility::default(), ChildOf(parent)))
        .id()
}

fn spawn_bike(commands: &mut Commands, root: Entity, s: &Shapes, pal: &Palette, kit: &TeamKit) {
    for axle in [REAR_AXLE, FRONT_AXLE] {
        let wheel = pivot(commands, root, Transform::from_translation(axle), WheelSpin);
        part(commands, wheel, &s.wheel, &pal.parts, Transform::IDENTITY);
    }
    part(commands, root, &s.frame, &kit.frame, Transform::IDENTITY);
    part(commands, root, &s.fittings, &pal.parts, Transform::IDENTITY);
    let crank = pivot(commands, root, Transform::from_translation(BB), CrankSpin);
    part(commands, crank, &s.cranks, &pal.parts, Transform::IDENTITY);
}

fn spawn_body(
    commands: &mut Commands,
    root: Entity,
    rider: usize,
    s: &Shapes,
    pal: &Palette,
    skin: &Handle<StandardMaterial>,
    kit: &TeamKit,
) {
    let body = pivot(commands, root, Transform::IDENTITY, Body);
    let torso = part(commands, body, &kit.torso, &pal.cloth, Transform::IDENTITY);
    commands.entity(torso).insert(Kit {
        rider,
        part: KitPart::Torso(kit.torso.clone()),
    });
    part(commands, body, &s.head, skin, Transform::IDENTITY);
    let helmet = part(commands, body, &s.helmet, &kit.jersey, Transform::IDENTITY);
    commands.entity(helmet).insert(Kit {
        rider,
        part: KitPart::Helmet(kit.jersey.clone()),
    });

    for side in [-1.0f32, 1.0] {
        let upper = pivot(commands, root, Transform::IDENTITY, Limb::UpperArm(side));
        part(commands, upper, &s.upper_arm, skin, Transform::IDENTITY);
        let sleeve = part(commands, upper, &s.sleeve, &kit.jersey, Transform::IDENTITY);
        commands.entity(sleeve).insert(Kit {
            rider,
            part: KitPart::Sleeve(kit.jersey.clone()),
        });
        let fore = pivot(commands, root, Transform::IDENTITY, Limb::Forearm(side));
        part(commands, fore, &s.forearm, skin, Transform::IDENTITY);

        let thigh = pivot(commands, root, Transform::IDENTITY, Limb::Thigh(side));
        part(commands, thigh, &s.thigh, skin, Transform::IDENTITY);
        let shin = pivot(commands, root, Transform::IDENTITY, Limb::Shin(side));
        part(commands, shin, &s.shin, skin, Transform::IDENTITY);
        part(commands, shin, &s.sock, &pal.cloth, Transform::IDENTITY);
        let foot = pivot(commands, root, Transform::IDENTITY, Limb::Foot(side));
        part(commands, foot, &s.shoe, &pal.cloth, Transform::IDENTITY);
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
    mut riders: Query<(&mut RiderModel, &mut Pedaling, &mut Transform)>,
    time: Res<Time>,
) {
    let race = &res.race;
    let dt = if res.paused { 0.0 } else { time.delta_secs() };
    for (mut model, mut pedal, mut tf) in &mut riders {
        let r = &race.riders[model.index];
        pedal.crank = (pedal.crank + r.cadence / 60.0 * TAU * dt) % TAU;
        pedal.wheel = (pedal.wheel + r.speed / WHEEL_R * dt) % TAU;
        let wants = wants_to_stand(
            r,
            race.course.grade_pct_at(r.dist),
            race.time,
            &mut model.surging,
        );
        let target = if wants && r.cadence > 0.0 { 1.0 } else { 0.0 };
        pedal.stand += (target - pedal.stand) * (dt * 4.0).min(1.0);

        let mut t = rider_transform(race, r.dist, r.lateral);
        // Out of the saddle the bike rocks with each pedal stroke.
        let rock = pedal.crank.sin() * 0.11 * pedal.stand;
        t.rotation *= Quat::from_rotation_z(rock);
        *tf = t;
    }
}

/// The ghost rides where you were at this time on your best ride, a little
/// to the side of you.
fn place_ghost(
    res: Res<RaceRes>,
    mut ghosts: Query<(&mut Pedaling, &mut Transform), With<GhostRider>>,
    time: Res<Time>,
) {
    let Some(best) = &res.best else {
        return;
    };
    let race = &res.race;
    let dt = if res.paused { 0.0 } else { time.delta_secs() };
    let d = best.dist_at(race.time);
    let speed = best.dist_at(race.time + 0.5) - best.dist_at(race.time - 0.5);
    let me = race.player();
    let lateral = me.lateral + if me.lateral > 0.0 { -0.9 } else { 0.9 };
    for (mut pedal, mut tf) in &mut ghosts {
        let cadence = if speed > 0.5 { 88.0 } else { 0.0 };
        pedal.crank = (pedal.crank + cadence / 60.0 * TAU * dt) % TAU;
        pedal.wheel = (pedal.wheel + speed / WHEEL_R * dt) % TAU;
        *tf = rider_transform(race, d as f64, lateral);
    }
}

/// The ghost is see-through, so it shouldn't throw a shadow.
fn ghost_casts_no_shadow(
    mut commands: Commands,
    ghosts: Query<Entity, Added<GhostRider>>,
    children: Query<&Children>,
) {
    for ghost in &ghosts {
        for part in children.iter_descendants(ghost) {
            commands.entity(part).insert(NotShadowCaster);
        }
    }
}

// The animated parts are disjoint sets of entities.
type WheelFilter = (
    With<WheelSpin>,
    Without<CrankSpin>,
    Without<Body>,
    Without<Limb>,
);
type CrankFilter = (With<CrankSpin>, Without<Body>, Without<Limb>);

fn animate_riders(
    riders: Query<(&Pedaling, &Children)>,
    mut wheels: Query<&mut Transform, WheelFilter>,
    mut cranks: Query<&mut Transform, CrankFilter>,
    mut bodies: Query<&mut Transform, (With<Body>, Without<Limb>)>,
    mut limbs: Query<(&Limb, &mut Transform)>,
) {
    for (model, children) in &riders {
        let lift = STAND_OFFSET * model.stand;
        // The upper body leans forward a little more when standing, and
        // stays upright while the bike rocks under it.
        let rock = model.crank.sin() * 0.11 * model.stand;
        let body = Transform::from_translation(lift).with_rotation(
            Quat::from_rotation_x(-0.12 * model.stand) * Quat::from_rotation_z(-rock * 0.6),
        );
        for &c in children {
            if let Ok(mut t) = wheels.get_mut(c) {
                t.rotation = Quat::from_rotation_x(-model.wheel);
            } else if let Ok(mut t) = cranks.get_mut(c) {
                t.rotation = Quat::from_rotation_x(-model.crank);
            } else if let Ok(mut t) = bodies.get_mut(c) {
                *t = body;
            } else if let Ok((limb, mut t)) = limbs.get_mut(c) {
                *t = pose_limb(*limb, model.crank, lift, &body);
            }
        }
    }
}

fn pose_limb(limb: Limb, crank: f32, lift: Vec3, body: &Transform) -> Transform {
    match limb {
        Limb::Thigh(side) | Limb::Shin(side) | Limb::Foot(side) => {
            let phase = crank + if side < 0.0 { PI } else { 0.0 };
            let pedal = BB + Vec3::new(side * 0.13, CRANK * phase.cos(), -CRANK * phase.sin());
            let hip = HIP + lift + Vec3::X * side * HIP_X;
            let (knee, _) = solve_leg(hip, pedal);
            match limb {
                Limb::Thigh(_) => {
                    Transform::from_translation(hip).with_rotation(bone_rotation(hip, knee))
                }
                Limb::Shin(_) => {
                    Transform::from_translation(knee).with_rotation(bone_rotation(knee, pedal))
                }
                // The foot sits on the pedal, toes dropping a little as it
                // pushes over the top.
                _ => Transform::from_translation(pedal + Vec3::new(0.0, 0.035, -0.01))
                    .with_rotation(Quat::from_rotation_x(-0.12 - 0.12 * phase.sin())),
            }
        }
        Limb::UpperArm(side) | Limb::Forearm(side) => {
            let mirror = Vec3::new(side, 1.0, 1.0);
            let shoulder = body.transform_point(ARM_ROOT * mirror);
            let hand = HOOD * mirror;
            // Elbows bend out and down.
            let elbow = two_bone(
                shoulder,
                hand,
                UPPER_ARM,
                FOREARM,
                Vec3::new(side * 0.7, -0.6, 0.3),
            );
            match limb {
                Limb::UpperArm(_) => Transform::from_translation(shoulder)
                    .with_rotation(bone_rotation(shoulder, elbow)),
                _ => Transform::from_translation(elbow).with_rotation(bone_rotation(elbow, hand)),
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

/// Two-bone IK in 3D: the middle joint between `a` and `b`, bent towards
/// `pole`.
fn two_bone(a: Vec3, b: Vec3, l1: f32, l2: f32, pole: Vec3) -> Vec3 {
    let dir = (b - a).normalize();
    let d = (b - a)
        .length()
        .clamp((l1 - l2).abs() + 1e-3, l1 + l2 - 1e-3);
    let along = (l1 * l1 - l2 * l2 + d * d) / (2.0 * d);
    let out = (l1 * l1 - along * along).max(0.0).sqrt();
    let bend = (pole - dir * pole.dot(dir)).normalize_or(Vec3::Y);
    a + dir * along + bend * out
}

/// Rotation turning a bone that hangs along −Y to point from `a` to `b`.
fn bone_rotation(a: Vec3, b: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::NEG_Y, (b - a).normalize())
}

/// Leader's jerseys and who wears them, reset for every race.
#[derive(Resource)]
struct LeaderJerseys {
    green_torso: Handle<Mesh>,
    dots_torso: Handle<Mesh>,
    green: Handle<StandardMaterial>,
    white: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    worn: (Option<usize>, Option<usize>),
}

/// The points leader rides in green, the mountains leader in white with red
/// dots and a red helmet.
fn dress_leaders(
    res: Res<RaceRes>,
    mut jerseys: ResMut<LeaderJerseys>,
    mut kits: Query<(&Kit, &mut Mesh3d, &mut MeshMaterial3d<StandardMaterial>)>,
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
    for (kit, mut mesh, mut mat) in &mut kits {
        let green = leaders.0 == Some(kit.rider);
        let dots = leaders.1 == Some(kit.rider) && !green;
        match &kit.part {
            KitPart::Torso(team) => {
                mesh.0 = if green {
                    jerseys.green_torso.clone()
                } else if dots {
                    jerseys.dots_torso.clone()
                } else {
                    team.clone()
                }
            }
            KitPart::Sleeve(team) => {
                mat.0 = if green {
                    jerseys.green.clone()
                } else if dots {
                    jerseys.white.clone()
                } else {
                    team.clone()
                }
            }
            KitPart::Helmet(team) => {
                mat.0 = if dots {
                    jerseys.red.clone()
                } else {
                    team.clone()
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
        let hip = HIP;
        for k in 0..12 {
            let phase = k as f32 / 12.0 * TAU;
            let pedal = BB + Vec3::new(0.0, CRANK * phase.cos(), -CRANK * phase.sin());
            let (knee, _) = solve_leg(hip, pedal);
            assert!(((knee - hip).length() - THIGH).abs() < 1e-3);
            assert!(((pedal - knee).length() - SHIN).abs() < 2e-3);
            assert!(knee.z < hip.z, "knee should be ahead of the hip");
        }
    }

    #[test]
    fn hands_reach_the_hoods_with_bent_elbows() {
        for stand in [0.0, 1.0] {
            let body = Transform::from_translation(STAND_OFFSET * stand)
                .with_rotation(Quat::from_rotation_x(-0.12 * stand));
            let shoulder = body.transform_point(ARM_ROOT);
            let reach = shoulder.distance(HOOD);
            assert!(
                reach < UPPER_ARM + FOREARM - 0.02,
                "arm stretched at stand {stand}"
            );
            let elbow = two_bone(
                shoulder,
                HOOD,
                UPPER_ARM,
                FOREARM,
                Vec3::new(0.7, -0.6, 0.3),
            );
            assert!((elbow.distance(shoulder) - UPPER_ARM).abs() < 1e-3);
            assert!((elbow.distance(HOOD) - FOREARM).abs() < 1e-3);
            assert!(elbow.x > shoulder.x.min(HOOD.x), "elbow should point out");
        }
    }
}
