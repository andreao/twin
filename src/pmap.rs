//! Persistent map with structural sharing (design_doc §7, C7).
//!
//! A branch is "two empty top layers over pointers to shared lower layers; forking
//! copies pointers, divergence costs only what is written."  That works because
//! every layer is a persistent, structurally-shared map: `set`/`without` return a
//! new map sharing all untouched subtrees (path copying via `Arc`), so snapshots
//! (§8) and branch roots (§7) are cheap and never torn.
//!
//! A bitmap-indexed hash-array-mapped trie (HAMT), the structure behind persistent
//! maps in Clojure/Scala.  Deep hash collisions fall back to a small bucket.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const BITS: u32 = 5;
const MASK: u64 = 31;
const MAX_SHIFT: u32 = 60;

fn hash_key<K: Hash>(k: &K) -> u64 {
    let mut h = DefaultHasher::new();
    k.hash(&mut h);
    h.finish()
}

fn idx(h: u64, shift: u32) -> u64 {
    (h >> shift) & MASK
}
fn bitpos(h: u64, shift: u32) -> u32 {
    1u32 << idx(h, shift)
}

enum Node<K, V> {
    Leaf { h: u64, k: K, v: V },
    // A collision bucket stores each entry's full hash, so matching is by key and
    // correct even for the (SipHash-unreachable) case of distinct hashes that agree
    // in the low 60 bits and thus exhaust the trie at MAX_SHIFT.
    Collision { entries: Vec<(u64, K, V)> },
    Bitmap { bitmap: u32, children: Vec<Arc<Node<K, V>>> },
}

fn get<'a, K: Eq, V>(mut node: &'a Node<K, V>, mut shift: u32, h: u64, key: &K) -> Option<&'a V> {
    loop {
        match node {
            Node::Leaf { h: lh, k, v } => return if *lh == h && k == key { Some(v) } else { None },
            Node::Collision { entries } => {
                // match by key (authoritative), regardless of the entries' hashes
                return entries.iter().find(|(_, k, _)| k == key).map(|(_, _, v)| v);
            }
            Node::Bitmap { bitmap, children } => {
                let bit = bitpos(h, shift);
                if bitmap & bit == 0 {
                    return None;
                }
                let i = (bitmap & (bit - 1)).count_ones() as usize;
                node = &children[i];
                shift += BITS;
            }
        }
    }
}

fn merge_leaves<K: Clone, V: Clone>(shift: u32, ah: u64, ak: K, av: V, h: u64, k: K, v: V) -> Arc<Node<K, V>> {
    if shift >= MAX_SHIFT {
        return Arc::new(Node::Collision { entries: vec![(ah, ak, av), (h, k, v)] });
    }
    let abit = bitpos(ah, shift);
    let nbit = bitpos(h, shift);
    if abit == nbit {
        let child = merge_leaves(shift + BITS, ah, ak, av, h, k, v);
        Arc::new(Node::Bitmap { bitmap: abit, children: vec![child] })
    } else {
        let a = Arc::new(Node::Leaf { h: ah, k: ak, v: av });
        let b = Arc::new(Node::Leaf { h, k, v });
        let children = if abit < nbit { vec![a, b] } else { vec![b, a] };
        Arc::new(Node::Bitmap { bitmap: abit | nbit, children })
    }
}

