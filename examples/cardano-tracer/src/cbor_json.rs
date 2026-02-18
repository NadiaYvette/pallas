use pallas::codec::minicbor::data::Type;
use pallas::codec::minicbor::{decode, Decoder};
use pallas::network::miniprotocols::traceobjects::TraceObject;

/// Recursively decode a CBOR value into a serde_json::Value.
pub fn decode_cbor_to_json(d: &mut Decoder) -> Result<serde_json::Value, decode::Error> {
    match d.datatype()? {
        Type::Null => {
            d.null()?;
            Ok(serde_json::Value::Null)
        }
        Type::Bool => Ok(serde_json::Value::Bool(d.bool()?)),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => Ok(serde_json::json!(d.u64()?)),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => {
            Ok(serde_json::json!(d.i64()?))
        }
        Type::F16 => Ok(serde_json::json!(d.f16()? as f64)),
        Type::F32 => Ok(serde_json::json!(d.f32()? as f64)),
        Type::F64 => Ok(serde_json::json!(d.f64()?)),
        Type::String | Type::StringIndef => Ok(serde_json::Value::String(d.str()?.to_string())),
        Type::Bytes | Type::BytesIndef => Ok(serde_json::Value::String(hex::encode(d.bytes()?))),
        Type::Array | Type::ArrayIndef => {
            let len = d.array()?;
            let mut vec = Vec::new();
            match len {
                Some(l) => {
                    for _ in 0..l {
                        vec.push(decode_cbor_to_json(d)?);
                    }
                }
                None => {
                    while d.datatype()? != Type::Break {
                        vec.push(decode_cbor_to_json(d)?);
                    }
                    d.skip()?; // consume Break
                }
            }
            Ok(serde_json::Value::Array(vec))
        }
        Type::Map | Type::MapIndef => {
            let len = d.map()?;
            let mut map = serde_json::Map::new();
            match len {
                Some(l) => {
                    for _ in 0..l {
                        let key = decode_cbor_to_json(d)?;
                        let val = decode_cbor_to_json(d)?;
                        let k = match key {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        };
                        map.insert(k, val);
                    }
                }
                None => {
                    while d.datatype()? != Type::Break {
                        let key = decode_cbor_to_json(d)?;
                        let val = decode_cbor_to_json(d)?;
                        let k = match key {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        };
                        map.insert(k, val);
                    }
                    d.skip()?; // consume Break
                }
            }
            Ok(serde_json::Value::Object(map))
        }
        Type::Tag => {
            d.tag()?;
            decode_cbor_to_json(d)
        }
        _ => {
            d.skip()?;
            Ok(serde_json::Value::Null)
        }
    }
}

/// Convert a TraceObject to a JSON log line for ForMachine output.
///
/// The `to_machine` field is a CBOR-encoded text string containing the
/// complete JSON log line (as produced by trace-dispatcher). We decode it
/// and write it directly. If it's not a string, we wrap the decoded value
/// in a JSON envelope with metadata from the TraceObject.
pub fn trace_object_to_log_line(obj: &TraceObject) -> String {
    let mut decoder = Decoder::new(obj.to_machine.raw_bytes());
    match decode_cbor_to_json(&mut decoder) {
        Ok(serde_json::Value::String(s)) => {
            // to_machine was a CBOR text string; the string itself is the JSON log line.
            s
        }
        Ok(v) => {
            // to_machine decoded to a non-string JSON value; wrap in envelope.
            let (ts_secs, ts_pico) = obj.timestamp.as_seconds_pico();
            serde_json::json!({
                "at": format!("{}.{:012}", ts_secs, ts_pico),
                "ns": obj.to_namespace,
                "sev": format!("{:?}", obj.severity),
                "thread": obj.thread_id,
                "host": obj.hostname,
                "data": v
            })
            .to_string()
        }
        Err(_) => {
            // Fallback: hex-encode the raw CBOR bytes.
            let (ts_secs, ts_pico) = obj.timestamp.as_seconds_pico();
            serde_json::json!({
                "at": format!("{}.{:012}", ts_secs, ts_pico),
                "ns": obj.to_namespace,
                "sev": format!("{:?}", obj.severity),
                "thread": obj.thread_id,
                "host": obj.hostname,
                "data": hex::encode(obj.to_machine.raw_bytes())
            })
            .to_string()
        }
    }
}

/// Convert a TraceObject to a human-readable log line for ForHuman output.
///
/// Uses the `to_human` field if available, prefixed with timestamp and severity.
/// Falls back to the machine representation if `to_human` is absent.
pub fn trace_object_to_human_line(obj: &TraceObject) -> String {
    let (ts_secs, _ts_pico) = obj.timestamp.as_seconds_pico();
    let ns = obj.to_namespace.join(".");
    let sev = format!("{:?}", obj.severity);

    if let Some(ref human) = obj.to_human {
        format!("[{}:{}] [{}] [{}] {}", ns, obj.thread_id, sev, ts_secs, human)
    } else {
        // Fall back to machine representation
        let machine_str = trace_object_to_log_line(obj);
        format!(
            "[{}:{}] [{}] [{}] {}",
            ns, obj.thread_id, sev, ts_secs, machine_str
        )
    }
}
