use clap::Parser;
use pallas::codec::minicbor::{encode, Encode, Encoder};
use pallas::network::miniprotocols::handshake::n2c;
use pallas::network::{
    miniprotocols::{
        datapoints, ekgmetrics, handshake, traceobjects, PROTOCOL_TFWP_DATAPOINTS,
        PROTOCOL_TFWP_EKG_METRICS, PROTOCOL_TFWP_TRACE_OBJECTS,
    },
    multiplexer::{Bearer, Plexer},
};
use std::path::PathBuf;
use tokio::net::UnixListener;
use tracing::{error, info};

#[derive(Clone, Debug)]
struct NodeInfo {
    pub node_version: String,
    pub node_commit: String,
    pub node_start_time: String,
    pub protocol: String,
}

impl<C> Encode<C> for NodeInfo {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), encode::Error<W::Error>> {
        e.map(4)?;
        e.str("nodeVersion")?.str(&self.node_version)?;
        e.str("nodeCommit")?.str(&self.node_commit)?;
        e.str("nodeStartTime")?.str(&self.node_start_time)?;
        e.str("protocol")?.str(&self.protocol)?;
        Ok(())
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the node socket
    #[arg(short, long)]
    socket: PathBuf,

    /// Protocol magic (default: mainnet)
    #[arg(long, default_value_t = 764824073)]
    magic: u64,

