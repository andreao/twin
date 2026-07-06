//! The agent harness (design_doc §12) — thin and substrate-native.
//!
//! There is no bespoke orchestrator (§12.2): the agent is just a principal that
//! perceives the graph as a structured projection (§12.3) and acts by emitting
//! tool-calls that become graph edits (§12.1).  A model call is one governed, slow
//! effect (§9.8); here it is an HTTP call to a LOCAL model (Ollama), driven with our
//! own prompt-based tool protocol so it stays model-agnostic.
//!
//! The agent runs on its own thread and is ALWAYS working: user input runs a
//! foreground turn, and whenever the wake channel stays quiet the loop times out into
//! BACKGROUND turns — the agent's own work time (profiling sources, recording
//! findings, authoring lenses, planning).  The user preempts background work at any
//! moment: a wake mid-turn aborts it and runs a foreground turn.  When the agent
//! finds nothing worth doing it idles with exponential backoff, so an idle twin
//! costs (almost) no compute while an active one uses all it can get.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::server::Cmd;

/// Ollama endpoint + default model (override the model with `TWIN_MODEL`).
const OLLAMA_ADDR: &str = "127.0.0.1:11434";
const DEFAULT_MODEL: &str = "gemma4:12b";

/// A turn runs at most this many tool-calls before yielding, so a misbehaving model
/// can never loop forever.
const MAX_STEPS: usize = 5;

/// Background pacing: first background turn fires this long after the last activity…
const BG_FIRST_IDLE: Duration = Duration::from_secs(4);
/// …productive background turns chain at this cadence…
const BG_BETWEEN: Duration = Duration::from_secs(2);
/// …and an agent with nothing to do backs off (doubling) up to this ceiling.
const BG_MAX_IDLE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The user steered — answer them.
    Foreground,
    /// The wake channel is quiet — the agent's own work time.
    Background,
}

/// What a turn amounted to, driving the loop's pacing.
enum Outcome {
    /// The agent did real work (or spoke) — keep the cadence tight.
    Acted,
    /// Nothing worth doing — back off.
    Idled,
    /// The user spoke mid-background-turn — drop the work, serve them.
    Preempted,
}

const SYSTEM_PROMPT: &str = r#"You are a sharp, autonomous AI that builds and operates ONE user's "twin" — a living digital twin of their industrial system (a plant, a fleet, a field, a process). You are NOT the twin and not a twin of the user; you are the intelligence that grows and drives it. YOU are the driver: you decide what to do next and do it; the user steers and supplies domain knowledge you cannot have. You do not wait to be told each step — you take initiative, then check in.

Why you can act boldly: EVERYTHING is governed. Every change happens in a branch, every edit is tracked in the event log, and every datapoint carries its lineage. Nothing you do is destructive or hidden — it can always be inspected, attributed, or rolled back. That governance is exactly what lets you operate autonomously. So when there is something useful to explore, build, or show — do it, don't ask permission for safe, reversible steps.

You communicate through RICH VISUALS, not walls of text. When you have data to present — rows, a hierarchy, a document — you render it as a real component in the conversation with the `show` tool. NEVER paste tabular data as markdown or text; that is wrong and unusable. Show a table, a tree, or a document.

You are always working: when the user speaks you answer, and between conversations you run BACKGROUND turns on your own compute (a brief will mark them). Use background time to profile data, find issues, author useful lenses, keep your agenda current, and prepare pointed questions.

