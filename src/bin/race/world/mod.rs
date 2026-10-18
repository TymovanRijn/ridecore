//! The procedural world around a course: terrain shaped and colored per
//! landscape zone, water, the road, roadside props, spectators, arches and
//! a distant horizon. Stylized low-poly, built once per race. Static props
//! are merged into a few large meshes so the scene stays fast.

mod ground;
mod osm;
mod props;
mod sky;
mod terrain;
pub use sky::HAZE;
mod zones;

use std::f32::consts::{PI, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilityRange;
use bevy::light::{CascadeShadowConfigBuilder, NotShadowCaster};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use std::sync::Arc;

use ridecore::map::{Cover, MapData};
use ridecore::race::course::{ClimbCategory, Course, SegmentKind};
use ridecore::rng::Rng;
use ridecore::route::Geo;

use crate::riders::RiderModel;
use crate::{RaceRes, Screen};
use ground::Grid;
use props::{Rgba, rgb};
use zones::{Zone, ZoneMap};

/// Marks everything that belongs to one race's world.
#[derive(Component)]
pub struct WorldPart;

const ROAD_HALF: f32 = 4.6;
const ROAD_LIFT: f32 = 0.12;
const SAMPLE_M: f64 = 4.0;
const CELL_M: f32 = 16.0;
/// Scenery is cut into tiles this big for culling.
const TILE_M: f32 = 220.0;
const MARGIN_M: f32 = 380.0;
/// Trees stop growing here; above it, alpine grass and rock.
const TREE_LINE_M: f32 = 2100.0;
const SNOW_LINE_M: f32 = 2700.0;
/// Real routes: fine terrain this far from the road, coarse beyond.
const CORRIDOR_M: f32 = 400.0;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        terrain::setup(app);
        app.insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.75, 0.85, 1.0),
            brightness: 450.0,
            ..default()
        })
        .add_systems(Startup, (spawn_sun, sky::spawn_dome))
        .add_systems(
            PostUpdate,
            sky::follow_camera.before(TransformSystems::Propagate),
        )
        .add_systems(OnEnter(Screen::Racing), spawn_world)
        .add_systems(OnEnter(Screen::Setup), despawn_world)
        .add_systems(
            Update,
            (turn_sails, cheer, sky::drift_clouds)
                .run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
        );
    }
}

fn spawn_sun(mut commands: Commands) {
    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            shadow_maps_enabled: true,
            color: Color::srgb(1.0, 0.96, 0.88),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0).looking_at(-sky::sun_direction(), Vec3::Y),
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 25.0,
            maximum_distance: 160.0,
            ..default()
        }
        .build(),
    ));
}

fn despawn_world(mut commands: Commands, parts: Query<Entity, With<WorldPart>>) {
    for e in &parts {
        commands.entity(e).despawn();
    }
}

/// A road centerline sample: position, travel direction and lap distance.
#[derive(Clone, Copy)]
struct RoadPoint {
    pos: Vec3,
    dir: Vec3,
    d: f64,
}

/// What lies nearest to a ground point: the road, which side, which zone.
struct Near {
    dist: f32,
    /// Meters to the right of the road (negative = left).
    lateral: f32,
    road_elevation: f32,
    zone: Zone,
    seed: u64,
}

/// Nearest-road queries over road samples bucketed on a grid.
struct RoadIndex {
    pts: Vec<RoadPoint>,
    cells: std::collections::HashMap<(i32, i32), Vec<usize>>,
    bucket: f32,
    mean_elevation: f32,
    water_level: f32,
    zones: ZoneMap,
    /// Real terrain for routes built from map data.
    geo: Option<Arc<Geo>>,
}

impl RoadIndex {
    fn new(pts: Vec<RoadPoint>, zones: ZoneMap, geo: Option<Arc<Geo>>) -> Self {
        let bucket = 50.0;
        let mut cells: std::collections::HashMap<(i32, i32), Vec<usize>> = Default::default();
        for (i, p) in pts.iter().enumerate() {
            cells
                .entry((
                    (p.pos.x / bucket).floor() as i32,
                    (p.pos.z / bucket).floor() as i32,
                ))
                .or_default()
                .push(i);
        }
        let mean_elevation = pts.iter().map(|p| p.pos.y).sum::<f32>() / pts.len() as f32;
        let min_elevation = pts.iter().map(|p| p.pos.y).fold(f32::MAX, f32::min);
        Self {
            pts,
            cells,
            bucket,
            mean_elevation,
            // Real valleys must not flood: no generated lakes on real routes.
            water_level: if geo.is_some() {
                f32::MIN
            } else {
                min_elevation - 1.2
            },
            zones,
            geo,
        }
    }

