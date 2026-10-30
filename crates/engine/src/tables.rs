//! Read-only table extraction from the displayed text layer, including recognised scan text.
//! Unlike Office's paragraph inference, this works on glyph geometry: numeric cells may be
//! right-aligned and a cell may consist of several separately positioned words.

use pdfcraft_render::{PageText, TextGlyph};
use std::fmt::Write as _;

/// Coordinates are displayed PDF points from the top left. Columns are internal separators,
/// not column left edges. Supply them to correct automatic inference in a selected region.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableOptions {
    pub rect: Option<[f32; 4]>,
    pub columns: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rect: [f32; 4],
    pub columns: Vec<f32>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageTables {
    pub page: usize,
    /// Whether the selected region has text. Recognition is a separate, undoable operation.
    pub has_text: bool,
    pub tables: Vec<Table>,
}

const MAX_GLYPHS: usize = 100_000;
const MAX_COLUMNS: usize = 128;
const MAX_CELLS: usize = 200_000;
const MAX_TEXT: usize = 16 * 1024 * 1024;

impl TableOptions {
    pub(crate) fn validate(&self, width: f32, height: f32) -> Result<[f32; 4], String> {
        let r = self.rect.unwrap_or([0.0, 0.0, width, height]);
        if !width.is_finite()
            || !height.is_finite()
            || !r.iter().all(|n| n.is_finite())
            || r[0] < 0.0
            || r[1] < 0.0
            || r[2] > width
            || r[3] > height
            || r[2] <= r[0]
            || r[3] <= r[1]
        {
            return Err("table region must be a nonempty rectangle inside the displayed page".into());
        }
        if self.columns.len() >= MAX_COLUMNS
            || self.columns.iter().any(|x| !x.is_finite() || *x <= r[0] || *x >= r[2])
            || self.columns.windows(2).any(|w| w.first() >= w.get(1))
        {
            return Err("table columns must be increasing separator positions inside the region (at most 127)".into());
        }
        Ok(r)
    }
}

struct Row {
    glyphs: Vec<usize>,
    rect: [f32; 4],
    gaps: Vec<[f32; 2]>,
}

