use crate::config::Endpoint;
use crate::node::{MetricValue, SharedRegistry};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

struct AppState {
    registry: SharedRegistry,
}

/// Start the Prometheus HTTP server.
pub async fn run_prometheus_server(
    endpoint: Endpoint,
    registry: SharedRegistry,
    shutdown: CancellationToken,
) {
    let state = Arc::new(AppState { registry });

    let router = Router::new()
        .route("/", get(list_nodes))
        .route("/{slug}", get(node_metrics))
        .with_state(state);

    let bind_addr = format!("{}:{}", endpoint.host, endpoint.port);
    info!("Prometheus endpoint on http://{}", bind_addr);

    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind Prometheus endpoint: {:?}", e);
            return;
        }
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .ok();
}

/// `GET /` — list connected nodes with links.
async fn list_nodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let reg = state.registry.read().await;
    let nodes = reg.all_nodes();

    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("application/json") {
        let names: Vec<&str> = nodes.iter().map(|(name, _)| *name).collect();
        let json = serde_json::json!(names);
        (
            StatusCode::OK,
            [("content-type", "application/json")],
            json.to_string(),
        )
            .into_response()
    } else {
        let mut html = String::from(
            "<html><head><title>Cardano Tracer - Nodes</title></head><body>\n\
             <h1>Connected Nodes</h1><ul>\n",
        );
        for (name, _) in &nodes {
            let slug = crate::node::slug_for_node(name);
            html.push_str(&format!(
                "<li><a href=\"/{}\">{}</a></li>\n",
                slug, name
            ));
        }
        html.push_str("</ul></body></html>");
        Html(html).into_response()
    }
}

/// `GET /{slug}` — Prometheus text exposition format for a node's metrics.
async fn node_metrics(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let reg = state.registry.read().await;
    let nodes = reg.all_nodes();

    // Find the node whose slugified name matches.
    let node = nodes
        .iter()
        .find(|(name, _)| crate::node::slug_for_node(name) == slug);

    match node {
        Some((_, metrics)) => {
            let mut output = String::new();
            for entry in *metrics {
                let sanitized = sanitize_metric_name(&entry.name);
                match &entry.value {
                    MetricValue::Counter(v) => {
                        output.push_str(&format!("# TYPE {} counter\n", sanitized));
                        output.push_str(&format!("{} {}\n", sanitized, v));
                    }
                    MetricValue::Gauge(v) => {
                        output.push_str(&format!("# TYPE {} gauge\n", sanitized));
                        output.push_str(&format!("{} {}\n", sanitized, v));
                    }
                    MetricValue::Label(s) => {
                        output.push_str(&format!("# TYPE {} gauge\n", sanitized));
                        output.push_str(&format!(
                            "{}{{value=\"{}\"}} 1\n",
                            sanitized,
                            escape_label_value(s)
                        ));
                    }
                }
            }
            (
                StatusCode::OK,
                [(
                    "content-type",
                    "text/plain; version=0.0.4; charset=utf-8",
                )],
                output,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "Node not found").into_response(),
    }
}

/// Sanitize a metric name for Prometheus (replace dots/dashes with underscores).
fn sanitize_metric_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Escape a label value for Prometheus text format.
fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
