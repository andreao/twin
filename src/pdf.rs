//! PDF extraction (design_doc §9.9) — the boundary side of reading a report or
//! drawing: page text for lenses and embeddings, embedded page images for the
//! OCR path when a page has no text layer (scanned reports).
//!
//! This is deliberately a LOOSE reader, not a conforming PDF parser: it scans for
//! `obj … endobj` regions, decodes FlateDecode streams (zlib — we have our own
//! inflate), and pulls text out of content streams by walking the text operators
//! (Tj / TJ / ' / "). Single-byte encodings only: text drawn through CID fonts
//! without a decodable byte mapping comes out as garbage, which the printable-
//! ratio check catches and reports as "no text layer" — exactly the signal the
//! caller needs to fall back to OCR on the page images instead.

use crate::inflate::zlib;

/// What a PDF yields: page-ish text blocks (one per content stream that draws
/// text, in document order) and embedded JPEG images (DCTDecode, ready for a
/// vision model as-is).
#[derive(Debug)]
pub struct Doc {
    pub pages: Vec<String>,
    pub images: Vec<Vec<u8>>,
}

pub fn extract(bytes: &[u8]) -> Result<Doc, String> {
    if !bytes.starts_with(b"%PDF") {
        return Err("not a PDF (missing %PDF header)".into());
    }
    let mut pages = Vec::new();
    let mut images = Vec::new();
    for (dict, data) in streams(bytes) {
        let flate = dict.contains("/FlateDecode") || dict.contains("/Fl ");
        let is_image = dict.contains("/Image");
        if is_image {
            if dict.contains("/DCTDecode") {
                images.push(data.to_vec()); // already a JPEG
            }
            continue;
        }
        let decoded: Vec<u8> = if flate {
            match zlib(data) {
                Ok(d) => d,
                Err(_) => continue, // e.g. object streams with predictors — not text
            }
        } else {
            data.to_vec()
        };
        if let Some(text) = content_text(&decoded) {
            pages.push(text);
        }
    }
    // an empty Doc is a real outcome, not an error: scanned pages and CAD plots
    // (every letter drawn as line strokes) have no text layer — the caller's
    // signal to render the page and OCR it with a vision model instead
    Ok(Doc { pages, images })
}

