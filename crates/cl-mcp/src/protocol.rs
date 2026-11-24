//! MCP over JSON-RPC 2.0, hand-rolled (plan §1: no new dependency), as the
//! host answers it (plan D1).
//!
//! [`handle`] takes one message from one connection and gives the answer
//! (none for a notification). It knows MCP's shape: `initialize` with
//! instructions saying what the connection can and cannot do, `ping`,
//! `tools/list` with `readOnlyHint`/`destructiveHint` on every tool, and
//! `tools/call`. What a call does is the [`Bridge`]'s (plan D2); every call
//! passes [`policy::decide`] first, so a refused tool never reaches it.
//! Tool failures are `isError` results, never JSON-RPC errors (MCP).

use std::fmt::Write as _;

use serde_json::{Map, Value, json};

use crate::policy::{self, Allowed, Policy};
use crate::tools::{Action, Class, Kind, Tool};

/// The newest MCP revision this server speaks; an older one a client asks
/// for is answered in kind.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// The revisions this server speaks.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// The server's name in `serverInfo`.
pub const SERVER_NAME: &str = "clientledger";
/// A tool call's arguments may be at most this large (plan §7).
pub const MAX_PAYLOAD: usize = 256 * 1024;
/// A tool's answer may be at most this large (plan §7).
pub const MAX_OUTPUT: usize = 1024 * 1024;

/// JSON-RPC: the request is not valid.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: no such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: the parameters are not valid.
pub const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC: the server could not answer.
pub const INTERNAL_ERROR: i64 = -32603;

/// Why the host has no policy for a connection right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// No company is open (Q-A11h).
    NoCompany,
    /// The company is open but the engine could not be asked.
    Engine(String),
}

impl Unavailable {
    /// Plain words for the agent.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Unavailable::NoCompany => "ClientLedger is open, but no company is open. Ask the user \
                 to open the company, then try again."
                .into(),
            Unavailable::Engine(why) => {
                format!("ClientLedger could not answer just now ({why}). Try again in a moment.")
            }
        }
    }
}

/// One allowed tool call, as the bridge gets it.
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    /// The connection.
    pub connection: &'a str,
    /// The tool.
    pub tool: &'static Tool,
    /// Its action.
    pub action: &'static Action,
    /// What the policy decided (class, review).
    pub allowed: Allowed,
    /// The arguments, `action` taken out.
    pub arguments: &'a Map<String, Value>,
}

/// What a tool call came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Done (a read's data, or a change applied without review).
    Done(Value),
    /// Waiting in MCP Activity for a person (review on).
    Waiting {
        /// The action's id, for `mcp_status` `get_action`.
        action_id: String,
    },
    /// Refused, in plain words (the engine's or the bridge's).
    Refused(String),
}

/// What the transports call: the host side of the bookkeeper (plan D2).
pub trait Bridge: Send + Sync {
    /// The policy for `connection` now (switches, state, live role).
    ///
    /// # Errors
    /// No company open, or the engine could not be asked.
    fn policy(&self, connection: &str) -> Result<Policy, Unavailable>;

    /// Carry out an allowed call.
    fn call(&self, call: Call<'_>) -> Outcome;

    /// A connection's initialize, for the log and the Activity's "last seen"
    /// (the client's name and version, never anything it sent in a call).
    fn hello(&self, _connection: &str, _client: &str) {}
}

/// A result for `id`.
#[must_use]
pub fn result(id: &Value, result: Value) -> Value {
    let mut m = Map::new();
    m.insert("jsonrpc".into(), Value::from("2.0"));
    m.insert("id".into(), id.clone());
    m.insert("result".into(), result);
    Value::Object(m)
}

/// An error for `id`.
#[must_use]
pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A tool result with one text part.
#[must_use]
pub fn tool_text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// The `initialize` result.
#[must_use]
pub fn initialize_result(params: &Value, instructions: &str) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    let version = if PROTOCOL_VERSIONS.contains(&asked) {
        asked
    } else {
        PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": version,
        // The relay tells the agent when the list changes (N5).
        "capabilities": {"tools": {"listChanged": true}},
        "serverInfo": {"name": SERVER_NAME, "title": "ClientLedger", "version": env!("CARGO_PKG_VERSION")},
        "instructions": instructions,
    })
}

