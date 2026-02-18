#!/usr/bin/env bash
set -euo pipefail

# Launch the trace-proxy chain: Haskell cardano-tracer + Rust trace-proxy.
#
# Architecture:
#   Node/Forwarder --> tracer.sock --> trace-proxy --> tracer-real.sock --> Haskell cardano-tracer --> logs/
#
# The proxy sits between the node and the real tracer, decoding and
# re-encoding TraceObjects to verify CBOR round-trip idempotency.
#
# Prerequisites:
#   - Must be run from inside 'nix develop' (provides Haskell cardano-tracer)
#   - Rust binaries must be built (run build-rust-binaries.sh first)
#
# Usage:
#   cd test/haskell && nix develop
#   ./scripts/launch-proxy-chain.sh
#
# Environment variables:
#   PALLAS_BIN_DIR   Path to Rust binaries (default: <repo>/target/release)
#   WORKDIR          Working directory (default: /tmp/manual-proxy-chain)
#   NETWORK_MAGIC    Protocol magic number (default: 42)
#   NODE_SOCKET      Socket where proxy listens for node (default: tracer.sock)
#   TRACER_SOCKET    Socket where Haskell cardano-tracer listens (default: tracer-real.sock)

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PALLAS_BIN_DIR="${PALLAS_BIN_DIR:-$REPO_ROOT/target/release}"
WORKDIR="${WORKDIR:-/tmp/manual-proxy-chain}"
NETWORK_MAGIC="${NETWORK_MAGIC:-42}"
NODE_SOCKET="${NODE_SOCKET:-tracer.sock}"
TRACER_SOCKET="${TRACER_SOCKET:-tracer-real.sock}"

PROXY_BINARY="$PALLAS_BIN_DIR/trace-proxy"
if [ ! -x "$PROXY_BINARY" ]; then
    echo "ERROR: $PROXY_BINARY not found or not executable."
    echo "Run build-rust-binaries.sh first."
    exit 1
fi

if ! command -v cardano-tracer &>/dev/null; then
    echo "ERROR: cardano-tracer (Haskell) not found on PATH."
    echo "Run this script from inside 'nix develop' in test/haskell/."
    exit 1
fi

# Prepare clean working directory
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR/logs" "$WORKDIR/tracer-statedir"

# Write config for Haskell cardano-tracer (uses absolute paths)
cat > "$WORKDIR/config.yaml" <<EOF
networkMagic: $NETWORK_MAGIC
network:
  tag: AcceptAt
  contents: "$WORKDIR/$TRACER_SOCKET"
logging:
- logRoot: "$WORKDIR/logs"
  logMode: FileMode
  logFormat: ForMachine
EOF

TRACER_PID=""
PROXY_PID=""

cleanup() {
    echo ""
    echo "Shutting down..."
    [ -n "$PROXY_PID" ]  && kill "$PROXY_PID" 2>/dev/null || true
    [ -n "$TRACER_PID" ] && kill "$TRACER_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    echo "Done."
}
trap cleanup EXIT INT TERM

echo "=== Proxy Chain ==="
echo "  workdir:        $WORKDIR"
echo "  node socket:    $WORKDIR/$NODE_SOCKET    (proxy listens here)"
echo "  tracer socket:  $WORKDIR/$TRACER_SOCKET  (Haskell cardano-tracer listens here)"
echo "  logs:           $WORKDIR/logs/"
echo ""

# 1. Start Haskell cardano-tracer
echo "Starting Haskell cardano-tracer..."
cd "$WORKDIR"
cardano-tracer \
    --config "$WORKDIR/config.yaml" \
    --state-dir "$WORKDIR/tracer-statedir" &
TRACER_PID=$!
echo "  PID: $TRACER_PID"
echo "  Waiting 10 seconds for startup..."
sleep 10

if ! kill -0 "$TRACER_PID" 2>/dev/null; then
    echo "ERROR: Haskell cardano-tracer exited prematurely."
    exit 1
fi
echo "  OK: cardano-tracer is running."
echo ""

# 2. Start Rust trace-proxy
echo "Starting trace-proxy..."
"$PROXY_BINARY" \
    --node-socket "$NODE_SOCKET" \
    --tracer-socket "$WORKDIR/$TRACER_SOCKET" \
    --magic "$NETWORK_MAGIC" &
PROXY_PID=$!
echo "  PID: $PROXY_PID"
echo "  Waiting 2 seconds for startup..."
sleep 2

if ! kill -0 "$PROXY_PID" 2>/dev/null; then
    echo "ERROR: trace-proxy exited prematurely."
    exit 1
fi
echo "  OK: trace-proxy is running."
echo ""

echo "=== Proxy chain is ready ==="
echo "Connect a node or test forwarder to: $WORKDIR/$NODE_SOCKET"
echo "Press Ctrl-C to stop both processes."
echo ""

wait
