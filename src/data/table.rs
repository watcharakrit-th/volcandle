//! Column detection: turns rows of text cells (from CSV, JSON or a spreadsheet)
//! into candles.
//!
//! With a header row, columns are matched by name in several languages
//! (`Open`, `<OPEN>`, `ราคาเปิด`, `Eröffnung`, `1. open`, …). Without one, the
//! layout is inferred from the cells: the first date-like column is the time,
//! the next four numeric columns are open/high/low/close (high and low are
//! recognized by their values), and the next numeric column is the volume.

use super::datetime::{self, DateOrder};
use super::number;
use super::Candle;

/// Read access to one row of cells.
pub trait Row {
    fn cell(&self, i: usize) -> &str;
    fn width(&self) -> usize;
    fn is_blank(&self) -> bool {
        (0..self.width()).all(|i| self.cell(i).trim().is_empty())
    }
}

impl Row for Vec<String> {
    fn cell(&self, i: usize) -> &str {
        self.get(i).map_or("", String::as_str)
    }
    fn width(&self) -> usize {
        self.len()
    }
}

impl Row for csv::StringRecord {
    fn cell(&self, i: usize) -> &str {
        self.get(i).unwrap_or("")
    }
    fn width(&self) -> usize {
        self.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Time,
    Open,
    High,
    Low,
    Close,
    Volume,
    Symbol,
}

/// How the time column is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeFmt {
    /// Unix time in s, ms, µs or ns.
    Epoch,
    /// `YYYYMMDD[HHMM[SS]]`.
    Compact,
    /// Excel serial day number.
    Excel,
    /// Written dates; the flag says whether the D/M/Y vs M/D/Y order mattered.
    Text(DateOrder, bool),
}

impl TimeFmt {
    fn label(self) -> String {
        match self {
            TimeFmt::Epoch => "Unix time".into(),
            TimeFmt::Compact => "YYYYMMDD".into(),
            TimeFmt::Excel => "Excel serial".into(),
            TimeFmt::Text(order, true) => format!("text, {}", order.label()),
            TimeFmt::Text(_, false) => "text".into(),
        }
    }
}

/// Where everything is in a table.
#[derive(Clone, Debug)]
pub struct Layout {
    /// Index of the header row, if any. Data starts after it.
    pub header: Option<usize>,
    pub date: usize,
    /// Separate time-of-day column (MetaTrader exports).
    pub tod: Option<usize>,
    pub time_fmt: TimeFmt,
    pub open: Option<usize>,
    pub high: Option<usize>,
    pub low: Option<usize>,
    pub close: usize,
    /// Volume columns in order of preference; the first one that is not all
    /// zeros wins (MetaTrader's `<VOL>` is often 0 while `<TICKVOL>` is not).
    pub volume: Vec<usize>,
    pub symbol: Option<usize>,
    pub decimal_comma: bool,
    /// Column names from the header, or `column N`.
    pub names: Vec<String>,
}

impl Layout {
    pub fn data_start(&self) -> usize {
        self.header.map_or(0, |h| h + 1)
    }

    fn name(&self, i: usize) -> String {
        self.names
            .get(i)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("column {}", i + 1))
    }

    /// One line describing which column became what.
    pub fn describe(&self, volume_col: Option<usize>) -> String {
        let mut time = format!("time = {}", self.name(self.date));
        if let Some(t) = self.tod {
            time += &format!(" + {}", self.name(t));
        }
        time += &format!(" ({})", self.time_fmt.label());
        let mut parts = vec![time];
        for (label, col) in [("open", self.open), ("high", self.high), ("low", self.low)] {
            if let Some(c) = col {
                parts.push(format!("{label} = {}", self.name(c)));
            }
        }
        parts.push(format!("close = {}", self.name(self.close)));
        parts.push(match volume_col {
            Some(v) => format!("volume = {}", self.name(v)),
            None => "no volume".into(),
        });
        if self.decimal_comma {
            parts.push("decimal comma".into());
        }
        parts.join(" · ")
    }
}

// ---------------------------------------------------------------------------
// Header names

