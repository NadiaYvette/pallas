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

#[cfg(test)]
mod tests {
    use super::*;
    use pallas::codec::minicbor::{self, Encoder};
    use pallas::codec::utils::AnyCbor;
    use pallas::network::miniprotocols::traceobjects::{
        Detail, Severity, TraceTimestamp,
    };

    /// Helper: encode a value to CBOR bytes via a closure.
    fn cbor_bytes(f: impl FnOnce(&mut Encoder<&mut Vec<u8>>)) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut e = Encoder::new(&mut buf);
        f(&mut e);
        buf
    }

    /// Wrap raw CBOR bytes into an AnyCbor by decoding them.
    fn any_cbor_from_raw(bytes: &[u8]) -> AnyCbor {
        minicbor::decode(bytes).unwrap()
    }

    /// Helper: construct a TraceObject with to_machine set from raw CBOR bytes.
    fn make_trace_object(to_machine_bytes: Vec<u8>) -> TraceObject {
        TraceObject {
            kind: None,
            to_human: None,
            to_machine: any_cbor_from_raw(&to_machine_bytes),
            to_namespace: vec!["Test".to_string(), "Namespace".to_string()],
            severity: Severity::Info,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1700000000,
                pico: 123456789,
            },
            hostname: "testhost".to_string(),
            thread_id: "42".to_string(),
        }
    }

    // --- decode_cbor_to_json tests ---

    #[test]
    fn test_decode_null() {
        let bytes = cbor_bytes(|e| { e.null().unwrap(); });
        let mut d = Decoder::new(&bytes);
        let v = decode_cbor_to_json(&mut d).unwrap();
        assert!(v.is_null());
    }

    #[test]
    fn test_decode_bool() {
        let bytes = cbor_bytes(|e| { e.bool(true).unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(decode_cbor_to_json(&mut d).unwrap(), serde_json::Value::Bool(true));

        let bytes = cbor_bytes(|e| { e.bool(false).unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(decode_cbor_to_json(&mut d).unwrap(), serde_json::Value::Bool(false));
    }

    #[test]
    fn test_decode_unsigned_int() {
        let bytes = cbor_bytes(|e| { e.u64(42).unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(decode_cbor_to_json(&mut d).unwrap(), serde_json::json!(42));
    }

    #[test]
    fn test_decode_negative_int() {
        let bytes = cbor_bytes(|e| { e.i64(-7).unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(decode_cbor_to_json(&mut d).unwrap(), serde_json::json!(-7));
    }

    #[test]
    fn test_decode_float() {
        let bytes = cbor_bytes(|e| { e.f64(3.14).unwrap(); });
        let mut d = Decoder::new(&bytes);
        let v = decode_cbor_to_json(&mut d).unwrap();
        assert!((v.as_f64().unwrap() - 3.14).abs() < 1e-10);
    }

    #[test]
    fn test_decode_string() {
        let bytes = cbor_bytes(|e| { e.str("hello world").unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::Value::String("hello world".to_string())
        );
    }

    #[test]
    fn test_decode_bytes() {
        let bytes = cbor_bytes(|e| { e.bytes(&[0xde, 0xad, 0xbe, 0xef]).unwrap(); });
        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::Value::String("deadbeef".to_string())
        );
    }

    #[test]
    fn test_decode_array() {
        let bytes = cbor_bytes(|e| {
            e.array(3).unwrap();
            e.u32(1).unwrap();
            e.u32(2).unwrap();
            e.u32(3).unwrap();
        });
        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::json!([1, 2, 3])
        );
    }

    #[test]
    fn test_decode_map() {
        let bytes = cbor_bytes(|e| {
            e.map(2).unwrap();
            e.str("a").unwrap();
            e.u32(1).unwrap();
            e.str("b").unwrap();
            e.u32(2).unwrap();
        });
        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::json!({"a": 1, "b": 2})
        );
    }

    #[test]
    fn test_decode_nested() {
        let bytes = cbor_bytes(|e| {
            e.map(1).unwrap();
            e.str("arr").unwrap();
            e.array(2).unwrap();
            e.str("x").unwrap();
            e.map(1).unwrap();
            e.str("inner").unwrap();
            e.bool(true).unwrap();
        });
        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::json!({"arr": ["x", {"inner": true}]})
        );
    }

    #[test]
    fn test_decode_tag_stripped() {
        // CBOR tag 1 wrapping an integer
        let bytes = cbor_bytes(|e| {
            e.tag(minicbor::data::Tag::new(1)).unwrap();
            e.u64(1700000000).unwrap();
        });
        let mut d = Decoder::new(&bytes);
        assert_eq!(decode_cbor_to_json(&mut d).unwrap(), serde_json::json!(1700000000));
    }

    // --- trace_object_to_log_line tests ---

    #[test]
    fn test_trace_object_to_log_line_string_passthrough() {
        // Critical test: when to_machine is a CBOR text string containing a JSON
        // log line, it must be returned verbatim (this is the path trace-dispatcher uses).
        let json_line = r#"{"at":"2024-01-15T14:30:00.000Z","ns":"Test.Namespace","data":{"kind":"Message1","mid":"1","workload":"42"},"sev":"Info","thread":"42","host":"testhost"}"#;
        let to_machine_bytes = cbor_bytes(|e| { e.str(json_line).unwrap(); });

        // AnyCbor wraps raw bytes — we need to construct it correctly
        let obj = TraceObject {
            kind: None,
            to_human: None,
            to_machine: any_cbor_from_raw(&to_machine_bytes),
            to_namespace: vec!["Test".to_string(), "Namespace".to_string()],
            severity: Severity::Info,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1700000000,
                pico: 0,
            },
            hostname: "testhost".to_string(),
            thread_id: "42".to_string(),
        };
        let result = trace_object_to_log_line(&obj);
        assert_eq!(result, json_line);
    }

    #[test]
    fn test_trace_object_to_log_line_non_string_envelope() {
        // When to_machine is a CBOR map (not a string), wrap in JSON envelope.
        let to_machine_bytes = cbor_bytes(|e| {
            e.map(2).unwrap();
            e.str("kind").unwrap();
            e.str("TestKind").unwrap();
            e.str("value").unwrap();
            e.u32(99).unwrap();
        });

        let obj = make_trace_object(to_machine_bytes);
        let result = trace_object_to_log_line(&obj);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Must have envelope fields
        assert!(parsed.get("at").is_some());
        assert!(parsed.get("ns").is_some());
        assert!(parsed.get("sev").is_some());
        assert!(parsed.get("thread").is_some());
        assert!(parsed.get("host").is_some());
        assert!(parsed.get("data").is_some());

        // data should be the decoded map
        let data = parsed.get("data").unwrap();
        assert_eq!(data.get("kind").unwrap(), "TestKind");
        assert_eq!(data.get("value").unwrap(), 99);
    }

    #[test]
    fn test_trace_object_to_log_line_non_string_cbor_integer() {
        // When to_machine is a CBOR integer (not a string), it should be wrapped
        // in an envelope with the decoded value.
        let to_machine_bytes = cbor_bytes(|e| { e.u32(42).unwrap(); });
        let obj = TraceObject {
            kind: None,
            to_human: None,
            to_machine: any_cbor_from_raw(&to_machine_bytes),
            to_namespace: vec!["Test".to_string()],
            severity: Severity::Error,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1700000000,
                pico: 0,
            },
            hostname: "testhost".to_string(),
            thread_id: "1".to_string(),
        };
        let result = trace_object_to_log_line(&obj);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // data should be the integer 42
        assert_eq!(parsed["data"], serde_json::json!(42));
        // Should have envelope fields
        assert!(parsed.get("at").is_some());
        assert!(parsed.get("sev").is_some());
        assert!(parsed.get("host").is_some());
    }

    #[test]
    fn test_trace_object_to_human_line_with_human() {
        let obj = TraceObject {
            kind: None,
            to_human: Some("Human readable message".to_string()),
            to_machine: any_cbor_from_raw(&cbor_bytes(|e| { e.str("dummy").unwrap(); })),
            to_namespace: vec!["Cardano".to_string(), "Node".to_string()],
            severity: Severity::Warning,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1700000000,
                pico: 0,
            },
            hostname: "testhost".to_string(),
            thread_id: "7".to_string(),
        };
        let result = trace_object_to_human_line(&obj);
        assert!(result.contains("[Cardano.Node:7]"));
        assert!(result.contains("[Warning]"));
        assert!(result.contains("Human readable message"));
    }

    #[test]
    fn test_trace_object_to_human_line_without_human() {
        let json_line = r#"{"kind":"Test"}"#;
        let to_machine_bytes = cbor_bytes(|e| { e.str(json_line).unwrap(); });
        let obj = TraceObject {
            kind: None,
            to_human: None,
            to_machine: any_cbor_from_raw(&to_machine_bytes),
            to_namespace: vec!["Test".to_string()],
            severity: Severity::Debug,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1700000000,
                pico: 0,
            },
            hostname: "testhost".to_string(),
            thread_id: "1".to_string(),
        };
        let result = trace_object_to_human_line(&obj);
        assert!(result.contains("[Test:1]"));
        assert!(result.contains("[Debug]"));
        // Should fall back to machine representation (the JSON string)
        assert!(result.contains(json_line));
    }

    #[test]
    fn test_stress_test_message_format() {
        // Verify the exact JSON format that the Haskell ForwardingStressTest expects.
        // Message1: {"kind":"Message1","mid":"<id>","workload":"<int>"}
        let msg_json = r#"{"at":"2024-01-15T14:30:00.000000000000Z","ns":["Test"],"data":{"kind":"Message1","mid":"1","workload":"42"},"sev":"Info","thread":"1","host":"test"}"#;
        let to_machine_bytes = cbor_bytes(|e| { e.str(msg_json).unwrap(); });

        let obj = TraceObject {
            kind: None,
            to_human: Some("Message1 1 42".to_string()),
            to_machine: any_cbor_from_raw(&to_machine_bytes),
            to_namespace: vec!["Test".to_string()],
            severity: Severity::Info,
            detail: Detail::Normal,
            timestamp: TraceTimestamp::Tag1000 {
                seconds: 1705312200,
                pico: 0,
            },
            hostname: "test".to_string(),
            thread_id: "1".to_string(),
        };

        // ForMachine: should return the CBOR string content verbatim
        let machine_line = trace_object_to_log_line(&obj);
        assert_eq!(machine_line, msg_json);

        // Verify the data field can be parsed back
        let parsed: serde_json::Value = serde_json::from_str(&machine_line).unwrap();
        let data = parsed.get("data").unwrap();
        assert_eq!(data.get("kind").unwrap(), "Message1");
        assert_eq!(data.get("mid").unwrap(), "1");
        assert_eq!(data.get("workload").unwrap(), "42");
    }

    #[test]
    fn test_decode_indefinite_array() {
        // Build indefinite-length array manually: 0x9f [items] 0xff
        let mut bytes = vec![0x9f]; // begin indefinite array
        // Add two unsigned integers: 1 (0x01) and 2 (0x02)
        bytes.push(0x01);
        bytes.push(0x02);
        bytes.push(0xff); // break

        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::json!([1, 2])
        );
    }

    #[test]
    fn test_decode_indefinite_map() {
        // Build indefinite-length map manually: 0xbf [key value pairs] 0xff
        let mut bytes = vec![0xbf]; // begin indefinite map
        // key: "x" (0x61 0x78), value: 1 (0x01)
        bytes.extend_from_slice(&[0x61, 0x78]); // text(1) "x"
        bytes.push(0x01); // unsigned(1)
        bytes.push(0xff); // break

        let mut d = Decoder::new(&bytes);
        assert_eq!(
            decode_cbor_to_json(&mut d).unwrap(),
            serde_json::json!({"x": 1})
        );
    }

    #[test]
    fn test_decode_map_with_non_string_keys() {
        // Map with integer keys — they should be converted to string representation.
        let bytes = cbor_bytes(|e| {
            e.map(2).unwrap();
            e.u32(1).unwrap();
            e.str("one").unwrap();
            e.u32(2).unwrap();
            e.str("two").unwrap();
        });
        let mut d = Decoder::new(&bytes);
        let v = decode_cbor_to_json(&mut d).unwrap();
        assert_eq!(v.get("1").unwrap(), "one");
        assert_eq!(v.get("2").unwrap(), "two");
    }
}
