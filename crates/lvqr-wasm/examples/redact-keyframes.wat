;; redact-keyframes.wat
;;
;; Drop-every-fragment LVQR filter. Returns -1 (any negative
;; value) from `on_fragment` so the host treats every inbound
;; fragment as dropped. Paired with the hot-reload test in
;; `crates/lvqr-cli/tests/wasm_hot_reload.rs`: the test starts a
;; server with `frame-counter.wasm` (which keeps everything),
;; copies this module over that path, waits for the reloader to
;; notice, and asserts the subsequent fragments increment the
;; `fragments_dropped` counter on the tap handle.
;;
;; The "redact" naming refers to the eventual use case (scrub
;; keyframes out of a live stream before they reach downstream
;; viewers). The session-C tap is read-only, so in practice
;; "redact" means "the counter says dropped"; full stream-
;; modifying filters are deferred to v1.1 per the note at the
;; top of `crates/lvqr-wasm/src/observer.rs`.
;;
;; Build (requires the `wat` CLI or `wasm-tools`):
;;   wat2wasm redact-keyframes.wat -o redact-keyframes.wasm
;; or via the `wat` Rust crate:
;;   cargo run -p lvqr-wasm --example build_fixtures
;;
;; The repo ships `redact-keyframes.wasm` alongside this file so
;; the test binary can `fs::copy` it over the configured
;; `--wasm-filter` path without a toolchain install.

(module
  (memory (export "memory") 1)
  (func (export "on_fragment") (param $ptr i32) (param $len i32) (result i32)
    i32.const -1))