fn union(a: &mut [f32; 4], b: [f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

fn inside(g: &TextGlyph, r: [f32; 4]) -> bool {
    let x = g.rect[0] * 0.5 + g.rect[2] * 0.5;
    let y = g.rect[1] * 0.5 + g.rect[3] * 0.5;
    x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
}

/// Deliberately conservative, text-based inference. No raster rules, merged cells or
/// cross-page joins are guessed. The preview/manual separators are part of the workflow.
pub(crate) fn extract(text: &PageText, page: usize, region: [f32; 4], options: &TableOptions) -> Result<PageTables, String> {
    if text.glyphs.len() > MAX_GLYPHS || text.glyphs.iter().map(|g| g.text.len()).try_fold(0usize, usize::checked_add).is_none_or(|n| n > MAX_TEXT) {
        return Err("page is too large for table extraction (100,000 glyphs / 16 MiB of text)".into());
    }
    let mut indices = Vec::new();
    for (i, g) in text.glyphs.iter().enumerate() {
        if g.text.trim().is_empty() {
            continue;
        }
        if !g.rect.iter().all(|n| n.is_finite()) || g.rect[2] <= g.rect[0] || g.rect[3] <= g.rect[1] {
            return Err("page text has invalid glyph geometry; table extraction would be incomplete".into());
        }
        if !g.text.trim().is_empty() && inside(g, region) {
            if g.text.chars().any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r')) {
                return Err("table text contains unsupported control characters".into());
            }
            if !g.direction.iter().all(|n| n.is_finite()) || g.direction[0] < 0.98 || g.direction[1].abs() > 0.15 {
                return Err("table extraction requires horizontal left-to-right text; rotate the page or select another region".into());
            }
            indices.push(i);
        }
    }
    let has_text = !indices.is_empty();
    // Build visual rows, independent of PDF paint order or the reading-order column groups.
    indices.sort_by(|a, b| {
        let centre = |i: &usize| text.glyphs.get(*i).map_or(0.0, |g| g.rect[1] * 0.5 + g.rect[3] * 0.5);
        centre(a).total_cmp(&centre(b))
    });
    let mut rows: Vec<Row> = Vec::new();
    for i in indices {
        let Some(g) = text.glyphs.get(i) else { continue };
        let joins = rows.last().is_some_and(|row| {
            let overlap = row.rect[3].min(g.rect[3]) - row.rect[1].max(g.rect[1]);
            overlap > 0.5 * (row.rect[3] - row.rect[1]).min(g.rect[3] - g.rect[1])
        });
        if joins && let Some(row) = rows.last_mut() {
            row.glyphs.push(i);
            union(&mut row.rect, g.rect);
        } else {
            rows.push(Row { glyphs: vec![i], rect: g.rect, gaps: Vec::new() });
        }
    }
    for row in &mut rows {
        row.glyphs.sort_by(|a, b| {
            let x = |i: &usize| text.glyphs.get(*i).map_or(0.0, |g| g.rect[0]);
            x(a).total_cmp(&x(b))
        });
        let mut right: Option<f32> = None;
        for &i in &row.glyphs {
            let Some(g) = text.glyphs.get(i) else { continue };
            // A cell gap must be appreciably wider than an ordinary word space.
            let gap = (g.rect[3] - g.rect[1]).max(8.0);
            if let Some(r) = right
                && g.rect[0] - r >= gap
            {
                row.gaps.push([r, g.rect[0]]);
            }
            right = Some(right.map_or(g.rect[2], |r| r.max(g.rect[2])));
        }
    }
    if !options.columns.is_empty() {
        let table = materialize(text, &rows, region, options.columns.clone())?;
        return Ok(PageTables { page, has_text, tables: if has_text { vec![table] } else { Vec::new() } });
    }
    // Break at captions, body text and large vertical gaps. A sparse single-cell row between
    // two multi-cell rows belongs to the run; text before or after it does not.
    let mut tables = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        if rows.get(start).is_none_or(|r| r.gaps.is_empty()) {
            start += 1;
            continue;
        }
        let mut end = start + 1;
        let mut last_multi = start;
        while let (Some(prev), Some(row)) = (end.checked_sub(1).and_then(|i| rows.get(i)), rows.get(end)) {
            let height = (prev.rect[3] - prev.rect[1]).max(row.rect[3] - row.rect[1]);
            if row.rect[1] - prev.rect[3] > height * 2.5 {
                break;
            }
            if row.gaps.is_empty() && rows.get(end + 1).is_none_or(|next| next.gaps.is_empty()) {
                break;
            }
            if !row.gaps.is_empty() {
                last_multi = end;
            }
            end += 1;
        }
        end = last_multi + 1;
        if let Some(run) = rows.get(start..end)
            && run.len() >= 2
        {
            let columns = infer_columns(run)?;
            if !columns.is_empty() {
                let mut rect = run.first().map_or(region, |r| r.rect);
                for r in run {
                    union(&mut rect, r.rect);
                }
                tables.push(materialize(text, run, rect, columns)?);
            }
        }
        start = end.max(start + 1);
    }
    Ok(PageTables { page, has_text, tables })
}

/// Intersect gutters repeated on a majority of rows; this accommodates right-aligned prices
/// without treating each price's left edge as a new column.
fn infer_columns(rows: &[Row]) -> Result<Vec<f32>, String> {
    let mut gaps: Vec<([f32; 2], usize)> = rows.iter().enumerate().flat_map(|(i, r)| r.gaps.iter().map(move |g| (*g, i))).collect();
    gaps.sort_by(|a, b| (a.0[1] - a.0[0]).total_cmp(&(b.0[1] - b.0[0])));
    let mut gutters: Vec<([f32; 2], std::collections::BTreeSet<usize>)> = Vec::new();
    for (g, row) in gaps {
        if let Some((r, support)) = gutters.iter_mut().find(|(r, support)| r[0].max(g[0]) + 2.0 < r[1].min(g[1]) && !support.contains(&row)) {
            r[0] = r[0].max(g[0]);
            r[1] = r[1].min(g[1]);
            support.insert(row);
        } else {
            if gutters.len() >= MAX_COLUMNS * 2 {
                return Err("too many candidate table columns; select a smaller region or supply separators".into());
            }
            gutters.push((g, std::collections::BTreeSet::from([row])));
        }
    }
    let mut columns: Vec<f32> =
        gutters.into_iter().filter(|(_, s)| s.len() >= 2 && s.len() >= rows.len().div_ceil(2)).map(|(g, _)| g[0] * 0.5 + g[1] * 0.5).collect();
    columns.sort_by(f32::total_cmp);
    columns.dedup_by(|a, b| (*a - *b).abs() < 2.0);
    Ok(columns)
}

