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
// use tracing::info;

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

/// Custom timestamp struct to match Haskell's `UTCTime` encoding.
/// Haskell encodes `UTCTime` as a Tag 1 containing an array `[day,
/// picoseconds]`. This differs from standard `SystemTime` encoding in
/// `minicbor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceTimestamp {
    pub day: i64,
    pub pico: i64,
}

impl<'b, C> Decode<'b, C> for TraceTimestamp {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, decode::Error> {
        let t = d.tag()?;
        if t == Tag::new(1) {
            let len = d.array()?;
            let day = d.i64()?;
            let pico = d.i64()?;

            // Consume any extra fields in the timestamp array (e.g. if it has 3 items)
            if let Some(l) = len {
                for _ in 2..l {
                    d.skip()?;
                }
            }

            Ok(TraceTimestamp { day, pico })
        } else if t.as_u64() == 1000 {
            // Handle Tag 1000 (SystemTime map)
            // Map { 1: seconds, -12: nanos? }
            // Note: -12 is 0x2b (N(11)).
            let len = d.map()?;
            let mut seconds = 0i64;
            let mut nanos = 0i64;

            let count = len.unwrap_or(0); // If indefinite, we loop until break, but here assume definite or handle
                                          // generic loop Actually minicbor
                                          // map() returns Option<u64>.
                                          // We should loop.

            // For simplicity, assume we know the keys.
            // But order is not guaranteed.
            for _ in 0..count {
                let key_type = d.datatype()?;
                let key = if key_type == Type::U8
                    || key_type == Type::U16
                    || key_type == Type::U32
                    || key_type == Type::U64
                {
                    d.i64()? // Positive key
                } else if key_type == Type::Int {
                    d.i64()? // Negative key
                } else {
                    d.skip()?;
                    continue;
                };

                if key == 1 {
                    seconds = d.i64()?;
                } else if key == -12 {
                    // 0x2b
                    nanos = d.i64()?;
                } else {
                    d.skip()?;
                }
            }

            // Convert to day/pico
            let day = seconds / 86400;
            let rem_seconds = seconds % 86400;
            let pico = (rem_seconds * 1_000_000_000_000) + (nanos * 1000);

            Ok(TraceTimestamp { day, pico })
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
        e.tag(Tag::new(1))?;
        e.array(2)?;
        e.i64(self.day)?;
        e.i64(self.pico)?;
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
    // Try to read null
    if d.datatype()? == Type::Null {
        d.null()?;
        return Ok(None);
    }

    // Try to read 0 (integer) as None
    if d.datatype()? == Type::U8
        || d.datatype()? == Type::U16
        || d.datatype()? == Type::U32
        || d.datatype()? == Type::U64
    {
        let val = d.u64()?;
        if val == 0 {
            return Ok(None);
        }
    }

    // Try to read array
    if d.datatype()? == Type::Array || d.datatype()? == Type::ArrayIndef {
        let len = d.array()?;
        match len {
            Some(0) => Ok(None),
            Some(1) => {
                let val = d.decode_with(ctx)?;
                Ok(Some(val))
            }
            _ => Err(decode::Error::message("invalid maybe array length")),
        }
    } else {
        // Assume it is the value itself (permissive decoding for TraceObject
        // compatibility)
        let val = d.decode_with(ctx)?;
        Ok(Some(val))
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
    pub to_machine: String,
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
            let s = d.str()?;
            // Return a dummy TraceObject wrapping this string
            return Ok(TraceObject {
                kind: None,
                to_human: Some(s.to_string()),
                to_machine: s.to_string(),
                to_namespace: vec!["StringFallback".to_string()],
                severity: Severity::Info,
                detail: Detail::Normal,
                timestamp: TraceTimestamp { day: 0, pico: 0 },
                hostname: "".to_string(),
                thread_id: "".to_string(),
            });
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
        // Encode as 9 fields if kind is present, else 8?
        // For compatibility with what we receive, we should probably encode 9 if we
        // have it.
        if let Some(k) = self.kind {
            e.array(9)?;
            e.u8(k)?;
        } else {
            e.array(8)?;
        }

        encode_maybe(e, ctx, &self.to_human)?;

        // Encode to_machine wrapped?
        // If we want to be compatible with what we receive, we should wrap it.
        // But let's stick to standard for now unless we know we need to send it.
        e.encode(&self.to_machine)?;

        e.encode(&self.to_namespace)?;
        e.encode(&self.severity)?;
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
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::Request(blocking, n) => {
                e.array(3)?;
                e.u16(1)?;
                e.bool(*blocking)?;
                e.u16(*n)?;
            }
            Message::Response(objects) => {
                e.array(2)?;
                e.u16(3)?;
                e.encode(objects)?;
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
