//! After the race: save the ride (FIT with GPS) and its stats card, and
//! connect to and upload to Strava without blocking the game.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::egui;

use ridecore::card::{self, CardInput, RideStats};
use ridecore::fit;
use ridecore::race::course::SegmentKind;
use ridecore::strava;

use crate::RaceRes;

/// A finished ride on disk, ready to share.
#[derive(Debug, Clone)]
pub struct SavedRide {
    pub fit: PathBuf,
    pub card: Option<PathBuf>,
    pub name: String,
    pub description: String,
}

fn rides_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Documents/RideCore")
}

/// Writes the FIT file and the stats card next to it.
pub fn save_ride(res: &RaceRes) -> Result<SavedRide, String> {
    let samples = &res.samples;
    let race = &res.race;
    let me = race.player();
    let route = &race.course.route;
    let stats = RideStats::from_samples(samples);
    let place = race.position(race.human);
    let field = race.riders.len();
    let sheltered = res.sheltered_s as f32 / samples.len().max(1) as f32 * 100.0;

    let mut badges = Vec::new();
    for (kind, jersey, what) in [
        (SegmentKind::Sprint, "Groene trui", "Sprintpunten"),
        (SegmentKind::Kom, "Bolletjestrui", "Bergpunten"),
    ] {
        let pts = race.points(kind);
        if pts.first().is_some_and(|&(r, _)| r == race.human) {
            badges.push(jersey.to_string());
        }
        if let Some(&(_, p)) = pts.iter().find(|&&(r, _)| r == race.human) {
            badges.push(format!("{what} {p}"));
        }
    }
    badges.push(format!("In het wiel {sheltered:.0}%"));
    let (xp, done, tried) = res.xp;
    if tried > 0 || xp > 0 {
        badges.push(format!("{xp} XP · uitdagingen {done}/{tried}"));
    }

    let dir = rides_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Map aanmaken mislukt: {e}"))?;
    let stamp = samples
        .first()
        .map(|s| s.unix_time)
        .unwrap_or_else(fit::now_unix);
    let fit_path = dir.join(format!("race-{stamp}.fit"));
    std::fs::write(&fit_path, fit::encode_activity(samples))
        .map_err(|e| format!("Opslaan mislukt: {e}"))?;

    let [r, g, b] = race.player_team().spec.color;
    let png = card::render(&CardInput {
        title: route.name.clone(),
        subtitle: format!("{} · {}", route.region, card::date_nl(stamp)),
        place: Some((place, field)),
        stats: stats.clone(),
        rider_kg: me.profile.kg,
        badges: badges.clone(),
        samples,
        accent: [r, g, b],
    });
    let card_path = dir.join(format!("race-{stamp}.png"));
    let card = std::fs::write(&card_path, png).ok().map(|_| card_path);

    let wkg = stats.avg_power / me.profile.kg;
    let description = format!(
        "🏁 {place}e van {field} · {} ({})\n⚡ {:.0} W gem · {:.0} W NP · {wkg:.2} W/kg · max {} W\n⛰️ {:.0} hoogtemeters · {:.1} km · {:.1} km/u\n🚴 {}\nGereden in RideCore: singleplayer wedstrijd tegen AI-teams op de echte route.",
        route.name,
        route.region,
        stats.avg_power,
        stats.norm_power,
        stats.max_power,
        stats.gain_m,
        stats.distance_m / 1000.0,
        stats.avg_speed_kmh,
        badges.join(" · "),
    );
    Ok(SavedRide {
        fit: fit_path,
        card,
        name: format!("RideCore: {} – {place}e van {field}", route.name),
        description,
    })
}

/// Writes a workout's FIT file and stats card.
pub fn save_training(
    res: &RaceRes,
    title: &str,
    badges: Vec<String>,
    accent: [u8; 3],
) -> Result<SavedRide, String> {
    let samples = &res.samples;
    let me = res.race.player();
    let route = &res.race.course.route;
    let stats = RideStats::from_samples(samples);
    let dir = rides_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Map aanmaken mislukt: {e}"))?;
    let stamp = samples
        .first()
        .map(|s| s.unix_time)
        .unwrap_or_else(fit::now_unix);
    let fit_path = dir.join(format!("training-{stamp}.fit"));
    std::fs::write(&fit_path, fit::encode_activity(samples))
        .map_err(|e| format!("Opslaan mislukt: {e}"))?;
    let png = card::render(&CardInput {
        title: title.to_string(),
        subtitle: format!("Training · {} · {}", route.name, card::date_nl(stamp)),
        place: None,
        stats: stats.clone(),
        rider_kg: me.profile.kg,
        badges: badges.clone(),
        samples,
        accent,
    });
    let card_path = dir.join(format!("training-{stamp}.png"));
    let card = std::fs::write(&card_path, png).ok().map(|_| card_path);
    let description = format!(
        "🏋️ {title}\n⚡ {:.0} W gem · {:.0} W NP · {:.2} W/kg\n🚴 {}\nGereden in RideCore, in ERG-modus op {}.",
        stats.avg_power,
        stats.norm_power,
        stats.avg_power / me.profile.kg,
        badges.join(" · "),
        route.name,
    );
    Ok(SavedRide {
        fit: fit_path,
        card,
        name: format!("RideCore training: {title}"),
        description,
    })
}

