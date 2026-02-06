# trace-proxy

`trace-proxy` is a debugging tool that sits between a `cardano-node` and a `cardano-tracer` (or another compatible endpoint).

It verifies the idempotency of the Pallas `traceobjects` mini-protocol implementation by:
1.  Receiving `TraceObject`s from the node.
2.  Decoding them using Pallas's `minicbor` implementation.
3.  Re-encoding them back to CBOR.
4.  Comparing the re-encoded bytes with the original raw bytes.
5.  Forwarding the original message to the destination tracer.

This ensures that the decode/encode cycle is lossless and correct.

## Usage

```bash
cargo run --release -p trace-proxy -- \
  --node-socket <PATH_TO_LISTEN_SOCKET> \
  --tracer-socket <PATH_TO_REAL_TRACER_SOCKET> \
  --magic <PROTOCOL_MAGIC>
```

### Arguments

*   `--node-socket`: The path where the proxy will create a Unix socket to listen for connections from the `cardano-node`.
*   `--tracer-socket`: The path to the existing Unix socket where the real `cardano-tracer` is listening.
*   `--magic`: The protocol magic (default: 764824073 for Mainnet).

### Example Workflow

1.  Start `cardano-tracer` listening on `./tracer.sock`.
2.  Start `trace-proxy`:
    ```bash
    cargo run --release -p trace-proxy -- \
      --node-socket ./proxy.sock \
      --tracer-socket ./tracer.sock
    ```
3.  Start `cardano-node` configured to connect to `./proxy.sock` instead of the tracer directly.

The proxy will log `Idempotency check passed` for successfully verified messages, or print a warning with hex dumps if a mismatch occurs.
