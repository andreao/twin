//! Serverless peer sync (§8 made distributed): the journal as a SET OF PER-ORIGIN
//! APPEND-ONLY LOGS.
//!
//! Every node owns exactly one log — the untagged lines it appends locally.  A line
//! merged from a peer is journaled with two extra fields: `o` (the origin node's
//! public id) and `s` (its position in that origin's log).  Replay does not care:
//! the extra fields are invisible to it, so old journals stay valid and a merged
//! journal reconstructs the merged twin.
//!
//! Reconciliation, not streaming: peers periodically exchange a HAVE map — "how
//! many events of each origin do I hold" — and stream each other the missing
//! tails, in order.  A receiver accepts only the next-expected sequence per
//! origin, so logs stay contiguous, transfers are idempotent, and an interrupted
//! exchange simply resumes on the next round.  Effects stop at the boundary: a
//! remote tool line applies in replay mode, so a peer's skill runs and fetches are
//! never re-executed here.
//!
//! The transport (behind the `sync` cargo feature) is iroh: a node is dialed by
//! its ed25519 public key and the connection finds its own route — hole-punched
//! peer-to-peer when possible, relayed when not.  No server, no account: pairing
//! is "run `twin-sync add <node-id>` on both sides".
//!
//! This module's merge logic is dependency-free and always compiled; only the
//! network engine needs iroh.

use std::collections::HashMap;

/// The protocol name on the wire; bump the suffix on breaking changes.
pub const ALPN: &[u8] = b"vardoger/sync/0";

/// Where a node's identity and peer list live, under the twin's home.
pub const SYNC_DIR: &str = "data/sync";

/// One journal line as the unit of replication.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncLine {
    pub origin: String,
    pub seq: u64,
    pub kind: String,
    pub payload: String,
}

/// Parse one journal line into its sync identity.  Untagged lines belong to
/// `local_id`'s log; their sequence is their position among untagged lines,
/// tracked by the caller via `local_count`.
fn parse_line(line: &str, local_id: &str, local_count: &mut u64) -> Option<SyncLine> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = v["k"].as_str()?.to_string();
    let payload = v["p"].as_str()?.to_string();
    match v["o"].as_str() {
        Some(o) => Some(SyncLine { origin: o.to_string(), seq: v["s"].as_u64()?, kind, payload }),
        None => {
            let seq = *local_count;
            *local_count += 1;
            Some(SyncLine { origin: local_id.to_string(), seq, kind, payload })
        }
    }
}

/// Per-origin event counts in one journal file — this node's HAVE map for it.
/// Contiguity is an invariant (receivers only accept next-in-order), so a count
/// is enough to name exactly which events are held.
pub fn have_map(journal_path: &str, local_id: &str) -> HashMap<String, u64> {
    let mut have: HashMap<String, u64> = HashMap::new();
    let mut local = 0u64;
    if let Ok(text) = std::fs::read_to_string(journal_path) {
        for line in text.lines() {
            if let Some(l) = parse_line(line, local_id, &mut local) {
                let e = have.entry(l.origin).or_insert(0);
                *e = (*e).max(l.seq + 1);
            }
        }
    }
    have
}

/// The events of `origin` from sequence `from` on, in order — the tail a peer
/// is missing.
pub fn events_from(journal_path: &str, local_id: &str, origin: &str, from: u64) -> Vec<SyncLine> {
    let mut out = Vec::new();
    let mut local = 0u64;
    if let Ok(text) = std::fs::read_to_string(journal_path) {
        for line in text.lines() {
            if let Some(l) = parse_line(line, local_id, &mut local) {
                if l.origin == origin && l.seq >= from {
                    out.push(l);
                }
            }
        }
    }
    out.sort_by_key(|l| l.seq);
    out
}

/// The peer list: one node id per line, comments allowed.  Editable by hand,
/// written by `twin-sync add`.
pub fn load_peers() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(format!("{SYNC_DIR}/peers")) {
        for line in text.lines() {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('#') {
                out.push(t.to_string());
            }
        }
    }
    out
}

