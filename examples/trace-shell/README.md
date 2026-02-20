# trace-shell

Interactive trace-forward protocol shell for Cardano tracing.

trace-shell connects to a `cardano-tracer` (Rust or Haskell) as a simulated
node and lets you craft custom trace objects, EKG metrics, and datapoints from
an interactive prompt. It speaks the same trace-forward protocol that a real
`cardano-node` uses.

Features:

- GNU readline-style line editing (arrow keys, Ctrl-A/E/K/U/W, Home/End)
- Persistent command history (`~/.trace-shell-history`)
- Tab completion for commands, subcommands, flags, and flag values
- Context-sensitive help (`help <command>` or `<command> help`)

## Quick start

### With the Rust cardano-tracer

The fastest way to get started. No Haskell toolchain needed.

```bash
# From the pallas repo root — builds both binaries, launches the Rust tracer
# in the background, then drops you into the shell:
./examples/trace-shell/try-it.sh
```

In a second terminal, watch the log output:

```bash
tail -f /tmp/trace-shell-demo/logs/*/node-*.json
```

### With the Haskell cardano-tracer

Use this to validate trace-shell against the stock Haskell implementation.
Requires `nix develop` in `test/haskell/` to provide the `cardano-tracer`
binary.

```bash
# Terminal 1: enter the Haskell dev shell and launch the tracer + shell
cd test/haskell && nix develop
# then from inside the nix shell:
../../examples/trace-shell/try-haskell.sh

# Terminal 2: watch logs
tail -f /tmp/trace-shell-haskell/logs/*/node-*.json
```

See [try-haskell.sh](try-haskell.sh) for details and environment variables.

### Manual connection

If you already have a tracer running on a known socket:

```bash
cargo run -p trace-shell -- --socket /path/to/tracer.sock --magic 764824073
```

Or connect from inside the shell:

```
trace> connect /path/to/tracer.sock --magic 764824073
trace> connect tcp:127.0.0.1:3300
```

## Commands

### Connection

| Command | Description |
|---------|-------------|
| `connect <socket> [--magic N]` | Connect to a tracer (Unix socket or `tcp:host:port`) |
| `disconnect` | Close the connection |
| `status` | Show connection state, queue depths, poll counts, log files |

### Trace objects

| Command | Description |
|---------|-------------|
| `trace add <msg> [flags]` | Queue a trace object for delivery to the tracer |
| `trace list` | Show queued (undelivered) trace objects |
| `trace clear` | Clear the trace queue |
| `trace auto <ms> [--prefix P]` | Auto-generate traces at an interval |
| `trace auto stop` | Stop auto-generation |

Flags for `trace add`:

- `--severity <S>` — Debug, Info, Notice, Warning, Error, Critical, Alert, Emergency (default: Info)
- `--detail <D>` — Minimal, Normal, Detailed, Maximum (default: Normal)
- `--ns <A.B.C>` — Dot-separated namespace (default: empty)
- `--hostname <H>` — Override hostname field
- `--thread <T>` — Override thread ID field

### EKG metrics

| Command | Description |
|---------|-------------|
| `metric set <name> <type> <val>` | Set a metric (type: `counter`, `gauge`, or `label`) |
| `metric get <name>` | Show a specific metric |
| `metric list` | List all metrics |
| `metric del <name>` | Delete a metric |
| `metric clear` | Clear all metrics |
| `metric incr <name> [delta]` | Increment a counter (default delta: 1) |

### Datapoints

| Command | Description |
|---------|-------------|
| `datapoint set <name> <value>` | Set a datapoint (UTF-8 value) |
| `datapoint get <name>` | Show a specific datapoint |
| `datapoint list` | List all datapoints |
| `datapoint del <name>` | Delete a datapoint |
| `datapoint clear` | Clear all datapoints |
| `datapoint nodeinfo [flags]` | Set the standard NodeInfo datapoint |

Flags for `datapoint nodeinfo`:

- `--version <V>` — Node version string
- `--commit <C>` — Git commit hash
- `--start-time <T>` — Node start time
- `--protocol <P>` — Protocol version

Aliases: `dp` = `datapoint`, `del`/`rm` = `delete`.

## How it works

trace-shell acts as a Mux Initiator running protocol Servers (the Responder
role in trace-forward terminology), which is the same topology a `cardano-node`
uses. The tracer (Rust or Haskell) is the one that initiates protocol requests:

```
tracer  ──Request(blocking, N)──>  trace-shell
tracer  <──Response([objects])───  trace-shell
```

The three multiplexed mini-protocols are:

1. **TraceObjects** — The tracer polls for trace messages. When `blocking=true`,
   trace-shell waits until data is available before responding (avoiding a
   busy-loop).
2. **EKG Metrics** — The tracer periodically requests a snapshot of all metrics.
3. **Datapoints** — The tracer periodically requests named datapoints (e.g.,
   `NodeInfo`).

Data you inject via the shell is stored in shared in-memory state. When the
tracer polls, it receives whatever is currently in the store. Trace objects are
delivered once and removed from the queue; metrics and datapoints persist until
you delete them.

## Network magic

The `--magic` flag (or `connect ... --magic N`) must match the tracer's
`networkMagic` config. Mismatched magic causes an immediate handshake refusal.

| Network | Magic |
|---------|-------|
| Mainnet | 764824073 (default) |
| Preprod | 1 |
| Preview | 2 |
| Local testnet | 42 |

## Scripts

| Script | Description |
|--------|-------------|
| [try-it.sh](try-it.sh) | Build and run trace-shell with the **Rust** cardano-tracer |
| [try-haskell.sh](try-haskell.sh) | Run trace-shell with the **Haskell** cardano-tracer (requires `nix develop`) |

Both scripts accept `--no-build` to skip `cargo build` and `--shell-only SOCK`
to connect to an already-running tracer.

## Building

```bash
cargo build -p trace-shell
```

Dependencies: `pallas` (with `hardano` feature), `tokio`, `clap`, `rustyline`,
`tracing`, `tracing-subscriber`.
