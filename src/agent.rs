//! The agent harness (design_doc §12) — thin and substrate-native.
//!
//! There is no bespoke orchestrator (§12.2): the agent is just a principal that
//! perceives the graph as a structured projection (§12.3) and acts by emitting
//! tool-calls that become graph edits (§12.1).  A model call is one governed, slow
//! effect (§9.8); here it is an HTTP call to a LOCAL model (Ollama), driven with our
//! own prompt-based tool protocol so it stays model-agnostic.  The agent runs on its
//! own thread: it greets on startup, then wakes whenever the user steers.
//!
//! Kept deliberately small for Phase 1 — tools: think / say / ask / record_profile /
//! wait.  Reading sources and authoring lenses (§9.9, §4.1) arrive in Phase 2.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::server::Cmd;

/// Ollama endpoint + default model (override the model with `TWIN_MODEL`).
const OLLAMA_ADDR: &str = "127.0.0.1:11434";
const DEFAULT_MODEL: &str = "gemma4:12b";

/// A turn runs at most this many tool-calls before yielding, so a misbehaving model
/// can never loop forever.
const MAX_STEPS: usize = 3;

const SYSTEM_PROMPT: &str = r#"You are a warm, sharp AI that helps ONE user build and operate their "twin" — a living digital twin of their industrial system or operation (a plant, a fleet, a field, a process). You are NOT the twin, and you are not a twin of the user; you are the intelligence that grows and tends the twin together with them. You lead the collaboration; the user steers and supplies domain knowledge you cannot have.

You start blind: you do not yet know who the user is, their role, their goals, or what data feeds the twin. Your first job is to understand them and their operation and grow the twin together, the way a great colleague would on day one — not by interrogating them, but by being genuinely curious and useful.

You act ONLY by emitting exactly ONE JSON object per turn — no prose outside it, no markdown fences. The tools:
- {"tool":"think","args":{"text":"..."}}  private reasoning, shown to the user as your thought process. Think briefly before you act.
- {"tool":"say","args":{"text":"..."}}  a short, warm message to the user.
- {"tool":"ask","args":{"question":"...","options":["...","..."]}}  ask ONE focused question; "options" are optional quick-picks. After you ask, you pause for the user.
- {"tool":"record_profile","args":{"field":"...","value":"..."}}  save a durable fact you learned about the user into the twin (e.g. field "role", "goal", "industry", "data"). Do this the moment you learn something.
- {"tool":"read_source","args":{"path":"/absolute/path/to/file.csv"}}  mount a local data file (CSV, JSON, or JSONL) into the twin. This federates it (no copy); it then appears in your perception under "sources" with its schema and a sample. Use it once the user gives you a path.
- {"tool":"inspect","args":{"source":"<name>"}}  compute quick stats (row count, numeric column ranges) over a mounted source; the result appears in the feed so you can then summarize it for the user. Use it to actually analyze the twin's data before making claims about it.

You perceive the twin as JSON each turn: "profile" (what you know about the user), "sources" (mounted data with schema + sample rows), "skills" (capabilities you have — e.g. obtain-oid pulls real oil-&-gas platform data), and "feed" (the conversation so far). A turn ends when you say or ask; think / record_profile / read_source continue the turn, so you can (e.g.) read_source and THEN say what you found in one turn.

Rules:
- Output ONE valid JSON object only. Nothing before or after it.
- On the very first turn (empty feed), open by briefly introducing yourself and inviting the user to tell you what they work on and what they want to understand or improve — then wait for them.
- Call it "the twin" — the digital, data-driven model of the user's operation (their plant / fleet / field). Never say it is a twin "of you"; it models their system, not the person.
- Be concise and human. No walls of text. One idea at a time.
- Every turn must end by either say-ing or ask-ing the user something. Never do nothing.
- Think at most once before acting. If the last feed item is already your thought, do NOT think again — act.
- Prefer one good question over many. Use options when there are natural choices.
- When you learn who they are or what they want, record_profile it immediately.
- When the user mentions data/files, ask for the path, then read_source it. If the user's latest message already contains a file path, your VERY NEXT action MUST be read_source with that exact path — do not ask again. Once a source is mounted, look at its schema and sample and say what you see and what you could build from it.
- Do not repeat yourself; the conversation so far is given to you — continue it naturally."#;

/// Spawn the agent thread. Returns a wake channel the graph pokes on user input.
pub fn spawn(tx: Sender<Cmd>) -> Sender<()> {
    let (wake_tx, wake_rx) = mpsc::channel::<()>();
    thread::spawn(move || agent_loop(tx, wake_rx));
    wake_tx
}

