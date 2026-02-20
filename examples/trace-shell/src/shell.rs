use crate::handlers;
use crate::state::SharedState;
use pallas::codec::minicbor::{self, Encoder};
use pallas::network::miniprotocols::ekgmetrics::MetricValue;
use pallas::network::miniprotocols::handshake::{self, n2c};
use pallas::network::miniprotocols::traceobjects::{
    Detail, Severity, TraceObject, TraceTimestamp,
};
use pallas::network::miniprotocols::{
    PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_EKG_METRICS, PROTOCOL_TFWP_TRACE_OBJECTS,
};
use pallas::network::multiplexer::{Bearer, Plexer, RunningPlexer};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::{Hinter, HistoryHinter};
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Connection lifecycle state.
pub enum ConnectionState {
    Disconnected,
    Connected {
        plexer_handle: RunningPlexer,
        task_handles: Vec<tokio::task::JoinHandle<()>>,
        address: String,
    },
}

/// Return the history file path (~/.trace-shell-history or /tmp fallback).
fn history_path() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".trace-shell-history"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/.trace-shell-history"))
}

/// Run the shell with an optional auto-connect on startup.
pub async fn run_shell_with_autoconnect(
    state: SharedState,
    auto_connect_args: Option<Vec<String>>,
    logdir: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = ConnectionState::Disconnected;
    let mut auto_handle: Option<tokio::task::JoinHandle<()>> = None;

    // Auto-connect if requested
    if let Some(args) = auto_connect_args {
        handle_connect(&args, &state, &mut connection).await;
    }

    // Set up rustyline editor with tab completion and history hints
    let mut editor = Editor::new()?;
    editor.set_helper(Some(ShellHelper {
        completer: ShellCompleter,
        hinter: HistoryHinter {},
    }));
    let hist = history_path();
    let _ = editor.load_history(&hist);

    let editor = Arc::new(Mutex::new(editor));

    loop {
        let ed = Arc::clone(&editor);
        let readline = tokio::task::spawn_blocking(move || {
            ed.lock().unwrap().readline("trace> ")
        })
        .await?;

        match readline {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                // Add to history
                if let Ok(ref mut ed) = editor.lock() {
                    let _ = ed.add_history_entry(&line);
                }
                let tokens = tokenize(&line);
                if tokens.is_empty() {
                    continue;
                }
                let cmd = tokens[0].to_lowercase();
                match cmd.as_str() {
                    "connect" => {
                        handle_connect(&tokens[1..], &state, &mut connection).await;
                    }
                    "disconnect" => {
                        if tokens.get(1).map(|s| s.as_str()) == Some("help") {
                            print_help_disconnect();
                        } else {
                            handle_disconnect(&mut connection, &mut auto_handle).await;
                        }
                    }
                    "status" => {
                        if tokens.get(1).map(|s| s.as_str()) == Some("help") {
                            print_help_status();
                        } else {
                            handle_status(&connection, &state, logdir.as_deref()).await;
                        }
                    }
                    "trace" => {
                        handle_trace(&tokens[1..], &state, &mut auto_handle).await;
                    }
                    "metric" => {
                        handle_metric(&tokens[1..], &state).await;
                    }
                    "datapoint" | "dp" => {
                        handle_datapoint(&tokens[1..], &state).await;
                    }
                    "help" => {
                        handle_help(&tokens[1..]);
                    }
                    "quit" | "exit" => {
                        handle_disconnect(&mut connection, &mut auto_handle).await;
                        break;
                    }
                    _ => {
                        println!(
                            "Unknown command: '{}'. Type 'help' for available commands.",
                            cmd
                        );
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("Use 'quit' or 'exit' to leave the shell.");
            }
            Err(ReadlineError::Eof) => {
                handle_disconnect(&mut connection, &mut auto_handle).await;
                break;
            }
            Err(e) => {
                println!("Input error: {}", e);
                break;
            }
        }
    }

    // Save history on exit
    if let Ok(ref mut ed) = editor.lock() {
        let _ = ed.save_history(&hist);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Rustyline completer, hinter, and helper
// ---------------------------------------------------------------------------

struct ShellCompleter;
struct ShellHelper {
    completer: ShellCompleter,
    hinter: HistoryHinter,
}

impl Helper for ShellHelper {}
impl Validator for ShellHelper {}

impl Highlighter for ShellHelper {
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        // Render hints in dim grey
        Cow::Owned(format!("\x1b[90m{}\x1b[0m", hint))
    }
}

impl Hinter for ShellHelper {
    type Hint = String;
    fn hint(&self, line: &str, pos: usize, ctx: &Context<'_>) -> Option<String> {
        self.hinter.hint(line, pos, ctx)
    }
}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        self.completer.complete(line, pos, ctx)
    }
}

