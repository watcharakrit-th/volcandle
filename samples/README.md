Synthetic sample files: random-walk prices, NOT market data.

Every `*_daily.*` file holds the same 260 daily bars and every `*_1h.*` file the same
500 hourly bars, each written in the layout of a real source (Yahoo, Investing.com,
MetaTrader 4/5, SET in Thai, German Excel CSV, AmiBroker, Binance, NinjaTrader,
TradingView UDF, Alpha Vantage, Excel, …). `tests/samples.rs` checks that they all
load to identical candles.
