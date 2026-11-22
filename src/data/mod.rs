//! OHLCV data: the candle model, loading any file with format detection, and
//! timeframe resampling.

pub mod datetime;
pub mod number;
pub mod resample;

mod delimited;
mod excel;
mod json;
mod table;

use std::path::Path;

pub use datetime::DateOrder;
pub use resample::Timeframe;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candle {
    /// Bar start, seconds since 1970-01-01 (see [`datetime`]).
    pub t: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// A loaded price history.
#[derive(Clone, Debug, Default)]
pub struct Series {
    /// Symbol (from a ticker column) or the file name.
    pub name: String,
    /// Sorted by time, unique times.
    pub candles: Vec<Candle>,
    /// Typical bar spacing in seconds (0 if unknown).
    pub interval: i64,
    /// Decimals to show for prices.
    pub decimals: usize,
    pub has_volume: bool,
    /// Short description of the file format.
    pub format: String,
    /// Which column became what.
    pub columns: String,
    /// Things worth knowing (skipped rows, repaired candles, …).
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LoadOptions {
    /// Forces D/M/Y or M/D/Y for all-numeric dates; `None` detects it.
    pub date_order: Option<DateOrder>,
}

pub fn load_file(path: &Path, opts: &LoadOptions) -> Result<Series, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    load_bytes(&name, &bytes, opts)
}

pub fn load_bytes(name: &str, bytes: &[u8], opts: &LoadOptions) -> Result<Series, String> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err("The file is empty.".into());
    }
    let order = opts.date_order;
    let (format, layout, conv) = if excel::looks_like_spreadsheet(bytes) {
        best_of(excel::grids(bytes)?, order, "Spreadsheet")?
    } else {
        let (text, encoding) = decode_text(bytes);
        let start = text.trim_start();
        let head = start.chars().take(512).collect::<String>().to_lowercase();
        if head.starts_with('<') && (head.contains("<html") || head.contains("<!doctype")) {
            return Err(concat!(
                "This is a web page (HTML), not a data file. The site may have shown ",
                "a login, cookie or captcha page instead of the download."
            )
            .into());
        }
        if start.starts_with('{') || start.starts_with('[') {
            let grids = json::grids(&text)?
                .into_iter()
                .map(|g| (String::new(), g))
                .collect();
            best_of(grids, order, "JSON")?
        } else {
            let (delim, layout, conv) = delimited::parse(&text, order)?;
            let header = match layout.header {
                Some(h) => format!("header on line {}", h + 1),
                None => "no header".into(),
            };
            (
                format!("Text ({} separated, {header}, {encoding})", delim.label()),
                layout,
                conv,
            )
        }
    };
    finish(name, format, &layout, conv)
}

