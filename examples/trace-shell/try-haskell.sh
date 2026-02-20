#!/usr/bin/env bash
set -euo pipefail

# Launch trace-shell paired with the Haskell cardano-tracer.
#
# This script:
#   1. Verifies that the Haskell cardano-tracer is available (requires nix develop)
#   2. Optionally builds trace-shell
#   3. Starts the Haskell cardano-tracer listening on a Unix socket
#   4. Launches trace-shell connected to it
#
# Prerequisites:
#   - Must be run from inside 'nix develop' in test/haskell/
#     (provides the Haskell cardano-tracer binary on PATH)
#   - trace-shell must be built (or use the default --build behavior)
#
# Usage:
#   ./try-haskell.sh                    Build trace-shell and run both
#   ./try-haskell.sh --no-build         Skip cargo build
#   ./try-haskell.sh --shell-only SOCK  Just launch the shell (tracer already running)
#
# Environment variables:
#   WORKDIR          Working directory (default: /tmp/trace-shell-haskell)
#   NETWORK_MAGIC    Protocol magic number (default: 42)
#   SOCKET_NAME      Socket filename inside WORKDIR (default: tracer.sock)
#
# Once inside the shell, try:
#
#   trace add "Hello from trace-shell" --severity Info --ns Node.Test
#   metric set node.peers gauge 7
#   datapoint nodeinfo --version 10.1.0
#   status
#
# Then check the log files:
#   cat /tmp/trace-shell-haskell/logs/*/node*.json

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="${WORKDIR:-/tmp/trace-shell-haskell}"
NETWORK_MAGIC="${NETWORK_MAGIC:-42}"
SOCKET_NAME="${SOCKET_NAME:-tracer.sock}"
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
            echo "  --no-build       Skip cargo build (use pre-built trace-shell)"
            echo "  --shell-only S   Start only the shell, connecting to socket S"
            echo ""
            echo "Default: build trace-shell, start Haskell cardano-tracer in background, then shell."
            echo ""
            echo "Prerequisites: run from inside 'nix develop' in test/haskell/ to get"
            echo "the Haskell cardano-tracer on PATH."
            echo ""
            echo "Environment variables:"
            echo "  WORKDIR=$WORKDIR"
            echo "  NETWORK_MAGIC=$NETWORK_MAGIC"
            echo "  SOCKET_NAME=$SOCKET_NAME"
            echo ""
            echo "Log output: \$WORKDIR/logs/<node-address>/node-*.json"
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

# --- Check for Haskell cardano-tracer ---
if ! command -v cardano-tracer &>/dev/null; then
    echo "ERROR: cardano-tracer (Haskell) not found on PATH."
    echo ""
    echo "This script requires the Haskell cardano-tracer, which is provided"
    echo "by the nix development shell. To set it up:"
    echo ""
    echo "  cd $REPO_ROOT/test/haskell"
    echo "  nix develop"
    echo "  ../../examples/trace-shell/try-haskell.sh"
    echo ""
    echo "If you want to test against the Rust cardano-tracer instead,"
    echo "use try-it.sh (no Haskell toolchain required)."
    exit 1
fi

# --- Build ---
if $DO_BUILD; then
    echo "Building trace-shell ..."
    (cd "$REPO_ROOT" && cargo build -p trace-shell 2>&1)
    echo ""
fi

TRACE_SHELL="$REPO_ROOT/target/debug/trace-shell"
if [ ! -x "$TRACE_SHELL" ]; then
    echo "ERROR: $TRACE_SHELL not found. Run without --no-build first."
    exit 1
fi

# --- Prepare working directory ---
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR/logs" "$WORKDIR/tracer-statedir"

cat > "$WORKDIR/config.yaml" <<EOF
networkMagic: $NETWORK_MAGIC
network:
  tag: AcceptAt
  contents: "$WORKDIR/$SOCKET_NAME"
logging:
- logRoot: "$WORKDIR/logs"
  logMode: FileMode
  logFormat: ForMachine
EOF

# --- Start Haskell cardano-tracer ---
echo "=== Starting Haskell cardano-tracer ==="
echo "  binary:  $(command -v cardano-tracer)"
echo "  workdir: $WORKDIR"
echo "  config:  $WORKDIR/config.yaml"
echo "  socket:  $WORKDIR/$SOCKET_NAME"
echo "  logs:    $WORKDIR/logs/<node-address>/node-*.json"
echo ""

# The Haskell cardano-tracer takes longer to start than the Rust one.
# Use absolute paths in config so we don't depend on the CWD.
cardano-tracer \
    --config "$WORKDIR/config.yaml" \
    --state-dir "$WORKDIR/tracer-statedir" \
    > "$WORKDIR/tracer.log" 2>&1 &
TRACER_PID=$!

# The Haskell tracer typically needs several seconds to start.
echo "  Waiting for Haskell cardano-tracer to start (up to 15s)..."
for i in $(seq 1 150); do
    if [ -S "$WORKDIR/$SOCKET_NAME" ]; then
        break
    fi
    # Check the tracer hasn't exited
    if ! kill -0 "$TRACER_PID" 2>/dev/null; then
        echo "ERROR: Haskell cardano-tracer exited prematurely."
        echo "Check $WORKDIR/tracer.log for details."
        exit 1
    fi
    sleep 0.1
done

if [ ! -S "$WORKDIR/$SOCKET_NAME" ]; then
    echo "ERROR: Socket $WORKDIR/$SOCKET_NAME did not appear after 15 seconds."
    echo "Check if the Haskell cardano-tracer started correctly:"
    echo "  cat $WORKDIR/tracer.log"
    exit 1
fi

echo "=== Haskell cardano-tracer is listening ==="
echo "  tracer PID: $TRACER_PID"
echo "  tracer log: $WORKDIR/tracer.log"
echo ""
echo "=== Launching trace-shell ==="
echo ""
echo "  Try these commands inside the shell:"
echo ""
echo "    trace add \"Hello from trace-shell\" --severity Info"
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
echo "Tracer log:          cat $WORKDIR/tracer.log"
