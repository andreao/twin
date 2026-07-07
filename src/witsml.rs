//! WITSML and EDM XML readers (design_doc §9.9, §8.1) — the boundary side of
//! mounting drilling data: daily drill reports, log curves, trajectories, BHA runs
//! (WITSML object documents) and Landmark EDM engineering exports.
//!
//! The flattening is GENERIC, not per-schema: a WITSML document is a plural
//! container of objects, and each object either IS a row (a bhaRun) or contains one
//! repeated group that is the real row grain (a drillReport's activities, a
//! trajectory's stations).  The reader finds the repeated group and explodes it,
//! carrying the object's scalar context onto every row — so the linking lenses
//! downstream (§8.1: joins on well names, depth intervals, report chains) get flat
//! rows with their context attached, whatever the object type.  An EDM export is
//! the degenerate case: thousands of attribute-only elements in one file, tag name
//! = table family.

use crate::source::infer_value;
use crate::xml::{self, Elem};
use serde_json::{Map, Value};

/// WITSML boilerplate containers that carry audit metadata, not data.
const SKIP: [&str; 3] = ["commonData", "customData", "documentInfo"];

/// Parse a drilling XML document (WITSML object doc or EDM export) into rows.
pub fn parse(text: &str) -> Result<Vec<Value>, String> {
    let root = xml::parse(text)?;
    if root.local().eq_ignore_ascii_case("logs") {
        return parse_logs(&root);
    }
    if edm_like(&root) {
        return parse_edm(&root);
    }
    // a WITSML plural container: one object per child; otherwise treat the root
    // itself as a single object document
    let objects: Vec<&Elem> = root.elems().filter(|e| !skip(e)).collect();
    let rows: Vec<Value> = if objects.is_empty() {
        explode(&root)
    } else {
        objects.iter().flat_map(|o| explode(o)).collect()
    };
    if rows.is_empty() {
        return Err(format!(
            "XML parsed but nothing row-shaped found under <{}>",
            root.local()
        ));
    }
    Ok(rows)
}

fn skip(e: &Elem) -> bool {
    SKIP.contains(&e.local())
}

/// A scalar element carries text and no element children.
fn is_scalar(e: &Elem) -> bool {
    e.elems().next().is_none()
}

/// An EDM export: most children are attribute-only leaf elements with several
/// distinct tag families (CD_WELL, CD_ASSEMBLY, …), data in attributes.
fn edm_like(root: &Elem) -> bool {
    let kids: Vec<&Elem> = root.elems().collect();
    if kids.len() < 3 {
        return false;
    }
    let attr_leaves = kids
        .iter()
        .filter(|e| !e.attrs.is_empty() && is_scalar(e) && e.text().is_empty())
        .count();
    attr_leaves * 5 >= kids.len() * 4 // ≥80%
}

/// EDM rows: every attribute-only element anywhere in the export, with its tag
/// name as the `family` column — the 13-tag-family monolith becomes one source
/// the lenses can filter by family.
fn parse_edm(root: &Elem) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    let mut stack: Vec<&Elem> = root.elems().collect();
    while let Some(e) = stack.pop() {
        if !e.attrs.is_empty() && is_scalar(e) {
            let mut obj = Map::new();
            obj.insert("family".into(), Value::String(e.local().to_string()));
            for (k, v) in &e.attrs {
                obj.insert(strip_prefix(k).to_string(), infer_value(v));
            }
            rows.push(Value::Object(obj));
        }
        stack.extend(e.elems());
    }
    if rows.is_empty() {
        return Err("EDM export has no attribute rows".into());
    }
    rows.reverse(); // stack order back to document order
    Ok(rows)
}

/// One WITSML object → rows.  Scalars (and attributes) become context columns; if
/// the object has a repeated child group (activities, stations, geology
/// intervals), each member becomes a row carrying the context — member fields win
/// a name collision because they are the more specific fact.
fn explode(obj: &Elem) -> Vec<Value> {
    let mut context = Map::new();
    flatten(obj, "", &mut context);
    match repeated_group(obj) {
        Some(name) => obj
            .find_all(&name)
            .into_iter()
            .map(|member| {
                let mut row = context.clone();
                flatten(member, "", &mut row);
                Value::Object(row)
            })
            .collect(),
        None if context.is_empty() => Vec::new(),
        None => vec![Value::Object(context)],
    }
}

/// The most frequent direct child element name occurring at least twice.
fn repeated_group(obj: &Elem) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in obj.elems() {
        if skip(e) || is_scalar(e) {
            continue;
        }
        match counts.iter_mut().find(|(n, _)| n == e.local()) {
            Some((_, c)) => *c += 1,
            None => counts.push((e.local().to_string(), 1)),
        }
    }
    counts
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .max_by_key(|(_, c)| *c)
        .map(|(n, _)| n)
}

/// Flatten an element's attributes and scalar descendants into dotted columns,
/// stopping at repeated groups (they are rows, not columns) and boilerplate.
fn flatten(e: &Elem, prefix: &str, out: &mut Map<String, Value>) {
    let repeated = repeated_group(e);
    for (k, v) in &e.attrs {
        let local = strip_prefix(k);
        if local.starts_with("xmlns") || local == "uom" || local == "version" {
            continue;
        }
        out.insert(col(prefix, local), infer_value(v));
    }
    for child in e.elems() {
        if skip(child) || Some(child.local().to_string()) == repeated {
            continue;
        }
        if is_scalar(child) {
            let t = child.text();
            if !t.is_empty() {
                out.insert(col(prefix, child.local()), infer_value(&t));
            }
        } else {
            flatten(child, &col(prefix, child.local()), out);
        }
    }
}