/// Lowercases and replaces punctuation with spaces, keeping non-ASCII letters
/// (Thai, accents): `<TICKVOL>` → `tickvol`, `Vol.` → `vol`, `open_time` → `open time`.
fn norm_header(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_ascii() || c.is_whitespace() {
                ' '
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Maps a normalized header to a role and a priority (lower is better).
fn role_of(h: &str) -> Option<(Role, u8)> {
    if h.is_empty() {
        return None;
    }
    let toks: Vec<&str> = h.split(' ').collect();
    let has = |w: &str| toks.contains(&w);
    let any = |ws: &[&str]| ws.iter().any(|w| has(w));
    let contains = |ws: &[&str]| ws.iter().any(|w| h.contains(w));

    // Columns that look like prices or volumes but are something else.
    let timeish = any(&["time", "timestamp", "period", "date"]);
    if any(&[
        "interest", "oi", "taker", "change", "chg", "trades", "count", "spread", "avg",
    ]) || any(&[
        "average",
        "vwap",
        "ignore",
        "number",
        "dividend",
        "split",
        "coefficient",
    ]) || contains(&["เปลี่ยนแปลง", "เฉลี่ย"])
        || (timeish && any(&["close", "end"]))
        || h == "closetime"
    {
        return None;
    }
    if any(&["symbol", "ticker", "code", "instrument", "pair", "name"])
        || contains(&["ชื่อหุ้น", "หลักทรัพย์"])
    {
        return Some((Role::Symbol, 0));
    }

    // Time
    let date_words = [
        "date",
        "datetime",
        "timestamp",
        "dtyyyymmdd",
        "day",
        "period",
        "data",
        "datum",
        "fecha",
        "วันที่",
        "日期",
        "날짜",
    ];
    if any(&date_words) || h.starts_with("open time") || h == "opentime" || h.contains("วันที่")
    {
        return Some((Role::Time, 0));
    }
    if any(&[
        "time",
        "t",
        "ts",
        "unix",
        "dt",
        "zeit",
        "hora",
        "heure",
        "เวลา",
        "start",
        "时间",
    ]) || h == "gmt time"
    {
        return Some((Role::Time, 1));
    }

    // Prices. Order matters: Thai "เปิด" (open) contains "ปิด" (close).
    if any(&[
        "open",
        "o",
        "opening",
        "first",
        "eröffnung",
        "apertura",
        "otwarcie",
        "ouverture",
    ]) || any(&["abertura", "eroeffnung", "开盘"])
        || h.contains("เปิด")
    {
        return Some((Role::Open, 0));
    }
    if any(&[
        "high", "h", "max", "maximum", "highest", "hoch", "máximo", "maximo", "haut",
    ]) || any(&["najwyzszy", "最高"])
        || h.contains("สูงสุด")
    {
        return Some((Role::High, 0));
    }
    if any(&[
        "low", "l", "min", "minimum", "lowest", "tief", "mínimo", "minimo", "bas",
    ]) || any(&["najnizszy", "最低"])
        || h.contains("ต่ำสุด")
    {
        return Some((Role::Low, 0));
    }
    if has("adj") || h.contains("adjusted") {
        return Some((Role::Close, 3));
    }
    if any(&[
        "close",
        "c",
        "closing",
        "schluss",
        "cierre",
        "zamkniecie",
        "clôture",
        "cloture",
    ]) || any(&["fechamento", "收盘"])
        || h.contains("ปิด")
    {
        return Some((Role::Close, 0));
    }
    if any(&[
        "last",
        "settle",
        "settlement",
        "ultimo",
        "último",
        "letzter",
    ]) {
        return Some((Role::Close, 1));
    }
    if any(&["price", "precio", "preis", "kurs", "prix", "rate"]) || h.contains("ราคา") {
        return Some((Role::Close, 2));
    }

    // Volume
    if any(&["value", "turnover", "notional", "amount"]) || h.contains("มูลค่า") || has("quote")
    {
        return Some((Role::Volume, 3));
    }
    let volume_words = [
        "volume", "vol", "v", "volumes", "volumen", "wolumen", "qty", "quantity",
    ];
    if any(&volume_words) || h.contains("ปริมาณ") || h.contains("成交量") {
        if h.contains("tick") || h == "tickvol" {
            return Some((Role::Volume, 2));
        }
        let exact = matches!(
            h,
            "volume" | "vol" | "v" | "real volume" | "realvol" | "volumen"
        );
        return Some((Role::Volume, if exact { 0 } else { 1 }));
    }
    if h == "tickvol" || h == "tick volume" || h == "tick vol" {
        return Some((Role::Volume, 2));
    }
    None
}

// ---------------------------------------------------------------------------
// Inference

const SAMPLE: usize = 300;
/// A column must parse in at least this share of sample rows.
const MIN_SHARE: f64 = 0.8;

fn share<T>(values: &[&str], f: impl Fn(&str) -> Option<T>) -> f64 {
    let filled: Vec<&str> = values
        .iter()
        .copied()
        .filter(|s| !s.trim().is_empty())
        .collect();
    if filled.is_empty() {
        return 0.0;
    }
    filled.iter().filter(|s| f(s).is_some()).count() as f64 / filled.len() as f64
}

fn parse_number_any(s: &str) -> Option<f64> {
    number::parse(s, false).or_else(|| number::parse(s, true))
}

fn is_numeric_column(values: &[&str]) -> bool {
    share(values, parse_number_any) >= MIN_SHARE
}

/// Decides how a column of sample cells encodes time, if it does.
pub fn infer_time_fmt(
    values: &[&str],
    order: Option<DateOrder>,
    allow_excel: bool,
) -> Option<TimeFmt> {
    let vals: Vec<&str> = values
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if vals.is_empty() {
        return None;
    }
    if vals.iter().all(|s| number::is_plain(s)) {
        let ok = |f: &dyn Fn(&str) -> Option<i64>| share(&vals, f) >= MIN_SHARE;
        if vals.iter().all(|s| s.bytes().all(|b| b.is_ascii_digit())) && ok(&datetime::from_compact)
        {
            return Some(TimeFmt::Compact);
        }
        let nums: Vec<f64> = vals.iter().filter_map(|s| s.parse().ok()).collect();
        if nums.iter().all(|v| v.abs() >= 1e8)
            && ok(&|s| s.parse().ok().and_then(datetime::from_epoch))
        {
            return Some(TimeFmt::Epoch);
        }
        if allow_excel && nums.iter().all(|v| (10_000.0..100_000.0).contains(v)) {
            return Some(TimeFmt::Excel);
        }
        return None;
    }
    let ambiguous = vals.iter().any(|s| {
        datetime::parse_text(s, DateOrder::Dmy) != datetime::parse_text(s, DateOrder::Mdy)
    });
    let order = order.unwrap_or_else(|| infer_order(&vals));
    (share(&vals, |s| datetime::parse_text(s, order)) >= MIN_SHARE)
        .then_some(TimeFmt::Text(order, ambiguous))
}

/// D/M/Y or M/D/Y? Whichever parses more values; on a tie, whichever gives the
/// smoother sequence (consecutive rows are close in time).
fn infer_order(vals: &[&str]) -> DateOrder {
    let parse = |o| {
        vals.iter()
            .map(|s| datetime::parse_text(s, o))
            .collect::<Vec<_>>()
    };
    let (dmy, mdy) = (parse(DateOrder::Dmy), parse(DateOrder::Mdy));
    let ok = |v: &[Option<i64>]| v.iter().flatten().count();
    if ok(&dmy) != ok(&mdy) {
        return if ok(&dmy) > ok(&mdy) {
            DateOrder::Dmy
        } else {
            DateOrder::Mdy
        };
    }
    let travel = |v: &[Option<i64>]| {
        let ts: Vec<i64> = v.iter().flatten().copied().collect();
        ts.windows(2)
            .map(|w| (w[1] - w[0]).unsigned_abs())
            .sum::<u64>()
    };
    if travel(&mdy) < travel(&dmy) {
        DateOrder::Mdy
    } else {
        DateOrder::Dmy
    }
}

fn parse_time(fmt: TimeFmt, s: &str) -> Option<i64> {
    let s = s.trim().trim_matches('"').trim();
    match fmt {
        TimeFmt::Epoch => s.parse().ok().and_then(datetime::from_epoch),
        TimeFmt::Compact => datetime::from_compact(s),
        TimeFmt::Excel => s.parse().ok().and_then(datetime::from_excel_serial),
        TimeFmt::Text(order, _) => datetime::parse_text(s, order),
    }
}

fn looks_like_tod(values: &[&str]) -> bool {
    if share(values, datetime::parse_time_of_day) < 0.9 {
        return false;
    }
    let vals: Vec<&str> = values
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if vals.iter().all(|s| s.contains(':')) {
        return true;
    }
    // Digits only (`093000`): must be fixed width with leading zeros, so that a
    // price column like `1234` is not mistaken for 12:34.
    let len = vals.first().map_or(0, |s| s.len());
    vals.iter()
        .all(|s| s.len() == len && s.bytes().all(|b| b.is_ascii_digit()))
        && (len == 4 || len == 6)
        && vals.iter().any(|s| s.starts_with('0'))
}

/// Whether every parsed value of a date column is at midnight.
fn dates_only(values: &[&str], fmt: TimeFmt) -> bool {
    values
        .iter()
        .filter_map(|s| parse_time(fmt, s))
        .all(|t| t.rem_euclid(datetime::DAY) == 0)
}

/// Infers the layout of a table from its first rows.
pub fn infer(rows: &[Vec<String>], order: Option<DateOrder>) -> Result<Layout, String> {
    let rows = &rows[..rows.len().min(SAMPLE + 40)];
    if rows.iter().all(|r| r.is_blank()) {
        return Err("The table is empty.".into());
    }
    let header = rows.iter().take(40).position(|r| {
        let roles: Vec<Role> = r
            .iter()
            .filter_map(|c| role_of(&norm_header(c)))
            .map(|x| x.0)
            .collect();
        let prices = roles
            .iter()
            .filter(|r| matches!(r, Role::Open | Role::High | Role::Low | Role::Close))
            .count();
        prices >= 2 || (prices >= 1 && roles.contains(&Role::Time))
    });
    match header {
        Some(h) => with_header(rows, h, order),
        None => headerless(rows, order),
    }
}

fn column<'a>(data: &[&'a Vec<String>], j: usize) -> Vec<&'a str> {
    data.iter().map(|r| r.cell(j)).collect()
}

