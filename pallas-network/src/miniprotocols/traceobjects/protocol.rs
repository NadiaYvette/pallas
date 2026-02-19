//! TraceObjects mini-protocol implementation.
//!
//! This protocol is used to forward structured log objects (traces) from a node
//! to a tracer.
//!
//! # Specification
//!
//! The protocol messages and data structures are defined in the
//! `trace-dispatcher` library within the `cardano-node` repository.
//!
//! Reference: [cardano-node/trace-dispatcher](https://github.com/IntersectMBO/cardano-node/tree/master/trace-dispatcher)
//!
//! Specifically, the `TraceObject` structure corresponds to the Haskell
//! definition in `Cardano.Logging.Types`.

use pallas_codec::minicbor::{
    self,
    data::{Tag, Type},
    decode, encode, Decode, Decoder, Encode, Encoder,
};
use pallas_codec::utils::AnyCbor;

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
#[cbor(index_only)]
pub enum Severity {
    #[n(0)]
    Debug,
    #[n(1)]
    Info,
    #[n(2)]
    Notice,
    #[n(3)]
    Warning,
    #[n(4)]
    Error,
    #[n(5)]
    Critical,
    #[n(6)]
    Alert,
    #[n(7)]
    Emergency,
}

#[derive(Clone, Debug, Decode, Encode, Eq, PartialEq)]
#[cbor(index_only)]
pub enum Detail {
    #[n(0)]
    Minimal,
    #[n(1)]
    Normal,
    #[n(2)]
    Detailed,
    #[n(3)]
    Maximum,
}

/// Timestamp preserving the original CBOR tag format for round-trip fidelity.
///
/// Haskell's `trace-forward` uses two timestamp encodings:
/// - Tag 1: `[day, picoseconds_since_midnight]` — UTCTime representation
/// - Tag 1000: `map { 1: seconds_since_epoch, -12: picoseconds_within_second }`
///   — SystemTime representation from the `serialise` library
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceTimestamp {
    /// Tag 1: [day, pico_since_midnight]
    Tag1 { day: i64, pico: i64 },
    /// Tag 1000: map { 1: seconds_since_epoch, -12: pico_within_second }
    Tag1000 { seconds: i64, pico: i64 },
}

impl TraceTimestamp {
    /// Returns (seconds_since_epoch, pico_within_second) for display/formatting.
    pub fn as_seconds_pico(&self) -> (i64, i64) {
        match self {
            TraceTimestamp::Tag1 { day, pico } => {
                let seconds = day * 86400 + pico / 1_000_000_000_000;
                let sub_pico = pico % 1_000_000_000_000;
                (seconds, sub_pico)
            }
            TraceTimestamp::Tag1000 { seconds, pico } => (*seconds, *pico),
        }
    }
}

impl<'b, C> Decode<'b, C> for TraceTimestamp {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, decode::Error> {
        let t = d.tag()?;
        if t == Tag::new(1) {
            let len = d.array()?;
            let day = d.i64()?;
            let pico = d.i64()?;

            // Consume any extra fields in the timestamp array
            if let Some(l) = len {
                for _ in 2..l {
                    d.skip()?;
                }
            }

            Ok(TraceTimestamp::Tag1 { day, pico })
        } else if t.as_u64() == 1000 {
            // Tag 1000: SystemTime map { 1: seconds, -12: picoseconds }
            let len = d.map()?;
            let mut seconds = 0i64;
            let mut pico = 0i64;

            let count = len.unwrap_or(0);

            for _ in 0..count {
                let key_type = d.datatype()?;
                let key = match key_type {
                    // Positive integer keys
                    Type::U8 | Type::U16 | Type::U32 | Type::U64 => d.i64()?,
                    // Negative integer keys (e.g. -12 encoded as 0x2b)
                    Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => d.i64()?,
                    _ => {
                        d.skip()?;
                        continue;
                    }
                };

                if key == 1 {
                    seconds = d.i64()?;
                } else if key == -12 {
                    pico = d.i64()?;
                } else {
                    d.skip()?;
                }
            }

            Ok(TraceTimestamp::Tag1000 { seconds, pico })
        } else {
            Err(decode::Error::message("expected timestamp tag (1 or 1000)"))
        }
    }
}

