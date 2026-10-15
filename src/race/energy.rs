//! Anaerobic work capacity (W′) balance, Skiba's differential model.
//!
//! Every second above critical power (CP) spends W′; below CP it recovers
//! exponentially. This is what makes attacks costly and lets riders crack.

#[derive(Debug, Clone, Copy)]
pub struct Energy {
    /// Critical power in watts (≈ FTP).
    pub cp: f32,
    /// Full W′ in joules.
    pub w_prime: f32,
    /// Remaining W′ in joules.
    pub balance: f32,
}

impl Energy {
    pub fn new(cp: f32, w_prime: f32) -> Self {
        Self {
            cp,
            w_prime,
            balance: w_prime,
        }
    }

    pub fn fraction(&self) -> f32 {
        self.balance / self.w_prime
    }

    pub fn step(&mut self, power: f32, dt: f32) {
        if power > self.cp {
            self.balance -= (power - self.cp) * dt;
        } else {
            self.balance += (self.w_prime - self.balance) * (self.cp - power) / self.w_prime * dt;
        }
        self.balance = self.balance.clamp(0.0, self.w_prime);
    }

    /// Highest power available right now. A full tank allows `peak`; an empty
    /// one leaves the rider just under CP, so efforts fade as W′ drains.
    pub fn max_power(&self, peak: f32) -> f32 {
        if self.balance <= 0.0 {
            return self.cp * 0.97;
        }
        self.cp + (peak - self.cp) * self.fraction().sqrt()
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
    fn empty_tank_caps_power_below_cp() {
        let mut e = Energy::new(300.0, 10_000.0);
        assert_eq!(e.max_power(1200.0), 1200.0);
        e.step(1300.0, 10.0);
        assert_eq!(e.balance, 0.0);
        assert!(e.max_power(1200.0) < 300.0);
    }
}
