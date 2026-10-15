//! Training: a library of structured workouts built on what sport science
//! says raises VO2max and FTP, the power zones they work in, and how a
//! ridden session is scored. The trainer holds each target in ERG mode, so
//! the rider only has to keep pedalling.

use crate::i18n::t;
use crate::workout::{Segment, Workout};

/// Power zones as fractions of FTP (Coggan, with sweet spot split out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Zone {
    Recovery,
    Endurance,
    Tempo,
    SweetSpot,
    Threshold,
    Vo2max,
    Anaerobic,
}

impl Zone {
    pub const ALL: [Zone; 7] = [
        Zone::Recovery,
        Zone::Endurance,
        Zone::Tempo,
        Zone::SweetSpot,
        Zone::Threshold,
        Zone::Vo2max,
        Zone::Anaerobic,
    ];

    pub fn of(ftp_fraction: f32) -> Zone {
        match ftp_fraction {
            f if f < 0.56 => Zone::Recovery,
            f if f < 0.76 => Zone::Endurance,
            f if f < 0.88 => Zone::Tempo,
            f if f < 0.95 => Zone::SweetSpot,
            f if f < 1.06 => Zone::Threshold,
            f if f < 1.21 => Zone::Vo2max,
            _ => Zone::Anaerobic,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Zone::Recovery => t("Herstel", "Recovery"),
            Zone::Endurance => t("Duur", "Endurance"),
            Zone::Tempo => "Tempo",
            Zone::SweetSpot => "Sweet spot",
            Zone::Threshold => t("Drempel", "Threshold"),
            Zone::Vo2max => "VO2max",
            Zone::Anaerobic => t("Anaeroob", "Anaerobic"),
        }
    }

    /// Zone colors, cool to hot, as training apps use them.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Zone::Recovery => [128, 136, 150],
            Zone::Endurance => [56, 150, 245],
            Zone::Tempo => [60, 200, 120],
            Zone::SweetSpot => [235, 205, 50],
            Zone::Threshold => [255, 150, 30],
            Zone::Vo2max => [240, 70, 50],
            Zone::Anaerobic => [200, 60, 200],
        }
    }

    /// Hard enough to count as an interval.
    pub fn is_work(self) -> bool {
        self >= Zone::SweetSpot
    }
}

/// What a workout is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Focus {
    Vo2max,
    Threshold,
    SweetSpot,
    Endurance,
    Recovery,
    Test,
}

impl Focus {
    pub fn label(self) -> &'static str {
        match self {
            Focus::Vo2max => "VO2max",
            Focus::Threshold => "FTP",
            Focus::SweetSpot => "Sweet spot",
            Focus::Endurance => t("Basis", "Base"),
            Focus::Recovery => t("Herstel", "Recovery"),
            Focus::Test => "Test",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Focus::Vo2max => Zone::Vo2max.rgb(),
            Focus::Threshold => Zone::Threshold.rgb(),
            Focus::SweetSpot => Zone::SweetSpot.rgb(),
            Focus::Endurance => Zone::Endurance.rgb(),
            Focus::Recovery => Zone::Recovery.rgb(),
            Focus::Test => [150, 110, 255],
        }
    }
}

/// One workout in the library, with why it works.
#[derive(Debug, Clone)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub focus: Focus,
    /// One line for the card.
    pub summary: String,
    /// Why it works, in plain words.
    pub why: String,
    /// Where the protocol comes from.
    pub source: String,
    pub workout: Workout,
    /// Leading segments that are warm-up, openers included: not intervals.
    pub warmup: usize,
    /// A ramp test: it ends when you can't hold the step any more, and sets
    /// a new FTP.
    pub ramp_test: bool,
}

impl Plan {
    pub fn secs(&self) -> u32 {
        self.workout.segments.iter().map(Segment::secs).sum()
    }

    /// Training stress score: an hour at FTP is 100.
    pub fn tss(&self) -> f32 {
        let mut sum = 0.0;
        for s in &self.workout.segments {
            let secs = s.secs() as f32;
            let f = match *s {
                Segment::Steady { power, .. } => power * power,
                Segment::Ramp { from, to, .. } => (from * from + from * to + to * to) / 3.0,
                Segment::FreeRide { .. } => 0.36,
            };
            sum += f * secs;
        }
        sum / 3600.0 * 100.0
    }

