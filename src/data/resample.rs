//! Interval detection and timeframe aggregation (1h → 4h, 1D → 1W, …).

use serde::{Deserialize, Serialize};

use super::datetime::{self, DAY, HOUR, MINUTE};
use super::Candle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Timeframe {
    /// The bars exactly as in the file.
    Native,
    Minutes(u32),
    Day,
    /// Weeks start on Monday.
    Week,
    Months(u32),
}

impl Timeframe {
    pub const PRESETS: [Timeframe; 15] = [
        Timeframe::Minutes(1),
        Timeframe::Minutes(3),
        Timeframe::Minutes(5),
        Timeframe::Minutes(15),
        Timeframe::Minutes(30),
        Timeframe::Minutes(60),
        Timeframe::Minutes(120),
        Timeframe::Minutes(240),
        Timeframe::Minutes(360),
        Timeframe::Minutes(720),
        Timeframe::Day,
        Timeframe::Week,
        Timeframe::Months(1),
        Timeframe::Months(3),
        Timeframe::Months(12),
    ];

    /// Approximate length in seconds.
    pub fn seconds(self) -> i64 {
        match self {
            Timeframe::Native => 0,
            Timeframe::Minutes(m) => i64::from(m) * MINUTE,
            Timeframe::Day => DAY,
            Timeframe::Week => 7 * DAY,
            Timeframe::Months(m) => i64::from(m) * 30 * DAY,
        }
    }

    pub fn label(self) -> String {
        match self {
            Timeframe::Native => "Raw".into(),
            Timeframe::Minutes(m) if m % 60 == 0 => format!("{}h", m / 60),
            Timeframe::Minutes(m) => format!("{m}m"),
            Timeframe::Day => "1D".into(),
            Timeframe::Week => "1W".into(),
            Timeframe::Months(m) => format!("{m}M"),
        }
    }

    /// Start of the bar that contains `t`.
    pub fn bucket(self, t: i64) -> i64 {
        match self {
            Timeframe::Native => t,
            Timeframe::Minutes(m) => {
                let len = i64::from(m.max(1)) * MINUTE;
                t - t.rem_euclid(len)
            }
            Timeframe::Day => t - t.rem_euclid(DAY),
            Timeframe::Week => {
                let day = t.div_euclid(DAY);
                (day - (day + 3).rem_euclid(7)) * DAY
            }
            Timeframe::Months(m) => {
                let p = datetime::parts(t);
                let index = p.year * 12 + i64::from(p.month) - 1;
                let start = index - index.rem_euclid(i64::from(m.max(1)));
                datetime::days_from_civil(start.div_euclid(12), start.rem_euclid(12) as u32 + 1, 1)
                    * DAY
            }
        }
    }
}

/// Timeframes that make sense for data with bars every `interval` seconds.
pub fn choices(interval: i64) -> Vec<Timeframe> {
    Timeframe::PRESETS
        .into_iter()
        .filter(|tf| {
            let s = tf.seconds();
            match tf {
                Timeframe::Minutes(_) => interval > 0 && s > interval && s % interval == 0,
                _ => s > interval,
            }
        })
        .collect()
}

/// Aggregates sorted candles: open = first, high = max, low = min,
/// close = last, volume = sum.
pub fn resample(src: &[Candle], tf: Timeframe) -> Vec<Candle> {
    if tf == Timeframe::Native {
        return src.to_vec();
    }
    let mut out: Vec<Candle> = Vec::new();
    for c in src {
        let start = tf.bucket(c.t);
        match out.last_mut() {
            Some(b) if b.t == start => {
                b.high = b.high.max(c.high);
                b.low = b.low.min(c.low);
                b.close = c.close;
                b.volume += c.volume;
            }
            _ => out.push(Candle { t: start, ..*c }),
        }
    }
    out
}