    /// Nearest road point within ~150 m (beyond that: treated as distant).
    fn near(&self, x: f32, z: f32) -> Near {
        let (cx, cz) = (
            (x / self.bucket).floor() as i32,
            (z / self.bucket).floor() as i32,
        );
        let mut best: Option<(f32, usize)> = None;
        for dx in -3..=3 {
            for dz in -3..=3 {
                for &i in self.cells.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                    let p = self.pts[i].pos;
                    let d = ((p.x - x).powi(2) + (p.z - z).powi(2)).sqrt();
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, i));
                    }
                }
            }
        }
        match best {
            Some((dist, i)) => {
                let rp = self.pts[i];
                let right = Vec3::new(-rp.dir.z, 0.0, rp.dir.x);
                let (_, zone, seed) = self.zones.stretch(rp.d);
                Near {
                    dist,
                    lateral: (Vec3::new(x, rp.pos.y, z) - rp.pos).dot(right),
                    road_elevation: rp.pos.y,
                    zone,
                    seed,
                }
            }
            None => Near {
                dist: 400.0,
                lateral: 400.0,
                road_elevation: self.mean_elevation,
                zone: Zone::Meadow,
                seed: 0,
            },
        }
    }

    fn height(&self, x: f32, z: f32) -> f32 {
        self.height_near(&self.near(x, z), x, z)
    }

    /// The real landscape, when the route has map data.
    fn map(&self) -> Option<&MapData> {
        self.geo.as_ref()?.map()
    }

    /// The road's bounding box grown by `margin`.
    fn extent(&self, margin: f32) -> (Vec2, Vec2) {
        let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in &self.pts {
            min = min.min(Vec2::new(p.pos.x, p.pos.z));
            max = max.max(Vec2::new(p.pos.x, p.pos.z));
        }
        (min - Vec2::splat(margin), max + Vec2::splat(margin))
    }

    /// Height of the ground as drawn: the terrain mesh's triangles between
    /// its 16 m corners (real routes), so things stand on what you see.
    fn surface(&self, x: f32, z: f32) -> f32 {
        if self.geo.is_none() {
            return self.height(x, z);
        }
        let (fx, fz) = (x / CELL_M, z / CELL_M);
        let (i, j) = (fx.floor(), fz.floor());
        let (tx, tz) = (fx - i, fz - j);
        let h = |di: f32, dj: f32| self.height((i + di) * CELL_M, (j + dj) * CELL_M);
        let (a, c) = (h(0.0, 0.0), h(1.0, 1.0));
        // Cells are split along the a–c diagonal, as in the mesh.
        if tz > tx {
            let d = h(0.0, 1.0);
            a + (d - a) * tz + (c - d) * tx
        } else {
            let b = h(1.0, 0.0);
            a + (b - a) * tx + (c - b) * tz
        }
    }

    fn height_near(&self, n: &Near, x: f32, z: f32) -> f32 {
        let d = n.dist.min(400.0);
        if d < 6.5 {
            return n.road_elevation - 0.35;
        }
        // Real terrain, eased into the road surface beside the road.
        if let Some(geo) = self.geo.as_ref()
            && let Some(dem) = geo.height(x, z)
        {
            // River beds sit below the water surface drawn over them.
            let dem = match geo.map() {
                Some(m) if d > 14.0 && m.cover(x, z) == Cover::Water => dem - 2.5,
                _ => dem,
            };
            let road = n.road_elevation - 0.35;
            return road + (dem - road) * smoothstep(8.0, 60.0, d);
        }
        // Far from the road, blend towards the course's mean height.
        let base = n.road_elevation
            + (self.mean_elevation - n.road_elevation) * smoothstep(60.0, 150.0, d);
        let amplitude = match n.zone {
            Zone::Rocky => 1.4,
            Zone::Village | Zone::Farmland => 0.45,
            _ => 1.0,
        };
        let hills = (fbm(x / 380.0, z / 380.0) * 48.0 + fbm(x / 70.0 + 13.0, z / 70.0 - 7.0) * 4.0)
            * amplitude;
        let h = base - 0.35 + hills * smoothstep(7.0, 95.0, d);
        // Lakes lie to the right of the road in lake stretches.
        if n.zone == Zone::Lake && n.lateral > 0.0 && n.dist < 400.0 {
            let k = smoothstep(18.0, 48.0, n.lateral);
            return h + (self.water_level - 4.0 - h) * k;
        }
        h
    }

    fn ground_color(&self, p: Vec3, slope_y: f32) -> Rgba {
        let n = self.near(p.x, p.z);
        if p.y < self.water_level + 0.5 {
            return rgb(0.82, 0.76, 0.55); // sand and shallows
        }
        // Corners are 16 m apart and colors blend between them, so only
        // villages get a paved verge; elsewhere grass runs up to the road.
        if n.dist < 9.0 && n.zone == Zone::Village {
            return rgb(0.55, 0.53, 0.5);
        }
        // High mountains: snow above 2700 m, rock and alpine grass above the
        // tree line.
        if p.y > SNOW_LINE_M + fbm(p.x / 80.0, p.z / 80.0) * 120.0 {
            return rgb(0.94, 0.95, 0.98);
        }
        if slope_y < 0.86 || (p.y > TREE_LINE_M + 250.0 && slope_y < 0.93) {
            let g = 0.45 + fbm(p.x / 9.0, p.z / 9.0) * 0.1;
            return rgb(g, g * 0.97, g * 0.9); // bare rock on steep slopes
        }
        if p.y > TREE_LINE_M {
            let k = fbm(p.x / 20.0, p.z / 20.0) * 0.05;
            return rgb(0.45 + k, 0.55 + k, 0.33); // alpine grass
        }
        if let Some(c) = self
            .map()
            .and_then(|m| osm::cover_color(m.cover(p.x, p.z), p))
        {
            return c;
        }
        let k = fbm(p.x / 15.0 + 50.0, p.z / 15.0) * 0.06;
        let m = fbm(p.x / 60.0, p.z / 60.0) * 0.5 + 0.5;
        let near_road = n.dist < 220.0;
        match n.zone {
            Zone::Farmland if near_road && n.dist > 11.0 => zones::field_color(p.x, p.z, n.seed),
            Zone::Forest if near_road => {
                props::forest_floor(rgb(0.2 + k, 0.36 + 0.06 * m + k, 0.16))
            }
            Zone::Rocky if near_road => rgb(0.42 + k, 0.47 + 0.05 * m + k, 0.3),
            Zone::Meadow if near_road => rgb(0.36 + k, 0.64 + 0.08 * m + k, 0.26),
            _ => rgb(0.30 + 0.12 * m + k, 0.52 + 0.14 * m + k, 0.22 + 0.06 * m),
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(x: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (z as u32).wrapping_mul(0xd816_3841);
    h = (h ^ (h >> 13)).wrapping_mul(0x5bd1_e995);
    (h ^ (h >> 15)) as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn value_noise(x: f32, z: f32) -> f32 {
    let (xi, zi) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x.floor(), z - z.floor());
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let a = hash(xi, zi) + (hash(xi + 1, zi) - hash(xi, zi)) * u;
    let b = hash(xi, zi + 1) + (hash(xi + 1, zi + 1) - hash(xi, zi + 1)) * u;
    a + (b - a) * v
}

fn fbm(x: f32, z: f32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.55, 1.0);
    for _ in 0..4 {
        sum += value_noise(x * freq, z * freq) * amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum
}

/// Collects triangles for a mesh; flat-shaded with per-vertex colors.
#[derive(Default)]
pub(crate) struct Tris {
    pos: Vec<[f32; 3]>,
    col: Vec<[f32; 4]>,
}

impl Tris {
    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3, color: [f32; 4]) {
        self.pos.extend([a.to_array(), b.to_array(), c.to_array()]);
        self.col.extend([color; 3]);
    }

    /// Quad a-b-c-d, counter-clockwise seen from its front. With travel
    /// direction `dir`, a road quad facing up is (right0, right1, left1, left0).
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: [f32; 4]) {
        self.tri(a, b, c, color);
        self.tri(a, c, d, color);
    }

    /// Splits into square tiles (by triangle center), so the renderer can
    /// skip tiles that are off screen or outside a shadow cascade.
    fn split(self, size: f32) -> Vec<Tris> {
        let mut tiles: std::collections::HashMap<(i32, i32), Tris> = Default::default();
        for (p, c) in self
            .pos
            .as_chunks::<3>()
            .0
            .iter()
            .zip(self.col.as_chunks::<3>().0)
        {
            let (x, z) = (
                (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                (p[0][2] + p[1][2] + p[2][2]) / 3.0,
            );
            let t = tiles
                .entry(((x / size).floor() as i32, (z / size).floor() as i32))
                .or_default();
            t.pos.extend_from_slice(p);
            t.col.extend_from_slice(c);
        }
        tiles.into_values().collect()
    }

    fn mesh(self) -> Mesh {
        let n = self.pos.len() as u32;
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
        .with_inserted_indices(Indices::U32((0..n).collect()));
        mesh.duplicate_vertices();
        mesh.compute_flat_normals();
        mesh
    }
}

fn lap_samples(course: &Course) -> Vec<f64> {
    let lap = course.route.length_m();
    let n = (lap / SAMPLE_M).ceil() as usize;
    (0..=n).map(|i| (i as f64 * SAMPLE_M).min(lap)).collect()
}

fn pose_v(course: &Course, d: f64, lateral: f32) -> (Vec3, Vec3) {
    let p = course.path.pose(&course.route, d, lateral);
    (Vec3::from_array(p.pos), Vec3::new(p.dir[0], 0.0, p.dir[1]))
}

/// Spawns one static piece of the world.
fn part<M: Material>(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mesh: Mesh,
    material: Handle<M>,
    shadows: bool,
) {
    let mut e = commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::default(),
        WorldPart,
    ));
    if !shadows {
        e.insert(NotShadowCaster);
    }
}

