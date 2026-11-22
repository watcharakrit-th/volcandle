//! volcandle — TradingView-style volume candles for any OHLCV file.
//!
//! Open a CSV/TSV/TXT, JSON or Excel file of prices (crypto, stocks, SET, gold,
//! oil, …): the format, columns, date style and number style are detected, and
//! the bars are drawn as volume candles (width = volume), equivolume bars or
//! regular candles, with TradingView-like navigation.

pub mod app;
pub mod browser;
pub mod chart;
pub mod data;
pub mod demo;
pub mod fonts;
pub mod indicators;

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

pub use app::run;

use data::{datetime, resample, DateOrder, LoadOptions};

/// `volcandle --check FILE`: loads the file without a window and prints what
/// was detected.
pub fn check(path: &Path, date_order: Option<DateOrder>) -> ExitCode {
    let started = Instant::now();
    let series = match data::load_file(path, &LoadOptions { date_order }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let d = series.decimals;
    let bar = |c: &data::Candle| {
        format!(
            "{}  O {:.d$}  H {:.d$}  L {:.d$}  C {:.d$}  V {}",
            datetime::format_datetime(c.t),
            c.open,
            c.high,
            c.low,
            c.close,
            chart::format_volume(c.volume)
        )
    };
    println!("file:     {}", path.display());
    println!("symbol:   {}", series.name);
    println!("format:   {}", series.format);
    println!("columns:  {}", series.columns);
    println!(
        "bars:     {} (every {})",
        series.candles.len(),
        resample::interval_label(series.interval)
    );
    if let (Some(first), Some(last)) = (series.candles.first(), series.candles.last()) {
        println!("first:    {}", bar(first));
        println!("last:     {}", bar(last));
    }
    println!("decimals: {d}");
    for note in &series.notes {
        println!("note:     {note}");
    }
    println!("read in {ms:.1} ms");
    ExitCode::SUCCESS
}
