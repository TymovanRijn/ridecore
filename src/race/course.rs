//! A race course: a route ridden for some laps, its sprint and KOM segments,
//! and the road's 3D centerline for rendering.

use crate::route::Route;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Sprint,
    Kom,
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub kind: SegmentKind,
    pub name: String,
    /// Race distance where the segment starts and ends (the line).
    pub start_m: f64,
    pub end_m: f64,
}

/// Points for the first five riders over a segment line.
pub const SEGMENT_POINTS: [u32; 5] = [10, 6, 4, 2, 1];

/// Length of an intermediate sprint zone before its line.
const SPRINT_ZONE_M: f64 = 300.0;
/// Climbs must gain at least this much to become a KOM.
const MIN_KOM_GAIN_M: f32 = 25.0;

#[derive(Debug, Clone)]
pub struct Course {
    pub route: Route,
    pub laps: u32,
    pub total_m: f64,
    pub segments: Vec<Segment>,
    /// Every climb worth showing, in race order (all laps).
    pub climbs: Vec<Climb>,
    pub path: Path,
}

/// A climb as a bike computer's climb mode sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Climb {
    pub start_m: f64,
    pub end_m: f64,
    pub gain_m: f32,
    pub avg_pct: f32,
    /// Steepest 100 m.
    pub max_pct: f32,
    pub category: ClimbCategory,
    /// The climb's real name, or the village it climbs to.
    pub name: Option<String>,
}

/// Famous climbs, found by their summit (lat, lon).
const FAMOUS: [(&str, f64, f64); 2] = [
    ("Côte de La Redoute", 50.4889, 5.6949),
    ("Côte de la Roche-aux-Faucons", 50.5496, 5.5752),
];

