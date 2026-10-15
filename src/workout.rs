//! Structured workouts in the `.zwo` XML format: parsing and a time-based
//! engine that yields the target power at any point in the session.

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// Power targets are fractions of FTP (1.0 = 100% FTP).
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Steady {
        secs: u32,
        power: f32,
        cadence: Option<u32>,
    },
    Ramp {
        secs: u32,
        from: f32,
        to: f32,
        cadence: Option<u32>,
    },
    /// No target: the trainer stays in simulation mode.
    FreeRide { secs: u32 },
}

impl Segment {
    pub fn secs(&self) -> u32 {
        match *self {
            Self::Steady { secs, .. } | Self::Ramp { secs, .. } | Self::FreeRide { secs } => secs,
        }
    }

    /// Target as FTP fraction `elapsed` seconds into this segment.
    fn target_at(&self, elapsed: f32) -> Option<f32> {
        match *self {
            Self::Steady { power, .. } => Some(power),
            Self::Ramp { secs, from, to, .. } => {
                let t = (elapsed / secs.max(1) as f32).clamp(0.0, 1.0);
                Some(from + (to - from) * t)
            }
            Self::FreeRide { .. } => None,
        }
    }

    fn cadence(&self) -> Option<u32> {
        match *self {
            Self::Steady { cadence, .. } | Self::Ramp { cadence, .. } => cadence,
            Self::FreeRide { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workout {
    pub name: String,
    pub segments: Vec<Segment>,
}

/// What the workout asks for at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub watts: Option<u32>,
    pub cadence: Option<u32>,
    pub segment: usize,
    pub segment_remaining_s: f32,
}

impl Workout {
    /// Returns `None` once the workout is finished.
    pub fn target_at(&self, elapsed_s: f32, ftp: u32) -> Option<Target> {
        let mut start = 0.0;
        for (i, seg) in self.segments.iter().enumerate() {
            let end = start + seg.secs() as f32;
            if elapsed_s < end {
                return Some(Target {
                    watts: seg
                        .target_at(elapsed_s - start)
                        .map(|f| (f * ftp as f32).round() as u32),
                    cadence: seg.cadence(),
                    segment: i,
                    segment_remaining_s: end - elapsed_s,
                });
            }
            start = end;
        }
        None
    }

    pub fn parse(xml: &str) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        // Untrimmed: a name like "A & B" arrives in pieces around the `&amp;`.
        reader.config_mut().trim_text(false);
        let mut name = String::new();
        let mut segments = Vec::new();
        let mut in_name = false;
        let mut in_workout = false;
        loop {
            match reader.read_event().context("invalid workout XML")? {
                Event::Start(e) if e.name().as_ref() == "name" => in_name = true,
                Event::End(e) if e.name().as_ref() == "name" => in_name = false,
                Event::Text(t) if in_name => name.push_str(&t.xml10_content()),
                // `&amp;` and friends arrive as separate events.
                Event::GeneralRef(r) if in_name => name.push_str(match r.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                }),
                Event::Start(e) if e.name().as_ref() == "workout" => in_workout = true,
                Event::End(e) if e.name().as_ref() == "workout" => in_workout = false,
                Event::Start(e) | Event::Empty(e) if in_workout => {
                    push_segments(&e, &mut segments)?
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if segments.is_empty() {
            bail!("workout has no segments");
        }
        let name = name.trim().to_string();
        Ok(Workout { name, segments })
    }
}

fn push_segments(e: &BytesStart, out: &mut Vec<Segment>) -> Result<()> {
    let attr = |key: &str| -> Option<f32> {
        e.try_get_attribute(key)
            .ok()
            .flatten()
            .and_then(|a| a.value.trim().parse().ok())
    };
    let secs = |key: &str| attr(key).map(|v| v.round() as u32).unwrap_or(0);
    let cadence = attr("Cadence").map(|c| c.round() as u32);
    match e.name().as_ref() as &str {
        "SteadyState" => out.push(Segment::Steady {
            secs: secs("Duration"),
            power: attr("Power").context("SteadyState without Power")?,
            cadence,
        }),
        "Warmup" | "Cooldown" | "Ramp" => out.push(Segment::Ramp {
            secs: secs("Duration"),
            from: attr("PowerLow").context("ramp without PowerLow")?,
            to: attr("PowerHigh").context("ramp without PowerHigh")?,
            cadence,
        }),
        "IntervalsT" => {
            let on = attr("OnPower").context("IntervalsT without OnPower")?;
            let off = attr("OffPower").context("IntervalsT without OffPower")?;
            let rest_cadence = attr("CadenceResting").map(|c| c.round() as u32);
            for _ in 0..secs("Repeat").max(1) {
                out.push(Segment::Steady {
                    secs: secs("OnDuration"),
                    power: on,
                    cadence,
                });
                out.push(Segment::Steady {
                    secs: secs("OffDuration"),
                    power: off,
                    cadence: rest_cadence,
                });
            }
        }
        "FreeRide" | "MaxEffort" => out.push(Segment::FreeRide {
            secs: secs("Duration"),
        }),
        _ => {} // text events and unknown elements carry no power target
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Workout {
        pub fn total_secs(&self) -> u32 {
            self.segments.iter().map(Segment::secs).sum()
        }
    }

    const SAMPLE: &str = r#"<workout_file>
        <author>T.Van Rijn</author>
        <name>New Workout</name>
        <sportType>bike</sportType>
        <tags/>
        <workout>
            <Warmup Duration="60" PowerLow="0.4" PowerHigh="0.6"/>
            <SteadyState Duration="300" Power="0.49977168" pace="0"/>
            <IntervalsT Repeat="4" OnDuration="240" OffDuration="180" OnPower="1.2805936" OffPower="0.49977168" pace="0"/>
            <FreeRide Duration="120" FlatRoad="1"/>
        </workout>
    </workout_file>"#;

    #[test]
    fn parses_name_and_expands_intervals() {
        let w = Workout::parse(SAMPLE).unwrap();
        assert_eq!(w.name, "New Workout");
        assert_eq!(w.segments.len(), 1 + 1 + 8 + 1);
        assert_eq!(w.total_secs(), 60 + 300 + 4 * 420 + 120);
    }

    #[test]
    fn ramp_interpolates_target() {
        let w = Workout::parse(SAMPLE).unwrap();
        assert_eq!(w.target_at(0.0, 250).unwrap().watts, Some(100));
        assert_eq!(w.target_at(30.0, 250).unwrap().watts, Some(125));
    }

    #[test]
    fn interval_and_free_ride_targets() {
        let w = Workout::parse(SAMPLE).unwrap();
        let on = w.target_at(360.0 + 10.0, 250).unwrap();
        assert_eq!(on.watts, Some(320));
        assert_eq!(on.segment_remaining_s, 230.0);
        let free = w.target_at(2040.0 + 5.0, 250).unwrap();
        assert_eq!(free.watts, None);
        assert!(w.target_at(w.total_secs() as f32, 250).is_none());
    }

    #[test]
    fn rejects_empty_workout() {
        assert!(Workout::parse("<workout_file><workout/></workout_file>").is_err());
    }
}
