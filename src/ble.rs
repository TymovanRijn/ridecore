//! Bluetooth link to a single FTMS smart trainer (Wahoo KICKR CORE).
//!
//! Runs on its own tokio runtime thread. The UI sends `ControlRequest`s and
//! receives `TrainerEvent`s over channels. Recovery mirrors what Zwift logs
//! for FTMS trainers: re-request control when the trainer says control is not
//! permitted (e.g. after a power cycle), resend once on timeout, and reconnect
//! after a disconnect.
//!
//! A Zwift Click (or Play/Ride) controller is looked for alongside the
//! trainer, on its own connection; its shift buttons arrive as `Shift`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use crossbeam_channel::Sender;
use futures::StreamExt;
use tokio::sync::{Notify, mpsc};
use tokio::time::sleep;

use crate::click::{self, ClickEvent};
use crate::ftms::{self, ControlRequest, ResultCode};
use crate::zwift_hub;

#[derive(Debug, Clone)]
pub enum TrainerEvent {
    Status(String),
    Connected(String),
    Disconnected,
    BikeData(ftms::IndoorBikeData),
    Power(ftms::PowerMeasurement),
    CommandFailed(u8, ResultCode),
    /// A shift button: +1 harder, -1 easier.
    Shift(i8),
    ControllerConnected(String),
    ControllerDisconnected,
    ControllerBattery(u8),
    /// An unmapped controller button (for diagnostics).
    ControllerButton(u32),
    /// Whether the trainer shifts itself (Zwift protocol) this session.
    NativeShifting(bool),
}

/// A write the trainer hasn't acknowledged by now is given up on.
const WRITE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Spawns the Bluetooth thread and returns the command sender.
pub fn spawn(events: Sender<TrainerEvent>) -> mpsc::UnboundedSender<ControlRequest> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.spawn(run_controller(events.clone()));
        rt.block_on(run(events, rx));
    });
    tx
}

/// Only looks for a Zwift Click / Play / Ride, without touching the trainer
/// (for the `click_probe` diagnostics tool).
pub fn spawn_controller_only(events: Sender<TrainerEvent>) {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(run_controller(events));
    });
}

async fn run(events: Sender<TrainerEvent>, mut commands: mpsc::UnboundedReceiver<ControlRequest>) {
    loop {
        if let Err(e) = session(&events, &mut commands).await {
            let _ = events.send(TrainerEvent::Status(format!("{e:#}")));
        }
        let _ = events.send(TrainerEvent::Disconnected);
        sleep(Duration::from_secs(2)).await;
    }
}

async fn find_trainer() -> Result<Peripheral> {
    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("no Bluetooth adapter")?;
    adapter
        .start_scan(ScanFilter {
            services: vec![ftms::FTMS_SERVICE],
        })
        .await?;
    for _ in 0..30 {
        sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            let Some(props) = p.properties().await? else {
                continue;
            };
            let name = props.local_name.unwrap_or_default();
            if props.services.contains(&ftms::FTMS_SERVICE) || name.contains("KICKR") {
                adapter.stop_scan().await.ok();
                return Ok(p);
            }
        }
    }
    adapter.stop_scan().await.ok();
    Err(anyhow!(crate::i18n::t(
        "geen FTMS-trainer gevonden; is je trainer wakker en niet met een andere app verbonden?",
        "no FTMS trainer found; is your trainer awake and not connected to another app?"
    )))
}

