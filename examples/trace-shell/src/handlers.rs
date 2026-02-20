use crate::state::SharedState;
use pallas::codec::minicbor;
use pallas::codec::utils::AnyCbor;
use pallas::network::miniprotocols::{datapoints, ekgmetrics, traceobjects};
use pallas::network::multiplexer;
use std::time::Duration;
use tracing::{debug, error};

/// How often to poll the queue when the tracer requested blocking delivery.
const BLOCKING_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Serve TraceObjects requests by draining from the shared trace queue.
///
/// The tracer sends Request(blocking, batch_limit). When `blocking` is true the
/// server should wait until at least one object is available before responding
/// (this prevents a busy-loop between the tracer and the shell). We poll the
/// shared queue every 100 ms until data appears.
pub async fn run_traceobjects_server(channel: multiplexer::AgentChannel, state: SharedState) {
    let mut server = traceobjects::Server::new(channel);
    loop {
        match server.recv_request().await {
            Ok(Some((blocking, n))) => {
                let objs = if blocking {
                    // Wait for at least one object to appear in the queue.
                    loop {
                        let drained = {
                            let mut s = state.write().await;
                            let d = s.drain_traces(n as usize);
                            if !d.is_empty() {
                                s.traces_served += d.len() as u64;
                            }
                            d
                        };
                        if !drained.is_empty() {
                            break drained;
                        }
                        tokio::time::sleep(BLOCKING_POLL_INTERVAL).await;
                    }
                } else {
                    let mut s = state.write().await;
                    let drained = s.drain_traces(n as usize);
                    s.traces_served += drained.len() as u64;
                    drained
                };
                debug!(
                    "TraceObjects: serving {} objects (requested {})",
                    objs.len(),
                    n
                );
                if let Err(e) = server.send_response(objs).await {
                    error!("TraceObjects: send error: {:?}", e);
                    break;
                }
            }
            Ok(None) => {
                debug!("TraceObjects: Done received");
                break;
            }
            Err(e) => {
                error!("TraceObjects: recv error: {:?}", e);
                break;
            }
        }
    }
}

/// Serve EKG metrics requests by snapshotting the shared metrics store.
///
/// Wire format expected by cardano-tracer's parse_ekg_metrics:
///   raw[0] = version tag (0u8, encoded as AnyCbor)
///   raw[1] = Vec<(String, MetricValue)> encoded as AnyCbor
pub async fn run_ekg_server(channel: multiplexer::AgentChannel, state: SharedState) {
    let mut server = ekgmetrics::Server::new(channel);
    loop {
        match server.recv_request().await {
            Ok(Some(req)) => {
                debug!("EKG: request {:?}", req);
                let response: Vec<AnyCbor> = {
                    let mut s = state.write().await;
                    s.ekg_polls += 1;
                    let snapshot = &s.metrics;

                    if snapshot.is_empty() {
                        vec![]
                    } else {
                        // Filter by request type
                        let filtered: std::collections::HashMap<String, ekgmetrics::MetricValue> =
                            match &req {
                                ekgmetrics::Request::GetAll
                                | ekgmetrics::Request::GetUpdated => {
                                    snapshot.clone()
                                }
                                ekgmetrics::Request::GetMetrics(names) => snapshot
                                    .iter()
                                    .filter(|(k, _)| names.contains(k))
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect(),
                            };

                        encode_ekg_response(&filtered)
                    }
                };
                if let Err(e) = server.send_response(response).await {
                    error!("EKG: send error: {:?}", e);
                    break;
                }
            }
            Ok(None) => {
                debug!("EKG: Done received");
                break;
            }
            Err(e) => {
                error!("EKG: recv error: {:?}", e);
                break;
            }
        }
    }
}

/// Encode a set of metrics into the Vec<AnyCbor> wire format expected by
/// cardano-tracer's `parse_ekg_metrics`.
///
/// Returns `[version_any, metrics_any]` — two AnyCbor items where:
///   - version_any = 0u8
///   - metrics_any = CBOR-encoded list of (name, [variant_index, value])
///
/// Note: We cannot use minicbor's derived `Encode` for `MetricValue` here
/// because it encodes as `[idx, [value]]` (wrapping the field in an array),
/// while `parse_ekg_metrics` expects `[idx, value]` (flat two-element tuple).
pub fn encode_ekg_response(
    metrics: &std::collections::HashMap<String, ekgmetrics::MetricValue>,
) -> Vec<AnyCbor> {
    if metrics.is_empty() {
        return vec![];
    }

    // Encode version tag (0u8) as AnyCbor
    let mut ver_buf = Vec::new();
    minicbor::encode(&0u8, &mut ver_buf).unwrap();
    let version_any: AnyCbor = minicbor::decode(&ver_buf).unwrap();

    // Manually encode metrics as Vec<(String, [variant_index, value])>
    // to match the wire format parse_ekg_metrics expects.
    let mut metrics_buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut metrics_buf);
    enc.array(metrics.len() as u64).unwrap();
    for (name, value) in metrics {
        enc.array(2).unwrap(); // (name, value) tuple
        enc.str(name).unwrap();
        // Encode metric value as [variant_index, raw_value]
        enc.array(2).unwrap();
        match value {
            ekgmetrics::MetricValue::Counter(v) => {
                enc.u8(0).unwrap();
                enc.i64(*v).unwrap();
            }
            ekgmetrics::MetricValue::Gauge(v) => {
                enc.u8(1).unwrap();
                enc.i64(*v).unwrap();
            }
            ekgmetrics::MetricValue::Label(s) => {
                enc.u8(2).unwrap();
                enc.str(s).unwrap();
            }
        }
    }
    let metrics_any: AnyCbor = minicbor::decode(&metrics_buf).unwrap();

    vec![version_any, metrics_any]
}