/// A tool as `tools/list` shows it, with only the `actions` allowed.
#[must_use]
pub fn tool_json(tool: &Tool, actions: &[(&Action, Allowed)]) -> Value {
    let names: Vec<&str> = actions.iter().map(|(a, _)| a.name).collect();
    let mut description = String::from(tool.description);
    for (a, ok) in actions {
        let _ = write!(description, "\n- {}: {}", a.name, a.what);
        if ok.review {
            description.push_str(" (waits for a person to accept it)");
        }
    }
    let mut props = Map::new();
    props.insert(
        "action".into(),
        json!({"type": "string", "enum": names, "description": "What to do."}),
    );
    props.insert(
        "id".into(),
        json!({"type": "string", "description": "The record's id (get, edit, void, email)."}),
    );
    if tool.kind == Kind::Read {
        props.insert(
            "page".into(),
            json!({"type": "integer", "minimum": 1, "description": "Page of 200 (list)."}),
        );
        props.insert(
            "filters".into(),
            json!({"type": "object", "description": "Filters the list or report takes, as text."}),
        );
    } else {
        props.insert(
            "fields".into(),
            json!({"type": "object", "description": "The form's fields. Every amount is text, like \"125.00\"."}),
        );
    }
    let destructive = actions
        .iter()
        .any(|(a, _)| a.record_edit || a.classes.contains(&Class::DeleteVoid));
    json!({
        "name": tool.name,
        "title": title(tool.name),
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": ["action"],
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": tool.kind == Kind::Read,
            "destructiveHint": tool.kind != Kind::Read && destructive,
            "idempotentHint": tool.kind == Kind::Read,
            "openWorldHint": tool.kind == Kind::Send,
        },
    })
}

fn title(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, word) in name.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            if i == 0 {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            out.extend(chars);
        }
    }
    out
}

/// The `tools/list` result for `p`.
#[must_use]
pub fn tools_list(p: &Policy) -> Value {
    let tools: Vec<Value> = policy::visible(p)
        .iter()
        .map(|(t, a)| tool_json(t, a))
        .collect();
    json!({"tools": tools})
}

/// The host's own status answer (open, and what the connection may do).
#[must_use]
pub fn status_text(p: &Policy) -> String {
    format!(
        "ClientLedger is open with a company. {}",
        policy::instructions(p)
    )
}

/// Answer one message from `connection`. `None` for a notification (or a
/// response the client sent).
#[must_use]
pub fn handle(bridge: &dyn Bridge, connection: &str, msg: &Value) -> Option<Value> {
    let Some(id) = msg.get("id").filter(|i| i.is_string() || i.is_number()) else {
        // Notifications (`initialized`, `cancelled`) need no answer.
        return None;
    };
    // A response to something we never send.
    msg.get("method")?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(error(id, INVALID_REQUEST, "The request has no method."));
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let client = params["clientInfo"]["name"].as_str().unwrap_or("");
            bridge.hello(connection, client);
            let text = match bridge.policy(connection) {
                Ok(p) => policy::instructions(&p),
                Err(u) => format!("{} {}", u.message(), policy::NEVER),
            };
            Some(result(id, initialize_result(&params, &text)))
        }
        "ping" => Some(result(id, json!({}))),
        "tools/list" => Some(match bridge.policy(connection) {
            Ok(p) => result(id, tools_list(&p)),
            Err(_) => result(id, json!({"tools": [status_only()]})),
        }),
        "tools/call" => Some(result(id, call(bridge, connection, &params))),
        "resources/list" => Some(result(id, json!({"resources": []}))),
        "resources/templates/list" => Some(result(id, json!({"resourceTemplates": []}))),
        "prompts/list" => Some(result(id, json!({"prompts": []}))),
        _ => Some(error(
            id,
            METHOD_NOT_FOUND,
            "ClientLedger does not answer that method.",
        )),
    }
}

