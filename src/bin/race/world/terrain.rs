//! The surface material for the ground and the road: the world builder's
//! vertex colors, with grass grain, rock, snow and asphalt textures picked
//! per pixel by slope and height in `terrain.wesl`.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;

use super::ground::tile_noise;
use super::{SNOW_LINE_M, TREE_LINE_M};

pub type Surface = ExtendedMaterial<StandardMaterial, SurfaceExt>;

const SHADER: &str = "embedded://race/bin/race/world/terrain.wesl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SurfaceExt {
    /// Snow line, tree line, mode (0 ground, 1 road, 2 scenery), unused.
    #[uniform(100)]
    params: Vec4,
    #[texture(101)]
    #[sampler(102)]
    grain: Handle<Image>,
    #[texture(103)]
    #[sampler(104)]
    rock: Handle<Image>,
}

impl MaterialExtension for SurfaceExt {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

pub fn setup(app: &mut App) {
    embedded_asset!(app, "terrain.wesl");
    app.add_plugins(MaterialPlugin::<Surface>::default());
}

/// The ground, road and scenery materials for one world.
pub fn materials(
    images: &mut Assets<Image>,
    surfaces: &mut Assets<Surface>,
) -> [Handle<Surface>; 3] {
    let grain = images.add(grain_texture());
    let rock = images.add(rock_texture());
    let mut make = |mode: f32, roughness: f32| {
        surfaces.add(Surface {
            base: StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: roughness,
                reflectance: 0.2,
                ..default()
            },
            extension: SurfaceExt {
                params: Vec4::new(SNOW_LINE_M, TREE_LINE_M, mode, 0.0),
                grain: grain.clone(),
                rock: rock.clone(),
            },
        })
    };
    [make(0.0, 0.96), make(1.0, 0.8), make(2.0, 0.92)]
}

/// A tiling RGBA image with a full mip chain, from `texel(u, v)` in 0..1.
fn tiling(size: usize, srgb: bool, texel: impl Fn(f32, f32) -> [f32; 4]) -> Image {
    let mut level: Vec<[f32; 4]> = (0..size * size)
        .map(|i| {
            texel(
                (i % size) as f32 / size as f32,
                (i / size) as f32 / size as f32,
            )
        })
        .collect();
    let (mut data, mut s, mut levels) = (Vec::new(), size, 0);
    loop {
        for c in &level {
            data.extend(c.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8));
        }
        levels += 1;
        if s == 1 {
            break;
        }
        let half = s / 2;
        level = (0..half * half)
            .map(|i| {
                let (x, y) = (i % half * 2, i / half * 2);
                let px = [
                    level[y * s + x],
                    level[y * s + x + 1],
                    level[(y + 1) * s + x],
                    level[(y + 1) * s + x + 1],
                ];
                std::array::from_fn(|k| px.iter().map(|p| p[k]).sum::<f32>() / 4.0)
            })
            .collect();
        s = half;
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
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
        anisotropy_clamp: 16,
        ..default()
    });
    image
}

/// Three independent grains: fine blades and pebbles (r), medium clumps
/// (g) and soft large patches (b), each around 0.5.
fn grain_texture() -> Image {
    tiling(512, false, |u, v| {
        let speck = tile_noise(u, v, 512, 5);
        let speck = if speck > 0.84 {
            0.12
        } else if speck < 0.12 {
            -0.12
        } else {
            0.0
        };
        let fine = tile_noise(u, v, 16, 1) * 0.25
            + tile_noise(u, v, 48, 2) * 0.25
            + tile_noise(u, v, 128, 3) * 0.25
            + tile_noise(u, v, 256, 4) * 0.25
            + speck;
        let mid = tile_noise(u, v, 4, 11) * 0.4
            + tile_noise(u, v, 12, 12) * 0.35
            + tile_noise(u, v, 32, 13) * 0.25;
        let soft = tile_noise(u, v, 3, 21) * 0.6 + tile_noise(u, v, 7, 22) * 0.4;
        [fine, mid, soft, 1.0]
    })
}

/// Weathered limestone: layered bands (along v, so they run level on
/// cliffs), cracks and lichen. Alpha holds a broad light/dark variation.
fn rock_texture() -> Image {
    tiling(512, true, |u, v| {
        let warp = tile_noise(u, v, 4, 31) * 0.08 + tile_noise(u, v, 12, 32) * 0.03;
        let band_v = v + warp;
        // Bands of different thickness and shade.
        let band = tile_noise(0.0, band_v, 23, 33) * 0.6 + tile_noise(0.5, band_v, 61, 34) * 0.4;
        let ridge = (band_v * 23.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let grit = tile_noise(u, v, 128, 35) * 0.5 + tile_noise(u, v, 256, 36) * 0.5;
        // Dark cracks along the bands and a few across them.
        let crack = {
            let c = (tile_noise(u, v, 24, 37) - 0.5).abs();
            if c < 0.025 { 0.55 } else { 1.0 }
        };
        let l = (0.42 + band * 0.32 + ridge * 0.06 + (grit - 0.5) * 0.18) * crack;
        // Warm grey, with greenish lichen and reddish iron stains in places.
        let lichen = (tile_noise(u, v, 9, 38) - 0.62).max(0.0) * 1.6;
        let iron = (tile_noise(u, v, 6, 39) - 0.65).max(0.0) * 1.4;
        let r = l * (1.0 + iron * 0.5 - lichen * 0.15);
        let g = l * (0.97 + lichen * 0.08 - iron * 0.1);
        let b = l * (0.9 - lichen * 0.2 - iron * 0.25);
        [r, g, b, tile_noise(u, v, 3, 40)]
    })
}
