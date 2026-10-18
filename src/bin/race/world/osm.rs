//! The real landscape from OpenStreetMap: woods where the map has woods
//! (detailed trees near the road, simple ones further out, over a forest
//! floor the surface shader draws as treetops), the real buildings, side
//! roads and rivers.
//!
//! Data © OpenStreetMap contributors (ODbL).

use std::f32::consts::TAU;

use bevy::prelude::*;

use ridecore::map::{BuildingKind, Cover, MapData, RoadClass};
use ridecore::rng::Rng;

use super::ground::Grid;
use super::props::{self, Rgba, forest_floor, leaf, rgb};
use super::{ROAD_HALF, RoadIndex, Tris, fbm};

/// The road's own corridor: nothing from the map is built this close.
const CLEAR_M: f32 = ROAD_HALF + 2.5;

fn shade(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

/// Ground color by land cover; `None` where the map doesn't say.
pub fn cover_color(cover: Cover, p: Vec3) -> Option<Rgba> {
    let k = fbm(p.x / 14.0 + 31.0, p.z / 14.0) * 0.05;
    let m = fbm(p.x / 55.0, p.z / 55.0 - 9.0) * 0.5 + 0.5;
    Some(match cover {
        Cover::Unknown => return None,
        Cover::Forest => forest_floor(rgb(0.13 + k, 0.23 + 0.04 * m + k, 0.1)),
        Cover::Farmland => super::zones::field_color(p.x, p.z, 0x5eed),
        Cover::Meadow => rgb(0.31 + k + 0.04 * m, 0.55 + 0.08 * m + k, 0.21),
        // Gardens and hedges between the houses.
        Cover::Town => rgb(0.34 + k + 0.06 * m, 0.5 + k, 0.27 + 0.04 * m),
        Cover::Industry => rgb(0.46 + k, 0.45 + k, 0.42 + k),
        Cover::Water => rgb(0.30, 0.33, 0.22),
        Cover::Scrub => rgb(0.32 + k, 0.42 + k, 0.2),
        Cover::Rock => rgb(0.56 + k, 0.54 + k, 0.5 + k),
        Cover::Orchard => rgb(0.36 + k, 0.55 + 0.05 * m + k, 0.23),
    })
}

// Cheap trees: there are tens of thousands of them.
const SPRUCE: [Rgba; 3] = [
    leaf(0.08, 0.22, 0.12),
    leaf(0.1, 0.26, 0.14),
    leaf(0.07, 0.19, 0.11),
];
const BEECH: [Rgba; 4] = [
    leaf(0.2, 0.4, 0.15),
    leaf(0.26, 0.46, 0.17),
    leaf(0.17, 0.35, 0.13),
    leaf(0.3, 0.44, 0.16),
];
const BARK: Rgba = rgb(0.3, 0.22, 0.15);

/// `near`: full detail for trees the camera passes close by.
fn spruce(t: &mut Tris, p: Vec3, s: f32, near: bool, rng: &mut Rng) {
    let g = *rng.pick(&SPRUCE);
    props::prism(t, p, 0.25 * s, 2.2 * s, 3, BARK);
    // Tiers of drooping branches narrowing to a spire, some trees taller
    // and slimmer than others.
    let tall = rng.range(0.9, 1.3);
    let tiers = if near { 4 } else { 3 };
    for k in 0..tiers {
        let f = k as f32 / tiers as f32;
        props::cone(
            t,
            p + Vec3::Y * (1.3 + f * 7.2 * tall) * s,
            (2.4 - f * 1.55) / tall.sqrt() * s,
            (3.8 - f * 0.9) * tall * s,
            if near { 6 } else { 5 },
            shade(g, 0.9 + f * 0.25),
        );
    }
}

fn beech(t: &mut Tris, p: Vec3, s: f32, near: bool, rng: &mut Rng) {
    let g = *rng.pick(&BEECH);
    props::prism(t, p, 0.3 * s, 4.6 * s, 3, BARK);
    // A lumpy crown: one main mass and smaller ones beside it.
    let crown = p + Vec3::Y * 6.8 * s;
    props::blob(t, crown, Vec3::new(2.9, 2.5, 2.8) * s, g);
    for _ in 0..if near { 3 } else { 1 } {
        let a = rng.range(0.0, TAU);
        let o = Vec3::new(a.cos() * 2.1, rng.range(-0.9, 1.3), a.sin() * 2.1) * s;
        let r = rng.range(1.4, 2.0) * s;
        props::blob(
            t,
            crown + o,
            Vec3::splat(r),
            shade(g, rng.range(0.85, 1.15)),
        );
    }
}

/// A distant tree: a single cone, cheap enough to fill the hillsides.
fn far_tree(t: &mut Tris, p: Vec3, s: f32, conifer: bool, rng: &mut Rng) {
    if conifer {
        let g = *rng.pick(&SPRUCE);
        props::cone(t, p + Vec3::Y * s, 2.6 * s, 10.0 * s, 5, g);
    } else {
        let g = *rng.pick(&BEECH);
        props::cone(t, p + Vec3::Y * 1.5 * s, 4.2 * s, 8.0 * s, 5, g);
    }
}

/// Trees where the map has woods, densest along the road. Distant ones go
/// in `far`, which casts no shadows.
pub fn forest_trees(t: &mut Tris, far: &mut Tris, index: &RoadIndex, map: &MapData, rng: &mut Rng) {
    let (min, max) = index.extent(900.0);
    // Sample on a jittered grid; spacing grows with distance from the road.
    let cell = 7.0;
    let (nx, nz) = (
        ((max.x - min.x) / cell) as i32,
        ((max.y - min.y) / cell) as i32,
    );
    for j in 0..nz {
        for i in 0..nx {
            let x = min.x + (i as f32 + rng.f32()) * cell;
            let z = min.y + (j as f32 + rng.f32()) * cell;
            if map.cover(x, z) != Cover::Forest {
                continue;
            }
            let n = index.near(x, z);
            // Keep: every spot near the road, fewer further out.
            let keep = match n.dist {
                d if d < CLEAR_M + 1.0 => 0.0,
                d if d < 70.0 => 0.95,
                d if d < 160.0 => 0.35,
                d if d < 210.0 => 0.25,
                d if d < 600.0 => 0.06,
                _ => 0.0,
            };
            if !rng.chance(keep) {
                continue;
            }
            let p = Vec3::new(x, index.surface(x, z) - 0.2, z);
            // Spruce plantations and beech woods come in patches.
            let s = rng.range(0.85, 1.35);
            let conifer = fbm(x / 160.0 + 7.0, z / 160.0) > 0.0;
            match (conifer, n.dist) {
                (_, d) if d > 210.0 => far_tree(far, p, s * 1.15, conifer, rng),
                (true, d) => spruce(t, p, s, d < 45.0, rng),
                (false, d) => beech(t, p, s, d < 45.0, rng),
            }
        }
    }
}

// Walloon houses: grey limestone, brown sandstone, whitewash and brick
// under dark slate.
const WALLS: [Rgba; 6] = [
    rgb(0.62, 0.6, 0.56),
    rgb(0.55, 0.47, 0.38),
    rgb(0.9, 0.88, 0.82),
    rgb(0.58, 0.33, 0.26),
    rgb(0.68, 0.64, 0.58),
    rgb(0.5, 0.29, 0.23),
];
const ROOFS: [Rgba; 4] = [
    rgb(0.2, 0.22, 0.26),
    rgb(0.24, 0.26, 0.3),
    rgb(0.17, 0.18, 0.21),
    rgb(0.5, 0.24, 0.17),
];
const WINDOW: Rgba = rgb(0.12, 0.14, 0.18);

fn area(ring: &[Vec2]) -> f32 {
    let mut s = 0.0;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

/// Triangulates a simple polygon (counter-clockwise in x/z) by ear
/// clipping; falls back to a fan if it gets stuck.
fn triangulate(ring: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..ring.len()).collect();
    let mut out = Vec::new();
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut guard = 0;
    while idx.len() > 3 && guard < 4 * ring.len() {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for k in 0..n {
            let (ia, ib, ic) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            let (a, b, c) = (ring[ia], ring[ib], ring[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&m| {
                if m == ia || m == ib || m == ic {
                    return false;
                }
                let p = ring[m];
                cross(a, b, p) > 0.0 && cross(b, c, p) > 0.0 && cross(c, a, p) > 0.0
            });
            if !inside {
                out.push([ia, ib, ic]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    for k in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[k], idx[k + 1]]);
    }
    out
}

/// The real buildings near the road, extruded from their footprints: small
/// rectangular ones get a slate gable roof, the rest a flat or low roof.
pub fn buildings(t: &mut Tris, index: &RoadIndex, map: &MapData, rng: &mut Rng) {
    for b in &map.buildings {
        let mut ring: Vec<Vec2> = b.footprint.iter().map(|p| Vec2::new(p[0], p[1])).collect();
        // Drop repeated corners.
        ring.dedup_by(|a, b| a.distance(*b) < 0.3);
        if ring.len() < 3 {
            continue;
        }
        let mut a = area(&ring);
        if a.abs() < 12.0 || a.abs() > 60_000.0 {
            continue;
        }
        if a < 0.0 {
            ring.reverse();
            a = -a;
        }
        if ring
            .iter()
            .any(|p| index.near(p.x, p.y).dist < CLEAR_M + 0.5)
        {
            continue;
        }
        let heights: Vec<f32> = ring.iter().map(|p| index.surface(p.x, p.y)).collect();
        let base = heights.iter().copied().fold(f32::MAX, f32::min) - 0.4;
        let ground_top = heights.iter().copied().fold(f32::MIN, f32::max);
        let floors = b.levels.unwrap_or(match b.kind {
            BuildingKind::Shed => 1,
            _ if a > 400.0 => 3,
            _ => 2,
        }) as f32;
        let (wall_h, wall, roof) = match b.kind {
            BuildingKind::Church => (11.0, rgb(0.6, 0.58, 0.54), rgb(0.2, 0.22, 0.26)),
            BuildingKind::Shed if a > 500.0 => (
                7.0 + floors,
                shade(rgb(0.6, 0.62, 0.64), rng.range(0.8, 1.05)),
                rgb(0.42, 0.44, 0.46),
            ),
            BuildingKind::Shed => (3.5 * floors.max(1.0), *rng.pick(&WALLS), *rng.pick(&ROOFS)),
            BuildingKind::House => (2.9 * floors, *rng.pick(&WALLS), *rng.pick(&ROOFS)),
        };
        let top = ground_top + wall_h;
        let centroid = ring.iter().copied().sum::<Vec2>() / ring.len() as f32;
        let inside = Vec3::new(centroid.x, (base + top) / 2.0, centroid.y);
        // Walls, with a dark band of windows per floor on houses.
        let n = ring.len();
        for k in 0..n {
            let (p, q) = (ring[k], ring[(k + 1) % n]);
            let (p0, q0) = (Vec3::new(p.x, base, p.y), Vec3::new(q.x, base, q.y));
            let (p1, q1) = (Vec3::new(p.x, top, p.y), Vec3::new(q.x, top, q.y));
            // Counter-clockwise footprint: outward is to the right of p→q.
            props::quad_out(t, [p0, q0, q1, p1], inside, shade(wall, 0.95));
            let len = p.distance(q);
            if b.kind == BuildingKind::House && len > 3.0 {
                let out = {
                    let d = (q - p).normalize();
                    Vec3::new(d.y, 0.0, -d.x) * 0.04
                };
                for f in 0..floors as usize {
                    let y = ground_top + 1.0 + f as f32 * 2.9;
                    let (a0, b0) = (p0.with_y(y) + out, q0.with_y(y) + out);
                    let (a1, b1) = (a0 + Vec3::Y * 1.2, b0 + Vec3::Y * 1.2);
                    let inset = (q0 - p0).normalize() * 0.8;
                    props::quad_out(
                        t,
                        [a0 + inset, b0 - inset, b1 - inset, a1 + inset],
                        inside,
                        WINDOW,
                    );
                }
            }
        }
        // Roof.
        let simple = n == 4 && b.kind != BuildingKind::Shed || b.kind == BuildingKind::Church;
        let rect = if simple { oriented_rect(&ring) } else { None };
        match rect {
            Some((center, half_w, half_d, yaw)) if half_w < 9.0 => {
                let pitch = if b.kind == BuildingKind::Church {
                    1.1
                } else {
                    0.85
                };
                props::gable_roof(
                    t,
                    Vec3::new(center.x, top, center.y),
                    half_w,
                    half_d,
                    half_w * pitch,
                    yaw,
                    roof,
                );
                if b.kind == BuildingKind::Church {
                    // A tower at one end.
                    let (s, c) = yaw.sin_cos();
                    let end = center + Vec2::new(s, c) * (half_d - 2.0);
                    let foot = Vec3::new(end.x, base, end.y);
                    props::prism(t, foot, 2.6, top - base + 9.0, 4, wall);
                    props::cone(t, foot + Vec3::Y * (top - base + 9.0), 2.9, 8.0, 4, roof);
                }
            }
            _ => {
                for tri in triangulate(&ring) {
                    let [p, q, r] = tri.map(|i| Vec3::new(ring[i].x, top, ring[i].y));
                    props::tri_out(t, p, q, r, p - Vec3::Y * 5.0, roof);
                }
            }
        }
    }
}

/// Center, half width, half depth and yaw of a footprint that is close to
/// a rectangle, with the ridge along its longer side.
fn oriented_rect(ring: &[Vec2]) -> Option<(Vec2, f32, f32, f32)> {
    let (mut best_len, mut dir) = (0.0, Vec2::X);
    for k in 0..ring.len() {
        let e = ring[(k + 1) % ring.len()] - ring[k];
        if e.length() > best_len {
            best_len = e.length();
            dir = e.normalize();
        }
    }
    let side = dir.perp();
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in ring {
        let l = Vec2::new(p.dot(dir), p.dot(side));
        lo = lo.min(l);
        hi = hi.max(l);
    }
    let size = hi - lo;
    if area(ring).abs() < 0.8 * size.x * size.y {
        return None;
    }
    let mid = (lo + hi) / 2.0;
    let center = dir * mid.x + side * mid.y;
    // The ridge runs along `dir` (the long side): local depth axis.
    let (half_d, half_w) = (size.x / 2.0, size.y / 2.0);
    let yaw = dir.x.atan2(dir.y);
    Some((center, half_w, half_d, yaw))
}

/// Side roads and tracks draped over the ground, stopping short of the
/// race road.
pub fn side_roads(index: &RoadIndex, map: &MapData) -> Tris {
    let mut t = Tris::default();
    for road in &map.roads {
        let color = match road.class {
            RoadClass::Track => rgb(0.45, 0.4, 0.31),
            RoadClass::Motorway => rgb(0.25, 0.25, 0.27),
            _ => rgb(0.3, 0.3, 0.32),
        };
        let half = road.width / 2.0;
        // Resample every few meters so the strip follows the ground.
        let mut pts = Vec::new();
        for w in road.line.windows(2) {
            let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
            let steps = (a.distance(b) / 6.0).ceil().max(1.0) as usize;
            for s in 0..steps {
                pts.push(a.lerp(b, s as f32 / steps as f32));
            }
        }
        if let Some(last) = road.line.last() {
            pts.push(Vec2::from(*last));
        }
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if index.near(a.x, a.y).dist < CLEAR_M + half
                || index.near(b.x, b.y).dist < CLEAR_M + half
            {
                continue;
            }
            let Some(dir) = (b - a).try_normalize() else {
                continue;
            };
            let side = dir.perp() * half;
            let lift = 0.1;
            let at = |p: Vec2| Vec3::new(p.x, index.surface(p.x, p.y) + lift, p.y);
            let mid = (a + b) / 2.0;
            let below = Vec3::new(mid.x, index.surface(mid.x, mid.y) - 5.0, mid.y);
            props::quad_out(
                &mut t,
                [at(a - side), at(a + side), at(b + side), at(b - side)],
                below,
                color,
            );
        }
    }
    t
}

/// River and lake surfaces on the map's water cells.
pub fn water(index: &RoadIndex, map: &MapData) -> Grid {
    let geo = index.geo.as_ref().expect("real route");
    let [x0, z0, x1, z1] = geo.bounds();
    let step = 20.0;
    let (nx, nz) = (((x1 - x0) / step) as i32, ((z1 - z0) / step) as i32);
    let mut grid = Grid::default();
    // The water level at a corner: the lowest ground nearby, so the surface
    // stays flat-ish where the elevation data is noisy.
    let level = |x: f32, z: f32| {
        let mut lo = f32::MAX;
        for (dx, dz) in [
            (0.0, 0.0),
            (-25.0, 0.0),
            (25.0, 0.0),
            (0.0, -25.0),
            (0.0, 25.0),
        ] {
            if let Some(h) = geo.height(x + dx, z + dz) {
                lo = lo.min(h);
            }
        }
        lo + 0.1
    };
    for j in 0..nz {
        for i in 0..nx {
            let (x, z) = (x0 + (i as f32 + 0.5) * step, z0 + (j as f32 + 0.5) * step);
            if map.cover(x, z) != Cover::Water || index.near(x, z).dist < CLEAR_M + 4.0 {
                continue;
            }
            let at = |i: i32, j: i32| {
                let (x, z) = (x0 + i as f32 * step, z0 + j as f32 * step);
                Vec3::new(x, level(x, z), z)
            };
            let a = grid.corner((i, j), || at(i, j));
            let b = grid.corner((i + 1, j), || at(i + 1, j));
            let c = grid.corner((i + 1, j + 1), || at(i + 1, j + 1));
            let d = grid.corner((i, j + 1), || at(i, j + 1));
            grid.cell(a, b, c, d);
        }
    }
    grid
}

/// Cows in the meadows next to the road (Belgian Blues, mostly).
pub fn herds(t: &mut Tris, index: &RoadIndex, map: &MapData, rng: &mut Rng) {
    let (min, max) = index.extent(250.0);
    for _ in 0..1800 {
        let (x, z) = (rng.range(min.x, max.x), rng.range(min.y, max.y));
        if map.cover(x, z) != Cover::Meadow || map.share(x, z, 30.0, Cover::Meadow) < 0.9 {
            continue;
        }
        let n = index.near(x, z);
        if n.dist < 20.0 || n.dist > 220.0 {
            continue;
        }
        for _ in 0..rng.range(2.0, 7.0) as usize {
            let (px, pz) = (x + rng.range(-9.0, 9.0), z + rng.range(-9.0, 9.0));
            let p = Vec3::new(px, index.surface(px, pz), pz);
            props::cow(t, p, rng.range(0.0, TAU), rng);
        }
    }
}