/// Spectators cheer harder the closer the riders are.
#[derive(Component)]
struct Spectator {
    base: Vec3,
    facing: Quat,
    phase: f32,
    flag: bool,
    /// How close the player is, 0..1, and whether to animate this frame.
    excite: f32,
    active: bool,
}

/// Windmill sails turning in the breeze.
#[derive(Component)]
struct Sails;

fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut surfaces: ResMut<Assets<terrain::Surface>>,
    mut fog: Query<&mut DistanceFog>,
    res: Res<RaceRes>,
) {
    let started = std::time::Instant::now();
    let course = &res.race.course;
    let ds = lap_samples(course);
    let center: Vec<RoadPoint> = ds
        .iter()
        .map(|&d| {
            let (pos, dir) = pose_v(course, d, 0.0);
            RoadPoint { pos, dir, d }
        })
        .collect();
    let geo = course.route.geo.clone();
    let mut zones = ZoneMap::new(course, 7, geo.is_none());
    if let Some(map) = geo.as_ref().and_then(|g| g.map()) {
        zones.adopt(|d| {
            let mut count = [0; 10];
            for lateral in [-40.0, -18.0, 18.0, 40.0] {
                let (p, _) = pose_v(course, d, lateral);
                count[map.cover(p.x, p.z) as usize] += 1;
            }
            let n = |c: Cover| count[c as usize];
            if n(Cover::Town) + n(Cover::Industry) >= 2 {
                Zone::Village
            } else if n(Cover::Forest) >= 2 {
                Zone::Forest
            } else if n(Cover::Farmland) >= 2 {
                Zone::Farmland
            } else if n(Cover::Rock) + n(Cover::Scrub) >= 2 {
                Zone::Rocky
            } else {
                Zone::Meadow
            }
        });
    }
    let index = RoadIndex::new(center, zones, geo.clone());

    let vertex_colored = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        ..default()
    });

    let [ground_surface, asphalt, scenery_surface] = terrain::materials(&mut images, &mut surfaces);
    let (road, lines) = road_meshes(course, &ds);
    part(&mut commands, &mut meshes, road, asphalt, false);
    part(
        &mut commands,
        &mut meshes,
        lines,
        vertex_colored.clone(),
        false,
    );

    // Ground: vertex colors, textured per pixel by the surface shader.
    let colored = |p: Vec3, n: Vec3| index.ground_color(p, n.y);
    let (min, max) = match &geo {
        Some(g) => {
            let (near, far, bounds) = real_terrain(&index, g);
            part(
                &mut commands,
                &mut meshes,
                near.mesh(colored),
                ground_surface.clone(),
                false,
            );
            if !far.is_empty() {
                part(
                    &mut commands,
                    &mut meshes,
                    far.mesh(colored),
                    ground_surface.clone(),
                    false,
                );
            }
            bounds
        }
        None => {
            let (ground, bounds) = generated_terrain(&index);
            part(
                &mut commands,
                &mut meshes,
                ground.mesh(colored),
                ground_surface.clone(),
                false,
            );
            // Water at one level everywhere; it only shows where the ground is lower.
            let water = materials.add(StandardMaterial {
                base_color: Color::srgba(0.16, 0.36, 0.52, 0.88),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.06,
                reflectance: 0.7,
                ..default()
            });
            let (lo, hi) = bounds;
            let y = index.water_level;
            let mut lake = Tris::default();
            lake.quad(
                Vec3::new(lo.x, y, lo.y),
                Vec3::new(lo.x, y, hi.y),
                Vec3::new(hi.x, y, hi.y),
                Vec3::new(hi.x, y, lo.y),
                [1.0; 4],
            );
            part(&mut commands, &mut meshes, lake.mesh(), water, false);
            bounds
        }
    };

    let mut scenery = Tris::default();
    let mut far_trees = Tris::default();
    let mut rng = Rng::new(11);
    let map = index.map();
    let sails = roadside(&mut scenery, &index, course, &mut rng);
    let (blades, stones) = ground::verges(&index, course, &mut rng);
    match map {
        // The real landscape: woods, buildings, roads, rivers and cows where
        // the map has them.
        Some(map) => {
            osm::forest_trees(&mut scenery, &mut far_trees, &index, map, &mut rng);
            osm::buildings(&mut scenery, &index, map, &mut rng);
            osm::herds(&mut scenery, &index, map, &mut rng);
            let roads = osm::side_roads(&index, map);
            for tile in roads.split(TILE_M) {
                part(
                    &mut commands,
                    &mut meshes,
                    tile.mesh(),
                    vertex_colored.clone(),
                    false,
                );
            }
            let river = osm::water(&index, map);
            if !river.is_empty() {
                let water = materials.add(StandardMaterial {
                    base_color: Color::srgba(0.18, 0.3, 0.36, 0.92),
                    alpha_mode: AlphaMode::Blend,
                    perceptual_roughness: 0.08,
                    reflectance: 0.6,
                    ..default()
                });
                part(
                    &mut commands,
                    &mut meshes,
                    river.mesh(|_, _| [1.0; 4]),
                    water,
                    false,
                );
            }
        }
        None => backdrop(&mut scenery, &index, min, max, &mut rng),
    }
    // In tiles, so the camera and each shadow cascade skip what they can't see.
    for tile in scenery.split(TILE_M) {
        part(
            &mut commands,
            &mut meshes,
            tile.mesh(),
            scenery_surface.clone(),
            true,
        );
    }
    // Distant woods: only the stretch around the camera, fading out
    // further away where the forest floor carries the look.
    for tile in far_trees.split(TILE_M) {
        commands.spawn((
            Mesh3d(meshes.add(tile.mesh())),
            MeshMaterial3d(scenery_surface.clone()),
            Transform::default(),
            WorldPart,
            NotShadowCaster,
            VisibilityRange {
                start_margin: 0.0..0.0,
                end_margin: 650.0..850.0,
                use_aabb: true,
            },
        ));
    }
    for tile in stones.split(TILE_M) {
        part(
            &mut commands,
            &mut meshes,
            tile.mesh(),
            vertex_colored.clone(),
            false,
        );
    }
    let grass = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        reflectance: 0.1,
        // Both sides drawn, but with the up-facing normal kept on the back
        // too (double_sided would flip it and make the blades dark).
        cull_mode: None,
        ..default()
    });
    // Grass only up close; it fades out by distance.
    for tile in blades.split(TILE_M) {
        commands.spawn((
            Mesh3d(meshes.add(tile.mesh())),
            MeshMaterial3d(grass.clone()),
            Transform::default(),
            WorldPart,
            NotShadowCaster,
            VisibilityRange {
                start_margin: 0.0..0.0,
                end_margin: 160.0..220.0,
                use_aabb: true,
            },
        ));
    }

    let sail_mesh = meshes.add(Cuboid::new(0.6, 7.5, 0.08));
    let sail_mat = materials.add(Color::srgb(0.92, 0.9, 0.85));
    for (hub, yaw) in sails {
        let rotor = commands
            .spawn((
                Sails,
                Transform::from_translation(hub).with_rotation(Quat::from_rotation_y(yaw)),
                Visibility::default(),
                WorldPart,
            ))
            .id();
        for k in 0..4 {
            let a = k as f32 * PI / 2.0;
            commands.spawn((
                Mesh3d(sail_mesh.clone()),
                MeshMaterial3d(sail_mat.clone()),
                Transform::from_rotation(Quat::from_rotation_z(a))
                    .with_translation(Quat::from_rotation_z(a) * Vec3::Y * 3.9),
                ChildOf(rotor),
            ));
        }
    }

    spawn_spectators(
        &mut commands,
        &mut meshes,
        &mut materials,
        &index,
        course,
        &mut rng,
    );
    spawn_arches(&mut commands, &mut meshes, &mut materials, course);
    let (sky_min, sky_max, ground_y) = match &geo {
        Some(g) => {
            let [x0, z0, x1, z1] = g.bounds();
            (Vec2::new(x0, z0), Vec2::new(x1, z1), g.edge_height())
        }
        None => (min, max, index.mean_elevation),
    };
    // Real mountains on the horizon when the route has them; otherwise a
    // ring of made-up peaks.
    let horizon = geo.as_deref().and_then(horizon_terrain);
    let real_horizon = horizon.is_some();
    if let Some(grid) = horizon {
        part(
            &mut commands,
            &mut meshes,
            grid.mesh(far_color),
            ground_surface.clone(),
            false,
        );
    }
    for mut fog in &mut fog {
        fog.falloff = crate::camera::air(if real_horizon {
            crate::camera::CLEAR_M
        } else {
            crate::camera::HAZY_M
        });
    }
    let (sky_center, sky_radius) = (
        (sky_min + sky_max) / 2.0,
        (sky_max - sky_min).length() / 2.0,
    );
    if !real_horizon {
        sky::mountains(
            &mut commands,
            &mut meshes,
            &mut materials,
            sky_center,
            sky_radius,
            ground_y,
            &mut rng,
        );
    }
    sky::spawn(
        &mut commands,
        &mut meshes,
        &mut materials,
        sky_center,
        sky_radius,
        ground_y,
        &mut rng,
    );
    if std::env::var_os("RIDECORE_SHOTS").is_some() {
        eprintln!("world built in {:.2} s", started.elapsed().as_secs_f32());
    }
}

