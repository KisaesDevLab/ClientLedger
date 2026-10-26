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

/// The optional `viewport` parameter (A02 D5): which window the method acts
/// on.
fn viewport_schema() -> Value {
    json!({
        "type": "string",
        "enum": crate::actions::VIEWPORTS,
        "description": "Which window: main (the default) or doc_viewer (the pop-out document viewer)."
    })
}

/// `object` whose properties also take `viewport`.
fn in_viewport(properties: Value, required: &[&str]) -> Value {
    let mut properties = properties;
    if let Some(map) = properties.as_object_mut() {
        map.insert("viewport".into(), viewport_schema());
    }
    object(&properties, required)
}

/// A method whose only input is the element it acts on (in a window).
fn target_only(name: &'static str, description: &'static str) -> Tool {
    Tool {
        name,
        description,
        input_schema: in_viewport(json!({"target": target_schema()}), &["target"]),
    }
}

/// `dialog_answer`'s parameters (A01 WP-D).
fn dialog_answer_schema() -> Value {
    object(
        &json!({
            "paths": {"type": "array", "items": {"type": "string"}},
            "cancel": {"type": "boolean"}
        }),
        &[],
    )
}

/// All methods, in a stable order. One flat table, read top to bottom.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "inspect",
            description: "Return the accessibility tree (id, role, label, bounds, focus, enabled) of one window, or the elements matching a target, plus the open windows. Secret field values are hidden.",
            input_schema: in_viewport(json!({"target": target_schema()}), &[]),
        },
        target_only("click", "Click the centre of an element."),
        Tool {
            name: "type",
            description: "Type text into the focused element, optionally focusing a target first.",
            input_schema: in_viewport(
                json!({"text": {"type": "string"}, "target": target_schema()}),
                &["text"],
            ),
        },
        Tool {
            name: "paste",
            description: "Paste text into the focused element as one clipboard paste (tabs and line breaks kept), optionally focusing a target first.",
            input_schema: in_viewport(
                json!({"text": {"type": "string"}, "target": target_schema()}),
                &["text"],
            ),
        },
        Tool {
            name: "key",
            description: "Press one key chord such as Ctrl+K, Enter, Escape, Shift+Tab, F1.",
            input_schema: in_viewport(json!({"key": {"type": "string"}}), &["key"]),
        },
        target_only("focus", "Move keyboard focus to an element."),
        Tool {
            name: "scroll",
            description: "Turn the mouse wheel over an element until what scrolls there reaches its start or end.",
            input_schema: in_viewport(
                json!({
                    "target": target_schema(),
                    "to": {"type": "string", "enum": ["start", "end"]}
                }),
                &["target", "to"],
            ),
        },
        target_only(
            "hover",
            "Move the pointer over the centre of an element without clicking (shows its tooltip).",
        ),
        Tool {
            name: "command",
            description: "Run a registered command by id, for example help.palette or file.quit.",
            input_schema: object(&json!({"id": {"type": "string"}}), &["id"]),
        },
        Tool {
            name: "screenshot",
            description: "Save a window as a PNG file.",
            input_schema: in_viewport(
                json!({"path": {"type": "string", "description": "Output file, must end in .png."}}),
                &["path"],
            ),
        },
        Tool {
            name: "resize",
            description: "Set a window's inner size in points.",
            input_schema: in_viewport(
                json!({
                    "width": {"type": "number", "exclusiveMinimum": 0},
                    "height": {"type": "number", "exclusiveMinimum": 0}
                }),
                &["width", "height"],
            ),
        },
        Tool {
            name: "move",
            description: "Move a window's outer top-left corner to a desktop position in points.",
            input_schema: in_viewport(
                json!({
                    "x": {"type": "number"},
                    "y": {"type": "number"}
                }),
                &["x", "y"],
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
            name: "dialog_answer",
            description: "Queue the answer for the next file or folder window the app opens (a controlled run never shows one): the chosen paths, or cancel.",
            input_schema: dialog_answer_schema(),
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
        assert_eq!(n, 17);
        for tool in &t {
            assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
            assert_ne!(tool.description.len(), 0);
        }
        assert!(is_method("click") && is_method("move") && !is_method("rm"));
    }

    /// Every method that acts on a window takes `viewport` (A02 D5); the
    /// others do not.
    #[test]
    fn window_methods_take_a_viewport() {
        let takes = [
            "inspect",
            "click",
            "type",
            "paste",
            "key",
            "focus",
            "scroll",
            "hover",
            "screenshot",
            "resize",
            "move",
        ];
        for tool in tools() {
            let has = tool.input_schema["properties"].get("viewport").is_some();
            assert_eq!(has, takes.contains(&tool.name), "{}", tool.name);
            if has {
                assert_eq!(
                    tool.input_schema["properties"]["viewport"]["enum"],
                    json!(["main", "doc_viewer"])
                );
            }
        }
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
