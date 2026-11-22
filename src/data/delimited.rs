//! Delimited text (CSV, TSV, semicolon, pipe or whitespace separated).

use super::table::{self, Converted, Layout};
use super::DateOrder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delim {
    Byte(u8),
    Whitespace,
}

impl Delim {
    pub fn label(self) -> &'static str {
        match self {
            Delim::Byte(b',') => "comma",
            Delim::Byte(b';') => "semicolon",
            Delim::Byte(b'\t') => "tab",
            Delim::Byte(b'|') => "pipe",
            Delim::Byte(_) => "custom",
            Delim::Whitespace => "spaces",
        }
    }
}

fn reader(text: &str, d: u8) -> csv::Reader<&[u8]> {
    csv::ReaderBuilder::new()
        .delimiter(d)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes())
}

fn whitespace_rows(text: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    text.lines()
        .map(|l| l.split_whitespace().map(str::to_owned).collect())
}

/// Picks the delimiter that splits the first lines into the most columns,
/// consistently (at least 60% of lines share that column count).
pub fn sniff(text: &str) -> Delim {
    const LINES: usize = 50;
    let mode = |counts: &[usize]| -> (usize, usize) {
        let mut best = (0, 0);
        for &c in counts {
            let n = counts.iter().filter(|&&x| x == c).count();
            if (n, c) > (best.1, best.0) {
                best = (c, n);
            }
        }
        best
    };
    let mut best: Option<(usize, Delim)> = None;
    for d in *b";\t,|" {
        let counts: Vec<usize> = reader(text, d)
            .into_records()
            .filter_map(Result::ok)
            .filter(|r| r.iter().any(|c| !c.is_empty()))
            .take(LINES)
            .map(|r| r.len())
            .collect();
        let (cols, n) = mode(&counts);
        if cols >= 2 && n * 10 >= counts.len() * 6 && best.is_none_or(|(c, _)| cols > c) {
            best = Some((cols, Delim::Byte(d)));
        }
    }
    if best.is_none_or(|(c, _)| c < 3) {
        let counts: Vec<usize> = whitespace_rows(text)
            .filter(|r| !r.is_empty())
            .take(LINES)
            .map(|r| r.len())
            .collect();
        let (cols, n) = mode(&counts);
        if cols >= 3 && n * 10 >= counts.len() * 6 {
            return Delim::Whitespace;
        }
    }
    best.map_or(Delim::Byte(b','), |(_, d)| d)
}

/// Parses delimited text: infers the layout from the first rows, then streams
/// every row through it.
pub fn parse(text: &str, order: Option<DateOrder>) -> Result<(Delim, Layout, Converted), String> {
    let delim = sniff(text);
    const SAMPLE_ROWS: usize = 400;
    let sample: Vec<Vec<String>> = match delim {
        Delim::Byte(d) => reader(text, d)
            .into_records()
            .filter_map(Result::ok)
            .take(SAMPLE_ROWS)
            .map(|r| r.iter().map(str::to_owned).collect())
            .collect(),
        Delim::Whitespace => whitespace_rows(text).take(SAMPLE_ROWS).collect(),
    };
    let layout = table::infer(&sample, order)?;
    let converted = match delim {
        Delim::Byte(d) => table::convert(
            &layout,
            reader(text, d).into_records().filter_map(Result::ok),
        ),
        Delim::Whitespace => table::convert(&layout, whitespace_rows(text)),
    };
    Ok((delim, layout, converted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing() {
        assert_eq!(sniff("a,b,c\n1,2,3\n"), Delim::Byte(b','));
        assert_eq!(sniff("a;b;c\n1,5;2,5;3\n4,5;5;6\n"), Delim::Byte(b';'));
        assert_eq!(sniff("a\tb\tc\n1\t2\t3\n"), Delim::Byte(b'\t'));
        assert_eq!(
            sniff("2024-01-02 1 2 3 4\n2024-01-03 1 2 3 4\n"),
            Delim::Whitespace
        );
        assert_eq!(
            sniff("\"Date\",\"Price\",\"Vol.\"\n\"01/02/2024\",\"42,283.6\",\"1.2K\"\n"),
            Delim::Byte(b',')
        );
    }
}
