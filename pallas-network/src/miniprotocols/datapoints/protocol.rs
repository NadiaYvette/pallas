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

use pallas_codec::minicbor::{decode, encode, Decode, Decoder, Encode, Encoder};
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
                            let val = match val_arr_len {
                                Some(0) => None,
                                Some(1) => {
                                    // Check if next is indefinite bytes (0x5f)
                                    let dt = d.datatype()?;
                                    if dt == pallas_codec::minicbor::data::Type::BytesIndef {
                                        // Use AnyCbor to capture the raw bytes of the indefinite byte string
                                        let any: AnyCbor = d.decode()?;
                                        let cbor_slice = any.raw_bytes();

                                        // Manually decode the content.
                                        // We skip the first byte (0x5f) and create a new decoder for the rest.
                                        if cbor_slice.is_empty() || cbor_slice[0] != 0x5f {
                                            return Err(decode::Error::message(
                                                "expected indefinite byte string start",
                                            ));
                                        }

                                        let mut sub_d = Decoder::new(&cbor_slice[1..]);
                                        let mut bytes = Vec::new();
                                        while sub_d.datatype()?
                                            != pallas_codec::minicbor::data::Type::Break
                                        {
                                            let chunk = sub_d.bytes()?;
                                            bytes.extend_from_slice(chunk);
                                        }

                                        Some(bytes)
                                    } else {
                                        Some(d.decode()?)
                                    }
                                }
                                None => {
                                    // Indefinite length wrapper array
                                    if d.datatype()? == pallas_codec::minicbor::data::Type::Break {
                                        d.skip()?;
                                        None
                                    } else {
                                        let v = if d.datatype()?
                                            == pallas_codec::minicbor::data::Type::BytesIndef
                                        {
                                            d.skip()?;
                                            let mut bytes = Vec::new();
                                            while d.datatype()?
                                                != pallas_codec::minicbor::data::Type::Break
                                            {
                                                let chunk: &[u8] = d.bytes()?;
                                                bytes.extend_from_slice(chunk);
                                            }
                                            d.skip()?;
                                            bytes
                                        } else {
                                            d.decode()?
                                        };
                                        // Expect break
                                        if d.datatype()?
                                            != pallas_codec::minicbor::data::Type::Break
                                        {
                                            return Err(decode::Error::message("expected break after indefinite value wrapper item"));
                                        }
                                        d.skip()?;
                                        Some(v)
                                    }
                                }
                                _ => {
                                    return Err(decode::Error::message(
                                        "invalid value wrapper length",
                                    ))
                                }
                            };
                            values.push((name, val));
                        }
                    }
                    None => {
                        // Indefinite length list of pairs
                        loop {
                            let dt = d.datatype()?;
                            if dt == pallas_codec::minicbor::data::Type::Break {
                                break;
                            }
                            d.array()?;
                            let name: String = d.decode()?;
                            let val_arr_len = d.array()?;
                            let val = match val_arr_len {
                                Some(0) => None,
                                Some(1) => {
                                    // Check if next is indefinite bytes (0x5f)
                                    let dt = d.datatype()?;
                                    if dt == pallas_codec::minicbor::data::Type::BytesIndef {
                                        // Use AnyCbor to capture the raw bytes of the indefinite byte string
                                        let any: AnyCbor = d.decode()?;
                                        let cbor_slice = any.raw_bytes();

                                        // Manually decode the content.
                                        // We skip the first byte (0x5f) and create a new decoder for the rest.
                                        if cbor_slice.is_empty() || cbor_slice[0] != 0x5f {
                                            return Err(decode::Error::message(
                                                "expected indefinite byte string start",
                                            ));
                                        }

                                        let mut sub_d = Decoder::new(&cbor_slice[1..]);
                                        let mut bytes = Vec::new();
                                        while sub_d.datatype()?
                                            != pallas_codec::minicbor::data::Type::Break
                                        {
                                            let chunk = sub_d.bytes()?;
                                            bytes.extend_from_slice(chunk);
                                        }

                                        Some(bytes)
                                    } else {
                                        Some(d.decode()?)
                                    }
                                }
                                _ => {
                                    return Err(decode::Error::message(
                                        "invalid value wrapper length or indefinite wrapper",
                                    ))
                                }
                            };
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
