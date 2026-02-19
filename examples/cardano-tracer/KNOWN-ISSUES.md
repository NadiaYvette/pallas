# Known Issues and Work Areas: cardano-tracer (Rust)

Identified by automated code review agent during trace-forward miniprotocol
development.

---

## Resolved

The following issues were identified and fixed in this branch:

- **Fire-and-forget spawned tasks**: EKG/Datapoints `JoinHandle`s are now
  tracked and aborted on disconnect (`connection.rs`).
- **Verbosity config unused**: The `verbosity` config field is now wired to
  actual severity filtering in the logging path.
- **`unimplemented!()` panic**: `CustomRequest::decode` now returns a proper
  error instead of panicking.
- **Silent `unwrap_or(0)` data loss**: EKG metric decode failures are now
  logged with `warn!`.
- **`params.clone()` per write**: Removed unnecessary clone in the logging
  hot path.
- **Symlink Windows no-op**: Split into `cfg(unix)` and `cfg(not(unix))`.
- **`cleanup_old_logs` early-exit**: Fixed by tracking remaining file count.
- **Handshake version validation**: Replaced blind accept-first-version with
  proper negotiation using ForwardingV_1/V_2 and magic check.

---

## Work Areas

### 1. JournalMode logging (stub)

**File:** `src/logging.rs` line 74
**Effort:** Medium — requires adding a systemd journal dependency

The `JournalMode` log output is explicitly skipped:

```rust
if lp.log_mode == LogMode::JournalMode {
    // JournalMode not yet implemented; skip.
    continue;
}
```

The Haskell `cardano-tracer` supports writing trace objects to the systemd
journal. Implementing this would require adding a dependency like
`tracing-journald` or `libsystemd` bindings, and mapping `TraceObject`
fields to structured journal entries.

### 2. EKG web endpoint (config parsed but not implemented)

**File:** `src/config.rs` line 25
**Effort:** Medium — new HTTP endpoint

The `hasEKG` config field is parsed but no corresponding web interface
exists. The Haskell `cardano-tracer` exposes a web page at this endpoint
with real-time metric values. Given that the Prometheus endpoint already
works and exposes the same data in a standard format, this is a lower
priority parity gap.

### 3. `decode_cbor_to_json` duplication with trace-compat

**Files:** `src/cbor_json.rs` and `../trace-compat/src/main.rs` lines 515-583
**Effort:** Low — refactoring

The `trace-compat` example contains a near-identical copy of the
`decode_cbor_to_json` function. Deduplicating would require either making
`cbor_json` a shared library crate or having `trace-compat` depend on
`cardano-tracer` as a library (which it currently is not).

### 4. Unbounded ChannelBuffer::temp growth (upstream)

**File:** `pallas-network/src/multiplexer.rs` lines 558-612
**Effort:** Low — upstream change

The `ChannelBuffer::temp` field (`Vec<u8>`) accumulates incoming chunks
until a complete CBOR message can be decoded. There is no upper bound.
A misbehaving peer could cause unbounded memory growth.

This is in pre-existing pallas-network code, not introduced by the
trace-forward work.

### 5. TCP Linger(0) causes RST on close (upstream)

**File:** `pallas-network/src/multiplexer.rs` line 90
**Effort:** Low — upstream change

`SO_LINGER=0` causes the kernel to send TCP RST instead of graceful FIN.
Irrelevant for Unix sockets (the common case) but problematic if the
tracer is used over TCP.

This is in pre-existing pallas-network code.

### 6. Additional protocol test coverage

**Effort:** Medium — test authoring

Current tests cover basic round-trips. Missing coverage includes:
- Indefinite-length CBOR encoding variants from real Haskell nodes
- The 9-field TraceObject format (with `kind` byte)
- Tag1 timestamp round-trip (only Tag1000 has wire-bytes test)
- EKG `GetUpdated` and `GetMetrics` request variants
- Datapoints with `BytesIndef` values
- Edge cases: empty batches, maximum batch sizes, Done at various points
- Captured real wire bytes from a mainnet cardano-node