// Static candidate lists
const TOP_COMMANDS: &[&str] = &[
    "connect",
    "datapoint",
    "disconnect",
    "dp",
    "exit",
    "help",
    "metric",
    "quit",
    "status",
    "trace",
];
const TRACE_SUBS: &[&str] = &["add", "auto", "clear", "help", "list"];
const METRIC_SUBS: &[&str] = &["clear", "del", "get", "help", "incr", "list", "set"];
const DP_SUBS: &[&str] = &["clear", "del", "get", "help", "list", "nodeinfo", "set"];
const HELP_TOPICS: &[&str] = &[
    "connect",
    "datapoint",
    "disconnect",
    "metric",
    "quit",
    "status",
    "trace",
];
const TRACE_ADD_FLAGS: &[&str] = &[
    "--detail",
    "--hostname",
    "--ns",
    "--severity",
    "--thread",
];
const CONNECT_FLAGS: &[&str] = &["--magic"];
const AUTO_FLAGS: &[&str] = &["--prefix"];
const NODEINFO_FLAGS: &[&str] = &["--commit", "--protocol", "--start-time", "--version"];
const SEVERITIES: &[&str] = &[
    "Alert",
    "Critical",
    "Debug",
    "Emergency",
    "Error",
    "Info",
    "Notice",
    "Warning",
];
const DETAILS: &[&str] = &["Detailed", "Maximum", "Minimal", "Normal"];
const METRIC_TYPES: &[&str] = &["counter", "gauge", "label"];

impl Completer for ShellCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let before = &line[..pos];
        let tokens: Vec<&str> = before.split_whitespace().collect();

        // Are we completing a new word (cursor after whitespace) or a partial word?
        let at_word_boundary = before.is_empty() || before.ends_with(char::is_whitespace);
        let (word_start, partial) = if at_word_boundary {
            (pos, "")
        } else {
            let start = before
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(0);
            (start, &before[start..])
        };

        // The "completed" tokens — those before the partial word being typed
        let completed: &[&str] = if at_word_boundary {
            &tokens[..]
        } else if tokens.len() > 1 {
            &tokens[..tokens.len() - 1]
        } else {
            &[]
        };

        let candidates: &[&str] = match completed {
            // Level 0: top-level command
            [] => TOP_COMMANDS,
            // Level 1: subcommands
            ["trace"] => TRACE_SUBS,
            ["metric"] => METRIC_SUBS,
            ["datapoint" | "dp"] => DP_SUBS,
            ["help"] => HELP_TOPICS,
            // Level 2: context-specific
            ["trace", "auto"] => &["stop"],
            ["metric", "set", _name] => METRIC_TYPES,
            // Flags: check what the previous completed token is
            _ => {
                // If typing a flag (starts with --)
                if partial.starts_with("--") {
                    context_flags(completed)
                }
                // If the previous token was a flag, complete its value
                else if let Some(flag) = completed.last() {
                    match *flag {
                        "--severity" => SEVERITIES,
                        "--detail" => DETAILS,
                        _ => &[],
                    }
                } else {
                    &[]
                }
            }
        };

        let partial_lower = partial.to_lowercase();
        let pairs: Vec<Pair> = candidates
            .iter()
            .filter(|c| c.to_lowercase().starts_with(&partial_lower))
            .map(|c| Pair {
                display: c.to_string(),
                replacement: c.to_string(),
            })
            .collect();

        Ok((word_start, pairs))
    }
}