async fn session(
    events: &Sender<TrainerEvent>,
    commands: &mut mpsc::UnboundedReceiver<ControlRequest>,
) -> Result<()> {
    let _ = events.send(TrainerEvent::Status(
        crate::i18n::t("Trainer zoeken…", "Looking for your trainer…").into(),
    ));
    let trainer = find_trainer().await?;
    trainer.connect().await.context("connect failed")?;
    trainer.discover_services().await?;
    let name = trainer
        .properties()
        .await?
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| "Trainer".into());

    let chars = trainer.characteristics();
    let find = |uuid| chars.iter().find(|c| c.uuid == uuid).cloned();
    let control = find(ftms::FTMS_CONTROL_POINT).context("trainer has no FTMS control point")?;
    for uuid in [
        ftms::INDOOR_BIKE_DATA,
        ftms::CYCLING_POWER_MEASUREMENT,
        ftms::FTMS_CONTROL_POINT,
    ] {
        if let Some(c) = find(uuid)
            && c.properties
                .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
        {
            trainer.subscribe(&c).await?;
        }
    }
    let mut notifications = trainer.notifications().await?;
    let _ = events.send(TrainerEvent::Connected(name));

    // Take control and start, as every FTMS session must.
    for req in [ControlRequest::RequestControl, ControlRequest::Start] {
        trainer
            .write(&control, &req.encode(), WriteType::WithResponse)
            .await?;
    }

    // Trainers with Zwift virtual shifting also speak Zwift's own protocol:
    // then grade, gear and ERG go through that, like they do in Zwift.
    let hub = match find(click::SYNC_RX) {
        Some(rx) if std::env::var_os("RIDECORE_NO_HUB").is_none() => {
            start_hub(&trainer, &rx, &find, &mut notifications, events)
                .await
                .then_some(rx)
        }
        _ => None,
    };
    let _ = events.send(TrainerEvent::NativeShifting(hub.is_some()));

    // Commands go out from their own task, so reading power never waits for
    // the trainer to acknowledge a write. Only the newest command of each
    // kind is kept: a grade that's already outdated is never sent.
    let outbox = Arc::new(Outbox::default());
    let mut writer = tokio::spawn({
        let (trainer, control, outbox, hub) =
            (trainer.clone(), control.clone(), outbox.clone(), hub.clone());
        async move {
            loop {
                let next = outbox.queue.lock().expect("outbox lock").pop_front();
                let Some(req) = next else {
                    outbox.ready.notified().await;
                    continue;
                };
                let write = async {
                    match &hub {
                        Some(rx) => {
                            let Some(bytes) = zwift_hub::encode(&req) else {
                                return Ok(());
                            };
                            trainer.write(rx, &bytes, WriteType::WithoutResponse).await?;
                            trainer
                                .write(rx, &zwift_hub::SETTINGS_REQUEST, WriteType::WithoutResponse)
                                .await
                        }
                        None if matches!(req, ControlRequest::SetGear { .. }) => Ok(()),
                        None => {
                            trainer
                                .write(&control, &req.encode(), WriteType::WithResponse)
                                .await
                        }
                    }
                };
                match tokio::time::timeout(WRITE_TIMEOUT, write).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Err::<(), anyhow::Error>(e.into()),
                    // A slow trainer: the next tick sends a fresh target anyway.
                    Err(_) => {}
                }
            }
        }
    });

    let mut last_target: Option<ControlRequest> = None;
    let result = loop {
        tokio::select! {
            n = notifications.next() => {
                let Some(n) = n else { break Err(anyhow!("trainer disconnected")) };
                if std::env::var_os("RIDECORE_DEBUG").is_some() {
                    let hex: Vec<String> = n.value.iter().map(|b| format!("{b:02X}")).collect();
                    eprintln!("[ble] {} {}", n.uuid, hex.join(" "));
                }
                if !forward_data(&n, events)
                    && n.uuid == ftms::FTMS_CONTROL_POINT
                    && let Some((op, code)) = ftms::parse_control_response(&n.value)
                {
                    match code {
                        ResultCode::Success => {}
                        // With the Zwift protocol in charge, FTMS control is
                        // not needed (and taking it back would fight it).
                        ResultCode::ControlNotPermitted if hub.is_some() => {}
                        ResultCode::ControlNotPermitted => {
                            // Trainer was probably power-cycled: regain
                            // control, then resend the current target.
                            outbox.push(ControlRequest::RequestControl);
                            if let Some(req) = last_target {
                                outbox.push(req);
                            }
                        }
                        other => {
                            let _ = events.send(TrainerEvent::CommandFailed(op, other));
                        }
                    }
                }
            }
            cmd = commands.recv() => {
                let Some(req) = cmd else { break Ok(()) };
                if matches!(req, ControlRequest::SetSimulation { .. } | ControlRequest::SetTargetPower { .. }) {
                    last_target = Some(req);
                }
                outbox.push(req);
            }
            done = &mut writer => {
                break match done {
                    Ok(Err(e)) => Err(e.context("writing to the trainer failed")),
                    _ => Err(anyhow!("trainer writer stopped")),
                };
            }
        }
    };
    writer.abort();
    result
}