/// Strava connection state, shared with background threads.
#[derive(Resource)]
pub struct StravaUi {
    pub client_id: String,
    pub client_secret: String,
    /// Who we're connected as; refreshed after connect/disconnect.
    pub connected: Option<String>,
    status: Arc<Mutex<String>>,
    busy: Arc<AtomicBool>,
    pub activity_url: Arc<Mutex<Option<String>>>,
    refresh: Arc<AtomicBool>,
    /// The keychain read, done off the main thread: macOS may hold it on an
    /// "allow access" prompt, and the game must not wait for that.
    loaded: Arc<Mutex<Option<Keychain>>>,
}

/// Client ID and connected athlete, as read from the keychain.
type Keychain = (String, Option<String>);

impl Default for StravaUi {
    fn default() -> Self {
        let ui = Self {
            client_id: String::new(),
            client_secret: String::new(),
            connected: None,
            status: Default::default(),
            busy: Default::default(),
            activity_url: Default::default(),
            refresh: Default::default(),
            loaded: Default::default(),
        };
        // Screenshot runs never touch the keychain.
        if std::env::var_os("RIDECORE_SHOTS").is_none() {
            ui.load_in_background();
        }
        ui
    }
}

impl StravaUi {
    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    fn run(&self, job: impl FnOnce(&Mutex<String>) + Send + 'static) {
        if self.busy.swap(true, Ordering::Relaxed) {
            return;
        }
        let (status, busy, refresh) =
            (self.status.clone(), self.busy.clone(), self.refresh.clone());
        std::thread::spawn(move || {
            job(&status);
            refresh.store(true, Ordering::Relaxed);
            busy.store(false, Ordering::Relaxed);
        });
    }

    /// Shows a message to the rider.
    pub fn note(&self, s: &str) {
        self.set_status(s);
    }

    fn set_status(&self, s: &str) {
        if let Ok(mut st) = self.status.lock() {
            *st = s.to_string();
        }
    }

    /// Picks up results of background work (call every frame).
    pub fn poll(&mut self) {
        if self.refresh.swap(false, Ordering::Relaxed) {
            self.load_in_background();
        }
        if let Some((id, who)) = self.loaded.lock().ok().and_then(|mut l| l.take()) {
            if self.client_id.is_empty() {
                self.client_id = id;
            }
            self.connected = who;
        }
    }

    fn load_in_background(&self) {
        let loaded = self.loaded.clone();
        std::thread::spawn(move || {
            let id = strava::client_id().unwrap_or_default();
            let who = strava::tokens().map(|t| t.athlete.unwrap_or_else(|| "je account".into()));
            if let Ok(mut l) = loaded.lock() {
                *l = Some((id, who));
            }
        });
    }

    pub fn upload(&self, ride: &SavedRide) {
        let ride = ride.clone();
        let url = self.activity_url.clone();
        self.set_status("Uploaden naar Strava…");
        self.run(move |status| {
            let result = std::fs::read(&ride.fit)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| {
                    let id = ride
                        .fit
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    strava::upload(&bytes, &ride.name, &ride.description, &id)
                });
            let msg = match result {
                Ok(link) => {
                    if let Ok(mut u) = url.lock() {
                        *u = Some(link);
                    }
                    "Geüpload naar Strava! Voeg de stats-kaart toe als foto in de Strava-app."
                        .to_string()
                }
                Err(e) => format!("Upload mislukt: {e:#}"),
            };
            if let Ok(mut s) = status.lock() {
                *s = msg;
            }
        });
    }
}

/// The Strava section of the setup screen.
pub fn settings_ui(ui: &mut egui::Ui, st: &mut StravaUi) {
    st.poll();
    if let Some(who) = st.connected.clone() {
        ui.horizontal(|ui| {
            ui.label(format!("Verbonden met Strava als {who}"));
            if ui.button("Ontkoppel").clicked() {
                strava::disconnect();
                st.connected = None;
            }
        });
    } else {
        ui.label(egui::RichText::new("Eenmalig: maak een API-app op strava.com/settings/api en zet 'Authorization Callback Domain' op localhost.").weak());
        egui::Grid::new("strava")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Client ID");
                ui.add(egui::TextEdit::singleline(&mut st.client_id).desired_width(160.0));
                ui.end_row();
                ui.label("Client Secret");
                ui.add(
                    egui::TextEdit::singleline(&mut st.client_secret)
                        .password(true)
                        .desired_width(260.0)
                        .hint_text("wordt in je Sleutelhanger bewaard"),
                );
                ui.end_row();
            });
        if ui
            .add_enabled(!st.busy(), egui::Button::new("Verbind met Strava"))
            .clicked()
        {
            let saved = if st.client_secret.is_empty() {
                Ok(())
            } else {
                strava::save_app_credentials(&st.client_id, &st.client_secret)
            };
            match saved {
                Ok(()) => {
                    st.client_secret.clear();
                    st.set_status("Je browser opent: log in en klik op 'Toestaan'…");
                    st.run(|status| {
                        let msg = match strava::connect() {
                            Ok(t) => format!("Verbonden als {}", t.athlete.unwrap_or_default()),
                            Err(e) => format!("Verbinden mislukt: {e:#}"),
                        };
                        if let Ok(mut s) = status.lock() {
                            *s = msg;
                        }
                    });
                }
                Err(e) => st.set_status(&format!("{e:#}")),
            }
        }
    }
    let status = st.status();
    if !status.is_empty() {
        ui.label(status);
    }
}

/// Opens a file with the default app (e.g. the card in Preview).
pub fn open_file(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Puts the card image on the clipboard so it can be pasted anywhere.
pub fn copy_image(path: &std::path::Path) -> bool {
    let script = format!(
        "set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)",
        path.display()
    );
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .status()
        .is_ok_and(|s| s.success())
}
