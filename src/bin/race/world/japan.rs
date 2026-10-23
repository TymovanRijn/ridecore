//! Japan: tiled roofs, apartment blocks and shop signs, cedar woods
//! turning in the autumn, and the furniture of a Japanese road: utility
//! poles and their wires, footbridges and vending machines in town, white
//! guardrails and curve mirrors in the mountains, torii at the shrines.

use bevy::prelude::*;

use ridecore::map::{Building, BuildingKind, MapData};
use ridecore::race::course::Course;
use ridecore::rng::Rng;

use super::osm::{oriented_rect, triangulate};
use super::props::{self, Rgba, leaf, rgb};
use super::trees::{self, Detail};
use super::zones::Zone;
use super::{RoadIndex, Tris, fbm, pose_v};

fn shade(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

// Houses: white, cream and beige siding under grey kawara tiles or
// coloured metal roofs.
const WALLS: [Rgba; 7] = [
    rgb(0.9, 0.89, 0.85),
    rgb(0.88, 0.84, 0.72),
    rgb(0.78, 0.72, 0.62),
    rgb(0.74, 0.75, 0.75),
    rgb(0.62, 0.53, 0.43),
    rgb(0.7, 0.74, 0.78),
    rgb(0.84, 0.8, 0.76),
];
const ROOFS: [Rgba; 6] = [
    rgb(0.33, 0.35, 0.38),
    rgb(0.22, 0.23, 0.26),
    rgb(0.25, 0.31, 0.42),
    rgb(0.4, 0.27, 0.2),
    rgb(0.2, 0.31, 0.27),
    rgb(0.45, 0.47, 0.5),
];
// Apartment blocks and shops: tiled concrete.
const BLOCKS: [Rgba; 5] = [
    rgb(0.86, 0.85, 0.82),
    rgb(0.76, 0.72, 0.65),
    rgb(0.68, 0.68, 0.67),
    rgb(0.8, 0.74, 0.66),
    rgb(0.6, 0.56, 0.52),
];
// Office and condo towers.
const TOWERS: [Rgba; 4] = [
    rgb(0.6, 0.65, 0.7),
    rgb(0.74, 0.75, 0.76),
    rgb(0.5, 0.56, 0.62),
    rgb(0.82, 0.8, 0.76),
];
const METAL: [Rgba; 3] = [
    rgb(0.7, 0.72, 0.74),
    rgb(0.55, 0.62, 0.68),
    rgb(0.78, 0.76, 0.7),
];
const WINDOW: Rgba = rgb(0.16, 0.2, 0.26);
const GLASS: Rgba = rgb(0.26, 0.34, 0.44);
const SIGNS: [Rgba; 7] = [
    rgb(0.86, 0.14, 0.12),
    rgb(0.96, 0.8, 0.12),
    rgb(0.14, 0.36, 0.78),
    rgb(0.96, 0.96, 0.93),
    rgb(0.12, 0.6, 0.32),
    rgb(0.96, 0.5, 0.1),
    rgb(0.9, 0.3, 0.55),
];
const VERMILION: Rgba = rgb(0.84, 0.24, 0.1);
const CONCRETE: Rgba = rgb(0.64, 0.64, 0.62);
const CABLE: Rgba = rgb(0.08, 0.08, 0.09);
const RAIL: Rgba = rgb(0.92, 0.92, 0.9);

/// A box from `a` to `b` at any slope, `w` wide and `h` tall: cables,
/// rails, stairs.
fn bar(t: &mut Tris, a: Vec3, b: Vec3, w: f32, h: f32, color: Rgba) {
    let Some(axis) = (b - a).try_normalize() else {
        return;
    };
    let side = axis.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X) * (w / 2.0);
    let up = side.cross(axis).normalize() * (h / 2.0);
    let mid = (a + b) / 2.0;
    let c = [up + side, up - side, -up - side, -up + side];
    for k in 0..4 {
        let (p, q) = (c[k], c[(k + 1) % 4]);
        props::quad_out(t, [a + p, b + p, b + q, a + q], mid, color);
    }
}

