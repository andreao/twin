//! The twin server: the V8 graph on one thread + a thin-client boundary (§11.11).
//!
//! The whole UI is a derived document computed server-side (on V8, §14); the browser
//! is a dumb host that applies the forward mutation stream (§11.3) and sends back the
//! backward event stream (§11.4).  Because a V8 Isolate is single-threaded, the graph
//! lives on ONE dedicated thread; the network is a set of connection threads that talk
//! to it over channels — the "wire is just a boundary lens" (§10) made literal.  The
//! agent (§12) is a THIRD party on the same channels: it perceives the graph as a
//! structured projection (§12.3) and acts by emitting tool-calls that become edits.

use std::collections::HashMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::agent;
use crate::jsgraph::JsGraph;
use crate::ws;

/// The self-contained thin client, embedded so `serve` needs no working directory.
const INDEX_HTML: &str = include_str!("../web/index.html");

/// Commands into the single graph thread.
pub enum Cmd {
    /// A new client connected; send it a full snapshot and keep its sender to broadcast.
    Connect(Sender<String>),
    /// A backward UI event (§11.4) as JSON — the user steering.
    Event(String),
    /// An agent tool-call (§12.1) to apply as a graph edit.
    AgentTool(String),
    /// The agent asks for its perception (§12.3); we reply with the projection JSON.
    Perceive(Sender<String>),
    /// The agent's working state, surfaced to the UI: foreground turns show the
    /// in-feed "thinking…" indicator, background turns pulse the agent rail.
    Status { working: bool, background: bool },
}

/// Wall-clock milliseconds — a BOUNDARY fact (§9.8): raw events are stamped here as
/// they cross into the graph; time is never computed inside the graph itself.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The twin's persistent memory (§8): every event that DRIVES the graph — raw user
/// events (already boundary-stamped) and agent tool calls — appended as JSONL.  On
/// boot the journal replays through the exact same code paths, so the entire twin
/// (chat, lenses, findings, agenda, stars, open columns) reconstructs from its own
/// history.  Mounts/skills are not journaled: they re-derive from manifest + skills
/// dir each boot, then history replays on top.  Delete the file for a fresh twin.
const JOURNAL: &str = "data/journal.jsonl";

/// A PROJECT is a completely separate everything: its own graph (V8 isolate on
/// its own thread), its own agent, its own journal, its own clients.  The only
/// shared things are the binary, the skills directory, and the demo-data files.
/// The pre-projects journal keeps living at data/journal.jsonl as "default";
/// every other project journals under data/projects/<slug>/.
type ProjectHandle = (Sender<Cmd>, Arc<std::sync::atomic::AtomicUsize>);
type Projects = Arc<Mutex<HashMap<String, ProjectHandle>>>;

/// Normalize a user-supplied project name to a stable slug.
fn project_slug(raw: &str) -> String {
    let s: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let t = s.trim_matches('-');
    if t.is_empty() { "default".into() } else { t.to_string() }
}

/// The journal path for one project — TWIN_JOURNAL still overrides the default
/// project's path, so tests and experiments never write into the real memory.
fn project_journal(slug: &str) -> String {
    if slug == "default" {
        std::env::var("TWIN_JOURNAL").unwrap_or_else(|_| JOURNAL.to_string())
    } else {
        format!("data/projects/{slug}/journal.jsonl")
    }
}

/// A project's human face: its title and one-line description.  The default
/// project is named by the data manifest (the same file that mounts its data);
/// created projects carry a project.json written at creation time.
fn project_meta(slug: &str) -> (String, String) {
    let read = |path: &str| -> Option<(String, String)> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let node = if slug == "default" { &v["project"] } else { &v };
        let title = node["title"].as_str().unwrap_or("").trim().to_string();
        let desc = node["description"].as_str().unwrap_or("").trim().to_string();
        (!title.is_empty() || !desc.is_empty()).then_some((title, desc))
    };
    let found = if slug == "default" {
        read("data/manifest.json")
    } else {
        read(&format!("data/projects/{slug}/project.json"))
    };
    match found {
        Some((t, d)) if !t.is_empty() => (t, d),
        Some((_, d)) => (prettify(slug), d),
        None => (prettify(slug), String::new()),
    }
}

