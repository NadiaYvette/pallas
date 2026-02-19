# Known Issues and Work Areas: trace-proxy

Identified by automated code review agent during trace-forward miniprotocol
development.

---

## Resolved

The following issues were identified and fixed in this branch:

- **Orphaned bridge tasks**: `handle_proxy` now collects all bridge
  `JoinHandle`s and awaits them, detecting panics and connection closure.
- **No shutdown mechanism**: Added SIGINT/SIGTERM signal handling with a
  `watch` channel to break the accept loop gracefully.
- **Dead code in `try_decode_message_with_bytes`**: Removed unreachable
  duplicate `is_end_of_input()` check.

---

## Work Areas

### 1. Per-connection shutdown propagation

**Effort:** Low-Medium

When the proxy receives a shutdown signal, the accept loop breaks, but
existing proxy sessions (bridge tasks) continue until the bearer closes.
A more graceful approach would propagate the shutdown to active sessions
so they can send protocol-level Done messages before closing.

### 2. TraceObjects decode/encode idempotency

See `pallas-network/src/miniprotocols/traceobjects/KNOWN-ISSUES.md` for
details. The proxy's idempotency check reports failures for messages with
wrapped/unwrapped field asymmetries, indefinite vs. definite-length arrays,
or the silently consumed extra U64 field. This is a diagnostic tool
limitation, not a data loss issue (raw bytes are always forwarded unchanged).
