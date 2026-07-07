//! Local-model host client (design_doc §9.8) — every model call is one governed,
//! slow effect: an HTTP call to the LOCAL Ollama host.  Chat drives the agent,
//! vision reads document pages the PDF text layer can't give us, and embed powers
//! semantic search.  Hand-rolled HTTP/1.1 over TcpStream (localhost only, no TLS
//! needed) and hand-rolled base64 — still zero new dependencies.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const ADDR: &str = "127.0.0.1:11434";

/// The default agent model; `TWIN_MODEL` overrides.
pub const DEFAULT_MODEL: &str = "gemma4:12b";

/// The chat model in effect for this process.
pub fn model() -> String {
    std::env::var("TWIN_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// The vision model for OCR: `TWIN_VISION_MODEL`, else the chat model (the gemma
/// line is multimodal, so one local model serves both by default).
pub fn vision_model() -> String {
    std::env::var("TWIN_VISION_MODEL").unwrap_or_else(|_| model())
}

/// The embedding model: `TWIN_EMBED_MODEL`, else the gemma-family embedder.
pub fn embed_model() -> String {
    std::env::var("TWIN_EMBED_MODEL").unwrap_or_else(|_| "embeddinggemma".to_string())
}

/// One chat turn: system + user → the model's text.
pub fn chat(model: &str, system: &str, user: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": model,
        "stream": false,
        // keep the model resident between calls — a cold reload is ~120s on CPU
        "keep_alive": "10m",
        "options": { "temperature": 0.2 },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    })
    .to_string();
    let v = post_json("/api/chat", &body)?;
    Ok(v["message"]["content"].as_str().unwrap_or("").to_string())
}

/// A vision turn: prompt + images (raw JPEG/PNG bytes, base64'd on the wire).
pub fn chat_with_images(model: &str, prompt: &str, images: &[Vec<u8>]) -> Result<String, String> {
    let imgs: Vec<String> = images.iter().map(|b| base64(b)).collect();
    let body = serde_json::json!({
        "model": model,
        "stream": false,
        "keep_alive": "10m",
        "options": { "temperature": 0.0 },
        "messages": [{ "role": "user", "content": prompt, "images": imgs }],
    })
    .to_string();
    let v = post_json("/api/chat", &body)?;
    Ok(v["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Embed a batch of texts; one vector per input, in order.
pub fn embed(model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let body = serde_json::json!({ "model": model, "input": inputs }).to_string();
    let v = post_json("/api/embed", &body)?;
    let arrs = v["embeddings"]
        .as_array()
        .ok_or_else(|| format!("no embeddings in response: {}", preview(&v)))?;
    Ok(arrs
        .iter()
        .map(|a| {
            a.as_array()
                .map(|xs| xs.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
                .unwrap_or_default()
        })
        .collect())
}

fn preview(v: &serde_json::Value) -> String {
    v.to_string().chars().take(120).collect()
}

fn post_json(path: &str, body: &str) -> Result<serde_json::Value, String> {
    let resp = post(path, body).map_err(|e| format!("local model host unreachable — {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("bad model response: {e}"))?;
    if let Some(err) = v["error"].as_str() {
        return Err(err.to_string());
    }
    Ok(v)
}

/// Minimal HTTP/1.1 POST to the local Ollama, returning the decoded body.
/// Localhost only; generous read timeout because a cold vision model on CPU is slow.
pub fn post(path: &str, body: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(ADDR)?;
    s.set_read_timeout(Some(Duration::from_secs(600)))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {ADDR}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    s.write_all(req.as_bytes())?;
    s.flush()?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    Ok(decode_http_body(&raw))
}

fn decode_http_body(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = match text.split_once("\r\n\r\n") {
        Some(hb) => hb,
        None => return String::new(),
    };
    if headers.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        dechunk(body)
    } else {
        body.to_string()
    }
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    loop {
        let nl = match body.find("\r\n") {
            Some(i) => i,
            None => break,
        };
        let size = usize::from_str_radix(body[..nl].trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = nl + 2;
        let end = start + size;
        if end > body.len() {
            break;
        }
        out.push_str(&body[start..end]);
        body = &body[(end + 2).min(body.len())..]; // skip trailing CRLF
    }
    out
}

/// Standard base64 (RFC 4648, with padding) — what the Ollama images field wants.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn dechunk_reassembles() {
        let chunked = "5\r\nhello\r\n1\r\n!\r\n0\r\n\r\n";
        assert_eq!(dechunk(chunked), "hello!");
    }
}
