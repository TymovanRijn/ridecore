//! Race lab: runs hundreds of headless races at thousands of times real
//! speed and checks how the bot riders race against what happens in real
//! road races. Run it after changing the race AI:
//!
//!     cargo run --release --example race_lab [races per scenario]
//!
//! Each scenario is a course and a field. An even field (every rider within
//! a few tenths of a W/kg, like a pro peloton or one race category) should
//! race like the real thing: flat races mostly end in a bunch sprint won by
//! a sprinter, summit finishes go to the best climbers. A mixed field (the
//! game's default spread of team strengths) is looser, like an open
//! amateur race.
//!
//! The player's slot is ridden by the bots' own pacing, so the lab measures
//! the bots and not a scripted player.

use std::time::Instant;

use ridecore::race::ai::{self, Intent};
use ridecore::race::field::{RiderKind, TeamSpec, rival_teams};
use ridecore::race::{PlayerSetup, Race, RaceConfig, pack};
use ridecore::route::{Route, real_routes};

/// A bunch sprint: the winner comes from a group of at least this many
/// riders with a kilometer to go.
const BUNCH: usize = 8;
/// Matches `BRIDGE_REACH_M` in the race.
const BRIDGE_REACH_M: f32 = 250.0;
const STEP_S: f32 = 0.25;

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Even,
    Mixed,
}

/// A measured share or count with the band real racing puts it in.
struct Target {
    what: &'static str,
    lo: f32,
    hi: f32,
    get: fn(&Summary) -> f32,
}

struct Scenario {
    name: &'static str,
    route: &'static str,
    laps: u32,
    field: Field,
    targets: Vec<Target>,
}

#[derive(Default)]
struct Outcome {
    winner_kind: usize,
    /// Riders on the winner's time.
    winning_group: usize,
    /// The winner's group a kilometer from the line.
    group_at_1k: usize,
    /// 1 = the strongest rider by W/kg won.
    winner_rank: usize,
    attacks: u32,
    kmh: f32,
    /// Average power of riders sitting in the bunch, as a share of FTP.
    sitting: f32,
    sim_s: f32,
}

#[derive(Default)]
struct Summary {
    races: usize,
    sprinter_wins: f32,
    bunch_sprints: f32,
    sprints_to_sprinters: f32,
    small_group: f32,
    top5: f32,
    attacks: f32,
    kmh: f32,
    sitting: f32,
}

impl Summary {
    fn of(outcomes: &[Outcome]) -> Self {
        let n = outcomes.len() as f32;
        let share =
            |f: &dyn Fn(&Outcome) -> bool| outcomes.iter().filter(|o| f(o)).count() as f32 / n;
        let sprints = outcomes.iter().filter(|o| o.group_at_1k >= BUNCH).count() as f32;
        Self {
            races: outcomes.len(),
            sprinter_wins: share(&|o| o.winner_kind == RiderKind::Sprinter as usize),
            bunch_sprints: sprints / n,
            sprints_to_sprinters: outcomes
                .iter()
                .filter(|o| o.group_at_1k >= BUNCH && o.winner_kind == RiderKind::Sprinter as usize)
                .count() as f32
                / sprints.max(1.0),
            small_group: share(&|o| o.winning_group <= 3),
            top5: share(&|o| o.winner_rank <= 5),
            attacks: outcomes.iter().map(|o| o.attacks as f32).sum::<f32>() / n,
            kmh: outcomes.iter().map(|o| o.kmh).sum::<f32>() / n,
            sitting: outcomes.iter().map(|o| o.sitting).sum::<f32>() / n,
        }
    }
}

fn scenarios() -> Vec<Scenario> {
    let t = |what, lo, hi, get| Target { what, lo, hi, get };
    vec![
        Scenario {
            name: "Flat, even field",
            route: "flat-loop",
            laps: 4,
            field: Field::Even,
            targets: vec![
                // Most flat races end in a bunch sprint; breaks win some.
                t("bunch sprint", 0.35, 0.85, |s| s.bunch_sprints),
                t("bunch sprints won by a sprinter", 0.6, 1.0, |s| {
                    s.sprints_to_sprinters
                }),
                t("attacks per race", 2.0, 15.0, |s| s.attacks),
                // In the wheels a rider rides well under threshold.
                t("power sitting in the bunch (× FTP)", 0.5, 0.85, |s| {
                    s.sitting
                }),
            ],
        },
        Scenario {
            name: "Coast, even field",
            route: "kust-noordwijk",
            laps: 1,
            field: Field::Even,
            // A short flat race: a break hardly ever survives.
            targets: vec![
                t("bunch sprint", 0.6, 1.0, |s| s.bunch_sprints),
                t("won by a sprinter", 0.5, 1.0, |s| s.sprinter_wins),
            ],
        },
        Scenario {
            name: "Liège finale, mixed field",
            route: "lbl-finale",
            laps: 1,
            field: Field::Mixed,
            targets: vec![
                // Hilly classics end in small groups among the strongest.
                t("won from a group of three or fewer", 0.6, 1.0, |s| {
                    s.small_group
                }),
                t("won by a top-5 rider (W/kg)", 0.5, 1.0, |s| s.top5),
                t("won by a sprinter", 0.0, 0.3, |s| s.sprinter_wins),
            ],
        },
        Scenario {
            name: "Alpe d'Huez, even field",
            route: "alpe-dhuez",
            laps: 1,
            field: Field::Even,
            targets: vec![
                t("won by a top-5 rider (W/kg)", 0.6, 1.0, |s| s.top5),
                t("won by a sprinter", 0.0, 0.1, |s| s.sprinter_wins),
                t("won from a group of three or fewer", 0.8, 1.0, |s| {
                    s.small_group
                }),
                t("attacks per race", 2.0, 25.0, |s| s.attacks),
            ],
        },
        Scenario {
            name: "Flat, mixed field",
            route: "flat-loop",
            laps: 4,
            field: Field::Mixed,
            targets: vec![t("won by a top-5 rider (W/kg)", 0.2, 0.9, |s| s.top5)],
        },
    ]
}

