use pallas::codec::minicbor::Encoder;
use pallas::codec::utils::AnyCbor;
use pallas::network::miniprotocols::handshake::n2c;
use pallas::network::miniprotocols::{
    handshake, traceobjects, PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_EKG_METRICS,
    PROTOCOL_TFWP_TRACE_OBJECTS,
};
use pallas::network::multiplexer::{Bearer, Plexer};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::time::timeout;

/// Path to the cardano-tracer binary (built in debug mode).
fn tracer_binary() -> PathBuf {
    // Integration tests run from the package directory.
    // The target directory is at the workspace root.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    workspace_root.join("target/debug/cardano-tracer")
}

/// Write a minimal AcceptAt config YAML to a temp directory and return the path.
fn write_config(dir: &std::path::Path, socket_name: &str) -> PathBuf {
    let socket_path = dir.join(socket_name);
    let logs_dir = dir.join("logs");
    let config_path = dir.join("config.yaml");
    let yaml = format!(
        r#"networkMagic: 42
network:
  tag: AcceptAt
  contents: "{}"
logging:
- logRoot: "{}"
  logMode: FileMode
  logFormat: ForMachine
"#,
        socket_path.display(),
        logs_dir.display()
    );
    std::fs::write(&config_path, yaml).unwrap();
    config_path
}

/// Helper: encode a CBOR text string.
fn cbor_string(s: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut e = Encoder::new(&mut buf);
    e.str(s).unwrap();
    buf
}

/// Create an AnyCbor from raw CBOR bytes.
fn any_cbor_from_raw(bytes: &[u8]) -> AnyCbor {
    pallas::codec::minicbor::decode(bytes).unwrap()
}

/// Build a simple TraceObject with a JSON log line as to_machine.
fn make_test_trace_object(json_line: &str) -> traceobjects::TraceObject {
    let to_machine_bytes = cbor_string(json_line);
    traceobjects::TraceObject {
        kind: None,
        to_human: Some("Test message".to_string()),
        to_machine: any_cbor_from_raw(&to_machine_bytes),
        to_namespace: vec!["Test".to_string()],
        severity: traceobjects::Severity::Info,
        detail: traceobjects::Detail::Normal,
        timestamp: traceobjects::TraceTimestamp::Tag1000 {
            seconds: 1700000000,
            pico: 0,
        },
        hostname: "testhost".to_string(),
        thread_id: "1".to_string(),
    }
}

/// Spawn cardano-tracer with the given config, wait for it to start listening.
fn spawn_tracer(config_path: &std::path::Path) -> std::process::Child {
    Command::new(tracer_binary())
        .arg("--config")
        .arg(config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn cardano-tracer")
}

/// Wait for the unix socket to appear (with timeout).
async fn wait_for_socket(socket_path: &std::path::Path, timeout_secs: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    while tokio::time::Instant::now() < deadline {
        if socket_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "Timeout waiting for socket {:?} to appear",
        socket_path
    );
}

#[tokio::test]
async fn test_smoke_start_and_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(dir.path(), "tracer.sock");
    let socket_path = dir.path().join("tracer.sock");

    let mut child = spawn_tracer(&config_path);

    // Wait for socket to appear.
    wait_for_socket(&socket_path, 5).await;
    assert!(socket_path.exists());

    // Kill the process.
    child.kill().ok();
    let status = child.wait().unwrap();
    // Process should have been killed (not a crash).
    assert!(!status.success() || status.code().is_none());
}

