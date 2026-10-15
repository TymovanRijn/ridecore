//! Turns raw trainer notifications into the live numbers a ride screen
//! shows: power (CPS preferred over FTMS), cadence, speed, heart rate.

use std::time::{Duration, Instant};

use crate::ble::TrainerEvent;
use crate::ftms::CrankCadence;

/// CPS power is preferred; fall back to FTMS power if CPS goes quiet.
const CPS_STALE_AFTER: Duration = Duration::from_secs(3);
/// Cadence and power read as zero when the trainer stops reporting them.
/// (When you stop pedalling the trainer itself reports zero, so this is
/// only for a link that has gone quiet.)
const SENSOR_STALE_AFTER: Duration = Duration::from_secs(5);

/// Latest trainer readings, with stale values expiring to zero.
#[derive(Default)]
pub struct LiveData {
    pub power_w: f32,
    pub cadence_rpm: f32,
    pub trainer_speed_mps: f32,
    pub heart_rate: Option<u8>,
    last_cps: Option<Instant>,
    last_power: Option<Instant>,
    last_cadence: Option<Instant>,
    crank: CrankCadence,
    /// Last few seconds of power for the 3 s average display.
    recent: Vec<(Instant, f32)>,
    /// Arrival times of trainer data packets, for the link-health readout.
    packets: Vec<Instant>,
}

impl LiveData {
    pub fn apply(&mut self, ev: &TrainerEvent) {
        let now = Instant::now();
        match ev {
            TrainerEvent::Power(m) => {
                self.power_w = m.power_w.max(0) as f32;
                self.last_cps = Some(now);
                self.last_power = Some(now);
                if let Some(rpm) = m.crank.and_then(|c| self.crank.update(c)) {
                    self.cadence_rpm = rpm;
                    self.last_cadence = Some(now);
                }
            }
            TrainerEvent::BikeData(d) => {
                if let Some(v) = d.speed_kph {
                    self.trainer_speed_mps = v / 3.6;
                }
                let cps_fresh = self.last_cps.is_some_and(|t| t.elapsed() < CPS_STALE_AFTER);
                if let (Some(p), false) = (d.power_w, cps_fresh) {
                    self.power_w = p.max(0) as f32;
                    self.last_power = Some(now);
                }
                if let (Some(c), false) = (d.cadence_rpm, cps_fresh) {
                    self.cadence_rpm = c;
                    self.last_cadence = Some(now);
                }
                if d.heart_rate.is_some() {
                    self.heart_rate = d.heart_rate;
                }
            }
            _ => {}
        }
        if matches!(ev, TrainerEvent::Power(_) | TrainerEvent::BikeData(_)) {
            self.packets.push(now);
            self.packets
                .retain(|t| t.elapsed() < Duration::from_secs(2));
            self.recent.push((now, self.power_w));
            self.recent
                .retain(|(t, _)| t.elapsed() < Duration::from_secs(3));
        }
    }

    /// Seconds since the last power reading, if there was one.
    pub fn power_age_s(&self) -> Option<f32> {
        self.last_power.map(|t| t.elapsed().as_secs_f32())
    }

    pub fn packets_per_s(&self) -> f32 {
        self.packets
            .iter()
            .filter(|t| t.elapsed() < Duration::from_secs(2))
            .count() as f32
            / 2.0
    }

    pub fn expire(&mut self) {
        if self
            .packets
            .last()
            .is_none_or(|t| t.elapsed() > SENSOR_STALE_AFTER)
        {
            self.trainer_speed_mps = 0.0;
        }
        if self
            .last_power
            .is_none_or(|t| t.elapsed() > SENSOR_STALE_AFTER)
        {
            self.power_w = 0.0;
        }
        if self
            .last_cadence
            .is_none_or(|t| t.elapsed() > SENSOR_STALE_AFTER)
        {
            self.cadence_rpm = 0.0;
        }
    }

    pub fn power_3s(&self) -> f32 {
        if self.recent.is_empty() {
            return self.power_w;
        }
        self.recent.iter().map(|(_, p)| p).sum::<f32>() / self.recent.len() as f32
    }
}
