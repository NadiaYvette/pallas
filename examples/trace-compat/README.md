# trace-compat

`trace-compat` is a testing tool designed to verify the interoperability of Pallas's `traceobjects` mini-protocol implementation against the official Haskell `cardano-tracer` test suite.

It acts as a drop-in replacement for the `cardano-tracer` binary during the `cardano-tracer-test-ext` execution, allowing the Haskell test harness to verify that Pallas can correctly decode and handle trace objects sent by the Haskell tracing library.

## Prerequisites

*   A local checkout of the [pallas](https://github.com/txpipe/pallas) repository (this repository).
*   A local checkout of the [cardano-node](https://github.com/IntersectMBO/cardano-node) repository.
*   A working Haskell development environment (Nix is recommended for `cardano-node`).

## Setup and Usage

### 1. Build trace-compat

First, build the release version of the `trace-compat` tool:

```bash
cargo build --release -p trace-compat
```

Take note of the absolute path to the resulting binary, typically:
`<PATH_TO_PALLAS>/target/release/trace-compat`

### 2. Configure cardano-node tests

Navigate to your `cardano-node` repository checkout. You need to modify the `cardano-tracer-test-ext` test suite to use your `trace-compat` binary instead of the default `cardano-tracer`.

Open `cardano-tracer/test/cardano-tracer-test-ext.hs` and locate the `setupFwdTracer` function. Modify the `Sys.spawnProcess` call to point to your `trace-compat` binary.

**Before:**
```haskell
     externalTracerHdl <- Sys.spawnProcess "cardano-tracer"
       [ "--config" ,    "config.yaml"
       ...
```

**After:**
```haskell
     externalTracerHdl <- Sys.spawnProcess "<PATH_TO_PALLAS>/target/release/trace-compat"
       [ "--config" ,    "config.yaml"
       ...
```

*Note: You may also need to increase the queue size in the same file if you encounter overflows during stress testing:*
```haskell
       initForwarding iomgr forwardingConf{ tofQueueSize = 100000 } $
```

### 3. Run the Test

Run the test suite using the `cardano-node` project's infrastructure. If using the `workbench` makefile (recommended):

```bash
make ci-test CMD="cabal test cardano-tracer-test-ext"
```

### 4. Verification

The test suite will:
1.  Launch `trace-compat`.
2.  Connect to it via a Unix socket.
3.  Send a stream of random `TraceObject`s.
4.  `trace-compat` will decode these objects and write them to a log file in the test directory.
5.  The test suite will read this log file and verify that the content matches exactly what was sent.

A successful run confirms that Pallas correctly implements the `traceobjects` mini-protocol and handles CBOR encoding/decoding in a way that is compatible with the Haskell implementation.

### 5. Manual Invocation (Advanced)

To run the test manually with a specific working directory (useful for ensuring a clean state):

```bash
make ci-test CMD="WORKDIR=<ABSOLUTE_PATH_TO_TEST_DIR> cabal test cardano-tracer-test-ext"
```

Replace `<ABSOLUTE_PATH_TO_TEST_DIR>` with a directory where you want the test artifacts (logs, sockets) to be created.
