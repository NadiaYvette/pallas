#!/usr/bin/env bash
set -euo pipefail

# Build Rust binaries and run one or all Haskell test suites.
#
# Prerequisites:
#   Must be run from inside 'nix develop' in test/haskell/.
#
# Usage:
#   ./scripts/run-haskell-tests.sh [SUITE]
#
# Suites:
#   all      Run all Pallas-related test suites (default)
#   compat   Stress test: Haskell forwarder -> trace-compat (Rust)
#   tracer   Stress test: Haskell forwarder -> Rust cardano-tracer
#   pallas   EKG/DataPoint: Haskell forwarder -> trace-compat
#   ext      Proxy chain stress: forwarder -> trace-proxy -> Haskell cardano-tracer
#   proxy    Proxy chain EKG/DataPoint verification
#   haskell  Pure Haskell tests (no Rust binaries needed)

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"
HASKELL_DIR="$REPO_ROOT/test/haskell"
SUITE="${1:-all}"

# Build Rust binaries (unless running pure Haskell tests)
if [ "$SUITE" != "haskell" ]; then
    "$SCRIPTS_DIR/build-rust-binaries.sh"
    echo ""
fi

export PALLAS_BIN_DIR="$REPO_ROOT/target/release"
echo "PALLAS_BIN_DIR=$PALLAS_BIN_DIR"
echo ""

cd "$HASKELL_DIR"

case "$SUITE" in
    all)
        echo "Running all Pallas test suites..."
        echo ""
        cabal test cardano-tracer-test-compat
        cabal test cardano-tracer-test-tracer
        cabal test cardano-tracer-test-pallas
        cabal test cardano-tracer-test-ext
        cabal test cardano-tracer-test-proxy
        ;;
    compat)
        cabal test cardano-tracer-test-compat
        ;;
    tracer)
        cabal test cardano-tracer-test-tracer
        ;;
    pallas)
        cabal test cardano-tracer-test-pallas
        ;;
    ext)
        cabal test cardano-tracer-test-ext
        ;;
    proxy)
        cabal test cardano-tracer-test-proxy
        ;;
    haskell)
        cabal test cardano-tracer-test
        ;;
    *)
        echo "Usage: run-haskell-tests.sh [all|compat|tracer|pallas|ext|proxy|haskell]"
        echo ""
        echo "Suites:"
        echo "  all      Run all Pallas-related test suites (default)"
        echo "  compat   Stress test: Haskell forwarder -> trace-compat (Rust)"
        echo "  tracer   Stress test: Haskell forwarder -> Rust cardano-tracer"
        echo "  pallas   EKG/DataPoint: Haskell forwarder -> trace-compat"
        echo "  ext      Proxy chain stress: forwarder -> trace-proxy -> Haskell cardano-tracer"
        echo "  proxy    Proxy chain EKG/DataPoint verification"
        echo "  haskell  Pure Haskell tests (no Rust binaries needed)"
        exit 1
        ;;
esac