fn agent_loop(tx: Sender<Cmd>, wake_rx: Receiver<()>) {
    let model = std::env::var("TWIN_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    // The agent itself opens the conversation — a real model turn (with the thinking
    // indicator), not a canned string, so it's clearly the agent talking.
    run_turn(&tx, &model);
    while wake_rx.recv().is_ok() {
        // coalesce a burst of wakes into one turn
        while wake_rx.try_recv().is_ok() {}
        run_turn(&tx, &model);
    }
}

/// One turn: perceive → model → apply a tool, repeated until the agent yields
/// (ask/wait) or hits the step cap. Thoughts are capped at one per turn so the model
/// can't loop on "thinking" without acting.
fn run_turn(tx: &Sender<Cmd>, model: &str) {
    tx.send(Cmd::Status(true)).ok();
    let mut thoughts = 0;
    let mut said = false;
    for _ in 0..MAX_STEPS {
        let ctx = perceive(tx);
        // Deterministic nudge: if the user just handed us a data-file path and nothing
        // is mounted, insist the model call read_source (small models otherwise stall).
        let mut nudge = path_nudge(&ctx).unwrap_or_default();
        let content = match call_model(model, &ctx, &nudge) {
            Ok(c) => c,
            Err(e) => {
                emit(tx, &say_json(&format!("(I couldn't reach my local model — {e}.)")));
                said = true;
                break;
            }
        };
        let tool = extract_json(&content).unwrap_or_else(|| say_json(content.trim()));
        let name = tool_name(&tool);
        eprintln!("[agent] {name}: {}", preview(&tool));
        if name == "think" {
            thoughts += 1;
            if thoughts > 1 {
                continue; // don't spin on thinking; re-prompt for an action
            }
        }
        emit(tx, &tool);
        // A turn ends when the agent addresses the user.
        if name == "say" || name == "ask" {
            said = true;
            break;
        }
        let _ = &mut nudge;
    }
    // guarantee every turn ends with something for the user (never silent)
    if !said {
        emit(tx, &say_json("What would you like to do next?"));
    }
    tx.send(Cmd::Status(false)).ok();
}

/// If the last user message carries a data-file path and no source is mounted yet,
/// return a hard instruction to mount it — coaxing a small local model past the stall.
fn path_nudge(ctx: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(ctx).ok()?;
    if v["sources"].as_array().map(|a| !a.is_empty()).unwrap_or(false) {
        return None; // already have a source
    }
    let feed = v["feed"].as_array()?;
    let last_user = feed
        .iter()
        .rev()
        .find(|it| it["kind"] == "user")
        .and_then(|it| it["text"].as_str())?;
    let looks_like_path = last_user.contains('/')
        && [".csv", ".json", ".jsonl", ".tsv", ".ndjson"]
            .iter()
            .any(|ext| last_user.to_ascii_lowercase().contains(ext));
    if looks_like_path {
        Some(format!(
            "The user's last message contains a data-file path. Your next tool call MUST be read_source with that exact path. Do not say or ask — read_source now. Message: {last_user}"
        ))
    } else {
        None
    }
}

fn perceive(tx: &Sender<Cmd>) -> String {
    let (rtx, rrx) = mpsc::channel::<String>();
    if tx.send(Cmd::Perceive(rtx)).is_err() {
        return String::from("{}");
    }
    rrx.recv().unwrap_or_else(|_| String::from("{}"))
}

fn emit(tx: &Sender<Cmd>, tool_json: &str) {
    tx.send(Cmd::AgentTool(tool_json.to_string())).ok();
}

fn say_json(text: &str) -> String {
    serde_json::json!({ "tool": "say", "args": { "text": text } }).to_string()
}

/// A short one-line preview of a tool-call for dev logging.
fn preview(tool_json: &str) -> String {
    let s: String = tool_json.chars().take(120).collect();
    s.replace('\n', " ")
}

fn tool_name(tool_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(tool_json)
        .ok()
        .and_then(|v| v.get("tool").and_then(|t| t.as_str()).map(String::from))
        .unwrap_or_default()
}

// ---- the model call (a governed slow effect, §9.8) --------------------------

fn call_model(model: &str, ctx: &str, nudge: &str) -> Result<String, String> {
    let extra = if nudge.is_empty() {
        String::new()
    } else {
        format!("\n\n{nudge}")
    };
    let user = format!(
        "Current twin state (JSON):\n{ctx}\n\nEmit your next single tool call as one JSON object.{extra}"
    );
    let body = serde_json::json!({
        "model": model,
        "stream": false,
        // keep the model resident between calls — a cold reload is ~120s on CPU,
        // a warm call ~20s; the agent makes several calls per turn.
        "keep_alive": "10m",
        "options": { "temperature": 0.2 },
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user },
        ],
    })
    .to_string();

    let resp = http_post("/api/chat", &body).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("bad model response: {e}"))?;
    Ok(v["message"]["content"].as_str().unwrap_or("").to_string())
}

/// Minimal HTTP/1.1 POST to the local Ollama, returning the decoded body. No deps —
/// localhost only. Handles Content-Length and chunked transfer-encoding.
fn http_post(path: &str, body: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(OLLAMA_ADDR)?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {OLLAMA_ADDR}\r\nContent-Type: application/json\r\n\
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

/// Extract the first balanced `{...}` object from model output (tolerating stray
/// prose or ```json fences), respecting string literals so braces inside strings
/// don't confuse the scan.
fn extract_json(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let cand = &s[start..=i];
                    return serde_json::from_str::<serde_json::Value>(cand)
                        .ok()
                        .map(|_| cand.to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_object() {
        let out = extract_json(r#"{"tool":"say","args":{"text":"hi"}}"#).unwrap();
        assert!(out.contains("\"say\""));
    }

    #[test]
    fn extracts_from_fenced_prose() {
        let s = "Sure!\n```json\n{\"tool\":\"think\",\"args\":{\"text\":\"a {brace} in text\"}}\n```\ndone";
        let out = extract_json(s).unwrap();
        assert_eq!(tool_name(&out), "think");
    }

    #[test]
    fn none_when_no_json() {
        assert!(extract_json("just talking, no json here").is_none());
    }

    #[test]
    fn dechunk_reassembles() {
        let chunked = "5\r\nhello\r\n1\r\n!\r\n0\r\n\r\n";
        assert_eq!(dechunk(chunked), "hello!");
    }
}