/// Generated terrain: one fine grid over the course and a margin around it.
fn generated_terrain(index: &RoadIndex) -> (Grid, (Vec2, Vec2)) {
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in &index.pts {
        min = min.min(Vec2::new(p.pos.x, p.pos.z));
        max = max.max(Vec2::new(p.pos.x, p.pos.z));
    }
    min -= Vec2::splat(MARGIN_M);
    max += Vec2::splat(MARGIN_M);
    let (nx, nz) = (
        ((max.x - min.x) / CELL_M) as i32,
        ((max.y - min.y) / CELL_M) as i32,
    );
    let mut ground = Grid::default();
    let corner = |g: &mut Grid, i: i32, j: i32| {
        g.corner((i, j), || {
            let (x, z) = (min.x + i as f32 * CELL_M, min.y + j as f32 * CELL_M);
            Vec3::new(x, index.height(x, z), z)
        })
    };
    for j in 0..nz {
        for i in 0..nx {
            let a = corner(&mut ground, i, j);
            let b = corner(&mut ground, i + 1, j);
            let c = corner(&mut ground, i + 1, j + 1);
            let d = corner(&mut ground, i, j + 1);
            ground.cell(a, b, c, d);
        }
    }
    (ground, (min, max))
}

/// Real terrain: a fine mesh in a corridor along the road and a coarse one
/// out to the edge of the mapped area, slightly lower so the fine mesh wins
/// where they overlap. Returns both meshes and the corridor's bounds.
fn real_terrain(index: &RoadIndex, geo: &Geo) -> (Grid, Grid, (Vec2, Vec2)) {
    let reach = (CORRIDOR_M / CELL_M).ceil() as i32;
    let mut cells: std::collections::HashSet<(i32, i32)> = Default::default();
    for p in index.pts.iter().step_by(3) {
        let (ci, cj) = (
            (p.pos.x / CELL_M).floor() as i32,
            (p.pos.z / CELL_M).floor() as i32,
        );
        for di in -reach..=reach {
            for dj in -reach..=reach {
                if di * di + dj * dj <= reach * reach {
                    cells.insert((ci + di, cj + dj));
                }
            }
        }
    }
    let corner = |g: &mut Grid, i: i32, j: i32| {
        g.corner((i, j), || {
            let (x, z) = (i as f32 * CELL_M, j as f32 * CELL_M);
            Vec3::new(x, index.height(x, z), z)
        })
    };
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    let mut near = Grid::default();
    for &(i, j) in &cells {
        let a = corner(&mut near, i, j);
        let b = corner(&mut near, i + 1, j);
        let c = corner(&mut near, i + 1, j + 1);
        let d = corner(&mut near, i, j + 1);
        near.cell(a, b, c, d);
        let (x, z) = (i as f32 * CELL_M, j as f32 * CELL_M);
        min = min.min(Vec2::new(x, z));
        max = max.max(Vec2::new(x + CELL_M, z + CELL_M));
    }

    let s = geo.region_spacing();
    let [x0, z0, x1, z1] = geo.bounds();
    let mut far = Grid::default();
    let (ni, nj) = (((x1 - x0) / s) as i32, ((z1 - z0) / s) as i32);
    let at = |i: i32, j: i32| {
        let (x, z) = (x0 + i as f32 * s, z0 + j as f32 * s);
        geo.region_height(x, z).map(|h| Vec3::new(x, h - 1.5, z))
    };
    for j in 0..nj {
        for i in 0..ni {
            let center = Vec2::new(x0 + (i as f32 + 0.5) * s, z0 + (j as f32 + 0.5) * s);
            if index.near(center.x, center.y).dist < CORRIDOR_M - 60.0 {
                continue;
            }
            let (Some(a), Some(b), Some(c), Some(d)) =
                (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1))
            else {
                continue;
            };
            let a = far.corner((i, j), || a);
            let b = far.corner((i + 1, j), || b);
            let c = far.corner((i + 1, j + 1), || c);
            let d = far.corner((i, j + 1), || d);
            far.cell(a, b, c, d);
        }
    }
    (near, far, (min, max))
}

