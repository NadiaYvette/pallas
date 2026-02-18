#!/usr/bin/env bash
set -euo pipefail

# Launch a cardano-node connected to a Rust cardano-tracer (or trace-compat).
#
# This starts a mainnet cardano-node that forwards traces to a Rust tracer
# via --tracer-socket-path-connect. The tracer must already be listening
# (start it first with launch-cardano-tracer.sh or launch-trace-compat.sh).
#
# Prerequisites:
#   - A local cardano-node repo with a pre-built cardano-node binary.
#     To build from scratch (one-time):
#       cd /path/to/cardano-node && nix develop
#       cabal build cardano-node
#     After building, the nix develop shell is NOT required to run this script.
#   - A Rust tracer must be listening on the tracer socket (see TRACER_SOCK)
#   - jq must be available (on most systems; also provided by nix develop)
#
# Usage:
#   # Terminal 1: start the Rust tracer
#   ./launch-cardano-tracer.sh
#
#   # Terminal 2: start the node (any shell, nix develop not required)
#   ./launch-node.sh
#
# Environment variables:
#   CARDANO_NODE_DIR   Path to cardano-node repo (default: ~/src/cardano-node)
#   WORKDIR            Node working directory (default: /tmp/manual-cardano-node)
#   TRACER_SOCK        Tracer socket to connect to (default: /tmp/manual-cardano-tracer/tracer.sock)
#   NODE_PORT          Node listening port (default: 3001)
#   NODE_HOST          Node listening address (default: 0.0.0.0)

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
CARDANO_NODE_DIR="${CARDANO_NODE_DIR:-$HOME/src/cardano-node}"
WORKDIR="${WORKDIR:-/tmp/manual-cardano-node}"
TRACER_SOCK="${TRACER_SOCK:-/tmp/manual-cardano-tracer/tracer.sock}"
NODE_PORT="${NODE_PORT:-3001}"
NODE_HOST="${NODE_HOST:-0.0.0.0}"

# Configuration and genesis files: prefer the cardano-node repo's canonical
# copies in configuration/cardano/ (which are always in sync with the node
# version). The repo's mainnet-config.json already has UseTraceDispatcher,
# TraceOptionForwarder, and matching genesis hashes.
CN_CONFIG_DIR="$CARDANO_NODE_DIR/configuration/cardano"

if [ -f "$CN_CONFIG_DIR/mainnet-config.json" ]; then
    NODE_CONFIG="$CN_CONFIG_DIR/mainnet-config.json"
    GENESIS_DIR="$CN_CONFIG_DIR"
    NODE_TOPOLOGY="$CN_CONFIG_DIR/mainnet-topology.json"
else
    # Fallback: use the copies in the pallas repo (may have stale genesis hashes)
    echo "WARNING: $CN_CONFIG_DIR not found, using pallas scratch/ copies."
    echo "         Genesis hashes may not match. Set CARDANO_NODE_DIR to fix."
    echo ""
    NODE_CONFIG="$REPO_ROOT/scratch/mainnet-config-new-tracing.json"
    GENESIS_DIR="$REPO_ROOT/test_data"
    NODE_TOPOLOGY="$REPO_ROOT/scratch/mainnet-topology.json"
fi

# --- Resolve cardano-node binary ---
#
# Resolution order:
#   1. cardano-node on PATH (e.g. from nix profile install)
#   2. Search dist-newstyle directly (fast, works from any shell after a prior build)
#   3. cabal list-bin (works inside nix develop, slow outside it)

CARDANO_NODE_BIN=""

if command -v cardano-node &>/dev/null; then
    CARDANO_NODE_BIN="$(command -v cardano-node)"
elif [ -d "$CARDANO_NODE_DIR" ]; then
    # Fast path: search dist-newstyle for a previously built binary
    CARDANO_NODE_BIN="$(find "$CARDANO_NODE_DIR/dist-newstyle" -name cardano-node -type f -executable 2>/dev/null | head -1 || true)"
    # Slow path: cabal list-bin (requires nix develop shell for dependency resolution)
    if [ -z "$CARDANO_NODE_BIN" ] || [ ! -x "$CARDANO_NODE_BIN" ]; then
        CARDANO_NODE_BIN="$(cd "$CARDANO_NODE_DIR" && cabal list-bin cardano-node 2>/dev/null || true)"
    fi
    if [ -z "$CARDANO_NODE_BIN" ] || [ ! -x "$CARDANO_NODE_BIN" ]; then
        echo "ERROR: cardano-node binary not found in $CARDANO_NODE_DIR."
        echo ""
        echo "Build it first (one-time, from the cardano-node repo):"
        echo "  cd $CARDANO_NODE_DIR && nix develop"
        echo "  cabal build cardano-node"
        echo ""
        echo "After building, this script can be run from any shell."
        exit 1
    fi
