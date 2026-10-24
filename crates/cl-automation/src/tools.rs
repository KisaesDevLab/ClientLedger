//! The tool table: every control-channel method with a JSON schema, exported
//! by `clientledger-cli tools` so a future MCP server can offer the same
//! operations (Q-16).

use serde::Serialize;
use serde_json::{Value, json};

/// One method.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    /// Method name.
    pub name: &'static str,
    /// What it does, in one sentence.
    pub description: &'static str,
    /// JSON Schema of the parameters object.
    pub input_schema: Value,
}

fn target_schema() -> Value {
    json!({
        "type": "object",
        "description": "Which element: all given fields must match.",
        "properties": {
            "id": {"type": "string", "description": "Element id from inspect."},
            "label": {"type": "string", "description": "Exact accessible label."},
            "label_contains": {"type": "string", "description": "Case-insensitive part of the label."},
            "description_contains": {"type": "string", "description": "Case-insensitive part of the description (a widget's tooltip text)."},
            "role": {"type": "string", "description": "Role such as button, tab, text_input."},
            "index": {"type": "integer", "minimum": 0, "description": "Which match when several."}
        },
        "additionalProperties": false
    })
}

fn object(properties: &Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

/// All methods, in a stable order.
#[must_use]
pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "inspect",
            description: "Return the accessibility tree (id, role, label, bounds, focus, enabled), or the elements matching a target. Secret field values are hidden.",
            input_schema: object(&json!({"target": target_schema()}), &[]),
        },
        Tool {
            name: "click",
            description: "Click the centre of an element.",
            input_schema: object(&json!({"target": target_schema()}), &["target"]),
        },
        Tool {
            name: "type",
            description: "Type text into the focused element, optionally focusing a target first.",
            input_schema: object(
                &json!({"text": {"type": "string"}, "target": target_schema()}),
                &["text"],
            ),
        },
        Tool {
            name: "paste",
            description: "Paste text into the focused element as one clipboard paste (tabs and line breaks kept), optionally focusing a target first.",
            input_schema: object(
                &json!({"text": {"type": "string"}, "target": target_schema()}),
                &["text"],
            ),
        },
        Tool {
            name: "key",
            description: "Press one key chord such as Ctrl+K, Enter, Escape, Shift+Tab, F1.",
            input_schema: object(&json!({"key": {"type": "string"}}), &["key"]),
        },
        Tool {
            name: "focus",
            description: "Move keyboard focus to an element.",
            input_schema: object(&json!({"target": target_schema()}), &["target"]),
        },
        Tool {
            name: "hover",
            description: "Move the pointer over the centre of an element without clicking (shows its tooltip).",
            input_schema: object(&json!({"target": target_schema()}), &["target"]),
        },
        Tool {
            name: "command",
            description: "Run a registered command by id, for example help.palette or file.quit.",
            input_schema: object(&json!({"id": {"type": "string"}}), &["id"]),
        },
        Tool {
            name: "screenshot",
            description: "Save the window as a PNG file.",
            input_schema: object(
                &json!({"path": {"type": "string", "description": "Output file, must end in .png."}}),
                &["path"],
            ),
        },
        Tool {
            name: "resize",
            description: "Set the window's inner size in points.",
            input_schema: object(
                &json!({
                    "width": {"type": "number", "exclusiveMinimum": 0},
                    "height": {"type": "number", "exclusiveMinimum": 0}
                }),
                &["width", "height"],
            ),
        },
        Tool {
            name: "set_theme",
            description: "Switch the colour theme for this run.",
            input_schema: object(
                &json!({"theme": {"type": "string", "enum": ["light", "dark", "auto"]}}),
                &["theme"],
            ),
        },
        Tool {
            name: "set_zoom",
            description: "Set the app zoom (80 to 200 percent).",
            input_schema: object(
                &json!({"percent": {"type": "integer", "minimum": 80, "maximum": 200}}),
                &["percent"],
            ),
        },
        Tool {
            name: "state",
            description: "Report window size, zoom, theme, open tabs, dialogs, and engine status.",
            input_schema: object(&json!({}), &[]),
        },
        Tool {
            name: "quit",
            description: "Ask the app to quit. Unsaved work prompts; nothing is discarded.",
            input_schema: object(&json!({}), &[]),
        },
    ]
}

/// True when `name` is a method.
#[must_use]
pub fn is_method(name: &str) -> bool {
    tools().iter().any(|t| t.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_schemas_are_objects() {
        let t = tools();
        let mut names: Vec<_> = t.iter().map(|x| x.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
        assert_eq!(n, 14);
        for tool in &t {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            assert_ne!(tool.description.len(), 0);
        }
        assert!(is_method("click") && !is_method("rm"));
    }

    #[test]
    fn required_fields_exist_in_properties() {
        for tool in tools() {
            let props = tool.input_schema["properties"].as_object().unwrap();
            for r in tool.input_schema["required"].as_array().unwrap() {
                assert!(props.contains_key(r.as_str().unwrap()), "{}", tool.name);
            }
        }
    }
}
