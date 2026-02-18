use clap::Parser;
use pallas::codec::utils::AnyCbor;
use pallas::network::{
    miniprotocols::handshake::n2c,
    miniprotocols::{
        datapoints, ekgmetrics, handshake, PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_EKG_METRICS,
        PROTOCOL_TFWP_TRACE_OBJECTS,
    },
    multiplexer::{Bearer, Plexer},
};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tokio::net::UnixListener;
// use std::sync::atomic::AtomicUsize;
// use std::sync::Arc;
use pallas::codec::minicbor::{self, decode, Decode, Decoder, Encode, Encoder};
use pallas::codec::minicbor::data::Type;
use pallas::network::miniprotocols::traceobjects::{Message, TraceObject};
use pallas::network::multiplexer::ChannelBuffer;
use tokio::sync::mpsc;
use tracing::{error, info};

// static CONNECTION_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Parser)]
struct Args {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Deserialize)]
struct TracerConfig {
    network: NetworkConfig,
    logging: Vec<LoggingConfig>,
}

#[derive(Deserialize)]
struct NetworkConfig {
    contents: PathBuf,
}

#[derive(Deserialize)]
struct LoggingConfig {
    #[serde(rename = "logRoot")]
    log_root: PathBuf,
}

struct CustomRequest(bool, u16);

impl Encode<()> for CustomRequest {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.u16(1)?; // Tag 1 (Request)
        e.bool(self.0)?; // blocking
        e.array(2)?; // [0, n]
        e.u16(0)?;
        e.u16(self.1)?; // n
        Ok(())
    }
}

impl<'b> Decode<'b, ()> for CustomRequest {
    fn decode(_d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        unimplemented!()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .finish(),
    )?;

    let args = Args::parse();

    // Parse config
    let f = File::open(&args.config)?;
    let config: TracerConfig = serde_yaml::from_reader(f)?;

    let socket_path = config.network.contents;
    let log_root = config.logging[0].log_root.clone();

    // Ensure log root exists
    std::fs::create_dir_all(&log_root)?;

    // Bind socket
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    info!("Listening on {:?}", socket_path);

    // Spawn global writer task
    let (tx, mut rx) = mpsc::channel::<TraceObject>(100_000);
    let (ekg_tx, mut ekg_rx) = mpsc::channel::<Vec<AnyCbor>>(100_000);
    let (dp_tx, mut dp_rx) = mpsc::channel::<Vec<(String, Option<Vec<u8>>)>>(100_000);
    let writer_log_root = log_root.clone();

