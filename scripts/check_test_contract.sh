#!/usr/bin/env bash
#
# 5-artifact test contract enforcement.
#
# Every crate in the in-scope list below must ship all five artifacts
# (proptest, fuzz, integration, E2E, conformance) per tests/CONTRACT.md.
# This script walks the in-scope crates and reports any missing slot.
#
# Tier 1 (now): educational. Missing artifacts are reported as warnings
# and the script exits 0 so CI does not block PRs. Existing crates catch
# up incrementally via the Tier 1 test-infra backlog.
#
# Tier 2 (soon): set LVQR_CONTRACT_STRICT=1 to flip to hard-fail mode.
# The CI workflow in .github/workflows/contract.yml will be updated to
# set this variable once the backlog is clean.
#
# Usage:
#   bash scripts/check_test_contract.sh
#   LVQR_CONTRACT_STRICT=1 bash scripts/check_test_contract.sh
#
# The script is intentionally dependency-free: plain POSIX shell plus
# standard GNU coreutils. It is meant to be runnable offline on any
# contributor laptop exactly the way CI runs it.

set -u

# Crates in scope per tests/CONTRACT.md. Edited in lockstep with the
# crate inventory when new protocol crates land (Tier 2).
IN_SCOPE=(
    lvqr-ingest
    lvqr-record
    lvqr-moq
    lvqr-fragment
    lvqr-codec
    lvqr-cmaf
    # Tier 2 crates below will be enabled as they land:
    # lvqr-whip
    # lvqr-whep
    # lvqr-hls
    # lvqr-dash
    # lvqr-srt
    # lvqr-rtsp
    # lvqr-archive
)

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
STRICT=${LVQR_CONTRACT_STRICT:-0}

missing_total=0
checked_total=0

# Color helpers. Skip if stdout is not a TTY and GitHub Actions is not
# running, so log files stay readable.
if [ -t 1 ] || [ "${GITHUB_ACTIONS:-}" = "true" ]; then
    C_RED=$'\033[31m'
    C_YEL=$'\033[33m'
    C_GRN=$'\033[32m'
    C_DIM=$'\033[2m'
    C_RST=$'\033[0m'
else
    C_RED=""
    C_YEL=""
    C_GRN=""
    C_DIM=""
    C_RST=""
fi

gh_warn() {
    # GitHub Actions annotation on the first matching file, regular log
    # line everywhere else.
    local file=$1 msg=$2
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
        printf '::warning file=%s::%s\n' "$file" "$msg"
    fi
    printf '%swarn%s: %s: %s\n' "$C_YEL" "$C_RST" "$file" "$msg"
}

gh_err() {
    local file=$1 msg=$2
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
        printf '::error file=%s::%s\n' "$file" "$msg"
    fi
    printf '%serror%s: %s: %s\n' "$C_RED" "$C_RST" "$file" "$msg"
}

