//! Anaerobic work capacity (W′) balance, Skiba's differential model.
//!
//! Every second above critical power (CP) spends W′; below CP it recovers
//! exponentially. This is what makes attacks costly and lets riders crack.
//!
//! Over hours CP itself sags with the work done ("durability", Maunder et
//! al. 2021; Spragg et al. 2023): little in the first hour and a half, then
//! a few percent per hour, so the end of a long race favours the fresh.

/// CP holds until this much work (in hours at CP) is done…
const FADE_FROM_H: f32 = 1.5;
/// …then drops this share per further hour of it…
const FADE_PER_H: f32 = 0.04;
/// …down to at most this much.
const MAX_FADE: f32 = 0.12;

#[derive(Debug, Clone, Copy)]
pub struct Energy {
    /// Critical power in watts (≈ FTP), as it is now.
    pub cp: f32,
    /// Critical power fresh.
    pub fresh_cp: f32,
    /// Work done so far in joules.
    pub work: f32,
    /// Full W′ in joules.
    pub w_prime: f32,
    /// Remaining W′ in joules.
    pub balance: f32,
}

impl Energy {
    pub fn new(cp: f32, w_prime: f32) -> Self {
        Self {
            cp,
            fresh_cp: cp,
            work: 0.0,
            w_prime,
            balance: w_prime,
        }
    }

    pub fn fraction(&self) -> f32 {
        self.balance / self.w_prime
    }

    pub fn step(&mut self, power: f32, dt: f32) {
        self.work += power.max(0.0) * dt;
        // Work in hours ridden at fresh CP.
        let hours = self.work / (self.fresh_cp * 3600.0);
        let fade = ((hours - FADE_FROM_H).max(0.0) * FADE_PER_H).min(MAX_FADE);
        self.cp = self.fresh_cp * (1.0 - fade);
        if power > self.cp {
            self.balance -= (power - self.cp) * dt;
        } else {
            self.balance += (self.w_prime - self.balance) * (self.cp - power) / self.w_prime * dt;
        }
        self.balance = self.balance.clamp(0.0, self.w_prime);
    }

    /// Highest power available right now. A full tank allows `peak`; an empty
    /// one leaves the rider just under CP, so efforts fade as W′ drains. A
    /// short sprint draws mostly on fast-recovering stores, so a half-empty
    /// tank still allows most of it.
    pub fn max_power(&self, peak: f32) -> f32 {
        if self.balance <= 0.0 {
            return self.cp * 0.97;
        }
        self.cp + (peak - self.cp) * self.fraction().powf(0.35)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efforts_above_cp_drain_and_recover_below() {
        let mut e = Energy::new(300.0, 20_000.0);
        for _ in 0..60 {
            e.step(500.0, 1.0); // one minute at +200 W
        }
        assert_eq!(e.balance, 20_000.0 - 12_000.0);
        let drained = e.balance;
        for _ in 0..120 {
            e.step(150.0, 1.0);
        }
        assert!(e.balance > drained + 4_000.0, "{}", e.balance);
        assert!(e.balance < 20_000.0);
    }

    #[test]
    fn cp_sags_late_in_a_long_race() {
        let mut e = Energy::new(300.0, 20_000.0);
        // An hour at 80%: still fresh.
        for _ in 0..3600 {
            e.step(240.0, 1.0);
        }
        assert_eq!(e.cp, 300.0);
        // Three more: a few percent down.
        for _ in 0..3 * 3600 {
            e.step(240.0, 1.0);
        }
        assert!(e.cp < 290.0 && e.cp > 275.0, "{}", e.cp);
    }

    #[test]
    fn empty_tank_caps_power_below_cp() {
        let mut e = Energy::new(300.0, 10_000.0);
        assert_eq!(e.max_power(1200.0), 1200.0);
        e.step(1300.0, 10.0);
        assert_eq!(e.balance, 0.0);
        assert!(e.max_power(1200.0) < 300.0);
    }

    #[test]
    fn half_a_tank_still_allows_most_of_a_sprint() {
        let mut e = Energy::new(300.0, 20_000.0);
        e.balance = 10_000.0;
        let kick = e.max_power(1500.0);
        assert!(kick > 1200.0 && kick < 1500.0, "{kick}");
        e.balance = 1_000.0;
        assert!(e.max_power(1500.0) < 900.0);
    }
}
