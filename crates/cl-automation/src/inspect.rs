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

/// Text that looks like a credential whatever field it sits in: an API key
/// prefix, a long unbroken mixed token, or a recovery key written as groups
/// joined by dashes.
#[must_use]
pub fn looks_like_secret_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    if ["sk-", "sk_", "ghp_", "xoxb-", "pk_live", "sk_live"]
        .iter()
        .any(|p| lower.starts_with(p))
        && t.len() >= 12
    {
        return true;
    }
    let letters = t.chars().any(|c| c.is_ascii_alphabetic());
    let digits = t.chars().any(|c| c.is_ascii_digit());
    if t.len() >= 24 && letters && digits {
        return true;
    }
    let groups: Vec<&str> = t.split('-').collect();
    groups.len() >= 4
        && groups
            .iter()
            .all(|g| (4..=8).contains(&g.len()) && g.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// A static label that shows a secret: it names one and carries a value
/// after a colon ("Recovery key: ABCD-..."), or is itself credential-shaped.
fn static_label_shows_secret(label: &str) -> bool {
    if looks_like_secret_text(label) {
        return true;
    }
    match label.split_once(':') {
        Some((name, rest)) => is_secret_field("label", Some(name)) && !rest.trim().is_empty(),
        None => false,
    }
}

impl Node {
    /// Hide secrets in this node and everything below it.
    ///
    /// A secret input hides its value and every descendant's text (egui puts
    /// the typed text in child text-run nodes too). Static text that shows a
    /// secret, and any value or label that looks like a credential, is hidden
    /// as well.
    pub fn redact(&mut self) {
        self.redact_inner(false);
    }

    fn redact_inner(&mut self, inside_secret: bool) {
        let secret_input =
            self.role.ends_with("input") && is_secret_field(&self.role, self.label.as_deref());
        if secret_input || inside_secret {
            if self.value.is_some() {
                self.value = Some(REDACTED.into());
            }
            if inside_secret && self.label.is_some() {
                self.label = Some(REDACTED.into());
            }
        } else {
            if self.role == "label" && self.label.as_deref().is_some_and(static_label_shows_secret)
            {
                self.label = Some(REDACTED.into());
            }
            if self.value.as_deref().is_some_and(looks_like_secret_text) {
                self.value = Some(REDACTED.into());
            }
        }
        let flag = secret_input || inside_secret;
        for c in &mut self.children {
            c.redact_inner(flag);
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
        self.clipped_excluding(tolerance, &[])
    }

    /// Like [`Snapshot::clipped`], skipping elements whose centre lies inside
    /// one of `ignore`: areas that scroll by design.
    #[must_use]
    pub fn clipped_excluding(&self, tolerance: f32, ignore: &[Bounds]) -> Vec<&Node> {
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
                let (cx, cy) = b.center();
                let skipped = ignore
                    .iter()
                    .any(|r| cx >= r.x && cx <= r.x + r.width && cy >= r.y && cy <= r.y + r.height);
                if starts_inside && over && !skipped {
                    out.push(n);
                }
            }
            stack.extend(n.children.iter());
        }
        out.reverse();
        out
    }

    /// Text inputs inside the window whose text runs past their own box
    /// by more than `tolerance` points (left or right): a field too narrow
    /// for what it shows, such as a name cut to "Plai". egui gives each
    /// laid-out row of a text input a `text_run` child with its bounds.
    /// Inputs whose centre lies inside one of `ignore` are not counted.
    #[must_use]
    pub fn truncated_inputs(&self, tolerance: f32, ignore: &[Bounds]) -> Vec<&Node> {
        let (w, h) = (self.window.width, self.window.height);
        let mut out = Vec::new();
        let mut stack = vec![&self.tree];
        while let Some(n) = stack.pop() {
            stack.extend(n.children.iter());
            if !n.role.ends_with("input") {
                continue;
            }
            let Some(b) = n.bounds.as_ref() else { continue };
            let shown = b.width > 0.0
                && b.height > 0.0
                && b.x >= 0.0
                && b.y >= 0.0
                && b.x + b.width <= w
                && b.y + b.height <= h;
            let (cx, cy) = b.center();
            let skipped = ignore
                .iter()
                .any(|r| cx >= r.x && cx <= r.x + r.width && cy >= r.y && cy <= r.y + r.height);
            if !shown || skipped {
                continue;
            }
            let cut = n.children.iter().any(|c| {
                c.role == "text_run"
                    && c.bounds.as_ref().is_some_and(|r| {
                        r.width > 0.0
                            && (r.x + r.width > b.x + b.width + tolerance || r.x < b.x - tolerance)
                    })
            });
            if cut {
                out.push(n);
            }
        }
        out.reverse();
        out
    }

    /// Pairs of controls and labels drawn over each other (by more than
    /// `tolerance` points both ways) inside the window, such as a menu
    /// title running into the next label. Only near relatives count: the
    /// two share a container that one of them sits in directly (content
    /// scrolled under a panel belongs to another part of the tree). One
    /// wholly inside the other is a layer or a composite widget and is not
    /// counted; neither are elements whose centre lies inside one of
    /// `ignore`.
    #[must_use]
    pub fn overlapping(&self, tolerance: f32, ignore: &[Bounds]) -> Vec<(&Node, &Node)> {
        const ROLES: [&str; 8] = [
            "button",
            "label",
            "link",
            "check_box",
            "text_input",
            "combo_box",
            "tab",
            "menu_item",
        ];
        let (w, h) = (self.window.width, self.window.height);
        // Every node with its parent's position in `all`.
        let mut all: Vec<(&Node, Option<usize>)> = Vec::new();
        let mut stack: Vec<(&Node, Option<usize>)> = vec![(&self.tree, None)];
        while let Some((n, parent)) = stack.pop() {
            let at = all.len();
            all.push((n, parent));
            stack.extend(n.children.iter().map(|c| (c, Some(at))));
        }
        let items: Vec<(usize, &Bounds)> = all
            .iter()
            .enumerate()
            .filter_map(|(i, (n, _))| {
                let b = n.bounds.as_ref()?;
                let shown = ROLES.contains(&n.role.as_str())
                    && b.width > 0.0
                    && b.height > 0.0
                    && b.x >= 0.0
                    && b.y >= 0.0
                    && b.x + b.width <= w
                    && b.y + b.height <= h;
                let (cx, cy) = b.center();
                let skipped = ignore
                    .iter()
                    .any(|r| cx >= r.x && cx <= r.x + r.width && cy >= r.y && cy <= r.y + r.height);
                (shown && !skipped).then_some((i, b))
            })
            .collect();
        let ancestors = |mut i: usize| {
            let mut out = Vec::new();
            while let Some(p) = all[i].1 {
                out.push(p);
                i = p;
            }
            out
        };
        let near = |a: usize, b: usize| {
            let up_a = ancestors(a);
            let common = ancestors(b).into_iter().find(|p| up_a.contains(p));
            common.is_some() && (common == all[a].1 || common == all[b].1)
        };
        let inside = |a: &Bounds, b: &Bounds| {
            a.x >= b.x - tolerance
                && a.y >= b.y - tolerance
                && a.x + a.width <= b.x + b.width + tolerance
                && a.y + a.height <= b.y + b.height + tolerance
        };
        let mut out = Vec::new();
        for (k, (ia, a)) in items.iter().enumerate() {
            for (ib, b) in &items[k + 1..] {
                let dx = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
                let dy = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
                if dx > tolerance
                    && dy > tolerance
                    && !inside(a, b)
                    && !inside(b, a)
                    && near(*ia, *ib)
                {
                    out.push((all[*ia].0, all[*ib].0));
                }
            }
        }
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
        let region = Bounds {
            x: 770.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        assert_eq!(s.clipped_excluding(1.0, &[region]).len(), 0);
    }

    #[test]
    fn truncation_finds_text_cut_inside_its_input() {
        let mut s = snapshot();
        let mut fits = node(
            "20",
            "text_input",
            Some("Line"),
            Some((10.0, 10.0, 200.0, 24.0)),
        );
        fits.children.push(node(
            "21",
            "text_run",
            None,
            Some((14.0, 14.0, 120.0, 16.0)),
        ));
        let mut cut = node(
            "22",
            "text_input",
            Some("Line"),
            Some((10.0, 40.0, 36.0, 24.0)),
        );
        cut.children
            .push(node("23", "text_run", None, Some((14.0, 44.0, 90.0, 16.0))));
        s.tree.children.push(fits);
        s.tree.children.push(cut);
        let found = s.truncated_inputs(1.0, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "22");
        let region = Bounds {
            x: 0.0,
            y: 30.0,
            width: 100.0,
            height: 50.0,
        };
        assert!(s.truncated_inputs(1.0, &[region]).is_empty());
    }

    #[test]
    fn overlapping_finds_controls_drawn_over_each_other() {
        let mut s = snapshot();
        assert_eq!(s.overlapping(1.0, &[]).len(), 0);
        // "Help" running into the "Go to" label.
        s.tree.children.push(node(
            "9",
            "label",
            Some("Go to"),
            Some((170.0, 12.0, 40.0, 18.0)),
        ));
        // A label inside a button is a composite, not an overlap; touching
        // edges are not either.
        s.tree.children.push(node(
            "10",
            "label",
            Some("Save"),
            Some((20.0, 14.0, 30.0, 16.0)),
        ));
        s.tree.children.push(node(
            "11",
            "button",
            Some("Next"),
            Some((210.0, 10.0, 40.0, 24.0)),
        ));
        let pairs = s.overlapping(1.0, &[]);
        let ids: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(a, b)| (a.id.as_str(), b.id.as_str()))
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        let (a, b) = ids[0];
        assert!([a, b].contains(&"3") && [a, b].contains(&"9"), "{ids:?}");
        let region = Bounds {
            x: 160.0,
            y: 0.0,
            width: 60.0,
            height: 40.0,
        };
        assert_eq!(s.overlapping(1.0, &[region]).len(), 0);
        // Content in another part of the tree (scrolled under a panel) is
        // not a near relative of the panel's labels.
        let mut s = snapshot();
        let mut panel = node("20", "pane", None, Some((0.0, 560.0, 800.0, 40.0)));
        let mut bar = node(
            "21",
            "generic_container",
            None,
            Some((0.0, 560.0, 800.0, 40.0)),
        );
        bar.children.push(node(
            "22",
            "label",
            Some("Single-user"),
            Some((10.0, 570.0, 80.0, 16.0)),
        ));
        panel.children.push(bar);
        let mut scroll = node("30", "scroll_view", None, Some((0.0, 0.0, 800.0, 560.0)));
        let mut content = node(
            "31",
            "generic_container",
            None,
            Some((0.0, 0.0, 800.0, 600.0)),
        );
        content.children.push(node(
            "32",
            "text_input",
            Some("Account"),
            Some((5.0, 550.0, 200.0, 30.0)),
        ));
        scroll.children.push(content);
        s.tree.children.push(panel);
        s.tree.children.push(scroll);
        assert_eq!(s.overlapping(1.0, &[]).len(), 0);
        // A sibling one level down still counts ("Go to" in the bar's
        // right-to-left part, "Help" in the bar itself).
        let mut s = snapshot();
        let mut part = node(
            "40",
            "generic_container",
            None,
            Some((150.0, 0.0, 650.0, 40.0)),
        );
        part.children.push(node(
            "41",
            "label",
            Some("Go to"),
            Some((170.0, 12.0, 40.0, 18.0)),
        ));
        s.tree.children.push(part);
        assert_eq!(s.overlapping(1.0, &[]).len(), 1);
    }

    #[test]
    fn text_run_children_of_a_secret_input_are_hidden_too() {
        let mut field = node("1", "text_input", Some("AI key"), None);
        field.value = Some("sk-live-123".into());
        let mut run = node("2", "text_run", Some("sk-live-123"), None);
        run.value = Some("sk-live-123".into());
        let mut inner = node("3", "text_run", Some("123"), None);
        inner.value = Some("123".into());
        run.children.push(inner);
        field.children.push(run);
        let mut root = node("0", "window", None, None);
        root.children.push(field);
        root.redact();
        let text = serde_json::to_string(&root).unwrap();
        assert!(
            !text.contains("sk-live-123") && !text.contains("\"123\""),
            "{text}"
        );
    }

    #[test]
    fn static_text_that_shows_a_secret_is_hidden() {
        let mut root = node("0", "window", None, None);
        root.children = vec![
            node(
                "1",
                "label",
                Some("Recovery key: ABCD-EFGH-JKLM-NPQR"),
                None,
            ),
            node("2", "label", Some("ABCD-EFGH-JKLM-NPQR-STUV"), None),
            node("3", "label", Some("Recovery key"), None),
            node("4", "label", Some("Books closed through 09/30/2026"), None),
        ];
        root.redact();
        let l: Vec<_> = root
            .children
            .iter()
            .map(|c| c.label.clone().unwrap())
            .collect();
        assert_eq!(l[0], REDACTED);
        assert_eq!(l[1], REDACTED);
        assert_eq!(l[2], "Recovery key", "a heading is not a secret");
        assert_eq!(l[3], "Books closed through 09/30/2026");
    }
}
