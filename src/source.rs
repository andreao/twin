//! File boundary reader (design_doc §9.9) — the Rust side of mounting a local file.
//!
//! Mounting is federation, not export (see the data-residence model): the twin keeps
//! the file as the source of truth and holds only a rebuildable view (§15.1).  This
//! reader turns a CSV / JSON / JSONL file into rows the graph ingests as a source
//! stream; the file stays put.  Format is inferred from extension, then content.

use serde_json::{Map, Value};

/// Read a local file into rows (each an object). Errors are human-readable — they go
/// straight back to the agent (and user) as an observation.
pub fn read_file(path: &str) -> Result<Vec<Value>, String> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".xlsx") {
        // binary container — bytes, not text
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        return crate::xlsx::parse(&bytes);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    if lower.ends_with(".json") {
        parse_json(&text)
    } else if lower.ends_with(".jsonl") || lower.ends_with(".ndjson") {
        parse_jsonl(&text)
    } else if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        parse_csv(&text, if lower.ends_with(".tsv") { '\t' } else { ',' })
    } else if lower.ends_with(".las") {
        crate::las::parse(&text)
    } else if lower.ends_with(".xml") {
        crate::witsml::parse(&text)
    } else {
        // unknown extension: try JSON, then JSONL, then fixed-width (well picks and
        // other survey exports), then CSV
        parse_json(&text)
            .or_else(|_| parse_jsonl(&text))
            .or_else(|_| parse_fixed_width(&text))
            .or_else(|_| parse_csv(&text, ','))
    }
}

/// Parse a fixed-width table (well picks, survey listings): column boundaries are
/// DETECTED as character positions that are blank in (nearly) every line — the
/// statistical "pattern grammar becomes a parser" move.  The first line is the
/// header; short lines are padded.
pub fn parse_fixed_width(text: &str) -> Result<Vec<Value>, String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    if lines.len() < 2 {
        return Err("too few lines for a fixed-width table".into());
    }
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    // blank_at[i] = how many lines are blank (or already ended) at column i
    let mut blank_at = vec![0usize; width];
    for l in &lines {
        let chars: Vec<char> = l.chars().collect();
        for (i, slot) in blank_at.iter_mut().enumerate() {
            if chars.get(i).map(|c| c.is_whitespace()).unwrap_or(true) {
                *slot += 1;
            }
        }
    }
    // a separator is a run of ≥2 columns blank in every line; fields lie between
    let all = lines.len();
    let mut fields: Vec<(usize, usize)> = Vec::new(); // [start, end)
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < width {
        let sep_len = blank_at[i..].iter().take_while(|&&b| b == all).count();
        if sep_len >= 2 || (sep_len >= 1 && i + sep_len == width) {
            if let Some(s) = start.take() {
                fields.push((s, i));
            }
            i += sep_len;
        } else {
            if start.is_none() {
                start = Some(i);
            }
            i += 1;
        }
    }
    if let Some(s) = start {
        fields.push((s, width));
    }
    if fields.len() < 2 {
        return Err("no fixed-width column structure detected".into());
    }
    let slice = |l: &str, (s, e): (usize, usize)| -> String {
        l.chars().skip(s).take(e - s).collect::<String>().trim().to_string()
    };
    let header: Vec<String> = fields.iter().map(|&f| slice(lines[0], f)).collect();
    if header.iter().any(|h| h.is_empty()) {
        return Err("fixed-width header has an unnamed column".into());
    }
    let mut rows = Vec::new();
    for l in &lines[1..] {
        let mut obj = Map::new();
        for (name, &f) in header.iter().zip(&fields) {
            obj.insert(name.clone(), infer_value(&slice(l, f)));
        }
        rows.push(Value::Object(obj));
    }
    Ok(rows)
}

fn parse_json(text: &str) -> Result<Vec<Value>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    match v {
        Value::Array(rows) => Ok(rows),
        obj @ Value::Object(_) => Ok(vec![obj]),
        _ => Err("JSON is not an array of objects".into()),
    }
}