/// The ground mesh's height beside the road.
fn spot(index: &RoadIndex, course: &Course, d: f64, lateral: f32) -> Vec3 {
    let (p, _) = pose_v(course, d, lateral);
    Vec3::new(p.x, index.surface(p.x, p.z), p.z)
}

/// A building from the map, Japanese style: two-storey houses with low
/// tiled roofs, flat-roofed blocks with window bands and shop signs, glassy
/// towers, temples under big dark roofs.
#[allow(clippy::too_many_arguments)]
pub fn building(
    t: &mut Tris,
    ring: &[Vec2],
    area: f32,
    base: f32,
    ground_top: f32,
    b: &Building,
    index: &RoadIndex,
    rng: &mut Rng,
) {
    let floors = b
        .levels
        .unwrap_or(match b.kind {
            BuildingKind::Shed if area > 800.0 => 1,
            BuildingKind::Shed => 2,
            BuildingKind::Temple => 1,
            _ if area > 700.0 => 5,
            _ if area > 220.0 => 3,
            _ => 2,
        })
        .max(1) as f32;
    let tower = floors >= 7.0;
    let block = !tower && (floors >= 3.0 || area > 220.0);
    let (floor_h, wall, roof) = match b.kind {
        BuildingKind::Temple => (
            5.5,
            if rng.chance(0.5) {
                rgb(0.6, 0.2, 0.12)
            } else {
                rgb(0.4, 0.29, 0.21)
            },
            rgb(0.19, 0.2, 0.23),
        ),
        BuildingKind::Shed => (3.8, *rng.pick(&METAL), rgb(0.42, 0.45, 0.48)),
        _ if tower => (3.4, *rng.pick(&TOWERS), rgb(0.5, 0.5, 0.5)),
        _ if block => (3.0, *rng.pick(&BLOCKS), rgb(0.55, 0.55, 0.53)),
        _ => (2.8, *rng.pick(&WALLS), *rng.pick(&ROOFS)),
    };
    let wall_h = if b.kind == BuildingKind::Temple {
        floor_h
    } else {
        floor_h * floors
    };
    let top = ground_top + wall_h;
    let centroid = ring.iter().copied().sum::<Vec2>() / ring.len() as f32;
    let inside = Vec3::new(centroid.x, (base + top) / 2.0, centroid.y);
    // The road, for the shop fronts and signs.
    let near = index.near(centroid.x, centroid.y);
    let right = Vec3::new(-near.dir.z, 0.0, near.dir.x);
    let to_road = -right * near.lateral.signum();
    let street = near.dist < 40.0 && b.kind != BuildingKind::Temple;
    let sign_color = *rng.pick(&SIGNS);
    let n = ring.len();
    for k in 0..n {
        let (p, q) = (ring[k], ring[(k + 1) % n]);
        let (p0, q0) = (Vec3::new(p.x, base, p.y), Vec3::new(q.x, base, q.y));
        let (p1, q1) = (Vec3::new(p.x, top, p.y), Vec3::new(q.x, top, q.y));
        props::quad_out(t, [p0, q0, q1, p1], inside, shade(wall, 0.95));
        let len = p.distance(q);
        if len < 3.0 || b.kind == BuildingKind::Temple {
            continue;
        }
        let d = (q - p).normalize();
        let out = Vec3::new(d.y, 0.0, -d.x);
        let off = out * 0.05;
        let inset = (q0 - p0).normalize() * 0.6;
        let faces_road = out.dot(to_road) > 0.7;
        let (band, glass, from) = match b.kind {
            _ if tower => (2.0, GLASS, 0),
            BuildingKind::Shed => (0.0, WINDOW, floors as usize),
            _ if block => (1.4, WINDOW, 0),
            _ => (1.1, WINDOW, 0),
        };
        for f in from..floors as usize {
            let y = ground_top + 0.8 + f as f32 * floor_h;
            // Ground floor shops on the street: a sign band over a wide
            // window.
            if f == 0 && street && faces_road && (block || tower) {
                let (a0, b0) = (p0.with_y(y - 0.3) + off, q0.with_y(y - 0.3) + off);
                props::quad_out(
                    t,
                    [
                        a0 + inset,
                        b0 - inset,
                        b0 - inset + Vec3::Y * 1.9,
                        a0 + inset + Vec3::Y * 1.9,
                    ],
                    inside,
                    rgb(0.55, 0.62, 0.66),
                );
                let (s0, s1) = (
                    p0.with_y(y + 1.75) + off * 2.0,
                    q0.with_y(y + 1.75) + off * 2.0,
                );
                props::quad_out(
                    t,
                    [s0, s1, s1 + Vec3::Y * 0.7, s0 + Vec3::Y * 0.7],
                    inside,
                    sign_color,
                );
                continue;
            }
            let (a0, b0) = (p0.with_y(y) + off, q0.with_y(y) + off);
            let (a1, b1) = (a0 + Vec3::Y * band, b0 + Vec3::Y * band);
            props::quad_out(
                t,
                [a0 + inset, b0 - inset, b1 - inset, a1 + inset],
                inside,
                glass,
            );
            // Balconies: a pale slab under each floor of windows.
            if block && f > 0 {
                let (c0, c1) = (
                    p0.with_y(y - 0.35) + off * 3.0,
                    q0.with_y(y - 0.35) + off * 3.0,
                );
                props::quad_out(
                    t,
                    [c0, c1, c1 + Vec3::Y * 0.22, c0 + Vec3::Y * 0.22],
                    inside,
                    rgb(0.9, 0.9, 0.88),
                );
            }
        }
    }
    // Roof.
    let rect = if n == 4 || b.kind == BuildingKind::Temple {
        oriented_rect(ring)
    } else {
        None
    };
    let small = !block && !tower && b.kind != BuildingKind::Shed;
    match rect {
        Some((center, half_w, half_d, yaw)) if b.kind == BuildingKind::Temple => {
            // A deep overhang and a steep, dark roof.
            props::gable_roof(
                t,
                Vec3::new(center.x, top, center.y),
                half_w * 1.15 + 0.8,
                half_d + 1.0,
                half_w * 0.95,
                yaw,
                roof,
            );
        }
        Some((center, half_w, half_d, yaw)) if small && half_w < 8.0 => {
            props::gable_roof(
                t,
                Vec3::new(center.x, top, center.y),
                half_w,
                half_d,
                half_w * 0.5,
                yaw,
                roof,
            );
        }
        _ => {
            for tri in triangulate(ring) {
                let [p, q, r] = tri.map(|i| Vec3::new(ring[i].x, top, ring[i].y));
                props::tri_out(t, p, q, r, p - Vec3::Y * 5.0, roof);
            }
            // Lift machinery and water tanks on the flat roofs.
            if (block || tower) && area > 150.0 {
                let c = Vec3::new(centroid.x, top, centroid.y);
                props::cuboid(
                    t,
                    c + Vec3::Y * 1.2,
                    Vec3::new(1.8, 1.2, 1.5),
                    rng.range(0.0, 3.0),
                    shade(wall, 0.85),
                );
            }
        }
    }
    // A vertical sign on the corner nearest the road.
    if street && (block || tower) && floors >= 3.0 && rng.chance(0.55) {
        let corner = ring
            .iter()
            .copied()
            .min_by(|a, b| {
                index
                    .near(a.x, a.y)
                    .dist
                    .total_cmp(&index.near(b.x, b.y).dist)
            })
            .unwrap_or(centroid);
        let h = (floors.min(6.0) * floor_h - 2.5).max(2.5);
        let foot = Vec3::new(corner.x, ground_top + 3.0, corner.y) + to_road * 0.7;
        let yaw = (-near.dir.z).atan2(near.dir.x);
        props::cuboid(
            t,
            foot + Vec3::Y * h / 2.0,
            Vec3::new(0.12, h / 2.0, 0.55),
            yaw,
            *rng.pick(&SIGNS),
        );
    }
}

