//! Zwift's own trainer protocol, which trainers with Zwift virtual shifting
//! (KICKR CORE Zwift One, Zwift Hub, …) speak next to FTMS. The app tells
//! the trainer the road (grade, rolling and air resistance) and the virtual
//! gear ratio; the trainer then works out the resistance itself from its own
//! flywheel speed, so shifts land instantly and nothing oscillates.
//!
//! It lives on the same Zwift service as the Click (`00000001-19ca-…`):
//! write `"RideOn" 02 01` to the sync-RX characteristic, then `41 08 05`;
//! the trainer answers with riding data (`0x03`) on the async characteristic.
//! Commands are `0x04` + a protobuf `HubCommand`:
//!
//! ```text
//! HubCommand      { 3: PowerTarget, 4: SimulationParam, 5: PhysicalParam }
//! SimulationParam { 1: sint32 wind×100, 2: sint32 incline×100,
//!                   3: CWa×10000, 4: Crr×100000 }
//! PhysicalParam   { 2: GearRatio×10000, 4: BikeKg×100, 5: RiderKg×100 }
//! ```
//!
//! Field map as published by the incyclist and qdomyos-zwift projects.

use crate::ftms::{ControlRequest, IndoorBikeData};

/// The handshake for a trainer (a controller gets plain "RideOn").
pub fn hello() -> Vec<u8> {
    let mut v = b"RideOn".to_vec();
    v.extend([0x02, 0x01]);
    v
}

/// Asks the trainer to start its riding-data stream.
pub const START: [u8; 3] = [0x41, 0x08, 0x05];

/// `HubRequest { DataId: 512 }`, sent after each command (as Zwift does) so
/// the trainer reports back its settings.
pub const SETTINGS_REQUEST: [u8; 4] = [0x00, 0x08, 0x80, 0x04];

const OP_RIDING_DATA: u8 = 0x03;
const OP_COMMAND: u8 = 0x04;

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn uint(out: &mut Vec<u8>, field: u32, v: u32) {
    varint(out, u64::from(field << 3));
    varint(out, u64::from(v));
}

fn sint(out: &mut Vec<u8>, field: u32, v: i32) {
    varint(out, u64::from(field << 3));
    varint(out, u64::from(((v << 1) ^ (v >> 31)) as u32));
}

fn message(out: &mut Vec<u8>, field: u32, body: &[u8]) {
    varint(out, u64::from(field << 3 | 2));
    varint(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn round_u32(v: f32) -> u32 {
    v.round().clamp(0.0, u32::MAX as f32) as u32
}

/// The bytes for `req` on the Zwift protocol, or None for FTMS-only
/// requests (taking control, starting).
pub fn encode(req: &ControlRequest) -> Option<Vec<u8>> {
    let mut cmd = Vec::new();
    match *req {
        ControlRequest::SetTargetPower { watts } => {
            uint(&mut cmd, 3, watts.max(0) as u32);
        }
        ControlRequest::SetSimulation {
            wind_mps,
            grade_pct,
            crr,
            cw,
        } => {
            // A grade of exactly 0 is sent as 0.01% (as incyclist does):
            // some firmware ignores a zero incline.
            let incline = match (grade_pct * 100.0).round() as i32 {
                0 => 1,
                g => g,
            };
            let mut sim = Vec::new();
            sint(&mut sim, 1, (wind_mps * 100.0).round() as i32);
            sint(&mut sim, 2, incline);
            uint(&mut sim, 3, round_u32(cw * 10_000.0));
            uint(&mut sim, 4, round_u32(crr * 100_000.0));
            message(&mut cmd, 4, &sim);
        }
        ControlRequest::SetGear {
            ratio,
            rider_kg,
            bike_kg,
        } => {
            let mut phys = Vec::new();
            uint(&mut phys, 2, round_u32(ratio * 10_000.0));
            uint(&mut phys, 4, round_u32(bike_kg * 100.0));
            uint(&mut phys, 5, round_u32(rider_kg * 100.0));
            message(&mut cmd, 5, &phys);
        }
        ControlRequest::RequestControl | ControlRequest::Start | ControlRequest::Reconnect => {
            return None;
        }
    }
    let mut out = vec![OP_COMMAND];
    out.extend(cmd);
    Some(out)
}

/// Riding data the trainer streams on the Zwift protocol:
/// `HubRidingData { 1: power, 2: cadence, 3: speed×100, 4: heart rate }`.
pub fn parse_riding_data(buf: &[u8]) -> Option<IndoorBikeData> {
    let (&op, body) = buf.split_first()?;
    if op != OP_RIDING_DATA {
        return None;
    }
    let mut d = IndoorBikeData::default();
    for (field, v) in crate::click::varint_fields(body)? {
        match field {
            1 => d.power_w = Some(v.min(i16::MAX as u64) as i16),
            2 => d.cadence_rpm = Some(v as f32),
            3 => d.speed_kph = Some(v as f32 / 100.0),
            4 if v > 0 => d.heart_rate = Some(v.min(255) as u8),
            _ => {}
        }
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gear_ratio_is_sent_as_physical_param() {
        let b = encode(&ControlRequest::SetGear {
            ratio: 2.4,
            rider_kg: 75.0,
            bike_kg: 8.0,
        })
        .unwrap();
        // 0x04, field 5 (len 10): 2 = 24000, 4 = 800, 5 = 7500.
        assert_eq!(
            b,
            [
                0x04, 0x2a, 0x0a, 0x10, 0xc0, 0xbb, 0x01, 0x20, 0xa0, 0x06, 0x28, 0xcc, 0x3a
            ]
        );
    }

    #[test]
    fn simulation_uses_zigzag_grades() {
        let b = encode(&ControlRequest::SetSimulation {
            wind_mps: 0.0,
            grade_pct: -1.0,
            crr: 0.004,
            cw: 0.51,
        })
        .unwrap();
        // incline -100 → zigzag 199; CWa 5100; Crr 400.
        assert_eq!(
            b,
            [
                0x04, 0x22, 0x0b, 0x08, 0x00, 0x10, 0xc7, 0x01, 0x18, 0xec, 0x27, 0x20, 0x90, 0x03
            ]
        );
        let flat = encode(&ControlRequest::SetSimulation {
            wind_mps: 0.0,
            grade_pct: 0.0,
            crr: 0.004,
            cw: 0.51,
        })
        .unwrap();
        assert_eq!(&flat[5..7], &[0x10, 0x02]);
    }

    #[test]
    fn power_target_and_ftms_only_requests() {
        assert_eq!(
            encode(&ControlRequest::SetTargetPower { watts: 250 }).unwrap(),
            [0x04, 0x18, 0xfa, 0x01]
        );
        assert_eq!(encode(&ControlRequest::Start), None);
    }

    #[test]
    fn riding_data_is_read() {
        // power 212, cadence 88, 33.45 km/h.
        let d = parse_riding_data(&[0x03, 0x08, 0xd4, 0x01, 0x10, 0x58, 0x18, 0x91, 0x1a]).unwrap();
        assert_eq!(d.power_w, Some(212));
        assert_eq!(d.cadence_rpm, Some(88.0));
        assert_eq!(d.speed_kph, Some(33.45));
        assert_eq!(parse_riding_data(&[0x2a, 0x08]), None);
    }
}
