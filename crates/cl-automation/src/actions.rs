//! Wire types: requests, responses, and the parameters of every method.
//!
//! The protocol is one JSON object per line over a loopback TCP connection.
//! Every request carries the token from the control file.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One request line.
#[derive(Clone, Deserialize, Serialize)]
pub struct Request {
    /// Secret from the control file.
    pub token: String,
    /// Method name (see [`crate::tools::tools`]).
    pub method: String,
    /// Method parameters (an object, or absent).
    #[serde(default)]
    pub params: Value,
    /// Caller's correlation value, echoed back.
    #[serde(default)]
    pub id: Option<Value>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            // Parameters can hold typed text (even a secret): never printed.
            .field("params", &"<hidden>")
            .finish_non_exhaustive()
    }
}

/// Machine-readable error kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Missing or wrong token.
    Unauthorized,
    /// The line was not a valid request.
    BadRequest,
    /// No such method.
    UnknownMethod,
    /// No element matches the target.
    NotFound,
    /// More than one element matches the target.
    Ambiguous,
    /// The app did not finish in time.
    Timeout,
    /// The app is closing.
    Closed,
    /// The request is valid but cannot be done now.
    Failed,
}

/// An error in a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Kind.
    pub code: ErrorCode,
    /// Plain-language detail (never contains the token or secrets).
    pub message: String,
}

/// One response line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// True when the request succeeded.
    pub ok: bool,
    /// The method's result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The error, when not ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    /// Echo of the request id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

impl Response {
    /// A success.
    #[must_use]
    pub fn success(result: Value) -> Response {
        Response {
            ok: true,
            result: Some(result),
            error: None,
            id: None,
        }
    }

    /// A failure.
    #[must_use]
    pub fn failure(code: ErrorCode, message: impl Into<String>) -> Response {
        Response {
            ok: false,
            result: None,
            error: Some(ErrorBody {
                code,
                message: message.into(),
            }),
            id: None,
        }
    }
}

/// Which element a method acts on. All given fields must match; the label
/// match is exact unless `label_contains` is used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Element id from `inspect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Exact accessible label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Label contains this text (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_contains: Option<String>,
    /// Description (a widget's tooltip text) contains this text
    /// (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_contains: Option<String>,
    /// Role name, for example `button`, `tab`, `text_input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Which match to use when several elements match (0 is the first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

impl Target {
    /// True when nothing is specified.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.label.is_none()
            && self.label_contains.is_none()
            && self.description_contains.is_none()
            && self.role.is_none()
    }

    /// Short text for messages.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.id {
            parts.push(format!("id {v}"));
        }
        if let Some(v) = &self.role {
            parts.push(format!("role {v}"));
        }
        if let Some(v) = &self.label {
            parts.push(format!("label \"{v}\""));
        }
        if let Some(v) = &self.label_contains {
            parts.push(format!("label containing \"{v}\""));
        }
        if let Some(v) = &self.description_contains {
            parts.push(format!("description containing \"{v}\""));
        }
        if parts.is_empty() {
            "anything".into()
        } else {
            parts.join(", ")
        }
    }
}

/// `inspect`: the element tree, or only elements matching `target`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectParams {
    /// Return only matching elements (flat list) when given.
    #[serde(default)]
    pub target: Option<Target>,
}

/// `click`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickParams {
    /// What to click (its centre).
    pub target: Target,
}

/// `type`: text typed into the focused element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeParams {
    /// The text.
    pub text: String,
    /// Focus this element first.
    #[serde(default)]
    pub target: Option<Target>,
}

/// `paste`: text pasted into the focused element as one clipboard paste,
/// so tabs and line breaks arrive together. `Debug` hides the text: pasted
/// rows hold names and amounts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasteParams {
    /// The text.
    pub text: String,
    /// Focus this element first.
    #[serde(default)]
    pub target: Option<Target>,
}

impl std::fmt::Debug for PasteParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasteParams")
            .field("text", &format_args!("<{} bytes>", self.text.len()))
            .field("target", &self.target)
            .finish()
    }
}