    /// The hard blocks, in order: the intervals the rider counts down.
    pub fn work_blocks(&self) -> Vec<usize> {
        self.workout
            .segments
            .iter()
            .enumerate()
            .skip(self.warmup)
            .filter(|(_, s)| peak(s).is_some_and(|p| Zone::of(p).is_work()))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Highest target in a segment, as an FTP fraction.
pub fn peak(s: &Segment) -> Option<f32> {
    match *s {
        Segment::Steady { power, .. } => Some(power),
        Segment::Ramp { from, to, .. } => Some(from.max(to)),
        Segment::FreeRide { .. } => None,
    }
}

/// Builds a workout block by block.
struct Build(Vec<Segment>);

impl Build {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn ramp(mut self, mins: f32, from: f32, to: f32) -> Self {
        self.0.push(Segment::Ramp {
            secs: (mins * 60.0) as u32,
            from,
            to,
            cadence: None,
        });
        self
    }
    fn steady(mut self, secs: u32, power: f32, cadence: Option<u32>) -> Self {
        self.0.push(Segment::Steady {
            secs,
            power,
            cadence,
        });
        self
    }
    /// `n` repeats of on/off; the last rest is left out.
    fn reps(mut self, n: u32, on: (u32, f32), off: (u32, f32), cadence: Option<u32>) -> Self {
        for k in 0..n {
            self = self.steady(on.0, on.1, cadence);
            if k + 1 < n {
                self = self.steady(off.0, off.1, None);
            }
        }
        self
    }
    /// The standard warm-up (`WARM_UP` blocks): an easy ramp and a couple
    /// of openers.
    fn warm_up(self) -> Self {
        self.ramp(6.0, 0.45, 0.7)
            .steady(120, 0.7, None)
            .steady(30, 1.05, Some(100))
            .steady(60, 0.55, None)
            .steady(30, 1.1, Some(100))
            .steady(120, 0.55, None)
    }
    fn cool_down(self, mins: f32) -> Self {
        self.ramp(mins, 0.6, 0.4)
    }
    fn done(self, name: &str) -> Workout {
        Workout {
            name: name.into(),
            segments: self.0,
        }
    }
}

/// Blocks in `Build::warm_up`.
const WARM_UP: usize = 6;

/// The workout library. `ftp` is needed for the ramp test, which steps up
/// in fixed watts.
pub fn library(ftp: f32) -> Vec<Plan> {
    let ftp = ftp.max(80.0);
    let mut plans = vec![
        Plan {
            id: "kennismaking".into(),
            name: t("Kennismaking", "First ride").into(),
            focus: Focus::Endurance,
            summary: t(
                "20 min: voel hoe ERG werkt, met drie korte versnellingen",
                "20 min: feel how ERG works, with three short efforts",
            )
            .into(),
            why: t(
                "Een rustige eerste rit. De trainer zet zelf de weerstand: jij trapt \
                 gewoon door, harder of zachter trappen verandert je vermogen niet. \
                 Probeer tijdens de versnellingen eens een hogere cadans.",
                "An easy first ride. The trainer sets the resistance itself: you just \
                 keep pedalling, and pedalling harder or softer doesn't change your \
                 power. Try a higher cadence during the efforts.",
            )
            .into(),
            source: "".into(),
            workout: Build::new()
                .ramp(5.0, 0.45, 0.65)
                .steady(180, 0.68, None)
                .reps(3, (60, 0.95), (90, 0.6), Some(95))
                .steady(180, 0.68, None)
                .cool_down(3.5)
                .done("Kennismaking"),
            warmup: 0,
            ramp_test: false,
        },
        Plan {
            id: "ronnestad-30-15".into(),
            name: "Rønnestad 30/15".into(),
            focus: Focus::Vo2max,
            summary: t(
                "3 × 13 × (30 s hard / 15 s rustig) — de VO2max-kraker",
                "3 × 13 × (30 s hard / 15 s easy) — the VO2max cracker",
            )
            .into(),
            why: t(
                "Korte blokken met nog kortere rust houden je zuurstofopname lang \
                 dicht bij het maximum, langer dan bij lange intervallen. In de studie \
                 van Rønnestad gingen wielrenners die dit drie keer per week deden er \
                 in tien weken duidelijk meer op vooruit (VO2max en vermogen) dan met \
                 4 × 5 minuten.",
                "Short blocks with even shorter rests keep your oxygen uptake close \
                 to its maximum for longer than long intervals do. In Rønnestad's \
                 study, cyclists who did this three times a week improved clearly \
                 more in ten weeks (VO2max and power) than with 4 × 5 minutes.",
            )
            .into(),
            source: "Rønnestad et al., Scand J Med Sci Sports 2015 & 2020".into(),
            workout: {
                let mut b = Build::new().warm_up();
                for set in 0..3 {
                    b = b.reps(13, (30, 1.2), (15, 0.5), Some(100));
                    if set < 2 {
                        b = b.steady(180, 0.5, None);
                    }
                }
                b.cool_down(8.0).done("Rønnestad 30/15")
            },
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "seiler-4x8".into(),
            name: "Seiler 4 × 8".into(),
            focus: Focus::Vo2max,
            summary: t(
                "4 × 8 min net boven je FTP, 2 min rust",
                "4 × 8 min just above your FTP, 2 min rest",
            )
            .into(),
            why: t(
                "Seiler liet goed getrainde sporters 4 × 4, 4 × 8 en 4 × 16 minuten \
                 rijden. De groep met 4 × 8 minuten op een pittig maar vol te houden \
                 tempo won het meest. Lang genoeg om je hart en longen te prikkelen, \
                 kort genoeg om het vol te houden.",
                "Seiler had well-trained athletes ride 4 × 4, 4 × 8 and 4 × 16 minutes. \
                 The 4 × 8 minute group, at a hard but sustainable pace, gained the \
                 most. Long enough to push your heart and lungs, short enough to hold.",
            )
            .into(),
            source: "Seiler et al., Scand J Med Sci Sports 2013".into(),
            workout: Build::new()
                .warm_up()
                .reps(4, (480, 1.05), (120, 0.5), Some(95))
                .cool_down(8.0)
                .done("Seiler 4 × 8"),
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "helgerud-4x4".into(),
            name: t("Noorse 4 × 4", "Norwegian 4 × 4").into(),
            focus: Focus::Vo2max,
            summary: t(
                "4 × 4 min diep, 3 min actieve rust",
                "4 × 4 min deep, 3 min active rest",
            )
            .into(),
            why: t(
                "De klassieker uit Noorwegen: vier blokken van vier minuten op \
                 90–95% van je maximale hartslag. Je hartslagvolume groeit en je \
                 VO2max stijgt snel, ook als je al fit bent.",
                "The Norwegian classic: four blocks of four minutes at 90–95% of \
                 your maximum heart rate. Your stroke volume grows and your VO2max \
                 rises quickly, even if you're already fit.",
            )
            .into(),
            source: "Helgerud et al., Med Sci Sports Exerc 2007".into(),
            workout: Build::new()
                .warm_up()
                .reps(4, (240, 1.15), (180, 0.55), Some(95))
                .cool_down(8.0)
                .done("Noorse 4 × 4"),
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "40-20".into(),
            name: "40/20's".into(),
            focus: Focus::Vo2max,
            summary: t(
                "2 × 10 × (40 s / 20 s) — kort, fel en verslavend",
                "2 × 10 × (40 s / 20 s) — short, sharp and addictive",
            )
            .into(),
            why: t(
                "Net als 30/15 een manier om lang rond je VO2max te zitten zonder \
                 dat het één groot gevecht wordt. De korte rust houdt de inspanning \
                 hoog; dit is ook precies hoe aanvallen in een koers voelen.",
                "Like 30/15, a way to spend a long time near your VO2max without it \
                 turning into one big fight. The short rests keep the effort high; \
                 this is also exactly how attacks in a race feel.",
            )
            .into(),
            source: t(
                "Gebaseerd op Billat (30-30) en Rønnestad",
                "Based on Billat (30-30) and Rønnestad",
            )
            .into(),
            workout: Build::new()
                .warm_up()
                .reps(10, (40, 1.2), (20, 0.5), Some(100))
                .steady(300, 0.5, None)
                .reps(10, (40, 1.2), (20, 0.5), Some(100))
                .cool_down(8.0)
                .done("40/20's"),
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "sweetspot-3x12".into(),
            name: "Sweet spot 3 × 12".into(),
            focus: Focus::SweetSpot,
            summary: t(
                "3 × 12 min op 90% FTP: veel winst, weinig vermoeidheid",
                "3 × 12 min at 90% FTP: big gains, little fatigue",
            )
            .into(),
            why: t(
                "Net onder je drempel (88–94% FTP) bouw je veel trainingsprikkel op \
                 voor relatief weinig vermoeidheid. Ideaal om je FTP op te krikken \
                 als je maar een paar uur per week hebt.",
                "Just below your threshold (88–94% FTP) you build a lot of training \
                 stimulus for relatively little fatigue. Ideal for raising your FTP \
                 when you only have a few hours a week.",
            )
            .into(),
            source: "Coggan & Allen, Training and Racing with a Power Meter".into(),
            workout: Build::new()
                .warm_up()
                .reps(3, (720, 0.9), (240, 0.55), Some(90))
                .cool_down(8.0)
                .done("Sweet spot 3 × 12"),
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "threshold-2x20".into(),
            name: t("Drempel 2 × 20", "Threshold 2 × 20").into(),
            focus: Focus::Threshold,
            summary: t(
                "2 × 20 min op 95–100% FTP: de FTP-klassieker",
                "2 × 20 min at 95–100% FTP: the FTP classic",
            )
            .into(),
            why: t(
                "Lang op je drempel rijden leert je lichaam om melkzuur sneller op \
                 te ruimen. Twee keer twintig minuten is zwaar maar haalbaar, en het \
                 is de training die het meest lijkt op een lange klim of een \
                 tijdrit.",
                "Riding at threshold for a long time teaches your body to clear \
                 lactate faster. Two times twenty minutes is hard but doable, and \
                 it's the workout that feels most like a long climb or a time trial.",
            )
            .into(),
            source: t(
                "Coggan & Allen; Hunter Allen's FTP-protocol",
                "Coggan & Allen; Hunter Allen's FTP protocol",
            )
            .into(),
            workout: Build::new()
                .warm_up()
                .steady(1200, 0.96, Some(90))
                .steady(300, 0.55, None)
                .steady(1200, 0.96, Some(90))
                .cool_down(8.0)
                .done("Drempel 2 × 20"),
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "over-unders".into(),
            name: "Over-unders".into(),
            focus: Focus::Threshold,
            summary: t(
                "3 × 9 min wisselend net onder en net boven je drempel",
                "3 × 9 min switching just below and just above threshold",
            )
            .into(),
            why: t(
                "Afwisselend 95% en 108% van je FTP: boven je drempel maak je \
                 melkzuur aan, net eronder leer je het weer kwijt te raken terwijl \
                 je doorrijdt. Precies wat je nodig hebt als het tempo in een koers \
                 steeds wordt opgeschroefd.",
                "Alternating 95% and 108% of your FTP: above threshold you build up \
                 lactate, just below it you learn to clear it while you keep riding. \
                 Exactly what you need when the pace in a race keeps going up.",
            )
            .into(),
            source: t(
                "Gebruikelijk protocol bij Carmichael Training Systems",
                "A common protocol at Carmichael Training Systems",
            )
            .into(),
            workout: {
                let mut b = Build::new().warm_up();
                for set in 0..3 {
                    for _ in 0..3 {
                        b = b.steady(120, 0.95, Some(90)).steady(60, 1.08, Some(95));
                    }
                    if set < 2 {
                        b = b.steady(300, 0.55, None);
                    }
                }
                b.cool_down(8.0).done("Over-unders")
            },
            warmup: WARM_UP,
            ramp_test: false,
        },
        Plan {
            id: "zone2-60".into(),
            name: t("Duurrit Zone 2", "Zone 2 endurance").into(),
            focus: Focus::Endurance,
            summary: t(
                "60 min rustig duurvermogen: de basis onder alles",
                "60 min of easy endurance: the base under everything",
            )
            .into(),
            why: t(
                "Topsporters rijden zo'n 80% van hun tijd rustig. In zone 2 groeien \
                 je mitochondriën en haarvaten, en verbrand je meer vet, zonder dat \
                 je moe wordt voor de zware dagen.",
                "Top athletes ride about 80% of their time easy. In zone 2 your \
                 mitochondria and capillaries grow and you burn more fat, without \
                 getting tired for the hard days.",
            )
            .into(),
            source: "Seiler, 'polarised training'; Stöggl & Sperlich 2014".into(),
            workout: Build::new()
                .ramp(8.0, 0.45, 0.68)
                .steady(900, 0.68, Some(88))
                .steady(60, 0.75, Some(95))
                .steady(900, 0.68, Some(88))
                .steady(60, 0.75, Some(95))
                .steady(660, 0.68, Some(88))
                .cool_down(5.0)
                .done("Duurrit Zone 2"),
            warmup: 0,
            ramp_test: false,
        },
        Plan {
            id: "recovery-30".into(),
            name: t("Hersteltraining", "Recovery ride").into(),
            focus: Focus::Recovery,
            summary: t(
                "30 min heel rustig losrijden",
                "30 min of very easy spinning",
            )
            .into(),
            why: t(
                "Na een zware dag of een koers helpt rustig doortrappen je benen \
                 sneller te herstellen dan stilzitten. Het mag echt makkelijk \
                 voelen.",
                "After a hard day or a race, easy pedalling helps your legs recover \
                 faster than sitting still. It should feel really easy.",
            )
            .into(),
            source: "".into(),
            workout: Build::new()
                .ramp(5.0, 0.4, 0.5)
                .steady(1200, 0.5, Some(90))
                .ramp(5.0, 0.5, 0.4)
                .done("Hersteltraining"),
            warmup: 0,
            ramp_test: false,
        },
        Plan {
            id: "ramp-test".into(),
            name: t("Ramptest", "Ramp test").into(),
            focus: Focus::Test,
            summary: t(
                "Elke minuut 20 W zwaarder, tot je niet meer kan",
                "20 W harder every minute, until you can't go on",
            )
            .into(),
            why: t(
                "De snelste manier om je FTP te meten. Na een korte warming-up gaat \
                 het vermogen elke minuut 20 W omhoog. Houd vol zolang het lukt; je \
                 FTP is ongeveer 75% van je beste minuut. Daarna rekent RideCore \
                 overal met je nieuwe FTP.",
                "The quickest way to measure your FTP. After a short warm-up the \
                 power goes up 20 W every minute. Hold on as long as you can; your \
                 FTP is about 75% of your best minute. After that RideCore uses your \
                 new FTP everywhere.",
            )
            .into(),
            source: t(
                "Ramptest-protocol zoals gebruikt door Zwift en TrainerRoad",
                "Ramp test protocol as used by Zwift and TrainerRoad",
            )
            .into(),
            workout: {
                let mut b = Build::new().ramp(5.0, 0.4, 0.55).steady(60, 0.55, None);
                let start = 100.0_f32.max(ftp * 0.4).round();
                // Most riders crack around 1.35 × FTP; the steps go on a
                // little beyond that for a great day.
                let mut w = start;
                while w <= ftp * 1.7 {
                    b = b.steady(60, w / ftp, Some(95));
                    w += 20.0;
                }
                b.done("Ramptest")
            },
            warmup: 0,
            ramp_test: true,
        },
    ];
    // The recorded workout carries the name in the current language.
    for p in &mut plans {
        p.workout.name = p.name.clone();
    }
    plans
}

/// FTP from a ramp test: 75% of the best one-minute power.
pub fn ftp_from_ramp(best_minute_w: f32) -> f32 {
    (best_minute_w * 0.75).round()
}

/// How one interval went.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockScore {
    pub secs: f32,
    pub watts: f32,
    pub target: f32,
}

impl BlockScore {
    pub fn add(&mut self, dt: f32, watts: f32, target: f32) {
        self.secs += dt;
        self.watts += watts * dt;
        self.target += target * dt;
    }

