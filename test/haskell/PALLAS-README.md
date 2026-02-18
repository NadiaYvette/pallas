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

## Origin

This infrastructure was originally part of the
[cardano-node](https://github.com/IntersectMBO/cardano-node) repository and
maintained in a separate `c-trace-fwd` repository. It was incorporated here
to co-locate tests with the Rust code they validate, then trimmed from ~1100
files to ~23 files by pulling library dependencies from CHaP instead of
carrying local copies.