/// Determine which flags are valid given the completed tokens.
fn context_flags(completed: &[&str]) -> &'static [&'static str] {
    if completed.len() >= 2 {
        match (completed[0], completed[1]) {
            ("trace", "add") => TRACE_ADD_FLAGS,
            ("trace", "auto") => AUTO_FLAGS,
            ("datapoint" | "dp", "nodeinfo") => NODEINFO_FLAGS,
            _ => &[],
        }
    } else {
        match completed.first() {
            Some(&"connect") => CONNECT_FLAGS,
            _ => &[],
        }
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = '"';

    for ch in input.chars() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// ---------------------------------------------------------------------------
// Flag parsing helpers
// ---------------------------------------------------------------------------

fn parse_flag_str<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn parse_flag_u64(args: &[String], flag: &str) -> Option<u64> {
    parse_flag_str(args, flag).and_then(|s| s.parse().ok())
}

// ---------------------------------------------------------------------------
// connect / disconnect / status
// ---------------------------------------------------------------------------

async fn handle_connect(
    args: &[String],
    state: &SharedState,
    connection: &mut ConnectionState,
) {
    if matches!(connection, ConnectionState::Connected { .. }) {
        println!("Already connected. Use 'disconnect' first.");
        return;
    }

    if args.is_empty() {
        println!("Usage: connect <socket-path | tcp:host:port> [--magic N]");
        println!();
        println!("Type 'help connect' or 'connect help' for details.");
        return;
    }

    if args[0].to_lowercase() == "help" {
        print_help_connect();
        return;
    }

    let target = &args[0];
    let magic = parse_flag_u64(args, "--magic").unwrap_or(764824073);

    println!("Connecting to {} (magic={}) ...", target, magic);

    let bearer_result = if let Some(addr) = target.strip_prefix("tcp:") {
        Bearer::connect_tcp(addr).await
    } else {
        Bearer::connect_unix(target).await
    };

    let bearer = match bearer_result {
        Ok(b) => b,
        Err(e) => {
            println!("Connection failed: {}", e);
            return;
        }
    };

    let mut plexer = Plexer::new(bearer);
    let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);
    let ekg_channel = plexer.subscribe_client(PROTOCOL_TFWP_EKG_METRICS);
    let dp_channel = plexer.subscribe_client(PROTOCOL_TFWP_DATAPOINTS);
    let hs_channel = plexer.subscribe_client(0);
    let plexer_handle = plexer.spawn();

    // Handshake
    let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
    let versions = n2c::VersionTable::v1_and_above(magic);
    match hs_client.handshake(versions).await {
        Ok(handshake::Confirmation::Accepted(v, _)) => {
            println!("Handshake accepted (version {}).", v);
        }
        Ok(handshake::Confirmation::Rejected(reason)) => {
            println!("Handshake rejected: {:?}", reason);
            plexer_handle.abort().await;
            return;
        }
        Ok(handshake::Confirmation::QueryReply(_)) => {
            println!("Handshake: unexpected query reply.");
            plexer_handle.abort().await;
            return;
        }
        Err(e) => {
            println!("Handshake error: {}", e);
            plexer_handle.abort().await;
            return;
        }
    }

    // Spawn protocol handlers
    let s1 = state.clone();
    let s2 = state.clone();
    let s3 = state.clone();
    let to_handle = tokio::spawn(handlers::run_traceobjects_server(to_channel, s1));
    let ekg_handle = tokio::spawn(handlers::run_ekg_server(ekg_channel, s2));
    let dp_handle = tokio::spawn(handlers::run_datapoints_server(dp_channel, s3));

    *connection = ConnectionState::Connected {
        plexer_handle,
        task_handles: vec![to_handle, ekg_handle, dp_handle],
        address: target.clone(),
    };

    println!("Connected. Protocol handlers are running.");
    println!("The tracer will now poll for traces, metrics, and datapoints.");
}

async fn handle_disconnect(
    connection: &mut ConnectionState,
    auto_handle: &mut Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(h) = auto_handle.take() {
        h.abort();
        println!("Stopped trace auto-generation.");
    }

    match std::mem::replace(connection, ConnectionState::Disconnected) {
        ConnectionState::Connected {
            plexer_handle,
            task_handles,
            address,
        } => {
            for h in task_handles {
                h.abort();
            }
            plexer_handle.abort().await;
            println!("Disconnected from {}.", address);
        }
        ConnectionState::Disconnected => {
            println!("Not connected.");
        }
    }
}

async fn handle_status(connection: &ConnectionState, state: &SharedState, logdir: Option<&str>) {
    match connection {
        ConnectionState::Connected { address, .. } => {
            println!("Connected to: {}", address);
        }
        ConnectionState::Disconnected => {
            println!("Not connected.");
        }
    }
    let s = state.read().await;
    println!("Trace queue:     {} pending", s.trace_queue.len());
    println!("Traces served:   {}", s.traces_served);
    println!("Metrics stored:  {}", s.metrics.len());
    println!("EKG polls:       {}", s.ekg_polls);
    println!("Datapoints:      {}", s.datapoints.len());
    println!("DP polls:        {}", s.dp_polls);

    if let Some(dir) = logdir {
        println!();
        println!("Log directory:   {}", dir);
        let path = std::path::Path::new(dir);
        if !path.exists() {
            println!("  (directory does not exist yet)");
        } else {
            match list_log_files(path) {
                files if files.is_empty() => {
                    println!("  (no log files yet — send some trace objects first)");
                }
                files => {
                    for (rel_path, size) in &files {
                        println!("  {} ({} bytes)", rel_path, size);
                    }
                }
            }
        }
    }
}

/// Scan a log directory tree for node-*.json / node-*.log files.
fn list_log_files(root: &std::path::Path) -> Vec<(String, u64)> {
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Look for log files inside node subdirectories
            if let Ok(sub) = std::fs::read_dir(&path) {
                for sub_entry in sub.flatten() {
                    let sub_path = sub_entry.path();
                    if let Some(name) = sub_path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("node-") && (name.ends_with(".json") || name.ends_with(".log")) {
                            let size = sub_path.metadata().map(|m| m.len()).unwrap_or(0);
                            let rel = format!(
                                "{}/{}",
                                path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                                name
                            );
                            results.push((rel, size));
                        }
                    }
                }
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            // Log files directly in root (less common)
            if name.starts_with("node-") && (name.ends_with(".json") || name.ends_with(".log")) {
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                results.push((name.to_string(), size));
            }
        }
    }
    results.sort();
    results
}

// ---------------------------------------------------------------------------
// trace subcommands
// ---------------------------------------------------------------------------

