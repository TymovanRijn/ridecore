//! The ground up close: terrain as one smooth-shaded mesh with colors per
//! corner (no visible triangles), a tiled detail texture so grass and rock
//! have grain, and grass tufts, flowers and stones along the verges.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use ridecore::race::course::Course;
use ridecore::rng::Rng;

use super::props::{self, Rgba, rgb};
use super::zones::Zone;
use super::{RoadIndex, SNOW_LINE_M, TREE_LINE_M, Tris, pose_v};

/// The detail texture repeats every this many meters.
const DETAIL_M: f32 = 3.5;

/// A terrain mesh on a grid of shared corners, so it can be smooth-shaded.
#[derive(Default)]
pub struct Grid {
    pos: Vec<Vec3>,
    idx: Vec<u32>,
    corners: HashMap<(i32, i32), u32>,
}

impl Grid {
    /// The corner at grid key `key`, placed at `p` the first time.
    pub fn corner(&mut self, key: (i32, i32), p: impl FnOnce() -> Vec3) -> u32 {
        *self.corners.entry(key).or_insert_with(|| {
            self.pos.push(p());
            self.pos.len() as u32 - 1
        })
    }

    /// Cell with corners a (i, j), b (i+1, j), c (i+1, j+1), d (i, j+1).
    pub fn cell(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.idx.extend([a, d, c, a, c, b]);
    }

