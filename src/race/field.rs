//! Builds the start list: teams with a strength (mean FTP in W/kg and a
//! spread), and riders of different types sampled from that strength.

use crate::physics::{BikeParams, CD, frontal_area_m2};
use crate::i18n::t;
use crate::rng::Rng;

use super::energy::Energy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiderKind {
    Sprinter,
    Climber,
    Rouleur,
    Domestique,
}

impl RiderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sprinter => "Sprinter",
            Self::Climber => t("Klimmer", "Climber"),
            Self::Rouleur => "Rouleur",
            Self::Domestique => t("Knecht", "Domestique"),
        }
    }
}

/// A team's strength as the user configures it.
#[derive(Debug, Clone)]
pub struct TeamSpec {
    pub name: String,
    pub color: [u8; 3],
    pub mean_wkg: f32,
    pub std_wkg: f32,
}

/// One rider's physical profile.
#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub kind: RiderKind,
    pub kg: f32,
    pub height_cm: f32,
    pub ftp: f32,
    /// Best short sprint power in watts.
    pub peak: f32,
    pub w_prime: f32,
    /// 0..1, how readily the rider attacks.
    pub aggression: f32,
}

impl Profile {
    pub fn bike(&self) -> BikeParams {
        BikeParams {
            rider_kg: self.kg,
            // Bigger riders have a bigger frontal area.
            cda: CD * frontal_area_m2(self.height_cm, self.kg),
            ..BikeParams::default()
        }
    }

    pub fn energy(&self) -> Energy {
        Energy::new(self.ftp, self.w_prime)
    }

    pub fn wkg(&self) -> f32 {
        self.ftp / self.kg
    }
}

/// Team line-up by position: leader types first.
const LINEUP: [RiderKind; 8] = [
    RiderKind::Sprinter,
    RiderKind::Climber,
    RiderKind::Rouleur,
    RiderKind::Domestique,
    RiderKind::Rouleur,
    RiderKind::Domestique,
    RiderKind::Domestique,
    RiderKind::Domestique,
];

pub fn sample_rider(rng: &mut Rng, team: &TeamSpec, kind: RiderKind) -> Profile {
    // (W/kg offset from the team mean, kg mean, peak W/kg range, W′ kJ)
    let (wkg_offset, kg_mean, peak_wkg, w_prime_kj) = match kind {
        RiderKind::Sprinter => (-0.25, 76.0, (17.5, 21.0), 28.0),
        RiderKind::Climber => (0.20, 61.0, (12.5, 15.0), 18.0),
        RiderKind::Rouleur => (0.05, 75.0, (14.0, 16.5), 23.0),
        RiderKind::Domestique => (0.0, 70.0, (13.0, 15.5), 20.0),
    };
    let kg = rng.normal(kg_mean, 4.5).clamp(52.0, 92.0);
    // Height tracks weight loosely: lighter riders are usually shorter.
    let height_cm = rng
        .normal(178.0 + (kg - 70.0) * 0.6, 4.0)
        .clamp(160.0, 198.0);
    let wkg = rng
        .normal(team.mean_wkg + wkg_offset, team.std_wkg)
        .clamp(1.5, 7.0);
    Profile {
        name: random_name(rng),
        kind,
        kg,
        height_cm,
        ftp: wkg * kg,
        peak: rng.range(peak_wkg.0, peak_wkg.1) * kg,
        w_prime: rng.normal(w_prime_kj, 2.5).clamp(12.0, 35.0) * 1000.0,
        aggression: rng.f32(),
    }
}

pub fn sample_team(rng: &mut Rng, team: &TeamSpec, size: usize) -> Vec<Profile> {
    (0..size)
        .map(|i| sample_rider(rng, team, LINEUP[i % LINEUP.len()]))
        .collect()
}

/// Rival team presets, strongest first, as multiples of a base W/kg.
pub const TEAM_PRESETS: [(&str, [u8; 3], f32, f32); 11] = [
    ("Oranje Leeuwen", [255, 120, 0], 1.16, 0.20),
    ("Polder Express", [30, 110, 230], 1.10, 0.25),
    ("Kasseien Crew", [200, 30, 40], 1.06, 0.30),
    ("Alpine Velo", [20, 170, 120], 1.03, 0.25),
    ("Duinrijders", [240, 200, 30], 1.00, 0.35),
    ("Windmolen CC", [140, 60, 200], 0.97, 0.30),
    ("Tulip Racing", [230, 80, 160], 0.94, 0.35),
    ("Noordzee Wielren", [0, 180, 220], 0.91, 0.40),
    ("Brug & Berg", [120, 120, 120], 0.88, 0.35),
    ("Kustpijlen", [255, 255, 255], 0.85, 0.40),
    ("Heuvelland", [100, 70, 40], 0.82, 0.40),
];

/// Rival teams whose average strength is `difficulty` × the player's W/kg,
/// spread from top teams to weaker ones.
pub fn rival_teams(count: usize, player_wkg: f32, difficulty: f32) -> Vec<TeamSpec> {
    TEAM_PRESETS
        .iter()
        .take(count)
        .map(|&(name, color, tier, std)| TeamSpec {
            name: name.into(),
            color,
            mean_wkg: (player_wkg * difficulty * tier * 100.0).round() / 100.0,
            std_wkg: std,
        })
        .collect()
}

fn random_name(rng: &mut Rng) -> String {
    const FIRST: [&str; 24] = [
        "Daan", "Lars", "Bram", "Sem", "Jens", "Thijs", "Ruben", "Milan", "Luca", "Pieter",
        "Stijn", "Jonas", "Gijs", "Koen", "Rik", "Tom", "Mads", "Arnaud", "Matteo", "Felix",
        "Oscar", "Nils", "Kasper", "Joris",
    ];
    const LAST: [&str; 26] = [
        "de Vries",
        "Jansen",
        "Bakker",
        "Visser",
        "Smit",
        "Meijer",
        "de Boer",
        "Mulder",
        "Bos",
        "Vos",
        "Peters",
        "Hendriks",
        "van Dijk",
        "Dekker",
        "Brouwer",
        "Kok",
        "Verbeek",
        "Lambert",
        "Rossi",
        "Moreau",
        "Novak",
        "Larsen",
        "Keller",
        "Duarte",
        "Lindqvist",
        "Okafor",
    ];
    format!("{} {}", rng.pick(&FIRST), rng.pick(&LAST))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_teams_produce_stronger_riders() {
        let mut rng = Rng::new(3);
        let teams = rival_teams(11, 3.5, 1.0);
        let avg = |t: &TeamSpec, rng: &mut Rng| {
            let riders: Vec<Profile> = (0..40).flat_map(|_| sample_team(rng, t, 6)).collect();
            riders.iter().map(Profile::wkg).sum::<f32>() / riders.len() as f32
        };
        let top = avg(&teams[0], &mut rng);
        let bottom = avg(&teams[10], &mut rng);
        assert!(top > bottom + 1.0, "top {top} bottom {bottom}");
        // The field average stays close to difficulty × player W/kg.
        assert!((avg(&teams[4], &mut rng) - 3.5).abs() < 0.15);
    }

    #[test]
    fn sprinters_have_the_biggest_kick() {
        let mut rng = Rng::new(9);
        let team = &rival_teams(1, 4.0, 1.0)[0];
        let mut s = 0.0;
        let mut c = 0.0;
        for _ in 0..50 {
            s += sample_rider(&mut rng, team, RiderKind::Sprinter).peak;
            c += sample_rider(&mut rng, team, RiderKind::Climber).peak;
        }
        assert!(s > c * 1.4);
    }
}