/// Tries every candidate table and keeps the one that yields the most candles.
fn best_of(
    grids: Vec<(String, json::Grid)>,
    order: Option<DateOrder>,
    kind: &str,
) -> Result<(String, table::Layout, table::Converted), String> {
    let mut best: Option<(String, table::Layout, table::Converted)> = None;
    let mut first_err = None;
    for (sheet, grid) in grids {
        match table::infer(&grid, order) {
            Ok(layout) => {
                let conv = table::convert(&layout, grid.into_iter());
                if best
                    .as_ref()
                    .is_none_or(|b| conv.candles.len() > b.2.candles.len())
                {
                    let format = if sheet.is_empty() {
                        kind.to_string()
                    } else {
                        format!("{kind}, sheet \"{sheet}\"")
                    };
                    best = Some((format, layout, conv));
                }
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    best.ok_or_else(|| {
        first_err.unwrap_or_else(|| format!("No price table found in this {kind} file."))
    })
}

fn finish(
    name: &str,
    format: String,
    layout: &table::Layout,
    conv: table::Converted,
) -> Result<Series, String> {
    if conv.candles.is_empty() {
        let mut msg = "No candles could be read.".to_string();
        if let Some(row) = &conv.first_bad {
            msg += &format!(" First unreadable row: \"{row}\".");
        }
        msg += &format!(" Detected: {}.", layout.describe(conv.volume_col));
        return Err(msg);
    }
    let mut candles = conv.candles;
    let mut notes = Vec::new();
    // Daily (or longer) bars: keep only the date. Sources like Yahoo stamp them
    // with the session open in UTC (03:00 for SET, 14:30 for New York).
    let mut merged = 0;
    if resample::detect_interval(&candles) >= 20 * datetime::HOUR
        && candles.iter().any(|c| c.t.rem_euclid(datetime::DAY) != 0)
    {
        let mut out: Vec<Candle> = Vec::with_capacity(candles.len());
        for mut c in candles {
            c.t -= c.t.rem_euclid(datetime::DAY);
            match out.last_mut() {
                Some(last) if last.t == c.t => {
                    *last = c;
                    merged += 1;
                }
                _ => out.push(c),
            }
        }
        candles = out;
    }
    if conv.skipped > 0 {
        let example = conv.first_bad.as_deref().unwrap_or_default();
        let example: String = example.chars().take(80).collect();
        notes.push(format!(
            "{} unreadable row(s) skipped, e.g. \"{example}\"",
            conv.skipped
        ));
    }
    if conv.duplicates + merged > 0 {
        notes.push(format!(
            "{} duplicate date(s)/time(s): kept the last row",
            conv.duplicates + merged
        ));
    }
    if conv.repaired > 0 {
        notes.push(format!(
            "{} candle(s) had high/low outside open/close and were repaired",
            conv.repaired
        ));
    }
    if conv.was_descending {
        notes.push("rows were newest-first; sorted oldest-first".into());
    }
    if conv.filled_open {
        notes.push("no open/high/low columns: candles are built from closing prices".into());
    }
    let has_volume = candles.iter().any(|c| c.volume > 0.0);
    if !has_volume {
        notes.push("no volume in this data: every candle has the same width".into());
    }
    Ok(Series {
        name: conv.symbol.unwrap_or_else(|| name.to_string()),
        interval: resample::detect_interval(&candles),
        decimals: price_decimals(&candles),
        has_volume,
        format,
        columns: layout.describe(conv.volume_col),
        notes,
        candles,
    })
}

/// Decimals needed to show the prices as written (up to 8), ignoring float
/// noise. Some sources (Yahoo) store prices as 32-bit floats, so 4083.7 arrives
/// as 4083.699951171875; such data is compared at float32 precision.
fn price_decimals(candles: &[Candle]) -> usize {
    let values: Vec<f64> = candles
        .iter()
        .take(3_000)
        .flat_map(|c| [c.open, c.high, c.low, c.close])
        .filter(|v| *v != 0.0 && v.is_finite())
        .collect();
    // Within double rounding of a float32 (JSON often prints them with 16 digits).
    let near_f32 = |v: f64| (f64::from(v as f32) - v).abs() <= v.abs() * 1e-12;
    let f32_like = values.iter().filter(|v| near_f32(**v)).count() * 10 >= values.len() * 9;
    let rel = if f32_like { 2e-7 } else { 1e-12 };
    let decimals_of = |v: f64| {
        (0..=8)
            .find(|&d| {
                let m = 10f64.powi(d);
                ((v * m).round() / m - v).abs() <= v.abs() * rel
            })
            .unwrap_or(8) as usize
    };
    values.iter().map(|v| decimals_of(*v)).max().unwrap_or(2)
}

/// Decodes text files: UTF-8 (with or without BOM), UTF-16 (MetaTrader 5 exports
/// it), and otherwise Windows-874 / TIS-620, the legacy Thai code page, which
/// equals Latin-1 for everything that is not Thai.
fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "UTF-8");
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                if le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(rest, true), "UTF-16");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(rest, false), "UTF-16");
    }
    // UTF-16 without a BOM: every other byte is zero in ASCII text.
    let head = &bytes[..bytes.len().min(512)];
    let zeros_odd = head.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    if head.len() >= 8 && zeros_odd * 10 >= head.len() / 2 * 9 {
        return (utf16(bytes, true), "UTF-16");
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_owned(), "UTF-8");
    }
    // Legacy 8-bit text. Thai words are runs of high bytes; accented Latin
    // letters (Windows-1252, e.g. "Eröffnung") are mostly single high bytes.
    let high = |i: usize| bytes.get(i).is_some_and(|b| *b >= 0x80);
    let (mut total, mut in_runs) = (0usize, 0usize);
    for i in (0..bytes.len()).filter(|&i| high(i)) {
        total += 1;
        in_runs += usize::from(high(i + 1) || (i > 0 && high(i - 1)));
    }
    if in_runs * 2 > total {
        let text = bytes
            .iter()
            .map(|&b| match b {
                0xA1..=0xFB => char::from_u32(0x0E01 + u32::from(b - 0xA1)).unwrap_or('?'),
                _ => char::from(b),
            })
            .collect();
        (text, "Windows-874 (Thai)")
    } else {
        (bytes.iter().map(|&b| char::from(b)).collect(), "Latin-1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFa,b").0, "a,b");
        assert_eq!(decode_text(b"\xFF\xFEa\0,\0b\0").0, "a,b");
        assert_eq!(decode_text(b"a\0,\0b\0,\0c\0").0, "a,b,c");
        // "ราคา" in TIS-620
        assert_eq!(decode_text(&[0xC3, 0xD2, 0xA4, 0xD2]).0, "ราคา");
        // "Eröffnung" in Windows-1252
        assert_eq!(decode_text(b"Er\xF6ffnung;Hoch").0, "Eröffnung;Hoch");
    }

    #[test]
    fn decimals() {
        let c = |v: f64| Candle {
            t: 0,
            open: v,
            high: v,
            low: v,
            close: v,
            volume: 0.0,
        };
        assert_eq!(price_decimals(&[c(42283.58), c(1.5)]), 2);
        assert_eq!(price_decimals(&[c(1.08345), c(0.1 + 0.2)]), 5);
        assert_eq!(price_decimals(&[c(3500.0)]), 0);
        let f32_noise = [4083.7f32, 3997.4, 4066.5, 30.25].map(|v| c(f64::from(v)));
        assert_eq!(price_decimals(&f32_noise), 2);
        // Yahoo prints float32 values with 16 significant digits.
        let printed = [
            "93.20999908447266",
            "95.11000061035156",
            "91.69999694824219",
        ]
        .map(|s| c(s.parse().unwrap()));
        assert_eq!(price_decimals(&printed), 2);
        assert_eq!(price_decimals(&[c(0.00001234), c(0.00001301)]), 8);
    }

    #[test]
    fn errors_are_helpful() {
        let e = load_bytes("x", b"hello world\nfoo bar\n", &LoadOptions::default()).unwrap_err();
        assert!(e.contains("date"), "{e}");
        assert!(load_bytes("x", b"  \n", &LoadOptions::default())
            .unwrap_err()
            .contains("empty"));
    }
}