fn with_header(rows: &[Vec<String>], h: usize, order: Option<DateOrder>) -> Result<Layout, String> {
    let names: Vec<String> = rows[h]
        .iter()
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .collect();
    let roles: Vec<Option<(Role, u8)>> = names.iter().map(|n| role_of(&norm_header(n))).collect();
    let data: Vec<&Vec<String>> = rows[h + 1..]
        .iter()
        .filter(|r| !r.is_blank())
        .take(SAMPLE)
        .collect();
    if data.is_empty() {
        return Err("The file has a header but no data rows.".into());
    }
    let ranked = |role: Role| -> Vec<usize> {
        let mut cols: Vec<(u8, usize)> = roles
            .iter()
            .enumerate()
            .filter_map(|(j, r)| match r {
                Some((rr, p)) if *rr == role => Some((*p, j)),
                _ => None,
            })
            .collect();
        cols.sort();
        cols.into_iter().map(|(_, j)| j).collect()
    };
    let numeric = |j: &usize| is_numeric_column(&column(&data, *j));
    // Volume may have gaps ("-", "null"), which simply count as zero.
    let has_volume = |j: &usize| share(&column(&data, *j), parse_number_any) >= 0.5;

    // Time: the best named column that parses; a second one that holds only a
    // clock (MetaTrader `<DATE>` + `<TIME>`) is added to it.
    let time_cols = ranked(Role::Time);
    let mut time = None;
    for &c in &time_cols {
        let vals = column(&data, c);
        if let Some(fmt) = infer_time_fmt(&vals, order, true) {
            let tod = if dates_only(&vals, fmt) {
                time_cols
                    .iter()
                    .copied()
                    .find(|&t| t != c && looks_like_tod(&column(&data, t)))
            } else {
                None
            };
            time = Some((c, tod, fmt));
            break;
        }
    }
    // Unnamed time column: try the first columns by content.
    if time.is_none() {
        for c in 0..names.len().min(3) {
            if let Some(fmt) = infer_time_fmt(&column(&data, c), order, false) {
                time = Some((c, None, fmt));
                break;
            }
        }
    }
    let Some((date, tod, time_fmt)) = time else {
        let example = time_cols
            .first()
            .and_then(|&c| {
                data.iter()
                    .map(|r| r.cell(c).trim())
                    .find(|s| !s.is_empty())
            })
            .map(str::to_string);
        return Err(match example {
            Some(e) => format!("Could not read the dates (for example \"{e}\")."),
            None => format!("No date/time column found. Columns: {}", names.join(", ")),
        });
    };

    let pick = |role: Role| ranked(role).into_iter().find(numeric);
    let (open, high, low) = (pick(Role::Open), pick(Role::High), pick(Role::Low));
    let close = pick(Role::Close)
        .or(open)
        .ok_or_else(|| format!("No close/price column found. Columns: {}", names.join(", ")))?;
    let volume: Vec<usize> = ranked(Role::Volume)
        .into_iter()
        .filter(has_volume)
        .collect();
    let symbol = ranked(Role::Symbol).into_iter().next();
    let price_cols: Vec<usize> = [open, high, low, Some(close)]
        .into_iter()
        .flatten()
        .chain(volume.iter().copied())
        .collect();
    let decimal_comma =
        number::detect_decimal_comma(price_cols.iter().flat_map(|&j| column(&data, j)));

    Ok(Layout {
        header: Some(h),
        date,
        tod,
        time_fmt,
        open,
        high,
        low,
        close,
        volume,
        symbol,
        decimal_comma,
        names,
    })
}