/// Serve Datapoints requests by looking up requested names in the shared store.
pub async fn run_datapoints_server(channel: multiplexer::AgentChannel, state: SharedState) {
    let mut server = datapoints::Server::new(channel);
    loop {
        match server.recv_request().await {
            Ok(Some(requested_names)) => {
                debug!("Datapoints: request {:?}", requested_names);
                let points = {
                    let mut s = state.write().await;
                    s.dp_polls += 1;
                    let mut result = Vec::new();
                    for name in &requested_names {
                        if let Some(value) = s.datapoints.get(name) {
                            result.push((name.clone(), value.clone()));
                        }
                    }
                    result
                };
                if let Err(e) = server.send_response(points).await {
                    error!("Datapoints: send error: {:?}", e);
                    break;
                }
            }
            Ok(None) => {
                debug!("Datapoints: Done received");
                break;
            }
            Err(e) => {
                error!("Datapoints: recv error: {:?}", e);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pallas::codec::minicbor;
    use pallas::codec::utils::AnyCbor;
    use pallas::network::miniprotocols::ekgmetrics::MetricValue;
    use std::collections::HashMap;

    /// Decode EKG metrics the same way cardano-tracer's parse_ekg_metrics does.
    fn decode_like_tracer(raw: &[AnyCbor]) -> Vec<(String, String)> {
        assert!(raw.len() >= 2, "expected at least 2 AnyCbor items, got {}", raw.len());

        // raw[0] = version tag
        let _version: u8 = minicbor::decode(raw[0].raw_bytes()).unwrap();

        // raw[1] = Vec<(String, AnyCbor)>
        let items: Vec<(String, AnyCbor)> =
            minicbor::decode(raw[1].raw_bytes()).unwrap();

        let mut result = Vec::new();
        for (name, val_any) in items {
            // Each value is (variant_index: u8, inner_value: AnyCbor)
            let (idx, val_inner): (u8, AnyCbor) =
                minicbor::decode(val_any.raw_bytes()).unwrap();

            let desc = match idx {
                0 => {
                    let v: i64 = minicbor::decode(val_inner.raw_bytes()).unwrap();
                    format!("Counter({})", v)
                }
                1 => {
                    let v: i64 = minicbor::decode(val_inner.raw_bytes()).unwrap();
                    format!("Gauge({})", v)
                }
                2 => {
                    let v: String = minicbor::decode(val_inner.raw_bytes()).unwrap();
                    format!("Label({})", v)
                }
                _ => panic!("unknown metric variant index {}", idx),
            };
            result.push((name, desc));
        }
        result
    }

    #[test]
    fn ekg_gauge_round_trip() {
        let mut metrics = HashMap::new();
        metrics.insert("node.peers".to_string(), MetricValue::Gauge(7));

        let raw = encode_ekg_response(&metrics);
        let decoded = decode_like_tracer(&raw);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, "node.peers");
        assert_eq!(decoded[0].1, "Gauge(7)");
    }

    #[test]
    fn ekg_counter_round_trip() {
        let mut metrics = HashMap::new();
        metrics.insert("rts.gc.bytes_allocated".to_string(), MetricValue::Counter(1024));

        let raw = encode_ekg_response(&metrics);
        let decoded = decode_like_tracer(&raw);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, "rts.gc.bytes_allocated");
        assert_eq!(decoded[0].1, "Counter(1024)");
    }

    #[test]
    fn ekg_label_round_trip() {
        let mut metrics = HashMap::new();
        metrics.insert("node.version".to_string(), MetricValue::Label("10.1.0".to_string()));

        let raw = encode_ekg_response(&metrics);
        let decoded = decode_like_tracer(&raw);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, "node.version");
        assert_eq!(decoded[0].1, "Label(10.1.0)");
    }

    #[test]
    fn ekg_multiple_metrics_round_trip() {
        let mut metrics = HashMap::new();
        metrics.insert("node.peers".to_string(), MetricValue::Gauge(7));
        metrics.insert("rts.gc.bytes_allocated".to_string(), MetricValue::Counter(1024));
        metrics.insert("node.version".to_string(), MetricValue::Label("10.1.0".to_string()));

        let raw = encode_ekg_response(&metrics);
        let decoded = decode_like_tracer(&raw);

        assert_eq!(decoded.len(), 3);

        // Sort by name for deterministic comparison
        let mut decoded = decoded;
        decoded.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(decoded[0], ("node.peers".to_string(), "Gauge(7)".to_string()));
        assert_eq!(decoded[1], ("node.version".to_string(), "Label(10.1.0)".to_string()));
        assert_eq!(decoded[2], ("rts.gc.bytes_allocated".to_string(), "Counter(1024)".to_string()));
    }

    #[test]
    fn ekg_empty_metrics() {
        let metrics = HashMap::new();
        let raw = encode_ekg_response(&metrics);
        assert!(raw.is_empty());
    }

    fn hex(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    }

    #[test]
    fn ekg_hex_dump() {
        // Print CBOR hex for manual inspection
        let mut metrics = HashMap::new();
        metrics.insert("node.peers".to_string(), MetricValue::Gauge(7));

        let pairs: Vec<(String, MetricValue)> = metrics
            .into_iter()
            .collect();
        let mut buf = Vec::new();
        minicbor::encode(&pairs, &mut buf).unwrap();
        eprintln!("CBOR hex for [(\"node.peers\", Gauge(7))]: {}", hex(&buf));

        // Also encode just the MetricValue
        let mut mv_buf = Vec::new();
        minicbor::encode(&MetricValue::Gauge(7), &mut mv_buf).unwrap();
        eprintln!("CBOR hex for Gauge(7): {}", hex(&mv_buf));
    }
}
