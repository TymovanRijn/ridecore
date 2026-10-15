//! Workouts you build yourself from blocks, kept between sessions in
//! ~/Library/Application Support/RideCore/workouts.json, and their `.zwo`
//! export and import.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::t;
use crate::tf;
use crate::training::{Focus, Plan, Zone};
use crate::workout::{Segment, Workout};

/// What block powers are written in. Watts stay put when your FTP changes;
/// percentages scale with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Watts,
    Percent,
}

/// One building block. Powers are in the workout's `Unit`: watts, or
/// percent of FTP (90.0 = 90%).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Steady {
        secs: u32,
        power: f32,
        #[serde(default)]
        cadence: Option<u32>,
    },
    Ramp {
        secs: u32,
        from: f32,
        to: f32,
        #[serde(default)]
        cadence: Option<u32>,
    },
    /// `reps` × (on, then off), like Zwift's IntervalsT.
    Intervals {
        reps: u32,
        on_secs: u32,
        on: f32,
        off_secs: u32,
        off: f32,
        #[serde(default)]
        cadence: Option<u32>,
    },
    /// No target: you ride as you like.
    Free { secs: u32 },
}

impl Block {
    pub fn secs(&self) -> u32 {
        match *self {
            Block::Steady { secs, .. } | Block::Ramp { secs, .. } | Block::Free { secs } => secs,
            Block::Intervals {
                reps,
                on_secs,
                off_secs,
                ..
            } => reps * (on_secs + off_secs),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Block::Steady { .. } => t("Blok", "Block"),
            Block::Ramp { .. } => t("Oplopend", "Ramp"),
            Block::Intervals { .. } => t("Intervallen", "Intervals"),
            Block::Free { .. } => t("Vrij rijden", "Free ride"),
        }
    }

    /// The hardest target in the block, in its unit.
    pub fn peak(&self) -> Option<f32> {
        match *self {
            Block::Steady { power, .. } => Some(power),
            Block::Ramp { from, to, .. } => Some(from.max(to)),
            Block::Intervals { on, off, .. } => Some(on.max(off)),
            Block::Free { .. } => None,
        }
    }

    fn map_power(&mut self, f: impl Fn(f32) -> f32) {
        match self {
            Block::Steady { power, .. } => *power = f(*power),
            Block::Ramp { from, to, .. } => {
                *from = f(*from);
                *to = f(*to);
            }
            Block::Intervals { on, off, .. } => {
                *on = f(*on);
                *off = f(*off);
            }
            Block::Free { .. } => {}
        }
    }
}

/// Ready-made blocks to drop into a workout, with powers in percent of FTP.
pub struct Template {
    pub name: &'static str,
    pub block: Block,
}

