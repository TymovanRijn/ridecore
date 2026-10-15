//! Low-poly building blocks and the props made from them. Everything is
//! appended into shared, vertex-colored triangle batches, so thousands of
//! props cost a handful of draw calls.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use ridecore::rng::Rng;

use super::Tris;

pub type Rgba = [f32; 4];

pub const fn rgb(r: f32, g: f32, b: f32) -> Rgba {
    [r, g, b, 1.0]
}

/// Adds a triangle facing away from `inside`, whatever order the corners
/// are given in, so closed solids are always lit from outside.
pub fn tri_out(t: &mut Tris, a: Vec3, b: Vec3, c: Vec3, inside: Vec3, color: Rgba) {
    let n = (b - a).cross(c - a);
    if n.dot((a + b + c) / 3.0 - inside) >= 0.0 {
        t.tri(a, b, c, color);
    } else {
        t.tri(a, c, b, color);
    }
}

pub fn quad_out(t: &mut Tris, q: [Vec3; 4], inside: Vec3, color: Rgba) {
    tri_out(t, q[0], q[1], q[2], inside, color);
    tri_out(t, q[0], q[2], q[3], inside, color);
}

fn yaw_rot(yaw: f32) -> Quat {
    Quat::from_rotation_y(yaw)
}

/// An axis-aligned box rotated by `yaw` around its center.
pub fn cuboid(t: &mut Tris, center: Vec3, half: Vec3, yaw: f32, color: Rgba) {
    let r = yaw_rot(yaw);
    let c = |x: f32, y: f32, z: f32| center + r * Vec3::new(x * half.x, y * half.y, z * half.z);
    let faces = [
        [
            c(-1., -1., 1.),
            c(1., -1., 1.),
            c(1., 1., 1.),
            c(-1., 1., 1.),
        ],
        [
            c(-1., -1., -1.),
            c(-1., 1., -1.),
            c(1., 1., -1.),
            c(1., -1., -1.),
        ],
        [
            c(1., -1., -1.),
            c(1., 1., -1.),
            c(1., 1., 1.),
            c(1., -1., 1.),
        ],
        [
            c(-1., -1., -1.),
            c(-1., -1., 1.),
            c(-1., 1., 1.),
            c(-1., 1., -1.),
        ],
        [
            c(-1., 1., -1.),
            c(-1., 1., 1.),
            c(1., 1., 1.),
            c(1., 1., -1.),
        ],
    ];
    for f in faces {
        quad_out(t, f, center, color);
    }
}

/// A box between two points (posts, rails), `w` thick and `h` tall.
pub fn beam(t: &mut Tris, a: Vec3, b: Vec3, w: f32, h: f32, color: Rgba) {
    let d = b - a;
    let yaw = (-d.z).atan2(d.x);
    let len = Vec2::new(d.x, d.z).length();
    cuboid(
        t,
        (a + b) / 2.0,
        Vec3::new(len / 2.0, h / 2.0, w / 2.0),
        yaw,
        color,
    );
}

/// An upright n-sided prism (trunks, towers, poles) with a top cap.
pub fn prism(t: &mut Tris, base: Vec3, radius: f32, height: f32, sides: usize, color: Rgba) {
    let inside = base + Vec3::Y * height * 0.5;
    let ring = |k: usize, y: f32| {
        let a = k as f32 / sides as f32 * TAU;
        base + Vec3::new(a.cos() * radius, y, a.sin() * radius)
    };
    let top = base + Vec3::Y * height;
    for k in 0..sides {
        quad_out(
            t,
            [
                ring(k, 0.0),
                ring(k + 1, 0.0),
                ring(k + 1, height),
                ring(k, height),
            ],
            inside,
            color,
        );
        tri_out(t, ring(k, height), ring(k + 1, height), top, inside, color);
    }
}

