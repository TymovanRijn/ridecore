//! Courses as elevation profiles: (distance, elevation) points, optionally
//! looping. Real routes also carry their geography (GPS position of every
//! point and the terrain around the road) so the 3D world, the trainer and
//! the recorded FIT file all follow the real road.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use crate::map::MapData;

#[derive(Debug, Clone)]
pub struct Route {
    pub id: String,
    pub name: String,
    pub region: String,
    /// (distance m, elevation m), strictly increasing distance, starting at 0.
    pub points: Vec<(f64, f32)>,
    pub looped: bool,
    /// Real-world geography for routes built from map data.
    pub geo: Option<Arc<Geo>>,
}

/// Where a real route lies on Earth and the terrain around it.
#[derive(Debug)]
pub struct Geo {
    /// Projection origin (lat, lon); local x is east, z is south, in meters.
    pub origin: (f64, f64),
    /// (lat, lon) of every profile point.
    pub latlon: Vec<(f64, f64)>,
    corridor_spacing: f32,
    corridor: HashMap<(i32, i32), f32>,
    region: Region,
    /// OpenStreetMap landscape, parsed on first use.
    map_json: Option<&'static str>,
    map: OnceLock<Option<MapData>>,
}

/// A coarse rectangular height grid over the whole area.
#[derive(Debug)]
struct Region {
    spacing: f32,
    i0: i32,
    j0: i32,
    ni: usize,
    nj: usize,
    heights: Vec<f32>,
}

const M_PER_DEG_LAT: f64 = 110_540.0;
const M_PER_DEG_LON: f64 = 111_320.0;

impl Geo {
    /// Local meters (x east, z south) of a GPS position.
    pub fn project(&self, lat: f64, lon: f64) -> [f32; 2] {
        let (lat0, lon0) = self.origin;
        [
            ((lon - lon0) * lat0.to_radians().cos() * M_PER_DEG_LON) as f32,
            ((lat0 - lat) * M_PER_DEG_LAT) as f32,
        ]
    }

    /// Terrain height: the fine corridor grid near the road, the regional
    /// grid elsewhere, `None` outside the mapped area.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let s = self.corridor_spacing;
        let (fx, fz) = (x / s, z / s);
        let (i, j) = (fx.floor() as i32, fz.floor() as i32);
        let corners = [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)]
            .map(|k| self.corridor.get(&k).copied());
        if let [Some(a), Some(b), Some(c), Some(d)] = corners {
            let (tx, tz) = (fx - i as f32, fz - j as f32);
            return Some(bilerp(a, b, c, d, tx, tz));
        }
        self.region.height(x, z)
    }

    /// Whether the fine terrain grid covers this point.
    pub fn in_corridor(&self, x: f32, z: f32) -> bool {
        let s = self.corridor_spacing;
        self.corridor
            .contains_key(&((x / s).round() as i32, (z / s).round() as i32))
    }

    /// (min x, min z, max x, max z) of the mapped area in local meters.
    pub fn bounds(&self) -> [f32; 4] {
        let r = &self.region;
        let (x0, z0) = (r.i0 as f32 * r.spacing, r.j0 as f32 * r.spacing);
        [
            x0,
            z0,
            x0 + (r.ni - 1) as f32 * r.spacing,
            z0 + (r.nj - 1) as f32 * r.spacing,
        ]
    }

    /// The real landscape around the road, when the route has map data.
    pub fn map(&self) -> Option<&MapData> {
        self.map
            .get_or_init(|| self.map_json.and_then(|j| MapData::parse(j).ok()))
            .as_ref()
    }

    pub fn region_spacing(&self) -> f32 {
        self.region.spacing
    }

    /// Height from the coarse regional grid only (for distant terrain).
    pub fn region_height(&self, x: f32, z: f32) -> Option<f32> {
        self.region.height(x, z)
    }

    /// Average height along the edge of the mapped area.
    pub fn edge_height(&self) -> f32 {
        let r = &self.region;
        let edge: Vec<f32> = (0..r.ni)
            .flat_map(|i| [r.heights[i], r.heights[(r.nj - 1) * r.ni + i]])
            .chain((0..r.nj).flat_map(|j| [r.heights[j * r.ni], r.heights[j * r.ni + r.ni - 1]]))
            .collect();
        edge.iter().sum::<f32>() / edge.len() as f32
    }
}

impl Region {
    fn height(&self, x: f32, z: f32) -> Option<f32> {
        let fx = x / self.spacing - self.i0 as f32;
        let fz = z / self.spacing - self.j0 as f32;
        if fx < 0.0 || fz < 0.0 || fx >= (self.ni - 1) as f32 || fz >= (self.nj - 1) as f32 {
            return None;
        }
        let (i, j) = (fx as usize, fz as usize);
        let h = |i: usize, j: usize| self.heights[j * self.ni + i];
        Some(bilerp(
            h(i, j),
            h(i + 1, j),
            h(i, j + 1),
            h(i + 1, j + 1),
            fx - i as f32,
            fz - j as f32,
        ))
    }
}