/// Passes trainer data (FTMS, Cycling Power, Zwift riding data) on to the
/// app. Returns whether `n` was data.
fn forward_data(n: &btleplug::api::ValueNotification, events: &Sender<TrainerEvent>) -> bool {
    if n.uuid == ftms::INDOOR_BIKE_DATA {
        if let Some(d) = ftms::parse_indoor_bike_data(&n.value) {
            let _ = events.send(TrainerEvent::BikeData(d));
        }
    } else if n.uuid == ftms::CYCLING_POWER_MEASUREMENT {
        if let Some(m) = ftms::parse_cycling_power(&n.value) {
            let _ = events.send(TrainerEvent::Power(m));
        }
    } else if n.uuid == click::ASYNC {
        if let Some(d) = zwift_hub::parse_riding_data(&n.value) {
            let _ = events.send(TrainerEvent::BikeData(d));
        }
    } else {
        return false;
    }
    true
}

/// Says hello on the trainer's Zwift protocol and waits for its riding
/// data. Returns whether the trainer took it up.
async fn start_hub(
    trainer: &Peripheral,
    rx: &btleplug::api::Characteristic,
    find: &impl Fn(uuid::Uuid) -> Option<btleplug::api::Characteristic>,
    notifications: &mut (impl futures::Stream<Item = btleplug::api::ValueNotification> + Unpin),
    events: &Sender<TrainerEvent>,
) -> bool {
    for uuid in [click::ASYNC, click::SYNC_TX] {
        if let Some(c) = find(uuid)
            && c.properties
                .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
            && trainer.subscribe(&c).await.is_err()
        {
            return false;
        }
    }
    for _ in 0..2 {
        if trainer
            .write(rx, &zwift_hub::hello(), WriteType::WithoutResponse)
            .await
            .is_err()
        {
            return false;
        }
        sleep(Duration::from_millis(300)).await;
        let _ = trainer
            .write(rx, &zwift_hub::START, WriteType::WithoutResponse)
            .await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
        while let Ok(Some(n)) = tokio::time::timeout_at(deadline, notifications.next()).await {
            debug_log("hub", &n);
            if n.uuid == click::ASYNC && zwift_hub::parse_riding_data(&n.value).is_some() {
                forward_data(&n, events);
                return true;
            }
            forward_data(&n, events);
        }
    }
    false
}

/// Commands waiting to be written to the trainer.
#[derive(Default)]
struct Outbox {
    queue: Mutex<VecDeque<ControlRequest>>,
    ready: Notify,
}

impl Outbox {
    /// Queues `req`, replacing any queued command of the same kind.
    fn push(&self, req: ControlRequest) {
        let mut q = self.queue.lock().expect("outbox lock");
        let replaces = |r: &ControlRequest| {
            let target = |r: &ControlRequest| {
                matches!(
                    r,
                    ControlRequest::SetSimulation { .. } | ControlRequest::SetTargetPower { .. }
                )
            };
            r.opcode() == req.opcode() || (target(r) && target(&req))
        };
        q.retain(|r| !replaces(r));
        q.push_back(req);
        drop(q);
        self.ready.notify_one();
    }
}

fn debug_log(tag: &str, n: &btleplug::api::ValueNotification) {
    if std::env::var_os("RIDECORE_DEBUG").is_some() {
        let hex: Vec<String> = n.value.iter().map(|b| format!("{b:02X}")).collect();
        eprintln!("[{tag}] {} {}", n.uuid, hex.join(" "));
    }
}

/// Appends each battery report from the controller, raw bytes included, to
/// `~/Library/Logs/RideCore-click-battery.log`, so the level can be followed
/// across rides (the per-ride trainer log is replaced on every start).
fn log_battery(raw: &[u8], level: u8) {
    use std::io::Write;
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = std::path::Path::new(&home).join("Library/Logs/RideCore-click-battery.log");
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let hex: Vec<String> = raw.iter().map(|b| format!("{b:02X}")).collect();
    let _ = writeln!(f, "{unix},{level},{}", hex.join(" "));
}

