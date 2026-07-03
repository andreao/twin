//! The incremental engine (design_doc §3, C1) — part of the trusted kernel (§4.1).
//!
//! "A spreadsheet taken seriously": every value is a cell depending on other cells;
//! changing an input recomputes only the cells that depend on it — and only if their
//! *output* actually changed.  Two properties erase the build/run distinction:
//!
//!   * demand-driven  — a node computes only when observed; unobserved cost nothing.
//!   * early cutoff    — recomputing to the same output stops propagation.
//!
//! Each node carries two stamps: when its value last *changed* and last *verified*.
//! Reading verifies-without-recomputing when no dependency changed since last verify
//! (Salsa/Adapton red-green).  Dependencies are dynamic: re-recorded each compute.
//!
//! Edit cost is proportional to the blast radius (validated A1: ~few % recompute on
//! a typical edit; 0 downstream when the output did not change).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Num(f64),
    Text(String),
}

type Compute = Rc<dyn Fn(&Engine) -> Value>;

pub struct NodeData {
    value: Value,
    changed: u64,  // revision when the output last changed (0 = never computed)
    verified: u64, // revision when last verified current
    deps: Vec<Node>,
    compute: Option<Compute>, // None => an input cell
}

pub type Node = Rc<RefCell<NodeData>>;

pub struct Engine {
    revision: Cell<u64>,
    stack: RefCell<Vec<Node>>,
    pub recompute_count: Cell<u64>,
    pub verify_count: Cell<u64>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        // revisions start at 1 so 0 is a reserved "never computed" sentinel.
        Engine {
            revision: Cell::new(1),
            stack: RefCell::new(Vec::new()),
            recompute_count: Cell::new(0),
            verify_count: Cell::new(0),
        }
    }

    pub fn input(&self, v: Value) -> Node {
        let rev = self.revision.get();
        Rc::new(RefCell::new(NodeData {
            value: v,
            changed: rev,
            verified: rev,
            deps: Vec::new(),
            compute: None,
        }))
    }

    pub fn derived<F: Fn(&Engine) -> Value + 'static>(&self, f: F) -> Node {
        Rc::new(RefCell::new(NodeData {
            value: Value::Num(0.0),
            changed: 0,
            verified: 0,
            deps: Vec::new(),
            compute: Some(Rc::new(f)),
        }))
    }

    pub fn set(&self, node: &Node, v: Value) {
        {
            let n = node.borrow();
            if n.compute.is_some() {
                panic!("cannot set a derived node");
            }
            if n.value == v {
                return; // no-op edit: nothing changed (early cutoff at the source)
            }
        }
        let rev = self.revision.get() + 1;
        self.revision.set(rev);
        let mut n = node.borrow_mut();
        n.value = v;
        n.changed = rev;
        n.verified = rev;
    }

    pub fn get(&self, node: &Node) -> Value {
        self.ensure_current(node);
        if let Some(top) = self.stack.borrow().last() {
            top.borrow_mut().deps.push(node.clone()); // record dynamic dependency
        }
        node.borrow().value.clone()
    }

    fn ensure_current(&self, node: &Node) {
        let (is_input, changed, verified) = {
            let n = node.borrow();
            (n.compute.is_none(), n.changed, n.verified)
        };
        if is_input {
            return;
        }
        let rev = self.revision.get();
        if changed == 0 {
            self.recompute(node);
            return;
        }
        if verified == rev {
            return; // already verified this revision -> cache
        }
        if !self.deps_changed(node, verified) {
            self.verify_count.set(self.verify_count.get() + 1);
            node.borrow_mut().verified = rev; // verified WITHOUT recompute (cheap path)
            return;
        }
        self.recompute(node);
    }

    fn deps_changed(&self, node: &Node, verified: u64) -> bool {
        let deps: Vec<Node> = node.borrow().deps.clone();
        for dep in &deps {
            self.ensure_current(dep);
            if dep.borrow().changed > verified {
                return true;
            }
        }
        false
    }

    fn recompute(&self, node: &Node) {
        self.recompute_count.set(self.recompute_count.get() + 1);
        let compute = node.borrow().compute.clone().unwrap();
        node.borrow_mut().deps.clear();
        self.stack.borrow_mut().push(node.clone());
        let new_value = compute(self);
        self.stack.borrow_mut().pop();
        let rev = self.revision.get();
        let mut n = node.borrow_mut();
        n.verified = rev;
        if n.changed == 0 || n.value != new_value {
            n.value = new_value;
            n.changed = rev; // output genuinely changed -> propagate
        }
        // else: early cutoff — output unchanged, downstream stays valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_cutoff_on_noop_output() {
        let e = Engine::new();
        let a = e.input(Value::Num(5.0));
        let a2 = a.clone();
        let sign = e.derived(move |eng| {
            Value::Text(if matches!(eng.get(&a2), Value::Num(x) if x > 0.0) { "pos".into() } else { "neg".into() })
        });
        let sign2 = sign.clone();
        let up = e.derived(move |eng| eng.get(&sign2));
        assert_eq!(e.get(&up), Value::Text("pos".into()));
        e.recompute_count.set(0);
        e.set(&a, Value::Num(9999.0)); // still positive -> sign output unchanged
        assert_eq!(e.get(&up), Value::Text("pos".into()));
        assert_eq!(e.recompute_count.get(), 1, "only sign recomputes; up is cut off");
    }

    #[test]
    fn verify_without_recompute_for_unrelated_edit() {
        let e = Engine::new();
        let a = e.input(Value::Num(1.0));
        let unrelated = e.input(Value::Num(0.0));
        let a2 = a.clone();
        let d = e.derived(move |eng| match eng.get(&a2) { Value::Num(x) => Value::Num(x + 100.0), v => v });
        assert_eq!(e.get(&d), Value::Num(101.0));
        e.recompute_count.set(0);
        e.set(&unrelated, Value::Num(7.0));
        assert_eq!(e.get(&d), Value::Num(101.0));
        assert_eq!(e.recompute_count.get(), 0, "unrelated edit must not recompute d");
    }

    #[test]
    fn demand_driven_unobserved_costs_nothing() {
        let e = Engine::new();
        let a = e.input(Value::Num(1.0));
        let calls = Rc::new(Cell::new(0));
        let calls2 = calls.clone();
        let a2 = a.clone();
        let _never = e.derived(move |eng| {
            calls2.set(calls2.get() + 1);
            eng.get(&a2)
        });
        e.set(&a, Value::Num(2.0));
        e.set(&a, Value::Num(3.0));
        assert_eq!(calls.get(), 0, "unobserved node never computes");
    }
}
