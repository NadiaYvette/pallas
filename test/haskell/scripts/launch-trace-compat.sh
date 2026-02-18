#!/usr/bin/env bash
set -euo pipefail

# Launch trace-compat (Rust) in a clean working directory.
#
# trace-compat acts as a drop-in replacement for Haskell cardano-tracer.
# It listens on a Unix socket, receives TraceObjects/EKG/DataPoints from
# a Haskell node (or test forwarder), and writes JSON log files.
#
# Usage:
#   ./launch-trace-compat.sh
#
# Environment variables:
#   PALLAS_BIN_DIR   Path to Rust binaries (default: <repo>/target/release)
#   WORKDIR          Working directory (default: /tmp/manual-trace-compat)
#   NETWORK_MAGIC    Protocol magic number (default: 42)
#   SOCKET_NAME      Socket filename (default: tracer.sock)
#
# Output files (once a forwarder connects):
#   $WORKDIR/logs/sock@0/node-1.json       TraceObject log (JSON lines)
#   $WORKDIR/logs/sock@0/ekg.json          EKG metrics (JSON lines)
#   $WORKDIR/logs/sock@0/datapoints.json   DataPoints (JSON lines)

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PALLAS_BIN_DIR="${PALLAS_BIN_DIR:-$REPO_ROOT/target/release}"
WORKDIR="${WORKDIR:-/tmp/manual-trace-compat}"
NETWORK_MAGIC="${NETWORK_MAGIC:-42}"
SOCKET_NAME="${SOCKET_NAME:-tracer.sock}"

BINARY="$PALLAS_BIN_DIR/trace-compat"
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

echo "=== trace-compat ==="
echo "  binary:  $BINARY"
echo "  workdir: $WORKDIR"
echo "  config:  $WORKDIR/config.yaml"
echo "  socket:  $WORKDIR/$SOCKET_NAME"
echo "  logs:    $WORKDIR/logs/sock@0/"
echo ""
echo "Connect a Haskell node or test forwarder to: $WORKDIR/$SOCKET_NAME"
echo "Press Ctrl-C to stop."
echo ""

cd "$WORKDIR"
exec "$BINARY" --config config.yaml
