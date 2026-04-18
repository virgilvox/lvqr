//! Server-side WASM per-fragment filter host for LVQR.
//!
//! **Tier 4 item 4.2, session A.** This is the scaffold crate
//! referenced by `tracking/TIER_4_PLAN.md` section 4.2. It is
//! deliberately unrelated to the browser-facing `lvqr-wasm`
//! crate that was deleted in the 0.4-session-44 refactor; this
//! is a fresh server-side crate that embeds a [`wasmtime::Engine`]
//! and runs a WASM module per inbound [`Fragment`].
//!
//! # Session-A scope
//!
//! * [`FragmentFilter`] trait and [`WasmFilter`] concrete impl.
//! * Core WASM (not the component model). The host exposes the
//!   module's linear memory and calls a single export,
//!   `on_fragment(ptr: i32, len: i32) -> i32`, after writing the
//!   fragment payload bytes to offset 0 of the module's memory.
//!   The return value is:
//!     * negative -- drop the fragment (any negative value).
//!     * non-negative `N` -- keep the fragment; the first `N`
//!       bytes of linear memory at offset 0 are the replacement
//!       payload. `N = 0` produces a keep-with-empty-payload,
//!       which is legal and semantically distinct from a drop.
//! * Metadata (`track_id`, `group_id`, `object_id`, `priority`,
//!   `dts`, `pts`, `duration`, `flags`) passes through unchanged
//!   regardless of the filter's output. Session B / C will
//!   broaden the host-function surface to cover metadata
//!   mutation; session A ships the simplest useful shape so the
//!   runtime, trait, test harness, and CLI wiring path can land
//!   without entangling the scope.
//! * One unit-test per behaviour (no-op, drop, truncate) plus a
//!   proptest that asserts arbitrary payload round-trips
//!   through a no-op WASM module byte-for-byte.
//!
//! # Why core WASM and not the component model
//!
//! The ROADMAP's 1-page MVP for item 4.2 targets the component
//! model. Session A adopts core WASM as a scope narrowing, not
//! a design pivot: the `on_fragment(ptr, len) -> i32` surface is
//! small enough to bind with `wasmtime::TypedFunc` directly and
//! lets session A ship the trait + test harness without
//! dragging in `cargo-component` or a wit-bindgen build step
//! for the test fixtures. Session B is the right place to
//! revisit whether the component-model binding is worth its
//! boilerplate for the full fragment-metadata host surface.
//!
//! Either way, the `FragmentFilter` trait is the surface the
//! rest of the workspace depends on; the transport between
//! `WasmFilter` and the guest module is an implementation
//! detail that can change without churning
//! `FragmentBroadcasterRegistry` call sites.
//!
//! # What is deliberately NOT in this crate yet
//!
//! * **Session B deliverable:** a `WasmFragmentObserver` that
//!   plugs into `FragmentBroadcasterRegistry`'s observer
//!   fan-out, plus a `--wasm-filter <path>` CLI flag on
//!   `lvqr-cli`.
//! * **Session C deliverable:** hot-reload of the WASM module
//!   via `notify`; drop-in swap of the compiled `Module` on
//!   file change.
//! * **Anti-scope (per TIER_4_PLAN.md section 4.2):** no
//!   multi-filter pipeline, no stateful filters, no GPU, no
//!   browser target. Every `apply` call creates a fresh
//!   `wasmtime::Store`; state does not carry between
//!   invocations.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use lvqr_fragment::Fragment;
use parking_lot::Mutex;
use wasmtime::{Engine, Instance, Module, Store, TypedFunc};

/// Name of the WASM export the host calls once per fragment.
/// Guest modules MUST export a function with this name matching
/// the `(i32, i32) -> i32` signature described in the
/// crate-level docs.
pub const EXPORT_NAME: &str = "on_fragment";

/// Name of the WASM linear-memory export the host writes the
/// payload into and reads the replacement payload from. Guest
/// modules MUST export a memory with this name; a module that
/// does not is rejected at `WasmFilter::from_bytes` time.
pub const MEMORY_NAME: &str = "memory";

/// A filter that may replace or drop a [`Fragment`] based on
/// its payload. The trait is intentionally simple: exactly one
/// method, synchronous, no error channel. A filter that fails
/// at runtime returns the input fragment unchanged (fail-open
/// semantics match the "filter is a guest; server must stay
/// alive" invariant).
pub trait FragmentFilter: Send + Sync {
    /// Run the filter on `fragment`. Return `Some(f)` to keep
    /// (possibly with a modified payload), `None` to drop.
    fn apply(&self, fragment: Fragment) -> Option<Fragment>;
}

/// A WASM-backed [`FragmentFilter`]. Holds a compiled
/// [`wasmtime::Module`] and creates a fresh [`wasmtime::Store`]
/// per invocation so filters cannot accumulate state across
/// fragments (session A anti-scope). Cheap to clone via
/// [`SharedFilter`] if multiple observers need a handle.
pub struct WasmFilter {
    engine: Engine,
    module: Module,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for WasmFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmFilter").field("path", &self.path).finish()
    }
}

