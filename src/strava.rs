//! Strava: connect once through the browser (OAuth), keep credentials in
//! the OS keychain, and upload FIT files as activities.
//!
//! Rides are uploaded as Virtual Rides with GPS, so Strava draws the real
//! route on its map.
//!
//! Strava's API has no endpoint for adding photos to an activity, so the
//! stats card is saved locally for the rider to add by hand.

use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpListener;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::i18n::t;
use crate::tf;

const SERVICE: &str = "RideCore Strava";
/// Strava must list `localhost` as the app's Authorization Callback Domain.
const CALLBACK_PORT: u16 = 8723;
const AUTH_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    pub athlete: Option<String>,
}

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).map_err(|e| anyhow!("keychain unavailable: {e}"))
}

fn load(key: &str) -> Option<String> {
    entry(key).ok()?.get_password().ok()
}

fn store(key: &str, value: &str) -> Result<()> {
    entry(key)?
        .set_password(value)
        .map_err(|e| anyhow!("could not save to the keychain: {e}"))
}

/// The app's Strava API credentials, as entered by the rider.
pub fn save_app_credentials(client_id: &str, client_secret: &str) -> Result<()> {
    if client_id.trim().is_empty() || !client_id.trim().chars().all(|c| c.is_ascii_digit()) {
        bail!(t(
            "Client ID is een getal (zie strava.com/settings/api)",
            "The Client ID is a number (see strava.com/settings/api)"
        ));
    }
    if client_secret.trim().len() < 20 {
        bail!(t(
            "Client Secret lijkt niet compleet",
            "The Client Secret looks incomplete"
        ));
    }
    store("client_id", client_id.trim())?;
    store("client_secret", client_secret.trim())
}

pub fn client_id() -> Option<String> {
    load("client_id")
}

pub fn tokens() -> Option<Tokens> {
    serde_json::from_str(&load("tokens")?).ok()
}

pub fn is_connected() -> bool {
    tokens().is_some()
}

pub fn disconnect() {
    if let Ok(e) = entry("tokens") {
        let _ = e.delete_credential();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    athlete: Option<Athlete>,
}

#[derive(Deserialize)]
struct Athlete {
    firstname: Option<String>,
    lastname: Option<String>,
}

fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse> {
    let mut res = agent()
        .post("https://www.strava.com/oauth/token")
        .send_form(form.iter().copied())?;
    let status = res.status();
    let body = res.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!(tf!(
            "Strava weigerde ({status}): {body}",
            "Strava refused ({status}): {body}"
        ));
    }
    Ok(serde_json::from_str(&body)?)
}

/// Opens the browser for the rider to authorize RideCore, waits for the
/// redirect on localhost, exchanges the code and stores the tokens.
pub fn connect() -> Result<Tokens> {
    let id = client_id().context(t(
        "vul eerst je Client ID en Client Secret in",
        "enter your Client ID and Client Secret first",
    ))?;
    let secret = load("client_secret").context(t(
        "vul eerst je Client Secret in",
        "enter your Client Secret first",
    ))?;
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .context(t("poort 8723 is bezet", "port 8723 is in use"))?;
    let redirect = format!("http://localhost:{CALLBACK_PORT}/strava");
    let url = format!(
        "https://www.strava.com/oauth/authorize?client_id={id}&response_type=code&redirect_uri={redirect}&approval_prompt=auto&scope=read,activity:read_all,activity:write"
    );
    open_in_browser(&url)?;

    listener.set_nonblocking(true)?;
    let start = Instant::now();
    let code = loop {
        if start.elapsed() > AUTH_TIMEOUT {
            bail!(t(
                "geen antwoord van Strava binnen 3 minuten",
                "no answer from Strava within 3 minutes"
            ));
        }
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        stream.set_nonblocking(false)?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;
        let query = line
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.split_once('?'))
            .map(|(_, q)| q.to_string())
            .unwrap_or_default();
        let param = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                .map(str::to_string)
        };
        let (ok, msg) = match (param("code"), param("error")) {
            (Some(c), _) => (
                Some(c),
                t(
                    "RideCore is verbonden met Strava. Je kunt dit tabblad sluiten.",
                    "RideCore is connected to Strava. You can close this tab.",
                ),
            ),
            (_, Some(_)) => (
                None,
                t(
                    "Toegang geweigerd. Je kunt dit tabblad sluiten en het opnieuw proberen.",
                    "Access denied. You can close this tab and try again.",
                ),
            ),
            _ => continue,
        };
        let page = format!(
            "<html><body style=\"font-family:sans-serif;padding:40px\"><h2>{msg}</h2></body></html>"
        );
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n{page}",
            page.len()
        );
        match ok {
            Some(code) => break code,
            None => bail!(t(
                "je hebt geen toegang gegeven op Strava",
                "you didn't give access on Strava"
            )),
        }
    };

    let t = token_request(&[
        ("client_id", &id),
        ("client_secret", &secret),
        ("code", &code),
        ("grant_type", "authorization_code"),
    ])?;
    let tokens = Tokens {
        access_token: t.access_token,
        refresh_token: t.refresh_token,
        expires_at: t.expires_at,
        athlete: t.athlete.map(|a| {
            format!(
                "{} {}",
                a.firstname.unwrap_or_default(),
                a.lastname.unwrap_or_default()
            )
            .trim()
            .to_string()
        }),
    };
    store("tokens", &serde_json::to_string(&tokens)?)?;
    Ok(tokens)
}