fn materialize(text: &PageText, rows: &[Row], rect: [f32; 4], columns: Vec<f32>) -> Result<Table, String> {
    let count = columns.len().saturating_add(1);
    if count > MAX_COLUMNS || rows.len().checked_mul(count).is_none_or(|n| n > MAX_CELLS) {
        return Err("table exceeds 128 columns or 200,000 cells; select a smaller region".into());
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells = vec![String::new(); count];
        let mut previous: Vec<Option<usize>> = vec![None; count];
        for &i in &row.glyphs {
            let Some(g) = text.glyphs.get(i) else { continue };
            if columns.iter().any(|x| *x > g.rect[0] + 0.5 && *x < g.rect[2] - 0.5) {
                return Err("a column separator cuts through text; adjust separators or select a smaller table region".into());
            }
            let column = columns.partition_point(|x| *x < g.rect[0] * 0.5 + g.rect[2] * 0.5);
            let (Some(cell), Some(prev)) = (cells.get_mut(column), previous.get_mut(column)) else { continue };
            if let Some(p) = *prev
                && let Some(before) = text.glyphs.get(p)
            {
                let space = text.space_before.get(i).copied().unwrap_or(false) || g.rect[0] - before.rect[2] > (g.rect[3] - g.rect[1]) * 0.15;
                if space && !cell.ends_with(char::is_whitespace) {
                    cell.push(' ');
                }
            }
            cell.push_str(&g.text);
            *prev = Some(i);
        }
        out.push(cells.into_iter().map(|s| s.trim().to_string()).collect());
    }
    Ok(Table { rect, columns, rows: out })
}

