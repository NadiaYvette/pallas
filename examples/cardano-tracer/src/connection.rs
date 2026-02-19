use crate::config::TracerConfig;
use crate::logging::SharedLogManager;
use crate::node::{MetricEntry, MetricValue, NodeId, SharedRegistry};
use pallas::codec::minicbor::{self, decode, Decode, Decoder, Encode, Encoder};
use pallas::codec::utils::AnyCbor;
use pallas::network::miniprotocols::handshake::n2c;
use pallas::network::miniprotocols::traceobjects::Message;
use pallas::network::miniprotocols::{
    datapoints, ekgmetrics, handshake, PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_EKG_METRICS,
    PROTOCOL_TFWP_TRACE_OBJECTS,
};
use pallas::network::multiplexer::{Bearer, ChannelBuffer, Plexer};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

/// Custom CBOR encoding for a TraceObjects Request message.
///
/// Encodes as `[1, blocking, [0, n]]` to match the Haskell trace-forward
/// wire format. This is necessary because the tracer acts as the protocol
/// "requester" (sends Request, receives Response) even though it's the
/// Mux Responder.
struct CustomRequest(bool, u16);

impl Encode<()> for CustomRequest {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.u16(1)?; // Tag 1 = Request
        e.bool(self.0)?; // blocking
        e.array(2)?; // [0, n]
        e.u16(0)?;
        e.u16(self.1)?; // batch size
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for CustomRequest {
    fn decode(_d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        Err(decode::Error::message("CustomRequest decode not supported"))
    }
}

/// Handle a single node connection: handshake, then run all 3 protocols
/// concurrently until the connection closes or shutdown is requested.
pub async fn handle_connection(
    bearer: Bearer,
    node_id: NodeId,
    config: Arc<TracerConfig>,
    registry: SharedRegistry,
    log_manager: SharedLogManager,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut plexer = Plexer::new(bearer);

    let to_channel = plexer.subscribe_server(PROTOCOL_TFWP_TRACE_OBJECTS);
    let ekg_channel = plexer.subscribe_server(PROTOCOL_TFWP_EKG_METRICS);
    let dp_channel = plexer.subscribe_server(PROTOCOL_TFWP_DATAPOINTS);
    let hs_channel = plexer.subscribe_server(0);
    let _plexer_handle = plexer.spawn();

    // 1. Handshake (server side: receive proposed versions, accept one).
    //
    // Accept trace-forward ForwardingV_1 (1) and ForwardingV_2 (2) with the
    // configured network magic, matching the Haskell cardano-tracer behavior.
    // The handshake will refuse connections with mismatched magic or
    // unsupported version numbers.
    let mut hs_server = handshake::Server::<n2c::VersionData>::new(hs_channel);
    let magic = config.network_magic as u64;
    let supported = n2c::VersionTable {
        values: [
            (1, n2c::VersionData::new(magic, None)), // ForwardingV_1
            (2, n2c::VersionData::new(magic, None)), // ForwardingV_2
        ]
        .into_iter()
        .collect(),
    };
    match hs_server.handshake(supported).await? {
        Some((v, _)) => info!("Node {:?}: accepted version {}", node_id, v),
        None => return Err("Handshake refused: no compatible version or magic mismatch".into()),
    }

    // 2. Register node in registry (using address-derived name initially).
    {
        let mut reg = registry.write().await;
        reg.register_node(node_id.clone(), node_id.0.clone());
    }

    // 3. Spawn EKG metrics handler (tracked for cleanup).
    let ekg_handle = {
        let ekg_registry = registry.clone();
        let ekg_node_id = node_id.clone();
        let ekg_freq = config.ekg_freq_secs();
        let ekg_shutdown = shutdown.clone();
        tokio::spawn(async move {
            run_ekg_handler(ekg_channel, ekg_node_id, ekg_registry, ekg_freq, ekg_shutdown).await;
        })
    };

    // 4. Spawn Datapoints handler (tracked for cleanup).
    let dp_handle = {
        let dp_registry = registry.clone();
        let dp_node_id = node_id.clone();
        let dp_shutdown = shutdown.clone();
        tokio::spawn(async move {
            run_datapoints_handler(dp_channel, dp_node_id, dp_registry, dp_shutdown).await;
        })
    };

    // 5. Run TraceObjects loop (main connection loop).
    let result = run_trace_objects_loop(
        to_channel,
        &node_id,
        &config,
        &registry,
        &log_manager,
        &shutdown,
    )
    .await;

    // 6. Abort subsidiary protocol tasks on disconnect.
    ekg_handle.abort();
    dp_handle.abort();

    // 7. Deregister node on disconnect.
    {
        let mut reg = registry.write().await;
        reg.deregister_node(&node_id);
    }
    info!("Node {:?}: disconnected", node_id);

    result
}

/// Request TraceObjects in a loop, writing each batch to the log manager.
async fn run_trace_objects_loop(
    channel: pallas::network::multiplexer::AgentChannel,
    node_id: &NodeId,
    config: &TracerConfig,
    registry: &SharedRegistry,
    log_manager: &SharedLogManager,
    shutdown: &CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buffer = ChannelBuffer::new(channel);
    let batch_size = config.request_num();

    loop {
        tokio::select! {
            result = async {
                // Send Request [1, blocking=true, [0, batch_size]]
                let request = CustomRequest(true, batch_size);
                buffer
                    .send_msg_chunks(&request)
                    .await
                    .map_err(|e| format!("TraceObjects send error: {:?}", e))?;

                // Receive Response
                let msg: Message = buffer
                    .recv_full_msg()
                    .await
                    .map_err(|e| format!("TraceObjects recv error: {:?}", e))?;

                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(msg)
            } => {
                match result? {
                    Message::Response(objs) => {
                        if !objs.is_empty() {
                            let node_name = {
                                let reg = registry.read().await;
                                reg.get_node_name(node_id)
                                    .unwrap_or_else(|| node_id.0.clone())
                            };
                            let mut mgr = log_manager.lock().await;
                            mgr.write_trace_objects(&node_name, &objs);
                        }
                    }
                    Message::Done => {
                        info!("Node {:?}: TraceObjects Done", node_id);
                        break;
                    }
                    Message::Request(..) => {
                        warn!("Node {:?}: unexpected Request message", node_id);
                    }
                }
            }
            _ = shutdown.cancelled() => {
                info!("Node {:?}: shutdown requested", node_id);
                break;
            }
        }
    }

    Ok(())
}

/// Poll EKG metrics periodically and update the node registry.
async fn run_ekg_handler(
    channel: pallas::network::multiplexer::AgentChannel,
    node_id: NodeId,
    registry: SharedRegistry,
    freq_secs: f64,
    shutdown: CancellationToken,
) {
    let mut client = ekgmetrics::Client::new(channel);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs_f64(freq_secs)) => {}
            _ = shutdown.cancelled() => break,
        }

        if let Err(e) = client.send_request(ekgmetrics::Request::GetAll).await {
            error!("Node {:?}: EKG request error: {:?}", node_id, e);
            break;
        }

        match client.recv_response().await {
            Ok(raw_metrics) => {
                let entries = parse_ekg_metrics(&raw_metrics);
                if !entries.is_empty() {
                    let mut reg = registry.write().await;
                    reg.update_metrics(&node_id, entries);
                }
            }
            Err(e) => {
                error!("Node {:?}: EKG response error: {:?}", node_id, e);
                break;
            }
        }
    }
}

