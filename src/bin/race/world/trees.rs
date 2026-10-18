//! Trees with soft, rounded lighting: crowns are clusters of lumps whose
//! normals point out of the whole crown, so it shades like one leafy mass
//! instead of facets, and the inside and underside are darker, like the
//! shade inside a real tree. Spruces are tiers of drooping branch skirts.
//!
//! Each kind comes in three levels of detail, picked by distance from the
//! road: there are tens of thousands of trees.

use std::f32::consts::TAU;

use bevy::prelude::*;

use ridecore::rng::Rng;

use super::Tris;
use super::props::{Rgba, prism};

#[derive(Clone, Copy, PartialEq)]
pub enum Detail {
    /// Next to the road.
    Near,
    /// In view from the road.
    Mid,
    /// Filling the hillsides.
    Far,
}

fn shade(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, c[3]]
}

/// A triangle wound to face the way its normals point.
fn soft(t: &mut Tris, p: [Vec3; 3], n: [Vec3; 3], c: [Rgba; 3]) {
    let face = (p[1] - p[0]).cross(p[2] - p[0]);
    if face.dot(n[0] + n[1] + n[2]) >= 0.0 {
        t.tri_shaded(p, n, c);
    } else {
        t.tri_shaded([p[0], p[2], p[1]], [n[0], n[2], n[1]], [c[0], c[2], c[1]]);
    }
}

/// A broadleaf tree: trunk, a few limbs and a crown of lumps around
/// `crown` (its center, `radii` its half size), in leaf and bark colors.
pub fn leafy(
    t: &mut Tris,
    p: Vec3,
    crown: Vec3,
    radii: Vec3,
    detail: Detail,
    [color, bark]: [Rgba; 2],
    rng: &mut Rng,
) {
    let trunk_h = crown.y - p.y;
    let trunk_r = radii.x * 0.11;
    if detail != Detail::Far {
        prism(t, p, trunk_r, trunk_h, 5, bark);
    }
    if detail == Detail::Near {
        // Limbs reaching into the crown.
        for k in 0..3 {
            let a = k as f32 / 3.0 * TAU + rng.range(0.0, 1.5);
            let from = p + Vec3::Y * trunk_h * 0.65;
            let to = crown
                + Vec3::new(
                    a.cos() * radii.x * 0.6,
                    radii.y * 0.1,
                    a.sin() * radii.z * 0.6,
                );
            super::props::beam(t, from, to, trunk_r * 0.9, trunk_r * 0.9, shade(bark, 0.8));
        }
    }
    let (count, lon, lat) = match detail {
        Detail::Near => (5, 7, 4),
        Detail::Mid => (2, 6, 4),
        Detail::Far => (1, 5, 3),
    };
    // Far away there's no trunk: the crown reaches down to the ground.
    let (crown, radii) = if detail == Detail::Far {
        let top = crown.y + radii.y;
        (
            Vec3::new(crown.x, (p.y + top) / 2.0, crown.z),
            Vec3::new(radii.x, (top - p.y) / 2.0, radii.z),
        )
    } else {
        (crown, radii)
    };
    for k in 0..count {
        let (center, r) = if count == 1 {
            (crown, radii)
        } else if k == 0 {
            (crown, radii * 0.78)
        } else {
            // Lumps around the outside, more on top than below.
            let a = k as f32 / (count - 1) as f32 * TAU + rng.range(-0.4, 0.4);
            let up = rng.range(-0.35, 0.7);
            let dir = Vec3::new(
                a.cos() * (1.0 - up.abs() * 0.5),
                up,
                a.sin() * (1.0 - up.abs() * 0.5),
            );
            (crown + dir * radii * 0.5, radii * rng.range(0.42, 0.55))
        };
        lump(t, center, r, crown, radii, lon, lat, color, rng);
    }
}

/// A bush: a few lumps on the ground around `center`.
pub fn shrub(t: &mut Tris, center: Vec3, radii: Vec3, color: Rgba, rng: &mut Rng) {
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU + rng.range(0.0, 1.0);
        let o = Vec3::new(a.cos(), 0.1, a.sin()) * radii * 0.35;
        let r = radii * rng.range(0.6, 0.75);
        lump(t, center + o, r, center, radii, 7, 4, color, rng);
    }
}

