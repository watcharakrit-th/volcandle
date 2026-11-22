//! Tick selection and label formatting for the price and time axes.

use crate::data::datetime::{self, DAY, HOUR, MINUTE};

/// A 1-2-2.5-5 step giving at most about `max_ticks` ticks over `range`.
pub fn nice_step(range: f64, max_ticks: f64) -> f64 {
    let raw = range / max_ticks.max(1.0);
    let mag = 10f64.powi(raw.log10().floor() as i32);
    let norm = raw / mag;
    let nice = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Decimals needed to write multiples of `step` exactly.
pub fn step_decimals(step: f64) -> usize {
    (0..=10)
        .find(|&d| {
            let x = step * 10f64.powi(d as i32);
            (x - x.round()).abs() < 1e-6 * x.abs().max(1.0)
        })
        .unwrap_or(10)
}

/// Price ticks between `lo` and `hi` for an axis `height` pixels tall.
/// Returns the ticks and the decimals their labels need.
pub fn price_ticks(lo: f64, hi: f64, log: bool, height: f32) -> (Vec<f64>, usize) {
    if !lo.is_finite() || !hi.is_finite() || hi <= lo || height < 10.0 {
        return (Vec::new(), 0);
    }
    let max_ticks = f64::from(height / 34.0).max(2.0);
    if log && lo > 0.0 && hi / lo > 3.0 {
        return (log_ticks(lo, hi, height), 0);
    }
    let step = nice_step(hi - lo, max_ticks);
    let (k0, k1) = ((lo / step).ceil() as i64, (hi / step).floor() as i64);
    let ticks = (k0..=k1).take(200).map(|k| k as f64 * step).collect();
    (ticks, step_decimals(step))
}

/// 1-2-5 style ticks on a log axis, as dense as fits.
fn log_ticks(lo: f64, hi: f64, height: f32) -> Vec<f64> {
    let span = (hi / lo).ln();
    let px = |a: f64, b: f64| (b / a).ln() / span * f64::from(height);
    let (e0, e1) = (lo.log10().floor() as i32, hi.log10().ceil() as i32);
    let sets: [&[f64]; 4] = [
        &[1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
        &[1.0, 2.0, 3.0, 5.0, 7.0],
        &[1.0, 2.0, 5.0],
        &[1.0],
    ];
    for set in sets {
        let ticks: Vec<f64> = (e0..=e1)
            .flat_map(|e| set.iter().map(move |m| m * 10f64.powi(e)))
            .filter(|v| (lo..=hi).contains(v))
            .collect();
        if ticks.windows(2).all(|w| px(w[0], w[1]) >= 28.0) {
            return ticks;
        }
    }
    // Decades are still too dense: keep every k-th decade.
    let k = ((f64::from(e1 - e0) * 28.0 / f64::from(height)).ceil() as i32).max(1);
    (e0..=e1)
        .filter(|e| e.rem_euclid(k) == 0)
        .map(|e| 10f64.powi(e))
        .filter(|v| (lo..=hi).contains(v))
        .collect()
}

pub fn format_price(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

/// `1.23M`, `45.6K`, `0.5`.
pub fn format_volume(v: f64) -> String {
    let a = v.abs();
    let (x, suffix) = if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        (v, "")
    };
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}{suffix}")
}

/// Spacing between time-axis labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeStep {
    Minutes(i64),
    Hours(i64),
    Days(i64),
    Weeks,
    Months(i64),
    Years(i64),
}

const TIME_STEPS: [TimeStep; 24] = [
    TimeStep::Minutes(1),
    TimeStep::Minutes(2),
    TimeStep::Minutes(5),
    TimeStep::Minutes(10),
    TimeStep::Minutes(15),
    TimeStep::Minutes(30),
    TimeStep::Hours(1),
    TimeStep::Hours(2),
    TimeStep::Hours(3),
    TimeStep::Hours(4),
    TimeStep::Hours(6),
    TimeStep::Hours(12),
    TimeStep::Days(1),
    TimeStep::Weeks,
    TimeStep::Months(1),
    TimeStep::Months(2),
    TimeStep::Months(3),
    TimeStep::Months(6),
    TimeStep::Years(1),
    TimeStep::Years(2),
    TimeStep::Years(5),
    TimeStep::Years(10),
    TimeStep::Years(25),
    TimeStep::Years(100),
];

