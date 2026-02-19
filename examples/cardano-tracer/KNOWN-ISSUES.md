# Known Issues: cardano-tracer (Rust)

Issues identified by automated code review agent during trace-forward
miniprotocol development. These require manual intervention to resolve.

---

## 1. Fire-and-forget spawned tasks

**File:** `src/connection.rs` lines 98-108
**Severity:** Medium — resource leak / silent failure

The EKG metrics and Datapoints handlers are spawned with `tokio::spawn` but
their `JoinHandle`s are discarded:

```rust
tokio::spawn(async move {
    run_ekg_handler(ekg_channel, ekg_node_id, ekg_registry, ekg_freq, ekg_shutdown).await;
});

tokio::spawn(async move {
    run_datapoints_handler(dp_channel, dp_node_id, dp_registry, dp_shutdown).await;
});
```

Similarly, the multiplexer plexer handle on line 64:

```rust
let _plexer_handle = plexer.spawn();
```

**Consequences:**

- If a spawned task panics, the panic is silently swallowed. The main
  connection loop (`run_trace_objects_loop`) continues running without
  EKG/Datapoints functionality, with no indication of failure.
- On connection teardown, the spawned tasks are not awaited. They will
  eventually stop when the shutdown token is cancelled or the channel closes,
  but there is a race window where the node is deregistered (line 123) while
  spawned tasks still hold references to the registry.

**Suggested fix:**

Collect the `JoinHandle`s and use `tokio::select!` or `futures::join!` to
run all protocol loops concurrently, propagating panics and errors:

```rust
let ekg_handle = tokio::spawn(async move { run_ekg_handler(...).await });
let dp_handle = tokio::spawn(async move { run_datapoints_handler(...).await });

let result = tokio::select! {
    r = run_trace_objects_loop(...) => r,
    r = ekg_handle => { /* handle join error / panic */ },
    r = dp_handle => { /* handle join error / panic */ },
};
```

Alternatively, abort the spawned tasks on disconnect before deregistering the
node.

---

## 2. Unbounded ChannelBuffer::temp growth (upstream)

**File:** `pallas-network/src/multiplexer.rs` lines 558-612
**Severity:** Low — memory pressure under adversarial conditions

The `ChannelBuffer::temp` field (`Vec<u8>`) accumulates incoming chunks until
a complete CBOR message can be decoded. There is no upper bound on this
buffer. A misbehaving or malicious peer could send an arbitrarily large
number of small chunks that never form a complete message, causing unbounded
memory growth.

This affects all three trace-forward protocols (TraceObjects, EKG Metrics,
Datapoints) since they all use `ChannelBuffer::recv_full_msg()`.

**Note:** This is in pre-existing pallas-network code, not introduced by the
trace-forward work, but it is documented here as it directly impacts the
tracer's resilience.

**Suggested fix:**

Add an optional `max_buffer_size` parameter to `ChannelBuffer`. Return an
error if `temp.len()` exceeds the limit after extending with a new chunk.
A reasonable default for trace-forward might be 16 MiB, since TraceObject
batches can be large but should not be unbounded.

---

## 3. TCP Linger(0) causes RST on close (upstream)

**File:** `pallas-network/src/multiplexer.rs` line 90
**Severity:** Low — data loss on orderly shutdown (TCP bearers only)

```rust
sock_ref.set_linger(Some(std::time::Duration::from_secs(0)))?;
```

Setting `SO_LINGER` to 0 seconds causes the kernel to send a TCP RST instead
of a graceful FIN when the socket is closed. This means any data still in the
kernel send buffer is discarded without being transmitted to the peer.

For Unix socket bearers (the common case for trace-forward), this setting is
irrelevant. But if the tracer is ever used over TCP (e.g., remote node
monitoring), a TraceObjects Done message or final EKG response could be lost.

**Note:** This is in pre-existing pallas-network code. It is documented here
because the trace-forward use case involves long-lived connections where
orderly shutdown semantics matter.

**Suggested fix:**

Either remove the `set_linger` call entirely (the kernel default is to linger
long enough to flush the send buffer) or use a non-zero timeout:

```rust
sock_ref.set_linger(Some(std::time::Duration::from_secs(5)))?;
```
