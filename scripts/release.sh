#!/bin/bash
set -euo pipefail

# LVQR Release Script
# Publishes all crates to crates.io in dependency order.
#
# Usage:
#   ./scripts/release.sh              # Publish all crates
#   ./scripts/release.sh --dry-run    # Verify without publishing

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
    echo "=== DRY RUN MODE ==="
fi

# Verify clean git state
if [[ -n "$(git status --porcelain)" ]]; then
    echo "ERROR: Working directory is not clean. Commit or stash changes first."
    exit 1
fi

echo "=== Checking code quality ==="
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

echo "=== Running tests ==="
cargo test -p lvqr-core --lib
cargo test -p lvqr-relay --lib
cargo test -p lvqr-admin --lib
cargo test -p lvqr-ingest --lib
cargo test -p lvqr-mesh --lib
cargo test -p lvqr-signal --lib
cargo test -p lvqr-relay --test relay_integration

echo "=== All checks passed ==="

# Publishing order (strict dependency tiers)
TIER0="lvqr-core"
TIER1="lvqr-signal"
TIER2="lvqr-relay lvqr-ingest lvqr-mesh"
TIER3="lvqr-admin"
TIER4="lvqr-cli"

publish_crate() {
    local crate=$1
    echo "--- Publishing $crate ---"
    if $DRY_RUN; then
        cargo publish -p "$crate" --dry-run
    else
        cargo publish -p "$crate"
        echo "    Waiting 30s for crates.io index..."
        sleep 30
    fi
}

publish_tier() {
    local tier_name=$1
    shift
    echo ""
    echo "=== $tier_name ==="
    for crate in "$@"; do
        publish_crate "$crate"
    done
}

publish_tier "Tier 0" $TIER0
publish_tier "Tier 1" $TIER1
publish_tier "Tier 2" $TIER2
publish_tier "Tier 3" $TIER3
publish_tier "Tier 4" $TIER4

echo ""
echo "=== Release complete ==="
echo "Published crates:"
echo "  - lvqr-core"
echo "  - lvqr-signal"
echo "  - lvqr-relay"
echo "  - lvqr-ingest"
echo "  - lvqr-mesh"
echo "  - lvqr-admin"
echo "  - lvqr-cli"