/// One rounded lump of a crown. Normals blend the lump's own with the whole
/// crown's, and color darkens towards the crown's bottom and center.
#[allow(clippy::too_many_arguments)]
pub(super) fn lump(
    t: &mut Tris,
    center: Vec3,
    r: Vec3,
    crown: Vec3,
    radii: Vec3,
    lon: usize,
    lat: usize,
    color: Rgba,
    rng: &mut Rng,
) {
    let twist = rng.range(0.0, TAU);
    let tint = rng.range(0.9, 1.1);
    let vertex = |i: usize, j: usize| {
        let th = std::f32::consts::PI * i as f32 / lat as f32;
        let ph = TAU * j as f32 / lon as f32 + i as f32 * 0.5 + twist;
        let dir = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
        let pos = center + dir * r;
        let whole = ((pos - crown) / (radii * radii)).normalize_or(Vec3::Y);
        let n = (whole * 0.7 + dir * 0.3).normalize();
        // 0 at the crown's bottom, 1 at its top.
        let height = ((pos.y - crown.y) / radii.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let outer = ((pos - crown) / radii).length().min(1.0);
        let light = (0.42 + 0.45 * height + 0.25 * outer) * tint;
        (pos, n, shade(color, light.min(1.15)))
    };
    for i in 0..lat {
        for j in 0..lon {
            let (a, b, c, d) = (
                vertex(i, j),
                vertex(i, j + 1),
                vertex(i + 1, j + 1),
                vertex(i + 1, j),
            );
            if i > 0 {
                soft(t, [a.0, c.0, b.0], [a.1, c.1, b.1], [a.2, c.2, b.2]);
            }
            if i + 1 < lat {
                soft(t, [a.0, d.0, c.0], [a.1, d.1, c.1], [a.2, d.2, c.2]);
            }
        }
    }
}

/// A spruce or pine: tiers of drooping branch skirts narrowing to a spire.
/// `height` is from the ground to the tip.
pub fn conifer(
    t: &mut Tris,
    p: Vec3,
    height: f32,
    width: f32,
    detail: Detail,
    [color, bark]: [Rgba; 2],
    rng: &mut Rng,
) {
    let base = if detail == Detail::Far {
        0.0
    } else {
        height * 0.12
    };
    if detail != Detail::Far {
        prism(t, p, width * 0.08, base + height * 0.2, 4, bark);
    }
    let (tiers, tips) = match detail {
        Detail::Near => (5, 8),
        Detail::Mid => (4, 7),
        Detail::Far => (1, 6),
    };
    let crown = height - base;
    let twist = rng.range(0.0, TAU);
    for k in 0..tiers {
        let f = k as f32 / tiers as f32;
        // Each skirt hangs from the trunk above the one below it.
        let bottom = p.y + base + crown * f * 0.92;
        let top = if tiers == 1 {
            p.y + height
        } else {
            bottom + crown * (1.9 / tiers as f32 + 0.06)
        }
        .min(p.y + height);
        let r = width * (1.0 - f * 0.85);
        let droop = r * 0.28;
        let light = 0.55 + 0.45 * f;
        let apex = Vec3::new(p.x, top, p.z);
        let inner = Vec3::new(p.x, bottom + (top - bottom) * 0.25, p.z);
        let tip = |j: usize| {
            let a = TAU * j as f32 / tips as f32 + twist + k as f32 * 0.7;
            let reach = if j.is_multiple_of(2) { 1.0 } else { 0.8 };
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            (
                Vec3::new(p.x, bottom - droop * reach, p.z) + out * r * reach,
                out,
            )
        };
        for j in 0..tips {
            let ((p0, o0), (p1, o1)) = (tip(j), tip(j + 1));
            // Upper side, lit, normals leaning out of the cone.
            let n = |o: Vec3| (o * 0.75 + Vec3::Y * 0.65).normalize();
            soft(
                t,
                [apex, p1, p0],
                [Vec3::Y, n(o1), n(o0)],
                [
                    shade(color, light * 0.8),
                    shade(color, light * 1.08),
                    shade(color, light * 1.08),
                ],
            );
            // Underside, in shade: only seen from close by.
            if detail == Detail::Near {
                let under = |o: Vec3| (o * 0.6 - Vec3::Y * 0.5).normalize();
                soft(
                    t,
                    [inner, p0, p1],
                    [-Vec3::Y, under(o0), under(o1)],
                    [
                        shade(color, 0.3),
                        shade(color, light * 0.6),
                        shade(color, light * 0.6),
                    ],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upper_faces_agree(t: &Tris) -> bool {
        t.pos
            .as_chunks::<3>()
            .0
            .iter()
            .zip(t.nrm.as_chunks::<3>().0)
            .all(|(p, n)| {
                let [a, b, c] = p.map(Vec3::from_array);
                let face = (b - a).cross(c - a);
                let n = n.map(Vec3::from_array);
                face.length() < 1e-9 || face.dot(n[0] + n[1] + n[2]) > 0.0
            })
    }

    #[test]
    fn tree_triangles_face_the_way_they_are_lit() {
        let mut rng = Rng::new(3);
        for detail in [Detail::Near, Detail::Mid, Detail::Far] {
            let mut t = Tris::default();
            leafy(
                &mut t,
                Vec3::ZERO,
                Vec3::Y * 6.0,
                Vec3::new(3.0, 2.5, 3.0),
                detail,
                [[0.2, 0.4, 0.15, 0.5], [0.3, 0.2, 0.1, 1.0]],
                &mut rng,
            );
            conifer(
                &mut t,
                Vec3::X * 10.0,
                14.0,
                3.0,
                detail,
                [[0.1, 0.25, 0.12, 0.5], [0.3, 0.2, 0.1, 1.0]],
                &mut rng,
            );
            assert!(upper_faces_agree(&t));
        }
    }
}
