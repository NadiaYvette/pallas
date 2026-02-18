# Haskell Test Infrastructure for Pallas Trace-Forwarding

This directory contains the Haskell test infrastructure for validating Pallas
Rust trace-forwarding binaries (trace-compat, trace-proxy, cardano-tracer)
against the real Haskell cardano-node tracing libraries.

## Architecture

The test suites depend on four CHaP-published Haskell libraries:

- **cardano-tracer** — The Haskell cardano-tracer library and executable
- **trace-forward** — Haskell trace-forward protocol library
- **trace-dispatcher** — Haskell trace-dispatcher (Cardano.Logging) library
- **trace-resources** — Resource monitoring types

These are pulled from [CHaP](https://chap.intersectmbo.org/) (Cardano Haskell
Packages) rather than carried locally. Only the test utility modules and test
executables live in this directory (~23 files, ~2K lines).

## Usage

### Prerequisites

1. [Nix](https://nixos.org/download.html) with flakes enabled
2. Rust binaries built: `cargo build --release -p trace-compat -p trace-proxy -p cardano-tracer`

### Running tests

```bash
cd test/haskell

# Enter the Haskell development shell
nix develop

# Build all packages
cabal build all

# Run individual test suites
cabal test cardano-tracer-test-compat    # Stress test: node -> trace-compat (Rust)
cabal test cardano-tracer-test-tracer    # Stress test: node -> Rust cardano-tracer
cabal test cardano-tracer-test-pallas    # EKG/DataPoint: node -> trace-compat (Rust)
cabal test cardano-tracer-test-ext       # Stress test: node -> trace-proxy -> Haskell cardano-tracer
cabal test cardano-tracer-test-proxy     # EKG/DataPoint: node -> trace-proxy -> Haskell cardano-tracer
```

### Environment variables

- `PALLAS_BIN_DIR` - Path to directory containing Rust binaries. Defaults to
  `../../../target/release` (resolved to absolute path at test startup).
- `WORKDIR` - Override the test working directory (default: `/tmp/test*`).

## Test suites

| Suite | What it tests | Duration |
|-------|--------------|----------|
| `cardano-tracer-test-compat` | trace-compat receives multi-threaded stress test messages | ~60s |
| `cardano-tracer-test-tracer` | Rust cardano-tracer receives multi-threaded stress test messages | ~75s |
| `cardano-tracer-test-pallas` | trace-compat receives EKG metrics, DataPoints, and TraceObjects | ~6s |
| `cardano-tracer-test-ext` | Proxy chain (trace-proxy + Haskell cardano-tracer) stress test | ~60s |
| `cardano-tracer-test-proxy` | Proxy chain EKG/DataPoint verification | ~60s |
| `cardano-tracer-test` | Pure Haskell cardano-tracer tests (no Rust binaries) | varies |

## Manual spawning

The Rust binaries can be launched manually from a shell. This is useful for
debugging, interactive testing with a real cardano-node, or verifying behavior
outside the automated test harness.

### Building the binaries

```bash
# From the pallas repo root:
cargo build --release -p trace-compat -p trace-proxy -p cardano-tracer

# Or use the provided script:
./test/haskell/scripts/build-rust-binaries.sh
```

### Launching trace-compat

trace-compat acts as a drop-in replacement for the Haskell cardano-tracer.
It listens on a Unix socket, receives TraceObjects, EKG metrics, and
DataPoints from a Haskell node (or test forwarder), and writes JSON log files.

```bash
# Using the script (creates a clean workdir, writes config, starts the binary):
./test/haskell/scripts/launch-trace-compat.sh

# Or manually:
mkdir -p /tmp/my-test/logs
cat > /tmp/my-test/config.yaml <<EOF
networkMagic: 764824073
network:
  tag: AcceptAt
  contents: "tracer.sock"
logging:
- logRoot: "logs"
  logMode: FileMode
  logFormat: ForMachine
EOF

cd /tmp/my-test
/path/to/pallas/target/release/trace-compat --config config.yaml
```

Once a forwarder connects, output appears in:
- `logs/sock@0/node-1.json` — TraceObject log (JSON lines)
- `logs/sock@0/ekg.json` — EKG metrics (JSON lines)
- `logs/sock@0/datapoints.json` — DataPoints (JSON lines)

The socket path in `config.yaml` is relative to the working directory.
Use `networkMagic: 764824073` for mainnet, `42` for testing.

### Launching Rust cardano-tracer

The Rust cardano-tracer is a full-featured trace aggregator supporting
multiple node connections, log rotation, and Prometheus metrics.

```bash
# Using the script:
./test/haskell/scripts/launch-cardano-tracer.sh

# Or manually (same config format as trace-compat):
cd /tmp/my-test
/path/to/pallas/target/release/cardano-tracer --config config.yaml
```

Output appears in `logs/<node-address>/` where the subdirectory name
is derived from the connecting socket address (e.g., `tracer.sock@0`).

### Launching the proxy chain

The proxy chain interposes trace-proxy between a node and the Haskell
cardano-tracer to verify CBOR round-trip idempotency:

```
Node/Forwarder --> tracer.sock --> trace-proxy --> tracer-real.sock --> Haskell cardano-tracer --> logs/
```

```bash
# Prerequisites: enter the Haskell dev shell first (provides cardano-tracer)
cd test/haskell && nix develop

# Using the script (starts both processes, cleans up on Ctrl-C):
./scripts/launch-proxy-chain.sh

# Or manually:
mkdir -p /tmp/proxy-test/logs /tmp/proxy-test/tracer-statedir

# Terminal 1: Start Haskell cardano-tracer
cat > /tmp/proxy-test/config.yaml <<EOF
networkMagic: 42
network:
  tag: AcceptAt
  contents: "/tmp/proxy-test/tracer-real.sock"
logging:
- logRoot: "/tmp/proxy-test/logs"
  logMode: FileMode
  logFormat: ForMachine
EOF
cardano-tracer --config /tmp/proxy-test/config.yaml --state-dir /tmp/proxy-test/tracer-statedir

# Terminal 2: Start trace-proxy (after cardano-tracer is running)
cd /tmp/proxy-test
/path/to/pallas/target/release/trace-proxy \
    --node-socket tracer.sock \
    --tracer-socket /tmp/proxy-test/tracer-real.sock \
    --magic 42

# Terminal 3: Connect a node or forwarder to /tmp/proxy-test/tracer.sock
```

The proxy logs idempotency check results to stderr. Successful round-trips
produce `Idempotency check passed`; mismatches produce hex dump warnings.

### Configuring a cardano-node

To connect a real cardano-node to any of the above, add to the node's
configuration:

```yaml
TraceOptionForwarder:
  filePath: "/tmp/my-test/tracer.sock"
```

Or set the `--tracer-socket-path-connect` CLI flag.

## Output file formats

All output files use JSON lines format (one JSON object per line).

### node-1.json (TraceObjects)

Each non-empty line is a trace message. The first line may be empty.

```json
{"at":"1234567890.000000000","ns":["demoNamespace"],"data":{"msg":"Very big message"},"sev":"Info","thread":"1","host":"nixos"}
```

Fields vary by message type. The `data` field contains the `toMachine` payload.

### ekg.json (EKG Metrics)

Each line contains a metric name, type, and value.

```json
{"name":"rts.gc.bytes_allocated","value":{"type":"Counter","val":1048576}}
{"name":"test.gauge","value":{"type":"Gauge","val":123}}
{"name":"ekg.server_timestamp_ms","value":{"type":"Counter","val":1706000000000}}
```

Metric types: `Counter`, `Gauge`, `Label`.

### datapoints.json (DataPoints)

Each line contains a named datapoint with its current value.

```json
{"name":"test.datapoint","value":"datapoint-value"}
{"name":"test.data.point","value":{"tdpName":"tdpName for Tests","tdpCommit":"ab23c45","tdpVersion":32}}
```

Values are arbitrary JSON (string, object, null).

### Quick verification with jq

```bash
# Count non-empty log lines:
grep -c '.' logs/sock@0/node-1.json

# Validate all lines are JSON:
jq empty logs/sock@0/node-1.json

# Find a specific EKG metric:
jq 'select(.name == "test.gauge")' logs/sock@0/ekg.json

# List all datapoint names:
jq -r '.name' logs/sock@0/datapoints.json

# Or use the verification script:
./test/haskell/scripts/verify-logs.sh /tmp/manual-trace-compat/logs
```

## Scripts

Shell scripts in `scripts/` for building, launching, and verifying:

| Script | Purpose |
|--------|---------|
| `build-rust-binaries.sh` | Build trace-compat, trace-proxy, and cardano-tracer in release mode |
| `launch-trace-compat.sh` | Launch trace-compat in a clean workdir with default config |
| `launch-cardano-tracer.sh` | Launch Rust cardano-tracer in a clean workdir with default config |
| `launch-proxy-chain.sh` | Launch trace-proxy + Haskell cardano-tracer (requires `nix develop`) |
| `verify-logs.sh <dir>` | Verify JSON validity and structure of output log files |
| `run-haskell-tests.sh [suite]` | Build Rust binaries and run Haskell test suites (requires `nix develop`) |

All scripts accept configuration via environment variables (see comments at
the top of each script). Common variables:

- `PALLAS_BIN_DIR` — Path to Rust binaries (default: `<repo>/target/release`)
- `WORKDIR` — Working directory (default: `/tmp/manual-<binary-name>`)
- `NETWORK_MAGIC` — Protocol magic number (default: `42`)

## Origin

This infrastructure was originally part of the
[cardano-node](https://github.com/IntersectMBO/cardano-node) repository and
maintained in a separate `c-trace-fwd` repository. It was incorporated here
to co-locate tests with the Rust code they validate, then trimmed from ~1100
files to ~23 files by pulling library dependencies from CHaP instead of
carrying local copies.
