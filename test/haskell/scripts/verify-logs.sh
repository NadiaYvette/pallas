#!/usr/bin/env bash
set -euo pipefail

# Verify output log files produced by trace-compat, cardano-tracer, or the
# proxy chain. Checks JSON validity, EKG metric structure, and datapoint
# structure.
#
# Usage:
#   ./verify-logs.sh <logs-directory>
#
# Examples:
#   ./verify-logs.sh /tmp/manual-trace-compat/logs
#   ./verify-logs.sh /tmp/manual-cardano-tracer/logs
#   ./verify-logs.sh /tmp/manual-proxy-chain/logs
#
# Requires: jq

if [ "${1:-}" = "" ]; then
    echo "Usage: verify-logs.sh <logs-directory>"
    echo ""
    echo "Examples:"
    echo "  verify-logs.sh /tmp/manual-trace-compat/logs"
    echo "  verify-logs.sh /tmp/manual-cardano-tracer/logs"
    exit 1
fi

LOGS_DIR="$1"

if ! command -v jq &>/dev/null; then
    echo "ERROR: jq is required but not found on PATH."
    echo "Install it with: nix-shell -p jq  (or your package manager)"
    exit 1
fi

if [ ! -d "$LOGS_DIR" ]; then
    echo "FAIL: Logs directory does not exist: $LOGS_DIR"
    exit 1
fi

