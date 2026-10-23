//! Wire format for the Bluetooth SIG Fitness Machine Service (FTMS) and
//! Cycling Power Service (CPS), as used by the Wahoo KICKR CORE.
//!
//! Pure encode/decode only; no I/O. Field layouts follow the public
//! FTMS 1.0 and CPS 1.1 specifications.

use uuid::Uuid;

const fn sig_uuid(short: u16) -> Uuid {
    Uuid::from_u128(((short as u128) << 96) | 0x0000_0000_0000_1000_8000_0080_5f9b_34fb)
}

pub const FTMS_SERVICE: Uuid = sig_uuid(0x1826);
pub const INDOOR_BIKE_DATA: Uuid = sig_uuid(0x2AD2);
pub const FTMS_CONTROL_POINT: Uuid = sig_uuid(0x2AD9);
pub const CYCLING_POWER_MEASUREMENT: Uuid = sig_uuid(0x2A63);

/// One Indoor Bike Data notification. Absent fields are `None`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct IndoorBikeData {
    pub speed_kph: Option<f32>,
    pub cadence_rpm: Option<f32>,
    pub distance_m: Option<u32>,
    pub resistance: Option<i16>,
    pub power_w: Option<i16>,
    pub heart_rate: Option<u8>,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn u24(&mut self) -> Option<u32> {
        self.take(3)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], 0]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

pub fn parse_indoor_bike_data(buf: &[u8]) -> Option<IndoorBikeData> {
    let mut r = Reader { buf, pos: 0 };
    let flags = r.u16()?;
    let has = |bit: u16| flags & (1 << bit) != 0;
    let mut d = IndoorBikeData::default();
    // Bit 0 is "More Data": speed is present when it is *clear*.
    if !has(0) {
        d.speed_kph = Some(r.u16()? as f32 / 100.0);
    }
    if has(1) {
        r.u16()?; // average speed
    }
    if has(2) {
        d.cadence_rpm = Some(r.u16()? as f32 / 2.0);
    }
    if has(3) {
        r.u16()?; // average cadence
    }
    if has(4) {
        d.distance_m = Some(r.u24()?);
    }
    if has(5) {
        d.resistance = Some(r.i16()?);
    }
    if has(6) {
        d.power_w = Some(r.i16()?);
    }
    if has(7) {
        r.i16()?; // average power
    }
    if has(8) {
        r.take(5)?; // total, per-hour, per-minute energy
    }
    if has(9) {
        d.heart_rate = Some(r.u8()?);
    }
    Some(d)
}

/// One Cycling Power Measurement notification.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PowerMeasurement {
    pub power_w: i16,
    /// Cumulative crank revolutions and last crank event time (1/1024 s).
    pub crank: Option<(u16, u16)>,
}

pub fn parse_cycling_power(buf: &[u8]) -> Option<PowerMeasurement> {
    let mut r = Reader { buf, pos: 0 };
    let flags = r.u16()?;
    let has = |bit: u16| flags & (1 << bit) != 0;
    let power_w = r.i16()?;
    if has(0) {
        r.u8()?; // pedal power balance
    }
    if has(2) {
        r.u16()?; // accumulated torque
    }
    if has(4) {
        r.u32()?; // cumulative wheel revolutions
        r.u16()?; // last wheel event time
    }
    let crank = if has(5) {
        Some((r.u16()?, r.u16()?))
    } else {
        None
    };
    Some(PowerMeasurement { power_w, crank })
}

/// Derives cadence from successive CPS crank samples, handling u16 wraparound.
#[derive(Default)]
pub struct CrankCadence {
    last: Option<(u16, u16)>,
}

impl CrankCadence {
    /// Returns rpm when a new crank event arrived since the previous sample.
    pub fn update(&mut self, sample: (u16, u16)) -> Option<f32> {
        let prev = self.last.replace(sample)?;
        let revs = sample.0.wrapping_sub(prev.0);
        let ticks = sample.1.wrapping_sub(prev.1);
        if revs == 0 || ticks == 0 {
            return None;
        }
        Some(revs as f32 * 60.0 * 1024.0 / ticks as f32)
    }
}

/// Control Point requests (FTMS 4.16.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlRequest {
    RequestControl,
    SetTargetPower {
        watts: i16,
    },
    Start,
    /// Wind speed m/s, grade %, rolling resistance, wind resistance kg/m.
    SetSimulation {
        wind_mps: f32,
        grade_pct: f32,
        crr: f32,
        cw: f32,
    },
    /// Virtual gear for trainers that shift themselves (Zwift protocol
    /// only; FTMS has no such command).
    SetGear {
        ratio: f32,
        rider_kg: f32,
        bike_kg: f32,
    },
    /// Not sent to the trainer: drop the connection and connect again
    /// (when the trainer stopped taking its commands).
    Reconnect,
}