/// Every `<< dict >> stream … endstream` region: the dictionary as lossy text
/// (only membership tests are run on it) and the raw stream bytes.
fn streams(bytes: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(s) = find(bytes, i, b"stream") {
        // the dictionary sits between the preceding balanced '<<' and 'stream' —
        // walked backwards balancing nested dicts (/DecodeParms << … >>)
        let dict_start = dict_open(bytes, s).unwrap_or(s.saturating_sub(1));
        let dict = String::from_utf8_lossy(&bytes[dict_start..s]).into_owned();
        // stream data starts after the keyword's EOL
        let mut d = s + b"stream".len();
        if bytes.get(d) == Some(&b'\r') {
            d += 1;
        }
        if bytes.get(d) == Some(&b'\n') {
            d += 1;
        }
        let Some(e) = find(bytes, d, b"endstream") else { break };
        // trailing EOL before 'endstream' belongs to the framing, not the data
        let mut end = e;
        while end > d && (bytes[end - 1] == b'\n' || bytes[end - 1] == b'\r') {
            end -= 1;
        }
        out.push((dict, &bytes[d..end]));
        i = e + b"endstream".len();
    }
    out
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The opening `<<` of the dictionary that ends just before `stream`, skipping
/// over any nested `<< … >>` pairs on the way back.
fn dict_open(bytes: &[u8], stream_kw: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = stream_kw.min(bytes.len());
    while i >= 2 {
        let pair = &bytes[i - 2..i];
        if pair == b">>" {
            depth += 1;
            i -= 2;
        } else if pair == b"<<" {
            if depth <= 1 {
                return Some(i - 2);
            }
            depth -= 1;
            i -= 2;
        } else {
            i -= 1;
        }
    }
    None
}

/// Walk a decoded content stream's text operators; None when the stream draws no
/// text or the bytes decode to garbage (no usable single-byte text layer).
fn content_text(content: &[u8]) -> Option<String> {
    if find(content, 0, b"BT").is_none() {
        return None;
    }
    let mut out = String::new();
    let mut pending: Vec<String> = Vec::new(); // string operands awaiting their operator
    let mut i = 0;
    while i < content.len() {
        match content[i] {
            b'(' => {
                let (s, next) = literal_string(content, i);
                pending.push(s);
                i = next;
            }
            b'<' if content.get(i + 1) != Some(&b'<') => {
                let (s, next) = hex_string(content, i);
                pending.push(s);
                i = next;
            }
            b'%' => {
                // comment to end of line
                while i < content.len() && content[i] != b'\n' {
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() || c == b'\'' || c == b'"' => {
                let start = i;
                while i < content.len()
                    && !content[i].is_ascii_whitespace()
                    && !b"()<>[]/%".contains(&content[i])
                {
                    i += 1;
                }
                match &content[start..i] {
                    b"Tj" | b"TJ" => {
                        for s in pending.drain(..) {
                            out.push_str(&s);
                        }
                        out.push(' ');
                    }
                    b"'" | b"\"" => {
                        out.push('\n');
                        for s in pending.drain(..) {
                            out.push_str(&s);
                        }
                    }
                    b"Td" | b"TD" | b"T*" | b"ET" => {
                        pending.clear();
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                    }
                    _ => pending.clear(),
                }
            }
            _ => i += 1,
        }
    }
    let text: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() < 3 {
        return None;
    }
    // garbage gate: a usable single-byte text layer is overwhelmingly ASCII
    // (accented letters are fine in small doses; CID-font bytes are not)
    let total = text.chars().count();
    let ascii = text.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').count();
    if ascii * 100 < total * 85 {
        return None;
    }
    Some(text)
}

/// A `(…)` literal string with \-escapes and balanced nested parens.
fn literal_string(content: &[u8], open: usize) -> (String, usize) {
    let mut s = String::new();
    let mut depth = 1;
    let mut i = open + 1;
    while i < content.len() && depth > 0 {
        match content[i] {
            b'\\' => {
                i += 1;
                match content.get(i) {
                    Some(b'n') => s.push('\n'),
                    Some(b't') => s.push(' '),
                    Some(b'r') | Some(b'f') | Some(b'b') => {}
                    Some(b'\n') => {} // line continuation
                    Some(d @ b'0'..=b'7') => {
                        // up to three octal digits
                        let mut v = (d - b'0') as u32;
                        for _ in 0..2 {
                            match content.get(i + 1) {
                                Some(d2 @ b'0'..=b'7') => {
                                    v = v * 8 + (d2 - b'0') as u32;
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        if let Some(c) = char::from_u32(v) {
                            s.push(c);
                        }
                    }
                    Some(&c) => s.push(c as char),
                    None => break,
                }
                i += 1;
            }
            b'(' => {
                depth += 1;
                s.push('(');
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth > 0 {
                    s.push(')');
                }
                i += 1;
            }
            c => {
                s.push(c as char);
                i += 1;
            }
        }
    }
    (s, i)
}

/// A `<…>` hex string; 2-digit pairs, odd trailing digit padded with 0.
fn hex_string(content: &[u8], open: usize) -> (String, usize) {
    let mut digits = Vec::new();
    let mut i = open + 1;
    while i < content.len() && content[i] != b'>' {
        if content[i].is_ascii_hexdigit() {
            digits.push(content[i]);
        }
        i += 1;
    }
    if digits.len() % 2 == 1 {
        digits.push(b'0');
    }
    let s = digits
        .chunks(2)
        .filter_map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            char::from_u32(hi * 16 + lo)
        })
        .collect();
    (s, i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal one-page PDF with an uncompressed content stream.
    fn tiny_pdf(content: &str) -> Vec<u8> {
        format!(
            "%PDF-1.4\n1 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\ntrailer\n",
            content.len(),
            content
        )
        .into_bytes()
    }

    #[test]
    fn text_operators_yield_lines() {
        let doc = extract(&tiny_pdf(
            "BT /F1 12 Tf 72 700 Td (Final Well Report) Tj 0 -20 Td (Well: 15/9-F-14) Tj ET",
        ))
        .unwrap();
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].contains("Final Well Report"), "{}", doc.pages[0]);
        assert!(doc.pages[0].contains("15/9-F-14"));
    }

    #[test]
    fn tj_arrays_and_escapes() {
        let doc = extract(&tiny_pdf(
            r"BT [(Hu) -20 (gin)] TJ (Fm. \(top\)) Tj <48454C4C4F> Tj ET",
        ))
        .unwrap();
        let t = &doc.pages[0];
        assert!(t.contains("Hugin"), "{t}");
        assert!(t.contains("Fm. (top)"), "{t}");
        assert!(t.contains("HELLO"), "{t}");
    }

    #[test]
    fn embedded_jpeg_is_surfaced_for_ocr() {
        let jpeg = [0xFFu8, 0xD8, 0xFF, 0xE0, 1, 2, 3, 0xFF, 0xD9];
        let mut pdf = b"%PDF-1.4\n5 0 obj\n<< /Subtype /Image /Filter /DCTDecode /Length 9 >>\nstream\n".to_vec();
        pdf.extend(jpeg);
        pdf.extend(b"\nendstream\nendobj\n");
        let doc = extract(&pdf).unwrap();
        assert_eq!(doc.images.len(), 1);
        assert_eq!(doc.images[0], jpeg);
        assert!(doc.pages.is_empty());
    }

    #[test]
    fn not_a_pdf_is_a_readable_error() {
        assert!(extract(b"hello").unwrap_err().contains("PDF"));
    }

    #[test]
    fn binary_garbage_text_is_rejected() {
        // a "text" stream of CID-font bytes: mostly non-printable after decode
        let bytes: Vec<u8> = (0u8..=255).cycle().take(400).collect();
        let mut content = b"BT <".to_vec();
        for b in &bytes {
            content.extend(format!("{b:02x}").bytes());
        }
        content.extend(b"> Tj ET");
        let mut pdf = b"%PDF-1.4\n1 0 obj\n<< >>\nstream\n".to_vec();
        pdf.extend(&content);
        pdf.extend(b"\nendstream\nendobj\n");
        let r = extract(&pdf);
        // either no pages at all, or the garbage was filtered out
        assert!(r.is_err() || r.unwrap().pages.is_empty());
    }
}