async fn handle_trace(
    args: &[String],
    state: &SharedState,
    auto_handle: &mut Option<tokio::task::JoinHandle<()>>,
) {
    if args.is_empty() {
        println!("Usage: trace <add|list|clear|auto|help> [args...]");
        println!();
        println!("Type 'help trace' or 'trace help' for details.");
        return;
    }

    match args[0].to_lowercase().as_str() {
        "add" => trace_add(&args[1..], state).await,
        "list" => trace_list(state).await,
        "clear" => trace_clear(state).await,
        "auto" => trace_auto(&args[1..], state, auto_handle).await,
        "help" => print_help_trace(),
        other => {
            println!(
                "Unknown trace subcommand: '{}'. Expected: add, list, clear, auto, help.",
                other
            );
        }
    }
}

async fn trace_add(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: trace add <message> [--severity S] [--detail D] [--ns A.B.C] [--hostname H] [--thread T]");
        println!();
        println!("Type 'help trace' or 'trace help' for details.");
        return;
    }

    // First positional arg is the message
    let message = &args[0];

    let severity = parse_flag_str(args, "--severity")
        .and_then(parse_severity)
        .unwrap_or(Severity::Info);

    let detail = parse_flag_str(args, "--detail")
        .and_then(parse_detail)
        .unwrap_or(Detail::Normal);

    let namespace: Vec<String> = parse_flag_str(args, "--ns")
        .map(|s| s.split('.').map(|p| p.to_string()).collect())
        .unwrap_or_else(|| vec!["Shell".to_string()]);

    let hostname = parse_flag_str(args, "--hostname")
        .unwrap_or("trace-shell")
        .to_string();

    let thread_id = parse_flag_str(args, "--thread")
        .unwrap_or("0")
        .to_string();

    let timestamp = now_timestamp();

    // Encode to_machine as CBOR string
    let to_machine = {
        let mut buf = Vec::new();
        minicbor::encode(message.as_str(), &mut buf).unwrap();
        minicbor::decode(&buf).unwrap()
    };

    let obj = TraceObject {
        kind: None,
        to_human: Some(message.clone()),
        to_machine,
        to_namespace: namespace.clone(),
        severity: severity.clone(),
        detail,
        timestamp,
        hostname,
        thread_id,
    };

    let mut s = state.write().await;
    s.enqueue_trace(obj);
    let pending = s.trace_queue.len();
    println!(
        "Queued trace: [{:?}] {} (ns={}, {} pending)",
        severity,
        message,
        namespace.join("."),
        pending
    );
}

async fn trace_list(state: &SharedState) {
    let s = state.read().await;
    if s.trace_queue.is_empty() {
        println!("Trace queue is empty.");
        return;
    }
    println!("Trace queue ({} pending):", s.trace_queue.len());
    for (i, obj) in s.trace_queue.iter().enumerate() {
        let human = obj
            .to_human
            .as_deref()
            .unwrap_or("<no human message>");
        println!(
            "  [{}] {:?} {} ns={}",
            i,
            obj.severity,
            human,
            obj.to_namespace.join(".")
        );
    }
}

async fn trace_clear(state: &SharedState) {
    let mut s = state.write().await;
    let count = s.trace_queue.len();
    s.trace_queue.clear();
    println!("Cleared {} trace objects from queue.", count);
}

async fn trace_auto(
    args: &[String],
    state: &SharedState,
    auto_handle: &mut Option<tokio::task::JoinHandle<()>>,
) {
    if args.is_empty() {
        println!("Usage: trace auto <interval-ms> [--prefix P]");
        println!("       trace auto stop");
        println!();
        println!("Type 'help trace' or 'trace help' for details.");
        return;
    }

    if args[0].to_lowercase() == "stop" {
        if let Some(h) = auto_handle.take() {
            h.abort();
            println!("Stopped trace auto-generation.");
        } else {
            println!("Auto-generation is not running.");
        }
        return;
    }

    let interval_ms: u64 = match args[0].parse() {
        Ok(v) if v > 0 => v,
        _ => {
            println!("Invalid interval. Must be a positive integer (milliseconds).");
            return;
        }
    };

    let prefix = parse_flag_str(args, "--prefix")
        .unwrap_or("auto")
        .to_string();

    if let Some(h) = auto_handle.take() {
        h.abort();
        println!("Stopped previous auto-generation.");
    }

    let prefix_display = prefix.clone();
    let state_clone = state.clone();
    *auto_handle = Some(tokio::spawn(async move {
        let mut counter: u64 = 0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
            counter += 1;
            let message = format!("{}-{}", prefix, counter);
            let to_machine = {
                let mut buf = Vec::new();
                minicbor::encode(message.as_str(), &mut buf).unwrap();
                minicbor::decode(&buf).unwrap()
            };
            let obj = TraceObject {
                kind: None,
                to_human: Some(message),
                to_machine,
                to_namespace: vec!["Shell".to_string(), "Auto".to_string()],
                severity: Severity::Info,
                detail: Detail::Normal,
                timestamp: now_timestamp(),
                hostname: "trace-shell".to_string(),
                thread_id: "auto".to_string(),
            };
            state_clone.write().await.enqueue_trace(obj);
        }
    }));

    println!(
        "Auto-generating traces every {}ms with prefix '{}'.",
        interval_ms, prefix_display
    );
    println!("Use 'trace auto stop' to stop.");
}

