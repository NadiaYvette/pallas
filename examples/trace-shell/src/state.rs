use pallas::network::miniprotocols::ekgmetrics::MetricValue;
use pallas::network::miniprotocols::traceobjects::TraceObject;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared state between the interactive shell and protocol handler tasks.
pub struct State {
    /// Queued trace objects, drained in FIFO order when the tracer polls.
    pub trace_queue: VecDeque<TraceObject>,
    /// Current EKG metrics served when the tracer polls.
    pub metrics: HashMap<String, MetricValue>,
    /// Stored datapoints served when the tracer requests by name.
    pub datapoints: HashMap<String, Option<Vec<u8>>>,
    /// Number of trace objects served to the tracer.
    pub traces_served: u64,
    /// Number of EKG poll requests received.
    pub ekg_polls: u64,
    /// Number of datapoint poll requests received.
    pub dp_polls: u64,
}

pub type SharedState = Arc<RwLock<State>>;

impl State {
    pub fn new() -> Self {
        Self {
            trace_queue: VecDeque::new(),
            metrics: HashMap::new(),
            datapoints: HashMap::new(),
            traces_served: 0,
            ekg_polls: 0,
            dp_polls: 0,
        }
    }

    /// Push a trace object to the back of the queue.
    pub fn enqueue_trace(&mut self, obj: TraceObject) {
        self.trace_queue.push_back(obj);
    }

    /// Drain up to `max` trace objects from the front of the queue.
    pub fn drain_traces(&mut self, max: usize) -> Vec<TraceObject> {
        let n = max.min(self.trace_queue.len());
        self.trace_queue.drain(..n).collect()
    }

    pub fn set_metric(&mut self, name: String, value: MetricValue) {
        self.metrics.insert(name, value);
    }

    pub fn get_metric(&self, name: &str) -> Option<&MetricValue> {
        self.metrics.get(name)
    }

    pub fn del_metric(&mut self, name: &str) -> bool {
        self.metrics.remove(name).is_some()
    }

    /// Increment a Counter metric by `delta`. Returns an error message if the
    /// metric does not exist or is not a Counter.
    pub fn incr_metric(&mut self, name: &str, delta: i64) -> Result<i64, String> {
        match self.metrics.get_mut(name) {
            Some(MetricValue::Counter(ref mut v)) => {
                *v += delta;
                Ok(*v)
            }
            Some(other) => Err(format!(
                "'{}' is a {}, not a Counter",
                name,
                match other {
                    MetricValue::Gauge(_) => "Gauge",
                    MetricValue::Label(_) => "Label",
                    _ => "unknown",
                }
            )),
            None => Err(format!("metric '{}' not found", name)),
        }
    }

    pub fn set_datapoint(&mut self, name: String, value: Option<Vec<u8>>) {
        self.datapoints.insert(name, value);
    }

    pub fn get_datapoint(&self, name: &str) -> Option<&Option<Vec<u8>>> {
        self.datapoints.get(name)
    }

    pub fn del_datapoint(&mut self, name: &str) -> bool {
        self.datapoints.remove(name).is_some()
    }
}
