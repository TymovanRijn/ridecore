use super::*;
use crate::race::field::rival_teams;

fn config(seed: u64, route: usize, laps: u32, teams: usize) -> RaceConfig {
    RaceConfig {
        seed,
        route: Route::builtin()[route].clone(),
        laps,
        teams: rival_teams(teams, 3.6, 1.0),
        riders_per_team: 6,
        player_team: teams - 1,
        player: PlayerSetup {
            name: "Speler".into(),
            kg: 75.0,
            height_cm: 180.0,
            ftp: 270.0,
            kind: RiderKind::Rouleur,
        },
        countdown_s: 3.0,
    }
}

/// Runs a race with the player holding a fixed fraction of FTP.
fn run(cfg: &RaceConfig, player_frac: f32, max_s: f32) -> Race {
    let mut race = Race::new(cfg);
    let ftp = cfg.player.ftp;
    while !race.riders.iter().all(Rider::finished) && race.time < max_s {
        race.step(0.25, ftp * player_frac, 88.0);
    }
    race
}

#[test]
fn countdown_holds_everyone_on_the_line() {
    let mut race = Race::new(&config(1, 0, 1, 3));
    race.step(1.0, 300.0, 90.0);
    assert!(!race.started());
    assert!(race.riders.iter().all(|r| r.speed == 0.0));
}

#[test]
fn a_flat_race_finishes_with_a_sane_result() {
    let race = run(&config(4, 0, 1, 4), 0.85, 3600.0);
    assert!(
        race.riders.iter().all(Rider::finished),
        "not everyone finished"
    );
    assert_eq!(race.finish.len(), race.riders.len());
    let winner = &race.finish[0];
    let kmh = race.course.total_m as f32 / winner.time * 3.6;
    assert!((36.0..52.0).contains(&kmh), "winning speed {kmh} km/h");
    assert!(race.finish.windows(2).all(|w| w[0].time <= w[1].time));
    assert!(race.finish.windows(2).all(|w| w[0].gap_s <= w[1].gap_s));
}

#[test]
fn same_seed_replays_identically() {
    let a = run(&config(11, 1, 1, 3), 0.8, 2400.0);
    let b = run(&config(11, 1, 1, 3), 0.8, 2400.0);
    let order = |r: &Race| r.finish.iter().map(|f| f.rider).collect::<Vec<_>>();
    assert_eq!(order(&a), order(&b));
    assert_eq!(a.events.len(), b.events.len());
}

#[test]
fn drafting_in_the_bunch_saves_energy() {
    // Same rider profile, once alone and once in a full race.
    let cfg = config(5, 0, 1, 4);
    let mut race = Race::new(&cfg);
    let mut sheltered = 0.0;
    let mut samples = 0.0;
    while race.time < 600.0 {
        race.step(0.25, 250.0, 88.0);
        if race.time > 120.0 {
            sheltered += race
                .riders
                .iter()
                .filter(|r| !r.human)
                .map(|r| r.draft)
                .sum::<f32>()
                / (race.riders.len() - 1) as f32;
            samples += 1.0;
        }
    }
    let mean_draft = sheltered / samples;
    assert!(
        mean_draft > 0.2,
        "average drag reduction in the bunch was only {mean_draft}"
    );
}

#[test]
fn stronger_riders_finish_ahead_of_weaker_ones() {
    // Across several races the top five finishers should be clearly
    // stronger (W/kg) than the last five.
    let (mut front, mut back) = (0.0, 0.0);
    for seed in 0..4 {
        let race = run(&config(100 + seed, 1, 1, 5), 0.8, 3600.0);
        let bots: Vec<f32> = race
            .finish
            .iter()
            .filter(|f| !race.riders[f.rider].human)
            .map(|f| race.riders[f.rider].profile.wkg())
            .collect();
        front += bots[..5].iter().sum::<f32>() / 5.0;
        back += bots[bots.len() - 5..].iter().sum::<f32>() / 5.0;
    }
    assert!(
        front - back > 4.0 * 0.2,
        "top five {:.2} vs last five {:.2} W/kg",
        front / 4.0,
        back / 4.0
    );
}

#[test]
fn attacks_happen_and_are_announced() {
    let race = run(&config(21, 1, 2, 5), 0.8, 5400.0);
    let attacks = race
        .events
        .iter()
        .filter(|e| e.text.contains("Aanval") || e.text.contains("demarreert"))
        .count();
    assert!(attacks > 0, "no attacks in a 28 km race");
    assert!(
        race.events
            .iter()
            .any(|e| e.text.contains("wint de wedstrijd"))
    );
}

#[test]
fn intermediate_sprints_award_points() {
    let race = run(&config(8, 0, 2, 4), 0.85, 3600.0);
    let pts = race.points(course::SegmentKind::Sprint);
    assert!(!pts.is_empty());
    assert_eq!(pts[0].1, 10);
}