/// Parse EKG metrics from the raw AnyCbor response.
///
/// The wire format is `[version_tag, metrics_list]` where `metrics_list`
/// is encoded as `Vec<(String, AnyCbor)>` and each value is `(index, AnyCbor)`
/// with index 0=Counter, 1=Gauge, 2=Label.
fn parse_ekg_metrics(raw: &[AnyCbor]) -> Vec<MetricEntry> {
    if raw.len() < 2 {
        return Vec::new();
    }

    let metrics_cbor = &raw[1];
    let items: Vec<(String, AnyCbor)> =
        match pallas::codec::minicbor::decode(metrics_cbor.raw_bytes()) {
            Ok(items) => items,
            Err(e) => {
                error!("Failed to decode EKG metrics: {:?}", e);
                return Vec::new();
            }
        };

    let mut entries = Vec::with_capacity(items.len());
    for (name, val_any) in items {
        let mv: Result<(u8, AnyCbor), _> =
            pallas::codec::minicbor::decode(val_any.raw_bytes());

        match mv {
            Ok((idx, val_inner)) => {
                let value = match idx {
                    0 => {
                        // Counter
                        match pallas::codec::minicbor::decode(val_inner.raw_bytes()) {
                            Ok(v) => MetricValue::Counter(v),
                            Err(e) => {
                                warn!("Failed to decode Counter value for {}: {:?}", name, e);
                                MetricValue::Counter(0)
                            }
                        }
                    }
                    1 => {
                        // Gauge
                        match pallas::codec::minicbor::decode(val_inner.raw_bytes()) {
                            Ok(v) => MetricValue::Gauge(v),
                            Err(e) => {
                                warn!("Failed to decode Gauge value for {}: {:?}", name, e);
                                MetricValue::Gauge(0)
                            }
                        }
                    }
                    2 => {
                        // Label
                        match pallas::codec::minicbor::decode(val_inner.raw_bytes()) {
                            Ok(v) => MetricValue::Label(v),
                            Err(e) => {
                                warn!("Failed to decode Label value for {}: {:?}", name, e);
                                MetricValue::Label(String::new())
                            }
                        }
                    }
                    _ => continue,
                };
                entries.push(MetricEntry { name, value });
            }
            Err(e) => {
                error!(
                    "Failed to decode metric {}: {:?}. Raw: {}",
                    name,
                    e,
                    hex::encode(val_any.raw_bytes())
                );
            }
        }
    }

    entries
}

/// Poll Datapoints periodically; use NodeInfo to update the node name.
async fn run_datapoints_handler(
    channel: pallas::network::multiplexer::AgentChannel,
    node_id: NodeId,
    registry: SharedRegistry,
    shutdown: CancellationToken,
) {
    let mut client = datapoints::Client::new(channel);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown.cancelled() => break,
        }

        // Request NodeInfo to potentially learn the node's name.
        if let Err(e) = client
            .send_request(vec!["NodeInfo".to_string()])
            .await
        {
            error!("Node {:?}: Datapoints request error: {:?}", node_id, e);
            break;
        }

        match client.recv_response().await {
            Ok(points) => {
                for (name, val_opt) in &points {
                    if name == "NodeInfo" {
                        if let Some(bytes) = val_opt {
                            if let Ok(json_str) = String::from_utf8(bytes.clone()) {
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(&json_str)
                                {
                                    if let Some(node_name) =
                                        parsed.get("nodeName").and_then(|v| v.as_str())
                                    {
                                        let mut reg = registry.write().await;
                                        reg.update_node_name(
                                            &node_id,
                                            node_name.to_string(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Node {:?}: Datapoints response error: {:?}", node_id, e);
                break;
            }
        }
    }
}
