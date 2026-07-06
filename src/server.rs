//! The twin server: the V8 graph on one thread + a thin-client boundary (§11.11).
//!
//! The whole UI is a derived document computed server-side (on V8, §14); the browser
//! is a dumb host that applies the forward mutation stream (§11.3) and sends back the
//! backward event stream (§11.4).  Because a V8 Isolate is single-threaded, the graph
//! lives on ONE dedicated thread; the network is a set of connection threads that talk
//! to it over channels — the "wire is just a boundary lens" (§10) made literal.  The
//! agent (§12) is a THIRD party on the same channels: it perceives the graph as a
//! structured projection (§12.3) and acts by emitting tool-calls that become edits.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
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

/// Start the server (blocking). `addr` like "127.0.0.1:8080".
pub fn serve(addr: &str) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel::<Cmd>();

    // The agent lives on its own thread (model calls block for seconds and must not
    // stall the graph). It returns a wake channel the graph pokes on user input.
    let wake = agent::spawn(tx.clone());
    thread::spawn(move || graph_loop(rx, wake));

    let listener = TcpListener::bind(addr)?;
    println!("twin serve — open http://{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let tx = tx.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_conn(stream, tx) {
                        eprintln!("conn: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}

/// Owns the V8 graph; serializes all edits. Broadcasts new mutation slices + status.
fn graph_loop(rx: Receiver<Cmd>, wake: Sender<()>) {
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
    if let Ok(text) = std::fs::read_to_string("data/manifest.json") {
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
            }
        }
    }

    let mut clients: Vec<Sender<String>> = Vec::new();
    // invariant: `cursor` == total mutations already broadcast to all live clients.
    let mut cursor = g.twin_total();

    for cmd in rx {
        match cmd {
            Cmd::Connect(tx) => {
                // full snapshot [0..now); replaying it rebuilds the current DOM exactly.
                let snapshot = wrap(&g.twin_from(0));
                if tx.send(snapshot).is_ok() {
                    clients.push(tx);
                }
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
                    // sourcing): stamp arrival time at the boundary and record the event
                    // verbatim in the graph's `input` stream; feed items, renders, and the
                    // agent's read of the user are all derived from it, with lineage.
                    v["ts"] = serde_json::json!(now_ms());
                    g.twin_event(&v.to_string());
                    // a boundary fetch (§9.9) additionally invokes a named host capability
                    // by (adapter, id).  The core routes by adapter key only; it doesn't
                    // know the domain (here: a time-series lens materialized on demand).
                    if etype == "fetch" {
                        let adapter = v["adapter"].as_str().unwrap_or("");
                        let id = v["id"].as_str().unwrap_or("");
                        let label = v["label"].as_str().unwrap_or(id);
                        g.twin_fetch(adapter, id, label);
                    }
                }
                broadcast_new(&mut g, &mut cursor, &mut clients);
                // wake the agent when the user addressed it (typed or answered a card);
                // pure navigation is still captured raw, and perceived on the next turn.
                if etype == "user_message" || etype == "choose" {
                    wake.send(()).ok();
                }
            }
            Cmd::AgentTool(json) => {
                dispatch_tool(&mut g, &json);
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
    match tool {
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

fn handle_conn(mut stream: TcpStream, tx: Sender<Cmd>) -> std::io::Result<()> {
    let request = ws::read_http_headers(&mut stream)?;
    if request.to_ascii_lowercase().contains("upgrade: websocket") {
        if ws::send_handshake(&mut stream, &request)? {
            serve_ws(stream, tx)?;
        }
    } else if let Some(name) = file_request(&request) {
        serve_file(stream, &name)?;
    } else {
        serve_html(stream)?;
    }
    Ok(())
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
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        INDEX_HTML.len(),
        INDEX_HTML
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()
}

fn serve_ws(stream: TcpStream, tx: Sender<Cmd>) -> std::io::Result<()> {
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
    Ok(())
}