pub fn add_peer(node_id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(SYNC_DIR)?;
    let mut peers = load_peers();
    if peers.iter().any(|p| p == node_id) {
        return Ok(());
    }
    peers.push(node_id.to_string());
    std::fs::write(format!("{SYNC_DIR}/peers"), peers.join("\n") + "\n")
}

/// This node's secret key: 32 bytes hex at data/sync/secret.key, minted from the
/// OS's randomness on first use.  The public id is derived from it (see `net`).
pub fn secret_key_bytes() -> std::io::Result<[u8; 32]> {
    let path = format!("{SYNC_DIR}/secret.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Some(k) = parse_hex32(text.trim()) {
            return Ok(k);
        }
    }
    let mut key = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut key)?;
    std::fs::create_dir_all(SYNC_DIR)?;
    std::fs::write(&path, to_hex(&key))?;
    Ok(key)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The iroh engine: an endpoint bound to this node's key, an accept loop, and a
/// reconcile loop that dials every peer on a short interval.
#[cfg(feature = "sync")]
pub mod net {
    use super::*;
    use crate::server::{self, Cmd, Projects};
    use tokio::io::{AsyncBufReadExt, BufReader};

    /// Start the sync engine on its own thread (it owns a tokio runtime).
    /// `projects` is the live project registry — remote events boot projects on
    /// demand and enter each graph through the same single-writer channel as
    /// everything else.
    pub fn spawn(projects: Projects) {
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return eprintln!("sync: no runtime: {e}"),
            };
            rt.block_on(run(projects));
        });
    }

    async fn run(projects: Projects) {
        let key = match secret_key_bytes() {
            Ok(k) => k,
            Err(e) => return eprintln!("sync: no key: {e}"),
        };
        let secret = iroh::SecretKey::from_bytes(&key);
        // the N0 preset = relays + address lookup: a peer is dialed by public key
        // alone and the connection finds its route (direct when it can punch through,
        // relayed when it cannot)
        let endpoint = match iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
        {
            Ok(ep) => ep,
            Err(e) => return eprintln!("sync: endpoint failed: {e}"),
        };
        let me = endpoint.id().to_string();
        println!("sync: this node is {me}");
        println!("sync: on a peer, run  twin-sync add {me}");

        // accept: every inbound connection is one reconcile exchange
        let ep = endpoint.clone();
        let pr = projects.clone();
        let my = me.clone();
        tokio::spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let (projects, me) = (pr.clone(), my.clone());
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(e) = exchange(&conn, &projects, &me, false).await {
                                eprintln!("sync: inbound exchange: {e}");
                            }
                        }
                        Err(e) => eprintln!("sync: accept: {e}"),
                    }
                });
            }
        });

        // reconcile: dial every peer, exchange, sleep, repeat.  Reconciliation is
        // idempotent, so the interval is just how fresh a peer can be.
        loop {
            for peer in load_peers() {
                if peer == me {
                    continue;
                }
                let Ok(id) = peer.parse::<iroh::EndpointId>() else {
                    eprintln!("sync: bad peer id: {peer}");
                    continue;
                };
                match endpoint.connect(id, ALPN).await {
                    Ok(conn) => {
                        if let Err(e) = exchange(&conn, &projects, &me, true).await {
                            eprintln!("sync: {peer}: exchange: {e}");
                        }
                    }
                    Err(e) => eprintln!("sync: {peer}: {e}"),
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    /// One reconcile exchange over one bi-directional stream, symmetric by
    /// construction: both sides send their HAVE first, then stream the tails the
    /// other is missing, then close.  Frames are JSON lines.
    async fn exchange(
        conn: &iroh::endpoint::Connection,
        projects: &Projects,
        me: &str,
        dialer: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (mut send, recv) = if dialer { conn.open_bi().await? } else { conn.accept_bi().await? };
        let mut recv = BufReader::new(recv);

        // my HAVE, across every project on disk
        let mut have: HashMap<String, HashMap<String, u64>> = HashMap::new();
        for slug in server::all_project_slugs() {
            have.insert(slug.clone(), have_map(&server::journal_path(&slug), me));
        }
        let hello = serde_json::json!({ "have": have_json(&have) }).to_string();
        send.write_all(hello.as_bytes()).await?;
        send.write_all(b"\n").await?;

        // their HAVE
        let mut line = String::new();
        recv.read_line(&mut line).await?;
        let theirs: serde_json::Value = serde_json::from_str(&line)?;
        let theirs = &theirs["have"];

        // stream them every tail they lack (projects they have never seen included)
        for (slug, mine) in &have {
            for (origin, count) in mine {
                let from = theirs[slug][origin].as_u64().unwrap_or(0);
                if from >= *count {
                    continue;
                }
                for ev in events_from(&server::journal_path(slug), me, origin, from) {
                    let frame = serde_json::json!({
                        "project": slug, "origin": ev.origin, "seq": ev.seq,
                        "kind": ev.kind, "payload": ev.payload,
                    })
                    .to_string();
                    send.write_all(frame.as_bytes()).await?;
                    send.write_all(b"\n").await?;
                }
            }
        }
        send.finish()?;

        // apply every tail they stream us, in arrival order, through each
        // project's own graph thread (which dedupes and journals)
        let mut applied = 0usize;
        loop {
            let mut line = String::new();
            if recv.read_line(&mut line).await? == 0 {
                break;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let (Some(slug), Some(origin), Some(seq), Some(kind), Some(payload)) = (
                v["project"].as_str(),
                v["origin"].as_str(),
                v["seq"].as_u64(),
                v["kind"].as_str(),
                v["payload"].as_str(),
            ) else {
                continue;
            };
            if origin == me {
                continue; // nobody else writes my log
            }
            let (tx, _) = server::project_handle(slug, projects);
            tx.send(Cmd::Sync {
                origin: origin.to_string(),
                seq,
                kind: kind.to_string(),
                payload: payload.to_string(),
            })
            .ok();
            applied += 1;
        }
        if applied > 0 {
            println!("sync: merged {applied} event(s) from {}", conn.remote_id());
        }
        Ok(())
    }

    fn have_json(have: &HashMap<String, HashMap<String, u64>>) -> serde_json::Value {
        let mut root = serde_json::Map::new();
        for (slug, origins) in have {
            let mut m = serde_json::Map::new();
            for (o, c) in origins {
                m.insert(o.clone(), serde_json::json!(c));
            }
            root.insert(slug.clone(), serde_json::Value::Object(m));
        }
        serde_json::Value::Object(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_journal(name: &str, lines: &[&str]) -> String {
        let path = std::env::temp_dir().join(format!("sync-test-{}-{name}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn untagged_lines_are_the_local_log() {
        let path = temp_journal("local", &[
            r#"{"k":"ev","p":"{\"type\":\"user_message\"}"}"#,
            r#"{"k":"tool","p":"{\"tool\":\"think\"}"}"#,
        ]);
        let have = have_map(&path, "me");
        assert_eq!(have.get("me"), Some(&2));
        let evs = events_from(&path, "me", "me", 1);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, "tool");
        assert_eq!(evs[0].seq, 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tagged_lines_belong_to_their_origin() {
        let path = temp_journal("tagged", &[
            r#"{"k":"ev","p":"a"}"#,
            r#"{"k":"ev","p":"b","o":"peer1","s":0}"#,
            r#"{"k":"ev","p":"c"}"#,
            r#"{"k":"tool","p":"d","o":"peer1","s":1}"#,
        ]);
        let have = have_map(&path, "me");
        assert_eq!(have.get("me"), Some(&2));
        assert_eq!(have.get("peer1"), Some(&2));
        // the local log's tail skips over merged lines
        let evs = events_from(&path, "me", "me", 0);
        assert_eq!(evs.iter().map(|e| e.payload.as_str()).collect::<Vec<_>>(), vec!["a", "c"]);
        // an origin's tail comes back in sequence order
        let evs = events_from(&path, "me", "peer1", 1);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].payload, "d");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hex_key_roundtrip() {
        let key = [7u8; 32];
        assert_eq!(parse_hex32(&to_hex(&key)), Some(key));
        assert_eq!(parse_hex32("zz"), None);
    }
}
