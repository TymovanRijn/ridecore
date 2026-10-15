//! Procedurally synthesized race audio: looping beds (drivetrain,
//! bunch, crowd, heartbeat) whose volume follows the race, plus one-shot
//! stings for good moments. No sound files are needed.

use std::f32::consts::TAU;
use std::sync::Arc;

use bevy::audio::{AudioSinkPlayback, PlaybackMode, Volume};
use bevy::prelude::*;

use ridecore::rng::Rng;

use crate::{RaceRes, Screen};

pub const RATE: u32 = 44_100;

#[derive(Resource)]
pub struct SoundBank {
    whir: Handle<AudioSource>,
    bunch: Handle<AudioSource>,
    crowd: Handle<AudioSource>,
    heart: Handle<AudioSource>,
    pub chime: Handle<AudioSource>,
    pub bell: Handle<AudioSource>,
    pub fanfare: Handle<AudioSource>,
    shift: Handle<AudioSource>,
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Bed {
    Whir,
    Bunch,
    Crowd,
    Heart,
}

/// Marks race sounds so they are cleared when leaving the race.
#[derive(Component)]
struct RaceSound;

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, build_bank)
            .add_systems(OnEnter(Screen::Racing), start_beds)
            .add_systems(OnEnter(Screen::Setup), stop_all)
            .add_systems(
                Update,
                mix_beds.run_if(in_state(Screen::Racing).or_else(in_state(Screen::Results))),
            )
            .add_systems(Update, shift_sound);
    }
}

/// `RIDECORE_MUTE=1` silences the game (used for automated test runs).
pub fn muted() -> bool {
    std::env::var_os("RIDECORE_MUTE").is_some()
}

/// Plays a one-shot sound.
pub fn play(commands: &mut Commands, sound: &Handle<AudioSource>, volume: f32) {
    if muted() {
        return;
    }
    commands.spawn((
        AudioPlayer(sound.clone()),
        PlaybackSettings {
            mode: PlaybackMode::Despawn,
            volume: Volume::Linear(volume),
            ..default()
        },
        RaceSound,
    ));
}

fn build_bank(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let mut add = |samples: Vec<f32>| {
        sources.add(AudioSource {
            bytes: Arc::from(wav(&samples)),
        })
    };
    commands.insert_resource(SoundBank {
        whir: add(looped(whir(4.0), 0.3)),
        bunch: add(looped(bunch(6.0), 0.5)),
        crowd: add(looped(crowd(6.0), 0.6)),
        heart: add(heartbeat()),
        chime: add(chime()),
        bell: add(bell()),
        fanfare: add(fanfare()),
        shift: add(shift_click()),
    });
}

fn start_beds(mut commands: Commands, bank: Res<SoundBank>) {
    if muted() {
        return;
    }
    for (bed, handle) in [
        (Bed::Whir, &bank.whir),
        (Bed::Bunch, &bank.bunch),
        (Bed::Crowd, &bank.crowd),
        (Bed::Heart, &bank.heart),
    ] {
        commands.spawn((
            bed,
            RaceSound,
            AudioPlayer(handle.clone()),
            PlaybackSettings {
                mode: PlaybackMode::Loop,
                volume: Volume::Linear(0.0),
                ..default()
            },
        ));
    }
}

fn stop_all(mut commands: Commands, sounds: Query<Entity, With<RaceSound>>) {
    for e in &sounds {
        commands.entity(e).despawn();
    }
}