pub fn templates() -> Vec<Template> {
    let steady = |secs, power| Block::Steady {
        secs,
        power,
        cadence: None,
    };
    let reps = |reps, on_secs, on, off_secs, off, cadence| Block::Intervals {
        reps,
        on_secs,
        on,
        off_secs,
        off,
        cadence,
    };
    vec![
        Template {
            name: t("Warming-up", "Warm-up"),
            block: Block::Ramp {
                secs: 600,
                from: 50.0,
                to: 75.0,
                cadence: None,
            },
        },
        Template {
            name: t("Uitrijden", "Cool-down"),
            block: Block::Ramp {
                secs: 480,
                from: 65.0,
                to: 45.0,
                cadence: None,
            },
        },
        Template {
            name: t("Duur 20 min", "Endurance 20 min"),
            block: steady(1200, 68.0),
        },
        Template {
            name: "Sweet spot 12 min",
            block: steady(720, 90.0),
        },
        Template {
            name: t("Drempel 10 min", "Threshold 10 min"),
            block: steady(600, 100.0),
        },
        Template {
            name: "4 × 4 VO2max",
            block: reps(4, 240, 115.0, 180, 55.0, Some(95)),
        },
        Template {
            name: "13 × 30/15",
            block: reps(13, 30, 120.0, 15, 50.0, Some(100)),
        },
        Template {
            name: "10 × 40/20",
            block: reps(10, 40, 120.0, 20, 50.0, Some(100)),
        },
        Template {
            name: t("Over-unders 3 × 3", "Over-unders 3 × 3"),
            block: reps(3, 120, 95.0, 60, 108.0, Some(90)),
        },
        Template {
            name: "Sprints 6 × 10 s",
            block: reps(6, 10, 200.0, 110, 50.0, Some(110)),
        },
        Template {
            name: t("Herstel 3 min", "Recovery 3 min"),
            block: steady(180, 50.0),
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomWorkout {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub unit: Unit,
    pub blocks: Vec<Block>,
}

/// Library ids of custom workouts start with this.
pub const ID_PREFIX: &str = "custom-";

impl CustomWorkout {
    pub fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self {
            id: format!("{ID_PREFIX}{nanos:x}"),
            name: name.into(),
            notes: String::new(),
            unit: Unit::Watts,
            blocks: Vec::new(),
        }
    }

    pub fn secs(&self) -> u32 {
        self.blocks.iter().map(Block::secs).sum()
    }

    /// A block power as a fraction of FTP.
    pub fn fraction(&self, value: f32, ftp: f32) -> f32 {
        match self.unit {
            Unit::Watts => value / ftp.max(1.0),
            Unit::Percent => value / 100.0,
        }
    }

    /// A block power in watts.
    pub fn watts(&self, value: f32, ftp: f32) -> f32 {
        self.fraction(value, ftp) * ftp
    }

    /// A power in percent of FTP (as from a template) in this workout's unit.
    pub fn from_percent(&self, pct: f32, ftp: f32) -> f32 {
        match self.unit {
            Unit::Watts => (pct / 100.0 * ftp / 5.0).round() * 5.0,
            Unit::Percent => pct,
        }
    }

    /// Adds a template block, converted to this workout's unit.
    pub fn template_block(&self, template: &Block, ftp: f32) -> Block {
        let mut b = template.clone();
        b.map_power(|p| self.from_percent(p, ftp));
        b
    }

    /// Switches unit, converting every power so the workout stays the same.
    pub fn set_unit(&mut self, unit: Unit, ftp: f32) {
        if unit == self.unit {
            return;
        }
        let ftp = ftp.max(1.0);
        let convert: Box<dyn Fn(f32) -> f32> = match unit {
            Unit::Percent => Box::new(move |w| (w / ftp * 100.0).round()),
            Unit::Watts => Box::new(move |p| (p / 100.0 * ftp).round()),
        };
        for b in &mut self.blocks {
            b.map_power(&convert);
        }
        self.unit = unit;
    }

    pub fn segments(&self, ftp: f32) -> Vec<Segment> {
        self.blocks
            .iter()
            .flat_map(|b| self.block_segments(b, ftp))
            .collect()
    }

    /// One block as workout segments, in FTP fractions.
    pub fn block_segments(&self, b: &Block, ftp: f32) -> Vec<Segment> {
        let f = |v| self.fraction(v, ftp);
        let mut out = Vec::new();
        {
            match *b {
                Block::Steady {
                    secs,
                    power,
                    cadence,
                } => out.push(Segment::Steady {
                    secs,
                    power: f(power),
                    cadence,
                }),
                Block::Ramp {
                    secs,
                    from,
                    to,
                    cadence,
                } => out.push(Segment::Ramp {
                    secs,
                    from: f(from),
                    to: f(to),
                    cadence,
                }),
                Block::Intervals {
                    reps,
                    on_secs,
                    on,
                    off_secs,
                    off,
                    cadence,
                } => {
                    for _ in 0..reps {
                        out.push(Segment::Steady {
                            secs: on_secs,
                            power: f(on),
                            cadence,
                        });
                        if off_secs > 0 {
                            out.push(Segment::Steady {
                                secs: off_secs,
                                power: f(off),
                                cadence: None,
                            });
                        }
                    }
                }
                Block::Free { secs } => out.push(Segment::FreeRide { secs }),
            }
        }
        out.retain(|s| s.secs() > 0);
        out
    }

    /// Power every second, in watts (free riding counts as 60% FTP).
    pub fn series(&self, ftp: f32) -> Vec<f32> {
        let mut w = Vec::with_capacity(self.secs() as usize);
        for s in self.segments(ftp) {
            let n = s.secs();
            for k in 0..n {
                let frac = match s {
                    Segment::Steady { power, .. } => power,
                    Segment::Ramp { from, to, .. } => from + (to - from) * k as f32 / n as f32,
                    Segment::FreeRide { .. } => 0.6,
                };
                w.push(frac * ftp);
            }
        }
        w
    }

    /// What the workout trains, from its hardest few minutes.
    pub fn focus(&self, ftp: f32) -> Focus {
        let w = self.series(ftp);
        // The zone you spend at least two minutes in, at the top end.
        let mut secs = [0u32; 7];
        for &p in &w {
            secs[Zone::of(p / ftp) as usize] += 1;
        }
        let top = (0..7).rev().find(|&z| secs[z] >= 120).unwrap_or(0);
        match Zone::ALL[top] {
            Zone::Vo2max | Zone::Anaerobic => Focus::Vo2max,
            Zone::Threshold => Focus::Threshold,
            Zone::SweetSpot => Focus::SweetSpot,
            Zone::Tempo | Zone::Endurance => Focus::Endurance,
            Zone::Recovery => Focus::Recovery,
        }
    }

    pub fn to_plan(&self, ftp: f32) -> Plan {
        let name = if self.name.trim().is_empty() {
            t("Mijn training", "My workout").to_string()
        } else {
            self.name.trim().to_string()
        };
        let n = self.blocks.len();
        Plan {
            id: self.id.clone(),
            name: name.clone(),
            focus: self.focus(ftp),
            summary: if n == 1 {
                t("Eigen training · 1 blok", "Your workout · 1 block").into()
            } else {
                tf!("Eigen training · {n} blokken", "Your workout · {n} blocks")
            },
            why: self.notes.clone(),
            source: String::new(),
            workout: Workout {
                name,
                segments: self.segments(ftp),
            },
            warmup: 0,
            ramp_test: false,
        }
    }

    /// A workout from plain segments (an imported `.zwo` or a library
    /// workout), in percent of FTP, with repeats folded into intervals.
    pub fn from_segments(name: &str, segments: &[Segment]) -> Self {
        let mut w = Self::new(name);
        w.unit = Unit::Percent;
        let pct = |f: f32| (f * 1000.0).round() / 10.0;
        let mut i = 0;
        while i < segments.len() {
            // On/off pairs that repeat at least twice.
            if let (
                Some(&Segment::Steady {
                    secs: a,
                    power: pa,
                    cadence,
                }),
                Some(&Segment::Steady {
                    secs: b, power: pb, ..
                }),
            ) = (segments.get(i), segments.get(i + 1))
            {
                let pair = |k: usize| {
                    matches!(
                        (segments.get(k), segments.get(k + 1)),
                        (
                            Some(&Segment::Steady { secs: x, power: px, .. }),
                            Some(&Segment::Steady { secs: y, power: py, .. }),
                        ) if x == a && y == b && px == pa && py == pb
                    )
                };
                let mut reps = 0;
                while pair(i + 2 * reps) {
                    reps += 1;
                }
                if reps >= 2 && pa != pb {
                    w.blocks.push(Block::Intervals {
                        reps: reps as u32,
                        on_secs: a,
                        on: pct(pa),
                        off_secs: b,
                        off: pct(pb),
                        cadence,
                    });
                    i += 2 * reps;
                    continue;
                }
            }
            w.blocks.push(match segments[i] {
                Segment::Steady {
                    secs,
                    power,
                    cadence,
                } => Block::Steady {
                    secs,
                    power: pct(power),
                    cadence,
                },
                Segment::Ramp {
                    secs,
                    from,
                    to,
                    cadence,
                } => Block::Ramp {
                    secs,
                    from: pct(from),
                    to: pct(to),
                    cadence,
                },
                Segment::FreeRide { secs } => Block::Free { secs },
            });
            i += 1;
        }
        // Neighbouring identical steady blocks are one block.
        w.blocks.dedup_by(|b, a| match (a, b) {
            (
                Block::Steady {
                    secs,
                    power: pa,
                    cadence: ca,
                },
                Block::Steady {
                    secs: sb,
                    power: pb,
                    cadence: cb,
                },
            ) if pa == pb && ca == cb => {
                *secs += *sb;
                true
            }
            _ => false,
        });
        w
    }

    /// The workout as a Zwift `.zwo` file.
    pub fn to_zwo(&self, ftp: f32) -> String {
        let f = |v| (self.fraction(v, ftp) * 1000.0).round() / 1000.0;
        let cad = |c: Option<u32>| c.map(|c| format!(" Cadence=\"{c}\"")).unwrap_or_default();
        let mut x = String::from("<workout_file>\n    <author>RideCore</author>\n");
        let _ = writeln!(x, "    <name>{}</name>", xml_escape(&self.name));
        let _ = writeln!(
            x,
            "    <description>{}</description>",
            xml_escape(&self.notes)
        );
        x.push_str("    <sportType>bike</sportType>\n    <workout>\n");
        for b in &self.blocks {
            let _ = match *b {
                Block::Steady {
                    secs,
                    power,
                    cadence,
                } => writeln!(
                    x,
                    "        <SteadyState Duration=\"{secs}\" Power=\"{}\"{}/>",
                    f(power),
                    cad(cadence)
                ),
                Block::Ramp {
                    secs,
                    from,
                    to,
                    cadence,
                } => writeln!(
                    x,
                    "        <Ramp Duration=\"{secs}\" PowerLow=\"{}\" PowerHigh=\"{}\"{}/>",
                    f(from),
                    f(to),
                    cad(cadence)
                ),
                Block::Intervals {
                    reps,
                    on_secs,
                    on,
                    off_secs,
                    off,
                    cadence,
                } => writeln!(
                    x,
                    "        <IntervalsT Repeat=\"{reps}\" OnDuration=\"{on_secs}\" OffDuration=\"{off_secs}\" OnPower=\"{}\" OffPower=\"{}\"{}/>",
                    f(on),
                    f(off),
                    cad(cadence)
                ),
                Block::Free { secs } => writeln!(x, "        <FreeRide Duration=\"{secs}\"/>"),
            };
        }
        x.push_str("    </workout>\n</workout_file>\n");
        x
    }

    /// Reads a `.zwo` file into a workout.
    pub fn from_zwo(xml: &str, fallback_name: &str) -> anyhow::Result<Self> {
        let w = Workout::parse(xml)?;
        let name = if w.name.trim().is_empty() {
            fallback_name
        } else {
            w.name.trim()
        };
        Ok(Self::from_segments(name, &w.segments))
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Summary numbers of a workout for one FTP.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub secs: u32,
    pub avg_w: f32,
    pub np_w: f32,
    pub intensity: f32,
    pub tss: f32,
    pub kj: f32,
}

pub fn stats(w: &CustomWorkout, ftp: f32) -> Stats {
    let series = w.series(ftp);
    let secs = series.len() as u32;
    if secs == 0 {
        return Stats::default();
    }
    let avg_w = series.iter().sum::<f32>() / secs as f32;
    let np_w = crate::training::normalized_power(&series);
    let intensity = np_w / ftp.max(1.0);
    Stats {
        secs,
        avg_w,
        np_w,
        intensity,
        tss: secs as f32 * np_w * intensity / (ftp.max(1.0) * 3600.0) * 100.0,
        kj: series.iter().sum::<f32>() / 1000.0,
    }
}

fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/RideCore/workouts.json"))
}

/// Folder for exported `.zwo` files.
pub fn export_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Documents/RideCore/workouts")
}

pub fn load_all() -> Vec<CustomWorkout> {
    store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_all(workouts: &[CustomWorkout]) -> anyhow::Result<()> {
    let path = store_path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(workouts)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Writes the workout to the export folder and returns the file.
pub fn export(w: &CustomWorkout, ftp: f32) -> anyhow::Result<PathBuf> {
    let dir = export_dir();
    std::fs::create_dir_all(&dir)?;
    let stem: String = w
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.trim_matches('_').is_empty() {
        "workout".to_string()
    } else {
        stem
    };
    let path = dir.join(format!("{stem}.zwo"));
    std::fs::write(&path, w.to_zwo(ftp))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CustomWorkout {
        let mut w = CustomWorkout::new("Test");
        w.blocks = vec![
            Block::Ramp {
                secs: 600,
                from: 125.0,
                to: 190.0,
                cadence: None,
            },
            Block::Intervals {
                reps: 4,
                on_secs: 240,
                on: 290.0,
                off_secs: 180,
                off: 140.0,
                cadence: Some(95),
            },
            Block::Free { secs: 300 },
        ];
        w
    }

    #[test]
    fn blocks_become_segments() {
        let w = sample();
        assert_eq!(w.secs(), 600 + 4 * 420 + 300);
        let segs = w.segments(250.0);
        assert_eq!(segs.len(), 1 + 8 + 1);
        assert_eq!(
            segs[1],
            Segment::Steady {
                secs: 240,
                power: 290.0 / 250.0,
                cadence: Some(95)
            }
        );
        let plan = w.to_plan(250.0);
        assert_eq!(plan.work_blocks().len(), 4);
        assert_eq!(plan.focus, Focus::Vo2max);
        assert_eq!(plan.secs(), w.secs());
    }

    #[test]
    fn unit_switch_keeps_the_watts() {
        let mut w = sample();
        w.set_unit(Unit::Percent, 250.0);
        assert_eq!(w.blocks[1].peak(), Some(116.0));
        w.set_unit(Unit::Watts, 250.0);
        assert_eq!(w.blocks[1].peak(), Some(290.0));
    }

    #[test]
    fn zwo_round_trip_folds_intervals() {
        let w = sample();
        let back = CustomWorkout::from_zwo(&w.to_zwo(250.0), "x").unwrap();
        assert_eq!(back.name, "Test");
        assert_eq!(back.blocks.len(), 3);
        assert!(matches!(
            back.blocks[1],
            Block::Intervals {
                reps: 4,
                on_secs: 240,
                off_secs: 180,
                ..
            }
        ));
        let (a, b) = (stats(&w, 250.0), stats(&back, 250.0));
        assert_eq!(a.secs, b.secs);
        assert!((a.tss - b.tss).abs() < 0.5, "{} vs {}", a.tss, b.tss);
    }

    #[test]
    fn library_workouts_become_editable() {
        let lib = crate::training::library(250.0);
        let r = lib.iter().find(|p| p.id == "ronnestad-30-15").unwrap();
        let w = CustomWorkout::from_segments(&r.name, &r.workout.segments);
        assert_eq!(w.secs(), r.secs());
        // 3 sets of 13 × 30/15 fold into interval blocks.
        let reps: u32 = w
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Intervals { reps, .. } => Some(*reps),
                _ => None,
            })
            .sum();
        assert!(reps >= 36, "{reps}");
        assert!(w.blocks.len() < 20, "{}", w.blocks.len());
    }

    #[test]
    fn names_with_markup_export_safely() {
        let mut w = sample();
        w.name = "A & B <3".into();
        let x = w.to_zwo(250.0);
        assert!(x.contains("A &amp; B &lt;3"));
        assert_eq!(CustomWorkout::from_zwo(&x, "x").unwrap().name, "A & B <3");
    }
}
