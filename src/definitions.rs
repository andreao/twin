//! Content-addressed definitions and the namespace (design_doc §4, C2).
//!
//! "A definition is the unit of code — one function or value.  Its identity is the
//! hash of its normalized syntax tree plus the hashes of its dependencies.
//! References are by hash, not by name; names live in a separate table — a
//! namespace."  (This is Unison's model and Git's object store, applied to live
//! code.)
//!
//! Consequences that fall out and are tested here:
//!   * renaming is free and never breaks callers (callers reference a hash)
//!   * identical code is shared automatically (same hash => one object)
//!   * a new name can point at an existing definition with zero new storage

use std::collections::HashMap;

use crate::hashing::{definition_hash, Hash};

/// A stored unit of code: normalized-source-addressed, immutable.
#[derive(Clone, Debug)]
pub struct Definition {
    pub hash: Hash,
    pub kind: String,
    pub source: String,
    pub deps: Vec<Hash>,
}

/// The content-addressed object store + a name→hash namespace (§4).
#[derive(Default)]
pub struct DefinitionStore {
    objects: HashMap<Hash, Definition>,
    /// names live in a SEPARATE table; this is what makes rename free
    namespace: HashMap<String, Hash>,
}

impl DefinitionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a definition by content; returns its hash.  Storing identical content
    /// twice is idempotent (automatic sharing).
    pub fn define(&mut self, kind: &str, source: &str, deps: &[Hash]) -> Hash {
        let hash = definition_hash(kind, source, deps);
        self.objects.entry(hash.clone()).or_insert_with(|| Definition {
            hash: hash.clone(),
            kind: kind.to_string(),
            source: source.to_string(),
            deps: deps.to_vec(),
        });
        hash
    }

    /// Bind a name to a hash in the namespace (the only place names live).
    pub fn bind_name(&mut self, name: &str, hash: &Hash) {
        self.namespace.insert(name.to_string(), hash.clone());
    }

    /// Define and name in one step (the common authoring path).
    pub fn publish(&mut self, name: &str, kind: &str, source: &str, deps: &[Hash]) -> Hash {
        let h = self.define(kind, source, deps);
        self.bind_name(name, &h);
        h
    }

    pub fn resolve(&self, name: &str) -> Option<&Definition> {
        let h = self.namespace.get(name)?;
        self.objects.get(h)
    }

    pub fn get(&self, hash: &Hash) -> Option<&Definition> {
        self.objects.get(hash)
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_code_shared_automatically() {
        let mut s = DefinitionStore::new();
        let a = s.publish("inc", "lens", "x => x + 1", &[]);
        let b = s.publish("plusOne", "lens", "x => x + 1", &[]);
        assert_eq!(a, b, "same content => same object");
        assert_eq!(s.object_count(), 1, "stored once, shared by two names");
    }

    #[test]
    fn rename_is_free_and_keeps_object() {
        let mut s = DefinitionStore::new();
        let h = s.publish("inc", "lens", "x => x + 1", &[]);
        // "rename" = bind a new name to the same hash; the old object is untouched
        s.bind_name("increment", &h);
        assert_eq!(s.resolve("increment").unwrap().hash, h);
        assert_eq!(s.object_count(), 1);
    }

    #[test]
    fn edit_makes_new_object_old_remains() {
        let mut s = DefinitionStore::new();
        let v1 = s.publish("inc", "lens", "x => x + 1", &[]);
        let v2 = s.publish("inc", "lens", "x => x + 2", &[]);
        assert_ne!(v1, v2);
        assert_eq!(s.object_count(), 2, "old version retained (history/replay)");
        assert_eq!(s.resolve("inc").unwrap().hash, v2, "name now points at v2");
        assert!(s.get(&v1).is_some(), "old code still resolvable by hash");
    }
}
