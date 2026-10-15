//! Minimal FIT activity encoder (Garmin FIT protocol 2.0), enough for
//! Strava, Garmin Connect and intervals.icu to import an indoor ride.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds between the Unix epoch and the FIT epoch (1989-12-31 00:00 UTC).
const FIT_EPOCH_OFFSET: u64 = 631_065_600;

/// One sample, normally recorded once per second.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    pub unix_time: u64,
    pub distance_m: f64,
    pub speed_mps: f32,
    pub power_w: u16,
    pub cadence_rpm: u8,
    pub heart_rate: Option<u8>,
    pub altitude_m: f32,
    /// GPS position (lat, lon) on real routes.
    pub position: Option<(f64, f64)>,
}

#[derive(Clone, Copy)]
enum Base {
    Enum = 0x00,
    U8 = 0x02,
    U16 = 0x84,
    S32 = 0x85,
    U32 = 0x86,
    U32z = 0x8C,
}

impl Base {
    fn size(self) -> u8 {
        match self {
            Base::Enum | Base::U8 => 1,
            Base::U16 => 2,
            Base::S32 | Base::U32 | Base::U32z => 4,
        }
    }
}

/// FIT sub_sport values.
const INDOOR_CYCLING: u32 = 6;
const VIRTUAL_ACTIVITY: u32 = 58;
/// "No value" for a signed 32-bit field.
const INVALID_S32: u32 = 0x7FFF_FFFF;

/// Degrees to FIT semicircles, as the raw bits of an `i32`.
fn semicircles(deg: f64) -> u32 {
    (deg * (2f64.powi(31) / 180.0)).round() as i32 as u32
}

const FILE_ID: (u8, u16) = (0, 0);
const RECORD: (u8, u16) = (1, 20);
const LAP: (u8, u16) = (2, 19);
const SESSION: (u8, u16) = (3, 18);
const ACTIVITY: (u8, u16) = (4, 34);

struct Writer {
    body: Vec<u8>,
}

impl Writer {
    fn define(&mut self, (local, global): (u8, u16), fields: &[(u8, Base)]) {
        self.body.push(0x40 | local);
        self.body.extend_from_slice(&[0, 0]); // reserved, little-endian
        self.body.extend_from_slice(&global.to_le_bytes());
        self.body.push(fields.len() as u8);
        for &(num, base) in fields {
            self.body.extend_from_slice(&[num, base.size(), base as u8]);
        }
    }

    /// Values must match the preceding definition's field order and sizes.
    fn data(&mut self, (local, _): (u8, u16), values: &[u32], fields: &[(u8, Base)]) {
        self.body.push(local);
        for (&v, &(_, base)) in values.iter().zip(fields) {
            self.body
                .extend_from_slice(&v.to_le_bytes()[..base.size() as usize]);
        }
    }

    fn message(&mut self, mesg: (u8, u16), fields: &[(u8, Base)], values: &[u32]) {
        self.define(mesg, fields);
        self.data(mesg, values, fields);
    }
}