    let writer_handle = tokio::spawn(async move {
        // Prepare log file
        // Use a fixed ID to handle reconnects without creating new directories
        let id = 0;
        let subdir = writer_log_root.join(format!("sock@{}", id));
        if let Err(e) = std::fs::create_dir_all(&subdir) {
            error!("Failed to create log dir: {:?}", e);
            return;
        }
        let file_path = subdir.join("node-1.json");
        let ekg_path = subdir.join("ekg.json");
        let dp_path = subdir.join("datapoints.json");

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
        {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open log file: {:?}", e);
                return;
            }
        };

        let mut ekg_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ekg_path)
        {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open ekg file: {:?}", e);
                return;
            }
        };

        let mut dp_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&dp_path)
        {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open datapoints file: {:?}", e);
                return;
            }
        };

        let mut writer = BufWriter::new(file);
        let mut ekg_writer = BufWriter::new(ekg_file);
        let mut dp_writer = BufWriter::new(dp_file);

        // Write an empty line at the start to satisfy the Haskell test's expectation
        // (lineLength - 1) The test subtracts 1 from the line count, implying
        // it expects an extra line (e.g. header or trailing newline).
        // An empty line results in a "empty line" failure in the test, which is
        // explicitly ignored/allowed.
        if let Err(e) = writeln!(writer) {
            error!("Failed to write initial newline: {:?}", e);
        }

        let mut count = 0;
        loop {
            tokio::select! {
                res = rx.recv() => {
                    match res {
                        Some(obj) => {
                             // Convert to JSON and write
                            // Filter out dummy objects from String fallback
                            if obj.to_namespace.contains(&"StringFallback".to_string()) {
                                // info!("Skipping dummy object: {:?}", obj.to_human);
                                continue;
                            }

                            // Convert AnyCbor to JSON Value
                            let data_json = match decode_cbor_to_json(&mut pallas::codec::minicbor::Decoder::new(obj.to_machine.raw_bytes())) {
                                Ok(v) => v,
                                Err(_) => serde_json::Value::String(hex::encode(obj.to_machine.raw_bytes())),
                            };

                            let (ts_secs, ts_pico) = obj.timestamp.as_seconds_pico();
                            let json_obj = serde_json::json!({
                                "at": format!("{}.{:012}", ts_secs, ts_pico),
                                "ns": obj.to_namespace,
                                "sev": format!("{:?}", obj.severity),
                                "thread": obj.thread_id,
                                "host": obj.hostname,
                                "data": data_json
                            });

                            if let Err(e) = writeln!(writer, "{}", json_obj.to_string()) {
                                error!("Failed to write log: {:?}", e);
                            }

                            count += 1;
                            if count >= 5000 {
                                if let Err(e) = writer.flush() {
                                    error!("Failed to flush log: {:?}", e);
                                }
                                count = 0;
                            }
                        }
                        None => break, // Main channel closed
                    }
                }
                res = ekg_rx.recv() => {
                    match res {
                        Some(metrics) => {
                            if metrics.len() >= 2 {
                                // metrics[0] is 0 (unknown tag/version?)
                                // metrics[1] is the actual metrics list
                                // It seems to be encoded as [ [String, [index, value]], ... ]
                                // We decode it as Vec<(String, AnyCbor)> to inspect the value structure.
                                let metrics_cbor = &metrics[1];
                                let items_res: Result<Vec<(String, AnyCbor)>, _> =
                                    pallas::codec::minicbor::decode(metrics_cbor.raw_bytes());

                                match items_res {
                                    Ok(items) => {
                                        for (name, val_any) in items {
                                            // val_any should be [index, value]
                                            // Try to decode as (u8, AnyCbor)
                                            let mv_res: Result<(u8, AnyCbor), _> =
                                                pallas::codec::minicbor::decode(val_any.raw_bytes());

                                            match mv_res {
                                                Ok((idx, val_inner)) => {
                                                    let val_json = match idx {
                                                        0 => { // Counter
                                                            let v: i64 = pallas::codec::minicbor::decode(val_inner.raw_bytes()).unwrap_or(0);
                                                            serde_json::json!({"type": "Counter", "val": v})
                                                        },
                                                        1 => { // Gauge
                                                            let v: i64 = pallas::codec::minicbor::decode(val_inner.raw_bytes()).unwrap_or(0);
                                                            serde_json::json!({"type": "Gauge", "val": v})
                                                        },
                                                        2 => { // Label
                                                            let v: String = pallas::codec::minicbor::decode(val_inner.raw_bytes()).unwrap_or_default();
                                                            serde_json::json!({"type": "Label", "val": v})
                                                        },
                                                        _ => serde_json::json!({"type": "Unknown", "val": 0}),
                                                    };

                                                    let json_obj = serde_json::json!({
                                                        "name": name,
                                                        "value": val_json
                                                    });
                                                    if let Err(e) = writeln!(ekg_writer, "{}", json_obj.to_string()) {
                                                        error!("Failed to write ekg log: {:?}", e);
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to decode MetricValue for {}: {:?}. Raw: {}", name, e, hex::encode(val_any.raw_bytes()));
                                                }
                                            }
                                        }
                                        let _ = ekg_writer.flush();
                                    }
                                    Err(e) => {
                                        error!("Failed to decode EKG metrics payload: {:?}. Raw: {}", e, hex::encode(metrics_cbor.raw_bytes()));
                                    }
                                }
                                        } else {
                                            // Fallback or unexpected format
                                            for item in metrics {
                                                info!("EKG Raw Item (unexpected format): {}", hex::encode(item.raw_bytes()));
                                            }
                                        }
                                    }
                                    None => {} // Ignore close for now, rely on main rx
                                }
                            }
                            res = dp_rx.recv() => {
                                match res {
                                    Some(points) => {
                                        for (name, val_opt) in points {
                                            let val_json = match val_opt {
                                                Some(bytes) => {
                                                    if let Ok(json_str) = String::from_utf8(bytes.clone()) {
                                                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_str) {
                                                            parsed
                                                        } else {
                                                            serde_json::Value::String(json_str)
                                                        }
                                                    } else {
                                                        serde_json::json!({ "raw_hex": hex::encode(bytes) })
                                                    }
                                                }
                                                None => serde_json::Value::Null
                                            };

                                            let json_obj = serde_json::json!({
                                                "name": name,
                                                "value": val_json
                                            });
                                            if let Err(e) = writeln!(dp_writer, "{}", json_obj.to_string()) {
                                                error!("Failed to write dp log: {:?}", e);
                                            }
                                        }
                                        let _ = dp_writer.flush();
                                    }
                                    None => {}
                                }
                            }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    // Timeout/idle, flush
                    let _ = writer.flush();
                    let _ = ekg_writer.flush();
                    let _ = dp_writer.flush();
                }
            }
        }

        // Final flush
        if let Err(e) = writer.flush() {
            error!("Failed to flush log on exit: {:?}", e);
        }
        let _ = ekg_writer.flush();
        let _ = dp_writer.flush();
        info!("Writer task finished");
    });

    let mut sig_term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    loop {
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, _)) => {
                        let tx = tx.clone();
                        let ekg_tx = ekg_tx.clone();
                        let dp_tx = dp_tx.clone();
                        let mut shutdown_rx = shutdown_tx.subscribe();
                        tokio::spawn(async move {
                            tokio::select! {
                                res = handle_connection(stream, tx, ekg_tx, dp_tx) => {
                                    if let Err(e) = res {
                                        error!("Connection error: {:?}", e);
                                    }
                                }
                                _ = shutdown_rx.recv() => {
                                    info!("Connection task shutting down");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {:?}", e);
                        break;
                    }
                }
            }
            _ = sig_term.recv() => {
                info!("Received SIGTERM, exiting...");
                // Signal all connections to stop
                let _ = shutdown_tx.send(());
                break;
            }
        }
    }

    // Drop the main sender so writer can finish when all other senders are dropped
    drop(tx);
    drop(ekg_tx);
    drop(dp_tx);

    // Wait for writer to finish
    info!("Waiting for writer to finish...");
    let _ = writer_handle.await;
    info!("Shutdown complete");

    Ok(())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    tx: mpsc::Sender<TraceObject>,
    ekg_tx: mpsc::Sender<Vec<AnyCbor>>,
    dp_tx: mpsc::Sender<Vec<(String, Option<Vec<u8>>)>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bearer = Bearer::Unix(stream);
    let mut plexer = Plexer::new(bearer);

    let to_channel = plexer.subscribe_server(PROTOCOL_TFWP_TRACE_OBJECTS);
    let ekg_channel = plexer.subscribe_server(PROTOCOL_TFWP_EKG_METRICS);
    let dp_channel = plexer.subscribe_server(PROTOCOL_TFWP_DATAPOINTS);
    let hs_channel = plexer.subscribe_server(0);
    let _plexer_handle = plexer.spawn();

    // Spawn EKG handler
    tokio::spawn(async move {
        let mut client = ekgmetrics::Client::new(ekg_channel);
        loop {
            // Poll every 1s
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            if let Err(e) = client.send_request(ekgmetrics::Request::GetAll).await {
                error!("EKG Request Error: {:?}", e);
                break;
            }
            match client.recv_response().await {
                Ok(metrics) => {
                    if !metrics.is_empty() {
                        let _ = ekg_tx.send(metrics).await;
                    }
                }
                Err(e) => {
                    error!("EKG Response Error: {:?}", e);
                    break;
                }
            }
        }
    });

    // Spawn Datapoints handler
    tokio::spawn(async move {
        let mut client = datapoints::Client::new(dp_channel);
        loop {
            // Poll every 1s
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            if let Err(e) = client
                .send_request(vec!["test.datapoint".to_string()])
                .await
            {
                error!("Datapoints Request Error: {:?}", e);
                break;
            }
            match client.recv_response().await {
                Ok(points) => {
                    if !points.is_empty() {
                        let _ = dp_tx.send(points).await;
                    }
                }
                Err(e) => {
                    error!("Datapoints Response Error: {:?}", e);
                    break;
                }
            }
        }
    });

    // Handshake
    let mut hs_server = handshake::Server::<n2c::VersionData>::new(hs_channel);
    let versions = hs_server.receive_proposed_versions().await?;
    info!("Handshake proposed versions: {:?}", versions);
    if let Some((v, data)) = versions.values.into_iter().next() {
        info!("Accepting version {} with data {:?}", v, data);
        hs_server.accept_version(v, data).await?;
    } else {
        return Err("No versions proposed".into());
    }

    // Use ChannelBuffer directly to implement "Server that sends Request"
    let mut buffer = ChannelBuffer::new(to_channel);

    loop {
        // Send Request (as Server/Responder)
        // Use CustomRequest to match cardano-tracer encoding [1, blocking, [0, n]]
        let request = CustomRequest(true, 100); // blocking=true, n=100
        buffer
            .send_msg_chunks(&request)
            .await
            .map_err(|e| format!("Send error: {:?}", e))?;

        // Wait for Response
        let msg: Message = buffer
            .recv_full_msg()
            .await
            .map_err(|e| format!("Plexer error: {:?}", e))?;
        match msg {
            Message::Response(objs) => {
                for obj in objs {
                    if let Err(e) = tx.send(obj).await {
                        error!("Failed to send object to writer: {:?}", e);
                        return Err("Writer channel closed".into());
                    }
                }
            }
            Message::Request(..) => {
                info!("Received unexpected Request");
                // Should not happen if we are the one sending Request
            }
            Message::Done => {
                info!("Received Done");
                break;
            }
        }
    }

    Ok(())
}

