//! xlsx reader (design_doc §9.9) — the boundary side of mounting an Excel
//! workbook, the format engineering exports actually arrive in.
//!
//! An .xlsx is a zip of XML parts; this reader walks the zip central directory
//! (no external zip crate — entries are stored or DEFLATEd, and we have
//! `inflate` for the latter), resolves shared strings, and flattens the first
//! worksheet into rows: first row = header, like the CSV reader.

use crate::inflate::inflate;
use crate::source::infer_value;
use crate::xml::{self, Elem};
use serde_json::{Map, Value};

/// Parse .xlsx bytes into rows (first sheet; first row is the header).
pub fn parse(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let shared = match entry(bytes, "xl/sharedStrings.xml")? {
        Some(xml_bytes) => shared_strings(&text(xml_bytes)?)?,
        None => Vec::new(),
    };
    // the first worksheet by conventional name; fall back to any sheet part
    let sheet = match entry(bytes, "xl/worksheets/sheet1.xml")? {
        Some(b) => b,
        None => first_sheet(bytes)?.ok_or("workbook has no worksheets")?,
    };
    rows_from_sheet(&text(sheet)?, &shared)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "worksheet XML is not UTF-8".into())
}

// ---- the zip container -------------------------------------------------------

/// Locate and decompress one named entry via the central directory.
fn entry(zip: &[u8], name: &str) -> Result<Option<Vec<u8>>, String> {
    for e in central_directory(zip)? {
        if e.name == name {
            return extract(zip, &e).map(Some);
        }
    }
    Ok(None)
}

fn first_sheet(zip: &[u8]) -> Result<Option<Vec<u8>>, String> {
    for e in central_directory(zip)? {
        if e.name.starts_with("xl/worksheets/") && e.name.ends_with(".xml") {
            return extract(zip, &e).map(Some);
        }
    }
    Ok(None)
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    header_offset: usize,
}

