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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Request(Vec<String>),
    Response(Vec<(String, Option<Vec<u8>>)>),
    Done,
}

// Manual implementation for flat-array message structure.
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
            Message::Response(values) => {
                e.array(2)?;
                e.u16(3)?;
                e.encode(values)?;
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
        d.array()?;
        let tag = d.u16()?;
        match tag {
            1 => Ok(Message::Request(d.decode()?)),
            3 => Ok(Message::Response(d.decode()?)),
            2 => Ok(Message::Done),
            _ => Err(decode::Error::message("unknown tag")),
        }
    }
}