/// A valid access token, refreshed when it is about to expire.
fn access_token() -> Result<String> {
    let mut t = tokens().context(crate::i18n::t(
        "niet verbonden met Strava",
        "not connected to Strava",
    ))?;
    if t.expires_at > now() + 120 {
        return Ok(t.access_token);
    }
    let id = client_id().context(crate::i18n::t("Client ID ontbreekt", "Client ID missing"))?;
    let secret = load("client_secret").context(crate::i18n::t(
        "Client Secret ontbreekt",
        "Client Secret missing",
    ))?;
    let r = token_request(&[
        ("client_id", &id),
        ("client_secret", &secret),
        ("refresh_token", &t.refresh_token),
        ("grant_type", "refresh_token"),
    ])?;
    t.access_token = r.access_token;
    t.refresh_token = r.refresh_token;
    t.expires_at = r.expires_at;
    store("tokens", &serde_json::to_string(&t)?)?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
struct UploadStatus {
    id: Option<u64>,
    status: Option<String>,
    error: Option<String>,
    activity_id: Option<u64>,
}

/// Uploads a FIT file and waits until Strava has turned it into an
/// activity. Returns the activity's URL.
pub fn upload(fit: &[u8], name: &str, description: &str, external_id: &str) -> Result<String> {
    let token = access_token()?;
    let boundary = format!("ridecore{}", now());
    let mut body = Vec::new();
    for (k, v) in [
        ("data_type", "fit"),
        ("name", name),
        ("description", description),
        // No "trainer" flag: Strava hides the map of trainer activities. The
        // FIT says virtual_activity, and we set VirtualRide below.
        ("external_id", external_id),
    ] {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        )?;
    }
    write!(
        body,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{external_id}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )?;
    body.extend_from_slice(fit);
    write!(body, "\r\n--{boundary}--\r\n")?;

    let agent = agent();
    let mut res = agent
        .post("https://www.strava.com/api/v3/uploads")
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .send(&body[..])?;
    let status = res.status();
    let text = res.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!(tf!(
            "upload geweigerd ({status}): {text}",
            "upload refused ({status}): {text}"
        ));
    }
    let mut up: UploadStatus = serde_json::from_str(&text)?;
    let id = up.id.context(t(
        "Strava gaf geen upload-id",
        "Strava returned no upload id",
    ))?;
    for _ in 0..30 {
        if let Some(activity) = up.activity_id {
            // Make it a Virtual Ride with its map, as Zwift rides appear.
            let _ = make_virtual_ride(activity, Some(name));
            return Ok(format!("https://www.strava.com/activities/{activity}"));
        }
        if let Some(err) = up.error.filter(|e| !e.is_empty()) {
            bail!(tf!(
                "Strava kon de rit niet verwerken: {err}",
                "Strava could not process the ride: {err}"
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
        let mut r = agent
            .get(format!("https://www.strava.com/api/v3/uploads/{id}"))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        up = serde_json::from_str(&r.body_mut().read_to_string()?)?;
    }
    bail!(tf!(
        "Strava is nog bezig ({}); de rit verschijnt zo vanzelf",
        "Strava is still busy ({}); the ride will show up shortly",
        up.status.unwrap_or_default()
    ))
}

/// Turns an activity into a Virtual Ride (not a trainer ride), so Strava
/// shows its GPS route on the map.
pub fn make_virtual_ride(activity: u64, name: Option<&str>) -> Result<()> {
    let token = access_token()?;
    let mut form = vec![("sport_type", "VirtualRide"), ("trainer", "false")];
    if let Some(n) = name {
        form.push(("name", n));
    }
    let mut res = agent()
        .put(format!(
            "https://www.strava.com/api/v3/activities/{activity}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send_form(form)?;
    let status = res.status();
    let body = res.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!(tf!(
            "Strava weigerde de wijziging ({status}): {body}",
            "Strava refused the change ({status}): {body}"
        ));
    }
    Ok(())
}

/// The athlete's most recent activities as raw JSON objects.
pub fn recent_activities(count: u32) -> Result<Vec<serde_json::Value>> {
    let token = access_token()?;
    let mut res = agent()
        .get(format!(
            "https://www.strava.com/api/v3/athlete/activities?per_page={count}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .call()?;
    let status = res.status();
    let body = res.body_mut().read_to_string()?;
    if !status.is_success() {
        bail!(tf!(
            "Strava weigerde ({status}): {body}",
            "Strava refused ({status}): {body}"
        ));
    }
    Ok(serde_json::from_str(&body)?)
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(url).spawn().context(t(
        "kon de browser niet openen",
        "could not open the browser",
    ))?;
    Ok(())
}
