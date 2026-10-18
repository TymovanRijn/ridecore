//! Meshes for riders and bikes. Bodies are lofted from rings, so limbs
//! taper, chests are wider than waists and helmets have a tail; colors that
//! vary across a part (jersey designs, shoe soles, rim decals) are vertex
//! colors on a white material.
//!
//! Model space as in the parent module: forward is −Z, up is +Y, right is +X.
//! Limbs hang along −Y from their joint, like the bones they ride on.

use std::f32::consts::{PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

use super::{
    BAR, BB, CRANK, FOREARM, FRONT_AXLE, HEAD, HIP, REAR_AXLE, SADDLE, SHIN, SHOULDER, THIGH,
    UPPER_ARM, WHEEL_R,
};

type Rgba = [f32; 4];

const fn grey(v: f32) -> Rgba {
    [v, v, v, 1.0]
}

const WHITE: Rgba = grey(1.0);
const BLACK: Rgba = grey(0.03);
/// Multiplies the skin material down to black (shorts, gloves).
const DARK: Rgba = grey(0.02);

/// One cross-section of a loft: an oval around `c`, `rx` wide to each side,
/// `up` above and `down` below (in the ring's own frame).
#[derive(Clone, Copy)]
pub struct Ring {
    c: Vec3,
    rx: f32,
    up: f32,
    down: f32,
}

impl Ring {
    fn round(c: Vec3, r: f32) -> Self {
        Ring {
            c,
            rx: r,
            up: r,
            down: r,
        }
    }
}

/// Collects triangles with normals and vertex colors into one mesh.
#[derive(Default)]
pub struct Builder {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    col: Vec<Rgba>,
    idx: Vec<u32>,
}

impl Builder {
    /// A smooth closed tube through `rings`. Each ring's frame follows the
    /// path without twisting: `x` starts along `side`, and "up" is
    /// `x × tangent`, which also makes the triangles face outwards. `power` above 2 makes the cross-section boxier.
    /// `color` gets the position along the rings (0..1) and the angle
    /// around them (0 = +x side, π/2 = up).
    fn loft(&mut self, rings: &[Ring], side: Vec3, power: f32, color: impl Fn(f32, f32) -> Rgba) {
        const SIDES: usize = 18;
        let n = rings.len();
        let first = self.pos.len() as u32;
        let idx0 = self.idx.len();
        let mut x = side;
        for (i, r) in rings.iter().enumerate() {
            let t = (rings[(i + 1).min(n - 1)].c - rings[i.saturating_sub(1)].c).normalize();
            x = (x - t * x.dot(t)).normalize();
            let up = x.cross(t);
            let s = i as f32 / (n - 1) as f32;
            for k in 0..SIDES {
                let a = k as f32 / SIDES as f32 * TAU;
                let (sin, cos) = a.sin_cos();
                let e = 2.0 / power;
                let (ex, ey) = (
                    cos.signum() * cos.abs().powf(e),
                    sin.signum() * sin.abs().powf(e),
                );
                let h = if ey > 0.0 { r.up } else { r.down };
                self.pos
                    .push((r.c + x * r.rx * ex + up * h * ey).to_array());
                self.col.push(color(s, a));
            }
        }
        let at = |i: usize, k: usize| first + (i * SIDES + k % SIDES) as u32;
        for i in 0..n - 1 {
            for k in 0..SIDES {
                self.idx.extend([
                    at(i, k),
                    at(i + 1, k),
                    at(i + 1, k + 1),
                    at(i, k),
                    at(i + 1, k + 1),
                    at(i, k + 1),
                ]);
            }
        }
        // Caps.
        for (i, s) in [(0, 0.0), (n - 1, 1.0)] {
            let center = self.pos.len() as u32;
            self.pos.push(rings[i].c.to_array());
            self.col.push(color(s, 0.0));
            for k in 0..SIDES {
                if i == 0 {
                    self.idx.extend([center, at(i, k), at(i, k + 1)]);
                } else {
                    self.idx.extend([center, at(i, k + 1), at(i, k)]);
                }
            }
        }
        self.smooth_normals(first, idx0);
    }

    fn smooth_normals(&mut self, first: u32, idx0: usize) {
        let first = first as usize;
        let mut acc = vec![Vec3::ZERO; self.pos.len() - first];
        for t in self.idx[idx0..].chunks(3) {
            let p = |i: u32| Vec3::from_array(self.pos[i as usize]);
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            for &i in t {
                acc[i as usize - first] += n;
            }
        }
        self.nrm
            .extend(acc.into_iter().map(|n| n.normalize_or(Vec3::Y).to_array()));
    }

    /// A loft along `path` with radii from `keys` of `[s, rx, up, down]`,
    /// sampled `count` times, where `s` runs over 0..1 of the path.
    fn shaped(
        &mut self,
        path: impl Fn(f32) -> Vec3,
        keys: &[[f32; 4]],
        count: usize,
        side: Vec3,
        power: f32,
        color: impl Fn(f32, f32) -> Rgba,
    ) {
        let rings: Vec<Ring> = (0..count)
            .map(|i| {
                let s = keys[0][0]
                    + (keys[keys.len() - 1][0] - keys[0][0]) * i as f32 / (count - 1) as f32;
                let k = interpolate(keys, s);
                Ring {
                    c: path(s),
                    rx: k[1],
                    up: k[2],
                    down: k[3],
                }
            })
            .collect();
        let (s0, s1) = (keys[0][0], keys[keys.len() - 1][0]);
        self.loft(&rings, side, power, |t, a| color(s0 + (s1 - s0) * t, a));
    }

    /// A round tube from `a` to `b`, tapering from `ra` to `rb`.
    fn tube(&mut self, a: Vec3, b: Vec3, ra: f32, rb: f32, color: Rgba) {
        let side = if (b - a).normalize().x.abs() > 0.9 {
            Vec3::Y
        } else {
            Vec3::X
        };
        self.loft(
            &[Ring::round(a, ra), Ring::round(b, rb)],
            side,
            2.0,
            |_, _| color,
        );
    }

    /// A box with flat faces.
    fn cuboid(&mut self, t: Transform, size: Vec3, color: Rgba) {
        let mesh = Cuboid::from_size(size).mesh().build();
        self.add(&mesh, t, |_| color);
    }

    /// A flat ring around the X axis (a rim or a chainring), `width` thick.
    fn disc(
        &mut self,
        center: Vec3,
        r_in: f32,
        r_out: f32,
        width: f32,
        color: impl Fn(f32) -> Rgba,
    ) {
        let mesh = Extrusion::new(Annulus::new(r_in, r_out), width)
            .mesh()
            .resolution(48)
            .build();
        let t = Transform::from_translation(center).with_rotation(Quat::from_rotation_y(PI / 2.0));
        self.add(&mesh, t, |p| color(p.z.atan2(-p.y)));
    }

    /// Appends a primitive mesh, colored per vertex (by its model position).
    fn add(&mut self, mesh: &Mesh, t: Transform, color: impl Fn(Vec3) -> Rgba) {
        let first = self.pos.len() as u32;
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return;
        };
        let Some(VertexAttributeValues::Float32x3(nrm)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            return;
        };
        for (p, n) in pos.iter().zip(nrm) {
            let p = t.transform_point(Vec3::from_array(*p));
            self.pos.push(p.to_array());
            self.nrm
                .push((t.rotation * Vec3::from_array(*n)).to_array());
            self.col.push(color(p - t.translation));
        }
        match mesh.indices() {
            Some(i) => self.idx.extend(i.iter().map(|i| first + i as u32)),
            None => self.idx.extend(first..self.pos.len() as u32),
        }
    }

    pub fn build(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.nrm)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
        .with_inserted_indices(Indices::U32(self.idx))
    }
}

