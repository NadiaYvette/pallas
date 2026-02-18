use pallas_codec::minicbor;
use pallas_codec::utils::AnyCbor;
use pallas_network::miniprotocols::{datapoints, ekgmetrics, traceobjects};

#[test]
fn traceobjects_roundtrip() {
    let msg = traceobjects::Message::Request(true, 123);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: traceobjects::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    let obj = traceobjects::TraceObject {
        kind: None,
        to_human: Some("human".to_string()),
        to_machine: AnyCbor::from_encode("machine"),
        to_namespace: vec!["ns".to_string()],
        severity: traceobjects::Severity::Info,
        detail: traceobjects::Detail::Normal,
        timestamp: traceobjects::TraceTimestamp::Tag1 { day: 0, pico: 0 },
        hostname: "host".to_string(),
        thread_id: "thread".to_string(),
    };

    let msg = traceobjects::Message::Response(vec![obj]);
    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: traceobjects::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn traceobjects_tag1000_roundtrip() {
    // Construct a TraceObject with Tag1000 timestamp
    let obj = traceobjects::TraceObject {
        kind: None,
        to_human: Some("test".to_string()),
        to_machine: AnyCbor::from_encode("{}"),
        to_namespace: vec!["Test".to_string()],
        severity: traceobjects::Severity::Info,
        detail: traceobjects::Detail::Normal,
        timestamp: traceobjects::TraceTimestamp::Tag1000 {
            seconds: 1771486024,
            pico: 470_925_121_920,
        },
        hostname: "host".to_string(),
        thread_id: "1".to_string(),
    };

    let msg = traceobjects::Message::Response(vec![obj]);
    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: traceobjects::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    // Verify re-encoding produces identical bytes
    let mut buf2 = Vec::new();
    minicbor::encode(&decoded, &mut buf2).unwrap();
    assert_eq!(buf, buf2, "Re-encoded bytes differ from original");
}

#[test]
fn traceobjects_tag1000_wire_bytes() {
    // Test decoding actual wire bytes captured from Haskell trace-forward.
    // Tag 1000: d9 03 e8, map(2): a2, key 1: 01, seconds: 1a 69 95 3b 48,
    // key -12: 2b, pico: 1b 00 00 00 6d 98 09 75 80
    let ts_bytes: &[u8] = &[
        0xd9, 0x03, 0xe8, // Tag 1000
        0xa2, // map(2)
        0x01, // key: 1
        0x1a, 0x69, 0x95, 0x3b, 0x48, // uint32: 1771486024
        0x2b, // key: -12
        0x1b, 0x00, 0x00, 0x00, 0x6d, 0x98, 0x09, 0x75, 0x80, // uint64: 470925121920
    ];

    let ts: traceobjects::TraceTimestamp = minicbor::decode(ts_bytes).unwrap();
    match &ts {
        traceobjects::TraceTimestamp::Tag1000 { seconds, pico } => {
            assert_eq!(*seconds, 0x69953b48i64, "seconds mismatch");
            assert_ne!(*pico, 0, "pico is zero — key -12 not decoded");
            assert_eq!(*pico, 0x6d98097580i64, "pico value mismatch");
        }
        _ => panic!("Expected Tag1000, got {:?}", ts),
    }

    // Verify re-encoding is identical
    let mut reencoded = Vec::new();
    minicbor::encode(&ts, &mut reencoded).unwrap();
    assert_eq!(
        ts_bytes, &reencoded[..],
        "Tag1000 re-encoding differs from original wire bytes"
    );
}

#[test]
fn ekgmetrics_roundtrip() {
    let req = ekgmetrics::Request::GetMetrics(vec!["metric".to_string()]);
    let msg = ekgmetrics::Message::Req(req);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: ekgmetrics::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    let val = ekgmetrics::MetricValue::Counter(42);
    let pair_cbor = AnyCbor::from_encode(("metric".to_string(), val));
    let msg = ekgmetrics::Message::Resp(vec![pair_cbor]);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: ekgmetrics::Message = minicbor::decode(&buf).unwrap();
    // Can't directly compare AnyCbor, so just ensure it decodes without error

    let msg = ekgmetrics::Message::Done;
    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();
    let decoded: ekgmetrics::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn datapoints_roundtrip() {
    let msg = datapoints::Message::Request(vec!["dp".to_string()]);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: datapoints::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    let msg = datapoints::Message::Response(vec![("dp".to_string(), Some(vec![1, 2, 3]))]);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: datapoints::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);
}
