# Known Issues: TraceObjects Protocol

Issues identified by automated code review agent during trace-forward
miniprotocol development. These require manual intervention to resolve.

---

## 1. TraceObject decode/encode asymmetry

**File:** `protocol.rs` lines 240-380
**Severity:** Medium — data loss on round-trip

The `TraceObject` decoder is intentionally permissive to handle multiple wire
formats observed from different Haskell node versions. The encoder, however,
always produces a single canonical format. This means a decode-then-encode
cycle is not idempotent — the re-encoded bytes may differ from the original.

### Specific asymmetries

**a. Wrapped vs. unwrapped fields**

The decoder accepts both wrapped (single-element array) and bare forms for
`to_machine`, `severity`, and `detail`:

```rust
// Decoder (line 264): accepts both [x] and x
let to_machine = if d.datatype()? == Type::Array {
    d.array()?;
    d.decode()?
} else {
    d.decode()?
};
```

The encoder always wraps severity and detail in single-element arrays, and
never wraps `to_machine`:

```rust
// Encoder (line 360): always bare
e.encode(&self.to_machine)?;
// Encoder (line 370): always wrapped
e.array(1)?;
e.encode(&self.severity)?;
```

If the original encoding used bare severity/detail, re-encoding wraps them.
If the original wrapped `to_machine`, re-encoding unwraps it.

**b. Namespace encoding**

The decoder accepts both array and bare string for `to_namespace`:

```rust
// Decoder (line 272): accepts [String] or String
let to_namespace = if d.datatype()? == Type::Array ... {
    d.decode()?
} else {
    let s: String = d.decode()?;
    vec![s]
};
```

The encoder always produces an indefinite-length array:

```rust
// Encoder (line 363): always indefinite array
e.begin_array()?;
for ns in &self.to_namespace { e.encode(ns)?; }
e.end()?;
```

If the original used a definite-length array, re-encoding changes it to
indefinite-length.

**c. Extra U64 field consumed silently**

The decoder silently consumes an extra integer before `hostname` (line 302):

```rust
if matches!(d.datatype()?, Type::U64 | Type::U32 | Type::U16 | Type::U8) {
    let _pico = d.u64()?;
}
```

This value is discarded and not present in the struct, so it cannot be
re-encoded. Any message containing this extra field loses it on round-trip.

**d. Hostname/thread_id format coercion**

The decoder converts integer hostnames and thread IDs to strings via
`format!("{}", d.u64()?)`. The encoder writes them as CBOR strings. The
CBOR representation changes from integer to text string.

### Impact

- The `trace-proxy` idempotency check (`bridge_trace_objects`) will report
  failures for any message exhibiting these asymmetries.
- A proxy that re-encodes TraceObjects (rather than forwarding raw bytes)
  would alter the wire format, potentially confusing downstream consumers.
- The `cardano-tracer` example is not affected in practice because it only
  decodes (never re-encodes) TraceObjects received from nodes.

### Suggested fix

This is a design trade-off. Two approaches:

1. **Preserve raw bytes:** Store the original CBOR bytes alongside the
   decoded struct (similar to how `AnyCbor` works). Use the raw bytes for
   forwarding and the decoded struct for processing. This is the approach
   the `trace-proxy` already uses for non-TraceObject protocols.

2. **Normalize on decode:** Accept the asymmetry as intentional
   normalization. Document the canonical output format and update the
   idempotency check to compare semantic equality rather than byte equality.
   This is simpler but means the proxy cannot guarantee bit-exact forwarding.

Option 1 is preferable if trace-proxy needs to be a transparent,
non-modifying intermediary. Option 2 is sufficient if the proxy is purely
a diagnostic tool (its current stated purpose).