fn headerless(rows: &[Vec<String>], order: Option<DateOrder>) -> Result<Layout, String> {
    // The most common row width; rows of other widths (titles, notes) are ignored.
    let mut counts = std::collections::BTreeMap::new();
    for r in rows.iter().filter(|r| r.width() >= 2) {
        *counts.entry(r.width()).or_insert(0usize) += 1;
    }
    let first_row = || {
        rows.iter()
            .find(|r| !r.is_blank())
            .map(|r| r.join(", "))
            .unwrap_or_default()
    };
    let width = counts
        .iter()
        .max_by_key(|(w, n)| (**n, **w))
        .map(|(w, _)| *w)
        .ok_or_else(|| {
            format!(
                "No header row and no date column found (first row: \"{}\").",
                first_row()
            )
        })?;
    let data: Vec<&Vec<String>> = rows
        .iter()
        .filter(|r| r.width() == width)
        .take(SAMPLE)
        .collect();

    let mut time = None;
    for c in 0..width.min(3) {
        if let Some(fmt) = infer_time_fmt(&column(&data, c), order, false) {
            time = Some((c, fmt));
            break;
        }
    }
    let (date, time_fmt) = time.ok_or_else(|| {
        format!(
            "No header row and no date column found (first row: \"{}\").",
            first_row()
        )
    })?;
    let tod = (date + 1 < width
        && dates_only(&column(&data, date), time_fmt)
        && looks_like_tod(&column(&data, date + 1)))
    .then_some(date + 1);

    let first_value = tod.unwrap_or(date) + 1;
    let numeric: Vec<usize> = (first_value..width)
        .filter(|&j| is_numeric_column(&column(&data, j)))
        .collect();
    let symbol = (0..width).find(|&j| {
        j != date && Some(j) != tod && {
            let vals = column(&data, j);
            !is_numeric_column(&vals)
                && vals.iter().any(|s| !s.trim().is_empty())
                && vals.windows(2).all(|w| w[0] == w[1])
        }
    });

    let (open, high, low, close, volume) = match numeric.as_slice() {
        [] => return Err("No price columns found next to the dates.".into()),
        [c] => (None, None, None, *c, vec![]),
        [c, v] => (None, None, None, *c, vec![*v]),
        [h, l, c] => (None, Some(*h), Some(*l), *c, vec![]),
        [a, b, c, d, rest @ ..] => {
            let (o, h, l, c) = assign_ohlc(&data, [*a, *b, *c, *d]);
            (
                Some(o),
                Some(h),
                Some(l),
                c,
                rest.first().copied().into_iter().collect(),
            )
        }
    };
    let price_cols: Vec<usize> = [open, high, low, Some(close)]
        .into_iter()
        .flatten()
        .chain(volume.iter().copied())
        .collect();
    let decimal_comma =
        number::detect_decimal_comma(price_cols.iter().flat_map(|&j| column(&data, j)));
    Ok(Layout {
        header: None,
        date,
        tod,
        time_fmt,
        open,
        high,
        low,
        close,
        volume,
        symbol,
        decimal_comma,
        names: Vec::new(),
    })
}

