# volcandle

A desktop chart for **volume candles**, like TradingView's *Volume candles* chart type:
each candlestick is wider when more was traded. It reads OHLCV price files from almost
any source (crypto, stocks, SET, gold, oil, …) and works out the format by itself.

- **Volume candles:** candles stay in their time slot and get wider with volume. By
  default the widest candle is the one with the largest volume on screen. In Settings
  you can instead compare against the largest volume of the last *N* bars, which keeps
  the widths fixed while you scroll.
- **Equivolume (CandleVolume):** each candle takes horizontal space in proportion to
  its volume. Busy periods stretch out and quiet ones shrink, so the time axis is not
  linear.
- **Candles:** regular candlesticks for comparison.

Also included: volume bars along the bottom, linear or log price scale, crosshair with
price and date tags, an OHLC legend, the last-price line, timeframe switching
(1m → 1h → 1D → 1W → 1M …), dark and light themes, automatic reload when the file
changes, and two indicators: **CDC ActionZone V3 2020** (by piriya33) and **RSI**.

> The files in `samples/` contain **synthetic** prices (random walks). They only show
> the supported file layouts and are not market data.

## Running it in the dev container

The dev container is defined in `D:\Rust\.devcontainer`. Open the `D:\Rust` folder in
VS Code, choose **Reopen in Container**, and open a terminal (`` Ctrl+` ``).

```sh
cd /workspaces/Rust/volcandle
cargo run --release -- samples/binance_klines_1h.csv
```

The window opens on your Windows desktop through WSLg. Without a file argument it
reopens the last file (or demo data the first time).

- Put your own files in `D:\Rust\volcandle\data` (`/workspaces/Rust/volcandle/data` in
  the container). That folder is listed first in the file panel and is git-ignored.
- Click any file in the left panel to open it. Type a folder path in the panel's path
  box to browse elsewhere.
- Dropping a file on the window works when the app runs natively. Through WSLg, dragging
  from Windows Explorer usually does not, so use the panel there.
- **Thai text** (Thai column headers, file names) needs a Thai font. The dev container
  Dockerfile installs `fonts-thai-tlwg`; run **Dev Containers: Rebuild Container** once
  to get it. On Windows or macOS the app uses Tahoma or Leelawadee. To use another font,
  set `VOLCANDLE_FONT=/path/to/font.ttf`.

To check what the app reads from a file, without opening a window:

```sh
cargo run --release -- --check data/PTT.BK.json
```

```
file:     data/PTT.BK.json
symbol:   PTT.BK
format:   JSON
columns:  time = timestamp (Unix time) · open = open · high = high · low = low · close = close · volume = volume
bars:     242 (every 1D)
first:    2025-11-19  O 30.50  H 30.50  L 30.25  C 30.25  V 21.29M
last:     2026-11-19  O 37.20  H 37.60  L 37.10  C 37.50  V 23.17M
```

If a date like `03/04/2024` is read the wrong way round, add `--dmy` or `--mdy` (or pick
the date order in **Settings**).

## What files it reads

| Kind | Examples |
|---|---|
| Text tables: `.csv` `.tsv` `.txt` `.prn` `.dat` | separated by comma, semicolon, tab, `\|` or spaces; with or without a header row; title lines above the header are skipped |
| JSON: `.json` `.jsonl` | Binance klines (`[[time, "open", …]]`), arrays of objects, TradingView UDF (`{"t":[…],"o":[…]}`), Yahoo Finance chart API, Alpha Vantage, JSON Lines, also nested under keys such as `data` or `result` |
| Spreadsheets: `.xlsx` `.xlsm` `.xlsb` `.xls` `.ods` | every sheet is tried; real Excel dates and serial numbers both work |

What it recognizes:

- **Column names** in English and other languages: `Date`, `Open time`, `<DATE>`
  `<TIME>`, `Price` (Investing.com), `Adj Close` (only used when there is no `Close`),
  `Vol.`, `<TICKVOL>`, `Volume BTC`, `1. open`, `Eröffnung`/`Schluss`, and Thai
  `วันที่`, `ราคาเปิด`, `ราคาสูงสุด`, `ราคาต่ำสุด`, `ราคาปิด`, `ปริมาณ`.
- **No header row:** the first date-like column is the time. The next four number
  columns are open, high, low and close; high and low are recognized by their values,
  so unusual orders work. The column after them is the volume.
- **Dates:** Unix time in s, ms, µs or ns (Binance switched from ms to µs in 2025, and
  mixed files work), `2024-01-02T09:30:00Z`, `2024.01.02`, `02/01/2024` (day/month vs
  month/day is detected), `Jan 2, 2024`, `02-Jan-24`, `20240102`, `20240102 093000`,
  Excel dates, separate date and time columns, Thai dates such as `02 ม.ค. 2567`, and
  Buddhist-Era years (2567 = 2024).
- **Numbers:** `1,234.56`, `1.234,56` (decimal comma), `45.2K`, `1.2M`, `3B`, `$`, `฿`, `%`.
  Empty volumes, `-` and `null` count as zero.
- **Encodings:** UTF-8, UTF-16 (MetaTrader 5 exports), Thai Windows-874 and Latin-1.
- **Clean-up:** newest-first files are sorted, duplicate times keep the last row,
  high/low are repaired when they don't contain open/close, and daily bars keep only
  their date. Yahoo stamps daily bars with the market's opening time, which is dropped.
  The status bar lists everything that was fixed.
- **Missing data:** files with only a close column become candles that open at the
  previous close. Without volume, every candle has the same width, and the legend says
  so.

### Where to get data

| Market | Source |
|---|---|
| Crypto | `tools/binance_update.sh` (see below), [data.binance.vision](https://data.binance.vision) (free monthly/daily kline CSVs, e.g. `spot/monthly/klines/BTCUSDT/1h/`), or a CSV export from your exchange |
| Stocks, SET | Yahoo Finance (SET symbols end in `.BK`, e.g. `PTT.BK`), a broker or SETSMART export, or AmiBroker/MetaStock ASCII files |
| Gold, oil | Futures on Yahoo Finance (`GC=F` gold, `CL=F` WTI oil), or an MT4/MT5 export (`XAUUSD`, `USOIL`) |

Spot gold and forex prices often have **no real volume**, because there is no
central exchange. Use futures data, or MetaTrader's *tick volume* (picked automatically
when `<VOL>` is all zeros).

### Binance data, kept up to date

`tools/binance_update.sh` downloads Binance spot candles into `data/` and brings them up
to date. Run it yourself whenever you want fresh data; it never runs on its own.

```sh
tools/binance_update.sh BTCUSDT 1h 1d     # first time: full history (about 15 s for 1h)
tools/binance_update.sh                   # later: update every Binance file in data/
tools/binance_update.sh ETHUSDT SOLUSDT 4h
tools/binance_update.sh --since 2025-01-01 BTCUSDT 1m   # short intervals: start later
```

On Windows, `tools\binance_update.ps1` does the same in PowerShell and writes identical
files. The only difference is `-Since` instead of `--since` (help: `-Help`):

```powershell
.\tools\binance_update.ps1 BTCUSDT 1h 1d
.\tools\binance_update.ps1 -Since 2025-01-01 BTCUSDT 1m
```

- **New candles only:** an update downloads only the candles after the file's last
  row, so it takes a second or two.
- **Finished candles only:** the candle still forming is left out and gets added on the
  next run.
- **Open files:** if the file is open in volcandle, the chart picks up the new candles
  by itself.
- **No account needed:** it uses Binance's public market-data API, so no API key.
- **Intervals:** `1s` `1m` `3m` `5m` `15m` `30m` `1h` `2h` `4h` `6h` `8h` `12h` `1d`
  `3d` `1w`. For months, download `1d` and pick **1M** in the app.
- **Gaps:** an hourly file has a few short gaps where Binance itself was down for
  maintenance, mostly in 2017–2018.
- **Other files are safe:** files the script didn't create are never changed.

Yahoo's chart API (`https://query1.finance.yahoo.com/v8/finance/chart/PTT.BK?range=5y&interval=1d`)
returns JSON that volcandle opens directly. The API is unofficial and may change or
block requests.

## Indicators

Turn them on with the **CDC** and **RSI** buttons in the toolbar; their settings are
under **Indicators**. Both are calculated on the bars you see, so they follow the
timeframe buttons (pick 1D and the RSI is a daily RSI).

**CDC ActionZone V3 2020** by piriya33, ported from his open-source TradingView script:

- **EMA lines:** fast EMA 12 (red) and slow EMA 26 (blue) of the close, with a green
  fill while the fast EMA is above the slow one and red while it is below.
- **Zones:** bars are painted by zone.

  | Zone | Meaning |
  |---|---|
  | Green | buy |
  | Blue | strong rally (pre-buy 2) |
  | Light blue | rally (pre-buy 1) |
  | Red | sell |
  | Orange | strong dip (pre-sell 2) |
  | Yellow | dip (pre-sell 1) |

- **Signals:** a green dot under a bar is a buy: the first green bar after a bearish
  trend. A red dot over a bar is a sell: the first red bar after a bullish trend. The
  dots are hidden when you zoom out so far that bars are narrower than 2 pixels.
- **Trend:** the label at the last bar and the legend line show the current trend.
- **Inputs:** every input of the script is there: source, EMA periods, smoothing,
  bar painting, lines, signals, ribbon, and the label.
  - **Fixed timeframe:** shows a higher timeframe's EMAs, as of its last finished bar,
    on the current chart, drawn as steps.
  - **Momentum signals (StochRSI):** optional; off by default, as in the original.
  - **Alerts:** not included, because they only exist on TradingView.
- **Source:** the script's author suggests `hl2` or `ohlc4` instead of `close` for
  markets that never close, such as crypto.

**RSI** is TradingView's built-in Relative Strength Index:

- **Calculation:** Wilder's smoothing, length 14, on the close.
- **Pane:** its own pane under the chart, with the 70/50/30 lines, the band, and the
  RSI-based moving average (SMA 14, yellow).

Both use TradingView's Pine Script formulas (`ta.ema`, `ta.rma`, `ta.rsi`,
`ta.barssince`, `request.security` without look-ahead), so on the same data the values
should match TradingView once the indicator has warmed up. The first few dozen bars of
a file can differ, because they depend on how the averages are started.

