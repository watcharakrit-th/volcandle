// SPDX-License-Identifier: MPL-2.0
//
// This file is a Rust port of the Pine Script indicator "CDC ActionZone V3 2020"
// © piriya33, published on TradingView:
//   https://www.tradingview.com/script/rGpAOoLi-CDC-ActionZone-V3-2020/
// Like the original, this file is subject to the terms of the Mozilla Public
// License 2.0. If a copy of the MPL was not distributed with this file, you can
// obtain one at https://mozilla.org/MPL/2.0/.

//! CDC ActionZone V3 2020 (piriya33): an EMA 12/26 crossover system whose
//! "zones" come from where price sits relative to the two EMAs.
//!
//! Ported from the Pine v5 version of the script. Each part below names the
//! Pine variables it reproduces. TradingView-only features (alerts) are left
//! out; drawing (bar colors, lines, fill, signals, ribbon, label) is done by the
//! chart from what [`compute`] returns.

use serde::{Deserialize, Serialize};

use super::{crossover, crossunder, ema, rsi::rsi, sma, stoch, Source};
use crate::data::resample::{self, Timeframe};
use crate::data::Candle;

/// Where the buy/sell ribbon goes (`plotRibsw`, `plotRibbonPos`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ribbon {
    Off,
    Top,
    Bottom,
}

/// The script's inputs, with its defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CdcSettings {
    pub enabled: bool,
    /// `xsrc`, "Source Data".
    pub source: Source,
    /// `xprd1`, "Fast EMA period".
    pub fast: usize,
    /// `xprd2`, "Slow EMA period".
    pub slow: usize,
    /// `xsmooth`, "Smoothing period (1 = no smoothing)".
    pub smoothing: usize,
    /// `fillSW`, "Paint Bar Colors".
    pub paint_bars: bool,
    /// `fastSW`, "Show fast moving average line".
    pub show_fast: bool,
    /// `slowSW`, "Show slow moving average line".
    pub show_slow: bool,
    /// `labelSwitch`, "Turn on assistive text".
    pub assistive_text: bool,
    /// `plotSigsw`, "Plot Buy/Sell Signals?".
    pub signals: bool,
    pub ribbon: Ribbon,
    /// `xfixtf` + `xtf`, "Use Fixed time frame Mode (advanced)".
    pub fixed_tf: Option<Timeframe>,
    /// `plotSig2sw`, "Plot momentum based Buy/Sell Signals?".
    pub momentum: bool,
    /// `plotSig2lv`, "Set signal threshold (higher = stricter)", 0 or 1.
    pub momentum_threshold: u8,
}

impl Default for CdcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            source: Source::Close,
            fast: 12,
            slow: 26,
            smoothing: 1,
            paint_bars: true,
            show_fast: true,
            show_slow: true,
            assistive_text: true,
            signals: true,
            ribbon: Ribbon::Off,
            fixed_tf: None,
            momentum: false,
            momentum_threshold: 1,
        }
    }
}

/// The six color zones (`Green`, `Blue`, `LBlue`, `Red`, `Orange`, `Yellow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Fast EMA above slow, price above fast: buy.
    Green,
    /// Fast below slow, price above both: pre-buy 2 (strong rally).
    Blue,
    /// Fast below slow, price between them: pre-buy 1 (rally).
    LightBlue,
    /// Fast below slow, price below fast: sell.
    Red,
    /// Fast above slow, price below both: pre-sell 2 (strong dip).
    Orange,
    /// Fast above slow, price between them: pre-sell 1 (dip).
    Yellow,
}

impl Zone {
    pub const ALL: [Zone; 6] = [
        Zone::Green,
        Zone::Blue,
        Zone::LightBlue,
        Zone::Red,
        Zone::Orange,
        Zone::Yellow,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Zone::Green => "Green",
            Zone::Blue => "Blue",
            Zone::LightBlue => "Light blue",
            Zone::Red => "Red",
            Zone::Orange => "Orange",
            Zone::Yellow => "Yellow",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Zone::Green => "buy",
            Zone::Blue => "strong rally",
            Zone::LightBlue => "rally",
            Zone::Red => "sell",
            Zone::Orange => "strong dip",
            Zone::Yellow => "dip",
        }
    }
}

/// `bullish` / `bearish`: which came last, a buy condition or a sell condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Bullish,
    Bearish,
    /// Before the first buy *and* sell condition have both happened.
    Sideways,
}

impl Trend {
    pub fn label(self) -> &'static str {
        match self {
            Trend::Bullish => "bullish",
            Trend::Bearish => "bearish",
            Trend::Sideways => "sideways",
        }
    }
}

