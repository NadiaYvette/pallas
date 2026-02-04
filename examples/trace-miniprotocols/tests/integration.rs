use pallas::codec::minicbor::{self, Decode};
use pallas::network::{
    miniprotocols::handshake::n2c,
    miniprotocols::{
        PROTOCOL_TFWP_DATAPOINTS, PROTOCOL_TFWP_TRACE_OBJECTS, datapoints, handshake, traceobjects,
    },
    multiplexer::{Bearer, Plexer},
};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::net::UnixListener;
use tokio::time::timeout;

#[tokio::test]
async fn test_server_mode_nodeinfo() {
    // 1. Setup Listener
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("tracer_test.socket");
    let listener = UnixListener::bind(&socket_path).unwrap();

    // 2. Spawn trace-miniprotocols in SERVER mode (Connects to us)
    let mut child = Command::new("../../target/debug/trace-miniprotocols")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--mode")
        .arg("server")
        .arg("--magic")
        .arg("12345")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn trace-miniprotocols");

    // 3. Accept connection
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("Timeout waiting for connection")
        .expect("Failed to accept connection");

    let bearer = Bearer::Unix(stream);
    let mut plexer = Plexer::new(bearer);

    // Test acts as Listener (Multiplexer Responder)
    // Child (Server mode) is Initiator.
    // Child runs MP Servers (Responder).
    // Test should run MP Clients (Initiator).
    let hs_channel = plexer.subscribe_server(0);
    let dp_channel = plexer.subscribe_server(PROTOCOL_TFWP_DATAPOINTS);

    let plexer_handle = plexer.spawn();

    // 4. Handshake
    // Test acts as Handshake Server using N2C VersionData
    let mut hs_server = handshake::Server::<n2c::VersionData>::new(hs_channel);
    let version_table = timeout(
        Duration::from_secs(5),
        hs_server.receive_proposed_versions(),
    )
    .await
    .expect("Timeout receiving version table")
    .expect("Failed to receive version table");

    assert!(version_table.values.contains_key(&1));

    // Accept v1 with magic 12345 (no extra params for v1)
    let v1_data = n2c::VersionData::new(12345, None);
    hs_server
        .accept_version(1, v1_data)
        .await
        .expect("Failed to accept version");

    // 5. Datapoints Interaction
    // Test runs Datapoints Client (sending Request). Child runs Server.
    let mut dp_client = datapoints::Client::new(dp_channel);

    // Send NodeInfo Request
    timeout(
        Duration::from_secs(5),
        dp_client.send_request(vec!["NodeInfo".to_string()]),
    )
    .await
    .expect("Timeout sending request")
    .expect("Failed to send NodeInfo request");

    // Receive Response
    let response = timeout(Duration::from_secs(5), dp_client.recv_response())
        .await
        .expect("Timeout receiving response")
        .expect("Failed to receive response");

    // Verify Response
    assert!(!response.is_empty(), "Response should not be empty");
    let (key, val) = &response[0];
    assert_eq!(key, "NodeInfo");
    assert!(val.is_some(), "NodeInfo value should be present");

    let val_bytes = val.as_ref().unwrap();

    let decoded: std::collections::HashMap<String, String> =
        minicbor::decode(val_bytes).expect("Failed to decode NodeInfo CBOR");
    assert_eq!(decoded.get("protocol").map(|s| s.as_str()), Some("Shelley"));

    // Cleanup
    child.kill().ok();
    plexer_handle.abort().await;
}

#[tokio::test]
async fn test_client_mode_polling() {
    // 1. Setup Socket Path
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("node_test.socket");

    // 2. Spawn trace-miniprotocols in CLIENT mode (Listens on socket)
    let mut child = Command::new("../../target/debug/trace-miniprotocols")
        .arg("--socket")
        .arg(&socket_path)
        .arg("--mode")
        .arg("client")
        .arg("--magic")
        .arg("12345")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn trace-miniprotocols");

    // Wait for child to bind
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 3. Connect to Child
    let stream = tokio::net::UnixStream::connect(&socket_path)
        .await
        .expect("Failed to connect to trace-miniprotocols");

    let bearer = Bearer::Unix(stream);
    let mut plexer = Plexer::new(bearer);

    // Test acts as Initiator (Multiplexer Initiator).
    // Child (Client mode) is Listener (Responder).
    // Child runs MP Clients (Initiator).
    // Test should run MP Servers (Responder).

    let hs_channel = plexer.subscribe_client(0);
    let to_channel = plexer.subscribe_client(PROTOCOL_TFWP_TRACE_OBJECTS);

    let plexer_handle = plexer.spawn();

    // 4. Handshake
    // Test acts as Handshake Client
    let mut hs_client = handshake::Client::<n2c::VersionData>::new(hs_channel);
    let v1_data = n2c::VersionData::new(12345, None);
    let mut values = std::collections::HashMap::new();
    values.insert(1, v1_data);
    let versions = n2c::VersionTable { values };

    hs_client
        .handshake(versions)
        .await
        .expect("Handshake failed");

    // 5. TraceObjects Interaction
    // Test runs TraceObjects Server (receiving Requests). Child runs Client.
    let mut to_server = traceobjects::Server::new(to_channel);

    // Expect a request (Child should poll)
    let req = timeout(Duration::from_secs(5), to_server.recv_request())
        .await
        .expect("Timeout receiving request")
        .expect("Failed to receive request");

    assert!(req.is_some());
    let (blocking, n) = req.unwrap();
    // Child sends (false, 10)
    assert_eq!(blocking, false);
    assert_eq!(n, 10);

    // Send dummy response
    let obj = traceobjects::TraceObject {
        to_human: Some("Test Trace".to_string()),
        to_machine: "{}".to_string(),
        to_namespace: vec!["Test".to_string()],
        severity: traceobjects::Severity::Info,
        detail: traceobjects::Detail::Normal,
        timestamp: traceobjects::TraceTimestamp { day: 0, pico: 0 },
        hostname: "test-node".to_string(),
        thread_id: "1".to_string(),
    };
    to_server
        .send_response(vec![obj])
        .await
        .expect("Failed to send response");

    // Cleanup
    child.kill().ok();
    plexer_handle.abort().await;
}