fn fit_time(unix: u64) -> u32 {
    unix.saturating_sub(FIT_EPOCH_OFFSET) as u32
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Encodes a complete single-lap indoor cycling activity.
pub fn encode_activity(samples: &[Sample]) -> Vec<u8> {
    let start = samples
        .first()
        .map(|s| s.unix_time)
        .unwrap_or_else(now_unix);
    let end = samples.last().map(|s| s.unix_time).unwrap_or(start);
    let elapsed_ms = ((end - start) * 1000) as u32;
    let distance_cm = samples
        .last()
        .map(|s| (s.distance_m * 100.0) as u32)
        .unwrap_or(0);
    let avg_power = if samples.is_empty() {
        0
    } else {
        (samples.iter().map(|s| s.power_w as u64).sum::<u64>() / samples.len() as u64) as u32
    };
    let max_power = samples.iter().map(|s| s.power_w as u32).max().unwrap_or(0);

    let mut w = Writer { body: Vec::new() };
    w.message(
        FILE_ID,
        &[
            (0, Base::Enum),
            (1, Base::U16),
            (2, Base::U16),
            (3, Base::U32z),
            (4, Base::U32),
        ],
        // activity file, manufacturer "development", product 1, serial, created
        &[4, 255, 1, 0x5249_4445, fit_time(start)],
    );

    // Rides on real routes carry GPS and are marked as virtual rides.
    let gps = samples.iter().any(|s| s.position.is_some());
    let sub_sport = if gps {
        VIRTUAL_ACTIVITY
    } else {
        INDOOR_CYCLING
    };
    let rec_fields = [
        (253, Base::U32),
        (0, Base::S32),
        (1, Base::S32),
        (5, Base::U32),
        (6, Base::U16),
        (7, Base::U16),
        (4, Base::U8),
        (3, Base::U8),
        (2, Base::U16),
    ];
    w.define(RECORD, &rec_fields);
    for s in samples {
        let (lat, lon) = s
            .position
            .map(|(a, b)| (semicircles(a), semicircles(b)))
            .unwrap_or((INVALID_S32, INVALID_S32));
        let values = [
            fit_time(s.unix_time),
            lat,
            lon,
            (s.distance_m * 100.0) as u32,
            (s.speed_mps * 1000.0) as u32,
            s.power_w as u32,
            s.cadence_rpm as u32,
            s.heart_rate.map(u32::from).unwrap_or(0xFF), // 0xFF = invalid
            ((s.altitude_m + 500.0) * 5.0).max(0.0).round() as u32,
        ];
        w.data(RECORD, &values, &rec_fields);
    }

    let t_end = fit_time(end);
    let t_start = fit_time(start);
    w.message(
        LAP,
        &[
            (253, Base::U32),
            (2, Base::U32),
            (7, Base::U32),
            (8, Base::U32),
            (9, Base::U32),
            (0, Base::Enum),
            (1, Base::Enum),
        ],
        // event lap(9), event_type stop(1)
        &[t_end, t_start, elapsed_ms, elapsed_ms, distance_cm, 9, 1],
    );
    w.message(
        SESSION,
        &[
            (253, Base::U32),
            (2, Base::U32),
            (7, Base::U32),
            (8, Base::U32),
            (9, Base::U32),
            (5, Base::Enum),
            (6, Base::Enum),
            (20, Base::U16),
            (21, Base::U16),
            (0, Base::Enum),
            (1, Base::Enum),
            (25, Base::U16),
            (26, Base::U16),
        ],
        // sport cycling(2), sub_sport, event session(8), stop(1)
        &[
            t_end,
            t_start,
            elapsed_ms,
            elapsed_ms,
            distance_cm,
            2,
            sub_sport,
            avg_power,
            max_power,
            8,
            1,
            0,
            1,
        ],
    );
    w.message(
        ACTIVITY,
        &[
            (253, Base::U32),
            (0, Base::U32),
            (1, Base::U16),
            (2, Base::Enum),
            (3, Base::Enum),
            (4, Base::Enum),
        ],
        // manual activity(0), event activity(26), stop(1)
        &[t_end, elapsed_ms, 1, 0, 26, 1],
    );

    let mut header = vec![14u8, 0x20];
    header.extend_from_slice(&2132u16.to_le_bytes()); // profile version 21.32
    header.extend_from_slice(&(w.body.len() as u32).to_le_bytes());
    header.extend_from_slice(b".FIT");
    let header_crc = crc16(&header);
    header.extend_from_slice(&header_crc.to_le_bytes());

    let mut file = header;
    file.extend_from_slice(&w.body);
    let crc = crc16(&file);
    file.extend_from_slice(&crc.to_le_bytes());
    file
}

/// FIT checksum (CRC-16/ARC, nibble-table form from the FIT SDK).
fn crc16(bytes: &[u8]) -> u16 {
    const TABLE: [u16; 16] = [
        0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401, 0xA001, 0x6C00, 0x7800,
        0xB401, 0x5000, 0x9C01, 0x8801, 0x4400,
    ];
    bytes.iter().fold(0u16, |mut crc, &b| {
        for nibble in [b & 0x0F, b >> 4] {
            let tmp = TABLE[(crc & 0x0F) as usize];
            crc = (crc >> 4) & 0x0FFF;
            crc = crc ^ tmp ^ TABLE[nibble as usize];
        }
        crc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicircles_round_trip() {
        let s = semicircles(45.092429) as i32;
        assert!((s as f64 * 180.0 / 2f64.powi(31) - 45.092429).abs() < 1e-6);
        assert_eq!(semicircles(-10.0) as i32, -119_304_647);
    }

    #[test]
    fn crc_matches_crc16_arc_check_value() {
        assert_eq!(crc16(b"123456789"), 0xBB3D);
    }

    #[test]
    fn file_is_self_consistent() {
        let samples: Vec<Sample> = (0..60)
            .map(|i| Sample {
                unix_time: 1_760_000_000 + i,
                distance_m: i as f64 * 8.0,
                speed_mps: 8.0,
                power_w: 200,
                cadence_rpm: 90,
                heart_rate: None,
                altitude_m: 10.0,
                position: Some((45.05 + i as f64 * 1e-4, 6.03)),
            })
            .collect();
        let f = encode_activity(&samples);
        assert_eq!(&f[8..12], b".FIT");
        assert_eq!(crc16(&f[..12]), u16::from_le_bytes([f[12], f[13]]));
        let data_len = u32::from_le_bytes([f[4], f[5], f[6], f[7]]) as usize;
        assert_eq!(f.len(), 14 + data_len + 2);
        // A file's CRC over everything including the trailing CRC is zero.
        assert_eq!(crc16(&f), 0);
    }
}