// Sugi (Japanese cedar) plantations, and the broadleaf woods in their
// October colours: mostly green, maples going red and orange higher up.
const SUGI: [Rgba; 3] = [
    leaf(0.1, 0.22, 0.11),
    leaf(0.13, 0.26, 0.12),
    leaf(0.09, 0.19, 0.1),
];
const KONARA: [Rgba; 4] = [
    leaf(0.22, 0.38, 0.15),
    leaf(0.28, 0.42, 0.16),
    leaf(0.18, 0.33, 0.13),
    leaf(0.36, 0.42, 0.17),
];
const AUTUMN: [Rgba; 4] = [
    leaf(0.75, 0.22, 0.08),
    leaf(0.82, 0.42, 0.1),
    leaf(0.78, 0.6, 0.14),
    leaf(0.6, 0.16, 0.08),
];
const BARK: Rgba = rgb(0.32, 0.22, 0.16);

/// A tree in a Japanese wood at `p`: tall, straight cedars in the
/// plantations, otherwise oak and maple.
pub fn tree(t: &mut Tris, p: Vec3, s: f32, detail: Detail, rng: &mut Rng) {
    // Cedar plantations cover most of the hills west of Tokyo.
    if fbm(p.x / 150.0 + 7.0, p.z / 150.0) > -0.18 {
        let g = *rng.pick(&SUGI);
        let tall = rng.range(1.0, 1.35);
        trees::conifer(
            t,
            p,
            15.0 * s * tall,
            2.1 * s / tall.sqrt(),
            detail,
            [g, BARK],
            rng,
        );
        return;
    }
    let turning = ((p.y - 250.0) / 500.0).clamp(0.08, 0.45);
    let g = if rng.chance(turning) {
        *rng.pick(&AUTUMN)
    } else {
        *rng.pick(&KONARA)
    };
    let crown = p + Vec3::Y * rng.range(5.8, 7.0) * s;
    trees::leafy(
        t,
        p,
        crown,
        Vec3::new(3.6, 3.0, 3.4) * s,
        detail,
        [g, BARK],
        rng,
    );
}