pub struct Cdc {
    /// `FastMA`, `SlowMA`.
    pub fast: Vec<f64>,
    pub slow: Vec<f64>,
    pub zone: Vec<Option<Zone>>,
    pub trend: Vec<Trend>,
    /// `buy`, `sell`: the first green bar after a bearish trend, the first red
    /// bar after a bullish one.
    pub buy: Vec<bool>,
    pub sell: Vec<bool>,
    /// `storsiBuySig`, `storsiSellSig` (0, 1 or 2); empty unless asked for.
    pub momentum_buy: Vec<u8>,
    pub momentum_sell: Vec<u8>,
    /// The fixed timeframe in use (lines are drawn as steps).
    pub fixed: Option<Timeframe>,
    /// A fixed timeframe was chosen but is not above the chart's, so it is not used.
    pub fixed_ignored: bool,
}

impl Cdc {
    /// Fast EMA above the slow one (`Bull`), below (`Bear`), or neither.
    pub fn bull(&self, i: usize) -> Option<bool> {
        let (f, s) = (self.fast[i], self.slow[i]);
        if f > s {
            Some(true)
        } else if f < s {
            Some(false)
        } else {
            None
        }
    }
}

/// Whether the chart (bars every `interval` seconds) can show `tf` as a fixed
/// higher timeframe.
pub fn fixed_tf_usable(tf: Timeframe, interval: i64) -> bool {
    resample::choices(interval).contains(&tf)
}

pub fn compute(candles: &[Candle], interval: i64, s: &CdcSettings) -> Cdc {
    let n = candles.len();

    // xPrice = ta.ema(xsrc, xsmooth)
    let price = ema(&s.source.series(candles), s.smoothing);

    // FastMA / SlowMA
    let fixed = s.fixed_tf.filter(|&tf| fixed_tf_usable(tf, interval));
    let (fast, slow) = match fixed {
        // ta.ema(xPrice, xprd1), ta.ema(xPrice, xprd2)
        None => (ema(&price, s.fast), ema(&price, s.slow)),
        // ta.ema(request.security(tf, ta.ema(xsrc, prd)[1], lookahead_on), xsmooth):
        // the EMA of the higher timeframe as of its last *finished* bar, so it
        // never changes after the fact.
        Some(tf) => {
            let higher = resample::resample(candles, tf);
            let src = s.source.series(&higher);
            let (hf, hs) = (ema(&src, s.fast), ema(&src, s.slow));
            let (mut f, mut sl) = (Vec::with_capacity(n), Vec::with_capacity(n));
            let mut k = 0;
            for c in candles {
                let bucket = tf.bucket(c.t);
                while k + 1 < higher.len() && higher[k].t < bucket {
                    k += 1;
                }
                if k >= 1 {
                    f.push(hf[k - 1]);
                    sl.push(hs[k - 1]);
                } else {
                    f.push(f64::NAN);
                    sl.push(f64::NAN);
                }
            }
            (ema(&f, s.smoothing), ema(&sl, s.smoothing))
        }
    };

    let mut zone = Vec::with_capacity(n);
    let mut trend = Vec::with_capacity(n);
    let (mut buy, mut sell) = (vec![false; n], vec![false; n]);
    let (mut last_buy, mut last_sell): (Option<usize>, Option<usize>) = (None, None);
    let (mut prev_green, mut prev_red) = (false, false);
    let mut prev_trend = Trend::Sideways;
    for i in 0..n {
        let (p, f, sl) = (price[i], fast[i], slow[i]);
        // Bull / Bear
        let (bull, bear) = (f > sl, f < sl);
        // Green, Blue, LBlue, Red, Orange, Yellow
        let z = if bull && p > f {
            Some(Zone::Green)
        } else if bear && p > f && p > sl {
            Some(Zone::Blue)
        } else if bear && p > f && p < sl {
            Some(Zone::LightBlue)
        } else if bear && p < f {
            Some(Zone::Red)
        } else if bull && p < f && p < sl {
            Some(Zone::Orange)
        } else if bull && p < f && p > sl {
            Some(Zone::Yellow)
        } else {
            None
        };
        let (green, red) = (z == Some(Zone::Green), z == Some(Zone::Red));

        // buycond = Green and Green[1] == 0 (Green[1] is na on the first bar)
        let buycond = green && i > 0 && !prev_green;
        let sellcond = red && i > 0 && !prev_red;
        if buycond {
            last_buy = Some(i);
        }
        if sellcond {
            last_sell = Some(i);
        }
        // bullish = ta.barssince(buycond) < ta.barssince(sellcond); na until both happened
        let t = match (last_buy, last_sell) {
            (Some(b), Some(s)) if b > s => Trend::Bullish,
            (Some(b), Some(s)) if s > b => Trend::Bearish,
            _ => Trend::Sideways,
        };
        // buy = bearish[1] and buycond; sell = bullish[1] and sellcond
        buy[i] = prev_trend == Trend::Bearish && buycond;
        sell[i] = prev_trend == Trend::Bullish && sellcond;

        zone.push(z);
        trend.push(t);
        prev_green = green;
        prev_red = red;
        prev_trend = t;
    }

    let (momentum_buy, momentum_sell) = if s.momentum {
        momentum(candles, &trend)
    } else {
        (Vec::new(), Vec::new())
    };

    Cdc {
        fast,
        slow,
        zone,
        trend,
        buy,
        sell,
        momentum_buy,
        momentum_sell,
        fixed,
        fixed_ignored: s.fixed_tf.is_some() && fixed.is_none(),
    }
}

