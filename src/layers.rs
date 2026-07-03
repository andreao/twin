//! Layered state and 0-copy branches (design_doc §7, C7).
//!
//! Resident state is a stack of layers, resolved top-down, each an independent
//! persistent map: ephemeral ▸ branch ▸ user ▸ shared.  A read returns the first
//! layer that has the key; a tombstone in a higher layer hides it in all lower ones.
//! Branching is 0-copy: a fork is fresh empty top layers over Arc pointers to the
//! shared lower layers, so divergence costs only what is written.

use crate::pmap::PersistentMap;

pub const EPHEMERAL: usize = 0;
pub const BRANCH: usize = 1;
pub const USER: usize = 2;
pub const SHARED: usize = 3;

/// A value cell: a real value, or a tombstone that hides lower layers (§7).
#[derive(Clone, Debug, PartialEq)]
pub enum Cell<V> {
    Val(V),
    Tombstone,
}

type Map<V> = PersistentMap<String, Cell<V>>;

/// A speculative version: a stack of persistent-map layer pointers.
pub struct Branch<V: Clone> {
    pub name: String,
    layers: [Map<V>; 4], // [ephemeral, branch, user, shared]
}

impl<V: Clone + PartialEq> Branch<V> {
    /// Top-down resolution with tombstones (§7).
    pub fn get(&self, key: &str) -> Option<V> {
        for layer in &self.layers {
            match layer.get(&key.to_string()) {
                Some(Cell::Tombstone) => return None, // hidden — stop the walk
                Some(Cell::Val(v)) => return Some(v.clone()),
                None => continue,
            }
        }
        None
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Which layer a resolved key came from (provenance of state).
    pub fn layer_of(&self, key: &str) -> Option<usize> {
        for (i, layer) in self.layers.iter().enumerate() {
            match layer.get(&key.to_string()) {
                Some(Cell::Tombstone) => return None,
                Some(Cell::Val(_)) => return Some(i),
                None => continue,
            }
        }
        None
    }

    /// Write to a layer — *which* layer is the persistence policy (§7/§11.8).
    pub fn write(&mut self, key: &str, value: V, layer: usize) {
        self.layers[layer] = self.layers[layer].set(key.to_string(), Cell::Val(value));
    }

    /// Locally delete by writing a tombstone (usually in an upper layer).
    pub fn delete(&mut self, key: &str, layer: usize) {
        self.layers[layer] = self.layers[layer].set(key.to_string(), Cell::Tombstone);
    }

    /// 0-copy fork: fresh empty ephemeral over Arc pointers to the lower layers.
    /// The child inherits the parent's branch writes as an immutable snapshot.
    pub fn fork(&self, name: &str) -> Branch<V> {
        Branch {
            name: name.to_string(),
            layers: [
                PersistentMap::new(),        // fresh ephemeral
                self.layers[BRANCH].clone(), // inherited branch snapshot (Arc share)
                self.layers[USER].clone(),
                self.layers[SHARED].clone(),
            ],
        }
    }

    pub fn drop_ephemeral(&mut self) {
        self.layers[EPHEMERAL] = PersistentMap::new(); // reset one pointer
    }

    /// Diff this branch's visible state against a base branch, as data (§11.5).
    pub fn diff(&self, base: &Branch<V>) -> Vec<(String, Option<V>, Option<V>)> {
        let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for layer in self.layers.iter().chain(base.layers.iter()) {
            for (k, _) in layer.entries() {
                keys.insert(k);
            }
        }
        let mut out = Vec::new();
        for k in keys {
            let a = base.get(&k);
            let b = self.get(&k);
            if a != b {
                out.push((k, a, b));
            }
        }
        out
    }
}

/// Holds the shared/user roots and mints branches over them (§7).
pub struct LayeredStore<V: Clone> {
    shared: Map<V>,
    user: Map<V>,
}

impl<V: Clone + PartialEq> Default for LayeredStore<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone + PartialEq> LayeredStore<V> {
    pub fn new() -> Self {
        LayeredStore { shared: PersistentMap::new(), user: PersistentMap::new() }
    }

    /// The system of record.  Promotion lands here (§7).
    pub fn write_shared(&mut self, key: &str, value: V) {
        self.shared = self.shared.set(key.to_string(), Cell::Val(value));
    }

    pub fn shared_len(&self) -> usize {
        self.shared.len()
    }

    /// Mint a 0-copy branch — all branches over the same base share the lower
    /// layers by Arc reference (this is where the million-branch result comes from).
    pub fn branch(&self, name: &str) -> Branch<V> {
        Branch {
            name: name.to_string(),
            layers: [
                PersistentMap::new(),
                PersistentMap::new(),
                self.user.clone(),   // pointer, not a copy
                self.shared.clone(), // pointer, not a copy
            ],
        }
    }

    /// Promote a value up the stack — explicit, never a copy (§7).
    pub fn promote_to_shared(&mut self, branch: &Branch<V>, key: &str) {
        match branch.get(key) {
            Some(v) => self.shared = self.shared.set(key.to_string(), Cell::Val(v)),
            None => self.shared = self.shared.without(&key.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> LayeredStore<String> {
        let mut s = LayeredStore::new();
        s.write_shared("pump-1", "ok".into());
        s.write_shared("pump-2", "ok".into());
        s
    }

    #[test]
    fn top_down_resolution_and_isolation() {
        let s = store();
        let mut b = s.branch("b1");
        assert_eq!(b.get("pump-1"), Some("ok".into()));
        b.write("pump-1", "alarm".into(), BRANCH);
        assert_eq!(b.get("pump-1"), Some("alarm".into()));
        assert_eq!(b.layer_of("pump-1"), Some(BRANCH));
        // shared untouched -> a fresh branch still sees ok
        assert_eq!(s.branch("b2").get("pump-1"), Some("ok".into()));
    }

    #[test]
    fn tombstone_local_delete() {
        let s = store();
        let mut b = s.branch("b1");
        b.delete("pump-1", BRANCH);
        assert!(!b.contains("pump-1"));
        assert!(s.branch("b2").contains("pump-1")); // shared still has it
    }

    #[test]
    fn fork_is_zero_copy_and_isolated() {
        let s = store();
        let mut b = s.branch("b1");
        b.write("pump-1", "watch".into(), BRANCH);
        let mut child = b.fork("b1-child");
        assert_eq!(child.get("pump-1"), Some("watch".into())); // inherits parent write
        child.write("pump-1", "alarm".into(), BRANCH);
        assert_eq!(b.get("pump-1"), Some("watch".into())); // parent unaffected
        assert_eq!(child.get("pump-1"), Some("alarm".into()));
    }

    #[test]
    fn promotion_and_diff() {
        let mut s = store();
        let mut b = s.branch("b1");
        b.write("pump-3", "ok".into(), BRANCH);
        let base = s.branch("base");
        let d = b.diff(&base);
        assert!(d.iter().any(|(k, _, _)| k == "pump-3"));
        s.promote_to_shared(&b, "pump-3");
        assert!(s.branch("other").contains("pump-3"));
    }
}