/// `mcp_status` alone (no company open, or the app closed).
#[must_use]
pub fn status_only() -> Value {
    json!({
        "name": "mcp_status",
        "title": "Mcp status",
        "description": "Whether ClientLedger is open and what this connection may do.",
        "inputSchema": {
            "type": "object",
            "properties": {"action": {"type": "string", "enum": ["get"]}},
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

fn call(bridge: &dyn Bridge, connection: &str, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return tool_text("The call has no tool name.", true);
    };
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        Some(_) => return tool_text("The arguments must be an object.", true),
    };
    if serde_json::to_vec(args).map_or(usize::MAX, |b| b.len()) > MAX_PAYLOAD {
        return tool_text(
            "The arguments are too large (at most 256 KB). Send fewer lines at a time.",
            true,
        );
    }
    let p = match bridge.policy(connection) {
        Ok(p) => p,
        Err(u) => return tool_text(&u.message(), true),
    };
    // mcp_status takes `get` when no action is given.
    let action = match args.get("action") {
        Some(Value::String(a)) => a.as_str(),
        None if name == "mcp_status" => "get",
        _ => return tool_text("Say which action to take (the tool lists them).", true),
    };
    let (tool, action, allowed) = match policy::decide(&p, name, action) {
        Ok(x) => x,
        Err(r) => return tool_text(&r.message(), true),
    };
    if tool.name == "mcp_status" && action.name == "get" {
        return tool_text(&status_text(&p), false);
    }
    let mut rest = args.clone();
    rest.remove("action");
    let outcome = bridge.call(Call {
        connection,
        tool,
        action,
        allowed,
        arguments: &rest,
    });
    match outcome {
        Outcome::Done(v) => {
            let text = match &v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if text.len() > MAX_OUTPUT {
                return tool_text(
                    "The answer is too large (more than 1 MB). Ask for a smaller page or a \
                     shorter date range.",
                    true,
                );
            }
            tool_text(&text, false)
        }
        Outcome::Waiting { action_id } => {
            let text = json!({
                "status": "waiting",
                "actionId": action_id,
                "message": "The change waits for a person to accept it in ClientLedger's MCP \
                    Activity. Follow it with mcp_status (action get_action).",
            });
            tool_text(&text.to_string(), false)
        }
        Outcome::Refused(why) => tool_text(&why, true),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::policy::{ConnectionState, Switches};
    use cl_model::permissions::Rights;
    use std::sync::Mutex;

    /// A fake bridge: a fixed policy, and every call it gets kept.
    pub(crate) struct Fake {
        pub policy: Mutex<Result<Policy, Unavailable>>,
        pub calls: Mutex<Vec<(String, String, Allowed, Value)>>,
        pub outcome: Outcome,
    }

    impl Fake {
        pub(crate) fn new(switches: Switches) -> Fake {
            Fake {
                policy: Mutex::new(Ok(Policy {
                    switches,
                    connection: ConnectionState::Active,
                    rights: Rights::all(),
                    reinstate: false,
                })),
                calls: Mutex::new(Vec::new()),
                outcome: Outcome::Done(json!({"rows": []})),
            }
        }
    }

    impl Bridge for Fake {
        fn policy(&self, _: &str) -> Result<Policy, Unavailable> {
            self.policy.lock().expect("lock").clone()
        }
        fn call(&self, call: Call<'_>) -> Outcome {
            self.calls.lock().expect("lock").push((
                call.tool.name.into(),
                call.action.name.into(),
                call.allowed,
                Value::Object(call.arguments.clone()),
            ));
            self.outcome.clone()
        }
    }

    pub(crate) fn on() -> Switches {
        Switches {
            on: true,
            ..Switches::default()
        }
    }

    fn ask(b: &Fake, method: &str, params: &Value) -> Value {
        handle(
            b,
            "c1",
            &json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}),
        )
        .expect("an answer")
    }

    #[test]
    fn initialize_says_what_the_connection_can_do() {
        let b = Fake::new(on());
        let r = ask(
            &b,
            "initialize",
            &json!({"protocolVersion": "2025-03-26", "clientInfo": {"name": "claude-ai"}}),
        );
        assert_eq!(r["id"], 7);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], true);
        let text = r["result"]["instructions"].as_str().expect("instructions");
        assert!(text.contains("can: Read"), "{text}");
        assert!(text.contains("It cannot: Create (turned off)"), "{text}");
        let r = ask(&b, "initialize", &json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_and_responses_get_no_answer() {
        let b = Fake::new(on());
        assert!(
            handle(
                &b,
                "c1",
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
            )
            .is_none()
        );
        assert!(handle(&b, "c1", &json!({"jsonrpc": "2.0", "id": 1, "result": {}})).is_none());
        let r = ask(&b, "sampling/createMessage", &json!({}));
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn every_listed_tool_carries_both_hints_and_a_closed_schema() {
        let mut s = on();
        for c in Class::ALL {
            s.set(c, true);
        }
        let b = Fake::new(s);
        let r = ask(&b, "tools/list", &json!({}));
        let tools = r["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() > 20);
        for t in tools {
            let a = &t["annotations"];
            assert!(a["readOnlyHint"].is_boolean(), "{t}");
            assert!(a["destructiveHint"].is_boolean(), "{t}");
            assert_eq!(t["inputSchema"]["additionalProperties"], false, "{t}");
            let name = t["name"].as_str().expect("name");
            if name.ends_with("_read") {
                assert_eq!(a["readOnlyHint"], true, "{name}");
                assert_eq!(a["destructiveHint"], false, "{name}");
            } else if name.ends_with("_change") || name.ends_with("_send") {
                assert_eq!(a["readOnlyHint"], false, "{name}");
            }
        }
        let bill = tools
            .iter()
            .find(|t| t["name"] == "bill_change")
            .expect("bill_change");
        assert_eq!(
            bill["annotations"]["destructiveHint"], true,
            "edit overwrites"
        );
        assert_eq!(
            bill["inputSchema"]["properties"]["action"]["enum"],
            json!(["create", "edit"])
        );
        let send = tools
            .iter()
            .find(|t| t["name"] == "invoice_send")
            .expect("send");
        assert_eq!(send["annotations"]["openWorldHint"], true);
    }

    #[test]
    fn a_create_only_tool_is_not_destructive() {
        let mut s = on();
        s.set(Class::Create, true);
        let r = ask(&Fake::new(s), "tools/list", &json!({}));
        let bill = r["result"]["tools"]
            .as_array()
            .and_then(|t| t.iter().find(|t| t["name"] == "bill_change").cloned())
            .expect("bill_change");
        assert_eq!(bill["annotations"]["destructiveHint"], false);
        assert!(
            bill["description"]
                .as_str()
                .is_some_and(|d| d.contains("waits for a person"))
        );
    }

    #[test]
    fn a_refused_call_never_reaches_the_bridge() {
        let b = Fake::new(on());
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_change", "arguments": {"action": "create", "fields": {}}}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("\"Create\" is turned off"))
        );
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "users_change", "arguments": {"action": "create"}}),
        );
        assert_eq!(r["result"]["isError"], true);
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_read", "arguments": "x"}),
        );
        assert_eq!(r["result"]["isError"], true);
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_read", "arguments": {}}),
        );
        assert_eq!(r["result"]["isError"], true, "no action");
        assert!(b.calls.lock().expect("lock").is_empty());
    }

    #[test]
    fn an_allowed_call_reaches_the_bridge_without_its_action() {
        let mut s = on();
        s.set(Class::Create, true);
        let mut b = Fake::new(s);
        b.outcome = Outcome::Waiting {
            action_id: "a1".into(),
        };
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_change", "arguments": {"action": "create", "fields": {"amount": "12.50"}}}),
        );
        assert_eq!(r["result"]["isError"], false);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("\"actionId\":\"a1\""))
        );
        let calls = b.calls.lock().expect("lock");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].0.as_str(), calls[0].1.as_str()),
            ("bill_change", "create")
        );
        assert!(calls[0].2.review);
        assert_eq!(calls[0].3, json!({"fields": {"amount": "12.50"}}));
    }

    #[test]
    fn oversized_arguments_and_answers_are_refused() {
        let b = Fake::new(on());
        let big = "x".repeat(MAX_PAYLOAD + 1);
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_read", "arguments": {"action": "list", "filters": {"q": big}}}),
        );
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("too large"))
        );
        assert!(b.calls.lock().expect("lock").is_empty());
        let mut b = Fake::new(on());
        b.outcome = Outcome::Done(Value::String("y".repeat(MAX_OUTPUT + 1)));
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_read", "arguments": {"action": "list"}}),
        );
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn the_status_tool_is_answered_here() {
        let b = Fake::new(on());
        let r = ask(&b, "tools/call", &json!({"name": "mcp_status"}));
        assert_eq!(r["result"]["isError"], false);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("ClientLedger is open"))
        );
        assert!(b.calls.lock().expect("lock").is_empty());
    }

    #[test]
    fn no_company_lists_only_the_status_tool() {
        let b = Fake::new(on());
        *b.policy.lock().expect("lock") = Err(Unavailable::NoCompany);
        let r = ask(&b, "tools/list", &json!({}));
        assert_eq!(r["result"]["tools"].as_array().map(Vec::len), Some(1));
        let r = ask(
            &b,
            "tools/call",
            &json!({"name": "bill_read", "arguments": {"action": "list"}}),
        );
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("no company is open"))
        );
        let r = ask(&b, "initialize", &json!({}));
        assert!(
            r["result"]["instructions"]
                .as_str()
                .is_some_and(|t| t.contains("no company"))
        );
    }

    /// Plan section 8 row 1: for every switch set, review setting,
    /// Delete/Void availability and connection state, the bridge is called
    /// for exactly the actions the policy allows, and `tools/list` offers
    /// exactly those.
    #[test]
    fn the_bridge_gets_only_what_the_switches_allow() {
        use crate::tools::TOOLS;
        let states = [
            ConnectionState::Active,
            ConnectionState::Revoked,
            ConnectionState::Unconfirmed,
        ];
        for bits in 0u32..64 {
            for reinstate in [false, true] {
                for state in states {
                    let mut s = Switches {
                        on: bits & 1 == 1,
                        ..Switches::default()
                    };
                    for (i, c) in Class::ALL.into_iter().enumerate() {
                        s.set(c, bits & (2 << i) != 0);
                        s.set_review(c, (bits >> i) & 1 == 0);
                    }
                    let b = Fake::new(s);
                    {
                        let mut p = b.policy.lock().expect("lock");
                        if let Ok(p) = p.as_mut() {
                            p.reinstate = reinstate;
                            p.connection = state;
                        }
                    }
                    let p = b.policy.lock().expect("lock").clone().expect("policy");
                    let listed = ask(&b, "tools/list", &json!({}));
                    let listed: Vec<(String, Vec<String>)> = listed["result"]["tools"]
                        .as_array()
                        .expect("tools")
                        .iter()
                        .map(|t| {
                            let actions = t["inputSchema"]["properties"]["action"]["enum"]
                                .as_array()
                                .expect("enum")
                                .iter()
                                .map(|a| a.as_str().expect("str").to_string())
                                .collect();
                            (t["name"].as_str().expect("name").to_string(), actions)
                        })
                        .collect();
                    for t in TOOLS {
                        for a in t.actions {
                            let allowed = policy::decide_action(&p, t, a).ok();
                            let shown = listed
                                .iter()
                                .any(|(n, acts)| n == t.name && acts.iter().any(|x| x == a.name));
                            assert_eq!(
                                shown,
                                allowed.is_some(),
                                "{}.{} {bits} {state:?}",
                                t.name,
                                a.name
                            );
                            let before = b.calls.lock().expect("lock").len();
                            let r = ask(
                                &b,
                                "tools/call",
                                &json!({"name": t.name, "arguments": {"action": a.name}}),
                            );
                            let after = b.calls.lock().expect("lock").len();
                            let host_answered = t.name == "mcp_status" && a.name == "get";
                            match allowed {
                                Some(ok) if !host_answered => {
                                    assert_eq!(after, before + 1, "{}.{}", t.name, a.name);
                                    assert_eq!(b.calls.lock().expect("lock")[before].2, ok);
                                }
                                _ => {
                                    assert_eq!(after, before, "{}.{} {bits}", t.name, a.name);
                                    if allowed.is_none() {
                                        assert_eq!(r["result"]["isError"], true);
                                    }
                                }
                            }
                            // Review on for any class it needs: it waits.
                            if let Some(ok) = allowed {
                                let wants = a.classes.iter().any(|&c| s.reviewed(c));
                                assert_eq!(ok.review, wants);
                                assert!(
                                    !a.classes.contains(&Class::DeleteVoid) || reinstate,
                                    "Delete/Void only with reinstate"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn titles_read_as_words() {
        assert_eq!(title("bill_change"), "Bill change");
        assert_eq!(title("mcp_status"), "Mcp status");
    }
}
