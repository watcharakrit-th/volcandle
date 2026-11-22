use std::path::PathBuf;
use std::process::ExitCode;

use volcandle::data::DateOrder;

const USAGE: &str = "\
volcandle - volume candles (candle width = volume) for any OHLCV file

USAGE:
    volcandle [OPTIONS] [FILE]

FILE can be CSV, TSV, TXT, JSON, XLSX, XLS or ODS. Columns, date style and
number style are detected automatically. Without FILE the last file (or
demo data) opens; pick files in the side panel or drop them on the window.

OPTIONS:
    --check        read FILE, print what was detected, and exit (no window)
    --dmy, --mdy   read dates like 03/04/2024 as day/month or month/day
    -h, --help     show this help
    -V, --version  show the version";

fn main() -> ExitCode {
    let mut file = None;
    let mut check = false;
    let mut order = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("-V" | "--version") => {
                println!("volcandle {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            Some("--check") => check = true,
            Some("--dmy") => order = Some(DateOrder::Dmy),
            Some("--mdy") => order = Some(DateOrder::Mdy),
            Some(s) if s.starts_with("--") => {
                eprintln!("volcandle: unknown option {s}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => file = Some(PathBuf::from(arg)),
        }
    }
    if check {
        let Some(path) = file else {
            eprintln!("volcandle: --check needs a FILE");
            return ExitCode::FAILURE;
        };
        return volcandle::check(&path, order);
    }
    match volcandle::run(file, order) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("volcandle: {e}");
            eprintln!(
                "(Is a display available? Inside the dev container DISPLAY should be :0 via WSLg.)"
            );
            ExitCode::FAILURE
        }
    }
}