fn bilerp(a: f32, b: f32, c: f32, d: f32, tx: f32, tz: f32) -> f32 {
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * tz
}

/// Great-circle distance in meters (same formula the route builder uses).
pub fn haversine(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6_371_008.8;
    let (la1, lo1, la2, lo2) = (
        a.0.to_radians(),
        a.1.to_radians(),
        b.0.to_radians(),
        b.1.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

impl Route {
    pub fn length_m(&self) -> f64 {
        self.points.last().map(|p| p.0).unwrap_or(0.0)
    }

    fn wrap(&self, d: f64) -> f64 {
        let len = self.length_m();
        if self.looped && len > 0.0 {
            d.rem_euclid(len)
        } else {
            d.clamp(0.0, len)
        }
    }

    /// Index of the segment containing `d` (already wrapped).
    fn segment(&self, d: f64) -> usize {
        self.points
            .partition_point(|p| p.0 <= d)
            .clamp(1, self.points.len() - 1)
            - 1
    }

    pub fn elevation_at(&self, d: f64) -> f32 {
        let d = self.wrap(d);
        let i = self.segment(d);
        let (d0, e0) = self.points[i];
        let (d1, e1) = self.points[i + 1];
        let t = ((d - d0) / (d1 - d0)) as f32;
        e0 + (e1 - e0) * t
    }

    /// Grade in percent at distance `d`.
    pub fn grade_pct_at(&self, d: f64) -> f32 {
        let d = self.wrap(d);
        let i = self.segment(d);
        let (d0, e0) = self.points[i];
        let (d1, e1) = self.points[i + 1];
        (e1 - e0) / (d1 - d0) as f32 * 100.0
    }

    pub fn finished(&self, d: f64) -> bool {
        !self.looped && d >= self.length_m()
    }

    /// GPS position at distance `d`, for real routes.
    pub fn latlon_at(&self, d: f64) -> Option<(f64, f64)> {
        let geo = self.geo.as_ref()?;
        let d = self.wrap(d);
        let i = self.segment(d);
        let t = (d - self.points[i].0) / (self.points[i + 1].0 - self.points[i].0);
        let (a, b) = (geo.latlon[i], geo.latlon[i + 1]);
        Some((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
    }

    /// Total climbing in meters.
    pub fn gain_m(&self) -> f32 {
        self.points
            .windows(2)
            .map(|w| (w[1].1 - w[0].1).max(0.0))
            .sum()
    }

    /// Steepest gradient (%) averaged over about 200 m.
    pub fn max_grade_pct(&self) -> f32 {
        let mut best = 0.0f32;
        for i in 0..self.points.len() {
            let j = self.points[i..]
                .iter()
                .position(|p| p.0 - self.points[i].0 >= 200.0)
                .map(|k| k + i);
            if let Some(j) = j {
                let (a, b) = (self.points[i], self.points[j]);
                best = best.max((b.1 - a.1) / (b.0 - a.0) as f32 * 100.0);
            }
        }
        best
    }

    /// Every course: the real-world routes first, then the generated ones.
    pub fn catalog() -> Vec<Route> {
        let mut all = real_routes().to_vec();
        all.extend(Self::builtin());
        for r in &mut all {
            r.name = crate::i18n::place(&r.name);
            r.region = crate::i18n::place(&r.region);
        }
        all
    }

    pub fn builtin() -> Vec<Route> {
        let r = |id: &str, name: &str, points, looped| Route {
            id: id.into(),
            name: name.into(),
            region: "Gegenereerd".into(),
            points,
            looped,
            geo: None,
        };
        vec![
            r(
                "flat-loop",
                "Flat Loop (10 km)",
                vec![(0.0, 10.0), (10_000.0, 10.0)],
                true,
            ),
            r(
                "rolling-hills",
                "Rolling Hills (14 km)",
                rolling(14_000.0, 7, 35.0),
                true,
            ),
            r(
                "mountain-climb",
                "Mountain Climb (12 km, 7%)",
                vec![
                    (0.0, 200.0),
                    (1_000.0, 205.0),
                    (4_000.0, 400.0),
                    (6_000.0, 560.0),
                    (9_000.0, 800.0),
                    (12_000.0, 1_000.0),
                ],
                false,
            ),
        ]
    }
}

/// Smooth sinusoidal hills sampled every 100 m.
fn rolling(length: f64, hills: u32, amplitude: f32) -> Vec<(f64, f32)> {
    let steps = (length / 100.0) as u32;
    (0..=steps)
        .map(|i| {
            let d = i as f64 * 100.0;
            let phase = (d / length) as f32 * hills as f32 * std::f32::consts::TAU;
            (d, 50.0 + amplitude * (1.0 - phase.cos()) / 2.0)
        })
        .collect()
}

// --- Real routes, built by tools/fetch_routes.py and embedded at compile time.

include!(concat!(env!("OUT_DIR"), "/routes.rs"));

#[derive(Deserialize)]
struct RouteFile {
    id: String,
    name: String,
    region: String,
    looped: bool,
    origin: [f64; 2],
    points: Vec<[f64; 3]>,
    terrain_spacing_m: f32,
    terrain: Vec<[f64; 3]>,
    area: RegionFile,
}

#[derive(Deserialize)]
struct RegionFile {
    spacing_m: f32,
    i0: i32,
    j0: i32,
    ni: usize,
    nj: usize,
    heights: Vec<f32>,
}

fn parse(json: &str) -> Result<Route, serde_json::Error> {
    let f: RouteFile = serde_json::from_str(json)?;
    let latlon: Vec<(f64, f64)> = f.points.iter().map(|p| (p[0], p[1])).collect();
    let mut points = Vec::with_capacity(f.points.len());
    let mut d = 0.0;
    for (k, p) in f.points.iter().enumerate() {
        if k > 0 {
            d += haversine(latlon[k - 1], latlon[k]);
        }
        points.push((d, p[2] as f32));
    }
    let corridor = f
        .terrain
        .iter()
        .map(|c| ((c[0] as i32, c[1] as i32), c[2] as f32))
        .collect();
    let r = f.area;
    Ok(Route {
        id: f.id.clone(),
        name: f.name,
        region: f.region,
        points,
        looped: f.looped,
        geo: Some(Arc::new(Geo {
            origin: (f.origin[0], f.origin[1]),
            latlon,
            corridor_spacing: f.terrain_spacing_m,
            corridor,
            region: Region {
                spacing: r.spacing_m,
                i0: r.i0,
                j0: r.j0,
                ni: r.ni,
                nj: r.nj,
                heights: r.heights,
            },
            map_json: MAP_FILES
                .iter()
                .find(|(id, _)| *id == f.id)
                .map(|(_, json)| *json),
            map: OnceLock::new(),
        })),
    })
}

/// The embedded real routes, parsed once.
pub fn real_routes() -> &'static [Route] {
    static ROUTES: OnceLock<Vec<Route>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        ROUTE_FILES
            .iter()
            .filter_map(|json| parse(json).ok())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple() -> Route {
        Route {
            id: "t".into(),
            name: "t".into(),
            region: "t".into(),
            points: vec![(0.0, 0.0), (1000.0, 50.0), (2000.0, 50.0)],
            looped: false,
            geo: None,
        }
    }

    #[test]
    fn grade_and_elevation_interpolate() {
        let r = simple();
        assert_eq!(r.grade_pct_at(500.0), 5.0);
        assert_eq!(r.elevation_at(500.0), 25.0);
        assert_eq!(r.grade_pct_at(1500.0), 0.0);
        assert!(r.finished(2000.0));
        assert_eq!(r.gain_m(), 50.0);
    }

    #[test]
    fn looped_routes_wrap() {
        let r = &Route::builtin()[1];
        let len = r.length_m();
        assert_eq!(r.elevation_at(250.0), r.elevation_at(250.0 + len));
        assert!(!r.finished(len * 3.0));
    }

    #[test]
    fn every_route_is_well_formed() {
        for r in Route::catalog() {
            assert_eq!(r.points[0].0, 0.0, "{}", r.name);
            assert!(r.points.windows(2).all(|w| w[1].0 > w[0].0), "{}", r.name);
            if let Some(g) = &r.geo {
                assert_eq!(g.latlon.len(), r.points.len(), "{}", r.name);
            }
        }
    }

    #[test]
    fn real_routes_have_gps_and_terrain_along_the_road() {
        for r in real_routes() {
            let g = r.geo.as_ref().unwrap();
            let mid = r.length_m() / 2.0;
            let (lat, lon) = r.latlon_at(mid).unwrap();
            let [x, z] = g.project(lat, lon);
            let ground = g.height(x, z).expect("terrain under the road");
            // The terrain model and the road profile agree within reason.
            assert!(
                (ground - r.elevation_at(mid)).abs() < 60.0,
                "{}: {ground} vs {}",
                r.name,
                r.elevation_at(mid)
            );
        }
    }

    #[test]
    fn every_embedded_route_parses() {
        for json in ROUTE_FILES {
            parse(json).expect("route JSON parses");
        }
        assert_eq!(real_routes().len(), ROUTE_FILES.len());
    }

    #[test]
    fn haversine_is_correct() {
        // One degree of latitude is about 111.2 km.
        assert!((haversine((45.0, 6.0), (46.0, 6.0)) - 111_195.0).abs() < 100.0);
    }
}
