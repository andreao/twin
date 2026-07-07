//! Document boundary adapter (design_doc §9.9) — the host side of the agent's
//! `read_document` tool: get the TEXT out of a mounted document, whatever it takes.
//!
//! The ladder, cheapest first:
//!   1. the PDF's own text layer (src/pdf.rs) — free;
//!   2. embedded page images (scanned reports) → OCR by the local vision model;
//!   3. no text, no images (CAD plots drawing letters as line strokes) → rasterize
//!      the page with the OS's renderer (`sips` on macOS — a host capability like
//!      curl-for-TLS) and OCR that.
//! The result is write-through cached (§7) under data/models/docs/, so journal
//! replay and re-reads never call the model again.

use crate::{ollama, pdf};
use serde_json::Value;

const CACHE_DIR: &str = "data/models/docs";
/// OCR is minutes per page on CPU — bound the ladder's cost per document.
const MAX_OCR_PAGES: usize = 3;

const OCR_PROMPT: &str = "You are reading one page of an engineering document \
(a well report, P&ID, or technical drawing). Transcribe ALL legible text on the \
page: title block fields, equipment tags, labels, notes, table contents. Output \
plain text only, one item per line, no commentary.";

/// A document read: one row per page/block, ready to mount as a derived source.
#[derive(Debug)]
pub struct DocText {
    pub rows: Vec<Value>,
    pub ocr_pages: usize,
    pub cached: bool,
}

/// Find a mounted document by basename across the project data directories.
pub fn find(name: &str) -> Option<String> {
    let base = name.rsplit('/').next().unwrap_or(name); // basename only — no traversal
    if base.is_empty() {
        return None;
    }
    let dirs = std::fs::read_dir("data").ok()?;
    for d in dirs.flatten() {
        let p = d.path().join("files").join(base);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// Read a document's text through the ladder, consulting the cache first.
pub fn read(name: &str) -> Result<DocText, String> {
    let base = name.rsplit('/').next().unwrap_or(name).to_string();
    let cache = format!("{CACHE_DIR}/{base}.json");
    if let Ok(text) = std::fs::read_to_string(&cache) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(rows) = v["rows"].as_array() {
                return Ok(DocText {
                    rows: rows.clone(),
                    ocr_pages: v["ocr_pages"].as_u64().unwrap_or(0) as usize,
                    cached: true,
                });
            }
        }
    }
    let path = find(&base).ok_or_else(|| format!("no document named {base} in the data directories"))?;
    let lower = base.to_ascii_lowercase();
    let (rows, ocr_pages) = if lower.ends_with(".pdf") {
        read_pdf(&path)?
    } else if lower.ends_with(".txt") || lower.ends_with(".md") {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        (vec![row(1, &text, "text")], 0)
    } else {
        return Err(format!("{base}: only .pdf/.txt/.md documents can be read"));
    };
    if rows.is_empty() {
        return Err(format!(
            "{base}: no text layer, no embedded images, and no page renderer available — nothing to read"
        ));
    }
    let _ = std::fs::create_dir_all(CACHE_DIR);
    let _ = std::fs::write(
        &cache,
        serde_json::json!({ "rows": rows, "ocr_pages": ocr_pages }).to_string(),
    );
    Ok(DocText { rows, ocr_pages, cached: false })
}

fn row(page: usize, text: &str, via: &str) -> Value {
    serde_json::json!({ "page": page, "text": text.trim(), "via": via })
}

fn read_pdf(path: &str) -> Result<(Vec<Value>, usize), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let doc = pdf::extract(&bytes)?;
    if !doc.pages.is_empty() {
        let rows = doc
            .pages
            .iter()
            .enumerate()
            .map(|(i, t)| row(i + 1, t, "text-layer"))
            .collect();
        return Ok((rows, 0));
    }
    // no text layer — OCR embedded page images, or a rasterized page as last resort
    let images = if !doc.images.is_empty() {
        doc.images
    } else {
        rasterize(path).into_iter().collect()
    };
    let model = ollama::vision_model();
    let mut rows = Vec::new();
    let mut ocr_pages = 0;
    for (i, img) in images.iter().take(MAX_OCR_PAGES).enumerate() {
        let text = ollama::chat_with_images(&model, OCR_PROMPT, std::slice::from_ref(img))
            .map_err(|e| format!("OCR failed — {e}"))?;
        if !text.trim().is_empty() {
            rows.push(row(i + 1, &text, "ocr"));
            ocr_pages += 1;
        }
    }
    Ok((rows, ocr_pages))
}

/// Render a PDF's first page to PNG bytes with the OS renderer, if one exists.
/// (`sips` ships with macOS; elsewhere this rung of the ladder is simply absent.)
fn rasterize(path: &str) -> Option<Vec<u8>> {
    let out = std::env::temp_dir().join(format!(
        "twin_page_{}.png",
        path.rsplit('/').next().unwrap_or("doc").replace('.', "_")
    ));
    let ok = std::process::Command::new("sips")
        .args(["-s", "format", "png", "-Z", "2000", path, "--out"])
        .arg(&out)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    let bytes = std::fs::read(&out).ok();
    let _ = std::fs::remove_file(&out);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_refuses_traversal_and_misses() {
        assert_eq!(find("../../etc/passwd"), find("passwd"));
        assert!(find("definitely-not-a-real-file.pdf").is_none());
    }

    #[test]
    fn a_missing_document_is_a_readable_error() {
        let e = read("something-unmounted.docx").unwrap_err();
        assert!(e.contains("no document named"), "{e}");
    }
}
