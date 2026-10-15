//! Zwift Click (and Play/Ride) shift buttons over Bluetooth.
//!
//! The controller exposes Zwift's own service (UUIDs confirmed in the Zwift
//! app binary). After connecting, the app writes "RideOn" to the sync-RX
//! characteristic; the controller answers "RideOn…" and from then on sends
//! button state on the async characteristic as `[opcode, protobuf…]`. Without
//! a public key in the handshake the messages are not encrypted.
//! Click firmware 1.0.0 ignores this handshake entirely; 1.1.0 answers it
//! (update through Zwift Companion).
//!
//! Button fields use 0 for pressed and 1 for released (Click/Play), or a
//! bitmap in which a cleared bit means pressed (Ride, Click v2).

use uuid::Uuid;

pub const SERVICE: Uuid = Uuid::from_u128(0x00000001_19ca_4651_86e5_fa29dcdd09d1);
/// Notifications with button state.
pub const ASYNC: Uuid = Uuid::from_u128(0x00000002_19ca_4651_86e5_fa29dcdd09d1);
/// The app writes here (handshake).
pub const SYNC_RX: Uuid = Uuid::from_u128(0x00000003_19ca_4651_86e5_fa29dcdd09d1);
/// Replies to what the app wrote.
pub const SYNC_TX: Uuid = Uuid::from_u128(0x00000004_19ca_4651_86e5_fa29dcdd09d1);
/// The 16-bit Zwift service some controllers advertise instead.
pub const SERVICE_SHORT: Uuid = Uuid::from_u128(0x0000fc82_0000_1000_8000_00805f9b34fb);
/// Bluetooth company identifier of Zwift, in advertised manufacturer data.
pub const ZWIFT_COMPANY_ID: u16 = 0x094a;

pub const RIDE_ON: &[u8] = b"RideOn";

const OP_PLAY: u8 = 0x07;
const OP_BATTERY: u8 = 0x19;
const OP_RIDE: u8 = 0x23;
const OP_CLICK: u8 = 0x37;

/// Ride / Click v2 bitmap: the shift buttons on each side.
const RIDE_SHIFT_UP: u32 = 0x0200 | 0x2000;
const RIDE_SHIFT_DOWN: u32 = 0x0400 | 0x4000;
const RIDE_ALL_RELEASED: u32 = 0xffff_ffff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickEvent {
    /// Harder gear (+).
    ShiftUp,
    /// Easier gear (−).
    ShiftDown,
    Battery(u8),
    /// A button we don't map yet; the value is its bit in the bitmap, or the
    /// protobuf field number for Click/Play messages.
    Other(u32),
}

/// Reads one protobuf message into (field, varint) pairs, skipping
/// length-delimited and fixed fields. Returns None on malformed input.
pub(crate) fn varint_fields(mut b: &[u8]) -> Option<Vec<(u32, u64)>> {
    fn varint(b: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = b.split_first()?;
            *b = rest;
            v |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    while !b.is_empty() {
        let key = varint(&mut b)?;
        let (field, wire) = ((key >> 3) as u32, key & 7);
        match wire {
            0 => out.push((field, varint(&mut b)?)),
            1 => b = b.get(8..)?,
            2 => {
                let len = varint(&mut b)? as usize;
                b = b.get(len..)?;
            }
            5 => b = b.get(4..)?,
            _ => return None,
        }
    }
    Some(out)
}

/// Turns notifications into button presses. Fires once when a button goes
/// down, not again while it is held.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Buttons currently down, as (opcode, id) pairs.
    held: Vec<(u8, u32)>,
}

