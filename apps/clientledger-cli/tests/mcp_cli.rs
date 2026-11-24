//! `clientledger-cli mcp`, run the way Claude Desktop runs it, with the app
//! closed: the handshake is answered, the one tool says to open the app
//! first, and nothing but MCP lines reaches stdout. (The open app is the
//! live test, plan D6.)

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[test]
fn closed_app_answers_in_plain_words_and_only_mcp_on_stdout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = Command::new(env!("CARGO_BIN_EXE_clientledger-cli"))
        .args(["mcp", "--connection", "c-1", "--data-dir"])
        .arg(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the CLI");
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "claude-ai"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "bill_read", "arguments": {"action": "list"}}}),
    ];
    {
        let mut stdin = child.stdin.take().expect("stdin");
        for m in &input {
            writeln!(stdin, "{m}").expect("write");
        }
    }
    let out = child.wait_with_output().expect("ends with its input");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf8");
    let replies: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("only MCP lines on stdout"))
        .collect();
    assert_eq!(replies.len(), 3, "{text}");
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(
        replies[0]["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    assert!(
        replies[0]["result"]["instructions"]
            .as_str()
            .is_some_and(|t| t.contains("not open"))
    );
    let tools = replies[1]["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(replies[2]["result"]["isError"], true);
    assert!(
        replies[2]["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("open ClientLedger"))
    );
}

#[test]
fn a_bad_connection_id_is_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_clientledger-cli"))
        .args(["mcp", "--connection", "../etc"])
        .stdin(Stdio::null())
        .output()
        .expect("run the CLI");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "nothing on stdout");
}