/// The real mountains beyond the mapped area, from the route's coarse
/// horizon grid. Under the mapped area it follows that terrain a little
/// lower, so the finer meshes cover it without a seam.
fn horizon_terrain(geo: &Geo) -> Option<Grid> {
    let (s, [x0, z0, x1, z1]) = geo.horizon()?;
    let [rx0, rz0, rx1, rz1] = geo.bounds();
    let inside = |x: f32, z: f32| x > rx0 && x < rx1 && z > rz0 && z < rz1;
    let pos = |i: i32, j: i32| (x0 + i as f32 * s, z0 + j as f32 * s);
    let mut grid = Grid::default();
    // The grid's last row and column have no cell beyond them.
    let (ni, nj) = (((x1 - x0) / s) as i32 - 1, ((z1 - z0) / s) as i32 - 1);
    for j in 0..nj {
        for i in 0..ni {
            let keys = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            if keys.iter().all(|&(a, b)| {
                let (x, z) = pos(a, b);
                inside(x, z)
            }) {
                continue;
            }
            let mut ids = [0; 4];
            for (k, &(a, b)) in keys.iter().enumerate() {
                let (x, z) = pos(a, b);
                let h = if inside(x, z) {
                    geo.region_height(x, z).map(|h| h - 4.0)
                } else {
                    None
                };
                let Some(h) = h.or_else(|| geo.horizon_height(x, z)) else {
                    break;
                };
                // The elevation data has the sea floor; draw the surface.
                let h = h.max(0.0);
                ids[k] = grid.corner((a, b), || Vec3::new(x, h, z));
            }
            grid.cell(ids[0], ids[1], ids[2], ids[3]);
        }
    }
    (!grid.is_empty()).then_some(grid)
}

/// Open water in the distance; the alpha tells the surface shader it's water.
const SEA: Rgba = [0.04, 0.12, 0.17, 0.0];