/// Walk the end-of-central-directory record back from the tail, then the
/// central directory entries. Comments up to 64 KiB are tolerated.
fn central_directory(zip: &[u8]) -> Result<Vec<Entry>, String> {
    if zip.len() < 22 {
        return Err("not a zip: too short".into());
    }
    let eocd = (0..=(zip.len() - 22).min(65_557))
        .map(|back| zip.len() - 22 - back)
        .find(|&i| zip[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
        .ok_or("not a zip: no end-of-central-directory")?;
    let count = u16le(zip, eocd + 10) as usize;
    let mut off = u32le(zip, eocd + 16) as usize;
    let mut out = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        if off + 46 > zip.len() || !zip[off..].starts_with(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err("corrupt zip central directory".into());
        }
        let method = u16le(zip, off + 10);
        let compressed = u32le(zip, off + 20) as usize;
        let name_len = u16le(zip, off + 28) as usize;
        let extra_len = u16le(zip, off + 30) as usize;
        let comment_len = u16le(zip, off + 32) as usize;
        let header_offset = u32le(zip, off + 42) as usize;
        let name_end = off + 46 + name_len;
        if name_end > zip.len() {
            return Err("truncated zip entry name".into());
        }
        let name = String::from_utf8_lossy(&zip[off + 46..name_end]).into_owned();
        out.push(Entry { name, method, compressed, header_offset });
        off = name_end + extra_len + comment_len;
    }
    Ok(out)
}

/// Read an entry's bytes via its local header (whose name/extra lengths differ
/// from the central directory's), then decompress by method.
fn extract(zip: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
    let h = e.header_offset;
    if h + 30 > zip.len() || !zip[h..].starts_with(&[0x50, 0x4b, 0x03, 0x04]) {
        return Err(format!("corrupt zip local header for {}", e.name));
    }
    let name_len = u16le(zip, h + 26) as usize;
    let extra_len = u16le(zip, h + 28) as usize;
    let start = h + 30 + name_len + extra_len;
    let end = start + e.compressed;
    if end > zip.len() {
        return Err(format!("truncated zip data for {}", e.name));
    }
    match e.method {
        0 => Ok(zip[start..end].to_vec()),
        8 => inflate(&zip[start..end]).map_err(|err| format!("{}: {err}", e.name)),
        m => Err(format!("{}: unsupported zip method {m}", e.name)),
    }
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

// ---- the sheet XML -----------------------------------------------------------

/// `<si>` items become the shared-string table; rich-text runs concatenate.
fn shared_strings(xml_text: &str) -> Result<Vec<String>, String> {
    let root = xml::parse(xml_text)?;
    Ok(root.find_all("si").iter().map(|si| si.text()).collect())
}

fn rows_from_sheet(xml_text: &str, shared: &[String]) -> Result<Vec<Value>, String> {
    let root = xml::parse(xml_text)?;
    let data = root.find("sheetData").ok_or("worksheet has no sheetData")?;
    let mut grid: Vec<Vec<(usize, Value)>> = Vec::new();
    for row in data.find_all("row") {
        let mut cells = Vec::new();
        let mut next_col = 0usize;
        for c in row.find_all("c") {
            let col = c
                .attr("r")
                .and_then(col_index)
                .unwrap_or(next_col);
            next_col = col + 1;
            cells.push((col, cell_value(c, shared)));
        }
        grid.push(cells);
    }
    let mut it = grid.into_iter();
    let header_cells = it.next().ok_or("empty worksheet")?;
    let ncols = header_cells.iter().map(|(c, _)| c + 1).max().unwrap_or(0);
    let mut header = vec![String::new(); ncols];
    for (c, v) in header_cells {
        if let Some(s) = v.as_str() {
            header[c] = s.to_string();
        } else if !v.is_null() {
            header[c] = v.to_string();
        }
    }
    let mut rows = Vec::new();
    for cells in it {
        let mut obj = Map::new();
        for (c, v) in cells {
            match header.get(c) {
                Some(name) if !name.is_empty() => {
                    obj.insert(name.clone(), v);
                }
                _ => {}
            }
        }
        if obj.values().any(|v| !v.is_null()) {
            rows.push(Value::Object(obj));
        }
    }
    if rows.is_empty() {
        return Err("worksheet has a header but no data rows".into());
    }
    Ok(rows)
}

/// Cell reference "BC23" → 0-based column index 54.
fn col_index(r: &str) -> Option<usize> {
    let letters: String = r.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    if letters.is_empty() {
        return None;
    }
    let mut n = 0usize;
    for ch in letters.chars() {
        n = n * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    Some(n - 1)
}

/// A cell's typed value: shared string, inline string, bool, or number.
fn cell_value(c: &Elem, shared: &[String]) -> Value {
    let t = c.attr("t").unwrap_or("");
    match t {
        "s" => {
            let idx: usize = c.find("v").map(|v| v.text()).unwrap_or_default().parse().unwrap_or(usize::MAX);
            shared.get(idx).cloned().map(Value::String).unwrap_or(Value::Null)
        }
        "inlineStr" => c
            .find("is")
            .map(|is| Value::String(is.text()))
            .unwrap_or(Value::Null),
        "b" => Value::Bool(c.find("v").map(|v| v.text()) == Some("1".into())),
        // "str" (formula result) and untyped numeric cells both live in <v>
        _ => c
            .find("v")
            .map(|v| infer_value(&v.text()))
            .unwrap_or(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal stored-method (no compression) xlsx in memory: enough of
    /// a zip for the reader, with sharedStrings and one sheet.
    fn tiny_xlsx() -> Vec<u8> {
        let strings = br#"<sst><si><t>well</t></si><si><t>formation</t></si><si><t>Hugin Fm.</t></si><si><r><t>Skager</t></r><r><t>rak Fm.</t></r></si></sst>"#;
        let sheet = br#"<worksheet><sheetData>
            <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>md</t></is></c></row>
            <row r="2"><c r="A2" t="inlineStr"><is><t>15/9-F-14</t></is></c><c r="B2" t="s"><v>2</v></c><c r="C2"><v>2145.5</v></c></row>
            <row r="3"><c r="A3" t="inlineStr"><is><t>15/9-F-14</t></is></c><c r="B3" t="s"><v>3</v></c><c r="C3"><v>2600</v></c></row>
        </sheetData></worksheet>"#;
        let mut zip = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in [
            ("xl/sharedStrings.xml", strings.as_slice()),
            ("xl/worksheets/sheet1.xml", sheet.as_slice()),
        ] {
            let offset = zip.len() as u32;
            // local header, method 0, sizes equal
            zip.extend([0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            zip.extend(0u32.to_le_bytes()); // crc (unchecked by the reader)
            zip.extend((data.len() as u32).to_le_bytes());
            zip.extend((data.len() as u32).to_le_bytes());
            zip.extend((name.len() as u16).to_le_bytes());
            zip.extend(0u16.to_le_bytes());
            zip.extend(name.as_bytes());
            zip.extend(data);
            // central entry
            cd.extend([0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend(0u32.to_le_bytes());
            cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((name.len() as u16).to_le_bytes());
            cd.extend([0u8; 12]);
            cd.extend(offset.to_le_bytes());
            cd.extend(name.as_bytes());
        }
        let cd_start = zip.len() as u32;
        zip.extend(&cd);
        zip.extend([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 2, 0, 2, 0]);
        zip.extend((cd.len() as u32).to_le_bytes());
        zip.extend(cd_start.to_le_bytes());
        zip.extend(0u16.to_le_bytes());
        zip
    }

    #[test]
    fn sheet_rows_resolve_shared_and_rich_strings() {
        let rows = parse(&tiny_xlsx()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["well"], "15/9-F-14");
        assert_eq!(rows[0]["formation"], "Hugin Fm.");
        assert_eq!(rows[0]["md"], Value::from(2145.5));
        assert_eq!(rows[1]["formation"], "Skagerrak Fm.", "rich-text runs concatenate");
        assert_eq!(rows[1]["md"], Value::from(2600i64));
    }

    #[test]
    fn garbage_is_a_readable_error() {
        let err = parse(b"not a zip at all").unwrap_err();
        assert!(err.contains("zip"), "{err}");
    }

    #[test]
    fn column_letters_map_to_indexes() {
        assert_eq!(col_index("A1"), Some(0));
        assert_eq!(col_index("Z9"), Some(25));
        assert_eq!(col_index("AA10"), Some(26));
        assert_eq!(col_index("BC23"), Some(54));
    }
}