impl ControlRequest {
    pub fn opcode(&self) -> u8 {
        match self {
            Self::RequestControl => 0x00,
            Self::SetTargetPower { .. } => 0x05,
            Self::Start => 0x07,
            Self::SetSimulation { .. } => 0x11,
            // Not an FTMS opcode; only used to tell requests apart.
            Self::SetGear { .. } => 0xF0,
            Self::Reconnect => 0xF1,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.opcode()];
        match *self {
            Self::SetTargetPower { watts } => out.extend_from_slice(&watts.to_le_bytes()),
            Self::SetSimulation {
                wind_mps,
                grade_pct,
                crr,
                cw,
            } => {
                out.extend_from_slice(&scale_i16(wind_mps, 1000.0).to_le_bytes());
                out.extend_from_slice(&scale_i16(grade_pct, 100.0).to_le_bytes());
                out.push((crr * 10_000.0).round().clamp(0.0, 255.0) as u8);
                out.push((cw * 100.0).round().clamp(0.0, 255.0) as u8);
            }
            _ => {}
        }
        out
    }
}

fn scale_i16(v: f32, scale: f32) -> i16 {
    (v * scale).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResultCode {
    Success,
    NotSupported,
    InvalidParameter,
    Failed,
    ControlNotPermitted,
    Unknown(u8),
}

/// Parses a Control Point indication: `0x80, request opcode, result`.
pub fn parse_control_response(buf: &[u8]) -> Option<(u8, ResultCode)> {
    match buf {
        [0x80, op, code, ..] => Some((
            *op,
            match code {
                1 => ResultCode::Success,
                2 => ResultCode::NotSupported,
                3 => ResultCode::InvalidParameter,
                4 => ResultCode::Failed,
                5 => ResultCode::ControlNotPermitted,
                c => ResultCode::Unknown(*c),
            },
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_expand_from_short_form() {
        assert_eq!(
            FTMS_SERVICE.to_string(),
            "00001826-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn parses_speed_cadence_power() {
        // flags 0x0044: cadence + power present, speed present (bit 0 clear)
        let buf = [0x44, 0x00, 0xC4, 0x09, 0xB4, 0x00, 0xFA, 0x00];
        let d = parse_indoor_bike_data(&buf).unwrap();
        assert_eq!(d.speed_kph, Some(25.0));
        assert_eq!(d.cadence_rpm, Some(90.0));
        assert_eq!(d.power_w, Some(250));
    }

    #[test]
    fn truncated_packet_is_rejected() {
        assert_eq!(parse_indoor_bike_data(&[0x44, 0x00, 0xC4]), None);
    }

    #[test]
    fn parses_power_with_crank_data() {
        // flags 0x0020: crank revolution data present
        let buf = [0x20, 0x00, 0x2C, 0x01, 0x0A, 0x00, 0x00, 0x04];
        let m = parse_cycling_power(&buf).unwrap();
        assert_eq!(m.power_w, 300);
        assert_eq!(m.crank, Some((10, 1024)));
    }

    #[test]
    fn cadence_from_crank_events_handles_wraparound() {
        let mut c = CrankCadence::default();
        assert_eq!(c.update((65535, 65000)), None);
        // 1 rev in 682 ticks (~0.666 s) => ~90 rpm, both counters wrap
        let rpm = c.update((0, 65000u16.wrapping_add(682))).unwrap();
        assert!((rpm - 90.09).abs() < 0.1, "{rpm}");
    }

    #[test]
    fn encodes_simulation_parameters() {
        let req = ControlRequest::SetSimulation {
            wind_mps: 0.0,
            grade_pct: 5.5,
            crr: 0.004,
            cw: 0.51,
        };
        assert_eq!(req.encode(), vec![0x11, 0x00, 0x00, 0x26, 0x02, 40, 51]);
    }

    #[test]
    fn encodes_target_power() {
        assert_eq!(
            ControlRequest::SetTargetPower { watts: 200 }.encode(),
            vec![0x05, 0xC8, 0x00]
        );
    }

    #[test]
    fn parses_control_response() {
        assert_eq!(
            parse_control_response(&[0x80, 0x11, 0x01]),
            Some((0x11, ResultCode::Success))
        );
        assert_eq!(
            parse_control_response(&[0x80, 0x05, 0x05]),
            Some((0x05, ResultCode::ControlNotPermitted))
        );
    }
}