fn set<K: Clone + Eq, V: Clone>(node: Option<&Arc<Node<K, V>>>, shift: u32, h: u64, key: K, val: V) -> Arc<Node<K, V>> {
    match node.map(|a| a.as_ref()) {
        None => Arc::new(Node::Leaf { h, k: key, v: val }),
        Some(Node::Leaf { h: lh, k, v }) => {
            if *lh == h && *k == key {
                Arc::new(Node::Leaf { h, k: key, v: val })
            } else if *lh == h {
                Arc::new(Node::Collision { entries: vec![(*lh, k.clone(), v.clone()), (h, key, val)] })
            } else {
                merge_leaves(shift, *lh, k.clone(), v.clone(), h, key, val)
            }
        }
        Some(Node::Collision { entries }) => {
            // a reachable collision at shift < MAX_SHIFT means all entries share one
            // hash; a key with a different hash is distinguishable here, so lift into
            // a bitmap.  At MAX_SHIFT (exhausted bits) we can only append by key.
            let bucket_hash = entries[0].0;
            if shift < MAX_SHIFT && h != bucket_hash {
                let bit = bitpos(bucket_hash, shift);
                let lifted = Arc::new(Node::Bitmap { bitmap: bit, children: vec![node.unwrap().clone()] });
                set(Some(&lifted), shift, h, key, val)
            } else {
                let mut e: Vec<(u64, K, V)> = entries.iter().filter(|(_, k, _)| *k != key).cloned().collect();
                e.push((h, key, val));
                Arc::new(Node::Collision { entries: e })
            }
        }
        Some(Node::Bitmap { bitmap, children }) => {
            let bit = bitpos(h, shift);
            let i = (bitmap & (bit - 1)).count_ones() as usize;
            let mut ch = children.clone();
            if bitmap & bit != 0 {
                ch[i] = set(Some(&children[i]), shift + BITS, h, key, val);
                Arc::new(Node::Bitmap { bitmap: *bitmap, children: ch })
            } else {
                ch.insert(i, Arc::new(Node::Leaf { h, k: key, v: val }));
                Arc::new(Node::Bitmap { bitmap: bitmap | bit, children: ch })
            }
        }
    }
}

fn without<K: Clone + Eq, V: Clone>(node: &Arc<Node<K, V>>, shift: u32, h: u64, key: &K) -> (Option<Arc<Node<K, V>>>, bool) {
    match node.as_ref() {
        Node::Leaf { h: lh, k, .. } => {
            if *lh == h && k == key { (None, true) } else { (Some(node.clone()), false) }
        }
        Node::Collision { entries } => {
            let e: Vec<(u64, K, V)> = entries.iter().filter(|(_, k, _)| k != key).cloned().collect();
            if e.len() == entries.len() {
                (Some(node.clone()), false)
            } else if e.len() == 1 {
                let (eh, k, v) = e.into_iter().next().unwrap();
                (Some(Arc::new(Node::Leaf { h: eh, k, v })), true) // remaining entry's own hash
            } else {
                (Some(Arc::new(Node::Collision { entries: e })), true)
            }
        }
        Node::Bitmap { bitmap, children } => {
            let bit = bitpos(h, shift);
            if bitmap & bit == 0 {
                return (Some(node.clone()), false);
            }
            let i = (bitmap & (bit - 1)).count_ones() as usize;
            let (child, removed) = without(&children[i], shift + BITS, h, key);
            if !removed {
                return (Some(node.clone()), false);
            }
            let mut ch = children.clone();
            match child {
                None => {
                    ch.remove(i);
                    let nb = bitmap & !bit;
                    if ch.is_empty() {
                        (None, true)
                    } else if ch.len() == 1 && matches!(ch[0].as_ref(), Node::Leaf { .. }) {
                        (Some(ch.into_iter().next().unwrap()), true)
                    } else {
                        (Some(Arc::new(Node::Bitmap { bitmap: nb, children: ch })), true)
                    }
                }
                Some(c) => {
                    ch[i] = c;
                    (Some(Arc::new(Node::Bitmap { bitmap: *bitmap, children: ch })), true)
                }
            }
        }
    }
}

fn collect<K: Clone, V: Clone>(node: &Node<K, V>, out: &mut Vec<(K, V)>) {
    match node {
        Node::Leaf { k, v, .. } => out.push((k.clone(), v.clone())),
        Node::Collision { entries } => out.extend(entries.iter().map(|(_, k, v)| (k.clone(), v.clone()))),
        Node::Bitmap { children, .. } => {
            for c in children {
                collect(c, out);
            }
        }
    }
}

/// An immutable map; mutators return a new map sharing untouched structure.
pub struct PersistentMap<K, V> {
    root: Option<Arc<Node<K, V>>>,
    count: usize,
}