else
    echo "ERROR: cardano-node not found on PATH and CARDANO_NODE_DIR does not exist: $CARDANO_NODE_DIR"
    echo ""
    echo "Set CARDANO_NODE_DIR to your cardano-node repo and build first:"
    echo "  cd /path/to/cardano-node && nix develop"
    echo "  cabal build cardano-node"
    echo ""
    echo "After building, this script can be run from any shell:"
    echo "  CARDANO_NODE_DIR=/path/to/cardano-node $0"
    exit 1
fi

echo "  cardano-node: $CARDANO_NODE_BIN"

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq not found on PATH."
    exit 1
fi

for f in "$NODE_CONFIG" "$NODE_TOPOLOGY"; do
    if [ ! -f "$f" ]; then
        echo "ERROR: Required file not found: $f"
        exit 1
    fi
done

for era in alonzo byron shelley conway; do
    if [ ! -f "$GENESIS_DIR/mainnet-${era}-genesis.json" ]; then
        echo "ERROR: Genesis file not found: $GENESIS_DIR/mainnet-${era}-genesis.json"
        exit 1
    fi
done

# --- Prepare working directory ---

mkdir -p "$WORKDIR/db"

NODE_DB_PATH="$WORKDIR/db"
NODE_SOCKET="$WORKDIR/node.socket"

# Patch the config JSON to use absolute paths for genesis and checkpoint files.
# The cardano-node config uses relative paths (resolved from its own directory),
# but we run from a different workdir so they must be made absolute.
PATCHED_CONFIG="$WORKDIR/node-config.json"
CHECKPOINTS_FILE="$GENESIS_DIR/mainnet-checkpoints.json"
jq \
    --arg alonzo  "$GENESIS_DIR/mainnet-alonzo-genesis.json" \
    --arg byron   "$GENESIS_DIR/mainnet-byron-genesis.json" \
    --arg shelley "$GENESIS_DIR/mainnet-shelley-genesis.json" \
    --arg conway  "$GENESIS_DIR/mainnet-conway-genesis.json" \
    --arg ckpt    "$CHECKPOINTS_FILE" \
    '.AlonzoGenesisFile  = $alonzo
   | .ByronGenesisFile   = $byron
   | .ShelleyGenesisFile = $shelley
   | .ConwayGenesisFile  = $conway
   | if .CheckpointsFile then .CheckpointsFile = $ckpt else . end' \
    < "$NODE_CONFIG" > "$PATCHED_CONFIG"

echo "=== cardano-node ==="
echo "  workdir:     $WORKDIR"
echo "  config:      $PATCHED_CONFIG"
echo "  topology:    $NODE_TOPOLOGY"
echo "  db:          $NODE_DB_PATH"
echo "  socket:      $NODE_SOCKET"
echo "  listen:      $NODE_HOST:$NODE_PORT"
echo "  tracer sock: $TRACER_SOCK"
echo "  genesis:     $GENESIS_DIR/"
echo ""

if [ ! -S "$TRACER_SOCK" ]; then
    echo "WARNING: Tracer socket does not exist yet: $TRACER_SOCK"
    echo "Make sure a tracer is running (launch-cardano-tracer.sh or launch-trace-compat.sh)."
    echo ""
fi

echo "Starting cardano-node..."
echo ""

exec "$CARDANO_NODE_BIN" run \
    --config "$PATCHED_CONFIG" \
    --database-path "$NODE_DB_PATH" \
    --socket-path "$NODE_SOCKET" \
    --host-addr "$NODE_HOST" \
    --port "$NODE_PORT" \
    --topology "$NODE_TOPOLOGY" \
    --tracer-socket-path-connect "$TRACER_SOCK"