`src/indicators/cdc.rs` is a port of piriya33's script and, like the original, is
under the [Mozilla Public License 2.0](https://mozilla.org/MPL/2.0/).

## Controls

| Action | How |
|---|---|
| Scroll through time | drag the chart, `←` / `→` (`Shift` = 10 bars), horizontal wheel |
| Zoom | mouse wheel, `↑` / `↓`, or drag the time axis. While the newest bar is on screen it stays in place; otherwise zooming follows the mouse |
| Scale prices | drag the price axis up or down (this turns *auto* off), or wheel over it |
| Fit prices again | double-click the price axis, `A`, or the **auto** button |
| Log scale | `L` or the **log** button |
| Volume bars | `V` |
| Chart type | `1` volume candles, `2` equivolume, `3` candles |
| Indicators | **CDC** and **RSI** buttons; settings under **Indicators** |
| Reset view | `R`, double-click the chart, or **⟲ Reset view** |
| Jump to start / end | `Home` / `End` |

**Settings** (⚙): dark or light theme; coloring by previous close; what the widest volume
candle is compared against; a time shift for display (crypto exchanges export UTC, so
choose +7:00 for Thailand time); the date order for ambiguous dates; and automatic
reload.

## Tests

```sh
cargo test
```

- Unit tests cover date parsing (every format above), number styles, header matching,
  layout inference, resampling, axis ticks and chart geometry.
- Indicator tests check the RSI against StockCharts' worked example, the CDC zones,
  the trend and buy/sell rules, the fixed timeframe (never uses an unfinished higher
  bar), and the momentum signals.
- `tests/chart_headless.rs` draws every chart type, theme and indicator option
  without a window, including 525,600 bars fully zoomed out.
- `tests/samples.rs` loads every file in `samples/` and checks that all daily layouts
  give identical candles, and that all hourly layouts do too. The layouts are Yahoo,
  Investing.com, MT4, MT5 (UTF-16), SET in Thai with Buddhist-Era dates, German
  decimal-comma CSV, AmiBroker, Binance CSV (ms + µs) and API JSON, NinjaTrader,
  TradingView UDF, Alpha Vantage, Yahoo chart JSON, Excel and space-separated text.

## How it works

```
src/
  data/datetime.rs   calendar math + tolerant date/time parsing (no date crate)
  data/number.rs     thousands separators, decimal commas, K/M/B suffixes
  data/table.rs      header matching, headerless layout inference, row → candle conversion
  data/delimited.rs  delimiter sniffing, CSV streaming
  data/json.rs       finds table-shaped structures in any JSON
  data/excel.rs      spreadsheets via calamine
  data/resample.rs   interval detection, timeframe aggregation
  data/mod.rs        encodings, format dispatch, clean-up, Series
  indicators/        Pine Script building blocks, RSI, CDC ActionZone (MPL-2.0)
  chart/             the chart widget (geometry, input, drawing, axes, theme)
  browser.rs         side-panel file browser
  app.rs             window, toolbar, settings, background loading, auto-reload
```

The chart keeps its horizontal axis in *world units*. A bar is 1 unit wide in the
candle modes, and volume ÷ mean volume wide in equivolume mode, so panning, zooming and
hit-testing are the same code in every mode. Candles, volume and grid are drawn as one
pixel-aligned mesh, so 1-pixel wicks stay sharp. Bars narrower than about two pixels
are merged per pixel column, so fully zoomed-out charts of 500,000+ bars stay fast.
