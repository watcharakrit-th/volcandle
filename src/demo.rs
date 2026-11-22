//! Synthetic demo prices (a random walk whose volume rises with volatility),
//! shown when no file is open. Clearly labeled as not real market data.

use crate::data::datetime::{self, DAY, HOUR};
use crate::data::{resample, Candle, Series};

/// Small deterministic PRNG (xorshift64*), so the demo looks the same every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    fn normal(&mut self) -> f64 {
        let u = self.f64().max(1e-12);
        let v = self.f64();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

fn round_to(v: f64, decimals: i32) -> f64 {
    let m = 10f64.powi(decimals);
    (v * m).round() / m
}

/// 150 days of hourly bars.
pub fn series() -> Series {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let start = datetime::days_from_civil(2025, 1, 6) * DAY;
    let mut price: f64 = 64_000.0;
    let mut regime = 1.0;
    let mut candles = Vec::with_capacity(24 * 150);
    for i in 0..24 * 150 {
        if rng.f64() < 0.012 {
            regime = 0.4 + rng.f64() * 2.2;
        }
        let hour = (i % 24) as f64;
        let session = 1.0 + 0.8 * (-(hour - 14.0).powi(2) / 12.0).exp();
        let sigma = 0.0035 * regime * session.sqrt();
        let shock = if rng.f64() < 0.01 {
            rng.normal() * 4.0
        } else {
            0.0
        };
        let ret = sigma * (rng.normal() + shock) + 0.00003;
        let open = price;
        let close = open * (1.0 + ret);
        let high = open.max(close) * (1.0 + sigma * rng.f64().powi(2) * 1.4);
        let low = open.min(close) * (1.0 - sigma * rng.f64().powi(2) * 1.4);
        let burst = if rng.f64() < 0.03 {
            2.0 + rng.f64() * 3.0
        } else {
            1.0
        };
        let volume =
            180.0 * session * regime * (1.0 + 90.0 * ret.abs()) * (0.55 + 0.9 * rng.f64()) * burst;
        candles.push(Candle {
            t: start + i as i64 * HOUR,
            open: round_to(open, 2),
            high: round_to(high, 2),
            low: round_to(low, 2),
            close: round_to(close, 2),
            volume: round_to(volume, 4),
        });
        price = close;
    }
    Series {
        name: "DEMO".into(),
        interval: resample::detect_interval(&candles),
        decimals: 2,
        has_volume: true,
        format: "Built-in demo".into(),
        columns: String::new(),
        notes: vec!["synthetic prices, not market data: open a file to chart real data".into()],
        candles,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_is_consistent() {
        let s = super::series();
        assert_eq!(s.candles.len(), 3600);
        assert_eq!(s.interval, super::HOUR);
        assert!(s.candles.iter().all(|c| c.low <= c.open.min(c.close)
            && c.high >= c.open.max(c.close)
            && c.volume > 0.0));
    }
}