fn teams(field: Field) -> Vec<TeamSpec> {
    let mut teams = rival_teams(6, 3.6, 1.0);
    if field == Field::Even {
        for t in &mut teams {
            t.mean_wkg = 3.9;
            t.std_wkg = 0.15;
        }
    }
    teams
}

fn route(id: &str) -> Route {
    Route::builtin()
        .into_iter()
        .chain(real_routes().iter().cloned())
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no route {id}"))
}

fn run(s: &Scenario, seed: u64) -> Outcome {
    let cfg = RaceConfig {
        seed,
        route: route(s.route),
        laps: s.laps,
        teams: teams(s.field),
        riders_per_team: 6,
        player_team: 5,
        player: PlayerSetup {
            name: "Lab".into(),
            kg: 72.0,
            height_cm: 180.0,
            ftp: 3.9 * 72.0,
            kind: RiderKind::Rouleur,
        },
        countdown_s: 0.0,
    };
    let mut race = Race::new(&cfg);
    let mut attacking = vec![false; race.riders.len()];
    let mut attacks = 0;
    let (mut sitting, mut samples) = (0.0f32, 0.0f32);
    let mut groups_at_1k = None;
    while !race.over() && race.time < 4.0 * 3600.0 {
        // The player's slot rides like a bot sitting in.
        let h = race.human;
        let wheels = pack::wheels(&race.riders, &race.groups);
        let ahead = pack::nearest_ahead(&race.riders, BRIDGE_REACH_M);
        let power = ai::power(&race, h, wheels[h], ahead[h]);
        race.step(STEP_S, power, 88.0);

        if groups_at_1k.is_none()
            && race
                .riders
                .iter()
                .any(|r| race.course.total_m - r.dist < 1000.0)
        {
            groups_at_1k = Some(race.groups.clone());
        }
        for (i, r) in race.riders.iter().enumerate() {
            let now = matches!(r.intent, Intent::Attack { .. });
            attacks += u32::from(now && !attacking[i]);
            attacking[i] = now;
        }
        if race.time > 300.0 && !race.groups.is_empty() {
            for &i in &race.groups[race.peloton()].riders {
                let r = &race.riders[i];
                if !r.human && r.intent == Intent::Sit && r.draft > 0.1 {
                    sitting += r.power / r.profile.ftp;
                    samples += 1.0;
                }
            }
        }
    }
    let finish = &race.finish;
    let winner = &race.riders[finish[0].rider];
    let mut wkg: Vec<f32> = race.riders.iter().map(|r| r.profile.wkg()).collect();
    wkg.sort_by(|a, b| b.total_cmp(a));
    Outcome {
        winner_kind: winner.profile.kind as usize,
        winning_group: finish.iter().filter(|f| f.gap_s == 0.0).count(),
        group_at_1k: groups_at_1k
            .unwrap_or_default()
            .iter()
            .find(|g| g.riders.contains(&finish[0].rider))
            .map_or(1, |g| g.riders.len()),
        winner_rank: wkg
            .iter()
            .position(|&w| w == winner.profile.wkg())
            .unwrap_or(0)
            + 1,
        attacks,
        kmh: race.course.total_m as f32 / finish[0].time * 3.6,
        sitting: sitting / samples.max(1.0),
        sim_s: race.time,
    }
}

/// Runs the seeds on every core.
fn run_all(s: &Scenario, seeds: u64) -> Vec<Outcome> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                scope.spawn(move || {
                    (t..seeds)
                        .step_by(threads as usize)
                        .map(|seed| run(s, seed))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("race thread"))
            .collect()
    })
}

fn main() {
    let seeds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let (mut passed, mut total) = (0, 0);
    let start = Instant::now();
    let mut simulated = 0.0;
    for s in scenarios() {
        let outcomes = run_all(&s, seeds);
        simulated += outcomes.iter().map(|o| o.sim_s as f64).sum::<f64>();
        let sum = Summary::of(&outcomes);
        let mut kinds = [0; 4];
        for o in &outcomes {
            kinds[o.winner_kind] += 1;
        }
        println!(
            "\n{} ({} races, winner at {:.1} km/h, {:.1} attacks)",
            s.name, sum.races, sum.kmh, sum.attacks
        );
        println!(
            "  winners: {} sprinters, {} climbers, {} rouleurs, {} domestiques",
            kinds[0], kinds[1], kinds[2], kinds[3]
        );
        for t in &s.targets {
            let v = (t.get)(&sum);
            let ok = (t.lo..=t.hi).contains(&v);
            passed += usize::from(ok);
            total += 1;
            let fmt = |x: f32| {
                if t.hi > 1.0 {
                    format!("{x:.1}")
                } else {
                    format!("{:.0}%", x * 100.0)
                }
            };
            println!(
                "  {} {:<38} {:>6}   (real racing: {}–{})",
                if ok { "✓" } else { "✗" },
                t.what,
                fmt(v),
                fmt(t.lo),
                fmt(t.hi)
            );
        }
    }
    let wall = start.elapsed().as_secs_f64();
    println!(
        "\n{passed}/{total} targets met · {:.0} hours of racing in {wall:.0} s ({:.0}× real time)",
        simulated / 3600.0,
        simulated / wall
    );
}
