# Known Issues: trace-proxy

Issues identified by automated code review agent during trace-forward
miniprotocol development. These require manual intervention to resolve.

---

## 1. Orphaned bridge tasks

**File:** `src/main.rs` lines 102-124
**Severity:** Medium — resource leak

The `handle_proxy` function spawns bridge tasks for each protocol but does
not collect their `JoinHandle`s:

```rust
tokio::spawn(bridge_trace_objects(node_rx, tracer_tx, "Node->Tracer"));
tokio::spawn(bridge_trace_objects(tracer_rx, node_tx, "Tracer->Node"));
// ...
tokio::spawn(forward_raw(node_rx, tracer_tx, p_label.clone()));
tokio::spawn(forward_raw(tracer_rx, node_tx, p_label));
```

The function then returns `Ok(())` immediately (line 123), without waiting
for any of the bridge tasks to complete. The bridge tasks become orphaned —
they continue running in the background with no supervision.

**Consequences:**

- If a bridge task panics, it is silently swallowed. The proxy appears healthy
  but one direction of one protocol is dead.
- There is no way for the caller to detect when all bridge tasks have
  finished (i.e., when the proxied connection has fully closed).
- The `_node_handle` and `_tracer_handle` plexer handles (lines 99-100) are
  also dropped immediately, though the plexer tasks continue running.

**Suggested fix:**

Collect all `JoinHandle`s and await them, propagating errors:

```rust
let mut handles = Vec::new();

// ... in the loop:
handles.push(tokio::spawn(bridge_trace_objects(...)));
handles.push(tokio::spawn(forward_raw(...)));

// Wait for all bridges to complete
for handle in handles {
    if let Err(e) = handle.await {
        error!("Bridge task failed: {:?}", e);
    }
}
```

This also enables the caller (`main`) to detect connection closure and log it.

---

## 2. No shutdown mechanism

**File:** `src/main.rs` lines 41-51
**Severity:** Low — operational concern

The proxy has no graceful shutdown path. The main accept loop runs
indefinitely with no signal handling:

```rust
loop {
    let (node_stream, _) = listener.accept().await?;
    // ...
    tokio::spawn(async move {
        if let Err(e) = handle_proxy(node_stream, tracer_socket).await {
            error!("Proxy error: {:?}", e);
        }
    });
}
```

`Ctrl-C` (SIGINT) kills the process immediately, which may leave partially
forwarded messages in transit.

**Suggested fix:**

Add a `CancellationToken` (matching the pattern used in
`cardano-tracer/src/connection.rs`) and a `tokio::signal::ctrl_c()` handler:

```rust
let shutdown = CancellationToken::new();

// Signal handler
let shutdown_signal = shutdown.clone();
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    info!("Shutdown requested");
    shutdown_signal.cancel();
});

loop {
    tokio::select! {
        result = listener.accept() => {
            let (node_stream, _) = result?;
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                handle_proxy(node_stream, tracer_socket, shutdown).await
            });
        }
        _ = shutdown.cancelled() => break,
    }
}
```

This is especially relevant for the proxy because it sits in the middle of a
data path — abrupt termination means the node and tracer see a broken pipe
rather than a protocol-level Done message.
