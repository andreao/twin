//! `twin serve` — the live UI spine (design_doc §11.11).
//!
//!   cargo run --bin serve            # http://127.0.0.1:8080
//!   cargo run --bin serve 0.0.0.0:9000

fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8080".to_string());
    twin_runtime::server::serve(&addr)
}
