//! Calendar math and tolerant date/time parsing, without a date crate.
//!
//! Times are `i64` seconds since 1970-01-01 00:00 on the *clock used in the
//! file*. Time-zone suffixes written in text dates (`Z`, `+07:00`) are ignored,
//! so an exchange's local session times stay exactly as they were exported.
//! Unix timestamps are UTC by definition.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

pub const MINUTE: i64 = 60;
pub const HOUR: i64 = 3_600;
pub const DAY: i64 = 86_400;

pub const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Years outside this range are treated as parse errors.
const MIN_YEAR: i64 = 1800;
const MAX_YEAR: i64 = 2200;

/// How to read an all-numeric date such as `03/04/2024`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateOrder {
    /// 3 April 2024 (most of the world, including Thailand).
    #[default]
    Dmy,
    /// March 4, 2024 (USA).
    Mdy,
}

impl DateOrder {
    pub fn label(self) -> &'static str {
        match self {
            DateOrder::Dmy => "D/M/Y",
            DateOrder::Mdy => "M/D/Y",
        }
    }
}

// ---------------------------------------------------------------------------
// Calendar math (proleptic Gregorian, Howard Hinnant's algorithms)

/// Days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Builds a timestamp from calendar fields, validating every field.
pub fn timestamp(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Option<i64> {
    if !(MIN_YEAR..=MAX_YEAR).contains(&y)
        || !(1..=12).contains(&m)
        || d == 0
        || d > days_in_month(y, m)
        || hh > 24
        || mm > 59
        || ss > 60
        || (hh == 24 && (mm, ss) != (0, 0))
    {
        return None;
    }
    let secs = i64::from(hh) * HOUR + i64::from(mm) * MINUTE + i64::from(ss.min(59));
    Some(days_from_civil(y, m, d) * DAY + secs)
}

/// Broken-down form of a timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Monday … 6 = Sunday.
    pub weekday: u32,
}

pub fn parts(t: i64) -> Parts {
    let days = t.div_euclid(DAY);
    let secs = t.rem_euclid(DAY);
    let (year, month, day) = civil_from_days(days);
    Parts {
        year,
        month,
        day,
        hour: (secs / HOUR) as u32,
        minute: (secs % HOUR / MINUTE) as u32,
        second: (secs % MINUTE) as u32,
        weekday: (days + 3).rem_euclid(7) as u32,
    }
}

// ---------------------------------------------------------------------------
// Formatting

/// `2024-01-15`
pub fn format_date(t: i64) -> String {
    let p = parts(t);
    format!("{:04}-{:02}-{:02}", p.year, p.month, p.day)
}

/// `2024-01-15 09:30` (seconds only when non-zero, time only when not midnight).
pub fn format_datetime(t: i64) -> String {
    let p = parts(t);
    let mut s = format_date(t);
    if (p.hour, p.minute, p.second) != (0, 0, 0) {
        s += &format!(" {:02}:{:02}", p.hour, p.minute);
        if p.second != 0 {
            s += &format!(":{:02}", p.second);
        }
    }
    s
}