/// A concrete utility pole with its crossarm; returns where the three
/// cables hang from.
fn pole(t: &mut Tris, p: Vec3, dir: Vec3, rng: &mut Rng) -> [Vec3; 3] {
    let across = Vec3::new(-dir.z, 0.0, dir.x);
    props::prism(t, p - Vec3::Y * 0.3, 0.17, 10.8, 6, CONCRETE);
    let arm = p + Vec3::Y * 9.7;
    bar(
        t,
        arm - across * 0.85,
        arm + across * 0.85,
        0.12,
        0.12,
        rgb(0.4, 0.4, 0.42),
    );
    if rng.chance(0.25) {
        // A transformer drum.
        props::prism(
            t,
            p + Vec3::Y * 7.2 + across * 0.45,
            0.32,
            1.1,
            8,
            rgb(0.6, 0.64, 0.66),
        );
    }
    // A yellow-and-black guard sleeve at the foot.
    props::prism(t, p, 0.19, 1.6, 6, rgb(0.95, 0.8, 0.12));
    [
        arm - across * 0.75 + Vec3::Y * 0.1,
        arm + across * 0.75 + Vec3::Y * 0.1,
        p + Vec3::Y * 10.4,
    ]
}

/// A cable from `a` to `b`, sagging in the middle.
fn cable(t: &mut Tris, a: Vec3, b: Vec3) {
    let sag = a.distance(b) * 0.018;
    let at = |s: f32| a.lerp(b, s) - Vec3::Y * sag * 4.0 * s * (1.0 - s);
    let n = 4;
    for k in 0..n {
        bar(
            t,
            at(k as f32 / n as f32),
            at((k + 1) as f32 / n as f32),
            0.05,
            0.05,
            CABLE,
        );
    }
}

/// A red torii, its beams across the path that leads from the road to the
/// shrine.
fn torii(t: &mut Tris, foot: Vec3, along: Vec3) {
    let half = 1.9;
    for s in [-1.0, 1.0] {
        props::prism(
            t,
            foot + along * half * s - Vec3::Y * 0.3,
            0.22,
            4.7,
            8,
            VERMILION,
        );
    }
    let at = |y: f32, reach: f32| {
        (
            foot + Vec3::Y * y - along * reach,
            foot + Vec3::Y * y + along * reach,
        )
    };
    let (a, b) = at(3.6, half + 0.5);
    bar(t, a, b, 0.2, 0.26, VERMILION);
    let (a, b) = at(4.25, half + 1.0);
    bar(t, a, b, 0.3, 0.3, VERMILION);
    let (a, b) = at(4.55, half + 1.35);
    bar(t, a, b, 0.42, 0.26, rgb(0.08, 0.08, 0.08));
}

