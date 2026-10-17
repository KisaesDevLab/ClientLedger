//! The element tree an `inspect` call returns, and how targets are resolved
//! against it. No egui here: `cl-ui` converts the AccessKit tree into these
//! plain types.

use serde::{Deserialize, Serialize};

use crate::actions::Target;

/// Shown instead of the value of a secret field.
pub const REDACTED: &str = "[hidden]";

/// A rectangle in egui points, origin at the window's top-left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Bounds {
    /// Centre point.
    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// One element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable id for this run (use it in a target).
    pub id: String,
    /// Role name in snake case (`button`, `tab`, `text_input`, ...).
    pub role: String,
    /// Accessible label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Current value of an input (hidden for secret fields).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
    /// Has keyboard focus.
    pub focused: bool,
    /// Can be used (not greyed out).
    pub enabled: bool,
    /// Child elements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

/// Window facts that come with a tree.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
    /// Physical pixels per point (zoom included).
    pub pixels_per_point: f32,
}

/// A whole tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The window.
    pub window: WindowInfo,
    /// Id of the focused element, if any.
    #[serde(default)]
    pub focused: Option<String>,
    /// Root element.
    pub tree: Node,
}

/// Why a target did not resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// Nothing matches.
    #[error("no element matches {0}")]
    NotFound(String),
    /// Several match and no `index` was given.
    #[error("{count} elements match {what}; add an index or a role to choose one")]
    Ambiguous {
        /// What was asked for.
        what: String,
        /// How many matched.
        count: usize,
    },
}

/// True when a field's value must not be shown: password inputs, and inputs
/// whose label names a key, secret, token, or password.
#[must_use]
pub fn is_secret_field(role: &str, label: Option<&str>) -> bool {
    if role == "password_input" {
        return true;
    }
    let Some(label) = label else { return false };
    let l = label.to_ascii_lowercase();
    [
        "password",
        "secret",
        "token",
        "api key",
        "ai key",
        "passphrase",
        "recovery key",
    ]
    .iter()
    .any(|w| l.contains(w))
}

impl Node {
    /// Replace secret values in this node and its children.
    pub fn redact(&mut self) {
        if self.value.is_some() && is_secret_field(&self.role, self.label.as_deref()) {
            self.value = Some(REDACTED.into());
        }
        for c in &mut self.children {
            c.redact();
        }
    }

    fn matches(&self, t: &Target) -> bool {
        if let Some(id) = &t.id
            && &self.id != id
        {
            return false;
        }
        if let Some(role) = &t.role
            && !self.role.eq_ignore_ascii_case(role)
        {
            return false;
        }
        if let Some(label) = &t.label
            && self.label.as_deref() != Some(label.as_str())
        {
            return false;
        }
        if let Some(part) = &t.label_contains {
            let hit = self
                .label
                .as_ref()
                .is_some_and(|l| l.to_lowercase().contains(&part.to_lowercase()));
            if !hit {
                return false;
            }
        }
        true
    }

    fn collect<'a>(&'a self, t: &Target, out: &mut Vec<&'a Node>) {
        if self.matches(t) {
            out.push(self);
        }
        for c in &self.children {
            c.collect(t, out);
        }
    }

    fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }
}

impl Snapshot {
    /// Every element matching `target`, in tree order. An empty target
    /// matches everything.
    #[must_use]
    pub fn find_all(&self, target: &Target) -> Vec<&Node> {
        let mut out = Vec::new();
        self.tree.collect(target, &mut out);
        out
    }

    /// The one element a target names.
    ///
    /// # Errors
    /// [`ResolveError`] when nothing matches, or several do and no index was
    /// given.
    pub fn resolve(&self, target: &Target) -> Result<&Node, ResolveError> {
        let all = self.find_all(target);
        let what = target.describe();
        match (target.index, all.len()) {
            (_, 0) => Err(ResolveError::NotFound(what)),
            (Some(i), n) => all.get(i).copied().ok_or(ResolveError::NotFound(format!(
                "{what} (index {i}, only {n} found)"
            ))),
            (None, 1) => Ok(all[0]),
            (None, count) => Err(ResolveError::Ambiguous { what, count }),
        }
    }