impl<C> Encode<C> for TraceTimestamp {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            TraceTimestamp::Tag1 { day, pico } => {
                e.tag(Tag::new(1))?;
                e.array(2)?;
                e.i64(*day)?;
                e.i64(*pico)?;
            }
            TraceTimestamp::Tag1000 { seconds, pico } => {
                e.tag(Tag::new(1000))?;
                e.map(2)?;
                e.i64(1)?;
                e.i64(*seconds)?;
                // Key -12 (CBOR negative int 11 = 0x2b)
                e.i64(-12)?;
                e.i64(*pico)?;
            }
        }
        Ok(())
    }
}

/// Helper to decode `Option<T>` where Haskell `Maybe` is encoded as a list.
/// `Nothing` -> `[]` (len 0)
/// `Just x` -> `[x]` (len 1)
fn decode_maybe<'b, T, C>(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Option<T>, decode::Error>
where
    T: Decode<'b, C>,
{
    let dt = d.datatype()?;

    match dt {
        // Null → Nothing
        Type::Null => {
            d.null()?;
            Ok(None)
        }
        // Array → Haskell Maybe encoding: [] = Nothing, [x] = Just x
        Type::Array | Type::ArrayIndef => {
            let len = d.array()?;
            match len {
                Some(0) => Ok(None),
                Some(1) => {
                    let val = d.decode_with(ctx)?;
                    Ok(Some(val))
                }
                _ => Err(decode::Error::message("invalid maybe array length")),
            }
        }
        // Anything else → treat as the value itself (permissive decoding for
        // TraceObject compatibility)
        _ => {
            let val = d.decode_with(ctx)?;
            Ok(Some(val))
        }
    }
}