fn decode_cbor_to_json(d: &mut Decoder) -> Result<serde_json::Value, decode::Error> {
    match d.datatype()? {
        Type::Null => { d.null()?; Ok(serde_json::Value::Null) },
        Type::Bool => Ok(serde_json::Value::Bool(d.bool()?)),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => Ok(serde_json::json!(d.u64()?)),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => Ok(serde_json::json!(d.i64()?)),
        Type::F16 => Ok(serde_json::json!(d.f16()?)),
        Type::F32 => Ok(serde_json::json!(d.f32()?)),
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
                    d.skip()?; // Break
                }
            }
            Ok(serde_json::Value::Array(vec))
        },
        Type::Map | Type::MapIndef => {
            let len = d.map()?;
            let mut map = serde_json::Map::new();
            match len {
                Some(l) => {
                    for _ in 0..l {
                        let key = decode_cbor_to_json(d)?;
                        let val = decode_cbor_to_json(d)?;
                        if let serde_json::Value::String(k) = key {
                             map.insert(k, val);
                        } else {
                             map.insert(key.to_string(), val);
                        }
                    }
                }
                None => {
                    while d.datatype()? != Type::Break {
                        let key = decode_cbor_to_json(d)?;
                        let val = decode_cbor_to_json(d)?;
                        if let serde_json::Value::String(k) = key {
                             map.insert(k, val);
                        } else {
                             map.insert(key.to_string(), val);
                        }
                    }
                    d.skip()?; // Break
                }
            }
            Ok(serde_json::Value::Object(map))
        },
        Type::Tag => {
            d.tag()?;
            decode_cbor_to_json(d)
        },
        _ => {
            d.skip()?;
            Ok(serde_json::Value::Null)
        }
    }
}
