//! JSON: finds every table-like structure in a document and turns it into rows
//! of text cells for [`super::table`].
//!
//! Recognized shapes (also when nested, e.g. under `"data"` or `"result"`):
//! - array of arrays: `[[1704067200000, "42283.58", …], …]` (Binance klines)
//! - array of objects: `[{"time": …, "open": …}, …]` (most APIs)
//! - object of equal-length arrays: `{"t": […], "o": […], …}` (TradingView UDF)
//! - Yahoo Finance chart: `timestamp` + `indicators.quote[0]`
//! - object keyed by date: `{"2024-01-02": {"1. open": …}}` (Alpha Vantage)
//! - JSON Lines: one object or array per line

use serde_json::Value;

use super::datetime::{self, DateOrder};

pub type Grid = Vec<Vec<String>>;

fn cell(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

fn is_scalar(v: &Value) -> bool {
    !v.is_array() && !v.is_object()
}

pub fn grids(text: &str) -> Result<Vec<Grid>, String> {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            // JSON Lines?
            let items: Option<Vec<Value>> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).ok())
                .collect();
            match items {
                Some(items) if !items.is_empty() => Value::Array(items),
                _ => return Err(format!("Invalid JSON: {e}")),
            }
        }
    };
    let mut out = Vec::new();
    collect(&value, 0, None, &mut out);
    Ok(out)
}

/// Adds the symbol found around a table (e.g. Yahoo's `meta.symbol`) as a column,
/// unless the table already has one.
fn with_symbol(mut grid: Grid, symbol: Option<&str>, has_header: bool) -> Grid {
    let Some(sym) = symbol else { return grid };
    if has_header {
        let named = grid.first().is_some_and(|h| {
            h.iter()
                .any(|k| matches!(k.to_lowercase().as_str(), "symbol" | "ticker"))
        });
        if named {
            return grid;
        }
        for (i, row) in grid.iter_mut().enumerate() {
            row.push(if i == 0 {
                "symbol".to_string()
            } else {
                sym.to_string()
            });
        }
    } else {
        for row in &mut grid {
            row.push(sym.to_string());
        }
    }
    grid
}

fn from_objects<'a>(
    items: impl Iterator<Item = &'a serde_json::Map<String, Value>> + Clone,
) -> Option<Grid> {
    let mut keys: Vec<String> = Vec::new();
    for o in items.clone().take(20) {
        for (k, v) in o {
            if is_scalar(v) && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    if keys.is_empty() {
        return None;
    }
    let mut grid = vec![keys.clone()];
    grid.extend(items.map(|o| {
        keys.iter()
            .map(|k| o.get(k).map(cell).unwrap_or_default())
            .collect()
    }));
    Some(grid)
}

fn collect(v: &Value, depth: usize, symbol: Option<&str>, out: &mut Vec<Grid>) {
    if depth > 8 {
        return;
    }
    match v {
        Value::Array(items) if !items.is_empty() => {
            if items.iter().take(5).all(Value::is_array) {
                let grid = items
                    .iter()
                    .map(|x| {
                        x.as_array()
                            .map(|a| a.iter().map(cell).collect())
                            .unwrap_or_default()
                    })
                    .collect();
                out.push(with_symbol(grid, symbol, false));
            } else if items.iter().take(5).all(Value::is_object) {
                if let Some(g) = from_objects(items.iter().filter_map(Value::as_object)) {
                    out.push(with_symbol(g, symbol, true));
                }
                collect(&items[0], depth + 1, symbol, out);
            }
        }
        Value::Object(map) => {
            let symbol = ["symbol", "ticker"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str))
                .or_else(|| v.pointer("/meta/symbol").and_then(Value::as_str))
                .or(symbol);
            // Yahoo Finance chart API.
            if let (Some(ts), Some(q)) = (
                map.get("timestamp").and_then(Value::as_array),
                v.pointer("/indicators/quote/0").and_then(Value::as_object),
            ) {
                let cols: Vec<(&String, &Vec<Value>)> = q
                    .iter()
                    .filter_map(|(k, v)| v.as_array().map(|a| (k, a)))
                    .collect();
                let mut grid = vec![std::iter::once("timestamp".to_string())
                    .chain(cols.iter().map(|(k, _)| (*k).clone()))
                    .collect::<Vec<_>>()];
                for (i, t) in ts.iter().enumerate() {
                    let mut row = vec![cell(t)];
                    row.extend(
                        cols.iter()
                            .map(|(_, a)| a.get(i).map(cell).unwrap_or_default()),
                    );
                    grid.push(row);
                }
                out.push(with_symbol(grid, symbol, true));
            }
            // Columnar: several scalar arrays of the same length.
            let arrays: Vec<(&String, &Vec<Value>)> = map
                .iter()
                .filter_map(|(k, v)| v.as_array().map(|a| (k, a)))
                .filter(|(_, a)| !a.is_empty() && a.iter().all(is_scalar))
                .collect();
            let mut lens: Vec<usize> = arrays.iter().map(|(_, a)| a.len()).collect();
            lens.sort_unstable();
            if let Some(len) = lens
                .iter()
                .copied()
                .max_by_key(|l| lens.iter().filter(|x| *x == l).count())
            {
                let cols: Vec<_> = arrays.iter().filter(|(_, a)| a.len() == len).collect();
                if cols.len() >= 3 {
                    let mut grid = vec![cols.iter().map(|(k, _)| (*k).clone()).collect::<Vec<_>>()];
                    grid.extend((0..len).map(|i| cols.iter().map(|(_, a)| cell(&a[i])).collect()));
                    out.push(with_symbol(grid, symbol, true));
                }
            }
            // Keyed by date: {"2024-01-02": {"1. open": "…", …}, …}
            let dated = map
                .iter()
                .filter(|(k, v)| v.is_object() && datetime::parse_text(k, DateOrder::Dmy).is_some())
                .count();
            if dated >= 2 && dated * 10 >= map.len() * 8 {
                let entries: Vec<(&String, &serde_json::Map<String, Value>)> = map
                    .iter()
                    .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
                    .collect();
                if let Some(mut g) = from_objects(entries.iter().map(|(_, o)| *o)) {
                    g[0].insert(0, "date".into());
                    for (row, (k, _)) in g[1..].iter_mut().zip(&entries) {
                        row.insert(0, (*k).clone());
                    }
                    out.push(with_symbol(g, symbol, true));
                }
            }
            for child in map.values() {
                collect(child, depth + 1, symbol, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let g = grids(r#"[[1704067200000,"1.0","2.0","0.5","1.5","10"]]"#).unwrap();
        assert_eq!(g[0][0][0], "1704067200000");
        let g = grids(r#"{"s":"ok","t":[1,2],"o":[1,2],"h":[1,2],"l":[1,2],"c":[1,2],"v":[1,2]}"#)
            .unwrap();
        assert_eq!(g[0].len(), 3);
        let g = grids(r#"{"data":[{"time":"2024-01-02","close":1.5}]}"#).unwrap();
        assert_eq!(g[0][0], vec!["close", "time"]);
        let g = grids(r#"{"symbol":"PTT","data":[{"time":"2024-01-02","close":1.5}]}"#).unwrap();
        assert_eq!(g[0][1], vec!["1.5", "2024-01-02", "PTT"]);
        let g = grids("{\"a\":1,\"b\":[1]}\n").unwrap();
        assert!(g.is_empty());
        let g = grids("{\"t\":1,\"c\":2}\n{\"t\":2,\"c\":3}\n").unwrap();
        assert_eq!(g[0].len(), 3);
    }
}