/// `Mon 15 Jan 2024 09:30`, the crosshair label on the time axis.
pub fn format_long(t: i64, with_time: bool) -> String {
    let p = parts(t);
    let mut s = format!(
        "{} {} {} {}",
        WEEKDAY_ABBR[p.weekday as usize],
        p.day,
        MONTH_ABBR[p.month as usize - 1],
        p.year
    );
    if with_time {
        s += &format!("  {:02}:{:02}", p.hour, p.minute);
        if p.second != 0 {
            s += &format!(":{:02}", p.second);
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Numeric date encodings

/// A Unix timestamp in seconds, milliseconds, microseconds or nanoseconds; the
/// unit is chosen per value from its magnitude, so files that switch units
/// (Binance spot data moved from ms to µs in 2025) still parse.
pub fn from_epoch(v: f64) -> Option<i64> {
    if !v.is_finite() {
        return None;
    }
    let a = v.abs();
    let secs = if a >= 1e17 {
        v / 1e9
    } else if a >= 1e14 {
        v / 1e6
    } else if a >= 1e11 {
        v / 1e3
    } else {
        v
    };
    let t = secs.floor() as i64;
    let lo = days_from_civil(MIN_YEAR, 1, 1) * DAY;
    let hi = days_from_civil(MAX_YEAR, 12, 31) * DAY;
    (lo..=hi).contains(&t).then_some(t)
}

/// An Excel serial date (days since 1899-12-30, fraction = time of day).
pub fn from_excel_serial(v: f64) -> Option<i64> {
    if !(61.0..2_958_466.0).contains(&v) {
        return None;
    }
    let days = v.floor();
    let secs = ((v - days) * DAY as f64).round() as i64;
    Some((days as i64 - 25_569) * DAY + secs)
}

/// `YYYYMMDD`, `YYYYMMDDHHMM` or `YYYYMMDDHHMMSS` (digits only).
pub fn from_compact(s: &str) -> Option<i64> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let (hh, mm, ss) = match s.len() {
        8 => (0, 0, 0),
        12 => (n(8..10)?, n(10..12)?, 0),
        14 => (n(8..10)?, n(10..12)?, n(12..14)?),
        _ => return None,
    };
    let y = normalize_year(i64::from(n(0..4)?), 4);
    timestamp(y, n(4..6)?, n(6..8)?, hh, mm, ss)
}

/// Two-digit years pivot at 1970; years from 2400 are Buddhist Era (Thai) years.
fn normalize_year(y: i64, digits: usize) -> i64 {
    if digits <= 2 {
        if y < 70 {
            2000 + y
        } else {
            1900 + y
        }
    } else if y >= 2400 {
        y - 543
    } else {
        y
    }
}

// ---------------------------------------------------------------------------
// Text dates

#[derive(Debug, Clone, PartialEq)]
enum Tok<'a> {
    Num(&'a str),
    Word(String),
    Sep(char),
}

const THAI_MONTHS: [(&str, &str); 24] = [
    ("มกราคม", "jan"),
    ("กุมภาพันธ์", "feb"),
    ("มีนาคม", "mar"),
    ("เมษายน", "apr"),
    ("พฤษภาคม", "may"),
    ("มิถุนายน", "jun"),
    ("กรกฎาคม", "jul"),
    ("สิงหาคม", "aug"),
    ("กันยายน", "sep"),
    ("ตุลาคม", "oct"),
    ("พฤศจิกายน", "nov"),
    ("ธันวาคม", "dec"),
    // Abbreviations, matched without their final dot so "ม.ค" and "ม.ค." both work.
    ("ม.ค", "jan"),
    ("ก.พ", "feb"),
    ("มี.ค", "mar"),
    ("เม.ย", "apr"),
    ("พ.ค", "may"),
    ("มิ.ย", "jun"),
    ("ก.ค", "jul"),
    ("ส.ค", "aug"),
    ("ก.ย", "sep"),
    ("ต.ค", "oct"),
    ("พ.ย", "nov"),
    ("ธ.ค", "dec"),
];

/// Thai digits to ASCII and Thai month names to English abbreviations.
fn normalize(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return Cow::Borrowed(s);
    }
    let mut out: String = s
        .chars()
        .map(|c| match c {
            '\u{0E50}'..='\u{0E59}' => char::from(b'0' + (c as u32 - 0x0E50) as u8),
            _ => c,
        })
        .collect();
    for (thai, en) in THAI_MONTHS {
        if out.contains(thai) {
            out = out.replace(thai, &format!(" {en} "));
        }
    }
    Cow::Owned(out)
}

fn tokenize(s: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c.is_ascii_digit() {
            let mut end = i + 1;
            while let Some(&(j, d)) = it.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                end = j + 1;
                it.next();
            }
            out.push(Tok::Num(&s[i..end]));
        } else if c.is_alphabetic() {
            let mut w: String = c.to_lowercase().collect();
            while let Some(&(_, d)) = it.peek() {
                if !d.is_alphabetic() {
                    break;
                }
                w.extend(d.to_lowercase());
                it.next();
            }
            out.push(Tok::Word(w));
        } else if c.is_whitespace() {
            while it.peek().is_some_and(|&(_, d)| d.is_whitespace()) {
                it.next();
            }
            out.push(Tok::Sep(' '));
        } else {
            out.push(Tok::Sep(c));
        }
    }
    out
}