async fn run_controller(events: Sender<TrainerEvent>) {
    let adapter = async {
        Manager::new()
            .await?
            .adapters()
            .await?
            .into_iter()
            .next()
            .context("no Bluetooth adapter")
    };
    let adapter = match adapter.await {
        Ok(a) => a,
        Err(e) => {
            let _ = events.send(TrainerEvent::Status(format!("{e:#}")));
            return;
        }
    };
    loop {
        match controller_session(&adapter, &events).await {
            Ok(true) => {
                let _ = events.send(TrainerEvent::ControllerDisconnected);
            }
            Ok(false) => {}
            Err(e) => {
                if std::env::var_os("RIDECORE_DEBUG").is_some() {
                    eprintln!("[click] {e:#}");
                }
                let _ = events.send(TrainerEvent::ControllerDisconnected);
            }
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn find_controller(adapter: &Adapter) -> Result<Option<Peripheral>> {
    adapter.start_scan(ScanFilter::default()).await?;
    for _ in 0..20 {
        sleep(Duration::from_millis(500)).await;
        for p in adapter.peripherals().await? {
            let Some(props) = p.properties().await? else {
                continue;
            };
            let name = props.local_name.clone().unwrap_or_default();
            let zwift_service = props.services.contains(&click::SERVICE)
                || props.services.contains(&click::SERVICE_SHORT);
            let known = click::is_controller(&props.manufacturer_data, &name);
            if known && (zwift_service || !name.is_empty() || !props.manufacturer_data.is_empty()) {
                if std::env::var_os("RIDECORE_DEBUG").is_some() {
                    eprintln!(
                        "[click] found {name:?} manufacturer {:02X?} services {:?}",
                        props.manufacturer_data, props.services
                    );
                }
                adapter.stop_scan().await.ok();
                return Ok(Some(p));
            }
        }
    }
    adapter.stop_scan().await.ok();
    Ok(None)
}

/// Connects to a controller and forwards its buttons until it goes away.
/// Returns whether a controller was connected at all.
async fn controller_session(adapter: &Adapter, events: &Sender<TrainerEvent>) -> Result<bool> {
    let Some(dev) = find_controller(adapter).await? else {
        return Ok(false);
    };
    dev.connect().await.context("controller connect failed")?;
    dev.discover_services().await?;
    let name = dev
        .properties()
        .await?
        .and_then(|p| p.local_name)
        .unwrap_or_else(|| "Zwift Click".into());
    let chars = dev.characteristics();
    let find = |uuid| chars.iter().find(|c| c.uuid == uuid).cloned();
    let debug = std::env::var_os("RIDECORE_DEBUG").is_some();
    if debug {
        for c in &chars {
            eprintln!("[click] char {} {:?}", c.uuid, c.properties);
        }
    }
    let rx = find(click::SYNC_RX).context("controller has no Zwift sync characteristic")?;
    let mut notifications = dev.notifications().await?;
    for uuid in [click::ASYNC, click::SYNC_TX] {
        match find(uuid) {
            Some(c) => {
                let r = dev.subscribe(&c).await;
                if debug {
                    eprintln!("[click] subscribe {uuid}: {r:?}");
                }
                r?;
            }
            None if debug => eprintln!("[click] missing {uuid}"),
            None => {}
        }
    }
    // Say "RideOn" until the controller answers; some firmware misses the
    // first write right after subscribing.
    let mut first = None;
    for attempt in 0..6 {
        let write_type = if attempt % 2 == 0
            && rx
                .properties
                .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        {
            WriteType::WithoutResponse
        } else {
            WriteType::WithResponse
        };
        let r = dev.write(&rx, click::RIDE_ON, write_type).await;
        if debug {
            eprintln!("[click] RideOn ({write_type:?}): {r:?}");
        }
        if let Ok(Some(n)) =
            tokio::time::timeout(Duration::from_secs(2), notifications.next()).await
        {
            first = Some(n);
            break;
        }
    }
    if debug {
        for c in &chars {
            if c.properties.contains(CharPropFlags::READ) {
                let v = dev.read(c).await;
                eprintln!(
                    "[click] read {}: {:?}",
                    c.uuid,
                    v.map(|b| (String::from_utf8_lossy(&b).into_owned(), b))
                );
            }
        }
    }
    let Some(first) = first else {
        let _ = dev.disconnect().await;
        bail!("controller connected but does not answer the RideOn handshake");
    };
    let _ = events.send(TrainerEvent::ControllerConnected(name));

    let mut decoder = click::Decoder::default();
    let mut pending = Some(first);
    while let Some(n) = match pending.take() {
        Some(n) => Some(n),
        None => notifications.next().await,
    } {
        debug_log("click", &n);
        for ev in decoder.feed(&n.value) {
            let _ = events.send(match ev {
                ClickEvent::ShiftUp => TrainerEvent::Shift(1),
                ClickEvent::ShiftDown => TrainerEvent::Shift(-1),
                ClickEvent::Battery(b) => {
                    log_battery(&n.value, b);
                    TrainerEvent::ControllerBattery(b)
                }
                ClickEvent::Other(id) => TrainerEvent::ControllerButton(id),
            });
        }
    }
    Ok(true)
}