// ---------------------------------------------------------------------------
// metric subcommands
// ---------------------------------------------------------------------------

async fn handle_metric(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: metric <set|get|list|del|clear|incr|help> [args...]");
        println!();
        println!("Type 'help metric' or 'metric help' for details.");
        return;
    }

    match args[0].to_lowercase().as_str() {
        "set" => metric_set(&args[1..], state).await,
        "get" => metric_get(&args[1..], state).await,
        "list" => metric_list(state).await,
        "del" | "delete" | "rm" => metric_del(&args[1..], state).await,
        "clear" => metric_clear(state).await,
        "incr" | "increment" => metric_incr(&args[1..], state).await,
        "help" => print_help_metric(),
        other => {
            println!(
                "Unknown metric subcommand: '{}'. Expected: set, get, list, del, clear, incr, help.",
                other
            );
        }
    }
}

async fn metric_set(args: &[String], state: &SharedState) {
    if args.len() < 3 {
        println!("Usage: metric set <name> counter|gauge|label <value>");
        println!();
        println!("Examples:");
        println!("  metric set rts.gc.bytes_allocated counter 1024");
        println!("  metric set node.peers gauge 7");
        println!("  metric set version label \"1.35.7\"");
        return;
    }

    let name = &args[0];
    let kind = args[1].to_lowercase();
    let raw_value = &args[2];

    let value = match kind.as_str() {
        "counter" => match raw_value.parse::<i64>() {
            Ok(v) => MetricValue::Counter(v),
            Err(_) => {
                println!("Invalid counter value: '{}'. Must be an integer.", raw_value);
                return;
            }
        },
        "gauge" => match raw_value.parse::<i64>() {
            Ok(v) => MetricValue::Gauge(v),
            Err(_) => {
                println!("Invalid gauge value: '{}'. Must be an integer.", raw_value);
                return;
            }
        },
        "label" => MetricValue::Label(raw_value.clone()),
        other => {
            println!(
                "Unknown metric type: '{}'. Expected: counter, gauge, label.",
                other
            );
            return;
        }
    };

    state.write().await.set_metric(name.clone(), value.clone());
    println!("Set metric '{}' = {:?}", name, value);
}

async fn metric_get(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: metric get <name>");
        return;
    }
    let name = &args[0];
    let s = state.read().await;
    match s.get_metric(name) {
        Some(v) => println!("{} = {:?}", name, v),
        None => println!("Metric '{}' not found.", name),
    }
}

async fn metric_list(state: &SharedState) {
    let s = state.read().await;
    if s.metrics.is_empty() {
        println!("No metrics stored.");
        return;
    }
    println!("Metrics ({}):", s.metrics.len());
    let mut names: Vec<&String> = s.metrics.keys().collect();
    names.sort();
    for name in names {
        println!("  {} = {:?}", name, s.metrics[name]);
    }
}

async fn metric_del(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: metric del <name>");
        return;
    }
    let name = &args[0];
    if state.write().await.del_metric(name) {
        println!("Deleted metric '{}'.", name);
    } else {
        println!("Metric '{}' not found.", name);
    }
}

async fn metric_clear(state: &SharedState) {
    let mut s = state.write().await;
    let count = s.metrics.len();
    s.metrics.clear();
    println!("Cleared {} metrics.", count);
}

async fn metric_incr(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: metric incr <name> [delta]");
        println!();
        println!("Increments a Counter metric by delta (default 1).");
        return;
    }
    let name = &args[0];
    let delta: i64 = if args.len() > 1 {
        match args[1].parse() {
            Ok(v) => v,
            Err(_) => {
                println!("Invalid delta: '{}'. Must be an integer.", args[1]);
                return;
            }
        }
    } else {
        1
    };

    match state.write().await.incr_metric(name, delta) {
        Ok(new_val) => println!("{} incremented by {} -> {}", name, delta, new_val),
        Err(msg) => println!("Error: {}", msg),
    }
}

// ---------------------------------------------------------------------------
// datapoint subcommands
// ---------------------------------------------------------------------------

async fn handle_datapoint(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: datapoint <set|get|list|del|clear|nodeinfo|help> [args...]");
        println!();
        println!("Type 'help datapoint' or 'datapoint help' for details.");
        return;
    }

    match args[0].to_lowercase().as_str() {
        "set" => datapoint_set(&args[1..], state).await,
        "get" => datapoint_get(&args[1..], state).await,
        "list" => datapoint_list(state).await,
        "del" | "delete" | "rm" => datapoint_del(&args[1..], state).await,
        "clear" => datapoint_clear(state).await,
        "nodeinfo" => datapoint_nodeinfo(&args[1..], state).await,
        "help" => print_help_datapoint(),
        other => {
            println!(
                "Unknown datapoint subcommand: '{}'. Expected: set, get, list, del, clear, nodeinfo, help.",
                other
            );
        }
    }
}