    #[cfg(test)]
    pub fn corners(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    /// The mesh, each corner colored by `color(position, normal)`.
    pub fn mesh(self, color: impl Fn(Vec3, Vec3) -> Rgba) -> Mesh {
        let mut normals = vec![Vec3::ZERO; self.pos.len()];
        for t in self.idx.as_chunks::<3>().0 {
            let (a, b, c) = (
                self.pos[t[0] as usize],
                self.pos[t[1] as usize],
                self.pos[t[2] as usize],
            );
            // Area-weighted face normal.
            let n = (b - a).cross(c - a);
            for &i in t {
                normals[i as usize] += n;
            }
        }
        let normals: Vec<Vec3> = normals
            .into_iter()
            .map(|n| n.try_normalize().unwrap_or(Vec3::Y))
            .collect();
        let colors: Vec<Rgba> = self
            .pos
            .iter()
            .zip(&normals)
            .map(|(&p, &n)| color(p, n))
            .collect();
        let uvs: Vec<[f32; 2]> = self
            .pos
            .iter()
            .map(|p| [p.x / DETAIL_M, p.z / DETAIL_M])
            .collect();
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            self.pos.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            normals.iter().map(|n| n.to_array()).collect::<Vec<_>>(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(self.idx))
    }
}

/// Tileable value noise on a `period`-cell lattice over the unit square.
fn tile_noise(u: f32, v: f32, period: i32, seed: u32) -> f32 {
    let h = |x: i32, y: i32| {
        let (x, y) = (x.rem_euclid(period) as u32, y.rem_euclid(period) as u32);
        let mut k = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed;
        k = (k ^ (k >> 13)).wrapping_mul(0x5bd1_e995);
        (k ^ (k >> 15)) as f32 / u32::MAX as f32
    };
    let (x, y) = (u * period as f32, v * period as f32);
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = h(xi, yi) + (h(xi + 1, yi) - h(xi, yi)) * sx;
    let b = h(xi, yi + 1) + (h(xi + 1, yi + 1) - h(xi, yi + 1)) * sx;
    a + (b - a) * sy
}

/// A grey, tiling grain multiplied over the ground's colors, with mipmaps so
/// it doesn't shimmer in the distance.
pub fn detail_texture() -> Image {
    const SIZE: usize = 256;
    let mut level: Vec<f32> = Vec::with_capacity(SIZE * SIZE);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (u, v) = (x as f32 / SIZE as f32, y as f32 / SIZE as f32);
            let n = tile_noise(u, v, 6, 1) * 0.35
                + tile_noise(u, v, 16, 2) * 0.3
                + tile_noise(u, v, 48, 3) * 0.2
                + tile_noise(u, v, 128, 4) * 0.15;
            // Speckles of light and dark: blades, pebbles, clumps.
            let speck = tile_noise(u, v, 256, 5);
            let s = if speck > 0.86 {
                0.08
            } else if speck < 0.1 {
                -0.08
            } else {
                0.0
            };
            level.push((0.74 + n * 0.3 + s).clamp(0.0, 1.0));
        }
    }
    let mut data = Vec::new();
    let mut size = SIZE;
    let mut levels = 0;
    loop {
        for &g in &level {
            let b = (g * 255.0) as u8;
            data.extend([b, b, b, 255]);
        }
        levels += 1;
        if size == 1 {
            break;
        }
        let half = size / 2;
        level = (0..half * half)
            .map(|i| {
                let (x, y) = (i % half * 2, i / half * 2);
                (level[y * size + x]
                    + level[y * size + x + 1]
                    + level[(y + 1) * size + x]
                    + level[(y + 1) * size + x + 1])
                    / 4.0
            })
            .collect();
        size = half;
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels;
    image.data = Some(data);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

/// Grass blades: thin triangles lit as if they were the ground they grow
/// from, so they never look darker than the field around them.
#[derive(Default)]
pub struct Blades {
    pos: Vec<[f32; 3]>,
    col: Vec<Rgba>,
}

impl Blades {
    fn blade(&mut self, base: Vec3, side: Vec3, tip: Vec3, root: Rgba, top: Rgba) {
        self.pos.extend([
            (base - side).to_array(),
            (base + side).to_array(),
            tip.to_array(),
        ]);
        self.col.extend([root, root, top]);
    }

    /// Square tiles, so distant grass can be culled tile by tile.
    pub fn split(self, size: f32) -> Vec<Blades> {
        let mut tiles: HashMap<(i32, i32), Blades> = HashMap::new();
        for (p, c) in self
            .pos
            .as_chunks::<3>()
            .0
            .iter()
            .zip(self.col.as_chunks::<3>().0)
        {
            let t = tiles
                .entry((
                    (p[0][0] / size).floor() as i32,
                    (p[0][2] / size).floor() as i32,
                ))
                .or_default();
            t.pos.extend_from_slice(p);
            t.col.extend_from_slice(c);
        }
        tiles.into_values().collect()
    }

    pub fn mesh(self) -> Mesh {
        let n = self.pos.len();
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
    }
}

fn tint(c: Rgba, k: f32) -> Rgba {
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

const FLOWERS: [Rgba; 5] = [
    rgb(0.98, 0.86, 0.20),
    rgb(0.97, 0.97, 0.95),
    rgb(0.62, 0.42, 0.85),
    rgb(0.92, 0.30, 0.30),
    rgb(0.30, 0.50, 0.95),
];

/// Tufts of grass, wild flowers and loose stones on both verges, densest
/// right beside the road where the camera sees them.
pub fn verges(index: &RoadIndex, course: &Course, rng: &mut Rng) -> (Blades, Tris) {
    let mut blades = Blades::default();
    let mut stones = Tris::default();
    let lap = course.route.length_m();
    // Keep long courses affordable.
    let step = (lap * 2.0 / 120_000.0).max(0.35);
    let mut d = 0.0;
    while d < lap {
        let zone = index.zones.at(d);
        for side in [-1.0f32, 1.0] {
            if zone == Zone::Village && rng.chance(0.8) {
                continue;
            }
            // Mostly close to the road, some further out.
            let lateral = 5.2 + rng.f32().powi(3) * 30.0;
            let (p, _) = pose_v(course, d + rng.range(-0.5, 0.5) as f64, side * lateral);
            let n = index.near(p.x, p.z);
            let y = index.height_near(&n, p.x, p.z);
            if y < index.water_level + 0.6 || y > SNOW_LINE_M - 60.0 {
                continue;
            }
            let base = Vec3::new(p.x, y, p.z);
            let alpine = y > TREE_LINE_M;
            if rng.chance(if zone == Zone::Rocky || alpine {
                0.06
            } else {
                0.015
            }) {
                props::rock(&mut stones, base, rng.range(0.18, 0.45), rng);
                continue;
            }
            // The tuft takes the color of the ground it grows from, a bit
            // darker at the root and sunlit at the tips.
            let color = index.ground_color(base, 1.0);
            let height = if alpine || zone == Zone::Rocky {
                rng.range(0.15, 0.3)
            } else {
                rng.range(0.22, 0.5)
            };
            for _ in 0..rng.range(7.0, 12.0) as usize {
                let a = rng.range(0.0, std::f32::consts::TAU);
                let off = Vec3::new(a.cos(), 0.0, a.sin()) * rng.range(0.0, 0.3);
                let lean = Vec3::new(rng.range(-0.2, 0.2), 0.0, rng.range(-0.2, 0.2));
                let h = height * rng.range(0.6, 1.0);
                let w = Vec3::new(-a.sin(), 0.0, a.cos()) * rng.range(0.025, 0.04);
                blades.blade(
                    base + off - Vec3::Y * 0.05,
                    w,
                    base + off + Vec3::Y * h + lean * h,
                    tint(color, 0.85),
                    tint(color, rng.range(1.1, 1.35)),
                );
            }
            let flowery = matches!(zone, Zone::Meadow | Zone::Lake) || alpine;
            if flowery && rng.chance(0.35) {
                let c = *rng.pick(&FLOWERS);
                let top = base + Vec3::Y * height * 0.8;
                for k in 0..3 {
                    let a = k as f32 * 2.1;
                    let r = Vec3::new(a.cos(), 0.0, a.sin()) * 0.07;
                    blades.blade(top + r, r.cross(Vec3::Y), top + Vec3::Y * 0.05, c, c);
                }
            }
        }
        d += step;
    }
    (blades, stones)
}