#[test]
fn protect_order_keeps_teammates_with_a_struggling_player() {
    let cfg = config(31, 0, 1, 4);
    let mut race = Race::new(&cfg);
    race.set_order(TeamOrder::Protect);
    // The player rides easy and gets dropped; teammates should come back.
    while race.time < 900.0 {
        race.step(0.25, 150.0, 85.0);
    }
    let me = race.human;
    let my_group = race.group_of(me);
    let mates_with_me = race
        .riders
        .iter()
        .filter(|r| r.team == race.riders[me].team && !r.human && race.group_of(r.id) == my_group)
        .count();
    for r in race
        .riders
        .iter()
        .filter(|r| r.team == race.riders[me].team && !r.human)
    {
        eprintln!(
            "mate {} {:?} group {} (me {}) bal {:.2} dist {:.0} (me {:.0})",
            r.id,
            r.intent,
            race.group_of(r.id),
            my_group,
            r.energy.fraction(),
            r.dist,
            race.riders[me].dist
        );
    }
    assert!(
        mates_with_me >= 3,
        "only {mates_with_me} teammates stayed with the player"
    );
}

#[test]
fn chases_must_end_well_before_the_line() {
    // Ten minutes from the line a bunch tolerates about 40 s; at the end, almost nothing.
    assert!((ai::allowed_gap_s(600.0) - 37.5).abs() < 0.1);
    assert_eq!(ai::allowed_gap_s(200.0), 5.0);
    assert!(ai::allowed_gap_s(3600.0) > ai::allowed_gap_s(1800.0));
}

#[test]
fn sustainable_power_spends_w_prime_over_the_climb() {
    // 300 W CP with 18 kJ left: 10 minutes to go allows 330 W, 1 minute much more.
    assert_eq!(ai::sustainable_power(300.0, 18_000.0, 600.0), 330.0);
    assert!(ai::sustainable_power(300.0, 18_000.0, 60.0) > 500.0);
}

#[test]
fn summit_finish_on_the_real_alpe_dhuez_is_decided_by_climbing_strength() {
    let route = crate::route::real_routes()
        .iter()
        .find(|r| r.id == "alpe-dhuez")
        .expect("Alpe d'Huez is embedded")
        .clone();
    let mut cfg = config(41, 0, 1, 5);
    cfg.route = route;
    let race = run(&cfg, 0.85, 7200.0);
    assert!(
        race.riders.iter().all(Rider::finished),
        "everyone reaches the top"
    );
    // A 3.6 W/kg field climbs 14 km and 1100 m in roughly 45–75 minutes.
    let minutes = race.finish[0].time / 60.0;
    assert!(
        (40.0..80.0).contains(&minutes),
        "winner took {minutes:.0} min"
    );
    let bots: Vec<f32> = race
        .finish
        .iter()
        .filter(|f| !race.riders[f.rider].human)
        .map(|f| race.riders[f.rider].profile.wkg())
        .collect();
    let top = bots[..5].iter().sum::<f32>() / 5.0;
    let bottom = bots[bots.len() - 5..].iter().sum::<f32>() / 5.0;
    assert!(
        top - bottom > 0.5,
        "climbers {top:.2} vs last five {bottom:.2} W/kg"
    );
    // Finishers are listed in order with growing gaps.
    let gaps: Vec<f32> = race.finish.iter().map(|f| f.gap_s).collect();
    assert!(gaps.windows(2).all(|w| w[0] <= w[1]));
}

/// Rides the player to the front of the bunch, then hands back the race.
fn player_on_the_front(seed: u64) -> (RaceConfig, Race) {
    let cfg = config(seed, 0, 1, 4);
    let mut race = Race::new(&cfg);
    while race.time < 120.0 {
        race.step(0.25, 0.85 * cfg.player.ftp, 88.0);
    }
    // Ride up until nobody in the player's own bunch is ahead.
    let ahead_in_bunch = |race: &Race| {
        let me = race.player().dist;
        race.riders
            .iter()
            .filter(|r| !r.human && r.dist > me && r.dist < me + 60.0)
            .count()
    };
    while ahead_in_bunch(&race) > 0 && race.time < 400.0 {
        race.step(0.25, 1.3 * cfg.player.ftp, 88.0);
    }
    assert_eq!(ahead_in_bunch(&race), 0, "never reached the front");
    (cfg, race)
}

/// Leading the bunch at an easy pace must not slow the race down: riders
/// come past and carry on at their own tempo.
#[test]
fn an_easy_player_does_not_hold_up_the_bunch() {
    let (cfg, mut race) = player_on_the_front(21);
    let start = race.time;
    while race.time < start + 120.0 {
        race.step(0.25, 0.6 * cfg.player.ftp, 88.0);
    }
    let me = race.player().dist;
    let ahead = race.riders.iter().filter(|r| r.dist > me + 20.0).count();
    assert!(
        ahead > race.riders.len() / 2,
        "only {ahead} riders went past an easy-riding player"
    );
}

