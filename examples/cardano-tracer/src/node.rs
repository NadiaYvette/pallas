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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_get_node_name() {
        let mut reg = NodeRegistry::new();
        let id = NodeId("node-1".to_string());
        reg.register_node(id.clone(), "my-node".to_string());
        assert_eq!(reg.get_node_name(&id), Some("my-node".to_string()));
    }

    #[test]
    fn test_deregister_node() {
        let mut reg = NodeRegistry::new();
        let id = NodeId("node-1".to_string());
        reg.register_node(id.clone(), "my-node".to_string());
        reg.deregister_node(&id);
        assert_eq!(reg.get_node_name(&id), None);
    }

    #[test]
    fn test_update_node_name() {
        let mut reg = NodeRegistry::new();
        let id = NodeId("node-1".to_string());
        reg.register_node(id.clone(), "initial".to_string());
        reg.update_node_name(&id, "updated".to_string());
        assert_eq!(reg.get_node_name(&id), Some("updated".to_string()));
    }

    #[test]
    fn test_update_metrics() {
        let mut reg = NodeRegistry::new();
        let id = NodeId("node-1".to_string());
        reg.register_node(id.clone(), "my-node".to_string());

        let metrics = vec![
            MetricEntry {
                name: "cpu".to_string(),
                value: MetricValue::Gauge(42),
            },
            MetricEntry {
                name: "txs".to_string(),
                value: MetricValue::Counter(100),
            },
        ];
        reg.update_metrics(&id, metrics);

        let all = reg.all_nodes();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "my-node");
        assert_eq!(all[0].1.len(), 2);
    }

    #[test]
    fn test_node_id_from_address_unix() {
        let id = node_id_from_address("/tmp/test.sock@0");
        // slashes replaced with dashes, double-dashes removed, leading dash trimmed
        assert!(!id.0.contains('/'));
        assert!(!id.0.contains("--"));
        assert!(!id.0.starts_with('-'));
        assert!(!id.0.ends_with('-'));
    }

    #[test]
    fn test_node_id_from_address_tcp() {
        let id = node_id_from_address("127.0.0.1:3001");
        assert_eq!(id.0, "127.0.0.1:3001");
    }

    #[test]
    fn test_node_id_from_address_spaces() {
        let id = node_id_from_address("some address with spaces");
        assert!(!id.0.contains(' '));
    }

    #[test]
    fn test_slug_for_node_simple() {
        assert_eq!(slug_for_node("MyNode"), "mynode");
    }

    #[test]
    fn test_slug_for_node_special_chars() {
        assert_eq!(slug_for_node("node-1_test"), "node-1_test");
        // dots and slashes removed
        assert_eq!(slug_for_node("node.1/test"), "node1test");
    }

    #[test]
    fn test_slug_for_node_mixed() {
        let slug = slug_for_node("Node@Host:3001");
        assert_eq!(slug, "nodehost3001");
    }

    #[test]
    fn test_multiple_nodes() {
        let mut reg = NodeRegistry::new();
        let id1 = NodeId("n1".to_string());
        let id2 = NodeId("n2".to_string());
        reg.register_node(id1.clone(), "node-a".to_string());
        reg.register_node(id2.clone(), "node-b".to_string());
        assert_eq!(reg.all_nodes().len(), 2);
        reg.deregister_node(&id1);
        assert_eq!(reg.all_nodes().len(), 1);
    }
}