/// High is the column that is most often the row maximum, low the one most often
/// the minimum; the other two are open and close, left to right.
fn assign_ohlc(data: &[&Vec<String>], cols: [usize; 4]) -> (usize, usize, usize, usize) {
    let mut max_hits = [0usize; 4];
    let mut min_hits = [0usize; 4];
    for r in data {
        let vals: Vec<f64> = cols
            .iter()
            .filter_map(|&j| parse_number_any(r.cell(j)))
            .collect();
        if vals.len() != 4 || vals.iter().all(|v| *v == vals[0]) {
            continue;
        }
        // Ties credit every tied column, so a candle that opens at its high
        // still counts for the real high column.
        let max = vals.iter().copied().fold(f64::MIN, f64::max);
        let min = vals.iter().copied().fold(f64::MAX, f64::min);
        for i in 0..4 {
            max_hits[i] += usize::from(vals[i] == max);
            min_hits[i] += usize::from(vals[i] == min);
        }
    }
    let best = |hits: &[usize; 4], skip: Option<usize>| {
        (0..4)
            .filter(|i| Some(*i) != skip)
            .max_by_key(|&i| (hits[i], usize::MAX - i))
            .unwrap_or(0)
    };
    let hi = if max_hits.iter().all(|&n| n == 0) {
        1
    } else {
        best(&max_hits, None)
    };
    let lo = if min_hits.iter().all(|&n| n == 0) {
        2
    } else {
        best(&min_hits, Some(hi))
    };
    let rest: Vec<usize> = (0..4).filter(|i| *i != hi && *i != lo).collect();
    (cols[rest[0]], cols[hi], cols[lo], cols[rest[1]])
}