/// Sets each bed's volume and pitch from what the player is experiencing.
fn mix_beds(
    time: Res<Time>,
    res: Res<RaceRes>,
    fans: Option<Res<crate::world::Fans>>,
    mut beds: Query<(&Bed, &mut AudioSink)>,
) {
    let race = &res.race;
    let me = race.player();
    let fans = fans.map_or(0.0, |f| f.density(me.dist));
    let v = me.speed;
    let near = race
        .riders
        .iter()
        .filter(|r| !r.human && (r.dist - me.dist).abs() < 14.0)
        .count() as f32;
    let crowd_zone = race
        .course
        .segments
        .iter()
        .map(|s| s.end_m)
        .chain(std::iter::once(race.course.total_m))
        .map(|line| line - me.dist)
        .filter(|d| (-60.0..300.0).contains(d))
        .map(|d| {
            if d < 0.0 {
                1.0 + d as f32 / 60.0
            } else {
                1.0 - d as f32 / 300.0
            }
        })
        .fold(0.0f32, f32::max);
    let low = (0.3 - me.energy.fraction()).max(0.0) / 0.3;
    let k = 1.0 - (-time.delta_secs() * 6.0).exp();
    for (bed, mut sink) in &mut beds {
        let (target, speed) = if res.paused {
            (0.0, 1.0)
        } else {
            match bed {
                Bed::Whir => ((v / 12.0).min(1.0) * 0.18, 0.7 + v / 25.0),
                Bed::Bunch => ((near / 12.0).min(1.0) * 0.4, 1.0),
                Bed::Crowd => ((crowd_zone * 0.6).max(fans * 0.5), 1.0),
                Bed::Heart => (low * 0.9, 1.0 + low * 0.8),
            }
        };
        let current = sink.volume().to_linear();
        sink.set_volume(Volume::Linear(current + (target - current) * k));
        sink.set_speed(speed);
    }
}

/// Encodes mono samples in −1..1 as a 16-bit WAV file.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32_000.0) as i16).to_le_bytes());
    }
    out
}

/// Crossfades the tail into the head so the buffer loops without a click.
fn looped(mut s: Vec<f32>, fade_s: f32) -> Vec<f32> {
    let n = (fade_s * RATE as f32) as usize;
    let len = s.len() - n;
    for i in 0..n {
        let t = i as f32 / n as f32;
        s[i] = s[i] * t + s[len + i] * (1.0 - t);
    }
    s.truncate(len);
    s
}

pub fn normalize(mut s: Vec<f32>, peak: f32) -> Vec<f32> {
    let max = s.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-6);
    for x in &mut s {
        *x *= peak / max;
    }
    s
}

/// Soft drivetrain hum: low-passed noise with a gentle ripple.
fn whir(secs: f32) -> Vec<f32> {
    let mut rng = Rng::new(5);
    let mut y = 0.0f32;
    let s = (0..(secs * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            y += (rng.range(-1.0, 1.0) - y) * 0.08;
            y * (0.8 + 0.2 * (t * TAU * 11.0).sin())
        })
        .collect();
    normalize(s, 0.6)
}

/// Many wheels and drivetrains: brighter noise with slow swells.
fn bunch(secs: f32) -> Vec<f32> {
    let mut rng = Rng::new(9);
    let (mut lo, mut hi) = (0.0f32, 0.0f32);
    let s = (0..(secs * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let w = rng.range(-1.0, 1.0);
            lo += (w - lo) * 0.25;
            hi += (lo - hi) * 0.02;
            (lo - hi) * (0.75 + 0.25 * (t * 0.7).sin())
        })
        .collect();
    normalize(s, 0.6)
}

/// Roadside crowd: band-limited noise with swelling cheers and whistles.
fn crowd(secs: f32) -> Vec<f32> {
    let mut rng = Rng::new(21);
    let n = (secs * RATE as f32) as usize;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    let mut s: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let w = rng.range(-1.0, 1.0);
            a += (w - a) * 0.35;
            b += (a - b) * 0.05;
            (a - b) * (0.6 + 0.4 * ((t * 1.3).sin() * (t * 0.37).cos()).abs())
        })
        .collect();
    // A few rising whistles.
    for _ in 0..5 {
        let start = rng.range(0.0, secs - 1.0);
        let base = rng.range(1400.0, 2200.0);
        let mut phase = 0.0f32;
        for k in 0..(0.6 * RATE as f32) as usize {
            let t = k as f32 / RATE as f32;
            phase += TAU * (base + t * 900.0) / RATE as f32;
            let env = (t / 0.05).min(1.0) * (1.0 - t / 0.6);
            let idx = ((start * RATE as f32) as usize + k).min(n - 1);
            s[idx] += phase.sin() * env * 0.15;
        }
    }
    normalize(s, 0.7)
}