impl<K, V> Clone for PersistentMap<K, V> {
    fn clone(&self) -> Self {
        // O(1): shares the whole trie by Arc (this is the "0-copy fork" primitive).
        PersistentMap { root: self.root.clone(), count: self.count }
    }
}

impl<K: Clone + Eq + Hash, V: Clone> PersistentMap<K, V> {
    pub fn new() -> Self {
        PersistentMap { root: None, count: 0 }
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        self.root.as_ref().and_then(|r| get(r, 0, hash_key(key), key))
    }
    pub fn contains(&self, key: &K) -> bool {
        self.get(key).is_some()
    }
    pub fn set(&self, key: K, val: V) -> Self {
        let h = hash_key(&key);
        let existed = self.get(&key).is_some();
        let root = set(self.root.as_ref(), 0, h, key, val);
        PersistentMap { root: Some(root), count: self.count + if existed { 0 } else { 1 } }
    }
    pub fn without(&self, key: &K) -> Self {
        let h = hash_key(key);
        match &self.root {
            None => self.clone(),
            Some(r) => {
                let (root, removed) = without(r, 0, h, key);
                if removed {
                    PersistentMap { root, count: self.count - 1 }
                } else {
                    self.clone()
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn entries(&self) -> Vec<(K, V)> {
        let mut out = Vec::new();
        if let Some(r) = &self.root {
            collect(r, &mut out);
        }
        out
    }
}

impl<K: Clone + Eq + Hash, V: Clone> Default for PersistentMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_delete() {
        let m = PersistentMap::new().set("a", 1).set("b", 2);
        assert_eq!(m.get(&"a"), Some(&1));
        assert_eq!(m.len(), 2);
        let m2 = m.without(&"a");
        assert_eq!(m2.get(&"a"), None);
        assert_eq!(m2.len(), 1);
        assert_eq!(m.get(&"a"), Some(&1)); // original unchanged (snapshot isolation)
    }

    #[test]
    fn many_keys_roundtrip() {
        let mut m = PersistentMap::new();
        for i in 0..2000 {
            m = m.set(i, i * i);
        }
        for i in 0..2000 {
            assert_eq!(m.get(&i), Some(&(i * i)));
        }
        assert_eq!(m.len(), 2000);
        for i in (0..2000).step_by(2) {
            m = m.without(&i);
        }
        assert_eq!(m.len(), 1000);
        assert_eq!(m.get(&0), None);
        assert_eq!(m.get(&1), Some(&1));
    }

    // A key whose hash is a constant -> every instance collides in one bucket,
    // exercising the Collision path (insert, replace, remove, collapse-to-leaf).
    #[derive(Clone, PartialEq, Eq, Debug)]
    struct ConstHash(u32);
    impl Hash for ConstHash {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u8(0); // identical for all instances
        }
    }

    #[test]
    fn collision_bucket_insert_get_remove() {
        let mut m: PersistentMap<ConstHash, u32> = PersistentMap::new();
        for i in 0..50 {
            m = m.set(ConstHash(i), i);
        }
        assert_eq!(m.len(), 50);
        for i in 0..50 {
            assert_eq!(m.get(&ConstHash(i)), Some(&i));
        }
        m = m.set(ConstHash(10), 999); // replace within the bucket
        assert_eq!(m.get(&ConstHash(10)), Some(&999));
        assert_eq!(m.len(), 50);
        for i in 0..49 {
            m = m.without(&ConstHash(i));
        }
        assert_eq!(m.len(), 1); // collapses down to the last entry
        assert_eq!(m.get(&ConstHash(49)), Some(&49));
        assert_eq!(m.get(&ConstHash(0)), None);
    }

    #[test]
    fn structural_sharing_is_cheap_clone() {
        let mut base = PersistentMap::new();
        for i in 0..1000 {
            base = base.set(i, i);
        }
        let forked = base.clone().set(9999, 1); // shares base structurally
        assert_eq!(forked.get(&500), Some(&500));
        assert_eq!(forked.get(&9999), Some(&1));
        assert_eq!(base.get(&9999), None); // base unaffected
    }
}