fn month_from_word(w: &str) -> Option<u32> {
    const FULL: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    if w.len() >= 3 {
        if let Some(i) = FULL.iter().position(|f| f.starts_with(w)) {
            return Some(i as u32 + 1);
        }
    }
    // A few common non-English abbreviations (German, French, Spanish, Dutch).
    let m = match w {
        "mär" | "märz" | "mrz" | "mars" => 3,
        "mai" | "mei" => 5,
        "juin" => 6,
        "juil" => 7,
        "okt" => 10,
        "dez" | "déc" | "dic" => 12,
        "janv" | "ene" => 1,
        "févr" | "fevr" | "fév" => 2,
        "avr" | "abr" => 4,
        "août" | "aout" | "ago" => 8,
        "sept" | "set" => 9,
        "mrt" => 3,
        _ => return None,
    };
    Some(m)
}

/// Words that may appear in a date without changing it.
fn is_filler(w: &str) -> bool {
    const FILLERS: [&str; 27] = [
        "t",
        "z",
        "utc",
        "gmt",
        "at",
        "of",
        "st",
        "nd",
        "rd",
        "th",
        "mon",
        "tue",
        "tues",
        "wed",
        "thu",
        "thur",
        "thurs",
        "fri",
        "sat",
        "sun",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    FILLERS.contains(&w)
}

/// Finds `H:MM` and returns its token index.
fn find_clock(toks: &[Tok]) -> Option<usize> {
    (0..toks.len().saturating_sub(2)).find(|&k| {
        matches!(
            (&toks[k], &toks[k + 1], &toks[k + 2]),
            (Tok::Num(h), Tok::Sep(':'), Tok::Num(m)) if h.len() <= 2 && m.len() == 2
        )
    })
}

/// Parses `H:MM[:SS[.fff]] [am|pm]` starting at `toks[0]`; returns
/// (seconds of day, index of the first token after the clock).
fn parse_clock(toks: &[Tok]) -> Option<(i64, usize)> {
    let num = |t: &Tok| match t {
        Tok::Num(n) => n.parse::<u32>().ok(),
        _ => None,
    };
    let mut h = num(&toks[0])?;
    let m = num(&toks[2])?;
    let mut s = 0;
    let mut k = 3;
    if let (Some(Tok::Sep(':')), Some(Tok::Num(ss))) = (toks.get(3), toks.get(4)) {
        if ss.len() == 2 {
            s = ss.parse().ok()?;
            k = 5;
        }
    }
    if let (Some(Tok::Sep('.' | ',')), Some(Tok::Num(_))) = (toks.get(k), toks.get(k + 1)) {
        k += 2; // fractional seconds
    }
    let mut j = k;
    if matches!(toks.get(j), Some(Tok::Sep(' '))) {
        j += 1;
    }
    match toks.get(j) {
        Some(Tok::Word(w)) if w == "pm" || w == "p" => {
            if h > 12 {
                return None;
            }
            if h < 12 {
                h += 12;
            }
            k = j + 1;
        }
        Some(Tok::Word(w)) if w == "am" || w == "a" => {
            if h > 12 {
                return None;
            }
            if h == 12 {
                h = 0;
            }
            k = j + 1;
        }
        _ => {}
    }
    if h > 24 || m > 59 || s > 60 || (h == 24 && (m, s) != (0, 0)) {
        return None;
    }
    Some((
        i64::from(h) * HOUR + i64::from(m) * MINUTE + i64::from(s.min(59)),
        k,
    ))
}

/// Parses a written date or date-time, e.g. `2024-01-02`, `2024-01-02T09:30:00Z`,
/// `02/01/2024 9:30 PM`, `2024.01.02`, `Jan 2, 2024`, `02-Jan-24`, `20240102 093000`,
/// `02 ม.ค. 2567` (Thai, Buddhist Era). `order` resolves `03/04/2024`.
pub fn parse_text(s: &str, order: DateOrder) -> Option<i64> {
    let s = s.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    let norm = normalize(s);
    let toks = tokenize(&norm);

    let mut tod = 0;
    let mut date_toks: &[Tok] = &toks;
    if let Some(k) = find_clock(&toks) {
        let (secs, end) = parse_clock(&toks[k..])?;
        tod = secs;
        date_toks = &toks[..k];
        if !date_toks.iter().any(|t| matches!(t, Tok::Num(_))) {
            date_toks = &toks[k + end..]; // "09:30 02/01/2024"
        }
    }

    let mut nums: Vec<&str> = Vec::with_capacity(4);
    let mut month = None;
    for t in date_toks {
        match t {
            Tok::Num(n) => nums.push(n),
            Tok::Word(w) => {
                if let Some(m) = month_from_word(w) {
                    if month.replace(m).is_some() {
                        return None;
                    }
                } else if !is_filler(w) {
                    return None;
                }
            }
            Tok::Sep(_) => {}
        }
    }
    let n = |s: &str| s.parse::<i64>().ok();
    let year = |s: &str| n(s).map(|y| normalize_year(y, s.len()));

    let (y, m, d) = match (month, nums.as_slice()) {
        (Some(m), [a]) if a.len() == 4 => (year(a)?, m, 1),
        (Some(m), [a, b]) => {
            if a.len() >= 3 || n(a)? > 31 {
                (year(a)?, m, n(b)?)
            } else {
                (year(b)?, m, n(a)?)
            }
        }
        (None, [a]) => return from_compact(a).map(|t| t + tod),
        (None, [a, b]) if a.len() == 8 && (b.len() == 4 || b.len() == 6) => {
            let day = from_compact(a)?;
            let clock = parse_time_of_day(b)?;
            return Some(day + clock);
        }
        (None, [a, b]) if a.len() == 4 && b.len() <= 2 => (year(a)?, n(b)? as u32, 1),
        (None, [a, b, c]) => {
            if a.len() == 4 {
                (year(a)?, n(b)? as u32, n(c)?)
            } else {
                match order {
                    DateOrder::Dmy => (year(c)?, n(b)? as u32, n(a)?),
                    DateOrder::Mdy => (year(c)?, n(a)? as u32, n(b)?),
                }
            }
        }
        _ => return None,
    };
    let d = u32::try_from(d).ok()?;
    Some(timestamp(y, m, d, 0, 0, 0)? + tod)
}

/// A separate time-of-day column: `09:30`, `9:30:15`, `09:30:00.000`, `9:30 PM`,
/// `0930`, `093000`.
pub fn parse_time_of_day(s: &str) -> Option<i64> {
    let s = s.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let v: i64 = s.parse().ok()?;
        let (h, m, sec) = match s.len() {
            1..=4 => (v / 100, v % 100, 0),
            5 | 6 => (v / 10_000, v / 100 % 100, v % 100),
            _ => return None,
        };
        if h > 24 || m > 59 || sec > 59 {
            return None;
        }
        return Some(h * HOUR + m * MINUTE + sec);
    }
    let toks = tokenize(s);
    if find_clock(&toks) != Some(0) {
        return None;
    }
    parse_clock(&toks).map(|(t, _)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        timestamp(y, m, d, hh, mm, ss).unwrap()
    }

    #[test]
    fn civil_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for z in [-800_000, -1, 0, 1, 19_000, 20_000, 60_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(parts(0).weekday, 3); // Thursday
        assert_eq!(parts(ts(2024, 1, 15, 9, 30, 0)).weekday, 0); // Monday
    }

    #[test]
    fn epoch_units() {
        let t = ts(2024, 1, 1, 0, 0, 0);
        assert_eq!(from_epoch(1_704_067_200.0), Some(t));
        assert_eq!(from_epoch(1_704_067_200_000.0), Some(t));
        assert_eq!(from_epoch(1_704_067_200_000_000.0), Some(t));
        assert_eq!(from_epoch(1_704_067_200_000_000_000.0), Some(t));
        assert_eq!(from_epoch(1_704_067_199_999.0), Some(t - 1));
    }

    #[test]
    fn excel_and_compact() {
        assert_eq!(from_excel_serial(45_292.0), Some(ts(2024, 1, 1, 0, 0, 0)));
        assert_eq!(from_excel_serial(45_292.5), Some(ts(2024, 1, 1, 12, 0, 0)));
        assert_eq!(from_compact("20240102"), Some(ts(2024, 1, 2, 0, 0, 0)));
        assert_eq!(from_compact("202401020930"), Some(ts(2024, 1, 2, 9, 30, 0)));
        assert_eq!(
            from_compact("20240102093015"),
            Some(ts(2024, 1, 2, 9, 30, 15))
        );
        assert_eq!(from_compact("25670102"), Some(ts(2024, 1, 2, 0, 0, 0)));
        assert_eq!(from_compact("20241302"), None);
        assert_eq!(from_compact("2024010"), None);
    }

    #[test]
    fn text_formats() {
        let d = ts(2024, 1, 2, 0, 0, 0);
        let dt = ts(2024, 1, 2, 9, 30, 0);
        let cases: &[(&str, i64)] = &[
            ("2024-01-02", d),
            ("2024/01/02", d),
            ("2024.01.02", d),
            ("2024-01-02T09:30:00Z", dt),
            ("2024-01-02T09:30:00.000+07:00", dt),
            ("2024-01-02 09:30", dt),
            ("02/01/2024", d),
            ("02.01.2024 09:30:00", dt),
            ("Jan 2, 2024", d),
            ("Jan 02, 2024 09:30", dt),
            ("2 January 2024", d),
            ("02-Jan-24", d),
            ("Tue, 02 Jan 2024 09:30:00 GMT", dt),
            ("20240102 093000", dt),
            ("20240102T0930", dt),
            ("02 ม.ค. 2567", d),
            ("2 มกราคม 2567", d),
            ("๒/๑/๒๕๖๗", d),
            ("02/01/2567 09:30", dt),
            ("09:30 02/01/2024", dt),
            ("1/2/2024 9:30:00 AM", ts(2024, 2, 1, 9, 30, 0)),
            ("02.01.2024 9:30 pm", ts(2024, 1, 2, 21, 30, 0)),
        ];
        for (s, want) in cases {
            assert_eq!(parse_text(s, DateOrder::Dmy), Some(*want), "{s}");
        }
        assert_eq!(parse_text("01/02/2024", DateOrder::Mdy), Some(d));
        assert_eq!(parse_text("13/02/2024", DateOrder::Mdy), None);
        for bad in [
            "",
            "abc",
            "42283.58",
            "2024-13-01",
            "31/02/2024",
            "Price",
            "12:3",
        ] {
            assert_eq!(parse_text(bad, DateOrder::Dmy), None, "{bad}");
        }
    }

    #[test]
    fn time_of_day() {
        assert_eq!(parse_time_of_day("09:30"), Some(9 * HOUR + 30 * MINUTE));
        assert_eq!(
            parse_time_of_day("9:30:15"),
            Some(9 * HOUR + 30 * MINUTE + 15)
        );
        assert_eq!(parse_time_of_day("00:00:00.000"), Some(0));
        assert_eq!(parse_time_of_day("0930"), Some(9 * HOUR + 30 * MINUTE));
        assert_eq!(
            parse_time_of_day("093015"),
            Some(9 * HOUR + 30 * MINUTE + 15)
        );
        assert_eq!(parse_time_of_day("12:00 AM"), Some(0));
        assert_eq!(parse_time_of_day("1:05 PM"), Some(13 * HOUR + 5 * MINUTE));
        assert_eq!(parse_time_of_day("2024-01-02"), None);
        assert_eq!(parse_time_of_day("9970"), None);
    }

    #[test]
    fn formatting() {
        let t = ts(2024, 1, 15, 9, 30, 0);
        assert_eq!(format_date(t), "2024-01-15");
        assert_eq!(format_datetime(t), "2024-01-15 09:30");
        assert_eq!(format_datetime(ts(2024, 1, 15, 0, 0, 0)), "2024-01-15");
        assert_eq!(format_long(t, true), "Mon 15 Jan 2024  09:30");
        assert_eq!(format_long(t, false), "Mon 15 Jan 2024");
    }
}
