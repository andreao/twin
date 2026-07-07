//! `twin-sync` — pair twins, no server anywhere (src/sync.rs).
//!
//!   twin-sync id            # this node's public id (mints a key on first run)
//!   twin-sync add <node-id> # sync with that node from now on
//!   twin-sync peers         # who this node syncs with
//!
//! Run it from the twin's home (the directory holding data/), then start the
//! node with syncing on:  cargo run --bin serve --features sync

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("id") => {
            let key = twin_runtime::sync::secret_key_bytes().expect("a key can be read or minted");
            let id = iroh::SecretKey::from_bytes(&key).public();
            println!("{id}");
            eprintln!("on a peer:  twin-sync add {id}");
        }
        Some("add") => {
            let Some(id) = args.get(2) else {
                return eprintln!("usage: twin-sync add <node-id>");
            };
            if id.parse::<iroh::EndpointId>().is_err() {
                return eprintln!("that is not a node id (expected the 64-char id `twin-sync id` prints)");
            }
            twin_runtime::sync::add_peer(id).expect("the peer list is writable");
            println!("added — both sides must add each other, then run serve with --features sync");
        }
        Some("peers") => {
            for p in twin_runtime::sync::load_peers() {
                println!("{p}");
            }
        }
        _ => eprintln!("usage: twin-sync id | add <node-id> | peers"),
    }
}
