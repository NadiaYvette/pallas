//! Datapoints mini-protocol implementation.
//!
//! This protocol is used to forward arbitrary data structures (datapoints) from
//! a node.
//!
//! # Specification
//!
//! The protocol messages and data structures are defined in the `ekg-forward`
//! library.
//!
//! Reference: [ekg-forward](https://github.com/input-output-hk/ekg-forward)
//!
//! See `System.Metrics.Protocol.Type` in the Haskell source for the protocol
//! definition.

use pallas_codec::minicbor::{data::Type, decode, encode, Decode, Decoder, Encode, Encoder};
use pallas_codec::utils::AnyCbor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Request(Vec<String>),
    Response(Vec<(String, Option<Vec<u8>>)>),
    Done,
}

impl Encode<()> for Message {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::Request(names) => {
                e.array(2)?;
                e.u16(1)?;
                e.encode(names)?;
            }
            Message::Done => {
                e.array(1)?;
                e.u16(2)?;
            }
            Message::Response(values) => {
                e.array(2)?;
                e.u16(3)?;
                e.array(values.len() as u64)?;
                for (name, value) in values {
                    e.array(2)?;
                    e.encode(name)?;
                    match value {
                        Some(bytes) => {
                            e.array(1)?;
                            e.encode(bytes)?;
                        }
                        None => {
                            e.array(0)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Decode an indefinite-length byte string captured as AnyCbor.
///
/// The raw bytes start with 0x5f (indefinite byte string marker) followed by
/// definite-length byte string chunks, terminated by a Break (0xff).
fn decode_indefinite_bytes_from_any(any: &AnyCbor) -> Result<Vec<u8>, decode::Error> {
    let cbor_slice = any.raw_bytes();

    if cbor_slice.is_empty() || cbor_slice[0] != 0x5f {
        return Err(decode::Error::message(
            "expected indefinite byte string start",
        ));
    }

    let mut sub_d = Decoder::new(&cbor_slice[1..]);
    let mut bytes = Vec::new();
    while sub_d.datatype()? != Type::Break {
        let chunk = sub_d.bytes()?;
        bytes.extend_from_slice(chunk);
    }

    Ok(bytes)
}

/// Decode a datapoint value from inside a value-wrapper array of known length.
fn decode_value_wrapper(
    d: &mut Decoder<'_>,
    val_arr_len: Option<u64>,
) -> Result<Option<Vec<u8>>, decode::Error> {
    match val_arr_len {
        Some(0) => Ok(None),
        Some(1) => {
            let dt = d.datatype()?;
            if dt == Type::BytesIndef {
                let any: AnyCbor = d.decode()?;
                Ok(Some(decode_indefinite_bytes_from_any(&any)?))
            } else {
                Ok(Some(d.decode()?))
            }
        }
        None => {
            // Indefinite length wrapper array
            if d.datatype()? == Type::Break {
                d.skip()?;
                Ok(None)
            } else {
                let v = if d.datatype()? == Type::BytesIndef {
                    d.skip()?;
                    let mut bytes = Vec::new();
                    while d.datatype()? != Type::Break {
                        let chunk: &[u8] = d.bytes()?;
                        bytes.extend_from_slice(chunk);
                    }
                    d.skip()?;
                    bytes
                } else {
                    d.decode()?
                };
                // Expect break to close the indefinite wrapper array
                if d.datatype()? != Type::Break {
                    return Err(decode::Error::message(
                        "expected break after indefinite value wrapper item",
                    ));
                }
                d.skip()?;
                Ok(Some(v))
            }
        }
        _ => Err(decode::Error::message("invalid value wrapper length")),
    }
}

impl<'b> Decode<'b, ()> for Message {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        d.array()?;
        let tag = d.u16()?;
        match tag {
            1 => Ok(Message::Request(d.decode()?)),
            2 => Ok(Message::Done),
            3 => {
                let len = d.array()?;
                let mut values = Vec::new();
                match len {
                    Some(l) => {
                        for _ in 0..l {
                            d.array()?;
                            let name: String = d.decode()?;
                            let val_arr_len = d.array()?;
                            let val = decode_value_wrapper(d, val_arr_len)?;
                            values.push((name, val));
                        }
                    }
                    None => {
                        // Indefinite length list of pairs
                        loop {
                            let dt = d.datatype()?;
                            if dt == Type::Break {
                                break;
                            }
                            d.array()?;
                            let name: String = d.decode()?;
                            let val_arr_len = d.array()?;
                            let val = decode_value_wrapper(d, val_arr_len)?;
                            values.push((name, val));
                        }
                        d.skip()?; // Skip the Break
                    }
                }
                Ok(Message::Response(values))
            }
            _ => Err(decode::Error::message("unknown tag")),
        }
    }
}