/// Distant slopes by height: farmland in the valleys, dark forest up the
/// sides, alpine grass above the tree line and scree towards the summits.
/// The surface shader adds the cliffs and the snow.
fn far_color(p: Vec3, n: Vec3) -> Rgba {
    if p.y < 0.5 {
        return SEA;
    }
    let k = fbm(p.x / 1500.0, p.z / 1500.0) * 0.05;
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| {
        let t = t.clamp(0.0, 1.0);
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let valley = [0.3 + k, 0.44 + k, 0.2];
    let forest = [0.11 + k, 0.2 + k, 0.09];
    let alpine = [0.36 + k, 0.42 + k, 0.25];
    let scree = [0.45 + k, 0.43 + k, 0.39];
    let flat = smoothstep(0.93, 0.99, n.y);
    let low = mix(
        forest,
        valley,
        flat * (1.0 - smoothstep(900.0, 1300.0, p.y)),
    );
    let c = if p.y < TREE_LINE_M - 200.0 {
        mix(
            low,
            alpine,
            smoothstep(TREE_LINE_M - 500.0, TREE_LINE_M - 200.0, p.y),
        )
    } else {
        mix(
            alpine,
            scree,
            smoothstep(TREE_LINE_M + 100.0, TREE_LINE_M + 500.0, p.y),
        )
    };
    rgb(c[0], c[1], c[2])
}

fn road_meshes(course: &Course, ds: &[f64]) -> (Mesh, Mesh) {
    let asphalt = [0.16, 0.16, 0.18, 1.0];
    let white = [0.95, 0.95, 0.95, 1.0];
    let mut road = Tris::default();
    let mut lines = Tris::default();
    for w in ds.windows(2) {
        let (d0, d1) = (w[0], w[1]);
        let up = Vec3::Y * ROAD_LIFT;
        let (l0, l1) = (
            pose_v(course, d0, -ROAD_HALF).0 + up,
            pose_v(course, d1, -ROAD_HALF).0 + up,
        );
        let (r0, r1) = (
            pose_v(course, d0, ROAD_HALF).0 + up,
            pose_v(course, d1, ROAD_HALF).0 + up,
        );
        road.quad(r0, r1, l1, l0, asphalt);
        let down = Vec3::Y * -2.0;
        road.quad(l0, l1, l1 + down, l0 + down, [0.3, 0.27, 0.22, 1.0]);
        road.quad(r0 + down, r1 + down, r1, r0, [0.3, 0.27, 0.22, 1.0]);
        let mark = Vec3::Y * (ROAD_LIFT + 0.015);
        for side in [-1.0f32, 1.0] {
            let (a, b) = (side * (ROAD_HALF - 0.45), side * (ROAD_HALF - 0.3));
            let (a0, a1) = (
                pose_v(course, d0, a).0 + mark,
                pose_v(course, d1, a).0 + mark,
            );
            let (b0, b1) = (
                pose_v(course, d0, b).0 + mark,
                pose_v(course, d1, b).0 + mark,
            );
            if side < 0.0 {
                lines.quad(a0, a1, b1, b0, white);
            } else {
                lines.quad(b0, b1, a1, a0, white);
            }
        }
        // Center dashes: 4 m on, 4 m off.
        if (d0 / SAMPLE_M) as i64 % 2 == 0 {
            let (a0, a1) = (
                pose_v(course, d0, -0.08).0 + mark,
                pose_v(course, d1, -0.08).0 + mark,
            );
            let (b0, b1) = (
                pose_v(course, d0, 0.08).0 + mark,
                pose_v(course, d1, 0.08).0 + mark,
            );
            lines.quad(b0, b1, a1, a0, white);
        }
    }
    (road.mesh(), lines.mesh())
}

/// A point `lateral` meters beside the road at lap distance `d`, on the ground.
fn ground_at(index: &RoadIndex, course: &Course, d: f64, lateral: f32) -> Vec3 {
    let (p, _) = pose_v(course, d, lateral);
    Vec3::new(p.x, index.height(p.x, p.z), p.z)
}

/// Props along both sides of the road, chosen by zone. Returns windmill
/// hubs for the animated sails.
fn roadside(t: &mut Tris, index: &RoadIndex, course: &Course, rng: &mut Rng) -> Vec<(Vec3, f32)> {
    let lap = course.route.length_m();
    let mut sails = Vec::new();
    let mut churches = 0;
    let mut last_mill = -1000.0f64;
    let step = 6.0;
    // With real map data the buildings, woods and herds come from the map.
    let invented = index.map().is_none();
    let mut d = 0.0;
    while d < lap {
        let zone = index.zones.at(d);
        let (_, dir) = pose_v(course, d, 0.0);
        let road_yaw = (-dir.z).atan2(dir.x);
        for side in [-1.0f32, 1.0] {
            // The building's front (local −Z) faces the road.
            let face_road = road_yaw + if side > 0.0 { 0.0 } else { PI } + PI / 2.0;
            let lake_side = zone == Zone::Lake && side > 0.0;
            match zone {
                Zone::Village => {
                    if invented && rng.chance(0.3) {
                        props::house(
                            t,
                            ground_at(index, course, d, side * rng.range(14.0, 18.0)),
                            face_road,
                            rng,
                        );
                    }
                    if (d / step) as i64 % 5 == 0 {
                        props::lamp_post(t, ground_at(index, course, d, side * 6.2), face_road);
                    }
                    if invented && churches == 0 && side > 0.0 && d > 150.0 && rng.chance(0.02) {
                        props::church(t, ground_at(index, course, d, 30.0), face_road);
                        churches += 1;
                    }
                }
                Zone::Farmland => {
                    if rng.chance(0.35) {
                        props::bush(
                            t,
                            ground_at(index, course, d, side * rng.range(8.5, 9.5)),
                            rng.range(0.9, 1.3),
                            rng,
                        );
                    }
                    if rng.chance(0.08) {
                        props::hay_bale(
                            t,
                            ground_at(index, course, d, side * rng.range(20.0, 110.0)),
                        );
                    }
                    if invented && rng.chance(0.012) {
                        let lat = side * rng.range(35.0, 80.0);
                        props::house(t, ground_at(index, course, d, lat), face_road, rng);
                        props::barn(
                            t,
                            ground_at(index, course, d + 22.0, lat * 1.05),
                            face_road,
                            rng,
                        );
                    }
                    if rng.chance(0.04) {
                        props::broadleaf(
                            t,
                            ground_at(index, course, d, side * rng.range(10.0, 14.0)),
                            rng.range(0.9, 1.3),
                            rng,
                            false,
                        );
                    }
                }
                Zone::Forest if invented => {
                    let autumn = fbm(d as f32 / 400.0, 3.0) > 0.1;
                    for _ in 0..3 {
                        let lat = side * rng.range(8.0, 130.0);
                        let p = ground_at(index, course, d + rng.range(-3.0, 3.0) as f64, lat);
                        let s = rng.range(0.8, 1.5);
                        match (rng.next_u64() % 3, autumn) {
                            (0, _) => {
                                if p.y < TREE_LINE_M {
                                    props::pine(t, p, s, rng)
                                }
                            }
                            (1, a) if p.y < TREE_LINE_M - 300.0 => {
                                props::broadleaf(t, p, s, rng, a)
                            }
                            (_, a) if p.y < TREE_LINE_M - 300.0 => props::birch(t, p, s, rng, a),
                            _ => {}
                        }
                    }
                }
                Zone::Forest => {}
                Zone::Meadow => {
                    if (d / step) as i64 % 3 == 0 {
                        let a = ground_at(index, course, d, side * 7.2);
                        let b = ground_at(index, course, d + step * 3.0, side * 7.2);
                        props::fence(t, a, b, rgb(0.48, 0.36, 0.25));
                    }
                    if invented && rng.chance(0.06) {
                        let herd = ground_at(index, course, d, side * rng.range(16.0, 90.0));
                        let sheep = rng.chance(0.4);
                        for _ in 0..rng.range(2.0, 6.0) as usize {
                            let p =
                                herd + Vec3::new(rng.range(-6.0, 6.0), 0.0, rng.range(-6.0, 6.0));
                            let p = Vec3::new(p.x, index.height(p.x, p.z), p.z);
                            if sheep {
                                props::sheep(t, p, rng.range(0.0, TAU));
                            } else {
                                props::cow(t, p, rng.range(0.0, TAU), rng);
                            }
                        }
                    }
                    if invented && d - last_mill > 900.0 && rng.chance(0.03) {
                        let p = ground_at(index, course, d, side * rng.range(45.0, 90.0));
                        sails.push(props::windmill(t, p, face_road));
                        last_mill = d;
                    }
                }
                Zone::Lake if lake_side => {
                    if rng.chance(0.3) {
                        let p = ground_at(index, course, d, rng.range(12.0, 22.0));
                        props::reed(t, p, rng);
                    }
                }
                Zone::Lake => {
                    if rng.chance(0.15) {
                        props::broadleaf(
                            t,
                            ground_at(index, course, d, side * rng.range(10.0, 60.0)),
                            rng.range(0.9, 1.4),
                            rng,
                            false,
                        );
                    }
                }
                Zone::Rocky => {
                    if rng.chance(0.25) {
                        props::rock(
                            t,
                            ground_at(index, course, d, side * rng.range(6.5, 70.0)),
                            rng.range(0.6, 2.4),
                            rng,
                        );
                    }
                    if rng.chance(0.2) {
                        props::pine(
                            t,
                            ground_at(index, course, d, side * rng.range(10.0, 90.0)),
                            rng.range(0.7, 1.3),
                            rng,
                        );
                    }
                }
            }
        }
        d += step;
    }

    // Finish straight: barriers on both sides of the last 250 m.
    // In lap coordinates the line is at the lap's end for either course type.
    let finish = lap;
    let mut b = finish - 250.0;
    while b < finish {
        for side in [-1.0f32, 1.0] {
            let p0 = ground_at(index, course, b, side * 5.4);
            let p1 = ground_at(index, course, b + 2.4, side * 5.4);
            let color = if ((b / 2.5) as i64) % 2 == 0 {
                rgb(0.95, 0.95, 0.95)
            } else {
                rgb(0.9, 0.35, 0.1)
            };
            props::beam(
                t,
                p0 + Vec3::Y * 0.55,
                p1 + Vec3::Y * 0.55,
                0.12,
                1.0,
                color,
            );
        }
        b += 2.5;
    }
    sails
}

/// Woods and copses away from the road, everywhere on the map.
fn backdrop(t: &mut Tris, index: &RoadIndex, min: Vec2, max: Vec2, rng: &mut Rng) {
    let mut placed = 0;
    for _ in 0..40_000 {
        if placed >= 6_000 {
            break;
        }
        let (x, z) = (rng.range(min.x, max.x), rng.range(min.y, max.y));
        let n = index.near(x, z);
        if n.dist < 130.0 || fbm(x / 140.0, z / 140.0) < -0.02 {
            continue;
        }
        if index.geo.as_ref().is_some_and(|g| !g.in_corridor(x, z)) {
            continue;
        }
        let y = index.height_near(&n, x, z);
        if y < index.water_level + 0.8 || y > TREE_LINE_M {
            continue;
        }
        let p = Vec3::new(x, y, z);
        let s = rng.range(0.8, 1.5);
        if rng.chance(0.5) {
            props::pine(t, p, s, rng);
        } else {
            props::broadleaf(t, p, s, rng, false);
        }
        placed += 1;
    }
}

/// Where the fans stand, as race distances on the first lap, so the crowd
/// can be heard (and heard cheering) as you ride past.
#[derive(Resource, Default)]
pub struct Fans {
    at: Vec<f64>,
    lap: f64,
    looped: bool,
}

impl Fans {
    /// How busy the roadside is around race distance `d`, 0..1.
    pub fn density(&self, d: f64) -> f32 {
        let d = if self.looped {
            d.rem_euclid(self.lap)
        } else {
            d
        };
        let lo = self.at.partition_point(|&x| x < d - 25.0);
        let hi = self.at.partition_point(|&x| x < d + 45.0);
        ((hi - lo) as f32 / 18.0).min(1.0)
    }
}

/// An arm that goes up when the riders come past.
#[derive(Component)]
struct Arm {
    side: f32,
}

/// Arm angle when raised, from hanging down.
const ARM_UP: f32 = PI - 0.55;

/// Crowds at the finish, sprint lines and KOM tops, lining the real climbs,
/// in the villages the road runs through, and small groups all along it.
fn spawn_spectators(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    index: &RoadIndex,
    course: &Course,
    rng: &mut Rng,
) {
    let lap = course.route.length_m();
    let body = meshes.add(Capsule3d::new(0.22, 0.75));
    let head = meshes.add(Sphere::new(0.14));
    let arm = meshes.add(Mesh::from(Capsule3d::new(0.065, 0.42)).translated_by(Vec3::Y * -0.28));
    let pole = meshes.add(Mesh::from(Cylinder::new(0.022, 1.6)).translated_by(Vec3::Y * -1.1));
    let band = meshes.add(Cuboid::new(0.3, 0.6, 0.02));
    let skin = materials.add(Color::srgb(0.87, 0.68, 0.55));
    let wood = materials.add(Color::srgb(0.45, 0.33, 0.22));
    let shirts: Vec<Handle<StandardMaterial>> = [
        Color::srgb(0.9, 0.2, 0.2),
        Color::srgb(0.2, 0.4, 0.9),
        Color::srgb(0.95, 0.85, 0.2),
        Color::srgb(1.0, 0.55, 0.0),
        Color::srgb(0.95, 0.95, 0.95),
        Color::srgb(0.2, 0.7, 0.3),
        Color::srgb(0.6, 0.3, 0.7),
        // Belgian black, and the Walloon cockerel's yellow and red.
        Color::srgb(0.08, 0.08, 0.09),
        Color::srgb(0.98, 0.82, 0.1),
    ]
    .into_iter()
    .map(|c| materials.add(c))
    .collect();
    let cloth = |materials: &mut Assets<StandardMaterial>, c: Color| {
        materials.add(StandardMaterial {
            base_color: c,
            double_sided: true,
            cull_mode: None,
            ..default()
        })
    };
    let (black, yellow, red, white, blue, green) = (
        cloth(materials, Color::srgb(0.06, 0.06, 0.07)),
        cloth(materials, Color::srgb(0.99, 0.84, 0.1)),
        cloth(materials, Color::srgb(0.85, 0.1, 0.12)),
        cloth(materials, Color::srgb(0.96, 0.96, 0.96)),
        cloth(materials, Color::srgb(0.1, 0.25, 0.65)),
        cloth(materials, Color::srgb(0.1, 0.55, 0.25)),
    );
    // Belgium (three times as likely), Wallonia, France, Italy.
    let flags = [
        [black.clone(), yellow.clone(), red.clone()],
        [black.clone(), yellow.clone(), red.clone()],
        [black, yellow.clone(), red.clone()],
        [yellow.clone(), yellow.clone(), yellow],
        [blue, white.clone(), red.clone()],
        [green, white, red],
    ];

    // (where, how many, how far before it they stand)
    let mut hotspots: Vec<(f64, usize, f32)> =
        vec![(if course.route.looped { 0.0 } else { lap }, 70, 90.0)];
    for seg in &course.segments {
        // Segments repeat every lap at the same spot; one crowd each.
        let d = if course.route.looped {
            seg.end_m.rem_euclid(lap)
        } else {
            seg.end_m
        };
        if hotspots.iter().all(|&(h, ..)| (h - d).abs() > 20.0) {
            hotspots.push((d, 35, 90.0));
        }
    }
    // The real climbs are lined with fans, thickest towards the top, as on
    // La Redoute.
    for c in course.climbs.iter().filter(|c| c.end_m <= lap + 1.0) {
        if c.name.is_none() && c.category == ClimbCategory::Uncategorized {
            continue;
        }
        let reach = (c.length_m() as f32 * 0.6).min(900.0);
        hotspots.push((c.end_m, (reach / 2.2) as usize, reach));
    }
    // Villages turn out to see the race come through.
    if let Some(map) = course.route.geo.as_ref().and_then(|g| g.map()) {
        let road: Vec<(f64, Vec3)> = (0..=(lap / 20.0) as usize)
            .map(|k| (k as f64 * 20.0, pose_v(course, k as f64 * 20.0, 0.0).0))
            .collect();
        for place in &map.places {
            if !matches!(place.kind.as_str(), "village" | "town" | "city" | "suburb") {
                continue;
            }
            let near = road.iter().min_by(|a, b| {
                let da = (a.1.x - place.pos[0]).hypot(a.1.z - place.pos[1]);
                let db = (b.1.x - place.pos[0]).hypot(b.1.z - place.pos[1]);
                da.total_cmp(&db)
            });
            if let Some(&(d, p)) = near
                && (p.x - place.pos[0]).hypot(p.z - place.pos[1]) < 350.0
            {
                hotspots.push((d + 90.0, 26, 220.0));
            }
        }
    }
    // A few people at the roadside everywhere else.
    let mut d = rng.range(80.0, 300.0) as f64;
    while d < lap - 50.0 {
        hotspots.push((d, 1 + (rng.next_u64() % 4) as usize, 10.0));
        d += rng.range(160.0, 420.0) as f64;
    }

    let mut at = Vec::new();
    let far = VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: 300.0..360.0,
        use_aabb: true,
    };
    for (center, count, before) in hotspots {
        for _ in 0..count {
            // Denser near the hotspot itself.
            let back = rng.f32().powf(1.6) * before;
            let d = center - back as f64 + rng.range(0.0, 30.0) as f64;
            if !(0.0..=lap).contains(&d) {
                continue;
            }
            at.push(d);
            let side = if rng.chance(0.5) { -1.0 } else { 1.0 };
            let p = ground_at(index, course, d, side * rng.range(5.5, 7.6));
            let road = ground_at(index, course, d + rng.range(-6.0, 6.0) as f64, 0.0);
            let facing = Transform::from_translation(p)
                .looking_at(Vec3::new(road.x, p.y, road.z), Vec3::Y)
                .rotation;
            let shirt = shirts[(rng.next_u64() % shirts.len() as u64) as usize].clone();
            let flag = rng
                .chance(0.18)
                .then(|| flags[(rng.next_u64() % flags.len() as u64) as usize].clone());
            let person = commands
                .spawn((
                    Spectator {
                        base: p,
                        facing,
                        phase: rng.range(0.0, TAU),
                        flag: flag.is_some(),
                        excite: 0.0,
                        active: false,
                    },
                    Transform::from_translation(p).with_rotation(facing),
                    Visibility::default(),
                    WorldPart,
                ))
                .id();
            commands.spawn((
                Mesh3d(body.clone()),
                MeshMaterial3d(shirt.clone()),
                Transform::from_xyz(0.0, 0.6, 0.0),
                far.clone(),
                ChildOf(person),
            ));
            commands.spawn((
                Mesh3d(head.clone()),
                MeshMaterial3d(skin.clone()),
                Transform::from_xyz(0.0, 1.32, 0.0),
                far.clone(),
                ChildOf(person),
            ));
            for s in [-1.0f32, 1.0] {
                // Flag wavers hold it up in their right hand all the time.
                let up = if flag.is_some() && s > 0.0 {
                    ARM_UP
                } else {
                    0.12
                };
                let shoulder = commands
                    .spawn((
                        Arm { side: s },
                        Mesh3d(arm.clone()),
                        MeshMaterial3d(shirt.clone()),
                        Transform::from_xyz(s * 0.25, 1.08, 0.0)
                            .with_rotation(Quat::from_rotation_z(s * up)),
                        far.clone(),
                        NotShadowCaster,
                        ChildOf(person),
                    ))
                    .id();
                if let (Some(colors), true) = (&flag, s > 0.0) {
                    commands.spawn((
                        Mesh3d(pole.clone()),
                        MeshMaterial3d(wood.clone()),
                        Transform::default(),
                        far.clone(),
                        NotShadowCaster,
                        ChildOf(shoulder),
                    ));
                    for (k, c) in colors.iter().enumerate() {
                        commands.spawn((
                            Mesh3d(band.clone()),
                            MeshMaterial3d(c.clone()),
                            Transform::from_xyz(0.16 + 0.3 * k as f32, -1.55, 0.0),
                            far.clone(),
                            NotShadowCaster,
                            ChildOf(shoulder),
                        ));
                    }
                }
            }
        }
    }
    at.sort_by(f64::total_cmp);
    commands.insert_resource(Fans {
        at,
        lap,
        looped: course.route.looped,
    });
}

