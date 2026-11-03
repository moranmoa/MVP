//! The sidecar's line protocol, driven in-process.

mod common;

use std::io::Cursor;
use std::sync::Arc;

use api::sidecar::{self, Lines, MAX_LINE};
use api::{Config, Core, FixedClock, LoopbackNetwork, PendingGroupKeys, Ports};
use identity::MemoryStore;
use serde_json::{json, Value};

/// Runs `input` through a fresh core; returns every output line.
fn run(input: &[u8]) -> Vec<Value> {
    let out = Lines::new(Vec::new());
    let core = Core::open(
        Config { db_path: None, app_version: "0.1.0-alpha".into() },
        Ports {
            secrets: Arc::new(MemoryStore::new()),
            network: Arc::new(LoopbackNetwork::new()),
            events: out.clone(),
            clock: Arc::new(FixedClock::new(common::NOW_MS)),
            cipher: Arc::new(PendingGroupKeys),
        },
    )
    .unwrap();
    sidecar::serve(&core, Cursor::new(input.to_vec()), &out).unwrap();
    drop(core);
    let bytes = Arc::try_unwrap(out).ok().expect("core dropped").into_inner();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.is_empty() || text.ends_with('\n'));
    text.lines().map(|line| serde_json::from_str(line).expect("each line is JSON")).collect()
}

fn bad_request(id: Value) -> Value {
    json!({ "type": "response", "id": id, "error": { "code": "bad-request" } })
}

#[test]
fn responses_carry_the_id_and_events_come_first() {
    let input = concat!(
        r#"{"id":1,"cmd":"getIdentity"}"#,
        "\n",
        r#"{"id":"b","cmd":"createIdentity","args":{"input":{"locale":"he","country":"IL","birthDate":"1990-05-01"}}}"#,
        "\n",
        r#"{"id":3,"cmd":"listServers"}"#,
        "\n",
    );
    let lines = run(input.as_bytes());
    assert_eq!(lines[0], json!({ "type": "response", "id": 1, "ok": null }));
    // createIdentity: four progress events, then its response.
    for event in &lines[1..5] {
        assert_eq!(event["type"], "event");
        assert_eq!(event["event"], "identity.progress");
        assert!(event.get("id").is_none());
    }
    assert_eq!(lines[5]["type"], "response");
    assert_eq!(lines[5]["id"], "b");
    assert_eq!(lines[5]["ok"]["words"].as_array().unwrap().len(), 12);
    assert_eq!(lines[6], json!({ "type": "response", "id": 3, "ok": [] }));
    assert_eq!(lines.len(), 7);
}

#[test]
fn malformed_lines_get_an_error_and_serving_goes_on() {
    let input = concat!(
        "not json\n",
        "[1,2]\n",
        "{\"cmd\":\"getIdentity\"}\n",
        "{\"id\":7}\n",
        "{\"id\":{\"x\":1},\"cmd\":\"getIdentity\"}\n",
        "{\"id\":8,\"cmd\":\"noSuchCommand\"}\n",
        "\n",
        "   \r\n",
        "{\"id\":9,\"cmd\":\"getIdentity\"}\r\n",
        "{\"id\":10,\"cmd\":\"getIdentity\"}",
    );
    let lines = run(input.as_bytes());
    assert_eq!(
        lines,
        [
            bad_request(Value::Null),
            bad_request(Value::Null),
            bad_request(Value::Null),
            bad_request(json!(7)),
            bad_request(Value::Null),
            bad_request(json!(8)),
            json!({ "type": "response", "id": 9, "ok": null }),
            json!({ "type": "response", "id": 10, "ok": null }),
        ]
    );
}

#[test]
fn a_line_over_the_limit_is_refused_whole() {
    let mut input = Vec::new();
    // Over the limit, with something that looks like a request at the end,
    // which must not be answered on its own.
    input.extend_from_slice(br#"{"id":1,"cmd":"getIdentity","pad":""#);
    input.resize(input.len() + MAX_LINE, b'x');
    input.extend_from_slice(b"\"}{\"id\":2,\"cmd\":\"getIdentity\"}\n");
    // Exactly at the limit is fine.
    let mut fits = br#"{"id":3,"cmd":"getIdentity","pad":""#.to_vec();
    fits.resize(MAX_LINE - 2, b'y');
    fits.extend_from_slice(b"\"}\n");
    input.extend_from_slice(&fits);
    // A long line with no newline before the end.
    input.resize(input.len() + MAX_LINE + 1, b'z');

    let lines = run(&input);
    assert_eq!(
        lines,
        [
            bad_request(Value::Null),
            json!({ "type": "response", "id": 3, "ok": null }),
            bad_request(Value::Null),
        ]
    );
}

#[test]
fn empty_input_is_a_clean_exit() {
    assert!(run(b"").is_empty());
}

#[test]
fn the_binary_wants_a_data_dir() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_mvs-core"))
        .arg("--nope")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
