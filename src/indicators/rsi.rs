//! Relative Strength Index, as TradingView's built-in "Relative Strength Index"
//! computes it: Wilder's RMA of the gains and losses, plus an optional
//! "RSI-based MA" (an SMA of the RSI).

use serde::{Deserialize, Serialize};

use super::{rma, sma, Source};
use crate::data::Candle;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RsiSettings {
    pub enabled: bool,
    pub source: Source,
    pub length: usize,
    pub overbought: f64,
    pub oversold: f64,
    /// Draw the RSI-based moving average (an SMA of the RSI).
    pub show_ma: bool,
    pub ma_length: usize,
}

impl Default for RsiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            source: Source::Close,
            length: 14,
            overbought: 70.0,
            oversold: 30.0,
            show_ma: true,
            ma_length: 14,
        }
    }
}

pub struct Rsi {
    pub rsi: Vec<f64>,
    pub ma: Vec<f64>,
}

pub fn compute(candles: &[Candle], s: &RsiSettings) -> Rsi {
    let rsi = rsi(&s.source.series(candles), s.length);
    let ma = sma(&rsi, s.ma_length);
    Rsi { rsi, ma }
}

/// Pine's `ta.rsi`. The first value appears on bar `len` (the first bar has no
/// change). No losses gives 100, no gains 0.
pub fn rsi(src: &[f64], len: usize) -> Vec<f64> {
    let change = |i: usize| {
        if i == 0 {
            f64::NAN
        } else {
            src[i] - src[i - 1]
        }
    };
    // f64::max would turn NaN into 0, so keep na explicitly.
    let up: Vec<f64> = (0..src.len())
        .map(|i| {
            let c = change(i);
            if c.is_nan() {
                c
            } else {
                c.max(0.0)
            }
        })
        .collect();
    let down: Vec<f64> = (0..src.len())
        .map(|i| {
            let c = change(i);
            if c.is_nan() {
                c
            } else {
                (-c).max(0.0)
            }
        })
        .collect();
    let (up, down) = (rma(&up, len), rma(&down, len));
    up.iter()
        .zip(&down)
        .map(|(&u, &d)| {
            if u.is_nan() || d.is_nan() {
                f64::NAN
            } else if d == 0.0 {
                100.0
            } else if u == 0.0 {
                0.0
            } else {
                100.0 - 100.0 / (1.0 + u / d)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from StockCharts' "RSI" ChartSchool article (its
    /// spreadsheet uses these 4-decimal closes; first value checked by hand:
    /// gains 3.3374 / 14, losses 1.3943 / 14, RS 2.3936, RSI 70.53).
    #[test]
    fn wilder_reference_values() {
        let closes = [
            44.3389, 44.0902, 44.1497, 43.6124, 44.3278, 44.8264, 45.0955, 45.4245, 45.8433,
            46.0826, 45.8931, 46.0328, 45.6140, 46.2820, 46.2820, 46.0028, 46.0328, 46.4116,
            46.2222, 45.6439,
        ];
        let r = rsi(&closes, 14);
        assert!(r[..14].iter().all(|v| v.is_nan()));
        let want = [70.53, 66.32, 66.55, 69.41, 66.36, 57.97];
        for (got, want) in r[14..].iter().zip(want) {
            assert!((got - want).abs() < 0.01, "{got} vs {want}");
        }
    }

    #[test]
    fn flat_and_one_way_markets() {
        let up: Vec<f64> = (0..30).map(f64::from).collect();
        assert_eq!(rsi(&up, 14)[20], 100.0);
        let down: Vec<f64> = (0..30).map(|i| 100.0 - f64::from(i)).collect();
        assert_eq!(rsi(&down, 14)[20], 0.0);
        assert_eq!(rsi(&[5.0; 30], 14)[20], 100.0); // no losses (TradingView shows 100)
    }
}
