// Ground and road surfaces: the vertex colors from the world builder,
// with grass, rock, scree and snow textures picked per pixel by slope and
// height, and asphalt grain on the road.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
};

struct Surface {
    // snow line (m), tree line (m), mode (0 ground, 1 road), unused
    params: vec4<f32>,
}

@group(3) @binding(100) var<uniform> surface: Surface;
@group(3) @binding(101) var grain_texture: texture_2d<f32>;
@group(3) @binding(102) var grain_sampler: sampler;
@group(3) @binding(103) var rock_texture: texture_2d<f32>;
@group(3) @binding(104) var rock_sampler: sampler;

// A grain value around 0.5 stretched to about -0.5..0.5.
fn contrast(v: f32, gain: f32) -> f32 {
    return clamp((v - 0.5) * gain, -0.5, 0.5);
}

fn grain(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(grain_texture, grain_sampler, uv);
}

// Rock projected from the three axes, so cliffs don't smear.
fn rock(p: vec3<f32>, n: vec3<f32>, scale: f32) -> vec4<f32> {
    var w = pow(abs(n), vec3(4.0));
    w = w / (w.x + w.y + w.z);
    // Side projections use -y as v, so the bands run level.
    let x = textureSample(rock_texture, rock_sampler, vec2(p.z, -p.y) / scale);
    let y = textureSample(rock_texture, rock_sampler, p.xz / scale);
    let z = textureSample(rock_texture, rock_sampler, vec2(p.x, -p.y) / scale);
    return x * w.x + y * w.y + z * w.z;
}

// Grain projected from the three axes, for solid objects.
fn grain3(p: vec3<f32>, n: vec3<f32>, scale: f32) -> vec4<f32> {
    var w = pow(abs(n), vec3(4.0));
    w = w / (w.x + w.y + w.z);
    return grain(p.zy / scale) * w.x + grain(p.xz / scale) * w.y + grain(p.xy / scale) * w.z;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let p = in.world_position.xyz;
    let n = normalize(in.world_normal);
    let base = pbr_input.material.base_color.rgb;
    var color = base;

    if surface.params.z > 1.5 {
        // Scenery: foliage (marked by alpha 0.5) gets clumps of needles and
        // leaves, with light catching some and shadow in the gaps.
        if pbr_input.material.base_color.a < 0.75 {
            let fine = contrast(grain3(p, n, 1.1).r, 2.4);
            let clumps = grain3(p, n, 3.2);
            let c = contrast(clumps.g, 2.2);
            let lum = dot(base, vec3(0.3, 0.59, 0.11));
            color = mix(vec3(lum), base, 0.85) * 0.72 * (0.8 + fine * 0.55) * (0.85 + c * 0.6);
            let tilt = vec3(contrast(clumps.r, 2.0), contrast(clumps.b, 2.0) * 0.5, c);
            pbr_input.N = normalize(pbr_input.N + tilt * 0.9);
        }
    } else if surface.params.z > 0.5 {
        // Asphalt: fine aggregate, patches of repair and wear.
        let fine = contrast(grain(p.xz / 1.1).g, 1.8);
        let patches = contrast(grain(p.xz / 29.0).b, 1.6);
        color = base * (0.9 + fine * 0.35) * (1.0 + patches * 0.3);
        pbr_input.material.perceptual_roughness = 0.82 - fine * 0.1;
    } else {
        // Natural, less candy-colored grass: a bit desaturated and olive.
        let lum = dot(base, vec3(0.3, 0.59, 0.11));
        var ground = mix(vec3(lum), base, 0.72) * vec3(0.95, 0.9, 0.72);
        // Large patches of lighter and darker vegetation break up the green:
        // dry, yellower grass on the light ones, lush and dark in the dips.
        let macro_a = contrast(grain(p.xz / 170.0).b, 2.2);
        let macro_b = contrast(grain(p.xz / 47.0 + vec2(0.37, 0.71)).b, 2.0);
        ground = ground * (0.92 + macro_a * 0.4 + macro_b * 0.25);
        let dry = smoothstep(0.0, 0.4, macro_a + macro_b * 0.5);
        ground = mix(ground, ground * vec3(1.25, 1.08, 0.62), dry * 0.6);
        // Grain at three scales, so the tiling isn't visible: blades and
        // bare soil up close, clumps and tufts further out.
        let fine = contrast(grain(p.xz / 2.3).r, 2.4);
        let mid = contrast(grain(p.xz / 9.0 + vec2(0.5, 0.25)).g, 2.2);
        let clumps = contrast(grain(p.xz / 31.0 + vec2(0.1, 0.6)).g, 2.0);
        ground = ground * (0.8 + fine * 0.6) * (0.92 + mid * 0.5) * (0.95 + clumps * 0.35);
        let soil = smoothstep(0.25, 0.45, -mid - fine * 0.5);
        ground = mix(ground, vec3(0.2, 0.15, 0.1) * (0.8 + fine), soil * 0.5);

        // Bare rock where it's steep, with strata on the cliffs; scree
        // (lighter, finer) at the foot of the steepest bits.
        let h = p.y;
        let slope = n.y;
        let noise = grain(p.xz / 23.0).b;
        let fine_snow = grain(p.xz / 2.7).r;
        let rocky_line = 0.80 + (noise - 0.5) * 0.1 - smoothstep(surface.params.y, surface.params.y + 500.0, h) * 0.1;
        let rock_mask = 1.0 - smoothstep(rocky_line - 0.06, rocky_line + 0.04, slope);
        let rock_color = rock(p, n, 7.0).rgb * (0.75 + rock(p, n, 53.0).a * 0.5);
        color = mix(ground, rock_color, rock_mask);

        // Snow above the snow line on all but the steepest faces, with a
        // ragged edge.
        let snow_line = surface.params.x + (noise - 0.5) * 260.0;
        let snow = smoothstep(snow_line, snow_line + 60.0, h) * smoothstep(0.45, 0.65, slope);
        color = mix(color, vec3(0.93, 0.95, 1.0) * (0.9 + fine_snow * 0.12), snow);
        pbr_input.material.perceptual_roughness = mix(0.96, 0.62, snow);
    }

    pbr_input.material.base_color = vec4(color, 1.0);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
