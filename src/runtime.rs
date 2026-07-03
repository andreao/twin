//! The execution substrate: raw V8, driven from Rust (design_doc §14).
//!
//! Node is rejected (§14): its module identity is a *path*, but we need a content
//! hash (C2); its global realm carries ambient authority we cannot cleanly remove,
//! but governance means *constructing* a Context's globals (§6.1).  So we run on V8
//! directly and reimplement the runtime around it.  V8 gives two primitives that
//! map onto the model:
//!
//!   * **Isolate = one shared heap** — so pure nodes are shared across branches by
//!     reference (C4): all branch Contexts here live in ONE Isolate.
//!   * **Context = one branch's / sandbox's globals** — per-Context globals are ours
//!     to define, which is what makes governed effects (C3/§6) clean.
//!
//! A `Branch` is a Context (a 0-copy speculative version at the code level); a
//! definition is JS bound into the Context's globals by name; hot-swap is rebinding
//! the name to new source (§5.1), observed live.

use std::sync::Once;

static V8_INIT: Once = Once::new();

fn ensure_v8() {
    V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// A speculative version at the code level: a V8 Context with its own globals.
pub struct Branch {
    ctx: v8::Global<v8::Context>,
    pub name: String,
    pub governed: bool,
}

/// Owns one Isolate (a shared heap) hosting many branch Contexts (§14).
pub struct Runtime {
    isolate: v8::OwnedIsolate,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        ensure_v8();
        Runtime {
            isolate: v8::Isolate::new(Default::default()),
        }
    }

    /// Mint a branch (a Context).  A *governed* branch has its ambient
    /// nondeterminism deleted and a seeded, recording `ctx` injected (§6).
    pub fn branch(&mut self, name: &str, governed: bool) -> Branch {
        let global = {
            let scope = &mut v8::HandleScope::new(&mut self.isolate);
            let ctx = v8::Context::new(scope, Default::default());
            v8::Global::new(scope, ctx)
        };
        let branch = Branch {
            ctx: global,
            name: name.to_string(),
            governed,
        };
        if governed {
            // construct the allowed globals (§6.1): delete ambient entropy/clock and
            // install the only governed door — `ctx` — as a seeded, recording PRNG.
            let _ = self.eval(&branch, GOVERN_SETUP);
        }
        branch
    }

    /// Seed (or re-seed) a governed branch's effect source.  Call before use.
    pub fn seed(&mut self, b: &Branch, seed: u64) {
        let _ = self.eval(b, &format!("__seed({}n);", seed));
    }

    /// Evaluate source in a branch; returns the result as a string, or an error.
    pub fn eval(&mut self, b: &Branch, src: &str) -> Result<String, String> {
        let scope = &mut v8::HandleScope::new(&mut self.isolate);
        let ctx = v8::Local::new(scope, &b.ctx);
        let scope = &mut v8::ContextScope::new(scope, ctx);
        let tc = &mut v8::TryCatch::new(scope);
        let code = match v8::String::new(tc, src) {
            Some(s) => s,
            None => return Err("source too large".into()),
        };
        let script = match v8::Script::compile(tc, code, None) {
            Some(s) => s,
            None => return Err(exception(tc)),
        };
        match script.run(tc) {
            Some(v) => Ok(v.to_rust_string_lossy(tc)),
            None => Err(exception(tc)),
        }
    }

    /// Bind a definition (a JS expression, usually a function) to a global name —
    /// this is "configuring a resident node with a content-addressed definition"
    /// (§4.1).  Re-binding the same name is a **hot-swap** (§5.1).
    pub fn bind(&mut self, b: &Branch, name: &str, source: &str) -> Result<(), String> {
        self.eval(b, &format!("globalThis[{:?}] = ({});", name, source))
            .map(|_| ())
    }

    /// Read the recorded governed-effect log of a branch (§6/§8) as JSON.
    pub fn effect_log(&mut self, b: &Branch) -> String {
        self.eval(b, "JSON.stringify(globalThis.__effectLog || [])")
            .unwrap_or_else(|_| "[]".into())
    }

    /// Share a heap object from one branch into another *by reference* (V2): proves
    /// pure structure is shared across branches in one Isolate, not copied (§14).
    pub fn read_shared_field(&mut self, producer: &Branch, expr: &str, consumer: &Branch,
                             field: &str) -> Result<String, String> {
        // create the object in the producer context, keep a Global handle
        let global_obj = {
            let scope = &mut v8::HandleScope::new(&mut self.isolate);
            let pctx = v8::Local::new(scope, &producer.ctx);
            let cs = &mut v8::ContextScope::new(scope, pctx);
            let code = v8::String::new(cs, expr).ok_or("bad expr")?;
            let script = v8::Script::compile(cs, code, None).ok_or("compile failed")?;
            let val = script.run(cs).ok_or("run failed")?;
            v8::Global::new(cs, val)
        };
        // read it from the CONSUMER context (a different branch, same Isolate heap)
        let scope = &mut v8::HandleScope::new(&mut self.isolate);
        let cctx = v8::Local::new(scope, &consumer.ctx);
        let cs = &mut v8::ContextScope::new(scope, cctx);
        let local = v8::Local::new(cs, &global_obj);
        let obj = local.to_object(cs).ok_or("not an object")?;
        let key = v8::String::new(cs, field).ok_or("bad field")?;
        let got = obj.get(cs, key.into()).ok_or("no field")?;
        Ok(got.to_rust_string_lossy(cs))
    }
}

fn exception(tc: &mut v8::TryCatch<v8::HandleScope>) -> String {
    if let Some(ex) = tc.exception() {
        let s = ex.to_rust_string_lossy(tc);
        format!("JS exception: {}", s)
    } else {
        "unknown JS error".into()
    }
}

/// The governed-Context bootstrap (§6.1): delete ambient nondeterminism, install a
/// seeded recording `ctx`.  This is the "construct the allowed context" disposition.
const GOVERN_SETUP: &str = r#"
(() => {
  // delete ambient authority (entropy, clock) — the strongest enforcement (§6.1)
  globalThis.Date = undefined;
  Math.random = () => { throw new Error('ungoverned entropy: use ctx.random()'); };
  // governed, recording effect source — the only door
  globalThis.__effectLog = [];
  let __s = 0n;
  let __clock = 1700000000000n;
  globalThis.__seed = (s) => { __s = BigInt(s); };
  const rec = (v) => { globalThis.__effectLog.push(v); return v; };
  globalThis.ctx = {
    random: () => {
      __s = (__s * 6364136223846793005n + 1442695040888963407n) & ((1n<<64n)-1n);
      return rec(Number(__s >> 11n) / 9007199254740992);
    },
    now: () => { __clock = __clock + 1n; return rec(Number(__clock)); },
    uuid: () => {
      __s = (__s * 2862933555777941757n + 3037000493n) & ((1n<<64n)-1n);
      return rec('id-' + __s.toString(16).padStart(16,'0'));
    },
  };
})();
"#;
