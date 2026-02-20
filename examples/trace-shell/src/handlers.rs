use crate::state::SharedState;
use pallas::codec::minicbor;
use pallas::codec::utils::AnyCbor;
use pallas::network::miniprotocols::{datapoints, ekgmetrics, traceobjects};
use pallas::network::multiplexer;
use tracing::{debug, error};

/// Serve TraceObjects requests by draining from the shared trace queue.
///
/// The tracer sends Request(blocking, batch_limit) and we respond with up to
/// batch_limit trace objects from the queue. If the queue is empty we send an
/// empty response; the tracer will re-request later.
pub async fn run_traceobjects_server(channel: multiplexer::AgentChannel, state: SharedState) {
    let mut server = traceobjects::Server::new(channel);
    loop {
        match server.recv_request().await {
            Ok(Some((_blocking, n))) => {
                let objs = {
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
                        let entries: Vec<(&String, &ekgmetrics::MetricValue)> = match &req {
                            ekgmetrics::Request::GetAll | ekgmetrics::Request::GetUpdated => {
                                snapshot.iter().collect()
                            }
                            ekgmetrics::Request::GetMetrics(names) => snapshot
                                .iter()
                                .filter(|(k, _)| names.contains(k))
                                .collect(),
                        };

                        if entries.is_empty() {
                            vec![]
                        } else {
                            // Encode version tag (0u8) as AnyCbor
                            let mut ver_buf = Vec::new();
                            minicbor::encode(&0u8, &mut ver_buf).unwrap();
                            let version_any: AnyCbor = minicbor::decode(&ver_buf).unwrap();

                            // Encode metrics list as Vec<(String, MetricValue)>
                            let pairs: Vec<(String, ekgmetrics::MetricValue)> = entries
                                .into_iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            let mut metrics_buf = Vec::new();
                            minicbor::encode(&pairs, &mut metrics_buf).unwrap();
                            let metrics_any: AnyCbor = minicbor::decode(&metrics_buf).unwrap();

                            vec![version_any, metrics_any]
                        }
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
