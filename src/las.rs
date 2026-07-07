//! LAS well-log reader (design_doc §9.9, §9.14) — the boundary side of mounting a
//! Log ASCII Standard file (LAS 1.2/2.0), the lingua franca of wireline log curves.
//!
//! A LAS file is one well's curves sampled along measured depth: header sections
//! (`~Version`, `~Well`, `~Curve`, `~Parameter`) then the `~ASCII` data block.  The
//! reader flattens it to rows — one per depth station, keyed by the curve mnemonics —
//! so depth-indexed series mount exactly like time-indexed ones (§9.14: one node
//! type, keyed differently).  The well's name rides along on every row because the
//! whole point of mounting logs is joining them to everything else about that well.

use serde_json::{Map, Value};

/// Parse LAS text into rows: one object per depth station, columns named by the
/// curve mnemonics, `NULL` values (e.g. -999.25) becoming JSON null.
pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    let mut section = ' ';
    let mut wrap = false;
    let mut null_val: Option<f64> = None;
    let mut well_name = String::new();
    let mut curves: Vec<String> = Vec::new();
    let mut rows: Vec<Value> = Vec::new();
    let mut pending: Vec<Value> = Vec::new(); // wrapped-mode accumulator

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(rest) = t.strip_prefix('~') {
            section = rest.chars().next().unwrap_or(' ').to_ascii_uppercase();
            continue;
        }
        match section {
            'V' | 'W' | 'C' | 'P' => {
                let Some((mnem, unit, data)) = header_line(t) else { continue };
                let _ = unit; // units belong in field annotations, not column names
                match section {
                    'V' if mnem == "WRAP" => wrap = data.eq_ignore_ascii_case("yes"),
                    'W' if mnem == "NULL" => null_val = data.parse().ok(),
                    'W' if mnem == "WELL" => well_name = data.to_string(),
                    'C' => curves.push(mnem.to_string()),
                    _ => {}
                }
            }
            'A' => {
                if curves.is_empty() {
                    return Err("LAS data section before any ~Curve section".into());
                }
                for tok in t.split_whitespace() {
                    let v: f64 = tok
                        .parse()
                        .map_err(|_| format!("LAS data value is not a number: {tok}"))?;
                    let is_null = null_val.map(|n| (v - n).abs() < 1e-9).unwrap_or(false);
                    pending.push(if is_null { Value::Null } else { json_num(v) });
                }
                // unwrapped: one line = one station; wrapped: flush per curve count
                while pending.len() >= curves.len() {
                    if !wrap && pending.len() > curves.len() {
                        return Err(format!(
                            "LAS row has {} values for {} curves",
                            pending.len(),
                            curves.len()
                        ));
                    }
                    let station: Vec<Value> = pending.drain(..curves.len()).collect();
                    let mut obj = Map::new();
                    if !well_name.is_empty() {
                        obj.insert("well".into(), Value::String(well_name.clone()));
                    }
                    for (name, v) in curves.iter().zip(station) {
                        obj.insert(name.clone(), v);
                    }
                    rows.push(Value::Object(obj));
                    if !wrap {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    if rows.is_empty() {
        return Err("no LAS data rows (missing ~ASCII section?)".into());
    }
    Ok(rows)
}

/// Split a LAS header line `MNEM.UNIT   DATA : DESCRIPTION` into its parts.
/// The mnemonic ends at the first dot; the unit runs to the first whitespace;
/// the data is everything up to the LAST colon (well names contain colons rarely,
/// but descriptions follow the last one by spec).
fn header_line(t: &str) -> Option<(&str, &str, &str)> {
    let dot = t.find('.')?;
    let mnem = t[..dot].trim();
    let after = &t[dot + 1..];
    let unit_end = after.find(char::is_whitespace).unwrap_or(after.len());
    let unit = &after[..unit_end];
    let rest = &after[unit_end..];
    let data = match rest.rfind(':') {
        Some(c) => rest[..c].trim(),
        None => rest.trim(),
    };
    Some((mnem, unit, data))
}

/// Keep integers as integers so downstream type inference sees clean columns.
fn json_num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9e15 {
        Value::from(v as i64)
    } else {
        Value::from(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
~Version ---------------------------------------------------
 VERS.   2.0 : CWLS log ASCII Standard
 WRAP.   NO  : One line per depth step
~Well ------------------------------------------------------
 STRT.M      2145.0000 : First reference value
 STOP.M      2146.5000 : Last reference value
 NULL.       -999.25   : Missing value
 WELL.   15/9-F-14     : Well name
~Curve -----------------------------------------------------
 DEPT.M     : Measured depth
 GR  .GAPI  : Gamma ray
 RHOB.G/CM3 : Bulk density
~ASCII -----------------------------------------------------
 2145.0000   55.2000    2.4500
 2145.5000  -999.25     2.4700
 2146.0000   58.1000   -999.25
";

    #[test]
    fn stations_become_rows_with_the_well_name() {
        let rows = parse(SAMPLE).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["well"], "15/9-F-14");
        assert_eq!(rows[0]["DEPT"], Value::from(2145));
        assert_eq!(rows[0]["GR"], Value::from(55.2));
    }

    #[test]
    fn null_sentinel_becomes_json_null() {
        let rows = parse(SAMPLE).unwrap();
        assert_eq!(rows[1]["GR"], Value::Null);
        assert_eq!(rows[2]["RHOB"], Value::Null);
    }

    #[test]
    fn wrapped_mode_reassembles_stations() {
        let wrapped = "\
~V
 VERS. 1.2 :
 WRAP. YES :
~W
 NULL. -999.25 :
 WELL. F-14 :
~C
 DEPT.M :
 GR.GAPI :
 RHOB.G/CM3 :
~A
 2145.0
 55.2  2.45
 2145.5 60.0
 2.50
";
        let rows = parse(wrapped).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["RHOB"], Value::from(2.5));
    }

    #[test]
    fn data_before_curves_is_an_error() {
        assert!(parse("~A\n 2145.0 55.2\n").is_err());
    }
}
