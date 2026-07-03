//! An Incremental Runtime — a content-addressed, branchable digital-twin substrate
//! on raw V8 (design_doc.md).
//!
//! This crate is the **trusted kernel** of §4.1: the bare-V8 substrate (§14), the
//! content-addressed store and name resolution (§4), and the governed-effect
//! boundary (§6).  Above this kernel, lenses and applications are content-addressed
//! JS data governed by the kernel — the reflective tower of §4.1.
//!
//! Why Rust-around-raw-V8 and not Node (§14): Node's module identity is a path, but
//! we need a content hash (C2); and governance means constructing a Context's
//! globals from scratch (§6.1), which Node's ambient global realm does not allow.

pub mod adapter;
pub mod agent;
pub mod cognite;
pub mod definitions;
pub mod engine;
pub mod hashing;
pub mod jsgraph;
pub mod layers;
pub mod pmap;
pub mod runtime;
pub mod server;
pub mod skills;
pub mod source;
pub mod ws;

pub use adapter::{MockDb, PollDiffAdapter};
pub use definitions::{Definition, DefinitionStore};
pub use hashing::{content_hash, definition_hash, Hash};
pub use jsgraph::JsGraph;
pub use runtime::{Branch, Runtime};