async fn datapoint_set(args: &[String], state: &SharedState) {
    if args.len() < 2 {
        println!("Usage: datapoint set <name> <value>");
        println!();
        println!("The value is stored as UTF-8 bytes. Use 'datapoint nodeinfo' for");
        println!("the standard NodeInfo datapoint with CBOR encoding.");
        return;
    }
    let name = &args[0];
    let value = args[1..].join(" ");
    state
        .write()
        .await
        .set_datapoint(name.clone(), Some(value.as_bytes().to_vec()));
    println!("Set datapoint '{}' = \"{}\"", name, value);
}

async fn datapoint_get(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: datapoint get <name>");
        return;
    }
    let name = &args[0];
    let s = state.read().await;
    match s.get_datapoint(name) {
        Some(Some(bytes)) => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                println!("{} = \"{}\"", name, text);
            } else {
                println!("{} = <{} bytes, non-UTF8>", name, bytes.len());
            }
        }
        Some(None) => println!("{} = <empty>", name),
        None => println!("Datapoint '{}' not found.", name),
    }
}

async fn datapoint_list(state: &SharedState) {
    let s = state.read().await;
    if s.datapoints.is_empty() {
        println!("No datapoints stored.");
        return;
    }
    println!("Datapoints ({}):", s.datapoints.len());
    let mut names: Vec<&String> = s.datapoints.keys().collect();
    names.sort();
    for name in names {
        let val = &s.datapoints[name];
        match val {
            Some(bytes) => {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    let display = if text.len() > 60 {
                        format!("{}...", &text[..57])
                    } else {
                        text.to_string()
                    };
                    println!("  {} = \"{}\"", name, display);
                } else {
                    println!("  {} = <{} bytes>", name, bytes.len());
                }
            }
            None => println!("  {} = <empty>", name),
        }
    }
}

async fn datapoint_del(args: &[String], state: &SharedState) {
    if args.is_empty() {
        println!("Usage: datapoint del <name>");
        return;
    }
    let name = &args[0];
    if state.write().await.del_datapoint(name) {
        println!("Deleted datapoint '{}'.", name);
    } else {
        println!("Datapoint '{}' not found.", name);
    }
}

async fn datapoint_clear(state: &SharedState) {
    let mut s = state.write().await;
    let count = s.datapoints.len();
    s.datapoints.clear();
    println!("Cleared {} datapoints.", count);
}

async fn datapoint_nodeinfo(args: &[String], state: &SharedState) {
    let version = parse_flag_str(args, "--version")
        .unwrap_or("1.35.7")
        .to_string();
    let commit = parse_flag_str(args, "--commit")
        .unwrap_or("00000000")
        .to_string();
    let start_time = parse_flag_str(args, "--start-time")
        .unwrap_or("2024-01-01 00:00:00 UTC")
        .to_string();
    let protocol = parse_flag_str(args, "--protocol")
        .unwrap_or("Shelley")
        .to_string();

    // Encode as CBOR map matching NodeInfo pattern from trace-miniprotocols
    let mut buf = Vec::new();
    let mut encoder = Encoder::new(&mut buf);
    if encoder
        .map(4)
        .and_then(|e| e.str("nodeVersion"))
        .and_then(|e| e.str(&version))
        .and_then(|e| e.str("nodeCommit"))
        .and_then(|e| e.str(&commit))
        .and_then(|e| e.str("nodeStartTime"))
        .and_then(|e| e.str(&start_time))
        .and_then(|e| e.str("protocol"))
        .and_then(|e| e.str(&protocol))
        .is_err()
    {
        println!("Failed to encode NodeInfo CBOR.");
        return;
    }

    state
        .write()
        .await
        .set_datapoint("NodeInfo".to_string(), Some(buf));
    println!(
        "Set NodeInfo datapoint: version={}, commit={}, protocol={}",
        version, commit, protocol
    );
}

// ---------------------------------------------------------------------------
// help system
// ---------------------------------------------------------------------------

fn handle_help(args: &[String]) {
    if args.is_empty() {
        print_general_help();
    } else {
        match args[0].to_lowercase().as_str() {
            "connect" => print_help_connect(),
            "disconnect" => print_help_disconnect(),
            "status" => print_help_status(),
            "trace" => print_help_trace(),
            "metric" => print_help_metric(),
            "datapoint" | "dp" => print_help_datapoint(),
            "quit" | "exit" => print_help_quit(),
            other => println!("No help available for '{}'. Type 'help' for command list.", other),
        }
    }
}

