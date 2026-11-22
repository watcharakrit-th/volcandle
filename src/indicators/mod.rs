//! Technical indicators, computed on the candles the chart shows (after
//! resampling). The building blocks follow TradingView's Pine Script
//! definitions, so values match TradingView once the indicator has warmed up.
//! `NaN` plays the role of Pine's `na` (not enough bars yet): every comparison
//! with it is false, as in Pine.

pub mod cdc;
pub mod rsi;

use serde::{Deserialize, Serialize};

use crate::data::Candle;

pub use cdc::{Cdc, CdcSettings, Ribbon, Trend, Zone};
pub use rsi::{Rsi, RsiSettings};

/// Which price an indicator reads from each candle (Pine's `close`, `hl2`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Close,
    Open,
    High,
    Low,
    Hl2,
    Hlc3,
    Ohlc4,
    Hlcc4,
}

impl Source {
    pub const ALL: [Source; 8] = [
        Source::Close,
        Source::Open,
        Source::High,
        Source::Low,
        Source::Hl2,
        Source::Hlc3,
        Source::Ohlc4,
        Source::Hlcc4,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Source::Close => "close",
            Source::Open => "open",
            Source::High => "high",
            Source::Low => "low",
            Source::Hl2 => "hl2",
            Source::Hlc3 => "hlc3",
            Source::Ohlc4 => "ohlc4",
            Source::Hlcc4 => "hlcc4",
        }
    }

    pub fn of(self, c: &Candle) -> f64 {
        match self {
            Source::Close => c.close,
            Source::Open => c.open,
            Source::High => c.high,
            Source::Low => c.low,
            Source::Hl2 => (c.high + c.low) / 2.0,
            Source::Hlc3 => (c.high + c.low + c.close) / 3.0,
            Source::Ohlc4 => (c.open + c.high + c.low + c.close) / 4.0,
            Source::Hlcc4 => (c.high + c.low + 2.0 * c.close) / 4.0,
        }
    }

    pub fn series(self, candles: &[Candle]) -> Vec<f64> {
        candles.iter().map(|c| self.of(c)).collect()
    }
}

/// Pine's `ta.ema`: starts at the first value, then
/// `alpha * x + (1 - alpha) * previous` with `alpha = 2 / (len + 1)`.
/// After an `na` input it starts again from the next value.
pub fn ema(src: &[f64], len: usize) -> Vec<f64> {
    let alpha = 2.0 / (len.max(1) as f64 + 1.0);
    let mut prev = f64::NAN;
    src.iter()
        .map(|&x| {
            prev = if prev.is_nan() {
                x
            } else {
                alpha * x + (1.0 - alpha) * prev
            };
            prev
        })
        .collect()
}

/// Pine's `ta.sma`: the mean of the last `len` values, `na` until there are
/// `len` of them and while any of them is `na`.
pub fn sma(src: &[f64], len: usize) -> Vec<f64> {
    let len = len.max(1);
    let mut out = Vec::with_capacity(src.len());
    let (mut sum, mut nans) = (0.0, 0usize);
    for (i, &x) in src.iter().enumerate() {
        if x.is_nan() {
            nans += 1;
        } else {
            sum += x;
        }
        if i >= len {
            let old = src[i - len];
            if old.is_nan() {
                nans -= 1;
            } else {
                sum -= old;
            }
        }
        // Recompute now and then so the running sum cannot drift.
        if nans == 0 && i + 1 >= len && i % 4096 == 0 {
            sum = src[i + 1 - len..=i].iter().sum();
        }
        out.push(if i + 1 >= len && nans == 0 {
            sum / len as f64
        } else {
            f64::NAN
        });
    }
    out
}

/// Pine's `ta.rma` (Wilder's smoothing): starts with the SMA of the first
/// `len` values, then `alpha * x + (1 - alpha) * previous` with `alpha = 1 / len`.
pub fn rma(src: &[f64], len: usize) -> Vec<f64> {
    let alpha = 1.0 / len.max(1) as f64;
    let seed = sma(src, len);
    let mut prev = f64::NAN;
    src.iter()
        .zip(&seed)
        .map(|(&x, &s)| {
            prev = if prev.is_nan() {
                s
            } else {
                alpha * x + (1.0 - alpha) * prev
            };
            prev
        })
        .collect()
}