/// "Momentum Signal using StochRSI": the script's hard-coded StochRSI(14, 14, 3, 3)
/// on close, levels 30/70. Returns (`storsiBuySig`, `storsiSellSig`).
fn momentum(candles: &[Candle], trend: &[Trend]) -> (Vec<u8>, Vec<u8>) {
    const SMOOTH_K: usize = 3;
    const SMOOTH_D: usize = 3;
    const RSI_LEN: usize = 14;
    const STO_LEN: usize = 14;
    const OS_LEVEL: f64 = 30.0;
    const OB_LEVEL: f64 = 70.0;

    // rsi1 = ta.rsi(close, RSIlen); k = ta.sma(ta.stoch(rsi1, rsi1, rsi1, STOlen), smoothK); d = ta.sma(k, smoothD)
    let rsi1 = rsi(&Source::Close.series(candles), RSI_LEN);
    let k = sma(&stoch(&rsi1, &rsi1, &rsi1, STO_LEN), SMOOTH_K);
    let d = sma(&k, SMOOTH_D);

    let mut buys = Vec::with_capacity(candles.len());
    let mut sells = Vec::with_capacity(candles.len());
    for i in 0..candles.len() {
        let up = crossover(&k, &d, i);
        // iff_1 = d > OSlevel and crossover ? 1 : 0; iff_2 = d < OSlevel and crossover ? 2 : iff_1
        let iff_2 = if d[i] < OS_LEVEL && up {
            2
        } else if d[i] > OS_LEVEL && up {
            1
        } else {
            0
        };
        buys.push(if trend[i] == Trend::Bullish { iff_2 } else { 0 });

        let down = crossunder(&k, &d, i);
        let iff_4 = if d[i] > OB_LEVEL && down {
            2
        } else if d[i] < OB_LEVEL && down {
            1
        } else {
            0
        };
        sells.push(if trend[i] == Trend::Bearish { iff_4 } else { 0 });
    }
    (buys, sells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::datetime::{DAY, HOUR};

    fn candles(closes: &[f64], step: i64) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Candle {
                t: i as i64 * step,
                open: c,
                high: c + 1.0,
                low: c - 1.0,
                close: c,
                volume: 1.0,
            })
            .collect()
    }

    /// Down, up, down: zones, trend and signals follow the script's rules.
    #[test]
    fn zones_trend_and_signals() {
        let mut closes: Vec<f64> = (0..40).map(|i| 100.0 - f64::from(i)).collect(); // fall
        closes.extend((0..40).map(|i| 61.0 + 2.0 * f64::from(i))); // rise
        closes.extend((0..40).map(|i| 139.0 - 2.0 * f64::from(i))); // fall
        let c = candles(&closes, DAY);
        let s = CdcSettings::default();
        let cdc = compute(&c, DAY, &s);
        let price = s.source.series(&c);

        for (i, &p) in price.iter().enumerate() {
            // Every zone matches its definition.
            let (f, sl) = (cdc.fast[i], cdc.slow[i]);
            let want = match cdc.zone[i] {
                Some(Zone::Green) => f > sl && p > f,
                Some(Zone::Blue) => f < sl && p > f && p > sl,
                Some(Zone::LightBlue) => f < sl && p > f && p < sl,
                Some(Zone::Red) => f < sl && p < f,
                Some(Zone::Orange) => f > sl && p < f && p < sl,
                Some(Zone::Yellow) => f > sl && p < f && p > sl,
                None => !(f > sl && p > f) && !(f < sl && p < f),
            };
            assert!(want, "bar {i}: {:?} p={p} fast={f} slow={sl}", cdc.zone[i]);
        }
        assert_eq!(cdc.zone[0], None, "both EMAs start at the first price");

        // The first fall makes red bars, but no sell signal: there was no
        // bullish trend before it (barssince(buycond) is na).
        assert!(cdc.zone[10] == Some(Zone::Red));
        assert!(!cdc.sell.iter().take(40).any(|&s| s));
        // The rise turns green: that green bar starts the first bullish trend,
        // still no buy signal (bearish[1] needs an earlier sell condition... and a buy).
        let first_green = (40..80)
            .find(|&i| cdc.zone[i] == Some(Zone::Green))
            .unwrap();
        assert_eq!(cdc.trend[first_green], Trend::Bullish);
        assert!(!cdc.buy[first_green]);
        // The second fall: the first red bar after the bullish trend is a sell.
        let first_red = (80..120).find(|&i| cdc.zone[i] == Some(Zone::Red)).unwrap();
        assert!(cdc.sell[first_red]);
        assert_eq!(cdc.trend[first_red], Trend::Bearish);
        assert_eq!(cdc.sell.iter().filter(|&&s| s).count(), 1);
        // Before the first red bar of the fall the zones step through yellow/orange.
        assert!((80..first_red).any(|i| matches!(cdc.zone[i], Some(Zone::Yellow | Zone::Orange))));
    }

    #[test]
    fn buy_needs_a_bearish_trend_before() {
        // fall, rise, fall, rise: the second rise's first green bar is a buy.
        let mut closes: Vec<f64> = (0..40).map(|i| 100.0 - f64::from(i)).collect();
        for _ in 0..2 {
            closes.extend((0..40).map(|i| 61.0 + 2.0 * f64::from(i)));
            closes.extend((0..40).map(|i| 139.0 - 2.0 * f64::from(i)));
        }
        let cdc = compute(&candles(&closes, DAY), DAY, &CdcSettings::default());
        let buys: Vec<usize> = (0..closes.len()).filter(|&i| cdc.buy[i]).collect();
        let sells: Vec<usize> = (0..closes.len()).filter(|&i| cdc.sell[i]).collect();
        assert_eq!(buys.len(), 1, "{buys:?}");
        assert!(buys[0] > 120 && buys[0] < 160, "{buys:?}");
        assert_eq!(sells.len(), 2, "{sells:?}");
        assert!(cdc.zone[buys[0]] == Some(Zone::Green));
        assert!(cdc.zone[buys[0] - 1] != Some(Zone::Green));
    }

    /// Fixed timeframe: every hourly bar sees the daily EMA of the previous
    /// (finished) day, never the current one.
    #[test]
    fn fixed_timeframe_uses_the_last_finished_bar() {
        let closes: Vec<f64> = (0..24 * 40)
            .map(|i| 100.0 + (f64::from(i) * 0.05).sin() * 10.0)
            .collect();
        let c = candles(&closes, HOUR);
        let s = CdcSettings {
            fixed_tf: Some(Timeframe::Day),
            ..CdcSettings::default()
        };
        let cdc = compute(&c, HOUR, &s);
        assert_eq!(cdc.fixed, Some(Timeframe::Day));
        let daily = resample::resample(&c, Timeframe::Day);
        let daily_fast = ema(&Source::Close.series(&daily), 12);
        for (i, candle) in c.iter().enumerate() {
            let day = (candle.t / DAY) as usize;
            if day == 0 {
                assert!(cdc.fast[i].is_nan());
            } else {
                assert!((cdc.fast[i] - daily_fast[day - 1]).abs() < 1e-9, "bar {i}");
            }
        }
        // A fixed timeframe that is not above the chart's is not used.
        let ignored = compute(&c, DAY, &s);
        assert!(ignored.fixed.is_none() && ignored.fixed_ignored);
    }

    #[test]
    fn momentum_signals_only_with_the_trend() {
        let closes: Vec<f64> = (0..400)
            .map(|i| 100.0 + f64::from(i) * 0.1 + (f64::from(i) * 0.4).sin() * 3.0)
            .collect();
        let s = CdcSettings {
            momentum: true,
            ..CdcSettings::default()
        };
        let cdc = compute(&candles(&closes, DAY), DAY, &s);
        assert_eq!(cdc.momentum_buy.len(), closes.len());
        assert!(cdc.momentum_buy.iter().any(|&v| v > 0));
        for i in 0..closes.len() {
            if cdc.momentum_buy[i] > 0 {
                assert_eq!(cdc.trend[i], Trend::Bullish);
            }
            if cdc.momentum_sell[i] > 0 {
                assert_eq!(cdc.trend[i], Trend::Bearish);
            }
        }
    }
}