/// A name for the climb that tops out at lap distance `top`: a famous one,
/// else the nearest village or town on the map.
fn climb_name(route: &Route, top: f64) -> Option<String> {
    let geo = route.geo.as_ref()?;
    let (lat, lon) = route.latlon_at(top)?;
    if let Some((name, ..)) = FAMOUS
        .iter()
        .find(|(_, la, lo)| crate::route::haversine((lat, lon), (*la, *lo)) < 600.0)
    {
        return Some((*name).to_string());
    }
    let [x, z] = geo.project(lat, lon);
    geo.map()?
        .places
        .iter()
        .filter(|p| matches!(p.kind.as_str(), "village" | "town"))
        .map(|p| (p, (p.pos[0] - x).hypot(p.pos[1] - z)))
        .filter(|(_, d)| *d < 1500.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| format!("Klim bij {}", p.name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClimbCategory {
    Uncategorized,
    Cat4,
    Cat3,
    Cat2,
    Cat1,
    Hc,
}

impl ClimbCategory {
    /// From length × grade, as cycling apps rate climbs.
    fn from_score(length_m: f64, avg_pct: f32) -> Self {
        let score = length_m as f32 * avg_pct;
        match score {
            s if s >= 80_000.0 => Self::Hc,
            s if s >= 64_000.0 => Self::Cat1,
            s if s >= 32_000.0 => Self::Cat2,
            s if s >= 16_000.0 => Self::Cat3,
            s if s >= 8_000.0 => Self::Cat4,
            _ => Self::Uncategorized,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Uncategorized => "Klim",
            Self::Cat4 => "Cat 4",
            Self::Cat3 => "Cat 3",
            Self::Cat2 => "Cat 2",
            Self::Cat1 => "Cat 1",
            Self::Hc => "HC",
        }
    }
}

impl Climb {
    pub fn length_m(&self) -> f64 {
        self.end_m - self.start_m
    }
}

/// Climb-mode thresholds (like Garmin ClimbPro): at least this long...
const CLIMB_MIN_LENGTH_M: f64 = 500.0;
/// ...this steep on average...
const CLIMB_MIN_AVG_PCT: f32 = 3.0;
/// ...and length (m) × grade (%) at least this.
const CLIMB_MIN_SCORE: f32 = 1500.0;

impl Course {
    pub fn new(route: Route, laps: u32) -> Self {
        let laps = if route.looped { laps.max(1) } else { 1 };
        let lap = route.length_m();
        let total_m = lap * laps as f64;
        let mut segments = Vec::new();
        let kom = biggest_climb(&route);
        for l in 0..laps {
            let base = lap * l as f64;
            if let Some((s, e)) = kom {
                segments.push(Segment {
                    kind: SegmentKind::Kom,
                    name: format!("KOM ronde {}", l + 1),
                    start_m: base + s,
                    end_m: base + e,
                });
            }
            if l + 1 < laps {
                segments.push(Segment {
                    kind: SegmentKind::Sprint,
                    name: format!("Tussensprint ronde {}", l + 1),
                    start_m: base + lap - SPRINT_ZONE_M,
                    end_m: base + lap,
                });
            }
        }
        segments.sort_by(|a, b| a.end_m.total_cmp(&b.end_m));
        let path = Path::for_route(&route);
        let one_lap = find_climbs(&route);
        let climbs = (0..laps)
            .flat_map(|l| {
                let base = lap * l as f64;
                one_lap.iter().map(move |c| Climb {
                    start_m: c.start_m + base,
                    end_m: c.end_m + base,
                    ..c.clone()
                })
            })
            .collect();
        Self {
            route,
            laps,
            total_m,
            segments,
            climbs,
            path,
        }
    }

    pub fn grade_pct_at(&self, d: f64) -> f32 {
        self.route.grade_pct_at(d.min(self.total_m - 0.1))
    }

    pub fn elevation_at(&self, d: f64) -> f32 {
        self.route.elevation_at(d.min(self.total_m))
    }

    /// The grade a rider feels at `d`: the climb over the 10 m behind and
    /// 20 m ahead, so a kink in the GPS track doesn't jerk the trainer.
    pub fn smooth_grade_pct(&self, d: f64) -> f32 {
        let a = (d - 10.0).max(0.0);
        let b = (d + 20.0).min(self.total_m);
        if b - a < 1.0 {
            return self.grade_pct_at(d);
        }
        (self.elevation_at(b) - self.elevation_at(a)) / (b - a) as f32 * 100.0
    }

    /// Meters to the top of the climb the rider is on (the highest point
    /// before a real descent), or `None` when not climbing.
    pub fn to_summit(&self, d: f64) -> Option<f64> {
        if self.grade_ahead_pct(d, 300.0) < 2.0 {
            return None;
        }
        let (mut top_d, mut top_e) = (d, self.elevation_at(d));
        let mut x = d;
        while x < self.total_m && x - d < 40_000.0 {
            x += 20.0;
            let e = self.elevation_at(x.min(self.total_m));
            if e > top_e {
                (top_d, top_e) = (x, e);
            } else if e < top_e - CLIMB_DIP_M {
                break;
            }
        }
        (top_d - d > 100.0).then_some(top_d.min(self.total_m) - d)
    }

    /// Mean grade (percent) over the next `window` meters.
    pub fn grade_ahead_pct(&self, d: f64, window: f64) -> f32 {
        let end = (d + window).min(self.total_m);
        if end <= d {
            return 0.0;
        }
        (self.elevation_at(end) - self.elevation_at(d)) / (end - d) as f32 * 100.0
    }

    /// The climb you are on, or the next one if it starts within `ahead_m`.
    pub fn climb_near(&self, d: f64, ahead_m: f64) -> Option<&Climb> {
        self.climbs
            .iter()
            .find(|c| c.end_m > d && c.start_m - ahead_m <= d)
    }

    pub fn next_segment(&self, d: f64) -> Option<&Segment> {
        self.segments.iter().find(|s| s.end_m > d)
    }
}

/// A dip smaller than this doesn't end a climb (real climbs have flat bits).
const CLIMB_DIP_M: f32 = 15.0;

/// The climb with the most elevation gain (at least `MIN_KOM_GAIN_M`),
/// measured from a low point to the summit.
fn biggest_climb(route: &Route) -> Option<(f64, f64)> {
    let pts = &route.points;
    let mut best: Option<(f64, f64, f32)> = None;
    let (mut low, mut peak) = (0usize, 0usize);
    let close = |low: usize, peak: usize, best: &mut Option<(f64, f64, f32)>| {
        let gain = pts[peak].1 - pts[low].1;
        if gain >= MIN_KOM_GAIN_M && best.is_none_or(|b| gain > b.2) {
            *best = Some((pts[low].0, pts[peak].0, gain));
        }
    };
    for i in 1..pts.len() {
        if pts[i].1 > pts[peak].1 {
            peak = i;
        } else if pts[i].1 < pts[peak].1 - CLIMB_DIP_M {
            // The climb is over: a real descent started after the peak.
            close(low, peak, &mut best);
            low = i;
            peak = i;
        }
        if pts[i].1 < pts[low].1 && peak == low {
            low = i;
            peak = i;
        }
    }
    close(low, peak, &mut best);
    best.map(|(s, e, _)| (s, e))
}

/// All climbs on one lap: from each low point to its summit (dips under
/// `CLIMB_DIP_M` don't end a climb), with the flat run-in trimmed off.
pub fn find_climbs(route: &Route) -> Vec<Climb> {
    let pts = &route.points;
    if pts.len() < 2 {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let (mut low, mut peak) = (0usize, 0usize);
    for i in 1..pts.len() {
        if pts[i].1 > pts[peak].1 {
            peak = i;
        } else if pts[i].1 < pts[peak].1 - CLIMB_DIP_M {
            spans.push((low, peak));
            low = i;
            peak = i;
        }
        if pts[i].1 < pts[low].1 && peak == low {
            low = i;
            peak = i;
        }
    }
    spans.push((low, peak));

    let grade = |a: f64, b: f64| {
        if b <= a {
            0.0
        } else {
            (route.elevation_at(b) - route.elevation_at(a)) / (b - a) as f32 * 100.0
        }
    };
    spans
        .into_iter()
        .filter_map(|(lo, pk)| {
            let mut start = pts[lo].0;
            let end = pts[pk].0;
            // Start where the road actually starts to rise.
            while start + 100.0 < end && grade(start, start + 100.0) < 2.0 {
                start += 25.0;
            }
            let length = end - start;
            let gain = route.elevation_at(end) - route.elevation_at(start);
            let avg = if length > 0.0 {
                gain / length as f32 * 100.0
            } else {
                0.0
            };
            if length < CLIMB_MIN_LENGTH_M
                || avg < CLIMB_MIN_AVG_PCT
                || length as f32 * avg < CLIMB_MIN_SCORE
            {
                return None;
            }
            let mut max_pct = avg;
            let mut x = start;
            while x + 100.0 <= end {
                max_pct = max_pct.max(grade(x, x + 100.0));
                x += 50.0;
            }
            Some(Climb {
                start_m: start,
                end_m: end,
                gain_m: gain,
                avg_pct: avg,
                max_pct,
                category: ClimbCategory::from_score(length, avg),
                name: climb_name(route, end),
            })
        })
        .collect()
}

/// Road centerline in the horizontal plane: the real road for routes with
/// GPS data, a generated shape otherwise. Elevation comes from the profile.
#[derive(Debug, Clone)]
pub struct Path {
    points: Vec<[f32; 2]>,
    /// Lap distance of every point.
    dists: Vec<f64>,
    lap_m: f64,
    looped: bool,
}

const STEP: f64 = 2.0;

/// Position on the road: x/z ground plane, y up, plus the travel heading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pos: [f32; 3],
    /// Unit travel direction in the ground plane.
    pub dir: [f32; 2],
}

impl Path {
    pub fn for_route(route: &Route) -> Self {
        let lap_m = route.length_m();
        let (points, dists) = match &route.geo {
            Some(geo) => (
                geo.latlon
                    .iter()
                    .map(|&(lat, lon)| geo.project(lat, lon))
                    .collect(),
                route.points.iter().map(|p| p.0).collect(),
            ),
            None => {
                let n = (lap_m / STEP).ceil() as usize + 1;
                let pts = if route.looped {
                    closed_loop(lap_m, n)
                } else {
                    winding(n)
                };
                let dists = (0..pts.len()).map(|i| i as f64 * STEP).collect();
                (pts, dists)
            }
        };
        Self {
            points,
            dists,
            lap_m,
            looped: route.looped,
        }
    }

    fn point(&self, i: isize) -> [f32; 2] {
        let n = self.points.len() as isize;
        let k = if self.looped {
            i.rem_euclid(n - 1)
        } else {
            i.clamp(0, n - 1)
        };
        self.points[k as usize]
    }

    /// Smooth (Catmull-Rom) position at lap distance `lap_d`, so real roads
    /// bend through their corners instead of kinking at every GPS point.
    fn sample(&self, lap_d: f64) -> [f32; 2] {
        let d = lap_d.clamp(0.0, self.lap_m);
        let i = self
            .dists
            .partition_point(|&x| x <= d)
            .clamp(1, self.dists.len() - 1)
            - 1;
        let span = (self.dists[i + 1] - self.dists[i]).max(1e-6);
        let t = ((d - self.dists[i]) / span).clamp(0.0, 1.0) as f32;
        let i = i as isize;
        let (p0, p1, p2, p3) = (
            self.point(i - 1),
            self.point(i),
            self.point(i + 1),
            self.point(i + 2),
        );
        let cr = |a: f32, b: f32, c: f32, e: f32| {
            0.5 * (2.0 * b
                + (-a + c) * t
                + (2.0 * a - 5.0 * b + 4.0 * c - e) * t * t
                + (-a + 3.0 * b - 3.0 * c + e) * t * t * t)
        };
        [
            cr(p0[0], p1[0], p2[0], p3[0]),
            cr(p0[1], p1[1], p2[1], p3[1]),
        ]
    }

    fn lap_d(&self, d: f64) -> f64 {
        if self.looped {
            d.rem_euclid(self.lap_m)
        } else {
            d.clamp(0.0, self.lap_m)
        }
    }

    /// Pose at race distance `d`, offset `lateral` meters to the right.
    pub fn pose(&self, route: &Route, d: f64, lateral: f32) -> Pose {
        let lap_d = self.lap_d(d);
        let p = self.sample(lap_d);
        let (ahead, behind) = if self.looped {
            (
                (lap_d + 1.5).rem_euclid(self.lap_m),
                (lap_d - 1.5).rem_euclid(self.lap_m),
            )
        } else {
            ((lap_d + 1.5).min(self.lap_m), (lap_d - 1.5).max(0.0))
        };
        let (q, b) = (self.sample(ahead), self.sample(behind));
        let (dx, dz) = (q[0] - b[0], q[1] - b[1]);
        let len = (dx * dx + dz * dz).sqrt().max(1e-4);
        let dir = [dx / len, dz / len];
        // Right-hand normal in the x/z plane.
        let right = [-dir[1], dir[0]];
        Pose {
            pos: [
                p[0] + right[0] * lateral,
                route.elevation_at(d),
                p[1] + right[1] * lateral,
            ],
            dir,
        }
    }
}

/// A smooth closed loop of the requested length (rescaled by arc length).
fn closed_loop(lap_m: f64, n: usize) -> Vec<[f32; 2]> {
    let raw: Vec<[f64; 2]> = (0..2000)
        .map(|i| {
            let th = i as f64 / 2000.0 * std::f64::consts::TAU;
            let r = 1.0
                + 0.18 * (2.0 * th + 0.7).sin()
                + 0.09 * (3.0 * th + 2.1).sin()
                + 0.05 * (5.0 * th).sin();
            [r * th.cos(), r * th.sin()]
        })
        .collect();
    resample(&raw, true, lap_m, n)
}

/// An open, gently winding road (for point-to-point climbs).
fn winding(n: usize) -> Vec<[f32; 2]> {
    let mut pts = Vec::with_capacity(n);
    let (mut x, mut z, mut heading) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        pts.push([x, z]);
        let s = i as f64 * STEP;
        // Hairpin-ish switchbacks: curvature alternates sign every ~600 m.
        heading += STEP * 0.004 * (s / 600.0 * std::f64::consts::PI).sin();
        x += STEP * heading.cos();
        z += STEP * heading.sin();
    }
    pts.into_iter().map(|[x, z]| [x as f32, z as f32]).collect()
}

/// Rescales a polyline to `length` and resamples it at equal arc length.
fn resample(raw: &[[f64; 2]], closed: bool, length: f64, n: usize) -> Vec<[f32; 2]> {
    let mut pts = raw.to_vec();
    if closed {
        pts.push(raw[0]);
    }
    let mut cum = vec![0.0];
    for w in pts.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        cum.push(cum.last().unwrap() + d);
    }
    let scale = length / cum.last().unwrap();
    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let target = (i as f64 * STEP / scale).min(*cum.last().unwrap());
        while j + 1 < cum.len() - 1 && cum[j + 1] < target {
            j += 1;
        }
        let t = ((target - cum[j]) / (cum[j + 1] - cum[j]).max(1e-9)).clamp(0.0, 1.0);
        let x = pts[j][0] + (pts[j + 1][0] - pts[j][0]) * t;
        let z = pts[j][1] + (pts[j + 1][1] - pts[j][1]) * t;
        out.push([(x * scale) as f32, (z * scale) as f32]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real(id: &str) -> Route {
        crate::route::real_routes()
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("route {id}"))
            .clone()
    }

    #[test]
    fn alpe_dhuez_is_one_hc_climb_with_its_real_numbers() {
        let c = Course::new(real("alpe-dhuez"), 1);
        let big: Vec<&Climb> = c.climbs.iter().filter(|c| c.gain_m > 300.0).collect();
        assert_eq!(big.len(), 1, "{:?}", c.climbs);
        let alpe = big[0];
        assert_eq!(alpe.category, ClimbCategory::Hc);
        assert!((alpe.gain_m - 1100.0).abs() < 80.0, "{alpe:?}");
        assert!((7.0..9.5).contains(&alpe.avg_pct), "{alpe:?}");
        assert!(alpe.max_pct > alpe.avg_pct);
        // Shown ahead of time and while riding it, gone after the top.
        assert!(c.climb_near(alpe.start_m - 500.0, 1000.0).is_some());
        assert!(c.climb_near(alpe.end_m + 10.0, 1000.0).is_none());
    }

    #[test]
    fn a_flat_coast_road_has_no_climbs() {
        let c = Course::new(real("kust-noordwijk"), 1);
        assert!(c.climbs.is_empty(), "{:?}", c.climbs);
    }

    #[test]
    fn climbs_repeat_on_every_lap() {
        let c = Course::new(Route::builtin()[1].clone(), 3);
        assert!(!c.climbs.is_empty());
        assert_eq!(c.climbs.len() % 3, 0);
        assert!(c.climbs.windows(2).all(|w| w[0].end_m <= w[1].start_m));
    }

    #[test]
    fn rolling_course_gets_koms_and_intermediate_sprints() {
        let c = Course::new(Route::builtin()[1].clone(), 3);
        assert_eq!(c.total_m, 42_000.0);
        let sprints = c
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::Sprint)
            .count();
        let koms = c
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::Kom)
            .count();
        assert_eq!((sprints, koms), (2, 3));
        assert!(c.segments.windows(2).all(|w| w[0].end_m <= w[1].end_m));
    }

    #[test]
    fn closed_loop_has_requested_length_and_closes() {
        let route = Route::builtin()[0].clone();
        let path = Path::for_route(&route);
        let len: f32 = path
            .points
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
            .sum();
        assert!((len - 10_000.0).abs() < 25.0, "{len}");
        let a = path.pose(&route, 0.0, 0.0).pos;
        let b = path.pose(&route, 10_000.0, 0.0).pos;
        assert!((a[0] - b[0]).abs() < 1.0 && (a[2] - b[2]).abs() < 1.0);
    }

    #[test]
    fn lateral_offset_is_perpendicular_to_travel() {
        let route = Route::builtin()[1].clone();
        let path = Path::for_route(&route);
        let c = path.pose(&route, 500.0, 0.0);
        let r = path.pose(&route, 500.0, 3.0);
        let off = [r.pos[0] - c.pos[0], r.pos[2] - c.pos[2]];
        let dot = off[0] * c.dir[0] + off[1] * c.dir[1];
        assert!(dot.abs() < 0.05, "{dot}");
        assert!(((off[0].powi(2) + off[1].powi(2)).sqrt() - 3.0).abs() < 0.01);
    }

    #[test]
    fn mountain_course_is_single_lap_with_summit_kom() {
        let c = Course::new(Route::builtin()[2].clone(), 5);
        assert_eq!(c.laps, 1);
        let kom = c
            .segments
            .iter()
            .find(|s| s.kind == SegmentKind::Kom)
            .unwrap();
        assert_eq!(kom.end_m, 12_000.0);
    }
}

