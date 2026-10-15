//! Checks a Zwift Click: connects (without touching the trainer) and prints
//! every button press for 90 seconds. Raw packets are shown too.
//!
//!     cargo run --release --bin click_probe

use std::time::{Duration, Instant};

use ridecore::ble::{self, TrainerEvent};

fn main() {
    // SAFETY: set before any other thread starts.
    unsafe { std::env::set_var("RIDECORE_DEBUG", "1") };
    let (tx, rx) = crossbeam_channel::unbounded();
    ble::spawn_controller_only(tx);
    println!("Druk op een knop van je Zwift Click om hem wakker te maken…");
    let end = Instant::now() + Duration::from_secs(90);
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(TrainerEvent::ControllerConnected(n)) => {
                println!(">> verbonden met {n}, druk nu op + en -")
            }
            Ok(TrainerEvent::Shift(1)) => println!(">> + (zwaarder)"),
            Ok(TrainerEvent::Shift(_)) => println!(">> - (lichter)"),
            Ok(TrainerEvent::ControllerBattery(b)) => println!(">> batterij {b}%"),
            Ok(TrainerEvent::ControllerButton(id)) => println!(">> andere knop 0x{id:X}"),
            Ok(TrainerEvent::ControllerDisconnected) => {
                println!(">> (nog) geen controller verbonden")
            }
            Ok(other) => println!(">> {other:?}"),
            Err(_) => break,
        }
    }
    println!("Klaar.");
}