#[tokio::test]
async fn test_handshake_and_trace_objects() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(dir.path(), "tracer.sock");
    let socket_path = dir.path().join("tracer.sock");
    let logs_dir = dir.path().join("logs");

    let mut child = spawn_tracer(&config_path);
    wait_for_socket(&socket_path, 5).await;

    // Connect as a node.
    let stream = tokio::net::UnixStream::connect(&socket_path)
        .await
        .expect("Failed to connect to tracer");
    let bearer = Bearer::Unix(stream);
    let mut plexer = Plexer::new(bearer);

    // We are the mux initiator (node); tracer is the responder.
    let hs_channel = plexer.subscribe_client(0);
    let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);
    let _ekg_channel = plexer.subscribe_client(PROTOCOL_TFWP_EKG_METRICS);
    let _dp_channel = plexer.subscribe_client(PROTOCOL_TFWP_DATAPOINTS);
    let plexer_handle = plexer.spawn();

    // Handshake: we are the client, tracer is the server.
    let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
    let v1_data = n2c::VersionData::new(42, None);
    let mut values = std::collections::HashMap::new();
    values.insert(1, v1_data);
    let versions = n2c::VersionTable { values };

    timeout(Duration::from_secs(5), hs_client.handshake(versions))
        .await
        .expect("Handshake timeout")
        .expect("Handshake failed");

    // TraceObjects: we act as the "server" (node side) of the trace-forward
    // protocol. The tracer sends us Requests, we send Responses.
    let mut to_server = traceobjects::Server::new(to_channel);

    // Receive a request from the tracer.
    let req = timeout(Duration::from_secs(5), to_server.recv_request())
        .await
        .expect("Timeout receiving TraceObjects request")
        .expect("Failed to receive request");

    assert!(req.is_some(), "Expected a TraceObjects request");
    let (blocking, _n) = req.unwrap();
    assert!(blocking, "Tracer should send blocking=true requests");

    // Send a response with a trace object containing a JSON log line.
    let json_line = r#"{"at":"2024-01-15T14:30:00.000Z","ns":["Test"],"data":{"kind":"TestMsg","value":"hello"},"sev":"Info","thread":"1","host":"testhost"}"#;
    let obj = make_test_trace_object(json_line);
    timeout(
        Duration::from_secs(5),
        to_server.send_response(vec![obj]),
    )
    .await
    .expect("Timeout sending TraceObjects response")
    .expect("Failed to send response");

    // Give the tracer a moment to write the log.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify log file was created with the expected content.
    let log_files: Vec<PathBuf> = glob::glob(&format!("{}/**/node-*.json", logs_dir.display()))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(
        !log_files.is_empty(),
        "Expected at least one log file in {:?}",
        logs_dir
    );

    let content = std::fs::read_to_string(&log_files[0]).unwrap();
    let lines: Vec<&str> = content.lines().collect();

    // First line should be empty (critical compatibility requirement).
    assert!(
        lines.len() >= 2,
        "Expected at least 2 lines (empty + log), got {}",
        lines.len()
    );
    assert_eq!(lines[0], "", "First line must be empty");

    // Second line should be the JSON log line verbatim.
    assert_eq!(lines[1], json_line, "Log line should match sent JSON exactly");

    // Symlink should exist.
    let symlink_files: Vec<PathBuf> = glob::glob(&format!("{}/**/node.json", logs_dir.display()))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(!symlink_files.is_empty(), "Symlink node.json should exist");

    // Cleanup.
    plexer_handle.abort().await;
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
async fn test_log_output_format_matches_haskell() {
    // Verify that the exact JSON format produced by trace-dispatcher is
    // preserved verbatim in the log file (critical for Haskell test compat).
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(dir.path(), "tracer.sock");
    let socket_path = dir.path().join("tracer.sock");
    let logs_dir = dir.path().join("logs");

    let mut child = spawn_tracer(&config_path);
    wait_for_socket(&socket_path, 5).await;

    let stream = tokio::net::UnixStream::connect(&socket_path)
        .await
        .unwrap();
    let bearer = Bearer::Unix(stream);
    let mut plexer = Plexer::new(bearer);

    let hs_channel = plexer.subscribe_client(0);
    let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);
    let _ekg = plexer.subscribe_client(PROTOCOL_TFWP_EKG_METRICS);
    let _dp = plexer.subscribe_client(PROTOCOL_TFWP_DATAPOINTS);
    let plexer_handle = plexer.spawn();

    let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
    let v1_data = n2c::VersionData::new(42, None);
    let mut values = std::collections::HashMap::new();
    values.insert(1, v1_data);
    let versions = n2c::VersionTable { values };
    timeout(Duration::from_secs(5), hs_client.handshake(versions))
        .await
        .unwrap()
        .unwrap();

    let mut to_server = traceobjects::Server::new(to_channel);
    let _req = timeout(Duration::from_secs(5), to_server.recv_request())
        .await
        .unwrap()
        .unwrap();

    // Send 3 messages matching the ForwardingStressTest format.
    let messages = vec![
        r#"{"at":"2024-01-15T14:30:00.000000000000Z","ns":["Test"],"data":{"kind":"Message1","mid":"1","workload":"42"},"sev":"Info","thread":"1","host":"test"}"#,
        r#"{"at":"2024-01-15T14:30:00.000000000001Z","ns":["Test"],"data":{"kind":"Message2","mid":"2","workload":"hello"},"sev":"Debug","thread":"1","host":"test"}"#,
        r#"{"at":"2024-01-15T14:30:00.000000000002Z","ns":["Test"],"data":{"kind":"Message3","mid":"3","workload":"3.14"},"sev":"Error","thread":"1","host":"test"}"#,
    ];

    let objs: Vec<traceobjects::TraceObject> = messages
        .iter()
        .map(|m| make_test_trace_object(m))
        .collect();

    timeout(
        Duration::from_secs(5),
        to_server.send_response(objs),
    )
    .await
    .unwrap()
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify log file content.
    let log_files: Vec<PathBuf> = glob::glob(&format!("{}/**/node-*.json", logs_dir.display()))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(!log_files.is_empty());

    let content = std::fs::read_to_string(&log_files[0]).unwrap();
    let lines: Vec<&str> = content.lines().collect();

    // 1 empty line + 3 messages = 4 lines
    assert_eq!(
        lines.len(),
        4,
        "Expected 4 lines (1 empty + 3 messages), got {}",
        lines.len()
    );

    // Each message line should be verbatim JSON.
    for (i, expected) in messages.iter().enumerate() {
        assert_eq!(
            lines[i + 1], *expected,
            "Message {} doesn't match",
            i + 1
        );

        // Verify each line is valid JSON with expected structure.
        let parsed: serde_json::Value = serde_json::from_str(lines[i + 1]).unwrap();
        assert!(parsed.get("data").is_some());
        assert!(parsed["data"].get("kind").is_some());
    }

    plexer_handle.abort().await;
    child.kill().ok();
    child.wait().ok();
}

