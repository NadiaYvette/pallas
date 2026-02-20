#!/usr/bin/env bash
set -euo pipefail

# Launch trace-shell paired with the Rust cardano-tracer for hands-on testing.
#
# This script:
#   1. Builds both trace-shell and cardano-tracer
#   2. Starts cardano-tracer listening on a Unix socket
#   3. Launches trace-shell connected to it
#
# Data you inject via the shell shows up in the tracer's log files.
#
# Usage:
#   ./try-it.sh                    Build and run both
#   ./try-it.sh --no-build         Skip cargo build
#   ./try-it.sh --shell-only SOCK  Just launch the shell (tracer already running)
#
# Once inside the shell, try:
#
#   metric set rts.gc.bytes_allocated counter 1024
#   metric set node.peers gauge 7
#   metric incr rts.gc.bytes_allocated 512
#   trace add "Hello from the shell" --severity Warning --ns Node.Test
#   trace add "Block forged" --ns Forge.Loop
#   datapoint nodeinfo --version 10.1.0
#   status
#   trace list
#   metric list
#
# Then check the log files:
#   cat /tmp/trace-shell-demo/logs/*/node-*.json
#
# The cardano-tracer polls every second for EKG metrics and every ~1s for
# trace objects (batch size 100). Your data appears in the logs within a
# second or two of being added.

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="/tmp/trace-shell-demo"
SOCKET_NAME="tracer.sock"
NETWORK_MAGIC=42
TRACER_PID=""

cleanup() {
    if [ -n "$TRACER_PID" ] && kill -0 "$TRACER_PID" 2>/dev/null; then
        kill "$TRACER_PID" 2>/dev/null || true
        wait "$TRACER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Parse flags
DO_BUILD=true
MODE="both"
SHELL_SOCK=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build)
            DO_BUILD=false
            shift
            ;;
        --shell-only)
            MODE="shell-only"
            SHELL_SOCK="${2:-}"
            if [ -z "$SHELL_SOCK" ]; then
                echo "Usage: $0 --shell-only <socket-path>"
                exit 1
            fi
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [--no-build] [--shell-only SOCK]"
            echo ""
            echo "  --no-build       Skip cargo build (use pre-built binaries)"
            echo "  --shell-only S   Start only the shell, connecting to socket S"
            echo ""
            echo "Default: build both, start Rust cardano-tracer in background, then shell."
            echo ""
            echo "Log output: $WORKDIR/logs/<node-address>/node-*.json"
            exit 0
            ;;
        *)
            echo "Unknown flag: $1. Use --help for usage."
            exit 1
            ;;
    esac
done

# --- shell-only mode ---
if [ "$MODE" = "shell-only" ]; then
    TRACE_SHELL="$REPO_ROOT/target/debug/trace-shell"
    if [ ! -x "$TRACE_SHELL" ]; then
        echo "ERROR: $TRACE_SHELL not found. Build first: cargo build -p trace-shell"
        exit 1
    fi
    echo "Starting trace-shell connected to $SHELL_SOCK ..."
    echo ""
    exec "$TRACE_SHELL" --socket "$SHELL_SOCK" --magic "$NETWORK_MAGIC"
fi

# --- Build ---
if $DO_BUILD; then
    echo "Building trace-shell and cardano-tracer ..."
    (cd "$REPO_ROOT" && cargo build -p trace-shell -p cardano-tracer 2>&1)
    echo ""
fi

TRACE_SHELL="$REPO_ROOT/target/debug/trace-shell"
CARDANO_TRACER="$REPO_ROOT/target/debug/cardano-tracer"

for bin in "$TRACE_SHELL" "$CARDANO_TRACER"; do
    if [ ! -x "$bin" ]; then
        echo "ERROR: $bin not found. Run without --no-build first."
        exit 1
    fi
done

# --- Prepare working directory ---
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
ekgRequestFreq: 1.0
loRequestNum: 100
EOF

# --- Start cardano-tracer ---
echo "=== Starting Rust cardano-tracer ==="
echo "  workdir: $WORKDIR"
echo "  config:  $WORKDIR/config.yaml"
echo "  socket:  $WORKDIR/$SOCKET_NAME"
echo "  logs:    $WORKDIR/logs/<node-address>/node-*.json"
echo ""

(cd "$WORKDIR" && RUST_LOG=info "$CARDANO_TRACER" --config config.yaml) \
    > "$WORKDIR/tracer.log" 2>&1 &
TRACER_PID=$!

# Wait for the socket to appear
for i in $(seq 1 50); do
    if [ -S "$WORKDIR/$SOCKET_NAME" ]; then
        break
    fi
    sleep 0.1
done

if [ ! -S "$WORKDIR/$SOCKET_NAME" ]; then
    echo "ERROR: Socket $WORKDIR/$SOCKET_NAME did not appear after 5 seconds."
    echo "Check if cardano-tracer started correctly."
    exit 1
fi

echo "=== cardano-tracer is listening ==="
echo "  tracer log: $WORKDIR/tracer.log"
echo ""
echo "=== Launching trace-shell ==="
echo ""
echo "  Try these commands inside the shell:"
echo ""
echo "    trace add \"Hello from the shell\" --severity Warning"
echo "    metric set node.peers gauge 7"
echo "    datapoint nodeinfo --version 10.1.0"
echo "    status"
echo ""
echo "  Log files appear after the first 'trace add'."
echo "  Use 'status' to see the exact log file path, then in another terminal:"
echo ""
echo "    tail -f $WORKDIR/logs/*/node-*.json"
echo ""

"$TRACE_SHELL" --socket "$WORKDIR/$SOCKET_NAME" --magic "$NETWORK_MAGIC" --logdir "$WORKDIR/logs"

echo ""
echo "=== Session ended ==="

# Show what was logged
echo ""
echo "Log files created:"
find "$WORKDIR/logs" -type f -name '*.json' 2>/dev/null | while read -r f; do
    lines=$(wc -l < "$f")
    echo "  $f ($lines lines)"
done

echo ""
echo "To view trace logs:  cat $WORKDIR/logs/*/node-*.json | head -20"