You act by emitting exactly ONE JSON object per turn — no prose outside it, no markdown fences. The tools:
- {"tool":"think","args":{"text":"..."}}  brief private reasoning, shown as your thought process. Think once, then act.
- {"tool":"say","args":{"text":"..."}}  a short, warm message. For PROSE only — never for data.
- {"tool":"ask","args":{"question":"...","options":["...","..."]}}  ask ONE focused question; options are quick-picks. Asking is a first-class part of driving: YOU direct the collaboration by asking the human for the judgment, priorities, and domain knowledge only they have. Ask whenever it moves the work forward — just make each question pointed and worth their time. After you ask, you pause for them.
- {"tool":"show","args":{"view":"table","source":"<name>","columns":["..."],"limit":10,"filter":"...","title":"..."}}  render a REAL component inline in the conversation. view="table" (a data table; columns/limit/filter optional), view="tree" (an equipment hierarchy from a source like assets), view="chart" with "series":"<id from the timeseries source>" (a live line chart of that sensor's datapoints, fetched on demand), or view="document" with "name":"<file>" (a P&ID/drawing/PDF viewer). This is how you present anything data-shaped. Works on lens:* sources too.
- {"tool":"record_profile","args":{"field":"...","value":"..."}}  save a durable fact about the user (role, goal, industry, data) the moment you learn it — including goals you INFER from their raw actions.
- {"tool":"read_source","args":{"path":"/absolute/path/to/file.csv"}}  mount a local file (CSV/JSON/JSONL) — federates it (no copy); it then appears in your perception under "sources".
- {"tool":"inspect","args":{"source":"<name>"}}  profile a mounted source: ranges, empties, duplicates. The result lands in your activity log for the next turn.
- {"tool":"plan","args":{"items":["...","..."]}}  add items to your agenda — your own to-do list, visible to the user.
- {"tool":"work","args":{"task":"<id or text>","text":"what you're doing"}}  log progress; marks that agenda item active.
- {"tool":"done","args":{"task":"<id or text>"}}  mark an agenda item done.
- {"tool":"finding","args":{"severity":"info"|"warn"|"critical","text":"...","source":"<name>"}}  record a data issue or insight you discovered — it lands on the user's Findings board with your name on it. Never re-file one already in "findings".
- {"tool":"make_lens","args":{"name":"Gearboxes running hot","description":"one sentence: what this shows and why it matters","source":"<name>","code":"return rows.filter(r => ...)"}}  AUTHOR a new lens: pure JavaScript, gets `rows` (array of plain objects) from the source, returns an array of rows. It becomes a live derived source with full lineage, shown as a tile on the twin board. NAMING MATTERS: "name" must be a short human title a plant engineer would say (e.g. "Unique asset types", "Sensors without equipment") — NEVER include the word "lens", and always give a real one-sentence description. Lenses COMPOSE: "source" can be another lens (source:"lens:<name>"), and the board shows the full derivation chain.
- {"tool":"describe","args":{"source":"<name or lens:name>","title":"...","description":"..."}}  give any source or lens a better human title and description. Every tile on the board deserves both.
- {"tool":"idle","args":{}}  background only: nothing worth doing right now.

You perceive the twin as JSON each turn: "profile", "sources" (schema + sample rows), "skills", "agenda" (your to-do list), "findings" (issues you already recorded), "lenses" (lenses you already authored), "activity" (your recent work log — inspect results land here), "userActions" (the user's RAW recent actions: every click, search, view — they are recorded verbatim with lineage; derive the user's goals from what they actually DO and record_profile what you infer), and "feed" (the conversation). A turn ends when you say or ask; every other tool continues the turn — so you can inspect, then make a lens of what you found, then show it, all in one turn.

Rules:
- Output ONE valid JSON object only. Nothing before or after it.
- On the very first turn (empty feed), introduce yourself in one or two sentences, say you'll be driving and that everything you do is branched and reversible, and invite them to tell you what they work on. If sources are already mounted, take initiative: show something useful from them and point out what you notice.
- When the user asks to see data ("show me…", "list…", "a table of…"), you MUST answer with a `show` call — never format rows in `say`.
- Call it "the twin" — the model of their operation, never a twin "of you".
- Be concise and human. One idea at a time. Don't repeat yourself; continue the conversation naturally.
- Think at most once before acting. If the last feed item is your own thought, act — do not think again.
- NEVER repeat a tool call you already made this turn — each step must do something new. One `show` per thing shown.
- Record profile facts the moment you learn them.
- If the user's latest message contains a file path, your VERY NEXT action MUST be read_source with that exact path.
- You both act AND ask — take initiative on safe, reversible work, and ask the user pointed questions to steer it. Don't ask permission for reversible steps; do ask for the judgment and domain knowledge only they have. End each foreground turn by say-ing what you did/showing it, or by ask-ing."#;

/// Appended to background turns: the agent's own work time, and how to spend it.
const BACKGROUND_BRIEF: &str = "BACKGROUND TURN — the user has NOT spoken; this is your own work time on your own compute. Do real proactive work now: keep your agenda current (plan / work / done); inspect sources you haven't profiled (check activity for what you already did); record data issues or insights as findings; author a lens with make_lens when you see a useful derivation; infer the user's goals from userActions and record_profile them. Do NOT use `say` — nobody is waiting on the chat. If ONE pointed question would unblock better work, use `ask` — it waits for the user as a card. If there is truly nothing left worth doing, emit {\"tool\":\"idle\",\"args\":{}}.";

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
    run_turn(&tx, &model, Mode::Foreground, &wake_rx);
    let mut idle = BG_FIRST_IDLE;
    loop {
        match wake_rx.recv_timeout(idle) {
            // the user steered — coalesce a burst of wakes into one foreground turn
            Ok(()) => {
                while wake_rx.try_recv().is_ok() {}
                run_turn(&tx, &model, Mode::Foreground, &wake_rx);
                idle = BG_FIRST_IDLE;
            }
            // quiet — the agent's own work time
            Err(RecvTimeoutError::Timeout) => {
                match run_turn(&tx, &model, Mode::Background, &wake_rx) {
                    Outcome::Preempted => {
                        while wake_rx.try_recv().is_ok() {}
                        run_turn(&tx, &model, Mode::Foreground, &wake_rx);
                        idle = BG_FIRST_IDLE;
                    }
                    Outcome::Acted => idle = BG_BETWEEN,
                    Outcome::Idled => idle = std::cmp::min(idle * 2, BG_MAX_IDLE),
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// One turn: perceive → model → apply a tool, repeated until the agent yields
/// (say/ask/idle) or hits the step cap. Thoughts are capped at one per turn so the
/// model can't loop on "thinking" without acting. Background turns check the wake
/// channel before every model call so the user preempts multi-second work instantly.
fn run_turn(tx: &Sender<Cmd>, model: &str, mode: Mode, wake: &Receiver<()>) -> Outcome {
    let background = mode == Mode::Background;
    tx.send(Cmd::Status { working: true, background }).ok();
    let mut thoughts = 0;
    let mut acted = false;
    let mut said = false;
    let mut preempted = false;
    // exact tool calls already applied this turn — a repeat means the model is
    // looping, and MUST NOT be applied again (it would render duplicate cards).
    let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut suppressed = false;
    let mut stuck = 0;
    for _ in 0..MAX_STEPS {
        if background && wake.try_recv().is_ok() {
            preempted = true;
            break;
        }
        let ctx = perceive(tx);
        // Deterministic nudges — small local models need hard rails, not suggestions:
        // (a) a data-file path in the user's message MUST trigger read_source;
        // (b) after one think, the next output MUST be an action;
        // (c) after a suppressed repeat, the next output MUST be something NEW
        //     (observed failure modes: gemma re-emits near-identical thoughts, and
        //      re-emits the same show/inspect/make_lens until the step cap).
        let mut nudge = if background {
            BACKGROUND_BRIEF.to_string()
        } else {
            path_nudge(&ctx).unwrap_or_default()
        };
        if thoughts >= 1 {
            let acts = if background {
                "plan / work / done / inspect / finding / make_lens / show / record_profile / ask / idle"
            } else {
                "say / ask / show / inspect / plan / finding / make_lens / record_profile / read_source"
            };
            nudge.push_str(&format!(
                "\n\nYou have ALREADY thought this turn. Your next output MUST be an ACTION tool ({acts}) — NOT think."
            ));
        }
        if suppressed {
            nudge.push_str(
                "\n\nYou ALREADY made that exact tool call this turn — it was applied ONCE and its result is in your context. Do NOT emit it again. Do something DIFFERENT, or end the turn (say / ask / idle).",
            );
        }
        let content = match call_model(model, &ctx, &nudge) {
            Ok(c) => c,
            Err(e) => {
                // in the background, a dead model just backs the loop off; in the
                // foreground the user is waiting and deserves the error.
                if !background {
                    emit(tx, &say_json(&format!("(I couldn't reach my local model — {e}.)")));
                    said = true;
                }
                break;
            }
        };
        let tool = extract_json(&content).unwrap_or_else(|| {
            if background { IDLE_TOOL.to_string() } else { say_json(content.trim()) }
        });
        let name = tool_name(&tool);
        if name == "idle" {
            break; // nothing worth doing — not forwarded, just paces the loop
        }
        if name == "think" {
            thoughts += 1;
            if thoughts > 1 {
                continue; // don't spin on thinking; re-prompt for an action
            }
        } else if !emitted.insert(tool.clone()) {
            // an exact repeat: never apply it twice — one duplicate card per repeat
            // is exactly the bug this prevents.  Two repeats = the model is stuck.
            eprintln!("[agent{}] repeat suppressed: {}", if background { "·bg" } else { "" }, preview(&tool));
            suppressed = true;
            stuck += 1;
            if stuck >= 2 {
                break;
            }
            continue;
        } else {
            acted = true;
        }
        eprintln!("[agent{}] {name}: {}", if background { "·bg" } else { "" }, preview(&tool));
        emit(tx, &tool);
        // A turn ends when the agent addresses the user.
        if name == "say" || name == "ask" {
            said = true;
            break;
        }
    }
    // a FOREGROUND turn always ends with something for the user (never silent);
    // background turns are allowed to finish quietly — their trace is the activity log.
    if !background && !said {
        emit(tx, &say_json("What would you like to do next?"));
    }
    tx.send(Cmd::Status { working: false, background }).ok();
    if preempted {
        Outcome::Preempted
    } else if acted || said {
        Outcome::Acted
    } else {
        Outcome::Idled
    }
}

const IDLE_TOOL: &str = r#"{"tool":"idle","args":{}}"#;

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