    /// Average power over the average target.
    pub fn ratio(&self) -> f32 {
        if self.target <= 0.0 {
            0.0
        } else {
            self.watts / self.target
        }
    }

    /// Held it: at least 95% of the target on average.
    pub fn nailed(&self) -> bool {
        self.ratio() >= 0.95
    }
}

/// Seconds spent in each zone, from one-second power samples.
pub fn zone_seconds(watts: &[f32], ftp: f32) -> [u32; 7] {
    let mut out = [0; 7];
    for &w in watts {
        out[Zone::of(w / ftp) as usize] += 1;
    }
    out
}

/// Normalized power: the 30 s rolling average to the fourth power, averaged,
/// fourth root. Close to what a ride "cost".
pub fn normalized_power(watts: &[f32]) -> f32 {
    if watts.len() < 30 {
        return watts.iter().sum::<f32>() / watts.len().max(1) as f32;
    }
    let rolled: Vec<f32> = watts
        .windows(30)
        .map(|w| w.iter().sum::<f32>() / 30.0)
        .collect();
    (rolled.iter().map(|p| p.powi(4)).sum::<f32>() / rolled.len() as f32).powf(0.25)
}

/// Best average power over `secs` seconds.
pub fn best_average(watts: &[f32], secs: usize) -> f32 {
    if watts.len() < secs || secs == 0 {
        return 0.0;
    }
    watts
        .windows(secs)
        .map(|w| w.iter().sum::<f32>() / secs as f32)
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_workout_is_sane() {
        for p in library(250.0) {
            let mins = p.secs() as f32 / 60.0;
            assert!((15.0..=95.0).contains(&mins), "{} lasts {mins} min", p.name);
            assert!(
                p.tss() > 10.0 && p.tss() < 150.0,
                "{} TSS {}",
                p.name,
                p.tss()
            );
            if !matches!(p.focus, Focus::Endurance | Focus::Recovery) {
                assert!(!p.work_blocks().is_empty(), "{} has no intervals", p.name);
            }
        }
    }

    #[test]
    fn the_30_15s_are_39_intervals_and_hurt_more_than_zone_2() {
        let lib = library(250.0);
        let find = |id: &str| lib.iter().find(|p| p.id == id).unwrap();
        let r = find("ronnestad-30-15");
        assert_eq!(r.work_blocks().len(), 39);
        assert!(r.tss() > find("zone2-60").tss() * 0.9);
        assert!(find("threshold-2x20").tss() > find("sweetspot-3x12").tss());
    }

    #[test]
    fn ramp_test_steps_up_twenty_watts_a_minute() {
        let lib = library(250.0);
        let ramp = lib.iter().find(|p| p.ramp_test).unwrap();
        let w = &ramp.workout;
        let a = w.target_at(6.0 * 60.0 + 30.0, 250).unwrap().watts.unwrap();
        let b = w.target_at(7.0 * 60.0 + 30.0, 250).unwrap().watts.unwrap();
        assert_eq!(b - a, 20);
        assert_eq!(ftp_from_ramp(400.0), 300.0);
    }

    #[test]
    fn zones_and_scores() {
        assert_eq!(Zone::of(0.9), Zone::SweetSpot);
        assert_eq!(Zone::of(1.0), Zone::Threshold);
        assert_eq!(Zone::of(1.2), Zone::Vo2max);
        let mut s = BlockScore::default();
        s.add(30.0, 290.0, 300.0);
        assert!(s.nailed());
        s.add(30.0, 200.0, 300.0);
        assert!(!s.nailed());
        let flat = vec![200.0; 120];
        assert!((normalized_power(&flat) - 200.0).abs() < 0.01);
        let mut spiky = vec![100.0; 60];
        spiky.extend(vec![300.0; 60]);
        assert!(normalized_power(&spiky) > 200.0);
        assert_eq!(best_average(&spiky, 60), 300.0);
        assert_eq!(
            zone_seconds(&[100.0, 250.0], 250.0)[Zone::Threshold as usize],
            1
        );
    }
}
