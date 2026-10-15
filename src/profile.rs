//! The rider's profile and training log, kept between sessions in
//! ~/Library/Application Support/RideCore/profile.json.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub ftp: f32,
    pub kg: f32,
    pub height_cm: f32,
    /// Index into the setup screen's rider roles.
    pub role: u8,
    pub sessions: Vec<Session>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Jij".into(),
            ftp: 250.0,
            kg: 75.0,
            height_cm: 180.0,
            role: 2,
            sessions: Vec::new(),
        }
    }
}

/// One finished workout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub unix_time: u64,
    pub workout: String,
    pub secs: u32,
    pub avg_w: f32,
    pub np_w: f32,
    pub tss: f32,
    pub kj: f32,
    /// Intervals held / intervals ridden.
    pub nailed: u32,
    pub intervals: u32,
    /// Set when a ramp test measured a new FTP.
    pub new_ftp: Option<f32>,
}

impl Profile {
    fn path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/RideCore/profile.json"))
    }

    /// The saved profile, or a fresh one.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write then rename, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Training stress of the last seven days.
    pub fn week_tss(&self, now_unix: u64) -> f32 {
        self.sessions
            .iter()
            .filter(|s| now_unix.saturating_sub(s.unix_time) < 7 * 86_400)
            .map(|s| s.tss)
            .fold(0.0, |a, b| a + b)
    }

    /// Days in a row (up to today) with at least one session.
    pub fn streak_days(&self, now_unix: u64) -> u32 {
        let today = now_unix / 86_400;
        let mut days: Vec<u64> = self.sessions.iter().map(|s| s.unix_time / 86_400).collect();
        days.sort_unstable();
        days.dedup();
        let mut streak = 0;
        let mut want = today;
        for &d in days.iter().rev() {
            if d == want || (streak == 0 && d + 1 == today) {
                streak += 1;
                want = d.saturating_sub(1);
            } else if d < want {
                break;
            }
        }
        streak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u64) -> Session {
        Session {
            unix_time: day * 86_400 + 3600,
            workout: "x".into(),
            secs: 3600,
            avg_w: 200.0,
            np_w: 210.0,
            tss: 60.0,
            kj: 700.0,
            nailed: 4,
            intervals: 4,
            new_ftp: None,
        }
    }

    #[test]
    fn streaks_count_consecutive_days() {
        let mut p = Profile::default();
        let now = 100 * 86_400 + 50_000;
        assert_eq!(p.streak_days(now), 0);
        p.sessions = vec![at(97), at(98), at(99), at(100)];
        assert_eq!(p.streak_days(now), 4);
        // Yesterday still counts until you ride today.
        p.sessions = vec![at(98), at(99)];
        assert_eq!(p.streak_days(now), 2);
        p.sessions = vec![at(95), at(99), at(100)];
        assert_eq!(p.streak_days(now), 2);
        assert_eq!(p.week_tss(now), 180.0);
    }

    #[test]
    fn old_files_still_load() {
        let p: Profile = serde_json::from_str(r#"{"name":"Tymo","ftp":260}"#).unwrap();
        assert_eq!(p.ftp, 260.0);
        assert_eq!(p.kg, 75.0);
    }
}
