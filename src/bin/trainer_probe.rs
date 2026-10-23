//! Lists what the trainer offers over Bluetooth and, if it speaks Zwift's
//! own trainer protocol, says hello on it and prints its riding data for
//! 20 seconds. Sends no resistance, unless `--commands`: then it also sends
//! a gear and a flat road, each followed by the settings request, to show
//! how the trainer acknowledges them.
//!
//!     cargo run --release --bin trainer_probe [-- --commands]

use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::Manager;
use futures::StreamExt;
use ridecore::{click, ftms};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let adapter = Manager::new().await?.adapters().await?.remove(0);
    adapter
        .start_scan(ScanFilter {
            services: vec![ftms::FTMS_SERVICE],
        })
        .await?;
    println!("Zoeken naar de trainer (trap even om hem wakker te maken)…");
    let mut trainer = None;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            if let Some(props) = p.properties().await?
                && (props.services.contains(&ftms::FTMS_SERVICE)
                    || props.local_name.as_deref().unwrap_or("").contains("KICKR"))
            {
                println!(
                    "gevonden: {:?} services {:?} manufacturer {:02X?}",
                    props.local_name, props.services, props.manufacturer_data
                );
                trainer = Some(p);
            }
        }
        if trainer.is_some() {
            break;
        }
    }
    adapter.stop_scan().await.ok();
    let trainer = trainer.ok_or_else(|| anyhow::anyhow!("geen trainer gevonden"))?;
    trainer.connect().await?;
    trainer.discover_services().await?;
    for s in trainer.services() {
        println!("service {}", s.uuid);
        for c in &s.characteristics {
            println!("    char {} {:?}", c.uuid, c.properties);
        }
    }
    let chars = trainer.characteristics();
    let find = |uuid| chars.iter().find(|c| c.uuid == uuid).cloned();
    let Some(rx) = find(click::SYNC_RX) else {
        println!("Geen Zwift-protocol op deze trainer.");
        return Ok(());
    };
    for uuid in [click::ASYNC, click::SYNC_TX] {
        if let Some(c) = find(uuid)
            && c.properties
                .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
        {
            trainer.subscribe(&c).await?;
        }
    }
    let mut notes = trainer.notifications().await?;
    let mut hello = b"RideOn".to_vec();
    hello.extend([0x02, 0x01]);
    trainer
        .write(&rx, &hello, WriteType::WithoutResponse)
        .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    trainer
        .write(&rx, &[0x41, 0x08, 0x05], WriteType::WithoutResponse)
        .await?;
    let commands = std::env::args().any(|a| a == "--commands");
    let start = tokio::time::Instant::now();
    let end = start + Duration::from_secs(20);
    let mut sent = 0;
    loop {
        if commands && sent < 2 && start.elapsed() > Duration::from_secs(4 + 5 * sent) {
            let req = if sent == 0 {
                ftms::ControlRequest::SetGear {
                    ratio: 2.4,
                    rider_kg: 75.0,
                    bike_kg: 8.0,
                }
            } else {
                ftms::ControlRequest::SetSimulation {
                    wind_mps: 0.0,
                    grade_pct: 0.0,
                    crr: ridecore::trainer::TRAINER_CRR,
                    cw: ridecore::trainer::TRAINER_CW,
                }
            };
            let bytes = ridecore::zwift_hub::encode(&req).expect("hub command");
            println!(">> {req:?}");
            trainer
                .write(&rx, &bytes, WriteType::WithoutResponse)
                .await?;
            trainer
                .write(
                    &rx,
                    &ridecore::zwift_hub::SETTINGS_REQUEST,
                    WriteType::WithoutResponse,
                )
                .await?;
            sent += 1;
        }
        let Ok(Some(n)) = tokio::time::timeout_at(
            end.min(tokio::time::Instant::now() + Duration::from_millis(500)),
            notes.next(),
        )
        .await
        else {
            if tokio::time::Instant::now() >= end {
                break;
            }
            continue;
        };
        let hex: Vec<String> = n.value.iter().map(|b| format!("{b:02X}")).collect();
        println!(
            "{} {} {:?}",
            &n.uuid.to_string()[..8],
            hex.join(" "),
            String::from_utf8_lossy(&n.value)
        );
    }
    trainer.disconnect().await.ok();
    Ok(())
}
