//! Assemble the committed `.wasm` fixtures from their sibling
//! `.wat` sources via the `wat` crate. The fixtures are small
//! (~80 bytes each) and the assembly is deterministic, so the
//! repo ships the `.wasm` alongside the `.wat` to keep
//! contributors from needing `wat2wasm` or `wasm-tools` on
//! their PATH. Re-run this helper whenever a `.wat` changes:
//!
//!   cargo run -p lvqr-wasm --example build_fixtures

use std::path::{Path, PathBuf};

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut compiled = 0usize;
    for entry in std::fs::read_dir(&dir).expect("read examples dir") {
        let entry = entry.expect("read entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wat") {
            continue;
        }
        compile_one(&path);
        compiled += 1;
    }
    if compiled == 0 {
        panic!("no .wat files found in {}", dir.display());
    }
}

fn compile_one(wat_path: &Path) {
    let src = std::fs::read_to_string(wat_path).unwrap_or_else(|e| panic!("reading {}: {e}", wat_path.display()));
    let bytes = wat::parse_str(&src).unwrap_or_else(|e| panic!("parsing {}: {e}", wat_path.display()));
    let wasm_path = wat_path.with_extension("wasm");
    std::fs::write(&wasm_path, &bytes).unwrap_or_else(|e| panic!("writing {}: {e}", wasm_path.display()));
    println!(
        "{} -> {} ({} bytes)",
        wat_path.display(),
        wasm_path.display(),
        bytes.len()
    );
}