pub fn cone(t: &mut Tris, base: Vec3, radius: f32, height: f32, sides: usize, color: Rgba) {
    let inside = base + Vec3::Y * height * 0.25;
    let tip = base + Vec3::Y * height;
    for k in 0..sides {
        let a0 = k as f32 / sides as f32 * TAU;
        let a1 = (k + 1) as f32 / sides as f32 * TAU;
        let p0 = base + Vec3::new(a0.cos() * radius, 0.0, a0.sin() * radius);
        let p1 = base + Vec3::new(a1.cos() * radius, 0.0, a1.sin() * radius);
        tri_out(t, p0, p1, tip, inside, color);
        tri_out(t, p0, p1, base, inside + Vec3::Y, color);
    }
}

/// A faceted ellipsoid: crowns, bushes, rocks, clouds, sheep.
pub fn blob(t: &mut Tris, center: Vec3, radii: Vec3, color: Rgba) {
    const LON: usize = 7;
    const LAT: usize = 4;
    let p = |i: usize, j: usize| {
        let th = PI * i as f32 / LAT as f32;
        let ph = TAU * j as f32 / LON as f32 + i as f32 * 0.45;
        center
            + Vec3::new(
                th.sin() * ph.cos() * radii.x,
                th.cos() * radii.y,
                th.sin() * ph.sin() * radii.z,
            )
    };
    for i in 0..LAT {
        for j in 0..LON {
            let (a, b, c, d) = (p(i, j), p(i, j + 1), p(i + 1, j + 1), p(i + 1, j));
            if i > 0 {
                tri_out(t, a, b, c, center, color);
            }
            if i + 1 < LAT {
                tri_out(t, a, c, d, center, color);
            }
        }
    }
}

/// A gable roof on walls whose top center is `top`; the ridge runs along the
/// building's local depth.
pub fn gable_roof(
    t: &mut Tris,
    top: Vec3,
    half_w: f32,
    half_d: f32,
    height: f32,
    yaw: f32,
    color: Rgba,
) {
    let r = yaw_rot(yaw);
    let o = 0.5; // overhang
    let p = |x: f32, y: f32, z: f32| top + r * Vec3::new(x, y, z);
    let (w, d) = (half_w + o, half_d + o);
    let (fl, fr, bl, br) = (
        p(-w, -0.2, -d),
        p(w, -0.2, -d),
        p(-w, -0.2, d),
        p(w, -0.2, d),
    );
    let (rf, rb) = (p(0.0, height, -d), p(0.0, height, d));
    let inside = top + Vec3::Y * height * 0.3;
    quad_out(t, [fl, rf, rb, bl], inside, color);
    quad_out(t, [fr, br, rb, rf], inside, color);
    tri_out(t, fl, fr, rf, inside, color);
    tri_out(t, bl, rb, br, inside, color);
}

fn pick(rng: &mut Rng, colors: &[Rgba]) -> Rgba {
    *rng.pick(colors)
}