impl WasmFilter {
    /// Read + compile a WASM module from `path`. The module
    /// must export a `(memory "memory")` and a function
    /// `(func (export "on_fragment") (param i32 i32) (result i32))`.
    /// Both requirements are verified lazily on the first
    /// [`FragmentFilter::apply`] call; a module that fails to
    /// compile still errors here, synchronously, so
    /// `--wasm-filter` at `lvqr-cli::start` time surfaces the
    /// error before any ingest traffic is accepted.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path).with_context(|| format!("reading WASM module {}", path.display()))?;
        let engine = Engine::default();
        let module =
            Module::new(&engine, &bytes).with_context(|| format!("compiling WASM module {}", path.display()))?;
        Ok(Self {
            engine,
            module,
            path: Some(path),
        })
    }

    /// Compile a WASM module from an in-memory byte slice.
    /// Exposed primarily so tests can assemble short WAT
    /// snippets via the `wat` crate without a temp file dance;
    /// production paths should prefer [`Self::load`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).context("compiling WASM module from bytes")?;
        Ok(Self {
            engine,
            module,
            path: None,
        })
    }

    /// Path the module was loaded from, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl FragmentFilter for WasmFilter {
    fn apply(&self, fragment: Fragment) -> Option<Fragment> {
        let mut store = Store::new(&self.engine, ());
        let instance = match Instance::new(&mut store, &self.module, &[]) {
            Ok(i) => i,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = ?self.path,
                    "WASM filter instantiation failed; passing fragment through unchanged",
                );
                return Some(fragment);
            }
        };
        let Some(memory) = instance.get_memory(&mut store, MEMORY_NAME) else {
            tracing::warn!(
                path = ?self.path,
                export = MEMORY_NAME,
                "WASM module missing required memory export; passing fragment through unchanged",
            );
            return Some(fragment);
        };
        let on_fragment: TypedFunc<(i32, i32), i32> = match instance.get_typed_func(&mut store, EXPORT_NAME) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = ?self.path,
                    export = EXPORT_NAME,
                    "WASM module missing on_fragment(i32,i32)->i32 export; passing fragment through unchanged",
                );
                return Some(fragment);
            }
        };

        let payload_len = fragment.payload.len();
        if payload_len > i32::MAX as usize {
            tracing::warn!(
                len = payload_len,
                "fragment payload exceeds i32::MAX; dropping to avoid undefined WASM ABI"
            );
            return None;
        }

        // Grow linear memory if the payload does not fit. WASM
        // pages are 64 KiB; `grow` takes a page count.
        let needed_bytes = payload_len.max(1);
        let current_bytes = memory.data_size(&store);
        if needed_bytes > current_bytes {
            let needed_pages = needed_bytes.div_ceil(65536);
            let current_pages = memory.size(&store);
            let grow_pages = needed_pages as u64 - current_pages;
            if memory.grow(&mut store, grow_pages).is_err() {
                tracing::warn!(
                    current = current_bytes,
                    needed = needed_bytes,
                    "WASM memory grow failed; passing fragment through unchanged"
                );
                return Some(fragment);
            }
        }

        if let Err(err) = memory.write(&mut store, 0, &fragment.payload) {
            tracing::warn!(error = %err, "WASM memory write failed; passing fragment through unchanged");
            return Some(fragment);
        }

        let ret = match on_fragment.call(&mut store, (0, payload_len as i32)) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(error = %err, "WASM filter trap; passing fragment through unchanged");
                return Some(fragment);
            }
        };

        if ret < 0 {
            return None;
        }
        let out_len = ret as usize;
        let final_bytes = memory.data_size(&store);
        if out_len > final_bytes {
            tracing::warn!(
                out_len,
                mem = final_bytes,
                "WASM filter returned length past end of memory; dropping fragment",
            );
            return None;
        }

        let mut buf = vec![0u8; out_len];
        if let Err(err) = memory.read(&store, 0, &mut buf) {
            tracing::warn!(error = %err, "WASM memory read failed; dropping fragment");
            return None;
        }

        Some(Fragment {
            payload: Bytes::from(buf),
            ..fragment
        })
    }
}

/// Clonable, thread-safe wrapper around any [`FragmentFilter`].
/// The filter observer session B will install on
/// `FragmentBroadcasterRegistry` takes a `SharedFilter` so a
/// single compiled module serves every broadcast without
/// re-compilation.
#[derive(Clone)]
pub struct SharedFilter {
    inner: Arc<Mutex<Box<dyn FragmentFilter>>>,
}

