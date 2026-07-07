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
    if std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
        return read_dir_rows(path);
    }
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

/// Mount a whole DIRECTORY as one source: every data file under it parses through
/// the same readers, and each row is tagged with `kind` (the object-type directory
/// it came from — trajectory, bhaRun, log…) and `file` (its relative path, the
/// lineage hook).  This is how a real corpus mounts — a WITSML tree is hundreds of
/// small files per well — and the extraction lenses then carve it by `kind`.
/// Bounded: a directory is a view, not an ingest (§15.1); the files stay put.
/// What counts as a mountable data file, for directory mounts and the
/// available-data scan alike.
const DATA_EXT: [&str; 8] = ["xml", "las", "csv", "tsv", "json", "jsonl", "ndjson", "xlsx"];

fn is_data_file(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .map(|x| DATA_EXT.contains(&x.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn read_dir_rows(root: &str) -> Result<Vec<Value>, String> {
    const MAX_FILES: usize = 500;
    const MAX_ROWS: usize = 20_000;
    let mut files = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_data_file(&p) {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for p in files.iter().take(MAX_FILES) {
        let rel = p
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string_lossy().into_owned());
        // the object type is the nearest non-numeric ancestor directory
        let kind = std::path::Path::new(&rel)
            .parent()
            .into_iter()
            .flat_map(|a| a.components().rev())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .find(|n| !n.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or_default();
        match read_file(p.to_str().unwrap_or_default()) {
            Ok(file_rows) => {
                for mut r in file_rows {
                    if let Value::Object(o) = &mut r {
                        o.insert("kind".into(), Value::String(kind.clone()));
                        o.insert("file".into(), Value::String(rel.clone()));
                    }
                    rows.push(r);
                    if rows.len() >= MAX_ROWS {
                        return Ok(rows);
                    }
                }
            }
            Err(_) => skipped += 1, // one odd file must not sink the mount
        }
    }
    if rows.is_empty() {
        return Err(format!(
            "{root}: no readable data files in the directory ({skipped} skipped)"
        ));
    }
    Ok(rows)
}

/// Scan a data root for mountable things the twin does NOT yet have — the
/// boundary fact behind the agent's "unmounted" perception, so pulled data never
/// sits invisible on disk waiting for a human to name it.  Deterministic and
/// bounded; offers the shallowest useful mount roots:
///   - a directory with data files directly in it (a datapoints/ of CSVs);
///   - at the depth cap, a subtree that holds data further down (one well's
///     WITSML tree) — the granularity a whole-directory mount wants;
///   - a loose data file.
/// Paths already mounted (or inside a mounted directory) are skipped, as are the
/// twin's own stores (models, projects, docs, journals).
pub fn scan_available(root: &str, mounted: &[String]) -> Vec<Value> {
    const MAX_OFFERS: usize = 12;
    const DEPTH_CAP: usize = 5;
    let skip = ["models", "projects", "docs"];
    let is_mounted = |p: &str| {
        mounted.iter().any(|m| {
            let m = m.trim_end_matches('/');
            !m.is_empty() && (p == m || p.starts_with(&format!("{m}/")))
        })
    };
    let mut offers = Vec::new();
    // (dir, depth); depth-first so offers group naturally by subtree
    let mut stack = vec![(std::path::PathBuf::from(root), 1usize)];
    while let Some((dir, depth)) = stack.pop() {
        if offers.len() >= MAX_OFFERS {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        let mut direct = 0usize;
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || (depth == 1 && skip.contains(&name.as_str())) {
                continue;
            }
            if p.is_dir() {
                subdirs.push(p);
            } else if is_data_file(&p) && depth > 1 {
                // depth 1 is the data root itself — its loose files are the
                // twin's own (journal, manifest), never an offer
                direct += 1;
            }
        }
        let path = dir.to_string_lossy().into_owned();
        if is_mounted(&path) {
            continue;
        }
        if direct > 0 {
            // this directory mounts as one source
            offers.push(offer(&path, count_data(&dir, 2000)));
        } else if depth >= DEPTH_CAP {
            let n = count_data(&dir, 2000);
            if n > 0 {
                offers.push(offer(&path, n));
            }
        } else {
            subdirs.sort();
            for s in subdirs.into_iter().rev() {
                stack.push((s, depth + 1));
            }
        }
    }
    offers.truncate(MAX_OFFERS);
    offers
}

fn offer(path: &str, files: usize) -> Value {
    serde_json::json!({ "path": path, "files": files })
}

/// Recursive data-file count, capped so a huge tree costs a bounded walk.
fn count_data(dir: &std::path::Path, cap: usize) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_data_file(&p) {
                n += 1;
                if n >= cap {
                    return n;
                }
            }
        }
    }
    n
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

    #[test]
    fn scan_offers_the_right_mount_roots() {
        let root = std::env::temp_dir().join(format!("twin_scan_{}", std::process::id()));
        let mk = |p: &str| std::fs::create_dir_all(root.join(p)).unwrap();
        let put = |p: &str| std::fs::write(root.join(p), "a,b\n1,2\n").unwrap();
        let _ = std::fs::remove_dir_all(&root);
        mk("a/points");
        put("a/points/1.csv");
        put("a/points/2.csv");
        mk("b/deep/nest/well1/traj");
        put("b/deep/nest/well1/traj/1.xml");
        mk("b/deep/nest/well2/traj");
        put("b/deep/nest/well2/traj/1.xml");
        mk("models"); // the twin's own store — never offered
        put("models/cache.json");
        put("journal.jsonl"); // loose root files are the twin's own
        let r = root.to_string_lossy().into_owned();

        let offers = scan_available(&r, &[]);
        let paths: Vec<&str> = offers.iter().filter_map(|o| o["path"].as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("a/points")), "csv dir offered: {paths:?}");
        assert!(
            paths.iter().any(|p| p.ends_with("nest/well1")) && paths.iter().any(|p| p.ends_with("nest/well2")),
            "deep trees offer per-subtree at the cap: {paths:?}"
        );
        assert!(!paths.iter().any(|p| p.contains("models")), "own stores excluded: {paths:?}");
        assert!(!paths.iter().any(|p| p.ends_with("journal.jsonl")), "root files excluded");

        // mounting a root hides it (and everything under it) from the next scan
        let mounted = vec![format!("{r}/a/points"), format!("{r}/b/deep/nest/well1")];
        let after = scan_available(&r, &mounted);
        let paths2: Vec<&str> = after.iter().filter_map(|o| o["path"].as_str()).collect();
        assert!(!paths2.iter().any(|p| p.ends_with("a/points")), "mounted dir still offered: {paths2:?}");
        assert!(!paths2.iter().any(|p| p.ends_with("well1")), "mounted subtree still offered");
        assert!(paths2.iter().any(|p| p.ends_with("well2")), "unmounted sibling must remain");
        let _ = std::fs::remove_dir_all(&root);
    }
}