impl Table {
    /// UTF-8, RFC 4180 quoting and CRLF. Spreadsheet formula prefixes are escaped with an
    /// apostrophe (like measurement CSV); use XLSX for lossless literal cell strings.
    pub fn csv(&self) -> String {
        let mut out = String::new();
        for row in &self.rows {
            for (i, value) in row.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let formula = value.trim_start().starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r']);
                out.push('"');
                if formula {
                    out.push('\'');
                }
                out.push_str(&value.replace('"', "\"\""));
                out.push('"');
            }
            out.push_str("\r\n");
        }
        out
    }

    /// A single worksheet of literal strings: preserves leading zeros and never creates formulas.
    pub fn xlsx(&self) -> Result<Vec<u8>, String> {
        let mut sheet = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
        );
        if self.rows.len() > 1_048_576 {
            return Err("too many rows for Excel".into());
        }
        for (i, row) in self.rows.iter().enumerate() {
            if row.len() > MAX_COLUMNS {
                return Err("too many table columns".into());
            }
            let _ = write!(sheet, "<row r=\"{}\">", i + 1);
            for (j, value) in row.iter().enumerate() {
                if value.encode_utf16().count() > 32_767
                    || value.chars().any(|c| !matches!(c, '\t' | '\n' | '\r') && (c < ' ' || matches!(c, '\u{fffe}' | '\u{ffff}')))
                {
                    return Err("a cell contains text Excel cannot store (invalid XML character or over 32,767 UTF-16 units)".into());
                }
                let mut col = String::new();
                let mut n = j + 1;
                while n > 0 {
                    col.insert(0, char::from(b'A' + ((n - 1) % 26) as u8));
                    n = (n - 1) / 26;
                }
                // OOXML ST_Xstring: preserve literal _xHHHH_ sequences instead of letting
                // Excel decode them as a character, and encode CR without XML normalization.
                let mut xml = String::new();
                for (offset, ch) in value.char_indices() {
                    let token = value.as_bytes().get(offset..offset.saturating_add(7));
                    let escape = token.is_some_and(|t| {
                        t.starts_with(b"_x") && t.last() == Some(&b'_') && t.get(2..6).is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
                    });
                    match ch {
                        '_' if escape => xml.push_str("_x005F_"),
                        '\r' => xml.push_str("_x000D_"),
                        '&' => xml.push_str("&amp;"),
                        '<' => xml.push_str("&lt;"),
                        '>' => xml.push_str("&gt;"),
                        _ => xml.push(ch),
                    }
                }
                let _ = write!(sheet, "<c r=\"{col}{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{xml}</t></is></c>", i + 1);
            }
            sheet.push_str("</row>");
            if sheet.len() > MAX_TEXT * 8 {
                return Err("table is too large to export".into());
            }
        }
        sheet.push_str("</sheetData></worksheet>");
        let mut zip = pdfcraft_export::Zip::default();
        for (name, body) in [
            (
                "[Content_Types].xml",
                "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>",
            ),
            (
                "_rels/.rels",
                "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>",
            ),
            (
                "xl/workbook.xml",
                "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Table\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
            ),
            (
                "xl/_rels/workbook.xml.rels",
                "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>",
            ),
            ("xl/worksheets/sheet1.xml", sheet.as_str()),
        ] {
            zip.add(name, body.as_bytes(), false);
        }
        Ok(zip.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(cells: &[(f32, f32, &str)]) -> PageText {
        let mut text = PageText::default();
        for &(x, y, s) in cells {
            for (i, ch) in s.chars().enumerate() {
                let x = x + i as f32 * 5.0;
                text.glyphs.push(TextGlyph {
                    text: ch.to_string(),
                    rect: [x, y, x + 5.0, y + 10.0],
                    quad: [[x, y], [x + 5.0, y], [x + 5.0, y + 10.0], [x, y + 10.0]],
                    direction: [1.0, 0.0],
                });
                text.line_of.push(0);
                text.space_before.push(false);
            }
        }
        text
    }

    #[test]
    fn right_aligned_numbers_blank_cells_and_separate_tables() {
        let t = text(&[
            (10.0, 10.0, "Name"),
            (100.0, 10.0, "Qty"),
            (185.0, 10.0, "Price"),
            (10.0, 30.0, "Apple juice"),
            (110.0, 30.0, "12"),
            (185.0, 30.0, "25.00"),
            (10.0, 50.0, "Pear"),
            (190.0, 50.0, "7.50"),
            (10.0, 120.0, "Second"),
            (110.0, 120.0, "1"),
            (10.0, 140.0, "Last"),
            (110.0, 140.0, "2"),
        ]);
        let found = extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).unwrap();
        assert_eq!(found.tables.len(), 2);
        assert_eq!(found.tables[0].rows, vec![vec!["Name", "Qty", "Price"], vec!["Apple juice", "12", "25.00"], vec!["Pear", "", "7.50"]]);
        assert_eq!(found.tables[1].rows, vec![vec!["Second", "1"], vec!["Last", "2"]]);
    }

    #[test]
    fn region_and_manual_columns_keep_sparse_rows_and_exclude_prose() {
        let t = text(&[(10.0, 10.0, "Caption"), (10.0, 40.0, "A"), (100.0, 40.0, "B"), (10.0, 60.0, "C"), (10.0, 100.0, "Footer")]);
        let options = TableOptions { rect: Some([0.0, 30.0, 180.0, 80.0]), columns: vec![80.0] };
        let region = options.validate(300.0, 200.0).unwrap();
        let found = extract(&t, 0, region, &options).unwrap();
        assert_eq!(found.tables[0].rows, vec![vec!["A", "B"], vec!["C", ""]]);
        let invalid = TableOptions { rect: None, columns: vec![120.0, 80.0] };
        assert!(invalid.validate(300.0, 200.0).is_err());
        assert!(TableOptions { rect: Some([0.0, 0.0, f32::NAN, 80.0]), columns: vec![] }.validate(300.0, 200.0).is_err());
        assert!(TableOptions { rect: Some([-1.0, 0.0, 20.0, 20.0]), columns: vec![] }.validate(300.0, 200.0).is_err());
        let cuts_text = TableOptions { rect: None, columns: vec![12.0] };
        assert!(extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &cuts_text).unwrap_err().contains("cuts through text"));
    }

    #[test]
    fn prose_blank_and_turned_text_are_not_silently_exported() {
        let mut t = text(&[(10.0, 10.0, "Just ordinary text"), (10.0, 30.0, "Another short line")]);
        assert!(extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).unwrap().tables.is_empty());
        assert!(!extract(&PageText::default(), 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).unwrap().has_text);
        t.glyphs[0].direction = [0.0, 1.0];
        assert!(extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).is_err());
        t.glyphs[0].direction = [1.0, 0.0];
        t.glyphs[0].rect[0] = f32::NAN;
        assert!(extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).unwrap_err().contains("invalid glyph geometry"));
        t.glyphs[0].rect[0] = 10.0;
        t.glyphs[0].text = "bad\u{1}".into();
        assert!(extract(&t, 0, [0.0, 0.0, 300.0, 200.0], &TableOptions::default()).unwrap_err().contains("control characters"));
    }

    #[test]
    fn exports_preserve_strings_and_escape_spreadsheet_formulas() {
        let table = Table {
            rect: [0.0; 4],
            columns: vec![1.0, 2.0],
            rows: vec![vec!["001".into(), "=1+2".into(), "a,\"b\"\nc&<d>".into()], vec!["\t1".into(), "-2".into(), "@SUM(A1)".into()]],
        };
        assert_eq!(table.csv(), "\"001\",\"'=1+2\",\"a,\"\"b\"\"\nc&<d>\"\r\n\"'\t1\",\"'-2\",\"'@SUM(A1)\"\r\n");
        let xlsx = table.xlsx().unwrap();
        assert!(xlsx.starts_with(b"PK"));
        let bytes = String::from_utf8_lossy(&xlsx);
        assert!(bytes.contains("t=\"inlineStr\""));
        assert!(bytes.contains(">=1+2</t>") && bytes.contains(">001</t>"));
        assert!(bytes.contains("c&amp;&lt;d&gt;"));
        let escaped = Table { rows: vec![vec!["_x0041_\r".into()]], ..table.clone() }.xlsx().unwrap();
        assert!(String::from_utf8_lossy(&escaped).contains("_x005F_x0041__x000D_"));
        let bad = Table { rows: vec![vec!["bad\u{1}".into()]], ..table };
        assert!(bad.xlsx().is_err());
    }

    #[test]
    fn searchable_ocr_layer_extracts_and_survives_save_reopen() {
        use crate::{Edit, Session, export::Exporter, ocr::PlacedWord};
        let mut session = Session::new();
        let bytes = session.create_blank(300.0, 200.0, 1).unwrap();
        let id = session.open("scan.pdf", None, bytes, None).unwrap();
        let mut exporter = Exporter::new(session.get(id).unwrap());
        assert!(!exporter.tables(0, &TableOptions::default()).unwrap().has_text);
        let words =
            [(20.0, 150.0, "Name"), (150.0, 150.0, "Qty"), (20.0, 126.0, "Apple"), (150.0, 126.0, "12"), (20.0, 102.0, "Pear"), (150.0, 102.0, "7")]
                .into_iter()
                .map(|(x, y, s)| PlacedWord { text: s.into(), origin: [x, y], across: [s.len() as f64 * 7.0, 0.0], up: [0.0, 14.0] })
                .collect();
        session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        let before = session.get(id).unwrap().display.clone();
        let rows = Exporter::new(session.get(id).unwrap()).tables(0, &TableOptions::default()).unwrap().tables.remove(0).rows;
        assert_eq!(rows, vec![vec!["Name", "Qty"], vec!["Apple", "12"], vec!["Pear", "7"]]);
        assert_eq!(session.get(id).unwrap().display, before);
        let saved = session.save_bytes(id).unwrap();
        let reopened = session.open("saved.pdf", None, saved, None).unwrap();
        assert_eq!(Exporter::new(session.get(reopened).unwrap()).tables(0, &TableOptions::default()).unwrap().tables[0].rows, rows);
        assert!(!session.get(reopened).unwrap().dirty);
        session.undo(id).unwrap();
        assert!(!Exporter::new(session.get(id).unwrap()).tables(0, &TableOptions::default()).unwrap().has_text);
    }
}