    /// Number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tree.count()
    }

    /// Always false: a tree has a root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Elements with bounds that begin inside the window but end past an edge
    /// by more than `tolerance` points: content the window cuts off. Elements
    /// wholly outside (scrolled away) are not counted.
    #[must_use]
    pub fn clipped(&self, tolerance: f32) -> Vec<&Node> {
        let (w, h) = (self.window.width, self.window.height);
        let mut out = Vec::new();
        let mut stack = vec![&self.tree];
        while let Some(n) = stack.pop() {
            if let Some(b) = &n.bounds
                && b.width > 0.0
                && b.height > 0.0
                && n.role != "window"
            {
                let starts_inside =
                    b.x < w && b.y < h && b.x + b.width > 0.0 && b.y + b.height > 0.0;
                let over = b.x + b.width > w + tolerance
                    || b.y + b.height > h + tolerance
                    || b.x < -tolerance
                    || b.y < -tolerance;
                if starts_inside && over {
                    out.push(n);
                }
            }
            stack.extend(n.children.iter());
        }
        out.reverse();
        out
    }
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub fn node(
        id: &str,
        role: &str,
        label: Option<&str>,
        b: Option<(f32, f32, f32, f32)>,
    ) -> Node {
        Node {
            id: id.into(),
            role: role.into(),
            label: label.map(Into::into),
            value: None,
            bounds: b.map(|(x, y, width, height)| Bounds {
                x,
                y,
                width,
                height,
            }),
            focused: false,
            enabled: true,
            children: vec![],
        }
    }

    pub fn snapshot() -> Snapshot {
        let mut root = node(
            "1",
            "window",
            Some("ClientLedger"),
            Some((0.0, 0.0, 800.0, 600.0)),
        );
        root.children = vec![
            node("2", "button", Some("Save"), Some((10.0, 10.0, 80.0, 24.0))),
            node(
                "3",
                "button",
                Some("Save & New"),
                Some((100.0, 10.0, 80.0, 24.0)),
            ),
            node("4", "tab", Some("Home"), Some((10.0, 50.0, 60.0, 24.0))),
            node("5", "tab", Some("Home"), Some((70.0, 50.0, 60.0, 24.0))),
        ];
        Snapshot {
            window: WindowInfo {
                width: 800.0,
                height: 600.0,
                pixels_per_point: 1.0,
            },
            focused: None,
            tree: root,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{node, snapshot};
    use super::*;

    fn t(label: Option<&str>, role: Option<&str>, index: Option<usize>) -> Target {
        Target {
            label: label.map(Into::into),
            role: role.map(Into::into),
            index,
            ..Target::default()
        }
    }

    #[test]
    fn resolves_by_label_role_id_and_contains() {
        let s = snapshot();
        assert_eq!(s.resolve(&t(Some("Save"), None, None)).unwrap().id, "2");
        assert_eq!(
            s.resolve(&t(None, Some("BUTTON"), Some(1))).unwrap().id,
            "3"
        );
        let by_id = Target {
            id: Some("4".into()),
            ..Target::default()
        };
        assert_eq!(s.resolve(&by_id).unwrap().role, "tab");
        let contains = Target {
            label_contains: Some("& new".into()),
            ..Target::default()
        };
        assert_eq!(s.resolve(&contains).unwrap().id, "3");
    }

    #[test]
    fn missing_and_ambiguous_targets_say_so() {
        let s = snapshot();
        assert!(matches!(
            s.resolve(&t(Some("Nope"), None, None)),
            Err(ResolveError::NotFound(_))
        ));
        assert!(matches!(
            s.resolve(&t(Some("Home"), None, None)),
            Err(ResolveError::Ambiguous { count: 2, .. })
        ));
        assert!(matches!(
            s.resolve(&t(Some("Home"), None, Some(5))),
            Err(ResolveError::NotFound(_))
        ));
    }

    #[test]
    fn secret_values_are_hidden() {
        let mut n = node("1", "text_input", Some("AI key"), None);
        n.value = Some("sk-live-123".into());
        let mut pw = node("2", "password_input", Some("Anything"), None);
        pw.value = Some("hunter2".into());
        let mut plain = node("3", "text_input", Some("Memo"), None);
        plain.value = Some("lunch".into());
        let mut named = node("4", "text_input", Some("Closing date password"), None);
        named.value = Some("abc".into());
        let mut root = node("0", "window", None, None);
        root.children = vec![n, pw, plain, named];
        root.redact();
        let vals: Vec<_> = root
            .children
            .iter()
            .map(|c| c.value.clone().unwrap())
            .collect();
        assert_eq!(vals[0], REDACTED);
        assert_eq!(vals[1], REDACTED);
        assert_eq!(vals[2], "lunch");
        assert_eq!(vals[3], REDACTED);
    }

    #[test]
    fn clipping_finds_content_cut_by_the_window_edge() {
        let mut s = snapshot();
        assert_eq!(s.clipped(1.0).len(), 0);
        s.tree.children.push(node(
            "9",
            "button",
            Some("Cut"),
            Some((780.0, 10.0, 60.0, 24.0)),
        ));
        // Wholly outside is scrolled away, not clipped.
        s.tree.children.push(node(
            "10",
            "button",
            Some("Away"),
            Some((900.0, 10.0, 60.0, 24.0)),
        ));
        let clipped = s.clipped(1.0);
        assert_eq!(clipped.len(), 1);
        assert_eq!(clipped[0].id, "9");
    }
}
