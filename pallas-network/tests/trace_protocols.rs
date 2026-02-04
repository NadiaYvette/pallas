use pallas_codec::minicbor;
use pallas_network::miniprotocols::{datapoints, ekgmetrics, traceobjects};
#[test]
fn traceobjects_roundtrip() {
    let msg = traceobjects::Message::Request(true, 123);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: traceobjects::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    let obj = traceobjects::TraceObject {
        to_human: Some("human".to_string()),
        to_machine: "machine".to_string(),
        to_namespace: vec!["ns".to_string()],
        severity: traceobjects::Severity::Info,
        detail: traceobjects::Detail::Normal,
        timestamp: traceobjects::TraceTimestamp { day: 0, pico: 0 },
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
fn ekgmetrics_roundtrip() {
    let req = ekgmetrics::Request::GetMetrics(vec!["metric".to_string()]);
    let msg = ekgmetrics::Message::Req(req);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: ekgmetrics::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

    let val = ekgmetrics::MetricValue::Counter(42);
    let msg = ekgmetrics::Message::Resp(vec![("metric".to_string(), val)]);

    let mut buf = Vec::new();
    minicbor::encode(&msg, &mut buf).unwrap();

    let decoded: ekgmetrics::Message = minicbor::decode(&buf).unwrap();
    assert_eq!(msg, decoded);

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