/// A readable fallback title from a slug: "wind-farm" → "Wind farm".
fn prettify(slug: &str) -> String {
    let words = slug.replace('-', " ");
    let mut ch = words.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => "Project".into(),
    }
}

/// Every project that exists on disk (has a journal), plus the default.
fn list_projects() -> Vec<String> {
    let mut out = vec!["default".to_string()];
    if let Ok(rd) = std::fs::read_dir("data/projects") {
        for e in rd.flatten() {
            if e.path().join("journal.jsonl").exists() || e.path().join("project.json").exists() {
                if let Some(n) = e.file_name().to_str() {
                    out.push(n.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Create a project: its directory, its project.json, and an empty journal so
/// it lists.  Never touches an existing project's metadata.
fn create_project(name: &str, description: &str) -> String {
    let slug = project_slug(name);
    if slug == "default" {
        return slug;
    }
    let dir = format!("data/projects/{slug}");
    let _ = std::fs::create_dir_all(&dir);
    let meta_path = format!("{dir}/project.json");
    if !std::path::Path::new(&meta_path).exists() {
        let meta = serde_json::json!({ "title": name.trim(), "description": description.trim() });
        let _ = std::fs::write(&meta_path, meta.to_string());
    }
    let jpath = format!("{dir}/journal.jsonl");
    if !std::path::Path::new(&jpath).exists() {
        let _ = std::fs::write(&jpath, "");
    }
    slug
}

/// Get the project's graph channel, booting the whole stack (graph thread, agent,
/// journal replay) on first touch.  The default project mounts the data manifest;
/// every other project starts EMPTY — its sources arrive by the agent's own
/// read_source calls, which are journaled and so replay.
fn project_tx(slug: &str, projects: &Projects) -> ProjectHandle {
    let mut map = projects.lock().unwrap();
    if let Some(h) = map.get(slug) {
        return h.clone();
    }
    let (tx, rx) = mpsc::channel::<Cmd>();
    let paused = Arc::new(AtomicBool::new(false));
    // how many clients are looking at this project RIGHT NOW — while zero, its
    // agent is parked: switching projects pauses the one you left
    let attended = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wake = agent::spawn(tx.clone(), paused.clone(), attended.clone());
    let jpath = project_journal(slug);
    if let Some(dir) = std::path::Path::new(&jpath).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let with_manifest = slug == "default";
    let name = slug.to_string();
    thread::spawn(move || graph_loop(rx, wake, paused, name, jpath, with_manifest));
    let handle = (tx, attended);
    map.insert(slug.to_string(), handle.clone());
    handle
}

fn journal_append(file: &mut Option<std::fs::File>, kind: &str, payload: &str) {
    if let Some(f) = file {
        let line = serde_json::json!({ "k": kind, "p": payload }).to_string();
        let _ = writeln!(f, "{line}");
    }
}

/// Apply one inbound event to the graph: raw capture (+ its boundary effect, for
/// fetches).  Shared by the live path and the boot replay.
fn apply_event(g: &mut JsGraph, stamped_json: &str) {
    g.twin_event(stamped_json);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stamped_json) {
        if v["type"] == "fetch" {
            let adapter = v["adapter"].as_str().unwrap_or("");
            let id = v["id"].as_str().unwrap_or("");
            let label = v["label"].as_str().unwrap_or(id);
            let panel = v["panel"].as_u64().unwrap_or(0) as usize;
            g.twin_fetch(adapter, id, label, panel);
        }
    }
}

/// Start the server (blocking). `addr` like "127.0.0.1:8080".
pub fn serve(addr: &str) -> std::io::Result<()> {
    let projects: Projects = Arc::new(Mutex::new(HashMap::new()));
    // the default project boots eagerly (it's the one with the demo mounts);
    // every other project boots on the first client that asks for it
    project_tx("default", &projects);

    let listener = TcpListener::bind(addr)?;
    println!("twin serve — open http://{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let projects = projects.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_conn(stream, projects) {
                        eprintln!("conn: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}

/// If this raw UI event is the run switch, flip the shared flag.  Called on the
/// live path AND the boot replay, so a paused twin stays paused across restarts.
fn note_pause(paused: &AtomicBool, ev_json: &str) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(ev_json) {
        match v["type"].as_str() {
            Some("pause") => paused.store(true, Ordering::Relaxed),
            Some("resume") => paused.store(false, Ordering::Relaxed),
            _ => {}
        }
    }
}

fn twin_state_msg(paused: &AtomicBool) -> String {
    format!("{{\"type\":\"twin\",\"paused\":{}}}", paused.load(Ordering::Relaxed))
}

/// Owns ONE project's V8 graph; serializes all its edits.  Broadcasts new
/// mutation slices + status to that project's clients only.
fn graph_loop(
    rx: Receiver<Cmd>,
    wake: Sender<agent::Wake>,
    paused: Arc<AtomicBool>,
    project: String,
    journal_file: String,
    with_manifest: bool,
) {
    let mut g = JsGraph::new_twin();

    // Core skills-loader (§4.1, §11.13): seed the twin's skill registry from the static
    // `skills/` directory on startup, so the agent knows what capabilities it has.
    let found = crate::skills::discover("skills");
    for sk in &found {
        g.twin_install_skill(&sk.name, &sk.title, &sk.description, &sk.files);
    }
    if !found.is_empty() {
        println!("loaded {} skill(s): {}", found.len(),
            found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
    }

    // Bootstrap mounts from a declarative, domain-NEUTRAL manifest (§7 residence): the
    // core loops over `{name, path, residence}` entries and mounts each generically —
    // it never knows what "assets" or "documents" mean.  The domain vocabulary lives in
    // the data manifest, not in the core.  (Interim seam until the agent-driven boundary
    // lens does this itself with lineage.)
    if let Ok(text) = with_manifest
        .then(|| std::fs::read_to_string("data/manifest.json"))
        .unwrap_or(Err(std::io::Error::other("no manifest for this project")))
    {
        if let Ok(m) = serde_json::from_str::<serde_json::Value>(&text) {
            for entry in m["mounts"].as_array().into_iter().flatten() {
                let name = entry["name"].as_str().unwrap_or("");
                let path = entry["path"].as_str().unwrap_or("");
                let residence = entry["residence"].as_str().unwrap_or("mounted");
                if name.is_empty() || path.is_empty() || !std::path::Path::new(path).exists() {
                    continue;
                }
                let status = g.twin_read_source(name, path, residence);
                println!("{status}");
                // human title/description come from the manifest too — folded in as a
                // describe event, the same path the agent uses (§8: everything is events)
                let title = entry["title"].as_str().unwrap_or("");
                let desc = entry["description"].as_str().unwrap_or("");
                let origin = entry["origin"].as_str().unwrap_or("");
                if !title.is_empty() || !desc.is_empty() || !origin.is_empty() {
                    let tool = serde_json::json!({
                        "tool": "describe",
                        "args": { "source": name, "title": title, "description": desc, "origin": origin }
                    })
                    .to_string();
                    g.twin_agent_tool(&tool);
                }
            }
        }
    }

    // Replay the journal: the twin REMEMBERS.  Same code paths as live traffic, so
    // the replay is exact; no clients are connected yet, so nothing is broadcast.
    let mut replayed = 0usize;
    if let Ok(text) = std::fs::read_to_string(&journal_file) {
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            match (v["k"].as_str(), v["p"].as_str()) {
                (Some("ev"), Some(p)) => {
                    apply_event(&mut g, p);
                    note_pause(&paused, p);
                    replayed += 1;
                }
                (Some("tool"), Some(p)) => { dispatch_tool(&mut g, p); replayed += 1; }
                _ => {}
            }
        }
    }
    if replayed > 0 {
        println!("[{project}] replayed {replayed} journal events — the twin remembers");
    }
    let mut journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_file)
        .ok();

    let mut clients: Vec<Sender<String>> = Vec::new();
    // invariant: `cursor` == total mutations already broadcast to all live clients.
    let mut cursor = g.twin_total();

    for cmd in rx {
        match cmd {
            Cmd::Connect(tx) => {
                // full snapshot [0..now); replaying it rebuilds the current DOM exactly.
                let snapshot = wrap(&g.twin_from(0));
                if tx.send(snapshot).is_ok() && tx.send(twin_state_msg(&paused)).is_ok() {
                    clients.push(tx);
                }
                // the user just showed up — the agent should be seen working, not
                // dozing at the far end of its idle backoff
                wake.send(agent::Wake::Presence).ok();
            }
            Cmd::Event(json) => {
                let mut ev: Option<serde_json::Value> = serde_json::from_str(&json).ok();
                let etype = ev
                    .as_ref()
                    .and_then(|v| v["type"].as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(v) = ev.as_mut() {
                    // EVERY user action is captured in its rawest form (§8 pure event
                    // sourcing): stamp arrival time at the boundary, apply it (raw capture
                    // + derivations + any boundary fetch), and journal it for replay.
                    v["ts"] = serde_json::json!(now_ms());
                    let stamped = v.to_string();
                    apply_event(&mut g, &stamped);
                    journal_append(&mut journal, "ev", &stamped);
                    if etype == "pause" || etype == "resume" {
                        note_pause(&paused, &stamped);
                        let msg = twin_state_msg(&paused);
                        clients.retain(|c| c.send(msg.clone()).is_ok());
                        // a resumed twin gets back to work immediately, not after backoff
                        if etype == "resume" {
                            wake.send(agent::Wake::Presence).ok();
                        }
                    }
                }
                broadcast_new(&mut g, &mut cursor, &mut clients);
                // wake the agent when the user addressed it (typed or answered a card);
                // pure navigation is still captured raw, and perceived on the next turn.
                if etype == "user_message" || etype == "choose" {
                    wake.send(agent::Wake::Steer).ok();
                }
            }
            Cmd::AgentTool(json) => {
                dispatch_tool(&mut g, &json);
                journal_append(&mut journal, "tool", &json);
                broadcast_new(&mut g, &mut cursor, &mut clients);
            }
            Cmd::Perceive(reply) => {
                reply.send(g.twin_perceive()).ok();
            }
            Cmd::Status { working, background } => {
                let msg = format!(
                    "{{\"type\":\"status\",\"working\":{working},\"background\":{background}}}"
                );
                clients.retain(|c| c.send(msg.clone()).is_ok());
            }
        }
    }
}

/// Apply an agent tool-call. Pure workspace tools (think/say/ask/record_profile) go
/// straight to the V8 graph; effectful ones (read_source, §9.9) are handled in Rust
/// because they touch the outside world, then land their result back as graph edits.
fn dispatch_tool(g: &mut JsGraph, json: &str) {
    let parsed: Option<serde_json::Value> = serde_json::from_str(json).ok();
    let tool = parsed.as_ref().and_then(|v| v["tool"].as_str()).unwrap_or("");
    let show_chart = tool == "show"
        && parsed.as_ref().map(|v| v["args"]["view"] == "chart").unwrap_or(false);
    match tool {
        // an inline chart is a boundary fetch (datapoints may materialize on demand),
        // so this one `show` view routes through the host; the rest are pure graph.
        _ if show_chart => {
            let args = &parsed.as_ref().unwrap()["args"];
            let adapter = args["adapter"].as_str().unwrap_or("cognite-datapoints");
            let id = ["series", "id", "source"]
                .iter()
                .find_map(|k| {
                    let v = &args[*k];
                    v.as_str().map(String::from).or_else(|| v.as_u64().map(|n| n.to_string()))
                })
                .unwrap_or_default();
            let label = args["title"].as_str()
                .or_else(|| args["label"].as_str())
                .unwrap_or(&id)
                .to_string();
            if !id.is_empty() {
                g.twin_show_chart(adapter, &id, &label);
            }
        }
        "read_source" => {
            let args = &parsed.as_ref().unwrap()["args"];
            let path = args["path"].as_str().unwrap_or("").trim().to_string();
            let mode = args["mode"].as_str().unwrap_or("mounted").to_string();
            if path.is_empty() {
                return;
            }
            let name = source_name(&path);
            let status = g.twin_read_source(&name, &path, &mode);
            eprintln!("[read_source] {status}");
        }
        _ => g.twin_agent_tool(json),
    }
}

/// A short, stable source name from a file path (basename without extension).
fn source_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let cleaned: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() { "source".into() } else { trimmed.to_string() }
}

/// Broadcast the mutations produced since `cursor` to all live clients.
fn broadcast_new(g: &mut JsGraph, cursor: &mut usize, clients: &mut Vec<Sender<String>>) {
    let total = g.twin_total();
    if total > *cursor {
        let msg = wrap(&g.twin_from(*cursor));
        *cursor = total;
        clients.retain(|c| c.send(msg.clone()).is_ok());
    }
}

/// Wrap a JSON mutations array in the client envelope.
fn wrap(mutations_json: &str) -> String {
    format!("{{\"type\":\"mutations\",\"batch\":{mutations_json}}}")
}

fn handle_conn(mut stream: TcpStream, projects: Projects) -> std::io::Result<()> {
    let request = ws::read_http_headers(&mut stream)?;
    if request.to_ascii_lowercase().contains("upgrade: websocket") {
        // the socket names its project (ws?project=<slug>); the project boots on demand
        let (tx, attended) = project_tx(&project_from_request(&request), &projects);
        if ws::send_handshake(&mut stream, &request)? {
            serve_ws(stream, tx, attended)?;
        }
    } else if request_path(&request).is_some_and(|p| p == "/projects") {
        if request.starts_with("POST") {
            let body = read_body(&mut stream, &request)?;
            serve_create_project(stream, &body)?;
        } else {
            serve_projects(stream)?;
        }
    } else if let Some(name) = file_request(&request) {
        serve_file(stream, &name)?;
    } else {
        serve_html(stream)?;
    }
    Ok(())
}

/// The request path (with query) of the first request line.
fn request_path(request: &str) -> Option<&str> {
    request.lines().next()?.split_whitespace().nth(1)
}

/// The project slug a websocket asks for: `GET /ws?project=<name>`.
fn project_from_request(request: &str) -> String {
    let query = request_path(request)
        .and_then(|p| p.split_once('?'))
        .map(|(_, q)| q)
        .unwrap_or("");
    for kv in query.split('&') {
        if let Some(v) = kv.strip_prefix("project=") {
            return project_slug(v);
        }
    }
    "default".into()
}

/// The request body, sized by Content-Length (the headers are already consumed).
fn read_body(stream: &mut TcpStream, request: &str) -> std::io::Result<String> {
    use std::io::Read;
    let len = request
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().ok()))
        .flatten()
        .unwrap_or(0)
        .min(65536);
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `POST /projects` {name, description} → create it, reply with its slug.
fn serve_create_project(mut stream: TcpStream, body: &str) -> std::io::Result<()> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let name = v["name"].as_str().unwrap_or("").trim();
    let desc = v["description"].as_str().unwrap_or("").trim();
    let (status, out) = if name.is_empty() {
        ("400 Bad Request".to_string(), serde_json::json!({ "error": "a project needs a name" }))
    } else {
        ("200 OK".to_string(), serde_json::json!({ "slug": create_project(name, desc) }))
    };
    let body = out.to_string();
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

/// `GET /projects` → the project list with titles + descriptions, for the switcher.
fn serve_projects(mut stream: TcpStream) -> std::io::Result<()> {
    let list: Vec<serde_json::Value> = list_projects()
        .into_iter()
        .map(|slug| {
            let (title, description) = project_meta(&slug);
            serde_json::json!({ "slug": slug, "title": title, "description": description })
        })
        .collect();
    let body = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

/// A `GET /file/<name>` request → the basename to serve from the documents dir.
fn file_request(request: &str) -> Option<String> {
    let path = request.lines().next()?.split_whitespace().nth(1)?;
    let rest = path.strip_prefix("/file/")?;
    let name = rest.rsplit('/').next().unwrap_or(rest); // basename only — no traversal
    (!name.is_empty()).then(|| name.to_string())
}

fn serve_file(mut stream: TcpStream, name: &str) -> std::io::Result<()> {
    match std::fs::read(format!("data/cognite/files/{name}")) {
        Ok(bytes) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content_type(name),
                bytes.len()
            );
            stream.write_all(header.as_bytes())?;
            stream.write_all(&bytes)?;
            stream.flush()
        }
        Err(_) => {
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found";
            stream.write_all(resp.as_bytes())?;
            stream.flush()
        }
    }
}

fn content_type(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".pdf") { "application/pdf" }
    else if n.ends_with(".svg") { "image/svg+xml" }
    else if n.ends_with(".png") { "image/png" }
    else if n.ends_with(".jpg") || n.ends_with(".jpeg") { "image/jpeg" }
    else if n.ends_with(".mp4") { "video/mp4" }
    else { "application/octet-stream" }
}

fn serve_html(mut stream: TcpStream) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        INDEX_HTML.len(),
        INDEX_HTML
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

fn serve_ws(
    stream: TcpStream,
    tx: Sender<Cmd>,
    attended: Arc<std::sync::atomic::AtomicUsize>,
) -> std::io::Result<()> {
    // presence is per-socket: while this client lives, its project is attended
    attended.fetch_add(1, Ordering::Relaxed);
    // register with the graph thread; it pushes mutation batches down `out_rx`.
    let (out_tx, out_rx) = mpsc::channel::<String>();
    tx.send(Cmd::Connect(out_tx)).ok();

    // writer thread: drain graph broadcasts to the socket.
    let mut wstream = stream.try_clone()?;
    thread::spawn(move || {
        for msg in out_rx {
            if ws::write_text(&mut wstream, &msg).is_err() {
                break;
            }
        }
    });

    // reader loop: decode inbound frames into backward events.
    let mut rstream = stream;
    loop {
        match ws::read_frame(&mut rstream) {
            Ok(ws::Frame::Text(t)) => {
                tx.send(Cmd::Event(t)).ok();
            }
            // Ignore pings: the writer thread owns socket writes, and browsers don't
            // send pings via the JS WebSocket API, so there's nothing to answer.
            Ok(ws::Frame::Ping(_)) | Ok(ws::Frame::Other) => {}
            Ok(ws::Frame::Close) | Err(_) => break,
        }
    }
    // the client left — if it was the last one, the project's agent parks itself
    attended.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_slugs_are_stable_and_safe() {
        assert_eq!(project_slug("Valhall Platform"), "valhall-platform");
        assert_eq!(project_slug("  ../../etc  "), "etc");
        assert_eq!(project_slug("!!!"), "default");
        assert_eq!(project_slug(""), "default");
    }

    #[test]
    fn project_journals_are_separate_files() {
        assert_eq!(project_journal("wind-farm"), "data/projects/wind-farm/journal.jsonl");
        assert!(project_journal("default").ends_with("journal.jsonl"));
    }

    #[test]
    fn ws_request_names_its_project() {
        let req = "GET /ws?project=Wind%20Farm&x=1 HTTP/1.1\r\nHost: x\r\n";
        assert_eq!(project_from_request(req), "wind-20farm");
        assert_eq!(project_from_request("GET /ws HTTP/1.1\r\n"), "default");
    }
}