#[cfg(test)]
mod named_climbs {
    #[test]
    fn the_liege_finale_climbs_have_their_real_names() {
        let r = crate::route::real_routes()
            .iter()
            .find(|r| r.id == "lbl-finale")
            .unwrap();
        let names: Vec<Option<String>> =
            super::find_climbs(r).into_iter().map(|c| c.name).collect();
        assert_eq!(names[0].as_deref(), Some("Côte de La Redoute"));
        assert!(
            names
                .iter()
                .any(|n| n.as_deref() == Some("Côte de la Roche-aux-Faucons"))
        );
    }
}

#[cfg(test)]
mod smooth_grade_tests {
    use super::Course;

    #[test]
    fn smoothed_grades_have_no_gps_spikes() {
        // The Tourmalet's raw track has a 42% kink; nothing like that is
        // real, and the trainer must never get it.
        for r in crate::route::Route::catalog() {
            let c = Course::new(r.clone(), 1);
            let mut d = 0.0;
            let mut prev = c.smooth_grade_pct(0.0);
            while d < c.total_m {
                let g = c.smooth_grade_pct(d);
                assert!(g.abs() < 30.0, "{} {g:.1}% at {d}", r.id);
                assert!(
                    (g - prev).abs() < 2.5,
                    "{} jumps {prev:.1} -> {g:.1} at {d}",
                    r.id
                );
                prev = g;
                d += 2.0;
            }
        }
    }
}
