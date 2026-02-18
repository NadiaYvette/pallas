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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_metric_name_dots() {
        assert_eq!(sanitize_metric_name("rts.gc.bytes_allocated"), "rts_gc_bytes_allocated");
    }

    #[test]
    fn test_sanitize_metric_name_dashes() {
        assert_eq!(sanitize_metric_name("node-metrics-count"), "node_metrics_count");
    }

    #[test]
    fn test_sanitize_metric_name_colons_preserved() {
        assert_eq!(sanitize_metric_name("cardano:node:txs"), "cardano:node:txs");
    }

    #[test]
    fn test_sanitize_metric_name_underscores_preserved() {
        assert_eq!(sanitize_metric_name("my_metric_name"), "my_metric_name");
    }

    #[test]
    fn test_sanitize_metric_name_mixed() {
        assert_eq!(
            sanitize_metric_name("rts.gc.par-tot-bytes-copied"),
            "rts_gc_par_tot_bytes_copied"
        );
    }

    #[test]
    fn test_escape_label_value_plain() {
        assert_eq!(escape_label_value("hello"), "hello");
    }

    #[test]
    fn test_escape_label_value_backslash() {
        assert_eq!(escape_label_value("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_label_value_quote() {
        assert_eq!(escape_label_value("say \"hi\""), "say \\\"hi\\\"");
    }

    #[test]
    fn test_escape_label_value_newline() {
        assert_eq!(escape_label_value("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn test_escape_label_value_combined() {
        assert_eq!(
            escape_label_value("a\\b\"c\nd"),
            "a\\\\b\\\"c\\nd"
        );
    }

    #[test]
    fn test_prometheus_counter_format() {
        let entry = crate::node::MetricEntry {
            name: "tx.count".to_string(),
            value: MetricValue::Counter(42),
        };
        let sanitized = sanitize_metric_name(&entry.name);
        let mut output = String::new();
        output.push_str(&format!("# TYPE {} counter\n", sanitized));
        output.push_str(&format!("{} {}\n", sanitized, 42));
        assert_eq!(output, "# TYPE tx_count counter\ntx_count 42\n");
    }

    #[test]
    fn test_prometheus_gauge_format() {
        let entry = crate::node::MetricEntry {
            name: "mem.used".to_string(),
            value: MetricValue::Gauge(1024),
        };
        let sanitized = sanitize_metric_name(&entry.name);
        let mut output = String::new();
        output.push_str(&format!("# TYPE {} gauge\n", sanitized));
        output.push_str(&format!("{} {}\n", sanitized, 1024));
        assert_eq!(output, "# TYPE mem_used gauge\nmem_used 1024\n");
    }

    #[test]
    fn test_prometheus_label_format() {
        let entry = crate::node::MetricEntry {
            name: "node.version".to_string(),
            value: MetricValue::Label("1.35.4".to_string()),
        };
        let sanitized = sanitize_metric_name(&entry.name);
        let escaped = escape_label_value("1.35.4");
        let mut output = String::new();
        output.push_str(&format!("# TYPE {} gauge\n", sanitized));
        output.push_str(&format!("{}{{value=\"{}\"}} 1\n", sanitized, escaped));
        assert_eq!(
            output,
            "# TYPE node_version gauge\nnode_version{value=\"1.35.4\"} 1\n"
        );
    }
}