# Find subdirectories (e.g., sock@0, tracer.sock@0, or Haskell cardano-tracer node names)
SUBDIRS=()
for d in "$LOGS_DIR"/*/; do
    [ -d "$d" ] && SUBDIRS+=("$d")
done

if [ ${#SUBDIRS[@]} -eq 0 ]; then
    echo "FAIL: No subdirectories found in $LOGS_DIR"
    echo "Expected a subdirectory like 'sock@0' or 'tracer.sock@0'."
    echo "Has a node/forwarder connected and sent data?"
    exit 1
fi

echo "Found ${#SUBDIRS[@]} node log directory(ies):"
for d in "${SUBDIRS[@]}"; do echo "  $d"; done
echo ""

PASS=0
FAIL=0

pass() { echo "    PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "    FAIL: $1"; FAIL=$((FAIL + 1)); }
info() { echo "    INFO: $1"; }

for SUBDIR in "${SUBDIRS[@]}"; do
    echo "=== $(basename "$SUBDIR") ==="

    # --- TraceObject logs (node-*.json) ---
    NODE_LOGS=()
    for f in "$SUBDIR"/node-*.json; do
        [ -f "$f" ] && NODE_LOGS+=("$f")
    done

    if [ ${#NODE_LOGS[@]} -eq 0 ]; then
        info "No node-*.json files found"
    else
        for f in "${NODE_LOGS[@]}"; do
            TOTAL_LINES=$(wc -l < "$f")
            NONEMPTY=$(grep -c '.' "$f" || true)
            echo "  $(basename "$f"): $TOTAL_LINES lines ($NONEMPTY non-empty)"

            # Check each non-empty line is valid JSON
            BAD=0
            while IFS= read -r line; do
                [ -z "$line" ] && continue
                if ! echo "$line" | jq empty 2>/dev/null; then
                    BAD=$((BAD + 1))
                    if [ "$BAD" -le 3 ]; then
                        echo "      bad line: ${line:0:80}..."
                    fi
                fi
            done < "$f"

            if [ "$NONEMPTY" -eq 0 ]; then
                info "File is empty (no forwarder data received yet?)"
            elif [ "$BAD" -eq 0 ]; then
                pass "All $NONEMPTY non-empty lines are valid JSON"
            else
                fail "$BAD of $NONEMPTY lines are not valid JSON"
            fi
        done
    fi

    # --- EKG metrics (ekg.json) ---
    EKG_FILE="$SUBDIR/ekg.json"
    if [ -f "$EKG_FILE" ]; then
        EKG_LINES=$(wc -l < "$EKG_FILE")
        EKG_NONEMPTY=$(grep -c '.' "$EKG_FILE" || true)
        echo "  ekg.json: $EKG_LINES lines ($EKG_NONEMPTY non-empty)"

        if [ "$EKG_NONEMPTY" -eq 0 ]; then
            info "ekg.json is empty (no EKG data received yet?)"
        else
            # Check JSON validity
            BAD=0
            TYPED=0
            while IFS= read -r line; do
                [ -z "$line" ] && continue
                if ! echo "$line" | jq empty 2>/dev/null; then
                    BAD=$((BAD + 1))
                    continue
                fi
                # Check for expected structure: {"name":"...","value":{"type":"...","val":...}}
                TYPE=$(echo "$line" | jq -r '.value.type // empty' 2>/dev/null)
                if [ "$TYPE" = "Gauge" ] || [ "$TYPE" = "Counter" ] || [ "$TYPE" = "Label" ]; then
                    TYPED=$((TYPED + 1))
                fi
            done < "$EKG_FILE"

            if [ "$BAD" -eq 0 ]; then
                pass "All $EKG_NONEMPTY EKG lines are valid JSON"
            else
                fail "$BAD of $EKG_NONEMPTY EKG lines are not valid JSON"
            fi

            if [ "$TYPED" -gt 0 ]; then
                pass "Found $TYPED EKG metrics with type (Gauge/Counter/Label)"
            else
                info "No EKG metrics with recognized type found"
            fi

            # Show sample metrics
            echo "    Sample metrics:"
            head -3 "$EKG_FILE" | while IFS= read -r line; do
                [ -z "$line" ] && continue
                NAME=$(echo "$line" | jq -r '.name // "?"' 2>/dev/null)
                TYPE=$(echo "$line" | jq -r '.value.type // "?"' 2>/dev/null)
                VAL=$(echo "$line" | jq -r '.value.val // .value // "?"' 2>/dev/null)
                echo "      $NAME ($TYPE) = $VAL"
            done
        fi
    else
        info "No ekg.json (only produced by trace-compat)"
    fi

    # --- DataPoints (datapoints.json) ---
    DP_FILE="$SUBDIR/datapoints.json"
    if [ -f "$DP_FILE" ]; then
        DP_LINES=$(wc -l < "$DP_FILE")
        DP_NONEMPTY=$(grep -c '.' "$DP_FILE" || true)
        echo "  datapoints.json: $DP_LINES lines ($DP_NONEMPTY non-empty)"

        if [ "$DP_NONEMPTY" -eq 0 ]; then
            info "datapoints.json is empty (no datapoints received yet?)"
        else
            BAD=0
            NAMED=0
            while IFS= read -r line; do
                [ -z "$line" ] && continue
                if ! echo "$line" | jq empty 2>/dev/null; then
                    BAD=$((BAD + 1))
                    continue
                fi
                NAME=$(echo "$line" | jq -r '.name // empty' 2>/dev/null)
                [ -n "$NAME" ] && NAMED=$((NAMED + 1))
            done < "$DP_FILE"

            if [ "$BAD" -eq 0 ]; then
                pass "All $DP_NONEMPTY DataPoint lines are valid JSON"
            else
                fail "$BAD of $DP_NONEMPTY DataPoint lines are not valid JSON"
            fi

            if [ "$NAMED" -gt 0 ]; then
                pass "Found $NAMED named datapoints"
            else
                info "No datapoints with 'name' field found"
            fi

            # Show sample datapoints
            echo "    Sample datapoints:"
            head -3 "$DP_FILE" | while IFS= read -r line; do
                [ -z "$line" ] && continue
                NAME=$(echo "$line" | jq -r '.name // "?"' 2>/dev/null)
                VAL=$(echo "$line" | jq -c '.value // "?"' 2>/dev/null)
                echo "      $NAME = $VAL"
            done
        fi
    else
        info "No datapoints.json (only produced by trace-compat)"
    fi

    echo ""
done

echo "=== Summary ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"

if [ "$FAIL" -eq 0 ]; then
    echo "  Result: ALL CHECKS PASSED"
    exit 0
else
    echo "  Result: SOME CHECKS FAILED"
    exit 1
fi
