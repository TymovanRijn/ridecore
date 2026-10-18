//! Your best ride on each course, kept as a ghost to race against: where you
//! were every second. Stored per course (route and laps) in
//! ~/Library/Application Support/RideCore/ghosts/.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ghost {
    pub route: String,
    pub laps: u32,
    pub unix_time: u64,
    /// Finish time in seconds.
    pub finish_s: f32,
    /// Distance in meters at each whole second from the start; never
    /// decreasing.
    pub dist: Vec<f32>,
}

impl Ghost {
    /// A ghost from a finished ride's distance at each second.
    pub fn new(route: &str, laps: u32, unix_time: u64, finish_s: f32, dist: &[f64]) -> Self {
        let mut furthest = 0.0f32;
        let dist = dist
            .iter()
            .map(|&d| {
                furthest = furthest.max(d as f32);
                furthest
            })
            .collect();
        Self {
            route: route.to_string(),
            laps,
            unix_time,
            finish_s,
            dist,
        }
    }

    /// Where the ghost is `t` seconds after the start.
    pub fn dist_at(&self, t: f32) -> f32 {
        let Some(&last) = self.dist.last() else {
            return 0.0;
        };
        if t <= 0.0 {
            return self.dist[0];
        }
        let i = t.floor() as usize;
        if i + 1 >= self.dist.len() {
            return last;
        }
        let f = t - i as f32;
        self.dist[i] + (self.dist[i + 1] - self.dist[i]) * f
    }

    /// When the ghost got to `d` meters, if it did.
    pub fn time_at(&self, d: f32) -> Option<f32> {
        let i = self.dist.partition_point(|&x| x < d);
        if i >= self.dist.len() {
            return None;
        }
        if i == 0 {
            return Some(0.0);
        }
        let (a, b) = (self.dist[i - 1], self.dist[i]);
        let f = if b > a { (d - a) / (b - a) } else { 0.0 };
        Some(i as f32 - 1.0 + f)
    }

    /// Seconds behind your best (negative: ahead) at `d` meters, `now`
    /// seconds into the race.
    pub fn gap(&self, now: f32, d: f32) -> Option<f32> {
        self.time_at(d).map(|t| now - t)
    }

    /// Whether this ride beats `best`.
    pub fn beats(&self, best: Option<&Ghost>) -> bool {
        best.is_none_or(|b| self.finish_s < b.finish_s)
    }

    fn path(route: &str, laps: u32) -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        let safe: String = route
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Some(
            PathBuf::from(home)
                .join("Library/Application Support/RideCore/ghosts")
                .join(format!("{safe}-{laps}.json")),
        )
    }

    /// Your best ride on this course, if you've finished it before.
    pub fn load(route: &str, laps: u32) -> Option<Self> {
        let s = std::fs::read_to_string(Self::path(route, laps)?).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path(&self.route, self.laps)
            .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(speed: f64, secs: usize) -> Ghost {
        let dist: Vec<f64> = (0..=secs).map(|s| s as f64 * speed).collect();
        Ghost::new("r", 1, 0, secs as f32, &dist)
    }

    #[test]
    fn position_and_time_interpolate() {
        let g = steady(10.0, 100);
        assert_eq!(g.dist_at(-5.0), 0.0);
        assert!((g.dist_at(12.5) - 125.0).abs() < 1e-3);
        assert_eq!(g.dist_at(500.0), 1000.0);
        assert!((g.time_at(125.0).unwrap() - 12.5).abs() < 1e-3);
        assert_eq!(g.time_at(0.0), Some(0.0));
        assert_eq!(g.time_at(1001.0), None);
    }

    #[test]
    fn gap_is_positive_when_behind() {
        let g = steady(10.0, 100);
        // At 30 s you've done 250 m; the ghost was there at 25 s.
        assert!((g.gap(30.0, 250.0).unwrap() - 5.0).abs() < 1e-3);
        assert!((g.gap(20.0, 250.0).unwrap() + 5.0).abs() < 1e-3);
    }

    #[test]
    fn only_faster_rides_replace_the_best() {
        let slow = steady(9.0, 120);
        let fast = steady(10.0, 100);
        assert!(slow.beats(None));
        assert!(fast.beats(Some(&slow)));
        assert!(!slow.beats(Some(&fast)));
    }

    #[test]
    fn distance_never_goes_backwards() {
        let g = Ghost::new("r", 1, 0, 3.0, &[0.0, 5.0, 4.0, 9.0]);
        assert_eq!(g.dist, vec![0.0, 5.0, 5.0, 9.0]);
    }
}