/// A roadside mirror on the outside of a blind bend.
fn curve_mirror(t: &mut Tris, foot: Vec3, facing: Vec3) {
    let orange = rgb(0.96, 0.46, 0.1);
    props::prism(t, foot - Vec3::Y * 0.2, 0.05, 2.9, 6, orange);
    let c = foot + Vec3::Y * 2.9 + facing * 0.12;
    let side = Vec3::new(-facing.z, 0.0, facing.x);
    let r = 0.42;
    let ring = |k: usize, r: f32| {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        c + side * a.cos() * r + Vec3::Y * a.sin() * r
    };
    let behind = c - facing;
    let front = c + facing;
    for k in 0..12 {
        // Mirror glass inside an orange rim, orange at the back.
        props::tri_out(
            t,
            c,
            ring(k, r * 0.85),
            ring(k + 1, r * 0.85),
            behind,
            rgb(0.78, 0.84, 0.9),
        );
        props::quad_out(
            t,
            [
                ring(k, r * 0.85),
                ring(k + 1, r * 0.85),
                ring(k + 1, r),
                ring(k, r),
            ],
            behind,
            orange,
        );
        props::tri_out(
            t,
            c - facing * 0.06,
            ring(k, r) - facing * 0.06,
            ring(k + 1, r) - facing * 0.06,
            front,
            orange,
        );
    }
}

/// A vending machine facing the road.
fn vending_machine(t: &mut Tris, foot: Vec3, to_road: Vec3, rng: &mut Rng) {
    let body = *rng.pick(&[
        rgb(0.92, 0.92, 0.9),
        rgb(0.82, 0.14, 0.12),
        rgb(0.16, 0.34, 0.7),
        rgb(0.92, 0.92, 0.9),
    ]);
    let yaw = (-to_road.z).atan2(to_road.x) + std::f32::consts::FRAC_PI_2;
    let c = foot + Vec3::Y * 0.92;
    props::cuboid(t, c, Vec3::new(0.5, 0.92, 0.38), yaw, body);
    // The lit display of drinks.
    let side = Vec3::new(-to_road.z, 0.0, to_road.x) * 0.42;
    let f = c + to_road * 0.39;
    props::quad_out(
        t,
        [
            f - side + Vec3::Y * 0.05,
            f + side + Vec3::Y * 0.05,
            f + side + Vec3::Y * 0.75,
            f - side + Vec3::Y * 0.75,
        ],
        c,
        rgb(0.93, 0.95, 0.97),
    );
}

/// A steel footbridge over the road, with stairs down on both sides.
fn footbridge(t: &mut Tris, index: &RoadIndex, course: &Course, d: f64) {
    let paint = rgb(0.22, 0.5, 0.56);
    let (c, dir) = pose_v(course, d, 0.0);
    let across = Vec3::new(-dir.z, 0.0, dir.x);
    let deck_y = c.y + 5.3;
    let reach = 9.5;
    let a = Vec3::new(c.x, deck_y, c.z) - across * reach;
    let b = Vec3::new(c.x, deck_y, c.z) + across * reach;
    bar(t, a, b, 2.2, 0.5, paint);
    for s in [-1.0, 1.0] {
        // Railings.
        let off = dir * 1.05 * s + Vec3::Y * 1.0;
        bar(t, a + off, b + off, 0.08, 0.08, paint);
        // Piers and stairs running down along the road.
        for end in [a, b] {
            let lat = (end - Vec3::new(c.x, deck_y, c.z)).dot(across);
            let foot = spot(index, course, d, lat);
            props::prism(
                t,
                foot - Vec3::Y * 0.3,
                0.22,
                deck_y - foot.y + 0.3,
                6,
                paint,
            );
            if s > 0.0 {
                let top = end + across * lat.signum() * 1.0;
                let low = spot(index, course, d + 10.0, lat + lat.signum() * 1.0) + Vec3::Y * 0.2;
                bar(t, top - Vec3::Y * 0.2, low, 1.4, 0.3, paint);
            }
        }
    }
}

