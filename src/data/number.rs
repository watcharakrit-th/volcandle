//! Tolerant number parsing: thousands separators, decimal commas, K/M/B suffixes.

/// Parses one numeric cell. `decimal_comma` selects European style (`1.234,5`).
///
/// Accepts `1,234.5`, `45.2K`, `1.2M`, `3B`, `$12.5`, `฿12.5`, `12%`, `(5)`,
/// `1 234,5`, `1.7E+12`. Returns `None` for empty cells, `-`, `null`, `N/A`
/// and any other text.
pub fn parse(s: &str, decimal_comma: bool) -> Option<f64> {
    let s = s.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    if !decimal_comma {
        if let Ok(v) = s.parse::<f64>() {
            return v.is_finite().then_some(v);
        }
    }
    let (body, mult) = split_suffix(s);
    let mut buf = String::with_capacity(body.len());
    let mut negative = false;
    let mut digits = false;
    for c in body.chars() {
        match c {
            '0'..='9' => {
                buf.push(c);
                digits = true;
            }
            '.' if !decimal_comma => buf.push('.'),
            ',' if decimal_comma => buf.push('.'),
            '.' | ',' => {} // thousands separator
            'e' | 'E' if digits && !buf.contains('e') => buf.push('e'),
            '+' if buf.ends_with('e') => buf.push('+'),
            '-' | '\u{2212}' if buf.ends_with('e') => buf.push('-'),
            '-' | '\u{2212}' | '(' if !digits => negative = true,
            ')' | '+' | ' ' | '\u{a0}' | '\u{202f}' | '\'' | '_' | '$' | '€' | '£' | '¥' | '฿'
            | '%' => {}
            _ => return None,
        }
    }
    if !digits {
        return None;
    }
    let v: f64 = buf.parse().ok()?;
    let v = if negative { -v } else { v } * mult;
    v.is_finite().then_some(v)
}

fn split_suffix(s: &str) -> (&str, f64) {
    let mult = match s.as_bytes().last() {
        Some(b'K' | b'k') => 1e3,
        Some(b'M' | b'm') => 1e6,
        Some(b'B' | b'b') => 1e9,
        Some(b'T' | b't') => 1e12,
        _ => return (s, 1.0),
    };
    (s[..s.len() - 1].trim_end(), mult)
}

/// Decides whether a file writes decimals with a comma, from sample cells of its
/// numeric columns. `1.234,56` votes comma, `1,234.56` votes dot, `12,5` votes
/// comma; `1,234` is ambiguous and is ignored.
pub fn detect_decimal_comma<'a>(cells: impl IntoIterator<Item = &'a str>) -> bool {
    let (mut comma, mut dot) = (0usize, 0usize);
    for s in cells {
        let s = s.trim().trim_matches('"');
        if !s.bytes().any(|b| b.is_ascii_digit()) {
            continue;
        }
        match (s.rfind(','), s.rfind('.')) {
            (Some(c), Some(d)) => {
                if c > d {
                    comma += 1;
                } else {
                    dot += 1;
                }
            }
            (Some(c), None) => {
                let after = s[c + 1..].bytes().take_while(u8::is_ascii_digit).count();
                if s.matches(',').count() == 1 && after != 3 {
                    comma += 1;
                }
            }
            (None, Some(_)) => {
                if s.matches('.').count() > 1 {
                    comma += 1;
                } else {
                    dot += 1;
                }
            }
            (None, None) => {}
        }
    }
    comma > dot
}

/// True for cells that are a plain number (`123`, `-1.5`, `1.7E+12`), as opposed
/// to dates such as `2024.01.02` or text.
pub fn is_plain(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.bytes().any(|b| b.is_ascii_digit())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'))
        && s.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_style() {
        let p = |s| parse(s, false);
        assert_eq!(p("42283.58"), Some(42283.58));
        assert_eq!(p("\"42,283.58\""), Some(42283.58));
        assert_eq!(p("1,234,567"), Some(1_234_567.0));
        assert_eq!(p("45.23K"), Some(45_230.0));
        assert_eq!(p("1.2M"), Some(1_200_000.0));
        assert_eq!(p("2.5B"), Some(2.5e9));
        assert_eq!(p("$1,234.5"), Some(1234.5));
        assert_eq!(p("฿12.50"), Some(12.5));
        assert_eq!(p("-0.65%"), Some(-0.65));
        assert_eq!(p("(5)"), Some(-5.0));
        assert_eq!(p("1.70406720E+12"), Some(1.7040672e12));
        assert_eq!(p("-37.63"), Some(-37.63));
        for bad in [
            "",
            "-",
            "null",
            "N/A",
            "nan",
            "inf",
            "abc",
            "2024-01-02",
            "1-2",
        ] {
            assert_eq!(p(bad), None, "{bad}");
        }
    }

    #[test]
    fn comma_style() {
        let p = |s| parse(s, true);
        assert_eq!(p("1.234,56"), Some(1234.56));
        assert_eq!(p("12,5"), Some(12.5));
        assert_eq!(p("1 234,5"), Some(1234.5));
        assert_eq!(p("1.234.567"), Some(1_234_567.0));
        assert_eq!(p("45,2K"), Some(45_200.0));
    }

    #[test]
    fn detect() {
        assert!(detect_decimal_comma(["1.234,56", "12,5", "13,25"]));
        assert!(!detect_decimal_comma(["1,234.56", "12.5", "13.25"]));
        assert!(!detect_decimal_comma(["1,234", "2,345", "12"]));
        assert!(!detect_decimal_comma(["42283.58", "42554.57"]));
    }

    #[test]
    fn plain() {
        assert!(is_plain("1704067200000"));
        assert!(is_plain("-1.5"));
        assert!(is_plain("1.7E+12"));
        assert!(!is_plain("2024.01.02"));
        assert!(!is_plain("2024-01-02"));
        assert!(!is_plain("inf"));
        assert!(!is_plain("1,234"));
    }
}