/// Highest (or lowest) of the last `len` values; `na` until there are `len`
/// values and while any of them is `na`.
fn window_extreme(src: &[f64], len: usize, highest: bool) -> Vec<f64> {
    let len = len.max(1);
    (0..src.len())
        .map(|i| {
            if i + 1 < len {
                return f64::NAN;
            }
            let w = &src[i + 1 - len..=i];
            if w.iter().any(|v| v.is_nan()) {
                f64::NAN
            } else if highest {
                w.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            } else {
                w.iter().copied().fold(f64::INFINITY, f64::min)
            }
        })
        .collect()
}

/// Pine's `ta.stoch(source, high, low, len)`:
/// `100 * (source - lowest(low)) / (highest(high) - lowest(low))`, `na` when the
/// range is zero.
pub fn stoch(src: &[f64], high: &[f64], low: &[f64], len: usize) -> Vec<f64> {
    let (hh, ll) = (
        window_extreme(high, len, true),
        window_extreme(low, len, false),
    );
    src.iter()
        .zip(hh.iter().zip(&ll))
        .map(|(&x, (&h, &l))| {
            let range = h - l;
            if range.is_nan() || range == 0.0 {
                f64::NAN
            } else {
                100.0 * (x - l) / range
            }
        })
        .collect()
}

/// Pine's `ta.crossover(a, b)` at bar `i`: `a` is above `b` now and was not on
/// the bar before.
pub fn crossover(a: &[f64], b: &[f64], i: usize) -> bool {
    i > 0 && a[i] > b[i] && a[i - 1] <= b[i - 1]
}

/// Pine's `ta.crossunder(a, b)` at bar `i`.
pub fn crossunder(a: &[f64], b: &[f64], i: usize) -> bool {
    i > 0 && a[i] < b[i] && a[i - 1] >= b[i - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| (x.is_nan() && y.is_nan()) || (x - y).abs() < 1e-9)
    }

    #[test]
    fn ema_follows_pine() {
        let e = ema(&[1.0, 2.0, 3.0], 2); // alpha = 2/3
        assert!(close(
            &e,
            &[
                1.0,
                1.0 + 2.0 / 3.0,
                1.0 + 2.0 / 3.0 + 2.0 / 3.0 * (3.0 - 1.0 - 2.0 / 3.0)
            ]
        ));
        // Length 1 changes nothing (CDC's default smoothing).
        assert!(close(&ema(&[5.0, 7.0, 2.0], 1), &[5.0, 7.0, 2.0]));
        // na input: na out, then a fresh start.
        assert!(close(
            &ema(&[f64::NAN, 4.0, f64::NAN, 6.0], 3),
            &[f64::NAN, 4.0, f64::NAN, 6.0]
        ));
    }

    #[test]
    fn sma_and_rma() {
        let nan = f64::NAN;
        assert!(close(&sma(&[1.0, 2.0, 3.0, 4.0], 2), &[nan, 1.5, 2.5, 3.5]));
        assert!(close(&sma(&[nan, 2.0, 4.0, 6.0], 2), &[nan, nan, 3.0, 5.0]));
        // RMA seeds with the SMA, then (prev * (len - 1) + x) / len.
        assert!(close(
            &rma(&[2.0, 4.0, 6.0, 8.0], 2),
            &[nan, 3.0, 4.5, 6.25]
        ));
        let long: Vec<f64> = (0..20_000).map(|i| (i % 7) as f64 * 1e6 + 0.1).collect();
        let s = sma(&long, 7);
        assert!((s[19_999] - (0..7).map(|k| k as f64 * 1e6 + 0.1).sum::<f64>() / 7.0).abs() < 1e-6);
    }

    #[test]
    fn stoch_and_crosses() {
        let x = [1.0, 3.0, 2.0, 2.0, 2.0];
        let s = stoch(&x, &x, &x, 3);
        assert!(close(&s, &[f64::NAN, f64::NAN, 50.0, 0.0, f64::NAN]));
        let a = [1.0, 2.0, 3.0, 1.0];
        let b = [2.0, 2.0, 2.0, 2.0];
        assert!(!crossover(&a, &b, 1)); // touching is not crossing
        assert!(crossover(&a, &b, 2));
        assert!(crossunder(&a, &b, 3));
        assert!(!crossover(&[f64::NAN, 3.0], &[2.0, 2.0], 1)); // na before: no cross
    }
}