/// The street furniture of a Japanese road.
pub fn street(
    t: &mut Tris,
    index: &RoadIndex,
    course: &Course,
    map: Option<&MapData>,
    rng: &mut Rng,
) {
    let lap = course.route.length_m();
    let mountain = |z: Zone| matches!(z, Zone::Forest | Zone::Rocky);
    // Utility poles along one side, changing sides now and then.
    let mut side = 1.0f32;
    let mut last: Option<[Vec3; 3]> = None;
    let mut d = 12.0;
    while d < lap - 15.0 {
        let zone = index.zones.at(d);
        if rng.chance(0.03) {
            side = -side;
            last = None;
        }
        let lat = side * if zone == Zone::Village { 6.5 } else { 7.0 };
        let p = spot(index, course, d, lat);
        let (road, dir) = pose_v(course, d, 0.0);
        // Not on bridges and embankments, where the ground falls away.
        if (road.y - p.y).abs() > 2.5 {
            last = None;
        } else {
            let hooks = pole(t, p, dir, rng);
            if let Some(prev) = last {
                for k in 0..3 {
                    cable(t, prev[k], hooks[k]);
                }
            }
            last = Some(hooks);
        }
        d += if zone == Zone::Village { 31.0 } else { 38.0 } + rng.range(-3.0, 3.0) as f64;
    }

    // Guardrails where the mountain road drops away, posts every 4 m.
    let step = 4.0;
    let mut d = 0.0;
    let mut runs: [Option<Vec3>; 2] = [None, None];
    while d < lap {
        let zone = index.zones.at(d);
        for (k, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            let (road, _) = pose_v(course, d, 0.0);
            let drop = road.y - spot(index, course, d, s * 10.0).y;
            if !mountain(zone) || drop < 1.2 {
                runs[k] = None;
                continue;
            }
            let (p, _) = pose_v(course, d, s * 5.9);
            let foot = Vec3::new(p.x, road.y, p.z);
            props::prism(t, foot - Vec3::Y * 0.4, 0.06, 1.2, 4, RAIL);
            if let Some(prev) = runs[k] {
                bar(
                    t,
                    prev + Vec3::Y * 0.62,
                    foot + Vec3::Y * 0.62,
                    0.06,
                    0.32,
                    RAIL,
                );
            }
            runs[k] = Some(foot);
        }
        d += step;
    }

    // Curve mirrors on the outside of tight bends.
    let mut last_mirror = -1000.0f64;
    let mut d = 20.0;
    while d < lap - 20.0 {
        let (_, a) = pose_v(course, d - 18.0, 0.0);
        let (_, b) = pose_v(course, d + 18.0, 0.0);
        let turn = a.angle_between(b);
        if turn > 0.9 && d - last_mirror > 140.0 && index.zones.at(d) != Zone::Village {
            // Turning right when a × b points down (y), so the outside is left.
            let outside = if a.cross(b).y < 0.0 { -1.0 } else { 1.0 };
            let foot = spot(index, course, d, outside * 6.6);
            let (road, _) = pose_v(course, d, 0.0);
            let facing = (Vec3::new(road.x, foot.y, road.z) - foot).normalize_or_zero();
            curve_mirror(t, foot, facing);
            last_mirror = d;
        }
        d += 5.0;
    }

    // Town: vending machines on the pavement, footbridges over the main road.
    let mut d = 0.0;
    let mut last_bridge = 300.0f64;
    while d < lap - 30.0 {
        if index.zones.at(d) == Zone::Village {
            for s in [-1.0f32, 1.0] {
                if rng.chance(0.012) {
                    let foot = spot(index, course, d, s * 7.6);
                    let (road, _) = pose_v(course, d, 0.0);
                    let to_road = (Vec3::new(road.x, foot.y, road.z) - foot).normalize_or_zero();
                    vending_machine(t, foot, to_road, rng);
                }
            }
            if d - last_bridge > 900.0 && rng.chance(0.05) {
                footbridge(t, index, course, d);
                last_bridge = d;
            }
        }
        d += 6.0;
    }

    // Torii by the road where a shrine stands close to it.
    let Some(map) = map else { return };
    let mut placed: Vec<Vec3> = Vec::new();
    for shrine in map.places.iter().filter(|p| p.kind == "shrine") {
        let [x, z] = shrine.pos;
        let n = index.near(x, z);
        if n.dist > 150.0 || n.dist < 3.0 {
            continue;
        }
        let right = Vec3::new(-n.dir.z, 0.0, n.dir.x);
        let at = Vec3::new(x, 0.0, z) - right * n.lateral + right * n.lateral.signum() * 9.5;
        let foot = Vec3::new(at.x, index.surface(at.x, at.z), at.z);
        if placed.iter().any(|q| q.distance(foot) < 40.0) {
            continue;
        }
        torii(t, foot, n.dir.with_y(0.0).normalize_or_zero());
        placed.push(foot);
    }
}

