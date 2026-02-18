#!/usr/bin/env bash
set -euo pipefail

# Build all three Pallas Rust trace-forwarding binaries in release mode.
#
# Usage:
#   ./build-rust-binaries.sh
#
# This is a prerequisite for the launch-* and run-haskell-tests.sh scripts.

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BIN_DIR="$REPO_ROOT/target/release"

echo "Building Rust trace-forwarding binaries..."
echo "  repo root: $REPO_ROOT"
echo ""

cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" \
    -p trace-compat -p trace-proxy -p cardano-tracer

echo ""
echo "Checking binaries:"
FAIL=0
for bin in trace-compat trace-proxy cardano-tracer; do
    if [ -x "$BIN_DIR/$bin" ]; then
        echo "  OK: $BIN_DIR/$bin"
    else
        echo "  MISSING: $BIN_DIR/$bin"
        FAIL=1
    fi
done

if [ "$FAIL" -ne 0 ]; then
    echo ""
    echo "ERROR: Some binaries are missing."
    exit 1
fi

echo ""
echo "All binaries built successfully."
