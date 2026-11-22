//! Spreadsheets (xlsx, xlsm, xlsb, xls, ods) via calamine. Every sheet becomes a
//! grid of text cells; date cells are written as ISO dates.

use calamine::{open_workbook_auto_from_rs, Data, Reader};

use super::datetime;
use super::json::Grid;

/// `true` if the bytes start like a ZIP (xlsx/ods/xlsb) or OLE (xls) file.
pub fn looks_like_spreadsheet(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

fn cell(d: &Data) -> String {
    match d {
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, mo, d, h, mi, s, _) = dt.to_ymd_hms_milli();
            datetime::timestamp(
                i64::from(y),
                u32::from(mo),
                u32::from(d),
                u32::from(h),
                u32::from(mi),
                u32::from(s),
            )
            .map(datetime::format_datetime)
            .unwrap_or_default()
        }
        Data::DateTime(dt) => dt.as_f64().to_string(),
        Data::Bool(_) | Data::Error(_) | Data::Empty => String::new(),
    }
}

/// One grid per non-empty sheet, with the sheet name.
pub fn grids(bytes: &[u8]) -> Result<Vec<(String, Grid)>, String> {
    let mut wb = open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Could not open the spreadsheet: {e}"))?;
    let mut out = Vec::new();
    for name in wb.sheet_names() {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let grid: Grid = range.rows().map(|r| r.iter().map(cell).collect()).collect();
        if !grid.is_empty() {
            out.push((name, grid));
        }
    }
    if out.is_empty() {
        return Err("The spreadsheet has no data.".into());
    }
    Ok(out)
}
