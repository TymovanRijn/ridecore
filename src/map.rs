//! The real landscape around a route, from OpenStreetMap (built by
//! tools/fetch_map.py): land cover on a grid, building footprints, side
//! roads and named places, all in the route's local meters.
//!
//! Data © OpenStreetMap contributors (ODbL).

use serde::Deserialize;

/// What covers the ground at a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    Unknown,
    Forest,
    Farmland,
    Meadow,
    Town,
    Industry,
    Water,
    Scrub,
    Rock,
    Orchard,
}

impl Cover {
    fn from_code(c: u8) -> Self {
        match c {
            1 => Self::Forest,
            2 => Self::Farmland,
            3 => Self::Meadow,
            4 => Self::Town,
            5 => Self::Industry,
            6 => Self::Water,
            7 => Self::Scrub,
            8 => Self::Rock,
            9 => Self::Orchard,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingKind {
    House,
    Church,
    /// Sheds, barns, warehouses, shops: big and plain.
    Shed,
}

#[derive(Debug, Clone)]
pub struct Building {
    /// Floors, when mapped.
    pub levels: Option<u32>,
    pub kind: BuildingKind,
    /// Footprint corners (x, z), not closed.
    pub footprint: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadClass {
    Motorway,
    Main,
    Local,
    Service,
    Track,
}

#[derive(Debug, Clone)]
pub struct SideRoad {
    pub class: RoadClass,
    pub width: f32,
    pub line: Vec<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct Place {
    pub name: String,
    pub pos: [f32; 2],
    /// "town", "village", "hamlet", "suburb", "city" or "peak".
    pub kind: String,
}

#[derive(Debug)]
pub struct MapData {
    cell_m: f32,
    x0: f32,
    z0: f32,
    nx: usize,
    nz: usize,
    cover: Vec<u8>,
    pub buildings: Vec<Building>,
    pub roads: Vec<SideRoad>,
    pub places: Vec<Place>,
}

#[derive(Deserialize)]
struct MapFile {
    cell_m: f32,
    x0: f32,
    z0: f32,
    nx: usize,
    nz: usize,
    cover: Vec<u32>,
    buildings: Vec<Vec<f32>>,
    roads: Vec<Vec<f32>>,
    places: Vec<(String, f32, f32, String)>,
}

fn pairs(v: &[f32]) -> Vec<[f32; 2]> {
    v.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
}

impl MapData {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let f: MapFile = serde_json::from_str(json)?;
        let mut cover = Vec::with_capacity(f.nx * f.nz);
        for run in f.cover.as_chunks::<2>().0 {
            cover.extend(std::iter::repeat_n(run[0] as u8, run[1] as usize));
        }
        cover.resize(f.nx * f.nz, 0);
        let buildings = f
            .buildings
            .iter()
            .filter(|b| b.len() >= 8)
            .map(|b| Building {
                levels: (b[0] > 0.0).then_some(b[0] as u32),
                kind: match b[1] as u32 {
                    1 => BuildingKind::Church,
                    2 => BuildingKind::Shed,
                    _ => BuildingKind::House,
                },
                footprint: pairs(&b[2..]),
            })
            .collect();
        let roads = f
            .roads
            .iter()
            .filter(|r| r.len() >= 6)
            .map(|r| SideRoad {
                class: match r[0] as u32 {
                    0 => RoadClass::Motorway,
                    1 => RoadClass::Main,
                    2 => RoadClass::Local,
                    3 => RoadClass::Service,
                    _ => RoadClass::Track,
                },
                width: r[1],
                line: pairs(&r[2..]),
            })
            .collect();
        let places = f
            .places
            .into_iter()
            .map(|(name, x, z, kind)| Place {
                name,
                pos: [x, z],
                kind,
            })
            .collect();
        Ok(Self {
            cell_m: f.cell_m,
            x0: f.x0,
            z0: f.z0,
            nx: f.nx,
            nz: f.nz,
            cover,
            buildings,
            roads,
            places,
        })
    }

    /// Land cover at a point; `Unknown` outside the mapped area.
    pub fn cover(&self, x: f32, z: f32) -> Cover {
        let i = ((x - self.x0) / self.cell_m).floor();
        let j = ((z - self.z0) / self.cell_m).floor();
        if i < 0.0 || j < 0.0 || i >= self.nx as f32 || j >= self.nz as f32 {
            return Cover::Unknown;
        }
        Cover::from_code(self.cover[j as usize * self.nx + i as usize])
    }

    /// Share of cells within `r` meters of a point that have this cover.
    pub fn share(&self, x: f32, z: f32, r: f32, cover: Cover) -> f32 {
        let step = self.cell_m;
        let n = (r / step).ceil() as i32;
        let (mut hit, mut all) = (0, 0);
        for di in -n..=n {
            for dj in -n..=n {
                if (di * di + dj * dj) as f32 * step * step > r * r {
                    continue;
                }
                all += 1;
                if self.cover(x + di as f32 * step, z + dj as f32 * step) == cover {
                    hit += 1;
                }
            }
        }
        hit as f32 / all.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use crate::route::real_routes;

    use super::*;

    #[test]
    fn the_liege_finale_has_its_real_landscape() {
        let r = real_routes().iter().find(|r| r.id == "lbl-finale").unwrap();
        let geo = r.geo.as_ref().unwrap();
        let map = geo.map().expect("map data");
        assert!(map.buildings.len() > 1000);
        assert!(map.roads.len() > 500);
        assert!(map.places.iter().any(|p| p.name.contains("Remouchamps")));
        // The Ardennes are wooded, and the road runs through villages.
        let [x0, z0, x1, z1] = geo.bounds();
        let mut forest = 0;
        let mut n = 0;
        for i in 0..50 {
            for j in 0..50 {
                let (x, z) = (
                    x0 + (x1 - x0) * i as f32 / 50.0,
                    z0 + (z1 - z0) * j as f32 / 50.0,
                );
                n += 1;
                if map.cover(x, z) == Cover::Forest {
                    forest += 1;
                }
            }
        }
        assert!(forest * 100 / n > 10, "forest {forest}/{n}");
        // Remouchamps sits by the Amblève: water within a few hundred meters.
        let p = map.places.iter().find(|p| p.name == "Remouchamps").unwrap();
        assert!(map.share(p.pos[0], p.pos[1], 600.0, Cover::Water) > 0.0);
    }
}