// ---------------------------------------------------------------------------
// Conversion

/// Result of turning rows into candles.
#[derive(Debug, Default)]
pub struct Converted {
    pub candles: Vec<Candle>,
    /// Rows that could not be read.
    pub skipped: usize,
    /// The first unreadable row, for error messages.
    pub first_bad: Option<String>,
    pub volume_col: Option<usize>,
    pub symbol: Option<String>,
    pub duplicates: usize,
    pub repaired: usize,
    pub was_descending: bool,
    pub filled_open: bool,
}

struct RawRow {
    t: i64,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    close: f64,
}

/// Reads every row with a known layout. `rows` must start at the top of the
/// table (the header and anything before it are skipped here).
pub fn convert<R: Row>(layout: &Layout, rows: impl Iterator<Item = R>) -> Converted {
    let dc = layout.decimal_comma;
    let num = |row: &R, j: usize| number::parse(row.cell(j), dc);
    let mut out = Converted::default();
    let mut raw: Vec<RawRow> = Vec::new();
    let mut vols: Vec<Vec<f64>> = vec![Vec::new(); layout.volume.len()];
    let mut nonzero = vec![false; layout.volume.len()];

    for row in rows.skip(layout.data_start()) {
        if row.is_blank() {
            continue;
        }
        let t = parse_time(layout.time_fmt, row.cell(layout.date)).and_then(|d| match layout.tod {
            Some(c) => datetime::parse_time_of_day(row.cell(c)).map(|s| d + s),
            None => Some(d),
        });
        let close = num(&row, layout.close);
        let (Some(t), Some(close)) = (t, close) else {
            out.skipped += 1;
            if out.first_bad.is_none() {
                out.first_bad = Some(
                    (0..row.width())
                        .map(|i| row.cell(i))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            continue;
        };
        if out.symbol.is_none() {
            out.symbol = layout
                .symbol
                .map(|c| row.cell(c).trim().to_string())
                .filter(|s| !s.is_empty());
        }
        for (k, &j) in layout.volume.iter().enumerate() {
            let v = num(&row, j).unwrap_or(0.0).max(0.0);
            nonzero[k] |= v > 0.0;
            vols[k].push(v);
        }
        raw.push(RawRow {
            t,
            open: layout.open.and_then(|j| num(&row, j)),
            high: layout.high.and_then(|j| num(&row, j)),
            low: layout.low.and_then(|j| num(&row, j)),
            close,
        });
    }

    let chosen = nonzero.iter().position(|&nz| nz);
    out.volume_col = chosen.map(|k| layout.volume[k]);
    let volumes = chosen.map(|k| std::mem::take(&mut vols[k]));

    out.was_descending = raw.len() >= 2 && raw.first().map(|r| r.t) > raw.last().map(|r| r.t);
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by_key(|&i| raw[i].t); // stable: equal times keep file order

    let mut prev_close: Option<f64> = None;
    for (pos, &i) in order.iter().enumerate() {
        // Duplicate timestamps: keep the last row of each run.
        if order
            .get(pos + 1)
            .is_some_and(|&next| raw[next].t == raw[i].t)
        {
            out.duplicates += 1;
            continue;
        }
        let r = &raw[i];
        let open = r.open.unwrap_or_else(|| {
            out.filled_open = true;
            prev_close.unwrap_or(r.close)
        });
        let high0 = r.high.unwrap_or(open.max(r.close));
        let low0 = r.low.unwrap_or(open.min(r.close));
        let high = high0.max(open).max(r.close).max(low0);
        let low = low0.min(open).min(r.close).min(high0);
        if high != high0 || low != low0 {
            out.repaired += 1;
        }
        out.candles.push(Candle {
            t: r.t,
            open,
            high,
            low,
            close: r.close,
            volume: volumes.as_ref().map_or(0.0, |v| v[i]),
        });
        prev_close = Some(r.close);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(text: &str, sep: char) -> Vec<Vec<String>> {
        text.lines()
            .map(|l| l.split(sep).map(|s| s.trim().to_string()).collect())
            .collect()
    }

    fn run(text: &str, sep: char) -> (Layout, Converted) {
        let rows = grid(text, sep);
        let layout = infer(&rows, None).unwrap();
        let conv = convert(&layout, rows.into_iter());
        (layout, conv)
    }

    #[test]
    fn header_roles() {
        let r = |h: &str| role_of(&norm_header(h)).map(|x| x.0);
        assert_eq!(r("Date"), Some(Role::Time));
        assert_eq!(r("open_time"), Some(Role::Time));
        assert_eq!(r("close_time"), None);
        assert_eq!(r("<TICKVOL>"), Some(Role::Volume));
        assert_eq!(r("Vol."), Some(Role::Volume));
        assert_eq!(r("Adj Close"), Some(Role::Close));
        assert_eq!(r("Price"), Some(Role::Close));
        assert_eq!(r("Change %"), None);
        assert_eq!(r("Open Interest"), None);
        assert_eq!(r("taker_buy_base_asset_volume"), None);
        assert_eq!(r("1. open"), Some(Role::Open));
        assert_eq!(r("ราคาเปิด"), Some(Role::Open));
        assert_eq!(r("ราคาปิด"), Some(Role::Close));
        assert_eq!(r("ราคาสูงสุด"), Some(Role::High));
        assert_eq!(r("ราคาต่ำสุด"), Some(Role::Low));
        assert_eq!(r("ปริมาณ (พันหุ้น)"), Some(Role::Volume));
        assert_eq!(r("Volume BTC"), Some(Role::Volume));
        assert_eq!(r("Ticker"), Some(Role::Symbol));
        assert_eq!(r("volatility"), None);
    }

    #[test]
    fn yahoo_style() {
        let (layout, conv) = run(
            "Date,Open,High,Low,Close,Adj Close,Volume\n\
             2024-01-02,10,12,9,11,10.5,1000\n\
             2024-01-03,11,13,10,12,11.5,2000\n",
            ',',
        );
        assert_eq!(layout.close, 4);
        assert_eq!(conv.candles.len(), 2);
        assert_eq!(conv.candles[1].close, 12.0);
        assert_eq!(conv.candles[1].volume, 2000.0);
    }

    #[test]
    fn headerless_binance() {
        let (layout, conv) = run(
            "1704067200000,42283.58,42554.57,42261.02,42475.23,1271.68108,1704070799999,5.4e7,47134,682.5,2.9e7,0\n\
             1704070800000,42475.23,42775.00,42431.65,42613.56,1196.37856,1704074399999,5.1e7,42681,632.2,2.7e7,0\n",
            ',',
        );
        assert_eq!(layout.time_fmt, TimeFmt::Epoch);
        assert_eq!(
            (layout.open, layout.high, layout.low, layout.close),
            (Some(1), Some(2), Some(3), 4)
        );
        assert_eq!(layout.volume, vec![5]);
        assert_eq!(conv.candles[0].t, 1_704_067_200);
        assert_eq!(conv.candles[1].volume, 1196.37856);
    }

    #[test]
    fn headerless_odd_column_order() {
        // date, open, close, high, low, volume
        let (layout, _) = run(
            "2024-01-02,10,11,12,9,100\n2024-01-03,11,10.5,11.5,10,200\n2024-01-04,10.5,12,12.5,10.2,300\n",
            ',',
        );
        assert_eq!(
            (layout.open, layout.high, layout.low, layout.close),
            (Some(1), Some(3), Some(4), 2)
        );
    }

    #[test]
    fn metatrader_date_and_time() {
        let (layout, conv) = run(
            "<DATE>\t<TIME>\t<OPEN>\t<HIGH>\t<LOW>\t<CLOSE>\t<TICKVOL>\t<VOL>\t<SPREAD>\n\
             2024.01.02\t00:00:00\t2062.47\t2065.10\t2058.30\t2060.00\t5120\t0\t12\n\
             2024.01.02\t01:00:00\t2060.00\t2061.10\t2055.30\t2057.00\t4300\t0\t12\n",
            '\t',
        );
        assert_eq!(layout.tod, Some(1));
        assert_eq!(
            conv.volume_col,
            Some(6),
            "falls back to TICKVOL when VOL is all zero"
        );
        assert_eq!(conv.candles[1].t - conv.candles[0].t, 3600);
    }

    #[test]
    fn descending_with_duplicates_and_close_only() {
        let (_, conv) = run(
            "Date,Close\n2024-01-04,12\n2024-01-03,11\n2024-01-03,11.5\n2024-01-02,10\n",
            ',',
        );
        assert!(conv.was_descending);
        assert_eq!(conv.duplicates, 1);
        assert!(conv.filled_open);
        let c: Vec<f64> = conv.candles.iter().map(|c| c.close).collect();
        assert_eq!(c, vec![10.0, 11.5, 12.0]);
        assert_eq!(conv.candles[1].open, 10.0, "open = previous close");
    }

    #[test]
    fn skips_bad_rows_and_preamble() {
        let (_, conv) = run(
            "https://www.example.com\nunix,date,symbol,open,high,low,close,Volume BTC,Volume USDT\n\
             1704067200,2024-01-01 00:00:00,BTC/USDT,1,2,0.5,1.5,10,15\n\
             1704070800,2024-01-01 01:00:00,BTC/USDT,1.5,2,1,1.2,null,6\n\
             oops,,,,,,,,\n",
            ',',
        );
        assert_eq!(conv.candles.len(), 2);
        assert_eq!(conv.skipped, 1);
        assert_eq!(conv.symbol.as_deref(), Some("BTC/USDT"));
        assert_eq!(conv.candles[1].volume, 0.0);
    }
}