#[tokio::test]
async fn test_multiple_connections() {
    // Two sequential connections should create two node subdirectories.
    let dir = tempfile::tempdir().unwrap();
    let config_path = write_config(dir.path(), "tracer.sock");
    let socket_path = dir.path().join("tracer.sock");
    let logs_dir = dir.path().join("logs");

    let mut child = spawn_tracer(&config_path);
    wait_for_socket(&socket_path, 5).await;

    // Connect, handshake, send one trace, and disconnect — twice.
    for _ in 0..2 {
        let stream = tokio::net::UnixStream::connect(&socket_path)
            .await
            .unwrap();
        let bearer = Bearer::Unix(stream);
        let mut plexer = Plexer::new(bearer);

        let hs_channel = plexer.subscribe_client(0);
        let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);
        let _ekg = plexer.subscribe_client(PROTOCOL_TFWP_EKG_METRICS);
        let _dp = plexer.subscribe_client(PROTOCOL_TFWP_DATAPOINTS);
        let plexer_handle = plexer.spawn();

        let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
        let v1_data = n2c::VersionData::new(42, None);
        let mut values = std::collections::HashMap::new();
        values.insert(1, v1_data);
        let versions = n2c::VersionTable { values };
        timeout(Duration::from_secs(5), hs_client.handshake(versions))
            .await
            .unwrap()
            .unwrap();

        let mut to_server = traceobjects::Server::new(to_channel);
        let _req = timeout(Duration::from_secs(5), to_server.recv_request())
            .await
            .unwrap()
            .unwrap();

        let json_line = r#"{"at":"2024-01-15T14:30:00.000Z","data":{"kind":"Test"}}"#;
        let obj = make_test_trace_object(json_line);
        timeout(
            Duration::from_secs(5),
            to_server.send_response(vec![obj]),
        )
        .await
        .unwrap()
        .unwrap();

        tokio::time::sleep(Duration::from_millis(300)).await;
        plexer_handle.abort().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Check that two different node subdirectories were created.
    let subdirs: Vec<_> = std::fs::read_dir(&logs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    assert!(
        subdirs.len() >= 2,
        "Expected at least 2 node subdirectories, got {}",
        subdirs.len()
    );

    child.kill().ok();
    child.wait().ok();
}