fn shade(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

const BARK: Rgba = rgb(0.36, 0.25, 0.16);
const PINES: [Rgba; 3] = [
    rgb(0.11, 0.32, 0.17),
    rgb(0.14, 0.38, 0.2),
    rgb(0.09, 0.27, 0.15),
];
const LEAVES: [Rgba; 3] = [
    rgb(0.25, 0.5, 0.2),
    rgb(0.33, 0.56, 0.22),
    rgb(0.2, 0.44, 0.18),
];
const AUTUMN: [Rgba; 4] = [
    rgb(0.85, 0.45, 0.12),
    rgb(0.78, 0.25, 0.12),
    rgb(0.9, 0.7, 0.2),
    rgb(0.6, 0.5, 0.15),
];

pub fn pine(t: &mut Tris, p: Vec3, s: f32, rng: &mut Rng) {
    prism(t, p, 0.22 * s, 2.0 * s, 5, BARK);
    let g = pick(rng, &PINES);
    for (k, (r, h)) in [(2.4, 4.0), (1.9, 3.4), (1.3, 2.8)].iter().enumerate() {
        cone(
            t,
            p + Vec3::Y * (1.6 + k as f32 * 1.7) * s,
            r * s,
            h * s,
            7,
            shade(g, 1.0 + k as f32 * 0.06),
        );
    }
}

pub fn broadleaf(t: &mut Tris, p: Vec3, s: f32, rng: &mut Rng, autumn: bool) {
    prism(t, p, 0.28 * s, 3.0 * s, 5, BARK);
    let g = if autumn && rng.chance(0.7) {
        pick(rng, &AUTUMN)
    } else {
        pick(rng, &LEAVES)
    };
    blob(t, p + Vec3::Y * 4.4 * s, Vec3::new(2.6, 2.2, 2.6) * s, g);
    blob(
        t,
        p + Vec3::new(1.2, 3.8, 0.6) * s,
        Vec3::splat(1.6) * s,
        shade(g, 0.9),
    );
    blob(
        t,
        p + Vec3::new(-1.0, 4.0, -0.9) * s,
        Vec3::splat(1.5) * s,
        shade(g, 1.1),
    );
}

pub fn birch(t: &mut Tris, p: Vec3, s: f32, rng: &mut Rng, autumn: bool) {
    prism(t, p, 0.16 * s, 4.5 * s, 5, rgb(0.9, 0.9, 0.86));
    for k in 0..3 {
        prism(
            t,
            p + Vec3::Y * (1.0 + k as f32 * 1.2) * s,
            0.17 * s,
            0.12 * s,
            5,
            rgb(0.15, 0.15, 0.15),
        );
    }
    let g = if autumn {
        rgb(0.92, 0.78, 0.25)
    } else {
        pick(rng, &[rgb(0.45, 0.65, 0.25), rgb(0.5, 0.7, 0.3)])
    };
    blob(t, p + Vec3::Y * 5.2 * s, Vec3::new(1.7, 2.4, 1.7) * s, g);
}

pub fn bush(t: &mut Tris, p: Vec3, s: f32, rng: &mut Rng) {
    let g = pick(rng, &LEAVES);
    blob(
        t,
        p + Vec3::Y * 0.6 * s,
        Vec3::new(1.2, 0.9, 1.1) * s,
        shade(g, 0.85),
    );
}

pub fn rock(t: &mut Tris, p: Vec3, s: f32, rng: &mut Rng) {
    let grey = rng.range(0.42, 0.6);
    let r = Vec3::new(
        rng.range(0.8, 1.6),
        rng.range(0.5, 1.0),
        rng.range(0.8, 1.5),
    ) * s;
    blob(
        t,
        p + Vec3::Y * r.y * 0.4,
        r,
        rgb(grey, grey * 0.98, grey * 0.94),
    );
}

const WALLS: [Rgba; 6] = [
    rgb(0.95, 0.93, 0.88),
    rgb(0.9, 0.84, 0.7),
    rgb(0.66, 0.32, 0.24),
    rgb(0.75, 0.8, 0.85),
    rgb(0.93, 0.85, 0.55),
    rgb(0.55, 0.27, 0.2),
];
const ROOFS: [Rgba; 4] = [
    rgb(0.25, 0.23, 0.25),
    rgb(0.62, 0.27, 0.18),
    rgb(0.4, 0.28, 0.22),
    rgb(0.3, 0.33, 0.38),
];
const DARK: Rgba = rgb(0.12, 0.13, 0.16);

/// A house facing `yaw` (its local −Z towards the road).
pub fn house(t: &mut Tris, p: Vec3, yaw: f32, rng: &mut Rng) {
    let (hw, hd, h) = (
        rng.range(3.2, 4.8),
        rng.range(3.0, 4.2),
        rng.range(4.0, 6.5),
    );
    let r = yaw_rot(yaw);
    cuboid(
        t,
        p + Vec3::Y * h / 2.0,
        Vec3::new(hw, h / 2.0, hd),
        yaw,
        pick(rng, &WALLS),
    );
    // Ridge parallel to the road: the roof is turned a quarter, so its width
    // spans the house's depth.
    gable_roof(
        t,
        p + Vec3::Y * h,
        hd,
        hw,
        rng.range(2.2, 3.4),
        yaw + PI / 2.0,
        pick(rng, &ROOFS),
    );
    // Door and windows on the road side.
    let front = |x: f32, y: f32| p + r * Vec3::new(x, y, -hd - 0.05);
    cuboid(t, front(0.0, 1.1), Vec3::new(0.5, 1.1, 0.08), yaw, DARK);
    for x in [-hw * 0.6, hw * 0.6] {
        cuboid(
            t,
            front(x, 1.6),
            Vec3::new(0.45, 0.5, 0.08),
            yaw,
            rgb(0.55, 0.7, 0.85),
        );
        if h > 5.0 {
            cuboid(
                t,
                front(x, 3.9),
                Vec3::new(0.45, 0.5, 0.08),
                yaw,
                rgb(0.55, 0.7, 0.85),
            );
        }
    }
    cuboid(
        t,
        p + r * Vec3::new(hw * 0.5, h + 1.6, hd * 0.3),
        Vec3::new(0.35, 0.9, 0.35),
        yaw,
        shade(pick(rng, &WALLS), 0.7),
    );
}

pub fn barn(t: &mut Tris, p: Vec3, yaw: f32, rng: &mut Rng) {
    let (hw, hd, h) = (6.0, rng.range(8.0, 11.0), 5.5);
    let wall = pick(
        rng,
        &[
            rgb(0.55, 0.16, 0.12),
            rgb(0.42, 0.3, 0.2),
            rgb(0.35, 0.38, 0.33),
        ],
    );
    cuboid(
        t,
        p + Vec3::Y * h / 2.0,
        Vec3::new(hw, h / 2.0, hd),
        yaw,
        wall,
    );
    gable_roof(t, p + Vec3::Y * h, hw, hd, 4.0, yaw, rgb(0.22, 0.22, 0.24));
}

pub fn church(t: &mut Tris, p: Vec3, yaw: f32) {
    let r = yaw_rot(yaw);
    let stone = rgb(0.82, 0.79, 0.72);
    cuboid(t, p + Vec3::Y * 4.5, Vec3::new(5.0, 4.5, 10.0), yaw, stone);
    gable_roof(
        t,
        p + Vec3::Y * 9.0,
        5.0,
        10.0,
        5.0,
        yaw,
        rgb(0.3, 0.3, 0.33),
    );
    let tower = p + r * Vec3::new(0.0, 0.0, -12.5);
    cuboid(
        t,
        tower + Vec3::Y * 9.0,
        Vec3::new(2.8, 9.0, 2.8),
        yaw,
        stone,
    );
    cone(
        t,
        tower + Vec3::Y * 18.0,
        3.6,
        11.0,
        4,
        rgb(0.22, 0.32, 0.3),
    );
    cuboid(
        t,
        tower + Vec3::Y * 14.0 + r * Vec3::new(0.0, 0.0, -2.85),
        Vec3::new(1.0, 1.0, 0.05),
        yaw,
        rgb(0.95, 0.95, 0.9),
    );
}

pub fn cow(t: &mut Tris, p: Vec3, yaw: f32, rng: &mut Rng) {
    let r = yaw_rot(yaw);
    let coat = pick(
        rng,
        &[
            rgb(0.95, 0.95, 0.93),
            rgb(0.45, 0.3, 0.2),
            rgb(0.12, 0.12, 0.12),
        ],
    );
    cuboid(t, p + Vec3::Y * 1.05, Vec3::new(0.85, 0.42, 0.4), yaw, coat);
    cuboid(
        t,
        p + Vec3::Y * 1.1 + r * Vec3::new(0.3, 0.05, 0.0),
        Vec3::new(0.35, 0.3, 0.41),
        yaw,
        DARK,
    );
    cuboid(
        t,
        p + Vec3::Y * 1.2 + r * Vec3::new(1.05, 0.0, 0.0),
        Vec3::new(0.25, 0.22, 0.2),
        yaw,
        coat,
    );
    for (x, z) in [(-0.6, -0.25), (-0.6, 0.25), (0.6, -0.25), (0.6, 0.25)] {
        cuboid(
            t,
            p + Vec3::Y * 0.32 + r * Vec3::new(x, 0.0, z),
            Vec3::new(0.08, 0.32, 0.08),
            yaw,
            coat,
        );
    }
}

pub fn sheep(t: &mut Tris, p: Vec3, yaw: f32) {
    let r = yaw_rot(yaw);
    blob(
        t,
        p + Vec3::Y * 0.6,
        Vec3::new(0.6, 0.38, 0.4),
        rgb(0.93, 0.92, 0.86),
    );
    cuboid(
        t,
        p + Vec3::Y * 0.7 + r * Vec3::new(0.62, 0.0, 0.0),
        Vec3::new(0.16, 0.16, 0.14),
        yaw,
        DARK,
    );
}

pub fn hay_bale(t: &mut Tris, p: Vec3) {
    prism(t, p, 0.75, 1.2, 8, rgb(0.85, 0.74, 0.4));
}

pub fn reed(t: &mut Tris, p: Vec3, rng: &mut Rng) {
    for _ in 0..5 {
        let o = Vec3::new(rng.range(-0.6, 0.6), 0.0, rng.range(-0.6, 0.6));
        prism(
            t,
            p + o,
            0.04,
            rng.range(1.0, 1.8),
            3,
            rgb(0.45, 0.55, 0.25),
        );
    }
}

pub fn lamp_post(t: &mut Tris, p: Vec3, yaw: f32) {
    let r = yaw_rot(yaw);
    prism(t, p, 0.08, 5.0, 6, rgb(0.25, 0.27, 0.3));
    beam(
        t,
        p + Vec3::Y * 5.0,
        p + Vec3::Y * 5.0 + r * Vec3::new(0.0, 0.0, -1.2),
        0.1,
        0.1,
        rgb(0.25, 0.27, 0.3),
    );
    cuboid(
        t,
        p + Vec3::Y * 4.9 + r * Vec3::new(0.0, 0.0, -1.2),
        Vec3::new(0.2, 0.1, 0.3),
        yaw,
        rgb(0.95, 0.92, 0.75),
    );
}

/// A wooden fence run from `a` to `b`.
pub fn fence(t: &mut Tris, a: Vec3, b: Vec3, color: Rgba) {
    let len = a.distance(b);
    let posts = (len / 2.5).ceil().max(1.0) as usize;
    for k in 0..=posts {
        let p = a.lerp(b, k as f32 / posts as f32);
        cuboid(
            t,
            p + Vec3::Y * 0.55,
            Vec3::new(0.07, 0.55, 0.07),
            0.0,
            color,
        );
    }
    for y in [0.45, 0.9] {
        beam(t, a + Vec3::Y * y, b + Vec3::Y * y, 0.06, 0.1, color);
    }
}

/// Windmill body; returns the hub position and facing for the sails.
pub fn windmill(t: &mut Tris, p: Vec3, yaw: f32) -> (Vec3, f32) {
    let r = yaw_rot(yaw);
    prism(t, p, 3.6, 9.0, 8, rgb(0.42, 0.33, 0.27));
    prism(t, p + Vec3::Y * 9.0, 2.9, 4.0, 8, rgb(0.38, 0.3, 0.25));
    cone(t, p + Vec3::Y * 13.0, 3.3, 3.5, 8, rgb(0.2, 0.2, 0.22));
    beam(
        t,
        p + Vec3::Y * 2.5 + r * Vec3::new(0.0, 0.0, -3.6),
        p + Vec3::Y * 2.5 + r * Vec3::new(0.0, 0.0, -6.5),
        3.0,
        0.2,
        rgb(0.5, 0.4, 0.3),
    );
    (p + Vec3::Y * 13.0 + r * Vec3::new(0.0, 0.0, -3.4), yaw)
}
