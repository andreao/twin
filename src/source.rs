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
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".json") {
        parse_json(&text)
    } else if lower.ends_with(".jsonl") || lower.ends_with(".ndjson") {
        parse_jsonl(&text)
    } else if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        parse_csv(&text, if lower.ends_with(".tsv") { '\t' } else { ',' })
    } else {
        // unknown extension: try JSON, then JSONL, then CSV
        parse_json(&text)
            .or_else(|_| parse_jsonl(&text))
            .or_else(|_| parse_csv(&text, ','))
    }
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
    if target == 0 || pts.len() <= target {
        return pts;
    }
    let step = (pts.len() / target).max(1);
    pts.into_iter().step_by(step).collect()
}

/// Numbers become numbers, true/false become bools, empty becomes null; else string.
fn infer_value(raw: &str) -> Value {
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