fn col(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

fn strip_prefix(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// WITSML logs carry their rows as CSV text in `<logData><data>` lines, with the
/// column order declared by `<logCurveInfo>` mnemonics — the wireline cousin of
/// the LAS layout, mounted the same shape (§9.14).
fn parse_logs(root: &Elem) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for log in root.find_all("log") {
        let well = log
            .find("nameWell")
            .map(|e| e.text())
            .unwrap_or_default();
        let mut curves: Vec<String> = log
            .find_all("logCurveInfo")
            .iter()
            .filter_map(|c| c.find("mnemonic").map(|m| m.text()))
            .collect();
        if curves.is_empty() {
            // 1.3 puts the order in indexCurve + columnIndex; fall back to uid attrs
            curves = log
                .find_all("logCurveInfo")
                .iter()
                .filter_map(|c| c.attr("uid").map(String::from))
                .collect();
        }
        let data = match log.find("logData") {
            Some(d) => d,
            None => continue,
        };
        for line in data.find_all("data") {
            let cells: Vec<String> = line.text().split(',').map(|s| s.trim().to_string()).collect();
            let mut obj = Map::new();
            if !well.is_empty() {
                obj.insert("well".into(), Value::String(well.clone()));
            }
            for (name, cell) in curves.iter().zip(cells) {
                obj.insert(name.clone(), infer_value(&cell));
            }
            rows.push(Value::Object(obj));
        }
    }
    if rows.is_empty() {
        return Err("no <logData> rows in WITSML logs document".into());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRILL_REPORTS: &str = r#"<?xml version="1.0"?>
<drillReports xmlns="http://www.witsml.org/schemas/1series" version="1.4.1.1">
  <drillReport uidWell="W-14" uidWellbore="B-14" uid="dr1">
    <nameWell>NO 15/9-F-14</nameWell>
    <dTimStart>2008-03-12T00:00:00Z</dTimStart>
    <mdReport uom="m">2145.5</mdReport>
    <activity>
      <dTimStart>2008-03-12T01:00:00Z</dTimStart>
      <proprietaryCode>DRL</proprietaryCode>
      <comments>Drilling 12 1/4" section</comments>
    </activity>
    <activity>
      <dTimStart>2008-03-12T07:30:00Z</dTimStart>
      <proprietaryCode>CIRC</proprietaryCode>
      <comments>Circulating bottoms up</comments>
    </activity>
    <commonData><dTimCreation>x</dTimCreation></commonData>
  </drillReport>
</drillReports>"#;

    #[test]
    fn drill_report_activities_become_rows_with_context() {
        let rows = parse(DRILL_REPORTS).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["nameWell"], "NO 15/9-F-14");
        assert_eq!(rows[0]["uidWell"], "W-14");
        assert_eq!(rows[0]["mdReport"], Value::from(2145.5));
        assert_eq!(rows[0]["proprietaryCode"], "DRL");
        assert_eq!(rows[1]["proprietaryCode"], "CIRC");
        // the activity's own dTimStart wins the collision with the report's
        assert_eq!(rows[1]["dTimStart"], "2008-03-12T07:30:00Z");
        assert!(rows[0].get("dTimCreation").is_none(), "commonData is skipped");
    }

    #[test]
    fn trajectory_stations_become_rows() {
        let xml = r#"<trajectorys version="1.4.1.1">
  <trajectory uidWell="W-14"><nameWell>NO 15/9-F-14</nameWell>
    <trajectoryStation uid="s1"><md uom="m">150</md><incl uom="dega">0.2</incl><azi uom="dega">310</azi></trajectoryStation>
    <trajectoryStation uid="s2"><md uom="m">300</md><incl uom="dega">1.5</incl><azi uom="dega">312</azi></trajectoryStation>
  </trajectory>
</trajectorys>"#;
        let rows = parse(xml).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["md"], Value::from(150i64));
        assert_eq!(rows[1]["incl"], Value::from(1.5));
        assert_eq!(rows[1]["nameWell"], "NO 15/9-F-14");
    }

    #[test]
    fn bha_run_without_repeats_is_one_row() {
        let xml = r#"<bhaRuns><bhaRun uid="b1"><nameWell>F-14</nameWell>
          <status><mdHole uom="m">2500</mdHole></status></bhaRun></bhaRuns>"#;
        let rows = parse(xml).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status.mdHole"], Value::from(2500i64));
    }

    #[test]
    fn witsml_log_data_lines_become_rows() {
        let xml = r#"<logs><log uid="L1"><nameWell>F-14</nameWell>
          <logCurveInfo uid="DEPT"><mnemonic>DEPT</mnemonic></logCurveInfo>
          <logCurveInfo uid="ROP"><mnemonic>ROP</mnemonic></logCurveInfo>
          <logData><data>2145.0, 32.5</data><data>2145.5, 30.1</data></logData>
        </log></logs>"#;
        let rows = parse(xml).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["DEPT"], Value::from(2145.0));
        assert_eq!(rows[1]["ROP"], Value::from(30.1));
        assert_eq!(rows[0]["well"], "F-14");
    }

    #[test]
    fn edm_export_rows_carry_their_family() {
        let xml = r#"<export>
          <CD_WELL well_id="W1" well_legal_name="NO 15/9-F-14" />
          <CD_WELLBORE wellbore_id="WB1" well_id="W1" />
          <CD_ASSEMBLY assembly_id="A1" wellbore_id="WB1" hole_size="12.25" />
        </export>"#;
        let rows = parse(xml).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["family"], "CD_WELL");
        assert_eq!(rows[2]["hole_size"], Value::from(12.25));
    }
}
