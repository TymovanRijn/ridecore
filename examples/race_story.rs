//! Tells the story of one headless race, minute by minute: the groups on
//! the road, their gaps and what their riders are doing, every attack and
//! catch as it happens, and the result. The quickest way to see why the
//! bots race the way they do:
//!
//!     cargo run --release --example race_story [route] [laps] [seed] [even]
//!
//! e.g. `race_story flat-loop 4 7 even`. Letters per rider, front to back:
//! s = sitting in, p = pulling, a = attacking, b = in the break,
//! m = marking an attack, S = sprinting, o = other.

use ridecore::race::ai::{self, Intent};
use ridecore::race::field::{RiderKind, rival_teams};
use ridecore::race::{PlayerSetup, Race, RaceConfig, pack};
use ridecore::route::{Route, real_routes};

fn letter(intent: &Intent) -> char {
    match intent {
        Intent::Sit => 's',
        Intent::Pull { .. } => 'p',
        Intent::Attack { .. } => 'a',
        Intent::Break => 'b',
        Intent::Mark { .. } => 'm',
        Intent::Sprint { .. } => 'S',
        _ => 'o',
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let id = args.get(1).map_or("flat-loop", String::as_str);
    let laps = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
    let seed = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let even = args.get(4).is_some_and(|s| s == "even");
    let route = Route::builtin()
        .into_iter()
        .chain(real_routes().iter().cloned())
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no route {id}"));
    let mut teams = rival_teams(6, 3.6, 1.0);
    if even {
        for t in &mut teams {
            t.mean_wkg = 3.9;
            t.std_wkg = 0.15;
        }
    }
    let cfg = RaceConfig {
        seed,
        route,
        laps,
        teams,
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
    for t in &race.teams {
        println!(
            "{:<18} plan {:<9} leader {}",
            t.spec.name,
            t.plan.label(),
            race.riders[t.leader].profile.name
        );
    }
    let (mut told, mut next) = (0, 60.0);
    while !race.over() && race.time < 4.0 * 3600.0 {
        // The player's slot rides like a bot sitting in.
        let h = race.human;
        let wheels = pack::wheels(&race.riders, &race.groups);
        let ahead = pack::nearest_ahead(&race.riders, 250.0);
        let power = ai::power(&race, h, wheels[h], ahead[h]);
        race.step(0.25, power, 88.0);

        for e in &race.events[told..] {
            println!("        {:5.0}s  {}", e.time, e.text);
        }
        told = race.events.len();
        if race.time >= next && !race.groups.is_empty() {
            next += 60.0;
            let lead = &race.riders[race.groups[0].riders[0]];
            let groups: Vec<String> = race
                .groups
                .iter()
                .map(|g| {
                    let riders: String = g
                        .riders
                        .iter()
                        .map(|&i| letter(&race.riders[i].intent))
                        .collect();
                    format!("{}@{:.0}s[{riders}]", g.riders.len(), g.gap_s)
                })
                .collect();
            println!(
                "{:3.0} min {:5.1} km {:4.1} km/h | {}",
                race.time / 60.0,
                lead.dist / 1000.0,
                lead.speed * 3.6,
                groups.join(" ")
            );
        }
    }
    println!();
    for f in race.finish.iter().take(10) {
        let r = &race.riders[f.rider];
        println!(
            "{:>20}  {:<10} {:.2} W/kg  {:<16} +{:.0}s",
            r.profile.name,
            r.profile.kind.label(),
            r.profile.wkg(),
            race.teams[r.team].spec.name,
            f.gap_s
        );
    }
}
