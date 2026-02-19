mod cbor_json;
mod config;
mod connection;
mod logging;
mod metrics;
mod node;

use crate::config::{Address, Network, TracerConfig};
use crate::logging::{LogManager, SharedLogManager};
use crate::node::{node_id_from_address, NodeRegistry, SharedRegistry};
use clap::Parser;
use pallas::network::multiplexer::Bearer;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "cardano-tracer", about = "Cardano trace aggregator (Rust)")]
struct Args {
    /// Path to the tracer configuration YAML file.
    #[arg(long)]
    config: PathBuf,

    /// Optional state directory for persistent data.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();
    let config = config::load_config(&args.config)?;
    let config = Arc::new(config);

    info!("Loaded config: {:?}", config);

    let registry: SharedRegistry = Arc::new(RwLock::new(NodeRegistry::new()));
    let log_manager: SharedLogManager = Arc::new(tokio::sync::Mutex::new(LogManager::new(
        &config.logging,
        config.rotation.clone(),
        config.verbosity.clone(),
    )));

    let shutdown = CancellationToken::new();

    // Spawn log rotation task.
    if let Some(ref rotation) = config.rotation {
        let lm = log_manager.clone();
        let rot = rotation.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            logging::run_rotation_loop(lm, rot, sd).await;
        });
    }

    // Spawn Prometheus server.
    if let Some(ref endpoint) = config.has_prometheus {
        let reg = registry.clone();
        let ep = endpoint.clone();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            metrics::run_prometheus_server(ep, reg, sd).await;
        });
    }

    // Spawn signal handler.
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let mut sig_term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut sig_int =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        tokio::select! {
            _ = sig_term.recv() => info!("Received SIGTERM"),
            _ = sig_int.recv() => info!("Received SIGINT"),
        }
        shutdown_signal.cancel();
    });

    // Run connection loop.
    match &config.network {
        Network::AcceptAt(addr_str) => {
            run_accept_loop(addr_str, config.clone(), registry, log_manager, shutdown).await?;
        }
        Network::ConnectTo(addr_strs) => {
            run_connect_loop(addr_strs, config.clone(), registry, log_manager, shutdown).await?;
        }
    }

    info!("Shutdown complete");
    Ok(())
}

/// AcceptAt mode: listen for incoming node connections.
async fn run_accept_loop(
    addr_str: &str,
    config: Arc<TracerConfig>,
    registry: SharedRegistry,
    log_manager: SharedLogManager,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let address = Address::parse(addr_str);

    match address {
        Address::Unix(ref path) => {
            // Ensure parent directory exists.
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            // Remove stale socket file.
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            let listener = tokio::net::UnixListener::bind(path)?;
            info!("Listening on {:?}", path);

            let mut conn_counter = 0u64;
            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, _addr)) => {
                                let node_id = node_id_from_address(
                                    &format!("{}@{}", path.display(), conn_counter)
                                );
                                conn_counter += 1;
                                let bearer = Bearer::Unix(stream);
                                let cfg = config.clone();
                                let reg = registry.clone();
                                let lm = log_manager.clone();
                                let sd = shutdown.clone();

                                tokio::spawn(async move {
                                    if let Err(e) = connection::handle_connection(
                                        bearer, node_id.clone(), cfg, reg, lm, sd,
                                    ).await {
                                        error!("Node {:?}: connection error: {:?}", node_id, e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Accept error: {:?}", e);
                            }
                        }
                    }
                    _ = shutdown.cancelled() => {
                        info!("Accept loop shutting down");
                        break;
                    }
                }
            }
        }
        Address::Tcp(ref host, port) => {
            let bind_addr = format!("{}:{}", host, port);
            let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
            info!("Listening on {}", bind_addr);

            loop {
                tokio::select! {
                    accept_res = listener.accept() => {
                        match accept_res {
                            Ok((stream, addr)) => {
                                let node_id = node_id_from_address(&addr.to_string());
                                let bearer = Bearer::Tcp(stream);
                                let cfg = config.clone();
                                let reg = registry.clone();
                                let lm = log_manager.clone();
                                let sd = shutdown.clone();

                                tokio::spawn(async move {
                                    if let Err(e) = connection::handle_connection(
                                        bearer, node_id.clone(), cfg, reg, lm, sd,
                                    ).await {
                                        error!("Node {:?}: connection error: {:?}", node_id, e);
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Accept error: {:?}", e);
                            }
                        }
                    }
                    _ = shutdown.cancelled() => {
                        info!("Accept loop shutting down");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// ConnectTo mode: initiate connections to listed node addresses with reconnection.
async fn run_connect_loop(
    addr_strs: &[String],
    config: Arc<TracerConfig>,
    registry: SharedRegistry,
    log_manager: SharedLogManager,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut handles = Vec::new();

    for addr_str in addr_strs {
        let address = Address::parse(addr_str);
        let addr_label = addr_str.clone();
        let cfg = config.clone();
        let reg = registry.clone();
        let lm = log_manager.clone();
        let sd = shutdown.clone();

        let handle = tokio::spawn(async move {
            loop {
                info!("Connecting to {}", addr_label);
                let bearer_result = match &address {
                    Address::Unix(path) => Bearer::connect_unix(path).await,
                    Address::Tcp(host, port) => {
                        Bearer::connect_tcp(format!("{}:{}", host, port)).await
                    }
                };

                match bearer_result {
                    Ok(bearer) => {
                        let node_id = node_id_from_address(&addr_label);
                        if let Err(e) = connection::handle_connection(
                            bearer,
                            node_id,
                            cfg.clone(),
                            reg.clone(),
                            lm.clone(),
                            sd.clone(),
                        )
                        .await
                        {
                            error!("Connection to {} error: {:?}", addr_label, e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to connect to {}: {:?}", addr_label, e);
                    }
                }

                // Wait before reconnecting.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    _ = sd.cancelled() => break,
                }
            }
        });

        handles.push(handle);
    }

    // Wait for shutdown.
    shutdown.cancelled().await;
    for h in handles {
        h.abort();
    }

    Ok(())
}
