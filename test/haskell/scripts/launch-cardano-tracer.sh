#!/usr/bin/env bash
set -euo pipefail

# Launch Rust cardano-tracer in a clean working directory.
#
# The Rust cardano-tracer is a full-featured trace aggregator. It listens
# on a Unix socket (or TCP), receives TraceObjects/EKG/DataPoints from
# cardano-node, and writes JSON log files.
#
# Usage:
#   ./launch-cardano-tracer.sh
#
# Environment variables:
#   PALLAS_BIN_DIR   Path to Rust binaries (default: <repo>/target/release)
#   WORKDIR          Working directory (default: /tmp/manual-cardano-tracer)
#   NETWORK_MAGIC    Protocol magic number (default: 42)
#   SOCKET_NAME      Socket filename (default: tracer.sock)
#
# Output files (once a node connects):
#   $WORKDIR/logs/tracer.sock@0/node-*.json   TraceObject log (JSON lines)
#
# Note: The Rust cardano-tracer creates log subdirectories named after the
# sanitized socket address (e.g., "tracer.sock@0" for the first connection),
# unlike trace-compat which always uses "sock@0".

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PALLAS_BIN_DIR="${PALLAS_BIN_DIR:-$REPO_ROOT/target/release}"
WORKDIR="${WORKDIR:-/tmp/manual-cardano-tracer}"
NETWORK_MAGIC="${NETWORK_MAGIC:-42}"
SOCKET_NAME="${SOCKET_NAME:-tracer.sock}"

BINARY="$PALLAS_BIN_DIR/cardano-tracer"
if [ ! -x "$BINARY" ]; then
    echo "ERROR: $BINARY not found or not executable."
    echo "Run build-rust-binaries.sh first."
    exit 1
fi

# Prepare clean working directory
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR/logs"

cat > "$WORKDIR/config.yaml" <<EOF
networkMagic: $NETWORK_MAGIC
network:
  tag: AcceptAt
  contents: "$SOCKET_NAME"
logging:
- logRoot: "logs"
  logMode: FileMode
  logFormat: ForMachine
EOF

echo "=== Rust cardano-tracer ==="
echo "  binary:  $BINARY"
echo "  workdir: $WORKDIR"
echo "  config:  $WORKDIR/config.yaml"
echo "  socket:  $WORKDIR/$SOCKET_NAME"
echo "  logs:    $WORKDIR/logs/<node-address>/"
echo ""
echo "Connect a cardano-node or test forwarder to: $WORKDIR/$SOCKET_NAME"
echo "Press Ctrl-C to stop."
echo ""

cd "$WORKDIR"
exec "$BINARY" --config config.yaml