/// The typical spacing of the bars, in seconds (median of the gaps, snapped to
/// calendar months, quarters and years). 0 if unknown.
pub fn detect_interval(candles: &[Candle]) -> i64 {
    let mut gaps: Vec<i64> = candles
        .windows(2)
        .map(|w| w[1].t - w[0].t)
        .filter(|d| *d > 0)
        .take(20_000)
        .collect();
    if gaps.is_empty() {
        return 0;
    }
    gaps.sort_unstable();
    let m = gaps[gaps.len() / 2];
    if (27 * DAY..=32 * DAY).contains(&m) {
        30 * DAY
    } else if (88 * DAY..=93 * DAY).contains(&m) {
        90 * DAY
    } else if (360 * DAY..=370 * DAY).contains(&m) {
        360 * DAY
    } else {
        m
    }
}

/// `1m`, `4h`, `1D`, `1W`, `1M`, `3M`, `12M`, `15s`.
pub fn interval_label(secs: i64) -> String {
    match secs {
        s if s <= 0 => "?".into(),
        s if s == 30 * DAY => "1M".into(),
        s if s == 90 * DAY => "3M".into(),
        s if s == 360 * DAY => "12M".into(),
        s if s % (7 * DAY) == 0 => format!("{}W", s / (7 * DAY)),
        s if s % DAY == 0 => format!("{}D", s / DAY),
        s if s % HOUR == 0 => format!("{}h", s / HOUR),
        s if s % MINUTE == 0 => format!("{}m", s / MINUTE),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(t: i64, o: f64, h: f64, l: f64, cl: f64, v: f64) -> Candle {
        Candle {
            t,
            open: o,
            high: h,
            low: l,
            close: cl,
            volume: v,
        }
    }

    #[test]
    fn buckets() {
        let t = datetime::timestamp(2024, 1, 17, 13, 45, 10).unwrap(); // Wednesday
        assert_eq!(
            Timeframe::Minutes(240).bucket(t),
            datetime::timestamp(2024, 1, 17, 12, 0, 0).unwrap()
        );
        assert_eq!(
            Timeframe::Day.bucket(t),
            datetime::timestamp(2024, 1, 17, 0, 0, 0).unwrap()
        );
        assert_eq!(
            Timeframe::Week.bucket(t),
            datetime::timestamp(2024, 1, 15, 0, 0, 0).unwrap()
        );
        assert_eq!(
            Timeframe::Months(1).bucket(t),
            datetime::timestamp(2024, 1, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            Timeframe::Months(3).bucket(datetime::timestamp(2024, 5, 20, 0, 0, 0).unwrap()),
            datetime::timestamp(2024, 4, 1, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn aggregate() {
        let src = [
            c(0, 10.0, 12.0, 9.0, 11.0, 1.0),
            c(3600, 11.0, 15.0, 10.0, 14.0, 2.0),
            c(7200, 14.0, 14.5, 8.0, 9.0, 3.0),
            c(4 * 3600, 9.0, 10.0, 8.5, 9.5, 4.0),
        ];
        let out = resample(&src, Timeframe::Minutes(240));
        assert_eq!(
            out,
            vec![
                c(0, 10.0, 15.0, 8.0, 9.0, 6.0),
                c(4 * 3600, 9.0, 10.0, 8.5, 9.5, 4.0)
            ]
        );
    }

    #[test]
    fn intervals() {
        let hourly: Vec<Candle> = (0..10)
            .map(|i| c(i * HOUR, 1.0, 1.0, 1.0, 1.0, 1.0))
            .collect();
        assert_eq!(detect_interval(&hourly), HOUR);
        assert_eq!(interval_label(HOUR), "1h");
        assert_eq!(interval_label(DAY), "1D");
        assert_eq!(interval_label(7 * DAY), "1W");
        assert_eq!(interval_label(15 * MINUTE), "15m");
        assert_eq!(choices(HOUR).first(), Some(&Timeframe::Minutes(120)));
        assert_eq!(
            choices(DAY),
            vec![
                Timeframe::Week,
                Timeframe::Months(1),
                Timeframe::Months(3),
                Timeframe::Months(12)
            ]
        );
        assert!(!choices(3 * MINUTE).contains(&Timeframe::Minutes(5)));
    }
}
