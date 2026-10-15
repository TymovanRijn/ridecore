//! The ride screen: connects the trainer link, physics, controller, workout
//! engine and recorder, and draws the dashboard.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Key, RichText};
use egui_plot::{Line, Plot, PlotPoints, VLine};
use tokio::sync::mpsc::UnboundedSender;

use ridecore::ble::TrainerEvent;
use ridecore::fit::{self, Sample};
use ridecore::ftms::ControlRequest;
use ridecore::physics::{BikeParams, RiderState};
use ridecore::route::Route;
use ridecore::sensors::LiveData;
use ridecore::trainer::{ControlTuning, Mode, TrainerController, VirtualGears};
use ridecore::workout::Workout;

/// Zwift sends simulation parameters at most every 0.75 s.
const CONTROL_INTERVAL: Duration = Duration::from_millis(750);
struct Ride {
    started: Instant,
    paused_for: Duration,
    paused_at: Option<Instant>,
    rider: RiderState,
    samples: Vec<Sample>,
    last_sample_s: u64,
    workout: Option<Workout>,
}

impl Ride {
    fn elapsed(&self) -> Duration {
        let paused = self.paused_for + self.paused_at.map(|p| p.elapsed()).unwrap_or_default();
        self.started.elapsed().saturating_sub(paused)
    }
}

pub struct RideApp {
    events: Receiver<TrainerEvent>,
    commands: UnboundedSender<ControlRequest>,
    status: String,
    trainer_name: Option<String>,
    live: LiveData,

    ftp: u32,
    bike: BikeParams,
    difficulty: f32,
    virtual_shifting: bool,
    routes: Vec<Route>,
    route_idx: usize,
    workouts: Vec<PathBuf>,
    workout_idx: Option<usize>,

    controller: TrainerController,
    ride: Option<Ride>,
    last_frame: Instant,
    last_control: Instant,
    message: Option<String>,
}

impl RideApp {
    pub fn new(events: Receiver<TrainerEvent>, commands: UnboundedSender<ControlRequest>) -> Self {
        Self {
            events,
            commands,
            status: "Starting Bluetooth…".into(),
            trainer_name: None,
            live: LiveData::default(),
            ftp: 250,
            bike: BikeParams::default(),
            difficulty: 0.5,
            virtual_shifting: true,
            routes: Route::catalog(),
            route_idx: 0,
            workouts: find_workouts(),
            workout_idx: None,
            controller: TrainerController::new(ControlTuning::default()),
            ride: None,
            last_frame: Instant::now(),
            last_control: Instant::now(),
            message: None,
        }
    }

    fn route(&self) -> &Route {
        &self.routes[self.route_idx]
    }

    fn start_ride(&mut self) {
        let workout = match self
            .workout_idx
            .map(|i| std::fs::read_to_string(&self.workouts[i]))
        {
            Some(Ok(xml)) => match Workout::parse(&xml) {
                Ok(w) => Some(w),
                Err(e) => {
                    self.message = Some(format!("Could not load workout: {e:#}"));
                    return;
                }
            },
            Some(Err(e)) => {
                self.message = Some(format!("Could not read workout: {e}"));
                return;
            }
            None => None,
        };
        self.controller = TrainerController::new(ControlTuning::default());
        self.controller.gears = self.virtual_shifting.then(VirtualGears::default);
        self.ride = Some(Ride {
            started: Instant::now(),
            paused_for: Duration::ZERO,
            paused_at: None,
            rider: RiderState::default(),
            samples: Vec::new(),
            last_sample_s: 0,
            workout,
        });
        self.message = None;
    }

