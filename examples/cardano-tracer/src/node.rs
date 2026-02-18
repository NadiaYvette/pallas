use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Unique identifier for a connected node, derived from socket address.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct NodeId(pub String);

/// Per-node EKG metric entry.
#[derive(Clone, Debug)]
pub struct MetricEntry {
    pub name: String,
    pub value: MetricValue,
}

#[derive(Clone, Debug)]
pub enum MetricValue {
    Counter(i64),
    Gauge(i64),
    Label(String),
}

/// State tracked for each connected node.
pub struct NodeState {
    pub node_id: NodeId,
    pub node_name: String,
    pub metrics: Vec<MetricEntry>,
}

/// Central registry of all connected nodes.
pub struct NodeRegistry {
    nodes: HashMap<NodeId, NodeState>,
}

pub type SharedRegistry = Arc<RwLock<NodeRegistry>>;

impl NodeRegistry {
    pub fn new() -> Self {
        NodeRegistry {
            nodes: HashMap::new(),
        }
    }

    pub fn register_node(&mut self, node_id: NodeId, node_name: String) {
        self.nodes.insert(
            node_id.clone(),
            NodeState {
                node_id,
                node_name,
                metrics: Vec::new(),
            },
        );
    }

    pub fn deregister_node(&mut self, node_id: &NodeId) {
        self.nodes.remove(node_id);
    }

    pub fn get_node_name(&self, node_id: &NodeId) -> Option<String> {
        self.nodes.get(node_id).map(|s| s.node_name.clone())
    }

    pub fn update_node_name(&mut self, node_id: &NodeId, name: String) {
        if let Some(state) = self.nodes.get_mut(node_id) {
            state.node_name = name;
        }
    }

    pub fn update_metrics(&mut self, node_id: &NodeId, metrics: Vec<MetricEntry>) {
        if let Some(state) = self.nodes.get_mut(node_id) {
            state.metrics = metrics;
        }
    }

    /// Get all nodes and their metrics (for Prometheus scraping).
    pub fn all_nodes(&self) -> Vec<(&str, &[MetricEntry])> {
        self.nodes
            .values()
            .map(|s| (s.node_name.as_str(), s.metrics.as_slice()))
            .collect()
    }
}

/// Derive a NodeId from a socket address string.
///
/// Sanitizes the address by removing problematic characters,
/// matching the Haskell `connIdToNodeId` logic.
pub fn node_id_from_address(addr: &str) -> NodeId {
    let sanitized = addr
        .replace(' ', "-")
        .replace('"', "-")
        .replace('/', "-")
        .replace('\\', "-")
        .replace("--", "")
        .trim_matches('-')
        .to_string();
    NodeId(sanitized)
}

/// Convert a node name to a URL-safe slug for Prometheus routes.
pub fn slug_for_node(name: &str) -> String {
    name.chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .collect::<String>()
        .to_lowercase()
}