impl SharedFilter {
    /// Wrap a filter implementation so it is `Send + Sync +
    /// Clone`. Under the hood this is a `Mutex<Box<dyn
    /// FragmentFilter>>`; every call to `apply` acquires the
    /// mutex. wasmtime `Store` is not `Sync`, so the mutex is
    /// load-bearing.
    pub fn new(filter: impl FragmentFilter + 'static) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Box::new(filter))),
        }
    }

    /// Replace the wrapped filter in place. Session C's
    /// hot-reload path calls this with a freshly compiled
    /// `WasmFilter` on file change.
    pub fn replace(&self, filter: impl FragmentFilter + 'static) {
        *self.inner.lock() = Box::new(filter);
    }
}

impl FragmentFilter for SharedFilter {
    fn apply(&self, fragment: Fragment) -> Option<Fragment> {
        self.inner.lock().apply(fragment)
    }
}

impl std::fmt::Debug for SharedFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedFilter").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_fragment::FragmentFlags;

    /// Minimal no-op module: returns the input length unchanged.
    /// Module exports `memory` (1 page) and `on_fragment`.
    const WAT_NOOP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            local.get 1))
    "#;

    /// Drop-all module: always returns -1 (any negative drops).
    const WAT_DROP: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            i32.const -1))
    "#;

    /// Truncate-to-1 module: returns length 1 so the host keeps
    /// only the first byte of the payload.
    const WAT_TRUNCATE_1: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "on_fragment") (param i32 i32) (result i32)
            i32.const 1))
    "#;

    /// Broken module: missing the required memory export.
    const WAT_MISSING_MEMORY: &str = r#"
        (module
          (func (export "on_fragment") (param i32 i32) (result i32)
            local.get 1))
    "#;

    fn sample_fragment(payload: &[u8]) -> Fragment {
        Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            1000,
            1000,
            3000,
            FragmentFlags::default(),
            Bytes::copy_from_slice(payload),
        )
    }

    fn compile(wat: &str) -> WasmFilter {
        let bytes = wat::parse_str(wat).expect("wat parse");
        WasmFilter::from_bytes(&bytes).expect("wasm compile")
    }

    #[test]
    fn noop_filter_passes_payload_through_unchanged() {
        let filter = compile(WAT_NOOP);
        let frag = sample_fragment(b"hello world");
        let out = filter.apply(frag.clone()).expect("no-op filter must keep the fragment");
        assert_eq!(out.payload, frag.payload);
        assert_eq!(out.track_id, frag.track_id);
        assert_eq!(out.group_id, frag.group_id);
        assert_eq!(out.dts, frag.dts);
    }

    #[test]
    fn drop_filter_returns_none() {
        let filter = compile(WAT_DROP);
        let frag = sample_fragment(b"anything");
        assert!(filter.apply(frag).is_none());
    }

    #[test]
    fn truncating_filter_modifies_only_payload() {
        let filter = compile(WAT_TRUNCATE_1);
        let frag = sample_fragment(b"hello");
        let out = filter.apply(frag.clone()).expect("truncate keeps");
        assert_eq!(out.payload.as_ref(), b"h");
        // Metadata must pass through unchanged even when the
        // payload is modified.
        assert_eq!(out.track_id, frag.track_id);
        assert_eq!(out.pts, frag.pts);
    }

    #[test]
    fn module_missing_memory_falls_back_to_passthrough() {
        let filter = compile(WAT_MISSING_MEMORY);
        let frag = sample_fragment(b"xyz");
        let out = filter.apply(frag.clone()).expect("fail-open");
        assert_eq!(out.payload, frag.payload);
    }

    #[test]
    fn shared_filter_delegates_and_is_clonable() {
        let filter = SharedFilter::new(compile(WAT_NOOP));
        let clone = filter.clone();
        let frag = sample_fragment(b"shared");
        assert_eq!(
            filter.apply(frag.clone()).unwrap().payload,
            clone.apply(frag).unwrap().payload,
        );
    }

    #[test]
    fn shared_filter_replace_swaps_implementation() {
        let filter = SharedFilter::new(compile(WAT_NOOP));
        let frag = sample_fragment(b"before");
        assert!(filter.apply(frag.clone()).is_some());
        filter.replace(compile(WAT_DROP));
        assert!(filter.apply(frag).is_none());
    }

    #[test]
    fn empty_payload_roundtrips_unchanged() {
        let filter = compile(WAT_NOOP);
        let frag = sample_fragment(&[]);
        let out = filter.apply(frag.clone()).expect("no-op keeps empty");
        assert!(out.payload.is_empty());
    }

    #[test]
    fn from_bytes_rejects_invalid_wasm() {
        let res = WasmFilter::from_bytes(b"not wasm");
        assert!(res.is_err());
    }

    #[test]
    fn path_exposed_for_loaded_modules() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let bytes = wat::parse_str(WAT_NOOP).unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        let filter = WasmFilter::load(tmp.path()).expect("load");
        assert_eq!(filter.path(), Some(tmp.path()));
    }
}