/// Helper to encode `Option<T>` as a list to match Haskell `Maybe` encoding.
fn encode_maybe<T, C, W: encode::Write>(
    e: &mut Encoder<W>,
    ctx: &mut C,
    val: &Option<T>,
) -> Result<(), encode::Error<W::Error>>
where
    T: Encode<C>,
{
    match val {
        None => {
            e.array(0)?;
            Ok(())
        }
        Some(x) => {
            e.array(1)?;
            e.encode_with(x, ctx)?;
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceObject {
    pub kind: Option<u8>, // New field observed in trace-forward (00)
    pub to_human: Option<String>,
    pub to_machine: AnyCbor,
    pub to_namespace: Vec<String>,
    pub severity: Severity,
    pub detail: Detail,
    pub timestamp: TraceTimestamp,
    pub hostname: String,
    pub thread_id: String,
}

// Manual Encode/Decode implementation required because:
// 1. `to_human` needs custom `Maybe` encoding (array-wrapped).
// 2. We need strictly ordered array encoding for the struct, which derive
//    macros might not guarantee with all options.
impl<'b, C> Decode<'b, C> for TraceObject {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, decode::Error> {
        // Handle case where TraceObject is represented as a String (e.g. simplified
        // logging or error)
        if matches!(d.datatype(), Ok(Type::String) | Ok(Type::StringIndef)) {
            let _s = d.str()?;
            return Err(decode::Error::message(format!(
                "Invalid message tag: String or StringIndef"
            )));
        }

        let len = d.array()?;

        // Handle 9 fields (new format) or 8 fields (old format)
        let kind = if len == Some(9) {
            let k = d.u8()?;
            Some(k)
        } else {
            None
        };

        let to_human = decode_maybe(d, ctx)?;

        // Handle wrapped to_machine
        let to_machine = if d.datatype()? == Type::Array {
            d.array()?;
            d.decode()?
        } else {
            d.decode()?
        };

        // Handle to_namespace (permissive: [String] or String)
        let to_namespace = if d.datatype()? == Type::Array || d.datatype()? == Type::ArrayIndef {
            d.decode()?
        } else {
            let s: String = d.decode()?;
            vec![s]
        };

        // Handle wrapped severity
        let severity = if d.datatype()? == Type::Array {
            d.array()?;
            d.decode()?
        } else {
            d.decode()?
        };

        // Handle wrapped detail
        let detail = if d.datatype()? == Type::Array {
            d.array()?;
            d.decode()?
        } else {
            d.decode()?
        };

        let timestamp = d.decode_with(ctx)?;

        // Handle the observed wire format where an extra U64 (pico?) is present before
        // hostname, and thread_id follows (potentially outside the array
        // count).

        // If next is U64, consume it (pico)
        if matches!(d.datatype()?, Type::U64 | Type::U32 | Type::U16 | Type::U8) {
            let _pico = d.u64()?;
        }

        // Now read hostname
        let hostname = if d.datatype()? == Type::String || d.datatype()? == Type::StringIndef {
            d.str()?.to_string()
        } else if d.datatype()? == Type::Null {
            d.skip()?;
            String::new()
        } else {
            if matches!(d.datatype()?, Type::U8 | Type::U16 | Type::U32 | Type::U64) {
                format!("{}", d.u64()?)
            } else {
                d.decode()?
            }
        };

        // Now read thread_id
        let thread_id = if d.datatype()? == Type::String || d.datatype()? == Type::StringIndef {
            d.str()?.to_string()
        } else {
            if matches!(d.datatype()?, Type::U8 | Type::U16 | Type::U32 | Type::U64) {
                format!("{}", d.u64()?)
            } else {
                d.decode()?
            }
        };

        Ok(TraceObject {
            kind,
            to_human,
            to_machine,
            to_namespace,
            severity,
            detail,
            timestamp,
            hostname,
            thread_id,
        })
    }
}

impl<C> Encode<C> for TraceObject {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        if let Some(k) = self.kind {
            e.array(9)?;
            e.u8(k)?;
        } else {
            e.array(8)?;
        }

        encode_maybe(e, ctx, &self.to_human)?;

        e.encode(&self.to_machine)?;

        // Encode namespace as indefinite-length array to match Haskell's cborg
        e.begin_array()?;
        for ns in &self.to_namespace {
            e.encode(ns)?;
        }
        e.end()?;

        // Severity and Detail: Haskell encodes enum constructors as [index]
        e.array(1)?;
        e.encode(&self.severity)?;
        e.array(1)?;
        e.encode(&self.detail)?;

        e.encode_with(&self.timestamp, ctx)?;
        e.encode(&self.hostname)?;
        e.encode(&self.thread_id)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Request(bool, u16),
    Response(Vec<TraceObject>),
    Done,
}

impl Encode<()> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::Request(blocking, n) => {
                e.array(3)?;
                e.u16(1)?;
                e.bool(*blocking)?;
                // Encode n as [0, n] for compatibility with cardano-tracer
                e.array(2)?;
                e.u16(0)?;
                e.u16(*n)?;
            }
            Message::Response(objects) => {
                e.array(2)?;
                e.u16(3)?;
                // Indefinite-length array to match Haskell's cborg encoding
                e.begin_array()?;
                for obj in objects {
                    e.encode_with(obj, ctx)?;
                }
                e.end()?;
            }
            Message::Done => {
                e.array(1)?;
                e.u16(2)?;
            }
        }
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Message {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let probe = d.datatype()?;
        // Some implementations (like cardano-tracer) wrap the message in an outer
        // array.
        if probe == Type::Array || probe == Type::ArrayIndef {
            d.array()?;
            let tag = d.u16()?;
            match tag {
                1 => {
                    let blocking = d.bool()?;

                    // Handle case where 'n' is encoded as a single-element array [0, n] or just n
                    // This behavior was observed with cardano-tracer.
                    let n = if d.datatype()? == Type::Array || d.datatype()? == Type::ArrayIndef {
                        let len = d.array()?;
                        if let Some(2) = len {
                            let _unknown = d.u16()?;
                            d.u16()?
                        } else {
                            d.u16()?
                        }
                    } else {
                        d.u16()?
                    };

                    Ok(Message::Request(blocking, n))
                }
                3 => {
                    let objects = d.decode()?;
                    Ok(Message::Response(objects))
                }
                2 => Ok(Message::Done),
                t => Err(decode::Error::message(format!(
                    "Invalid message tag: {}",
                    t
                ))),
            }
        } else {
            // Assume it's just the tag (Standard mode)
            let tag = d.u16()?;
            match tag {
                1 => {
                    let blocking = d.bool()?;
                    let n = d.u16()?;
                    Ok(Message::Request(blocking, n))
                }
                2 => Ok(Message::Done),
                t => Err(decode::Error::message(format!(
                    "Invalid message tag: {}",
                    t
                ))),
            }
        }
    }
}