fn print_general_help() {
    println!(
        "\
trace-shell: Interactive trace-forward protocol shell

  Connects to a cardano-tracer as a simulated node and lets you craft
  custom trace objects, metrics, and datapoints via a command language.
  The tracer polls the shell for data; you control what it receives.

COMMANDS:

  Connection:
    connect <target> [--magic N]   Connect to a tracer
    disconnect                     Disconnect from tracer
    status                         Show connection state and statistics

  Trace Objects:
    trace add <msg> [flags]        Queue a trace object
    trace list                     Show queued trace objects
    trace clear                    Clear the trace queue
    trace auto <ms> [--prefix P]   Auto-generate traces at interval
    trace auto stop                Stop auto-generation

  EKG Metrics:
    metric set <name> <type> <val> Set a metric (counter|gauge|label)
    metric get <name>              Show a specific metric
    metric list                    List all metrics
    metric del <name>              Delete a metric
    metric clear                   Clear all metrics
    metric incr <name> [delta]     Increment a counter

  Datapoints:
    datapoint set <name> <value>   Set a datapoint (UTF-8)
    datapoint get <name>           Show a specific datapoint
    datapoint list                 List all datapoints
    datapoint del <name>           Delete a datapoint
    datapoint clear                Clear all datapoints
    datapoint nodeinfo [flags]     Set standard NodeInfo datapoint

  General:
    help                           Show this help
    help <command>                 Detailed help for a command
    quit / exit                    Disconnect and exit

  Aliases: 'dp' = 'datapoint', 'del'/'rm' = 'delete'

Type 'help <command>' for detailed usage, flags, and examples.
Help is also available as a subcommand: 'trace help', 'metric help', etc."
    );
}

fn print_help_connect() {
    println!(
        "\
CONNECT — Connect to a tracer as a simulated node

SYNTAX:
  connect <socket-path>          Connect via Unix domain socket
  connect tcp:<host>:<port>      Connect via TCP
  connect <target> --magic N     Specify network magic (default: 764824073 = mainnet)

DESCRIPTION:
  Establishes a multiplexed connection to a trace-forward server (e.g.,
  cardano-tracer). Performs a handshake proposing ForwardingV_1 and V_2,
  then spawns background tasks that serve TraceObjects, EKG Metrics, and
  Datapoints protocol requests from the tracer.

  The shell acts as a Mux Initiator running protocol Servers (Responder
  role), matching the behavior of a real cardano-node.

  Only one connection can be active at a time. Use 'disconnect' first
  if you need to reconnect.

FLAGS:
  --magic N    Network magic for handshake. Common values:
               764824073 = mainnet (default)
               1         = preprod
               2         = preview
               42        = local testnet

EXAMPLES:
  connect /tmp/tracer.sock
  connect /run/cardano/tracer.socket --magic 42
  connect tcp:127.0.0.1:3002"
    );
}

fn print_help_disconnect() {
    println!(
        "\
DISCONNECT — Disconnect from the tracer

SYNTAX:
  disconnect

DESCRIPTION:
  Aborts all background protocol handler tasks, closes the multiplexer,
  and returns to disconnected state. Also stops trace auto-generation
  if it was running.

  The shared state (metrics, datapoints, trace queue) is preserved
  after disconnecting. You can reconnect and the tracer will see the
  same data."
    );
}

fn print_help_status() {
    println!(
        "\
STATUS — Show connection state and statistics

SYNTAX:
  status

DESCRIPTION:
  Displays the current connection state (connected/disconnected with
  address), sizes of internal data stores, and cumulative poll counters:

  - Trace queue:    Number of trace objects waiting to be served
  - Traces served:  Total trace objects sent to tracer since session start
  - Metrics stored: Number of EKG metrics in the store
  - EKG polls:      Number of metric requests received from tracer
  - Datapoints:     Number of datapoints in the store
  - DP polls:       Number of datapoint requests received from tracer

  If the shell was started with --logdir, also shows the tracer's log
  directory and lists any log files found there with their sizes.
  Log files are created by the tracer when trace objects arrive."
    );
}

fn print_help_trace() {
    println!(
        "\
TRACE — Manage trace objects

  The tracer periodically polls for trace objects via the TraceObjects
  protocol (Request with a batch limit). Objects are served in FIFO
  order from an internal queue. You add objects to the queue; the
  tracer drains them when it polls.

SUBCOMMANDS:

  trace add <message> [flags]
    Queue a trace object with the given human-readable message.

    FLAGS:
      --severity S    Severity level (default: Info)
                      Values: Debug, Info, Notice, Warning, Error,
                              Critical, Alert, Emergency
      --detail D      Detail level (default: Normal)
                      Values: Minimal, Normal, Detailed, Maximum
      --ns A.B.C      Dot-separated namespace (default: Shell)
      --hostname H    Hostname field (default: trace-shell)
      --thread T      Thread ID field (default: 0)

    The to_machine field is set to a CBOR-encoded copy of the message.
    Timestamp is set to the current system time.

    EXAMPLES:
      trace add \"Block forged successfully\"
      trace add \"Peer disconnected\" --severity Warning --ns Node.Network
      trace add \"Test message\" --severity Debug --detail Maximum

  trace list
    Show all queued trace objects with index, severity, message, and
    namespace.

  trace clear
    Remove all trace objects from the queue. Prints how many were cleared.

  trace auto <interval-ms> [--prefix P]
    Start auto-generating trace objects at the given interval.
    Each object has message \"<prefix>-<N>\" where N increments.
    Default prefix: \"auto\". Useful for load testing.

    EXAMPLES:
      trace auto 100                 Generate every 100ms
      trace auto 1000 --prefix load  Generate every 1s with prefix \"load\"

  trace auto stop
    Stop auto-generation."
    );
}