/// A hard attack from the front opens a gap instead of towing the whole
/// bunch along.
#[test]
fn a_player_attack_is_not_glued_to_by_everyone() {
    let (cfg, mut race) = player_on_the_front(22);
    let start = race.time;
    while race.time < start + 90.0 {
        race.step(0.25, 1.5 * cfg.player.ftp, 88.0);
    }
    let me = race.player().dist;
    let with_me = race
        .riders
        .iter()
        // Teammates may come along; rivals mostly shouldn't.
        .filter(|r| r.team != race.player().team && (me - r.dist) < 15.0)
        .count();
    assert!(
        with_me < race.riders.len() / 3,
        "{with_me} riders stayed glued to the attack"
    );
}

/// The Liège finale raced by the bots, with the player sitting in: the
/// race should be decided on the climbs, with favourites marking each
/// other and a small group (not a bunch sprint) at the finish.
#[test]
fn the_liege_finale_is_raced_like_the_real_thing() {
    let route = crate::route::real_routes()
        .iter()
        .find(|r| r.id == "lbl-finale")
        .unwrap()
        .clone();
    let mut groups_at_line = Vec::new();
    let mut marks = 0;
    let mut climb_attacks = 0;
    for seed in 1..=4 {
        let cfg = RaceConfig {
            seed,
            route: route.clone(),
            laps: 1,
            teams: rival_teams(8, 3.6, 1.0),
            riders_per_team: 6,
            player_team: 7,
            player: PlayerSetup {
                name: "Speler".into(),
                kg: 75.0,
                height_cm: 180.0,
                ftp: 270.0,
                kind: RiderKind::Rouleur,
            },
            countdown_s: 3.0,
        };
        let race = run(&cfg, 0.8, 4.0 * 3600.0);
        if std::env::var_os("RIDECORE_PRINT").is_some() {
            println!("--- seed {seed}");
            for e in &race.events {
                println!("{:>6.0}s {}", e.time, e.text);
            }
            for f in race.finish.iter().take(8) {
                println!("  {} +{:.0}s", race.riders[f.rider].profile.name, f.gap_s);
            }
        }
        marks += race
            .events
            .iter()
            .filter(|e| e.text.contains(" mee!"))
            .count();
        climb_attacks += race
            .events
            .iter()
            .filter(|e| e.text.contains("klim"))
            .count();
        // Riders within 2 s of the winner.
        groups_at_line.push(race.finish.iter().filter(|f| f.gap_s < 2.0).count());
    }
    assert!(marks > 0, "nobody ever marked an attack");
    assert!(climb_attacks > 0, "nobody attacked on a climb");
    let mean = groups_at_line.iter().sum::<usize>() as f32 / groups_at_line.len() as f32;
    assert!(
        mean < 20.0,
        "finish groups {groups_at_line:?}: a bunch sprint"
    );
}

#[test]
fn the_player_gets_into_the_wheel_without_steering() {
    // A rider who just pedals to stay a couple of meters behind whoever is
    // ahead should be sheltered most of the time.
    let cfg = config(5, 0, 1, 4);
    let mut race = Race::new(&cfg);
    let (mut sheltered, mut samples) = (0, 0);
    let mut power = 250.0;
    while race.time < 600.0 {
        race.step(0.25, power, 88.0);
        let me = race.player();
        let gap = race
            .riders
            .iter()
            .filter(|r| !r.human && r.dist > me.dist)
            .map(|r| (r.dist - me.dist) as f32)
            .fold(f32::MAX, f32::min);
        // Nobody ahead: ease up and let the bunch come past.
        power = if gap == f32::MAX {
            150.0
        } else {
            (250.0 + 40.0 * (gap - 2.5)).clamp(120.0, 500.0)
        };
        if std::env::var("RIDECORE_PRINT").is_ok() && ((race.time * 4.0) as u32).is_multiple_of(20)
        {
            let w = race
                .riders
                .iter()
                .filter(|r| !r.human && r.dist > me.dist)
                .min_by(|a, b| a.dist.total_cmp(&b.dist));
            println!(
                "{:5.0}s gap {gap:5.1} dy {:5.2} draft {:.2} v {:.1}/{:.1} P {power:.0}",
                race.time,
                w.map_or(0.0, |w| w.lateral - me.lateral),
                me.draft,
                me.speed,
                w.map_or(0.0, |w| w.speed)
            );
        }
        if race.time > 120.0 {
            samples += 1;
            if me.draft > 0.25 {
                sheltered += 1;
            }
        }
    }
    let share = sheltered as f32 / samples as f32;
    assert!(
        share > 0.7,
        "sheltered only {:.0}% of the time",
        share * 100.0
    );
}