type ArmParts = (&'static Arm, &'static ChildOf, &'static mut Transform);

/// Spectators jump, sway and throw their arms up when the player comes by.
/// Fans far away are left alone, so a long route costs nothing.
fn cheer(
    time: Res<Time>,
    res: Res<RaceRes>,
    riders: Query<(&RiderModel, &Transform)>,
    mut fans: Query<(&mut Spectator, &mut Transform), Without<RiderModel>>,
    mut arms: Query<ArmParts, (Without<Spectator>, Without<RiderModel>)>,
) {
    let Some((_, me)) = riders.iter().find(|(m, _)| m.index == res.race.human) else {
        return;
    };
    let t = time.elapsed_secs();
    for (mut fan, mut tf) in &mut fans {
        let near = (1.0 - fan.base.distance(me.translation) / 90.0).clamp(0.0, 1.0);
        fan.active = near > 0.0 || fan.excite > 0.0;
        fan.excite = near;
        if !fan.active {
            continue;
        }
        let jump = ((t * 9.0 + fan.phase).sin().max(0.0)) * 0.35 * near;
        let sway = near * (0.05 * (t * 2.0 + fan.phase).sin() + 0.1 * (t * 6.0 + fan.phase).sin());
        tf.translation = fan.base + Vec3::Y * jump;
        tf.rotation = fan.facing * Quat::from_rotation_z(sway);
    }
    for (arm, parent, mut tf) in &mut arms {
        let Ok((fan, _)) = fans.get(parent.parent()) else {
            continue;
        };
        if !fan.active {
            continue;
        }
        let near = fan.excite;
        let holds_flag = fan.flag && arm.side > 0.0;
        let raise = if holds_flag {
            1.0
        } else {
            (near * 1.6).min(1.0)
        };
        let wave = near * 0.4 * (t * 7.0 + fan.phase + arm.side).sin();
        tf.rotation = Quat::from_rotation_z(arm.side * (0.12 + raise * (ARM_UP - 0.12)) + wave);
    }
}

fn turn_sails(time: Res<Time>, mut sails: Query<&mut Transform, With<Sails>>) {
    for mut t in &mut sails {
        t.rotate_local_z(time.delta_secs() * 0.6);
    }
}

fn spawn_arches(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    course: &Course,
) {
    let lap = course.route.length_m();
    let post = meshes.add(Cylinder::new(0.22, 6.2));
    let banner = meshes.add(Cuboid::new(ROAD_HALF * 2.0 + 1.4, 1.3, 0.35));
    let line = meshes.add(Cuboid::new(ROAD_HALF * 2.0, 0.02, 0.6));
    let grey = materials.add(Color::srgb(0.85, 0.85, 0.88));
    let mut arch = |commands: &mut Commands, d: f64, color: Color, with_line: bool| {
        let (p, dir) = pose_v(course, d, 0.0);
        let rot = Transform::default().looking_to(dir, Vec3::Y).rotation;
        let right = Vec3::new(-dir.z, 0.0, dir.x);
        let mat = materials.add(StandardMaterial {
            base_color: color,
            emissive: color.to_linear() * 0.15,
            ..default()
        });
        for side in [-1.0, 1.0] {
            commands.spawn((
                Mesh3d(post.clone()),
                MeshMaterial3d(grey.clone()),
                Transform::from_translation(p + right * side * (ROAD_HALF + 0.6) + Vec3::Y * 3.1),
                WorldPart,
            ));
        }
        commands.spawn((
            Mesh3d(banner.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_translation(p + Vec3::Y * 5.8).with_rotation(rot),
            WorldPart,
        ));
        if with_line {
            commands.spawn((
                Mesh3d(line.clone()),
                MeshMaterial3d(mat),
                Transform::from_translation(p + Vec3::Y * (ROAD_LIFT + 0.02)).with_rotation(rot),
                WorldPart,
                NotShadowCaster,
            ));
        }
    };

    let finish_d = if course.route.looped { 0.0 } else { lap };
    arch(commands, finish_d, Color::srgb(0.95, 0.95, 0.95), true);
    // The flamme rouge: one kilometer to go.
    arch(
        commands,
        (finish_d - 1000.0).rem_euclid(lap),
        Color::srgb(0.85, 0.05, 0.05),
        false,
    );
    let mut done: Vec<i64> = vec![finish_d as i64];
    for seg in &course.segments {
        let d = if course.route.looped {
            seg.end_m.rem_euclid(lap)
        } else {
            seg.end_m
        };
        if done.iter().any(|&x| (x - d as i64).abs() < 20) {
            continue;
        }
        done.push(d as i64);
        let color = match seg.kind {
            SegmentKind::Sprint => Color::srgb(0.1, 0.75, 0.25),
            SegmentKind::Kom => Color::srgb(0.85, 0.12, 0.12),
        };
        arch(commands, d, color, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ridecore::route::{Route, real_routes};

    fn index_for(route: &Route) -> (Course, RoadIndex) {
        let course = Course::new(route.clone(), 1);
        let center = lap_samples(&course)
            .iter()
            .map(|&d| {
                let (pos, dir) = pose_v(&course, d, 0.0);
                RoadPoint { pos, dir, d }
            })
            .collect();
        let zones = ZoneMap::new(&course, 7, route.geo.is_none());
        let index = RoadIndex::new(center, zones, route.geo.clone());
        (course, index)
    }

    #[test]
    fn real_route_worlds_build_quickly_and_follow_the_road() {
        for id in ["alpe-dhuez", "grossglockner", "kust-noordwijk"] {
            let route = real_routes().iter().find(|r| r.id == id).unwrap();
            let started = std::time::Instant::now();
            let (course, index) = index_for(route);
            let geo = route.geo.as_ref().unwrap();
            let (near, far, _) = real_terrain(&index, geo);
            let mut props = Tris::default();
            let mut rng = Rng::new(1);
            roadside(&mut props, &index, &course, &mut rng);
            let secs = started.elapsed().as_secs_f32();
            assert!(
                near.corners() > 10_000 && far.corners() > 1_000,
                "{id}: terrain too small"
            );
            assert!(secs < 8.0, "{id}: world took {secs:.1} s to build");
            // Under the road the ground sits just below the road surface (it
            // follows the nearest 4 m road sample, so allow a little slack).
            for k in 1..20 {
                let (p, _) = pose_v(&course, course.total_m * k as f64 / 20.0, 0.0);
                let ground = index.height(p.x, p.z);
                let surface = p.y + ROAD_LIFT;
                assert!(
                    ground < surface && surface - ground < 0.9,
                    "{id}: ground {ground} road surface {surface}"
                );
            }
        }
    }
}
