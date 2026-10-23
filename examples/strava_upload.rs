//! Uploads a saved ride to Strava by hand, for when the upload from the
//! results screen didn't happen: uses the same login (keychain) and the
//! same Virtual Ride handling as the app.
//!
//! cargo run --release --example strava_upload -- <ride.fit> <name> [description]

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (path, name) = match &args[..] {
        [path, name, ..] => (path, name),
        _ => anyhow::bail!("usage: strava_upload <ride.fit> <name> [description]"),
    };
    let description = args.get(2).map(String::as_str).unwrap_or("");
    let fit = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let id = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    anyhow::ensure!(
        ridecore::strava::is_connected(),
        "not connected to Strava: connect in the app first"
    );
    let url = ridecore::strava::upload(&fit, name, description, &id)?;
    println!("{url}");
    Ok(())
}
