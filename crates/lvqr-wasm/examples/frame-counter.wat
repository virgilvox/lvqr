;; frame-counter.wat
;;
;; Minimal LVQR fragment filter. Keeps every fragment unchanged
;; (returns the input length as the output length). The host
;; counts invocations via the WasmFilterBridgeHandle, so the
;; filter itself does not need to maintain state or write to
;; WASI stderr. This is the smallest possible filter that
;; exercises the full host-to-guest ABI at test time.
;;
;; Build (requires the `wat` CLI or `wasm-tools`):
;;   wat2wasm frame-counter.wat -o frame-counter.wasm
;; or programmatically via the `wat` Rust crate:
;;   wat::parse_file("frame-counter.wat")
;;
;; The repo ships `frame-counter.wasm` alongside this file so
;; users can point `--wasm-filter` at it without a toolchain
;; install.

(module
  (memory (export "memory") 1)
  (func (export "on_fragment") (param $ptr i32) (param $len i32) (result i32)
    local.get $len))