/// `key`: one key press such as `Ctrl+K`, `Enter`, `Escape`, `Shift+Tab`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyParams {
    /// The key and modifiers, joined by `+`.
    pub key: String,
}

/// `focus`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusParams {
    /// What to focus.
    pub target: Target,
}

/// `command`: run a registered command by id (`file.quit`, `help.palette`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandParams {
    /// Command id from the command table.
    pub id: String,
}

/// `screenshot`: save the window as a PNG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotParams {
    /// Where to write the PNG (must end in `.png`).
    pub path: String,
}

/// `resize`: set the window's inner size in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeParams {
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
}

/// `set_theme`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeParams {
    /// `light`, `dark`, or `auto`.
    pub theme: String,
}

/// `set_zoom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoomParams {
    /// 80 to 200.
    pub percent: u16,
}

/// A parsed key chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    /// `Ctrl` in the chord. The app treats it as the keymap's command
    /// modifier: Control on Windows and Linux, Command on macOS, so a
    /// scenario written as `Ctrl+K` passes on every platform.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt (Option) held.
    pub alt: bool,
    /// `Cmd` in the chord; the same modifier as `Ctrl`.
    pub cmd: bool,
    /// Key name as egui spells it (`K`, `Enter`, `Escape`, `F1`, `+`).
    pub key: String,
}

impl KeySpec {
    /// Parse `Ctrl+Shift+K`, `Enter`, `Ctrl++`.
    ///
    /// # Errors
    /// A plain message when the text is empty or a modifier is unknown.
    pub fn parse(text: &str) -> Result<KeySpec, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a key is needed, for example Ctrl+K".into());
        }
        let (mods, key) = if let Some(head) = text.strip_suffix("++") {
            (head, "+")
        } else if text == "+" {
            ("", "+")
        } else if let Some((head, last)) = text.rsplit_once('+') {
            (head, last)
        } else {
            ("", text)
        };
        let mut spec = KeySpec {
            ctrl: false,
            shift: false,
            alt: false,
            cmd: false,
            key: key.trim().to_string(),
        };
        if spec.key.is_empty() {
            return Err(format!("`{text}` has no key after the modifiers"));
        }
        for m in mods.split('+').filter(|m| !m.trim().is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => spec.ctrl = true,
                "shift" => spec.shift = true,
                "alt" | "option" => spec.alt = true,
                "cmd" | "command" | "meta" | "super" => spec.cmd = true,
                other => return Err(format!("unknown modifier `{other}`")),
            }
        }
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_specs() {
        let k = KeySpec::parse("Ctrl+Shift+K").unwrap();
        assert!(k.ctrl && k.shift && !k.alt && !k.cmd);
        assert_eq!(k.key, "K");
        assert_eq!(KeySpec::parse("Enter").unwrap().key, "Enter");
        let plus = KeySpec::parse("Ctrl++").unwrap();
        assert!(plus.ctrl);
        assert_eq!(plus.key, "+");
        assert_eq!(KeySpec::parse("+").unwrap().key, "+");
        assert!(KeySpec::parse("").is_err());
        assert!(KeySpec::parse("Hyper+K").is_err());
        assert!(KeySpec::parse("Ctrl+").is_err());
    }

    #[test]
    fn request_debug_hides_the_token() {
        let r = Request {
            token: "secret-token".into(),
            method: "state".into(),
            params: serde_json::json!({"text": "typed-secret"}),
            id: None,
        };
        let shown = format!("{r:?}");
        assert!(!shown.contains("secret-token") && !shown.contains("typed-secret"));
    }

    #[test]
    fn params_reject_unknown_fields() {
        let bad: Result<ClickParams, _> =
            serde_json::from_value(serde_json::json!({"target": {"label": "x"}, "extra": 1}));
        assert!(bad.is_err());
        let bad: Result<Target, _> = serde_json::from_value(serde_json::json!({"nope": 1}));
        assert!(bad.is_err());
    }
}