    /// Mode: server (default) or client.
    /// server: Connects to a Tracer (acts as node-side proxy or similar, or a
    /// Tracer client).         Mux Initiator.
    ///         TraceObjects: Client (Initiator).
    ///         EKG: Client (Initiator).
    ///         Datapoints: Client (Initiator).
    /// client: Listens for a Node (acts as a Tracer).
    ///         Mux Responder.
    ///         TraceObjects: Server (Responder).
    ///         EKG: Client (Initiator) - Wait, if we are Mux Responder, we need
    /// to check if we can be MP Client.         Datapoints: Client
    /// (Initiator).
    #[arg(long, default_value = "server")]
    mode: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .finish(),
    )?;

    let args = Args::parse();

    match args.mode.as_str() {
        "server" => {
            // "Server" mode (Original): Connects to Tracer.
            // Acts as Initiator for Mux.
            info!("Connecting to socket: {:?}", args.socket);
            let bearer = Bearer::connect_unix(&args.socket).await?;
            let mut plexer = Plexer::new(bearer);

            let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);
            let ekg_channel = plexer.subscribe_client(PROTOCOL_TFWP_EKG_METRICS);
            let dp_channel = plexer.subscribe_client(PROTOCOL_TFWP_DATAPOINTS);
            let hs_channel = plexer.subscribe_client(0);

            let plexer_handle = plexer.spawn();

            let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
            let versions = n2c::VersionTable::v1_and_above(args.magic);

            info!("Starting Handshake...");
            let handshake_result = hs_client.handshake(versions).await?;
            if let handshake::Confirmation::Rejected(reason) = handshake_result {
                error!("Handshake rejected: {:?}", reason);
                return Ok(());
            }
            info!("Handshake accepted");

            info!("Starting in SERVER mode (Connected to Tracer)");

            // In this mode, we originally acted as:
            // traceobjects::Server
            // ekgmetrics::Server
            // datapoints::Server
            // BUT we used `subscribe_client`. This means we are Mux Initiator.
            // If we are Mux Initiator, we send on ID.
            // MP Server reads from ID (Initiator sends Requests) and writes to ID (Server
            // sends Responses). Wait.
            // MP Client (Initiator) sends Requests.
            // MP Server (Responder) sends Responses.
            // Mux Initiator (A) sends on ID. Mux Responder (B) reads from ID.
            // Mux Initiator (A) reads from ID^0x8000. Mux Responder (B) sends on ID^0x8000.

            // If we are Mux Initiator (A):
            // We write to ID. We read from ID^0x8000.

            // If we run MP Server:
            // MP Server RECVs Requests.
            // MP Server SENDs Responses.

            // If MP Client is on B:
            // MP Client sends Request (on ID^0x8000).
            // A reads Request from ID^0x8000. Correct.
            // A sends Response on ID. Correct.

            // So Mux Initiator can run MP Server.

            // Original code ran:
            // traceobjects::Server (Responder)
            // ekgmetrics::Server (Responder)
            // datapoints::Server (Responder)

            // This matches "Node connecting to Tracer" where Node acts as Responder for
            // these protocols? BUT Node sends Traces. Node is TraceObjects
            // Client? Actually, in `trace-forward`:
            // TraceObjects: Client (sends traces) -> Server (receives traces).
            // If Node runs TraceObjects Client, it sends Request.
            // If Node is Mux Initiator, it writes to ID.
            // If Tracer is Mux Responder, it reads from ID.

            // So if Node is Mux Initiator and TraceObjects Client:
            // Node sends Trace (Request) on ID.
            // Tracer reads Trace (Request) from ID.

            // If Original Code (Server Mode) ran TraceObjects Server:
            // It expects to RECV Requests.
            // Since it is Mux Initiator, it reads from ID^0x8000.
            // So Peer (Tracer) must write to ID^0x8000.
            // Peer (Tracer) is Mux Responder. It writes to ID^0x8000.
            // So Peer (Tracer) must be sending Requests (MP Client).

            // This implies original "Server Mode" expected Tracer to be TraceObjects Client
            // (sender of traces). This is BACKWARDS for `cardano-tracer`.
            // `cardano-tracer` receives traces.

            // HOWEVER, my test with `cardano-tracer` PASSED in Server Mode.
            // Why?
            // `traceobjects::Server::new(to_channel)`.
            // `server.recv_request()`.
            // Maybe `cardano-tracer` acts as Client for TraceObjects?
            // No, `cardano-tracer` logs say "ForwarderMode: Initiator".
            // "Initiator" in `trace-forward` lib usually means "Sender of Traces".
            // So `cardano-tracer` was configured as Initiator?
            // In `cardano-tracer-test-ext.hs`: `initForwarderMode = Initiator`.
            // So the test configures the TRACER to be the Initiator (Sender)?
            // That's for the "Forwarding Stress Test".

            // But against the LIVE `cardano-tracer`:
            // I ran `trace-miniprotocols --mode server` against `tracer.socket`.
            // It worked.
            // I received `NodeInfo` request.
            // `Datapoints`: Req ["NodeInfo"].

            // So `cardano-tracer` (Live) sent a Request for NodeInfo.
            // So `cardano-tracer` acted as Datapoints Client.
            // My code acted as Datapoints Server.
            // My code was Mux Initiator.
            // Mux Initiator reads from ID^0x8000.
            // Mux Responder (Tracer) writes to ID^0x8000.
            // So Tracer (Responder) sent Request (Client).
            // This works.

            // So for "Server Mode" (acting as Node towards Tracer):
            // We connect (Mux Init).
            // We run MP Servers (Resp).
            // This seems correct for EKG/Datapoints.

            // What about TraceObjects?
            // In "Server Mode" I implemented `traceobjects::Server`.
            // It waited for requests.
            // `trace_miniprotocols: TraceObjects: Req blocking=true, n=100`
            // So `cardano-tracer` sent a Request for Traces.
            // So `cardano-tracer` acted as TraceObjects Client (Requester)?
            // Wait. `TraceObjects` protocol:
            // Msg::Acquire -> Client sends.
            // Msg::Data -> Server sends?
            // No, usually Client sends Data?
            // Let's check `pallas` `traceobjects` protocol definition.

            // If `cardano-tracer` requests traces, then it pulls them?
            // This matches `trace-forward` "pull" model?

            let to_task = tokio::spawn(async move {
                let mut server = traceobjects::Server::new(to_channel);
                loop {
                    match server.recv_request().await {
                        Ok(Some((blocking, n))) => {
                            info!("TraceObjects: Req blocking={}, n={}", blocking, n);
                            // Dummy response
                            let timestamp = traceobjects::TraceTimestamp { day: 0, pico: 0 };
                            let obj = traceobjects::TraceObject {
                                to_human: Some("Dummy Trace from Rust".to_string()),
                                to_machine: "{}".to_string(),
                                to_namespace: vec!["Rust".to_string(), "Trace".to_string()],
                                severity: traceobjects::Severity::Info,
                                detail: traceobjects::Detail::Normal,
                                timestamp,
                                hostname: "rust-client".to_string(),
                                thread_id: "0".to_string(),
                            };
                            info!("TraceObjects: Responding with {:?}", obj);
                            server.send_response(vec![obj]).await.ok();
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error!("TraceObjects Error: {:?}", e);
                            break;
                        }
                    }
                }
            });

            let ekg_task = tokio::spawn(async move {
                let mut server = ekgmetrics::Server::new(ekg_channel);
                loop {
                    match server.recv_request().await {
                        Ok(Some(req)) => {
                            let mut points = vec![];
                            info!("EKG: Req {:?}", req);
                            // let metrics = vec![("rust.metric".to_string(),
                            // ekgmetrics::MetricValue::Gauge(42))];
                            info!("EKG: Responding with {:?}", points);
                            server.send_response(points).await.ok();
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error!("EKG Error: {:?}", e);
                            break;
                        }
                    }
                }
            });

            let dp_task = tokio::spawn(async move {
                let mut server = datapoints::Server::new(dp_channel);
                loop {
                    match server.recv_request().await {
                        Ok(Some(req)) => {
                            info!("Datapoints: Req {:?}", req);
                            let mut points = vec![];
                            if req.contains(&"NodeInfo".to_string()) {
                                info!("Responding to NodeInfo request");
                                let node_info = NodeInfo {
                                    node_version: "1.35.7".to_string(),
                                    node_commit: "00000000".to_string(),
                                    node_start_time: "2023-01-01 00:00:00 UTC".to_string(),
                                    protocol: "Shelley".to_string(),
                                };
                                let mut buf = Vec::new();
                                let mut encoder = Encoder::new(&mut buf);
                                if let Ok(_) = node_info.encode(&mut encoder, &mut ()) {
                                    points.push(("NodeInfo".to_string(), Some(buf)));
                                } else {
                                    error!("Failed to encode NodeInfo");
                                }
                            }
                            server.send_response(points).await.ok();
                        }
                        Ok(None) => break,
                        Err(e) => {
                            error!("Datapoints Error: {:?}", e);
                            break;
                        }
                    }
                }
            });

            let _ = tokio::join!(to_task, ekg_task, dp_task);
            plexer_handle.abort().await;
        }
        "client" => {
            // "Client" mode (New): Listens for Node. Acts as Tracer.
            // Acts as Responder for Mux.
            info!("Binding to socket: {:?}", args.socket);
            if args.socket.exists() {
                std::fs::remove_file(&args.socket)?;
            }
            let listener = UnixListener::bind(&args.socket)?;

            info!("Waiting for connection...");
            let (stream, _) = listener.accept().await?;
            info!("Accepted connection");

            let bearer = Bearer::Unix(stream);
            let mut plexer = Plexer::new(bearer);

            // Mux Responder: subscribe_server.
            let to_channel = plexer.subscribe_server(PROTOCOL_TFWP_TRACE_OBJECTS);
            let ekg_channel = plexer.subscribe_server(PROTOCOL_TFWP_EKG_METRICS);
            let dp_channel = plexer.subscribe_server(PROTOCOL_TFWP_DATAPOINTS);
            let hs_channel = plexer.subscribe_server(0);

            let plexer_handle = plexer.spawn();

            // Handshake Server
            let mut hs_server = handshake::Server::<n2c::VersionData>::new(hs_channel);
            info!("Receiving proposed versions...");
            let version_table = hs_server.receive_proposed_versions().await?;
            info!("Proposed versions: {:?}", version_table);

            // Accept v16 or highest
            // For now just accept the first one or hardcode 32784 (v16) if present
            if let Some((v, data)) = version_table.values.into_iter().next() {
                info!("Accepting version {}", v);
                hs_server.accept_version(v, data).await?;
            } else {
                error!("No versions proposed");
                return Ok(());
            }

            info!("Handshake accepted. Starting in CLIENT mode (acting as Tracer)");

            // We are Mux Responder.
            // We want to Request Traces (if that's how it works) or Receive them?
            // If in Server mode (Node connecting to Tracer), the Node (Mux Init) ran MP
            // Servers. So Tracer (Mux Resp) must run MP Clients.

            // TraceObjects Client (Requests traces)
            let to_task = tokio::spawn(async move {
                let mut client = traceobjects::Client::new(to_channel);
                loop {
                    // Poll for traces
                    if let Err(e) = client.send_request(false, 10).await {
                        error!("TraceObjects Request Error: {:?}", e);
                        break;
                    }
                    match client.recv_response().await {
                        Ok(objs) => {
                            info!("TraceObjects: Received {} objects", objs.len());
                            for obj in objs {
                                info!("  - {:?}", obj);
                            }
                        }
                        Err(e) => {
                            error!("TraceObjects Response Error: {:?}", e);
                            break;
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            });

            // EKG Client
            let ekg_task = tokio::spawn(async move {
                let mut client = ekgmetrics::Client::new(ekg_channel);
                loop {
                    if let Err(e) = client.send_request(ekgmetrics::Request::GetUpdated).await {
                        error!("EKG Request Error: {:?}", e);
                        break;
                    }
                    match client.recv_response().await {
                        Ok(metrics) => {
                            info!("EKG: Received {} metrics", metrics.len());
                            for (name, val) in metrics {
                                info!("  - {}: {:?}", name, val);
                            }
                        }
                        Err(e) => {
                            error!("EKG Response Error: {:?}", e);
                            break;
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            });

            // Datapoints Client
            let dp_task = tokio::spawn(async move {
                let mut client = datapoints::Client::new(dp_channel);
                loop {
                    if let Err(e) = client.send_request(vec![]).await {
                        error!("Datapoints Request Error: {:?}", e);
                        break;
                    }
                    match client.recv_response().await {
                        Ok(points) => {
                            info!("Datapoints: Received {} points", points.len());
                            for (name, val) in points {
                                info!("  - {}: {:?}", name, val);
                            }
                        }
                        Err(e) => {
                            error!("Datapoints Response Error: {:?}", e);
                            break;
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            });

            let _ = tokio::join!(to_task, ekg_task, dp_task);
            plexer_handle.abort().await;
        }
        _ => {
            error!("Invalid mode: {}. Use 'server' or 'client'", args.mode);
        }
    }

    Ok(())
}