has_file_matching() {
    # Usage: has_file_matching <glob...>. Returns 0 if at least one real
    # file matches any supplied glob, 1 otherwise. Uses `find` with
    # `-name` so we do not depend on bash 4+ globstar, which macOS bash
    # 3.2 does not ship. For recursive matches the caller passes a
    # `<dir>|<filename-glob>` pair separated by a pipe.
    local pattern
    for pattern in "$@"; do
        if [[ "$pattern" == *"|"* ]]; then
            local dir=${pattern%%|*}
            local glob=${pattern##*|}
            [ -d "$dir" ] || continue
            if find "$dir" -type f -name "$glob" -print -quit 2>/dev/null | grep -q .; then
                return 0
            fi
        else
            # Plain glob: expand via compgen so unmatched globs produce
            # no output instead of the literal pattern.
            local match
            match=$(compgen -G "$pattern" 2>/dev/null | head -n 1)
            [ -n "$match" ] && return 0
        fi
    done
    return 1
}

check_crate() {
    local crate=$1
    local crate_dir="$REPO_ROOT/crates/$crate"

    if [ ! -d "$crate_dir" ]; then
        printf '%sskip%s: %s (crate does not exist yet)\n' "$C_DIM" "$C_RST" "$crate"
        return
    fi

    checked_total=$((checked_total + 1))
    printf '\n%s== %s ==%s\n' "$C_DIM" "$crate" "$C_RST"

    local slot_file="$crate_dir/Cargo.toml"
    local local_missing=0

    # 1. proptest harness
    if has_file_matching \
        "$crate_dir/tests/proptest_*.rs" \
        "$crate_dir/tests/*_proptest.rs"; then
        printf '  %sok%s   proptest\n' "$C_GRN" "$C_RST"
    else
        gh_warn "$slot_file" "missing proptest harness (tests/proptest_*.rs)"
        local_missing=$((local_missing + 1))
    fi

    # 2. cargo-fuzz target
    if has_file_matching \
        "$crate_dir/fuzz/fuzz_targets/*.rs"; then
        printf '  %sok%s   fuzz\n' "$C_GRN" "$C_RST"
    else
        gh_warn "$slot_file" "missing cargo-fuzz target (fuzz/fuzz_targets/*.rs)"
        local_missing=$((local_missing + 1))
    fi

    # 3. integration test (real network, not a unit test)
    if has_file_matching \
        "$crate_dir/tests/*integration*.rs" \
        "$crate_dir/tests/*_bridge*.rs"; then
        printf '  %sok%s   integration\n' "$C_GRN" "$C_RST"
    else
        gh_warn "$slot_file" "missing integration test (tests/*integration*.rs)"
        local_missing=$((local_missing + 1))
    fi

    # 4. E2E test (crosses subsystem boundaries, real I/O).
    # An E2E may live in the crate's own tests/, or in the workspace-
    # level tests/e2e/ (playwright). Cross-crate E2Es that only exist
    # in lvqr-cli/tests/ (e.g. rtmp_ws_e2e.rs covers lvqr-ingest's data
    # path) are accepted case-by-case in tests/CONTRACT.md and flagged
    # here so the dependency is visible; set CONTRACT_E2E_EXEMPT_<crate>=1
    # in the environment when running in strict mode to silence the
    # warning for a specific crate.
    local exempt_var="CONTRACT_E2E_EXEMPT_${crate//-/_}"
    if has_file_matching \
        "$crate_dir/tests/*_e2e.rs" \
        "$REPO_ROOT/tests/e2e|*.spec.ts" \
        "$REPO_ROOT/tests/e2e|*.spec.js"; then
        printf '  %sok%s   e2e\n' "$C_GRN" "$C_RST"
    elif [ "${!exempt_var:-}" = "1" ]; then
        printf '  %sok%s   e2e (exempt: %s)\n' "$C_GRN" "$C_RST" "$exempt_var"
    else
        gh_warn "$slot_file" "missing E2E test (tests/*_e2e.rs or tests/e2e/**)"
        local_missing=$((local_missing + 1))
    fi

    # 5. Conformance check (external validator or golden file).
    if has_file_matching \
        "$crate_dir/tests/*conformance*.rs" \
        "$crate_dir/tests/golden_*.rs" \
        "$crate_dir/tests/*_golden.rs"; then
        printf '  %sok%s   conformance\n' "$C_GRN" "$C_RST"
    else
        gh_warn "$slot_file" "missing conformance check (tests/*conformance*.rs or tests/golden_*.rs)"
        local_missing=$((local_missing + 1))
    fi

    missing_total=$((missing_total + local_missing))
}

for crate in "${IN_SCOPE[@]}"; do
    check_crate "$crate"
done

printf '\n'
printf '5-artifact test contract: %d crate(s) checked, %d missing slot(s)\n' \
    "$checked_total" "$missing_total"

if [ "$missing_total" -eq 0 ]; then
    printf '%sall in-scope crates satisfy the contract%s\n' "$C_GRN" "$C_RST"
    exit 0
fi

if [ "$STRICT" = "1" ]; then
    gh_err "tests/CONTRACT.md" "5-artifact contract violations in strict mode; see warnings above"
    exit 1
fi

printf '%scontract enforcement is in Tier 1 educational mode (soft-fail); set LVQR_CONTRACT_STRICT=1 to hard-fail%s\n' \
    "$C_DIM" "$C_RST"
exit 0