fn thump(s: &mut [f32], at: f32, freq: f32, amp: f32) {
    let start = (at * RATE as f32) as usize;
    for k in 0..(0.18 * RATE as f32) as usize {
        let t = k as f32 / RATE as f32;
        if let Some(x) = s.get_mut(start + k) {
            *x += (TAU * freq * t).sin() * (-t * 28.0).exp() * amp;
        }
    }
}

/// One second of "lub-dub"; playback speed sets the heart rate.
fn heartbeat() -> Vec<f32> {
    let mut s = vec![0.0; RATE as usize];
    thump(&mut s, 0.0, 55.0, 1.0);
    thump(&mut s, 0.27, 48.0, 0.7);
    normalize(s, 0.9)
}

pub fn tone(s: &mut [f32], at: f32, freq: f32, dur: f32, amp: f32, decay: f32) {
    let start = (at * RATE as f32) as usize;
    for k in 0..(dur * RATE as f32) as usize {
        let t = k as f32 / RATE as f32;
        if let Some(x) = s.get_mut(start + k) {
            let attack = (t / 0.005).min(1.0);
            *x += ((TAU * freq * t).sin() + 0.3 * (TAU * freq * 2.0 * t).sin())
                * attack
                * (-t * decay).exp()
                * amp;
        }
    }
}

/// The mechanical tick of a derailleur moving: a short noise burst with a
/// metallic ring.
fn shift_click() -> Vec<f32> {
    let n = (0.12 * RATE as f32) as usize;
    let mut rng = Rng::new(7);
    let mut s: Vec<f32> = (0..n)
        .map(|k| {
            let t = k as f32 / RATE as f32;
            (rng.f32() * 2.0 - 1.0) * (-t * 90.0).exp()
        })
        .collect();
    tone(&mut s, 0.0, 3400.0, 0.12, 0.35, 45.0);
    tone(&mut s, 0.018, 2600.0, 0.1, 0.25, 50.0);
    normalize(s, 0.7)
}

fn shift_sound(
    mut commands: Commands,
    bank: Option<Res<SoundBank>>,
    trainer: Res<crate::link::Trainer>,
    mut heard: Local<u32>,
) {
    if trainer.shifts != *heard {
        *heard = trainer.shifts;
        if let Some(bank) = bank {
            play(&mut commands, &bank.shift, 0.5);
        }
    }
}

/// A bright two-note "ding" for good moments.
fn chime() -> Vec<f32> {
    let mut s = vec![0.0; (0.9 * RATE as f32) as usize];
    tone(&mut s, 0.0, 1318.5, 0.9, 0.6, 6.0);
    tone(&mut s, 0.09, 1975.5, 0.8, 0.5, 6.0);
    normalize(s, 0.8)
}

/// A last-lap bell: inharmonic partials, rung twice.
fn bell() -> Vec<f32> {
    let mut s = vec![0.0; (2.6 * RATE as f32) as usize];
    for at in [0.0, 0.45] {
        for (f, a) in [
            (880.0, 0.6),
            (1760.0 * 1.19, 0.3),
            (2640.0 * 1.07, 0.2),
            (440.0, 0.25),
        ] {
            tone(&mut s, at, f, 2.0, a, 2.2);
        }
    }
    normalize(s, 0.8)
}

/// A short rising fanfare for podium finishes.
fn fanfare() -> Vec<f32> {
    let mut s = vec![0.0; (1.8 * RATE as f32) as usize];
    for (i, f) in [523.3, 659.3, 784.0, 1046.5].iter().enumerate() {
        tone(&mut s, i as f32 * 0.14, *f, 1.2, 0.5, 2.5);
    }
    tone(&mut s, 0.56, 784.0, 1.2, 0.35, 2.0);
    normalize(s, 0.8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let w = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]), 6);
        assert_eq!(w.len(), 44 + 6);
    }

    #[test]
    fn loops_have_no_seam() {
        let s = looped(whir(2.0), 0.3);
        // The first sample continues smoothly from the last.
        assert!((s[0] - s[s.len() - 1]).abs() < 0.1);
    }
}