    fn finish_ride(&mut self) {
        let Some(ride) = self.ride.take() else { return };
        if ride.samples.len() < 10 {
            self.message = Some("Ride too short to save.".into());
            return;
        }
        let dir = dirs_documents().join("RideCore");
        let path = dir.join(format!("ride-{}.fit", ride.samples[0].unix_time));
        let result = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&path, fit::encode_activity(&ride.samples)));
        self.message = Some(match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Could not save ride: {e}"),
        });
    }

    fn mode(&self, ride: &Ride) -> Mode {
        let elapsed = ride.elapsed().as_secs_f32();
        match ride
            .workout
            .as_ref()
            .and_then(|w| w.target_at(elapsed, self.ftp))
            .and_then(|t| t.watts)
        {
            Some(target_w) => Mode::Erg { target_w },
            None => Mode::Sim {
                difficulty: self.difficulty,
            },
        }
    }

    /// Advances physics, recording and trainer control by one frame.
    fn simulate(&mut self, dt: f32) {
        let Some(ride) = self.ride.as_ref() else {
            return;
        };
        if ride.paused_at.is_some() {
            return;
        }
        let mode = self.mode(ride);
        let power = self.live.power_w;
        let route = &self.routes[self.route_idx];
        let ride = self.ride.as_mut().expect("checked above");

        let grade = route.grade_pct_at(ride.rider.distance) / 100.0;
        ride.rider.step(&self.bike, power, grade, 0.0, dt);

        let secs = ride.elapsed().as_secs();
        if secs > ride.last_sample_s || ride.samples.is_empty() {
            ride.last_sample_s = secs;
            ride.samples.push(Sample {
                unix_time: fit::now_unix(),
                distance_m: ride.rider.distance,
                speed_mps: ride.rider.speed,
                power_w: power as u16,
                cadence_rpm: self.live.cadence_rpm.round().min(254.0) as u8,
                heart_rate: self.live.heart_rate,
                altitude_m: route.elevation_at(ride.rider.distance),
                position: route.latlon_at(ride.rider.distance),
            });
        }

        if self.last_control.elapsed() >= CONTROL_INTERVAL {
            let tick_dt = self.last_control.elapsed().as_secs_f32();
            self.last_control = Instant::now();
            let ahead = ride.rider.distance
                + (ride.rider.speed * self.controller.tuning.grade_lookahead_s) as f64;
            let grade_ahead = route.grade_pct_at(ahead);
            if let Some(req) = self.controller.tick(
                mode,
                grade_ahead,
                power,
                self.live.trainer_speed_mps,
                &self.bike,
                tick_dt,
            ) {
                let _ = self.commands.send(req);
            }
        }

        if route.finished(ride.rider.distance) {
            self.finish_ride();
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if let Some(g) = self.controller.gears.as_mut() {
                if i.key_pressed(Key::ArrowUp) || i.key_pressed(Key::Equals) {
                    g.shift_up();
                }
                if i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::Minus) {
                    g.shift_down();
                }
            }
        });
    }

    fn metrics(&self, ui: &mut egui::Ui) {
        let ride = self.ride.as_ref();
        let speed = ride.map(|r| r.rider.speed * 3.6).unwrap_or(0.0);
        let dist = ride.map(|r| r.rider.distance / 1000.0).unwrap_or(0.0);
        let elapsed = ride.map(|r| r.elapsed().as_secs()).unwrap_or(0);
        let grade = ride
            .map(|r| self.route().grade_pct_at(r.rider.distance))
            .unwrap_or(0.0);
        let wkg = self.live.power_3s() / self.bike.rider_kg;
        ui.horizontal_wrapped(|ui| {
            metric(
                ui,
                "POWER",
                format!("{:.0} W", self.live.power_3s()),
                Color32::from_rgb(255, 140, 0),
            );
            metric(
                ui,
                "W/KG",
                format!("{wkg:.1}"),
                Color32::from_rgb(255, 140, 0),
            );
            metric(
                ui,
                "CADENCE",
                format!("{:.0} rpm", self.live.cadence_rpm),
                Color32::LIGHT_BLUE,
            );
            metric(ui, "SPEED", format!("{speed:.1} km/h"), Color32::WHITE);
            metric(ui, "DISTANCE", format!("{dist:.2} km"), Color32::WHITE);
            metric(
                ui,
                "TIME",
                format!(
                    "{}:{:02}:{:02}",
                    elapsed / 3600,
                    elapsed / 60 % 60,
                    elapsed % 60
                ),
                Color32::WHITE,
            );
            metric(ui, "GRADE", format!("{grade:.1} %"), Color32::LIGHT_GREEN);
            if let Some(g) = &self.controller.gears {
                metric(
                    ui,
                    "GEAR",
                    format!("{} / {}", g.index + 1, g.ratios.len()),
                    Color32::YELLOW,
                );
            }
            if let Some(hr) = self.live.heart_rate {
                metric(
                    ui,
                    "HEART",
                    format!("{hr} bpm"),
                    Color32::from_rgb(255, 80, 80),
                );
            }
        });
    }

    fn workout_panel(&self, ui: &mut egui::Ui, ride: &Ride) {
        let Some(w) = &ride.workout else { return };
        let elapsed = ride.elapsed().as_secs_f32();
        match w.target_at(elapsed, self.ftp) {
            Some(t) => {
                let target = t
                    .watts
                    .map(|w| format!("{w} W"))
                    .unwrap_or_else(|| "Free ride".into());
                let erg = if self.controller.erg_paused {
                    "  (ERG paused: pedal to resume)"
                } else {
                    ""
                };
                ui.label(
                    RichText::new(format!(
                        "{} · interval {}/{} · target {target} · {:.0}s left{erg}",
                        w.name,
                        t.segment + 1,
                        w.segments.len(),
                        t.segment_remaining_s
                    ))
                    .size(18.0),
                );
            }
            None => {
                ui.label(RichText::new(format!("{} complete!", w.name)).size(18.0));
            }
        }
        let mut t = 0.0;
        let mut pts = Vec::new();
        for s in &w.segments {
            let secs = s.secs() as f64;
            let (a, b) = match w.target_at(t as f32 + 0.01, self.ftp).and_then(|x| x.watts) {
                Some(_) => (
                    w.target_at(t as f32, self.ftp)
                        .and_then(|x| x.watts)
                        .unwrap_or(0) as f64,
                    w.target_at((t + secs) as f32 - 0.01, self.ftp)
                        .and_then(|x| x.watts)
                        .unwrap_or(0) as f64,
                ),
                None => (0.0, 0.0),
            };
            pts.push([t, a]);
            pts.push([t + secs, b]);
            t += secs;
        }
        Plot::new("workout")
            .height(120.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |p| {
                p.line(
                    Line::new("target", PlotPoints::from(pts))
                        .fill(0.0)
                        .color(Color32::from_rgb(80, 160, 255)),
                );
                p.vline(VLine::new("now", elapsed as f64).color(Color32::WHITE));
            });
    }

    fn elevation_plot(&self, ui: &mut egui::Ui) {
        let route = self.route();
        let pts: Vec<[f64; 2]> = route
            .points
            .iter()
            .map(|&(d, e)| [d / 1000.0, e as f64])
            .collect();
        let pos = self.ride.as_ref().map(|r| {
            let len = route.length_m();
            if route.looped {
                r.rider.distance % len
            } else {
                r.rider.distance.min(len)
            }
        });
        Plot::new("elevation")
            .height(160.0)
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |p| {
                p.line(
                    Line::new(route.name.clone(), PlotPoints::from(pts))
                        .fill(0.0)
                        .color(Color32::from_rgb(120, 200, 120)),
                );
                if let Some(d) = pos {
                    p.vline(
                        VLine::new("you", d / 1000.0)
                            .color(Color32::from_rgb(255, 140, 0))
                            .width(2.0),
                    );
                }
            });
    }

    fn setup_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Ride setup");
        egui::Grid::new("setup").num_columns(2).show(ui, |ui| {
            ui.label("FTP");
            ui.add(
                egui::DragValue::new(&mut self.ftp)
                    .range(80..=600)
                    .suffix(" W"),
            );
            ui.end_row();
            ui.label("Rider weight");
            ui.add(
                egui::DragValue::new(&mut self.bike.rider_kg)
                    .range(35.0..=160.0)
                    .speed(0.1)
                    .suffix(" kg"),
            );
            ui.end_row();
            ui.label("Trainer difficulty");
            ui.add(
                egui::Slider::new(&mut self.difficulty, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
            ui.end_row();
            ui.label("Virtual shifting");
            ui.checkbox(&mut self.virtual_shifting, "↑/↓ keys shift gears");
            ui.end_row();
            ui.label("Route");
            egui::ComboBox::from_id_salt("route")
                .selected_text(self.route().name.clone())
                .show_ui(ui, |ui| {
                    for (i, r) in self.routes.iter().enumerate() {
                        ui.selectable_value(&mut self.route_idx, i, r.name.clone());
                    }
                });
            ui.end_row();
            ui.label("Workout");
            let label = self
                .workout_idx
                .and_then(|i| self.workouts[i].file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Free ride".into());
            egui::ComboBox::from_id_salt("workout")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.workout_idx, None, "Free ride");
                    for (i, p) in self.workouts.iter().enumerate() {
                        let name = p
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.selectable_value(&mut self.workout_idx, Some(i), name);
                    }
                });
            ui.end_row();
        });
        ui.add_space(8.0);
        let ready = self.trainer_name.is_some();
        if ui
            .add_enabled(
                ready,
                egui::Button::new(RichText::new("Start ride").size(20.0)),
            )
            .clicked()
        {
            self.start_ride();
        }
        if !ready {
            ui.label("Waiting for the KICKR CORE… start pedaling to wake it.");
        }
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: String, color: Color32) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(110.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).small().weak());
            ui.label(RichText::new(value).size(26.0).strong().color(color));
        });
    });
}