/// Where the map has no buildings along a town street (much of suburban
/// Tokyo isn't mapped yet), a row of them in the local style: houses,
/// shops and the odd apartment block, set back behind the pavement.
pub fn infill(t: &mut Tris, index: &RoadIndex, course: &Course, map: &MapData, rng: &mut Rng) {
    // 8 m cells covered by a mapped building.
    let cell = 8.0;
    let key = |p: Vec2| ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
    let mut taken = std::collections::HashSet::new();
    let take = |ring: &[Vec2], taken: &mut std::collections::HashSet<(i32, i32)>| {
        let lo = ring.iter().copied().fold(Vec2::MAX, Vec2::min);
        let hi = ring.iter().copied().fold(Vec2::MIN, Vec2::max);
        let ((i0, j0), (i1, j1)) = (key(lo), key(hi));
        for i in i0..=i1 {
            for j in j0..=j1 {
                taken.insert((i, j));
            }
        }
    };
    for b in &map.buildings {
        let ring: Vec<Vec2> = b.footprint.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        take(&ring, &mut taken);
    }
    let lap = course.route.length_m();
    let mut next = [0.0f64; 2];
    let mut d = 0.0;
    while d < lap - 15.0 {
        if index.zones.at(d) != Zone::Village {
            d += 4.0;
            continue;
        }
        for (k, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            if d < next[k] {
                continue;
            }
            let width = rng.range(7.0, 12.0);
            let depth = rng.range(8.0, 13.0);
            let front = 8.8 + rng.range(0.0, 1.5);
            next[k] = d + width as f64 + rng.range(1.0, 3.5) as f64;
            let at = |d: f64, lat: f32| {
                let (p, _) = pose_v(course, d, lat);
                Vec2::new(p.x, p.z)
            };
            let far = d + width as f64;
            let mut ring = vec![
                at(d, s * front),
                at(far, s * front),
                at(far, s * (front + depth)),
                at(d, s * (front + depth)),
            ];
            let centroid = ring.iter().copied().sum::<Vec2>() / 4.0;
            let free = ring.iter().chain([&centroid]).all(|p| {
                !taken.contains(&key(*p))
                    && map.cover(p.x, p.y) != ridecore::map::Cover::Water
                    && index.near(p.x, p.y).dist > super::ROAD_HALF + 3.2
            });
            if !free {
                continue;
            }
            let heights: Vec<f32> = ring.iter().map(|p| index.surface(p.x, p.y)).collect();
            let base = heights.iter().copied().fold(f32::MAX, f32::min) - 0.4;
            let ground_top = heights.iter().copied().fold(f32::MIN, f32::max);
            if ground_top - base > 3.5 {
                continue;
            }
            let mut a = super::osm::area(&ring);
            if a < 0.0 {
                ring.reverse();
                a = -a;
            }
            let levels = match rng.f32() {
                x if x < 0.6 => 2,
                x if x < 0.8 => 3,
                x if x < 0.95 => rng.range(4.0, 6.0) as u32,
                _ => rng.range(6.0, 10.0) as u32,
            };
            let b = Building {
                levels: Some(levels),
                kind: if rng.chance(0.1) {
                    BuildingKind::Shed
                } else {
                    BuildingKind::House
                },
                footprint: ring.iter().map(|p| [p.x, p.y]).collect(),
            };
            building(t, &ring, a, base, ground_top, &b, index, rng);
            take(&ring, &mut taken);
        }
        d += 2.0;
    }
}