impl Decoder {
    pub fn feed(&mut self, data: &[u8]) -> Vec<ClickEvent> {
        let Some((&op, body)) = data.split_first() else {
            return Vec::new();
        };
        if data.starts_with(RIDE_ON) {
            return Vec::new();
        }
        let Some(fields) = varint_fields(body) else {
            return Vec::new();
        };
        let get = |n: u32| fields.iter().find(|(f, _)| *f == n).map(|&(_, v)| v);
        let mut down: Vec<(u32, ClickEvent)> = Vec::new();
        match op {
            OP_CLICK => {
                // ClickKeyPadStatus { plus = 1; minus = 2 }, 0 = pressed.
                for (f, ev) in [(1, ClickEvent::ShiftUp), (2, ClickEvent::ShiftDown)] {
                    if get(f) == Some(0) {
                        down.push((f, ev));
                    }
                }
            }
            OP_PLAY => {
                // PlayKeyPadStatus: field 1 says which side (0 = right),
                // field 6 is the shift paddle; 0 = pressed.
                let right = get(1) == Some(0);
                if get(6) == Some(0) {
                    let ev = if right {
                        ClickEvent::ShiftUp
                    } else {
                        ClickEvent::ShiftDown
                    };
                    down.push((6 + u32::from(right) * 100, ev));
                }
                for f in 2..=7 {
                    if f != 6 && get(f) == Some(0) {
                        down.push((f + u32::from(right) * 100, ClickEvent::Other(f)));
                    }
                }
            }
            OP_RIDE => {
                // RideKeyPadStatus { buttonMap = 1 }, cleared bit = pressed.
                let map = get(1).map_or(RIDE_ALL_RELEASED, |v| v as u32);
                for bit in 0..32 {
                    let mask = 1u32 << bit;
                    if map & mask != 0 {
                        continue;
                    }
                    let ev = if RIDE_SHIFT_UP & mask != 0 {
                        ClickEvent::ShiftUp
                    } else if RIDE_SHIFT_DOWN & mask != 0 {
                        ClickEvent::ShiftDown
                    } else {
                        ClickEvent::Other(mask)
                    };
                    down.push((mask, ev));
                }
            }
            OP_BATTERY => {
                // The Click sends its level in field 2 (`19 10 5D` = 93%).
                return get(2)
                    .or(get(1))
                    .map(|v| vec![ClickEvent::Battery(v.min(100) as u8)])
                    .unwrap_or_default();
            }
            _ => return Vec::new(),
        }
        let pressed: Vec<ClickEvent> = down
            .iter()
            .filter(|(id, _)| !self.held.contains(&(op, *id)))
            .map(|&(_, ev)| ev)
            .collect();
        self.held.retain(|(o, _)| *o != op);
        self.held.extend(down.iter().map(|&(id, _)| (op, id)));
        pressed
    }
}

/// Whether advertised manufacturer data belongs to a Zwift controller. The
/// first byte is the device type; trainers (Zwift Hub) are left alone.
pub fn is_controller(manufacturer: &std::collections::HashMap<u16, Vec<u8>>, name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("kickr") || lower.contains("hub") {
        return false;
    }
    if lower.contains("zwift click") || lower.contains("zwift play") || lower.contains("zwift ride")
    {
        return true;
    }
    manufacturer.contains_key(&ZWIFT_COMPANY_ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_plus_fires_once_per_press() {
        let mut d = Decoder::default();
        // plus pressed (field 1 = 0), minus released (field 2 = 1)
        assert_eq!(
            d.feed(&[0x37, 0x08, 0x00, 0x10, 0x01]),
            vec![ClickEvent::ShiftUp]
        );
        // still held: no repeat
        assert!(d.feed(&[0x37, 0x08, 0x00, 0x10, 0x01]).is_empty());
        // released, then minus
        assert!(d.feed(&[0x37, 0x08, 0x01, 0x10, 0x01]).is_empty());
        assert_eq!(
            d.feed(&[0x37, 0x08, 0x01, 0x10, 0x00]),
            vec![ClickEvent::ShiftDown]
        );
    }

    #[test]
    fn ride_bitmap_maps_shift_buttons() {
        let mut d = Decoder::default();
        let msg = |map: u32| {
            let mut v = vec![0x23, 0x08];
            let mut m = u64::from(map);
            loop {
                let b = (m & 0x7f) as u8;
                m >>= 7;
                if m == 0 {
                    v.push(b);
                    break;
                }
                v.push(b | 0x80);
            }
            v
        };
        assert!(d.feed(&msg(RIDE_ALL_RELEASED)).is_empty());
        assert_eq!(d.feed(&msg(!0x2000)), vec![ClickEvent::ShiftUp]);
        assert!(d.feed(&msg(!0x2000)).is_empty());
        assert_eq!(d.feed(&msg(!0x0400)), vec![ClickEvent::ShiftDown]);
        assert_eq!(d.feed(&msg(!0x10)), vec![ClickEvent::Other(0x10)]);
    }

    #[test]
    fn battery_and_noise() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(&[0x19, 0x10, 0x5d]), vec![ClickEvent::Battery(93)]);
        assert_eq!(d.feed(&[0x19, 0x08, 0x55]), vec![ClickEvent::Battery(85)]);
        assert!(d.feed(b"RideOn\x01\x03").is_empty());
        assert!(d.feed(&[0x15]).is_empty());
        assert!(d.feed(&[]).is_empty());
        assert!(d.feed(&[0x37, 0xff]).is_empty());
    }

    #[test]
    fn controller_detection() {
        let mut m = std::collections::HashMap::new();
        assert!(is_controller(&m, "Zwift Click"));
        assert!(!is_controller(&m, "KICKR CORE 1234"));
        m.insert(ZWIFT_COMPANY_ID, vec![0x09]);
        assert!(is_controller(&m, ""));
        assert!(!is_controller(&m, "KICKR CORE 1234"));
    }
}