impl TimeStep {
    fn approx_secs(self) -> i64 {
        match self {
            TimeStep::Minutes(m) => m * MINUTE,
            TimeStep::Hours(h) => h * HOUR,
            TimeStep::Days(d) => d * DAY,
            TimeStep::Weeks => 7 * DAY,
            TimeStep::Months(m) => m * 30 * DAY,
            TimeStep::Years(y) => y * 365 * DAY,
        }
    }

    /// Labels go where this key changes between neighbouring bars.
    pub fn key(self, t: i64, p: &datetime::Parts) -> i64 {
        match self {
            TimeStep::Minutes(m) => t.div_euclid(m * MINUTE),
            TimeStep::Hours(h) => t.div_euclid(h * HOUR),
            TimeStep::Days(d) => t.div_euclid(DAY).div_euclid(d),
            TimeStep::Weeks => (t.div_euclid(DAY) + 3).div_euclid(7),
            TimeStep::Months(m) => (p.year * 12 + i64::from(p.month) - 1).div_euclid(m),
            TimeStep::Years(y) => p.year.div_euclid(y),
        }
    }

    pub fn pick(secs_per_px: f64, min_px: f64, interval: i64) -> TimeStep {
        let want = (secs_per_px * min_px).max(interval as f64);
        TIME_STEPS
            .into_iter()
            .find(|s| s.approx_secs() as f64 >= want)
            .unwrap_or(TimeStep::Years(100))
    }
}

/// Label for a bar that starts a new period: the year when the year changes,
/// else the month, else the day number, else the time. Higher levels matter more.
pub fn boundary_label(prev: &datetime::Parts, cur: &datetime::Parts) -> (String, u8) {
    if cur.year != prev.year {
        (cur.year.to_string(), 4)
    } else if cur.month != prev.month {
        (datetime::MONTH_ABBR[cur.month as usize - 1].to_string(), 3)
    } else if cur.day != prev.day {
        (cur.day.to_string(), 2)
    } else {
        (format!("{:02}:{:02}", cur.hour, cur.minute), 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(nice_step(100.0, 10.0), 10.0);
        assert_eq!(nice_step(7.0, 10.0), 1.0);
        assert!((nice_step(0.023, 10.0) - 0.0025).abs() < 1e-12);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(500.0), 0);
        assert_eq!(step_decimals(0.0005), 4);
    }

    #[test]
    fn linear_ticks() {
        let (t, d) = price_ticks(41_870.0, 42_960.0, false, 340.0);
        assert_eq!(d, 0);
        assert!(t.len() >= 4 && t.len() <= 11, "{t:?}");
        assert!(t.iter().all(|v| v % 100.0 == 0.0), "{t:?}");
    }

    #[test]
    fn log_axis() {
        let (t, _) = price_ticks(150.0, 69_000.0, true, 600.0);
        assert!(t.contains(&1000.0) && t.contains(&10_000.0), "{t:?}");
        assert!(t.iter().all(|v| (150.0..=69_000.0).contains(v)));
    }

    #[test]
    fn volumes() {
        assert_eq!(format_volume(1_234_567.0), "1.23M");
        assert_eq!(format_volume(45_600.0), "45.6K");
        assert_eq!(format_volume(2_000_000_000.0), "2B");
        assert_eq!(format_volume(0.5), "0.5");
        assert_eq!(format_volume(0.0), "0");
    }

    #[test]
    fn time_step_choice() {
        // 1 hour per pixel, labels at least 90 px apart → 4h is too small, 12h? no: ≥ 90h → Weeks.
        assert_eq!(TimeStep::pick(3600.0, 90.0, 3600), TimeStep::Weeks);
        assert_eq!(TimeStep::pick(60.0, 90.0, 60), TimeStep::Hours(2));
        assert_eq!(TimeStep::pick(1.0, 90.0, DAY), TimeStep::Days(1));
    }
}
