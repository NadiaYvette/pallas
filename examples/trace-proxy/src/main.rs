use clap::Parser;
use pallas::codec::minicbor;
use pallas::codec::Fragment;
use pallas::network::{
    miniprotocols::{
        traceobjects, PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_EKG_METRICS,
        PROTOCOL_TFWP_TRACE_OBJECTS,
    },
    multiplexer::{AgentReceiver, AgentSender, Bearer, Plexer},
};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tracing::{error, info, warn};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    node_socket: PathBuf,
    #[arg(long)]
    tracer_socket: PathBuf,
    #[arg(long, default_value_t = 764824073)]
    magic: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .finish(),
    )?;

    let args = Args::parse();

    if args.node_socket.exists() {
        std::fs::remove_file(&args.node_socket)?;
    }
    let listener = UnixListener::bind(&args.node_socket)?;
    info!("Proxy listening on {:?}", args.node_socket);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Signal handler for graceful shutdown.
    tokio::spawn(async move {
        let mut sig_term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut sig_int =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        tokio::select! {
            _ = sig_term.recv() => info!("Received SIGTERM"),
            _ = sig_int.recv() => info!("Received SIGINT"),
        }
        let _ = shutdown_tx.send(true);
    });

    loop {
        let mut shutdown = shutdown_rx.clone();
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((node_stream, _)) => {
                        info!("Node connected");
                        let tracer_socket = args.tracer_socket.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_proxy(node_stream, tracer_socket).await {
                                error!("Proxy error: {:?}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {:?}", e);
                    }
                }
            }
            _ = shutdown.changed() => {
                info!("Proxy shutting down");
                break;
            }
        }
    }

    Ok(())
}

async fn connect_with_retry(socket: &PathBuf) -> Result<UnixStream, Box<dyn std::error::Error>> {
    let start = std::time::Instant::now();
    loop {
        match UnixStream::connect(socket).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if start.elapsed().as_secs() > 30 {
                    return Err(Box::new(e));
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

async fn handle_proxy(
    node_stream: UnixStream,
    tracer_socket: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let tracer_stream = connect_with_retry(&tracer_socket).await?;
    info!("Connected to Real Tracer at {:?}", tracer_socket);

    let node_bearer = Bearer::Unix(node_stream);
    let tracer_bearer = Bearer::Unix(tracer_stream);

    let mut node_plexer = Plexer::new(node_bearer);
    let mut tracer_plexer = Plexer::new(tracer_bearer);

    let protocols = vec![
        0,
        PROTOCOL_TFWP_TRACE_OBJECTS,
        PROTOCOL_TFWP_DATAPOINTS,
        PROTOCOL_TFWP_EKG_METRICS,
    ];

    let mut node_channels = Vec::new();
    for p in &protocols {
        node_channels.push(node_plexer.subscribe_server(*p));
    }

    let mut tracer_channels = Vec::new();
    for p in &protocols {
        tracer_channels.push(tracer_plexer.subscribe_client(*p));
    }

    let _node_handle = node_plexer.spawn();
    let _tracer_handle = tracer_plexer.spawn();

    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    for (_i, p) in protocols.iter().enumerate() {
        if node_channels.is_empty() || tracer_channels.is_empty() {
            break;
        }
        let node_chan = node_channels.remove(0);
        let tracer_chan = tracer_channels.remove(0);

        let (node_tx, node_rx) = node_chan.into_split();
        let (tracer_tx, tracer_rx) = tracer_chan.into_split();

        if *p == PROTOCOL_TFWP_TRACE_OBJECTS {
            // Decode and re-encode TraceObjects to exercise protocol support
            handles.push(tokio::spawn(bridge_trace_objects(node_rx, tracer_tx, "Node->Tracer")));
            handles.push(tokio::spawn(bridge_trace_objects(tracer_rx, node_tx, "Tracer->Node")));
        } else {
            let p_label = format!("Protocol {}", p);
            handles.push(tokio::spawn(forward_raw(node_rx, tracer_tx, p_label.clone())));
            handles.push(tokio::spawn(forward_raw(tracer_rx, node_tx, p_label)));
        }
    }

    // Wait for all bridge tasks to complete (connection closed on either side).
    for handle in handles {
        if let Err(e) = handle.await {
            error!("Bridge task failed: {:?}", e);
        }
    }

    info!("Proxy session ended");
    Ok(())
}

async fn forward_raw(mut rx: AgentReceiver, mut tx: AgentSender, label: String) {
    loop {
        match rx.dequeue_chunk().await {
            Ok(chunk) => {
                info!("{}: Forwarding chunk {} bytes", label, chunk.len());
                if tx.enqueue_chunk(chunk).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn try_decode_message_with_bytes<M>(buffer: &mut Vec<u8>) -> Result<Option<(M, Vec<u8>)>, String>
where
    M: Fragment,
{
    let mut decoder = minicbor::Decoder::new(buffer);
    let maybe_msg = decoder.decode();

    match maybe_msg {
        Ok(msg) => {
            let pos = decoder.position();
            let raw: Vec<u8> = buffer.drain(0..pos).collect();
            Ok(Some((msg, raw)))
        }
        Err(err) if err.is_end_of_input() => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

async fn bridge_trace_objects(mut rx: AgentReceiver, mut tx: AgentSender, label: &'static str) {
    let mut decode_buf = Vec::new();
    let mut decode_failed = false;

    loop {
        // Try to decode accumulated data for diagnostic purposes
        if !decode_failed {
            loop {
                match try_decode_message_with_bytes::<traceobjects::Message>(&mut decode_buf) {
                    Ok(Some((msg, raw_bytes))) => {
                        info!("{}: Decoded Message: {:?}", label, msg);

                        // Idempotency check (informational only)
                        let mut reencoded = Vec::new();
                        if let Err(e) = minicbor::encode(&msg, &mut reencoded) {
                            warn!("{}: Re-encoding failed: {:?}", label, e);
                        } else if raw_bytes != reencoded {
                            warn!("{}: Idempotency check FAILED!", label);
                            warn!("Original (len={}): {:02x?}", raw_bytes.len(), raw_bytes);
                            warn!("Re-encoded (len={}): {:02x?}", reencoded.len(), reencoded);
                        } else {
                            info!("{}: Idempotency check passed.", label);
                        }
                        continue; // Try to decode more from buffer
                    }
                    Ok(None) => break, // Need more data
                    Err(e) => {
                        warn!("{}: Decode Error (switching to raw forwarding): {:?}", label, e);
                        warn!("Buffer (len={}): {:02x?}", decode_buf.len(), &decode_buf[..decode_buf.len().min(64)]);
                        decode_failed = true;
                        decode_buf.clear();
                        break;
                    }
                }
            }
        }

        // Always forward raw chunks
        match rx.dequeue_chunk().await {
            Ok(chunk) => {
                if !decode_failed {
                    decode_buf.extend(&chunk);
                }
                if tx.enqueue_chunk(chunk).await.is_err() {
                    break;
                }
            }
            Err(_) => {
                info!("{}: Connection closed (AgentDequeue)", label);
                break;
            }
        }
    }
}