fn dirs_documents() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Documents")
}

/// `.zwo` files from the Zwift workouts folder and RideCore's own folder.
fn find_workouts() -> Vec<PathBuf> {
    let docs = dirs_documents();
    let mut roots = vec![docs.join("RideCore/Workouts")];
    if let Ok(entries) = std::fs::read_dir(docs.join("Zwift/Workouts")) {
        roots.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    let mut found: Vec<PathBuf> = roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zwo")))
        .collect();
    found.sort();
    found
}

impl eframe::App for RideApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match &ev {
                TrainerEvent::Status(s) => self.status = s.clone(),
                TrainerEvent::Connected(name) => {
                    self.status = format!("Connected to {name}");
                    self.trainer_name = Some(name.clone());
                }
                TrainerEvent::Disconnected => {
                    self.trainer_name = None;
                    self.status = "Trainer disconnected, reconnecting…".into();
                }
                TrainerEvent::CommandFailed(op, code) => {
                    self.status = format!("Trainer rejected command 0x{op:02X}: {code:?}");
                }
                TrainerEvent::Shift(dir) => {
                    if let Some(g) = self.controller.gears.as_mut() {
                        if *dir > 0 {
                            g.shift_up();
                        } else {
                            g.shift_down();
                        }
                    }
                }
                TrainerEvent::ControllerConnected(name) => {
                    self.status = format!("Shifter connected: {name}");
                }
                _ => {}
            }
            self.live.apply(&ev);
        }
        self.live.expire();
        self.handle_keys(ctx);
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.25);
        self.last_frame = Instant::now();
        self.simulate(dt);
        ctx.request_repaint_after(Duration::from_millis(50));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("RideCore").strong());
                ui.separator();
                ui.label(&self.status);
                if self.trainer_name.is_some() {
                    ui.separator();
                    ui.label(format!(
                        "Trainer data: {:.1} packets/s · flywheel {:.1} km/h · {:.0} W · {:.0} rpm",
                        self.live.packets_per_s(),
                        self.live.trainer_speed_mps * 3.6,
                        self.live.power_w,
                        self.live.cadence_rpm,
                    ));
                }
            });
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.metrics(ui);
            ui.add_space(8.0);
            self.elevation_plot(ui);
            if let Some(ride) = &self.ride {
                self.workout_panel(ui, ride);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let paused = self.ride.as_ref().is_some_and(|r| r.paused_at.is_some());
                    if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
                        let r = self.ride.as_mut().expect("ride is active");
                        match r.paused_at.take() {
                            Some(p) => r.paused_for += p.elapsed(),
                            None => r.paused_at = Some(Instant::now()),
                        }
                    }
                    if ui.button("Finish & save").clicked() {
                        self.finish_ride();
                    }
                });
            } else {
                self.setup_panel(ui);
            }
            if let Some(m) = &self.message {
                ui.add_space(8.0);
                ui.label(m);
            }
        });
    }
}