/// Values between keys `[s, …]`, eased so the shape has no kinks.
fn interpolate<const N: usize>(keys: &[[f32; N]], s: f32) -> [f32; N] {
    let i = keys
        .windows(2)
        .position(|w| s <= w[1][0])
        .unwrap_or(keys.len() - 2);
    let (a, b) = (keys[i], keys[i + 1]);
    let t = ((s - a[0]) / (b[0] - a[0])).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

fn limb(keys: &[[f32; 4]], color: impl Fn(f32, f32) -> Rgba) -> Mesh {
    let mut b = Builder::default();
    b.shaped(|s| Vec3::new(0.0, -s, 0.0), keys, 14, Vec3::X, 2.0, color);
    b.build()
}

/// How a team's jersey is laid out.
#[derive(Clone, Copy)]
pub enum Design {
    /// Accent-colored side panels.
    Panels,
    /// A band across the chest.
    Band,
    /// Accent shoulders and upper back.
    Yoke,
    /// A diagonal sash.
    Sash,
    /// One color only (the green jersey).
    Plain,
    /// White with red dots (the mountains jersey).
    Dots,
}

impl Design {
    pub fn for_team(team: usize) -> Self {
        [Design::Panels, Design::Band, Design::Yoke, Design::Sash][team % 4]
    }
}

/// The color that goes with a jersey: white on dark colors, near-black on
/// light ones.
pub fn accent(main: LinearRgba) -> LinearRgba {
    let lum = main.red * 0.3 + main.green * 0.59 + main.blue * 0.11;
    if lum > 0.35 {
        LinearRgba::rgb(0.012, 0.014, 0.02)
    } else {
        LinearRgba::rgb(0.85, 0.85, 0.85)
    }
}

/// The torso in a jersey of `main` and `second` and bib shorts, for the
/// cloth material. Runs along the spine from the saddle to the neck, leaning
/// forward.
pub fn torso(main: LinearRgba, second: LinearRgba, design: Design) -> Mesh {
    let (main, second) = (main.to_f32_array(), second.to_f32_array());
    let spine = SHOULDER - HIP;
    let len = spine.length();
    let along = spine / len;
    // Perpendicular to the spine, towards the back.
    let back = Vec3::X.cross(along);
    // [s, half width, back, front], and how far the back arches.
    let keys = [
        [-0.10, 0.03, 0.02, 0.02],
        [-0.05, 0.13, 0.07, 0.08],
        [0.05, 0.16, 0.08, 0.11],
        [0.25, 0.14, 0.065, 0.11],
        [0.5, 0.155, 0.075, 0.13],
        [0.72, 0.175, 0.08, 0.13],
        [0.88, 0.19, 0.075, 0.11],
        [0.97, 0.15, 0.055, 0.08],
        [1.04, 0.065, 0.05, 0.05],
        [1.07, 0.02, 0.02, 0.02],
    ];
    let hunch = |s: f32| (s.clamp(0.0, 1.0) * PI).sin() * 0.035;
    let mut b = Builder::default();
    b.shaped(
        |s| HIP + along * s * len + back * hunch(s),
        &keys,
        44,
        Vec3::X,
        2.6,
        |s, a| {
            let (y, x) = a.sin_cos();
            if s < 0.16 {
                return BLACK;
            }
            if s > 0.97 {
                return second;
            }
            // Race number on the lower back.
            if (0.24..0.42).contains(&s) && y > 0.55 && x.abs() < 0.42 {
                return [0.92, 0.92, 0.9, 1.0];
            }
            let accent = match design {
                Design::Panels => x.abs() > 0.8,
                Design::Band => (0.52..0.66).contains(&s),
                Design::Yoke => s > 0.74 && y > -0.3,
                Design::Sash => (s - 0.48 - 0.28 * x).abs() < 0.08,
                Design::Plain => false,
                Design::Dots => {
                    let u = s * 11.0;
                    let v = a / TAU * 16.0 + if u.floor() as i32 % 2 == 0 { 0.5 } else { 0.0 };
                    (u.fract() - 0.5).hypot(v.fract() - 0.5) < 0.3
                }
            };
            if accent { second } else { main }
        },
    );
    b.build()
}

/// The upper arm's short sleeve, for the jersey material.
pub fn sleeve() -> Mesh {
    limb(
        &[
            [-0.05, 0.02, 0.02, 0.02],
            [-0.02, 0.064, 0.064, 0.064],
            [0.1, 0.058, 0.058, 0.058],
            [0.16, 0.054, 0.054, 0.054],
            [0.175, 0.03, 0.03, 0.03],
        ],
        |_, _| WHITE,
    )
}

/// The upper arm, for the skin material.
pub fn upper_arm() -> Mesh {
    limb(
        &[
            [-0.03, 0.02, 0.02, 0.02],
            [0.0, 0.055, 0.055, 0.055],
            [0.12, 0.05, 0.05, 0.05],
            [0.24, 0.04, 0.04, 0.04],
            [UPPER_ARM, 0.036, 0.036, 0.036],
            [UPPER_ARM + 0.02, 0.02, 0.02, 0.02],
        ],
        |_, _| WHITE,
    )
}

/// The forearm with a dark glove at the end, for the skin material.
pub fn forearm() -> Mesh {
    let f = FOREARM;
    limb(
        &[
            [-0.02, 0.025, 0.025, 0.025],
            [0.0, 0.037, 0.037, 0.037],
            [0.08, 0.042, 0.04, 0.042],
            [f - 0.07, 0.029, 0.026, 0.026],
            [f - 0.03, 0.035, 0.03, 0.03],
            [f + 0.03, 0.04, 0.035, 0.035],
            [f + 0.06, 0.02, 0.02, 0.02],
        ],
        |s, _| if s > f - 0.05 { DARK } else { WHITE },
    )
}

/// The thigh in black shorts, the knee bare, for the skin material.
pub fn thigh() -> Mesh {
    limb(
        &[
            [-0.05, 0.03, 0.03, 0.03],
            [0.0, 0.085, 0.09, 0.08],
            [0.12, 0.085, 0.092, 0.08],
            [0.3, 0.068, 0.075, 0.065],
            [THIGH - 0.03, 0.052, 0.055, 0.05],
            [THIGH + 0.02, 0.03, 0.03, 0.03],
        ],
        |s, _| if s < THIGH - 0.1 { DARK } else { WHITE },
    )
}

/// The lower leg with its calf at the back, for the skin material.
pub fn shin() -> Mesh {
    limb(
        &[
            [-0.03, 0.03, 0.03, 0.03],
            [0.0, 0.05, 0.05, 0.05],
            [0.09, 0.054, 0.05, 0.068],
            [0.2, 0.048, 0.044, 0.055],
            [0.32, 0.036, 0.034, 0.036],
            [SHIN - 0.02, 0.03, 0.03, 0.03],
            [SHIN + 0.01, 0.02, 0.02, 0.02],
        ],
        |_, _| WHITE,
    )
}

/// A white sock over the ankle.
pub fn sock() -> Mesh {
    limb(
        &[
            [SHIN - 0.15, 0.03, 0.03, 0.03],
            [SHIN - 0.13, 0.039, 0.039, 0.041],
            [SHIN - 0.02, 0.034, 0.034, 0.034],
            [SHIN + 0.02, 0.02, 0.02, 0.02],
        ],
        |_, _| WHITE,
    )
}

/// A cycling shoe with a black sole, heel at +Z, for the white material.
pub fn shoe() -> Mesh {
    let mut b = Builder::default();
    // [distance forward, half width, top, bottom] from heel to toe.
    let keys = [
        [-0.10, 0.012, 0.012, 0.012],
        [-0.09, 0.034, 0.05, 0.035],
        [-0.03, 0.041, 0.052, 0.035],
        [0.05, 0.043, 0.036, 0.035],
        [0.13, 0.037, 0.023, 0.03],
        [0.17, 0.012, 0.01, 0.015],
    ];
    b.shaped(
        |s| Vec3::new(0.0, -s * 0.06, -s),
        &keys,
        14,
        Vec3::X,
        2.4,
        |_, a| if a.sin() < -0.55 { grey(0.06) } else { WHITE },
    );
    b.build()
}

/// Head and neck, for the skin material.
pub fn head() -> Mesh {
    let mut b = Builder::default();
    let neck = SHOULDER + Vec3::new(0.0, -0.02, 0.03);
    b.tube(
        neck,
        HEAD + Vec3::new(0.0, -0.05, 0.035),
        0.056,
        0.05,
        WHITE,
    );
    let skull = Sphere::new(0.095).mesh().uv(20, 14);
    b.add(
        &skull,
        Transform::from_translation(HEAD).with_scale(Vec3::new(0.86, 1.0, 1.05)),
        |_| WHITE,
    );
    // Ears and nose, so heads turned to the camera aren't plain eggs.
    for side in [-1.0, 1.0] {
        b.add(
            &Sphere::new(0.022).mesh().uv(8, 6),
            Transform::from_translation(HEAD + Vec3::new(side * 0.08, -0.01, 0.005))
                .with_scale(Vec3::new(0.5, 1.0, 0.8)),
            |_| WHITE,
        );
    }
    b.tube(
        HEAD + Vec3::new(0.0, -0.005, -0.095),
        HEAD + Vec3::new(0.0, -0.03, -0.112),
        0.01,
        0.014,
        WHITE,
    );
    b.build()
}

/// An aero road helmet with vents and dark glasses below its brim, at the
/// head's center, for the team material.
pub fn helmet() -> Mesh {
    let mut b = Builder::default();
    // [distance forward, half width, top, bottom] from the tail to the brim.
    let keys = [
        [-0.19, 0.01, 0.01, 0.005],
        [-0.17, 0.05, 0.035, 0.01],
        [-0.12, 0.09, 0.07, 0.01],
        [-0.05, 0.11, 0.1, 0.01],
        [0.02, 0.114, 0.108, 0.01],
        [0.08, 0.106, 0.092, 0.01],
        [0.12, 0.085, 0.065, 0.01],
        [0.142, 0.04, 0.03, 0.01],
        [0.147, 0.01, 0.01, 0.005],
    ];
    let lift = |s: f32| 0.045 - (s + 0.02).powi(2) * 1.3;
    b.shaped(
        |s| HEAD + Vec3::new(0.0, lift(s), -s),
        &keys,
        22,
        Vec3::X,
        2.2,
        |s, a| {
            let (y, x) = a.sin_cos();
            let vent = y > 0.35 && (-0.13..0.1).contains(&s) && (x * 7.0).cos() > 0.4;
            if y < 0.0 {
                grey(0.08)
            } else if vent {
                grey(0.05)
            } else {
                WHITE
            }
        },
    );
    // Wraparound glasses.
    let glasses: Vec<Ring> = (0..9)
        .map(|i| {
            let x = (i as f32 / 8.0 - 0.5) * 0.17;
            Ring {
                c: HEAD + Vec3::new(x, 0.014, -0.106 + 6.5 * x * x),
                rx: 0.02,
                up: 0.004,
                down: 0.004,
            }
        })
        .collect();
    b.loft(&glasses, Vec3::Y, 2.0, |_, _| grey(0.02));
    b.build()
}

/// The frame and fork, for the team's frame material.
pub fn frame() -> Mesh {
    let mut b = Builder::default();
    let (ht_top, ht_bottom, cluster) = frame_points();
    b.tube(
        cluster + Vec3::Y * 0.01,
        ht_top - Vec3::Y * 0.025,
        0.018,
        0.019,
        WHITE,
    );
    b.tube(BB, ht_bottom + Vec3::Y * 0.025, 0.029, 0.024, WHITE);
    b.tube(BB, cluster + Vec3::Y * 0.015, 0.021, 0.018, WHITE);
    b.tube(ht_bottom, ht_top, 0.025, 0.022, WHITE);
    b.tube(
        BB - Vec3::X * 0.04,
        BB + Vec3::X * 0.04,
        0.026,
        0.026,
        WHITE,
    );
    for side in [-1.0, 1.0] {
        let x = Vec3::X * side;
        b.tube(BB + x * 0.03, REAR_AXLE + x * 0.062, 0.013, 0.008, WHITE);
        b.tube(
            cluster + x * 0.016,
            REAR_AXLE + x * 0.062,
            0.01,
            0.007,
            WHITE,
        );
        b.tube(
            ht_bottom + x * 0.03,
            FRONT_AXLE + x * 0.05,
            0.017,
            0.009,
            WHITE,
        );
    }
    b.build()
}

fn frame_points() -> (Vec3, Vec3, Vec3) {
    let ht_top = Vec3::new(0.0, 0.86, -0.37);
    let ht_bottom = Vec3::new(0.0, 0.71, -0.41);
    let up_seat = (SADDLE - BB).normalize();
    let cluster = BB + up_seat * ((0.83 - BB.y) / up_seat.y);
    (ht_top, ht_bottom, cluster)
}

/// Bars, stem, saddle, seatpost, drivetrain and a bidon, for the cloth
/// material: everything on the bike that isn't team-colored or spinning.
pub fn fittings() -> Mesh {
    let mut b = Builder::default();
    let (ht_top, _, cluster) = frame_points();
    let tape = grey(0.035);
    let stem_top = ht_top + Vec3::new(0.0, 0.03, -0.007);
    b.tube(ht_top, stem_top, 0.019, 0.019, tape);
    b.tube(stem_top, BAR, 0.017, 0.016, grey(0.06));

    // Drop bar from the left drop, over the tops, to the right drop.
    let half: Vec<Vec3> = [
        (0.0, 0.0, 0.0),
        (0.1, 0.0, 0.0),
        (0.19, 0.0, 0.0),
        (0.205, 0.006, -0.05),
        (0.205, 0.008, -0.085),
        (0.205, -0.03, -0.12),
        (0.205, -0.09, -0.115),
        (0.205, -0.125, -0.07),
        (0.205, -0.13, 0.0),
    ]
    .iter()
    .map(|&(x, y, z)| BAR + Vec3::new(x, y, z))
    .collect();
    let mut path: Vec<Vec3> = half
        .iter()
        .rev()
        .map(|p| p * Vec3::new(-1.0, 1.0, 1.0))
        .collect();
    path.extend(half.iter().skip(1));
    let rings: Vec<Ring> = smooth_path(&path, 4)
        .into_iter()
        .map(|p| Ring::round(p, 0.012))
        .collect();
    b.loft(&rings, Vec3::Y, 2.0, |_, _| tape);
    // Brake hoods and levers.
    for side in [-1.0, 1.0] {
        let h = BAR + Vec3::new(side * 0.205, 0.0, 0.0);
        b.tube(
            h + Vec3::new(0.0, 0.008, -0.07),
            h + Vec3::new(0.0, 0.032, -0.135),
            0.017,
            0.013,
            tape,
        );
        b.cuboid(
            Transform::from_translation(h + Vec3::new(0.0, -0.035, -0.14))
                .with_rotation(Quat::from_rotation_x(-0.25)),
            Vec3::new(0.01, 0.08, 0.014),
            grey(0.45),
        );
    }

    // Saddle and seatpost.
    b.tube(
        cluster,
        SADDLE - Vec3::Y * 0.015,
        0.0135,
        0.0135,
        grey(0.05),
    );
    let saddle = [
        [-0.13, 0.01, 0.008, 0.008],
        [-0.12, 0.068, 0.014, 0.012],
        [-0.05, 0.064, 0.016, 0.012],
        [0.03, 0.032, 0.013, 0.01],
        [0.13, 0.019, 0.01, 0.009],
        [0.155, 0.006, 0.005, 0.005],
    ];
    b.shaped(
        |s| SADDLE + Vec3::new(0.0, (s * 2.0).powi(2) * -0.01, -s),
        &saddle,
        12,
        Vec3::X,
        3.0,
        |_, _| grey(0.03),
    );

    // Chainring, cassette and chain, on the right.
    let ring_at = BB + Vec3::X * 0.065;
    b.disc(ring_at, 0.075, 0.105, 0.006, |_| grey(0.4));
    let cassette = REAR_AXLE + Vec3::X * 0.058;
    b.disc(cassette, 0.015, 0.05, 0.03, |_| grey(0.5));
    for (from, to) in [(0.105, 0.05), (-0.105, -0.05)] {
        b.tube(
            ring_at + Vec3::Y * from,
            cassette + Vec3::new(0.007, to, 0.0),
            0.004,
            0.004,
            grey(0.12),
        );
    }

    // A bidon on the down tube.
    let (_, ht_bottom, _) = frame_points();
    let down = (ht_bottom - BB).normalize();
    let above = Vec3::X.cross(down);
    let bottle = BB + down * 0.22 + above * 0.062;
    b.tube(bottle, bottle + down * 0.19, 0.034, 0.034, grey(0.9));
    b.tube(
        bottle + down * 0.19,
        bottle + down * 0.22,
        0.026,
        0.012,
        grey(0.05),
    );
    b.build()
}

/// Catmull–Rom through `points`, `steps` segments between each pair.
fn smooth_path(points: &[Vec3], steps: usize) -> Vec<Vec3> {
    let n = points.len();
    let p = |i: isize| points[i.clamp(0, n as isize - 1) as usize];
    let mut out = Vec::new();
    for i in 0..n as isize - 1 {
        let (p0, p1, p2, p3) = (p(i - 1), p(i), p(i + 1), p(i + 2));
        for k in 0..steps {
            let t = k as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out.push(points[n - 1]);
    out
}

/// A wheel around its axle (the X axis): tire, deep carbon rim with decals,
/// hub and spokes, for the cloth material.
pub fn wheel() -> Mesh {
    let mut b = Builder::default();
    let tire = Torus::new(WHEEL_R - 0.026, WHEEL_R)
        .mesh()
        .minor_resolution(10)
        .major_resolution(48)
        .build();
    b.add(
        &tire,
        Transform::from_rotation(Quat::from_rotation_z(PI / 2.0)),
        |_| grey(0.035),
    );
    let rim_in = WHEEL_R - 0.07;
    b.disc(Vec3::ZERO, rim_in, WHEEL_R - 0.02, 0.022, |a| {
        // Two pale logos on each side, so the turning wheel is visible.
        if (a * 2.0 / PI).rem_euclid(1.0) < 0.28 {
            grey(0.6)
        } else {
            grey(0.045)
        }
    });
    b.tube(Vec3::X * -0.05, Vec3::X * 0.05, 0.016, 0.016, grey(0.3));
    for k in 0..20 {
        let a = k as f32 / 20.0 * TAU;
        let flange = Vec3::new(if k % 2 == 0 { 0.03 } else { -0.03 }, 0.0, 0.0);
        let dir = Vec3::new(0.0, a.cos(), a.sin());
        let hub = flange + dir * 0.022;
        let rim = dir * (rim_in + 0.005);
        b.cuboid(
            Transform::from_translation((hub + rim) / 2.0)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, (rim - hub).normalize())),
            Vec3::new(0.0035, (rim - hub).length(), 0.0035),
            grey(0.08),
        );
    }
    b.build()
}

/// Both crank arms with pedals, around the bottom bracket.
pub fn cranks() -> Mesh {
    let mut b = Builder::default();
    // At crank angle 0 the right pedal is at the top and the left at the bottom.
    for side in [-1.0f32, 1.0] {
        let end = Vec3::new(side * 0.085, side * CRANK, 0.0);
        b.tube(Vec3::X * side * 0.07, end, 0.016, 0.012, grey(0.1));
        b.cuboid(
            Transform::from_translation(end + Vec3::X * side * 0.04),
            Vec3::new(0.07, 0.015, 0.08),
            grey(0.08),
        );
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions_and_normals(mesh: &Mesh) -> (Vec<Vec3>, Vec<Vec3>) {
        let get = |id| match mesh.attribute(id) {
            Some(VertexAttributeValues::Float32x3(v)) => {
                v.iter().map(|p| Vec3::from_array(*p)).collect()
            }
            _ => panic!("missing attribute"),
        };
        (get(Mesh::ATTRIBUTE_POSITION), get(Mesh::ATTRIBUTE_NORMAL))
    }

    /// Every triangle is wound the way its normals point, so none are
    /// culled from the outside.
    fn faces_agree_with_normals(mesh: &Mesh) -> bool {
        let (pos, nrm) = positions_and_normals(mesh);
        let idx: Vec<usize> = mesh.indices().unwrap().iter().collect();
        idx.chunks(3).all(|t| {
            let face = (pos[t[1]] - pos[t[0]]).cross(pos[t[2]] - pos[t[0]]);
            face.length() < 1e-9 || face.dot(nrm[t[0]] + nrm[t[1]] + nrm[t[2]]) > 0.0
        })
    }

    #[test]
    fn tubes_face_outwards() {
        let mut b = Builder::default();
        b.tube(Vec3::ZERO, Vec3::new(0.0, 0.3, -0.4), 0.05, 0.03, WHITE);
        let mesh = b.build();
        let (pos, nrm) = positions_and_normals(&mesh);
        let axis = Vec3::new(0.0, 0.3, -0.4).normalize();
        for (p, n) in pos.iter().zip(&nrm) {
            let radial = *p - axis * p.dot(axis);
            if radial.length() > 0.02 {
                assert!(radial.dot(*n) > 0.0);
            }
        }
        assert!(faces_agree_with_normals(&mesh));
    }

    #[test]
    fn body_and_bike_faces_agree_with_their_normals() {
        for mesh in [
            torso(LinearRgba::RED, LinearRgba::WHITE, Design::Sash),
            thigh(),
            shin(),
            forearm(),
            shoe(),
            head(),
            helmet(),
            frame(),
            fittings(),
            wheel(),
            cranks(),
        ] {
            assert!(faces_agree_with_normals(&mesh));
        }
    }

    #[test]
    fn interpolation_hits_the_keys() {
        let keys = [[0.0, 1.0], [1.0, 3.0], [2.0, 2.0]];
        assert_eq!(interpolate(&keys, 0.0), [0.0, 1.0]);
        assert_eq!(interpolate(&keys, 1.0), [1.0, 3.0]);
        assert_eq!(interpolate(&keys, 2.0), [2.0, 2.0]);
        assert!((interpolate(&keys, 0.5)[1] - 2.0).abs() < 1e-6);
    }
}