fn parse_jsonl(text: &str) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("line {}: {e}", i + 1))?;
        rows.push(v);
    }
    if rows.is_empty() {
        return Err("no JSON lines found".into());
    }
    Ok(rows)
}

/// A small RFC-4180-ish CSV parser: quoted fields, embedded delimiters/newlines,
/// and doubled-quote escaping. First row is the header.
fn parse_csv(text: &str, delim: char) -> Result<Vec<Value>, String> {
    let grid = csv_grid(text, delim);
    let mut iter = grid.into_iter();
    let header = iter.next().ok_or("empty file")?;
    if header.is_empty() {
        return Err("no header row".into());
    }
    let mut rows = Vec::new();
    for cells in iter {
        if cells.len() == 1 && cells[0].trim().is_empty() {
            continue; // blank line
        }
        let mut obj = Map::new();
        for (i, col) in header.iter().enumerate() {
            let raw = cells.get(i).cloned().unwrap_or_default();
            obj.insert(col.clone(), infer_value(&raw));
        }
        rows.push(Value::Object(obj));
    }
    if rows.is_empty() {
        return Err("no data rows".into());
    }
    Ok(rows)
}

fn csv_grid(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delim {
            row.push(std::mem::take(&mut field));
        } else if c == '\n' {
            row.push(std::mem::take(&mut field));
            rows.push(std::mem::take(&mut row));
        } else if c != '\r' {
            field.push(c);
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Read a `timestamp,value` datapoints CSV and downsample to ~`target` points for a
/// chart (the chart is a lens over the raw series; it doesn't need every point).
pub fn read_series_downsampled(path: &str, target: usize) -> Vec<(i64, f64)> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let pts: Vec<(i64, f64)> = text
        .lines()
        .skip(1)
        .filter_map(|l| {
            let (t, v) = l.split_once(',')?;
            Some((t.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    downsample(pts, target)
}

/// Keep ~`target` evenly-spaced points (a chart doesn't need every one).
pub fn downsample(pts: Vec<(i64, f64)>, target: usize) -> Vec<(i64, f64)> {
    if target == 0 || pts.len() <= target {
        return pts;
    }
    let step = (pts.len() / target).max(1);
    pts.into_iter().step_by(step).collect()
}

/// Materialize a series to disk (write-through cache after an on-demand fetch, §7).
pub fn write_series(path: &str, pts: &[(i64, f64)]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "timestamp,value")?;
    for (t, v) in pts {
        writeln!(f, "{t},{v}")?;
    }
    Ok(())
}

/// Numbers become numbers, true/false become bools, empty becomes null; else string.
pub(crate) fn infer_value(raw: &str) -> Value {
    let t = raw.trim();
    if t.is_empty() {
        return Value::Null;
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = t.parse::<f64>() {
        if f.is_finite() {
            return Value::from(f);
        }
    }
    match t {
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        _ => Value::String(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_with_quotes_and_types() {
        let rows = read_file_from(".csv", "id,name,temp\n1,\"Pump, A\",38.5\n2,Fan,44\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], Value::String("Pump, A".into()));
        assert_eq!(rows[0]["temp"], Value::from(38.5));
        assert_eq!(rows[1]["id"], Value::from(2i64));
    }

    #[test]
    fn json_array() {
        let rows = read_file_from(".json", r#"[{"a":1},{"a":2}]"#).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn jsonl() {
        let rows = read_file_from(".jsonl", "{\"a\":1}\n{\"a\":2}\n").unwrap();
        assert_eq!(rows.len(), 2);
    }

    // helper: parse by pretending an extension, without touching the filesystem
    fn read_file_from(ext: &str, text: &str) -> Result<Vec<Value>, String> {
        if ext == ".json" {
            parse_json(text)
        } else if ext == ".jsonl" {
            parse_jsonl(text)
        } else {
            parse_csv(text, ',')
        }
    }
}
