//! Every file in `samples/` holds one of two synthetic series, written in a
//! different real-world layout. Each must parse to the same candles.

use std::path::PathBuf;

use volcandle::data::datetime::{self, DAY, HOUR};
use volcandle::data::{load_file, Candle, LoadOptions, Series};

fn load(name: &str) -> Series {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("samples")
        .join(name);
    load_file(&path, &LoadOptions::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn same(name: &str, got: &[Candle], want: &[Candle], volume_tolerance: f64) {
    assert_eq!(got.len(), want.len(), "{name}: number of candles");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let ctx = format!("{name}: candle {i} ({})", datetime::format_datetime(w.t));
        assert_eq!(g.t, w.t, "{ctx}: time");
        for (a, b, what) in [
            (g.open, w.open, "open"),
            (g.high, w.high, "high"),
            (g.low, w.low, "low"),
            (g.close, w.close, "close"),
        ] {
            assert!((a - b).abs() < 1e-6, "{ctx}: {what} {a} != {b}");
        }
        let tol = (w.volume * volume_tolerance).max(1e-6);
        assert!(
            (g.volume - w.volume).abs() <= tol,
            "{ctx}: volume {} != {}",
            g.volume,
            w.volume
        );
    }
}

#[test]
fn daily_layouts_agree() {
    let reference = load("yahoo_daily.csv");
    assert_eq!(reference.candles.len(), 260);
    assert_eq!(reference.interval, DAY);
    assert_eq!(reference.decimals, 2);
    assert!(reference.has_volume);
    assert_eq!(
        reference.candles[0].t,
        datetime::timestamp(2024, 1, 2, 0, 0, 0).unwrap()
    );

    for name in [
        "mt5_export_daily.csv",
        "set_thai_daily.csv",
        "german_daily.csv",
        "amibroker_daily.txt",
        "tradingview_udf_daily.json",
        "alphavantage_daily.json",
        "api_objects_daily.json",
        "excel_daily.xlsx",
    ] {
        same(name, &load(name).candles, &reference.candles, 0.0);
    }
    // Investing.com rounds volume to 3 significant digits ("1.60M").
    same(
        "investing_daily.csv",
        &load("investing_daily.csv").candles,
        &reference.candles,
        0.01,
    );
}

#[test]
fn hourly_layouts_agree() {
    let reference = load("binance_klines_1h.csv");
    assert_eq!(reference.candles.len(), 500);
    assert_eq!(reference.interval, HOUR);
    assert_eq!(reference.decimals, 2);
    for name in [
        "binance_api_1h.json",
        "ninjatrader_1h.txt",
        "mt4_history_1h.csv",
        "yahoo_chart_1h.json",
        "spaces_1h.txt",
    ] {
        same(name, &load(name).candles, &reference.candles, 0.0);
    }
}

#[test]
fn details_are_reported() {
    let investing = load("investing_daily.csv");
    assert!(
        investing.notes.iter().any(|n| n.contains("newest-first")),
        "{:?}",
        investing.notes
    );
    assert!(
        investing.columns.contains("close = Price"),
        "{}",
        investing.columns
    );
    assert!(investing.columns.contains("M/D/Y"), "{}", investing.columns);

    let mt5 = load("mt5_export_daily.csv");
    assert!(mt5.format.contains("UTF-16"), "{}", mt5.format);
    assert!(
        mt5.columns.contains("volume = <TICKVOL>"),
        "{}",
        mt5.columns
    );

    let german = load("german_daily.csv");
    assert!(
        german.columns.contains("decimal comma"),
        "{}",
        german.columns
    );
    assert!(
        german.columns.contains("open = Eröffnung"),
        "{}",
        german.columns
    );

    assert_eq!(load("amibroker_daily.txt").name, "DEMO");
    assert!(load("excel_daily.xlsx").format.contains("sheet \"Prices\""));
}

#[test]
fn close_only_builds_candles() {
    let s = load("close_only_daily.csv");
    let reference = load("yahoo_daily.csv");
    assert_eq!(s.candles.len(), 260);
    assert!(!s.has_volume);
    for (i, c) in s.candles.iter().enumerate() {
        assert_eq!(c.close, reference.candles[i].close);
        let open = if i == 0 {
            c.close
        } else {
            s.candles[i - 1].close
        };
        assert_eq!(c.open, open);
        assert_eq!(c.high, c.open.max(c.close));
    }
}

#[test]
fn resampling_samples() {
    use volcandle::data::resample::{resample, Timeframe};
    let hourly = load("binance_klines_1h.csv");
    let daily = resample(&hourly.candles, Timeframe::Day);
    assert_eq!(daily.len(), 21); // 500 hours = 20 days + 20 hours
    let first_day = &hourly.candles[..24];
    assert_eq!(daily[0].open, first_day[0].open);
    assert_eq!(daily[0].close, first_day[23].close);
    assert_eq!(
        daily[0].high,
        first_day.iter().map(|c| c.high).fold(f64::MIN, f64::max)
    );
    let volume: f64 = first_day.iter().map(|c| c.volume).sum();
    assert!((daily[0].volume - volume).abs() < 1e-9);
}
