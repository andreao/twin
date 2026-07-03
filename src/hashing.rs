//! Content addressing (design_doc §4, C2).
//!
//! Identity is content: a definition is addressed by the hash of its *normalized
//! meaning* plus the hashes of its dependencies.  Two definitions that *mean* the
//! same thing get the same hash, so they are shared automatically, invalidation is
//! exact, and merge is conflict-free by construction.
//!
//! The production design hashes a normalized JS AST.  Here we apply a cheap source
//! normalization (strip line comments and collapse insignificant whitespace) before
//! hashing — enough to demonstrate that formatting changes do not change identity,
//! while real AST normalization (stripping local-variable names too) is the §4 next
//! step.  Names live in a separate namespace, so references are by hash, not name.

use sha2::{Digest, Sha256};

pub const HASH_LEN: usize = 16;

/// A content hash, rendered as a short hex string (the object id).
pub type Hash = String;

/// Normalize JS-ish source so that formatting differences don't change identity.
///
/// This strips `//` line comments and collapses runs of ASCII whitespace to a
/// single space (outside of nothing fancy — a real impl would tokenize). It is the
/// analogue of "strip formatting and comments" from §4.
pub fn normalize_source(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let code = match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        };
        for tok in code.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(tok);
        }
    }
    out
}

fn digest(parts: &[&[u8]]) -> Hash {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    let full = h.finalize();
    let mut s = String::with_capacity(HASH_LEN);
    for b in full.iter().take(HASH_LEN / 2) {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Hash an arbitrary content blob.
pub fn content_hash(bytes: &[u8]) -> Hash {
    digest(&[bytes])
}

/// Hash a *definition*: its kind, normalized body, and dependency hashes (C2).
///
/// Because the dependency hashes are folded in, editing a dependency changes the
/// dependent's hash too — so the engine's "which nodes depended on the old hash"
/// is exact (§4).
pub fn definition_hash(kind: &str, source: &str, dep_hashes: &[Hash]) -> Hash {
    let normalized = normalize_source(source);
    let mut parts: Vec<&[u8]> = vec![kind.as_bytes(), normalized.as_bytes()];
    for d in dep_hashes {
        parts.push(d.as_bytes());
    }
    digest(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_does_not_change_identity() {
        let a = definition_hash("lens", "x => x + 1", &[]);
        let b = definition_hash("lens", "x  =>  x + 1   // a comment", &[]);
        assert_eq!(a, b, "formatting/comments must not change the content id");
    }

    #[test]
    fn meaning_change_makes_new_hash() {
        let a = definition_hash("lens", "x => x + 1", &[]);
        let b = definition_hash("lens", "x => x + 2", &[]);
        assert_ne!(a, b);
    }

    #[test]
    fn deps_fold_in() {
        let a = definition_hash("lens", "x => f(x)", &["depA".into()]);
        let b = definition_hash("lens", "x => f(x)", &["depB".into()]);
        assert_ne!(a, b, "editing a dependency must change the dependent's hash");
    }
}
