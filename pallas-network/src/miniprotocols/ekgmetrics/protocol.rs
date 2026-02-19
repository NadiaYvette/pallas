//! EKG Metrics mini-protocol implementation.
//!
//! This protocol is used to query EKG-style metrics (gauges, counters, labels)
//! from a node.
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

use pallas_codec::minicbor::{self, data::Type, decode, encode, Decode, Decoder, Encode, Encoder};
use pallas_codec::utils::AnyCbor;
use tracing::{debug, error};

#[derive(Clone, Debug, PartialEq, Eq, Decode, Encode)]
pub enum MetricValue {
    #[n(0)]
    Counter(#[n(0)] i64),
    #[n(1)]
    Gauge(#[n(0)] i64),
    #[n(2)]
    Label(#[n(0)] String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    GetAll,
    GetMetrics(Vec<String>),
    GetUpdated,
}

// Manual implementation to handle array-wrapped request enum variants.
// CDDL specifies an array [tag, value?] for these requests.
impl Encode<()> for Request {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Request::GetAll => {
                e.array(1)?;
                e.u16(0)?;
            }
            Request::GetMetrics(names) => {
                e.array(2)?;
                e.u16(1)?;
                e.encode(names)?;
            }
            Request::GetUpdated => {
                e.array(1)?;
                e.u16(2)?;
            }
        }
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for Request {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let _len = d
            .array()?
            .ok_or(decode::Error::message("Indefinite array not supported"))?;
        let tag = d.u16()?;
        match tag {
            0 => Ok(Request::GetAll),
            1 => {
                let names = d.decode()?;
                Ok(Request::GetMetrics(names))
            }
            2 => Ok(Request::GetUpdated),
            _ => Err(decode::Error::message("Invalid request tag")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Req(Request),
    Resp(Vec<AnyCbor>),
    Done,
}

impl Encode<()> for Message {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        match self {
            Message::Req(req) => {
                e.array(2)?;
                e.u16(0)?;
                e.encode(req)?;
            }
            Message::Resp(metrics) => {
                e.array(2)?;
                e.u16(1)?;
                e.encode(metrics)?;
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
    fn decode(
        d: &mut minicbor::Decoder<'b>,
        _ctx: &mut (),
    ) -> Result<Self, minicbor::decode::Error> {
        // Handle ambiguity between Done (Tag 1, Array length 1) and Resp (Tag 1, Array
        // length 2). Some implementations might not wrap Done in an array, so
        // we check data type.
        if d.datatype()? == Type::Array {
            let len = d.array()?.ok_or(minicbor::decode::Error::message(
                "Indefinite array not supported",
            ))?;
            let tag = d.u16()?;
            match (tag, len) {
                (0, 2) => Ok(Message::Req(d.decode()?)),
                (1, 1) => Ok(Message::Done),
                (1, 2) => {
                    let dt = d.datatype()?;
                    debug!("EKG Resp payload type: {:?}", dt);
                    if dt == Type::U8 {
                        let val = d.u8()?;
                        error!("EKG Resp: unexpected U8 payload: {}", val);
                        return Err(minicbor::decode::Error::message("Unexpected U8 payload"));
                    }
                    Ok(Message::Resp(d.decode()?))
                }
                (2, 1) => Ok(Message::Done),
                _ => Err(minicbor::decode::Error::message("Invalid message")),
            }
        } else {
            // Assume it's just the tag (u16/u8) for Done
            let tag = d.u16()?;
            match tag {
                2 => Ok(Message::Done), // Done is tag 2
                _ => Err(minicbor::decode::Error::message("Invalid message tag")),
            }
        }
    }
}