fn print_help_metric() {
    println!(
        "\
METRIC — Manage EKG metrics

  Metrics are served when the tracer polls via the EKG Metrics protocol.
  Unlike trace objects (which are transient and drained from a queue),
  metrics are persistent key-value pairs that persist until explicitly
  changed or deleted.

  Three metric types are supported, matching the Haskell EKG library:
    counter  - Monotonically increasing integer (e.g., bytes_allocated)
    gauge    - Current integer value (e.g., peer_count)
    label    - String value (e.g., version)

SUBCOMMANDS:

  metric set <name> counter|gauge|label <value>
    Set a metric. Creates it if new, overwrites if existing.

    EXAMPLES:
      metric set rts.gc.bytes_allocated counter 102400
      metric set node.peers gauge 7
      metric set version label \"1.35.7\"
      metric set node.slot gauge 12345678

  metric get <name>
    Display a single metric's type and value.

  metric list
    List all metrics sorted by name.

  metric del <name>
    Delete a metric. The tracer will no longer see it on next poll.

  metric clear
    Delete all metrics.

  metric incr <name> [delta]
    Increment a Counter metric by delta (default: 1). Errors if the
    metric does not exist or is not a Counter.

    EXAMPLES:
      metric incr rts.gc.bytes_allocated 512
      metric incr request_count"
    );
}

fn print_help_datapoint() {
    println!(
        "\
DATAPOINT — Manage datapoints

  Datapoints are named values served when the tracer requests them by
  name via the Datapoints protocol. Like metrics, they persist until
  changed or deleted.

  Values are stored as raw bytes. Plain strings are stored as UTF-8.
  The 'nodeinfo' subcommand creates a standard NodeInfo datapoint with
  proper CBOR encoding.

  Alias: 'dp' can be used instead of 'datapoint'.

SUBCOMMANDS:

  datapoint set <name> <value>
    Set a datapoint. The value (and any remaining tokens) are joined
    and stored as UTF-8 bytes.

    EXAMPLES:
      datapoint set test.point \"hello world\"
      datapoint set my.config {{\"key\": \"value\"}}

  datapoint get <name>
    Display a datapoint's value. Shows as string if valid UTF-8,
    otherwise shows byte count.

  datapoint list
    List all datapoints with truncated values.

  datapoint del <name>
    Delete a datapoint.

  datapoint clear
    Delete all datapoints.

  datapoint nodeinfo [flags]
    Set the standard NodeInfo datapoint as a CBOR-encoded map with
    fields matching what cardano-tracer expects.

    FLAGS:
      --version V      Node version (default: 1.35.7)
      --commit C       Git commit hash (default: 00000000)
      --start-time T   Node start time (default: 2024-01-01 00:00:00 UTC)
      --protocol P     Protocol name (default: Shelley)

    EXAMPLES:
      datapoint nodeinfo
      datapoint nodeinfo --version 10.1.4 --commit abc123"
    );
}

fn print_help_quit() {
    println!(
        "\
QUIT / EXIT — Disconnect and exit

SYNTAX:
  quit
  exit

DESCRIPTION:
  Disconnects from the tracer (if connected), stops auto-generation,
  and exits the shell."
    );
}

// ---------------------------------------------------------------------------
// parsing helpers
// ---------------------------------------------------------------------------

fn parse_severity(s: &str) -> Option<Severity> {
    match s.to_lowercase().as_str() {
        "debug" => Some(Severity::Debug),
        "info" => Some(Severity::Info),
        "notice" => Some(Severity::Notice),
        "warning" | "warn" => Some(Severity::Warning),
        "error" | "err" => Some(Severity::Error),
        "critical" | "crit" => Some(Severity::Critical),
        "alert" => Some(Severity::Alert),
        "emergency" | "emerg" => Some(Severity::Emergency),
        _ => {
            println!(
                "Unknown severity: '{}'. Valid: Debug, Info, Notice, Warning, Error, Critical, Alert, Emergency.",
                s
            );
            None
        }
    }
}

fn parse_detail(s: &str) -> Option<Detail> {
    match s.to_lowercase().as_str() {
        "minimal" | "min" => Some(Detail::Minimal),
        "normal" => Some(Detail::Normal),
        "detailed" => Some(Detail::Detailed),
        "maximum" | "max" => Some(Detail::Maximum),
        _ => {
            println!(
                "Unknown detail level: '{}'. Valid: Minimal, Normal, Detailed, Maximum.",
                s
            );
            None
        }
    }
}

fn now_timestamp() -> TraceTimestamp {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    TraceTimestamp::Tag1000 {
        seconds: now.as_secs() as i64,
        pico: (now.subsec_nanos() as i64) * 1000,
    }
}
