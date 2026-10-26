//! Scenario files: YAML lists of actions and assertions, run once per theme.
//!
//! ```yaml
//! name: shell
//! themes: [light, dark]
//! steps:
//!   - assert_exists: {role: tab, label: Home}
//!   - key: Ctrl+K
//!   - assert_state: {key: palette_open, equals: true}
//!   - screenshot: palette
//! ```
//!
//! Key chords use the spelling in DESIGN.md. `Ctrl` (or `Cmd`) is the
//! keymap's command modifier, so `Ctrl+K` presses Cmd+K on macOS.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::actions::{MAIN_VIEWPORT, ScrollTo, Target, is_viewport};
use crate::client::{ClientError, Transport};
use crate::inspect::{Bounds, Snapshot};

/// How long an assertion keeps retrying before it fails.
const ASSERT_TIMEOUT: Duration = Duration::from_secs(3);
const ASSERT_POLL: Duration = Duration::from_millis(60);

/// A scenario file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Short name (also the screen name for `ui-check`).
    pub name: String,
    /// What it checks.
    #[serde(default)]
    pub description: String,
    /// Themes to run, in order. Default: light then dark.
    #[serde(default = "default_themes")]
    pub themes: Vec<String>,
    /// The steps.
    #[serde(with = "serde_yaml_ng::with::singleton_map_recursive")]
    pub steps: Vec<Step>,
    /// Steps that bring the screen's main view back for `ui-check
    /// --compare` (run once, light theme, after `steps`, before the compare
    /// screenshot). Optional.
    #[serde(default, with = "serde_yaml_ng::with::singleton_map_recursive")]
    pub compare: Vec<Step>,
}

fn default_themes() -> Vec<String> {
    vec!["light".into(), "dark".into()]
}

/// A target in one window: the [`Target`] fields plus an optional
/// `viewport` (`main`, the default, or `doc_viewer`), written flat:
/// `click: {role: button, label: Save, viewport: doc_viewer}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "TargetInRaw")]
pub struct TargetIn {
    /// Which element.
    pub target: Target,
    /// Which window (`None`: the main window).
    pub viewport: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInRaw {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    label_contains: Option<String>,
    #[serde(default)]
    description_contains: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    viewport: Option<String>,
}

impl From<TargetInRaw> for TargetIn {
    fn from(r: TargetInRaw) -> TargetIn {
        TargetIn {
            target: Target {
                id: r.id,
                label: r.label,
                label_contains: r.label_contains,
                description_contains: r.description_contains,
                role: r.role,
                index: r.index,
            },
            viewport: r.viewport,
        }
    }
}

impl From<Target> for TargetIn {
    fn from(target: Target) -> TargetIn {
        TargetIn {
            target,
            viewport: None,
        }
    }
}

impl TargetIn {
    fn describe(&self) -> String {
        format!(
            "{}{}",
            self.target.describe(),
            in_window(self.viewport.as_ref())
        )
    }
}

/// ` in the doc_viewer window` for a step aimed at another window.
fn in_window(viewport: Option<&String>) -> String {
    match viewport {
        Some(v) if v != MAIN_VIEWPORT => format!(" in the {v} window"),
        _ => String::new(),
    }
}

/// A key step: `key: Ctrl+K`, or `key: {key: F6, viewport: doc_viewer}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "KeyRaw")]
pub struct KeyStep {
    /// The chord.
    pub key: String,
    /// Which window (`None`: the main window).
    pub viewport: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KeyRaw {
    Plain(String),
    In(KeyInRaw),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInRaw {
    key: String,
    #[serde(default)]
    viewport: Option<String>,
}

impl From<KeyRaw> for KeyStep {
    fn from(r: KeyRaw) -> KeyStep {
        match r {
            KeyRaw::Plain(key) => KeyStep {
                key,
                viewport: None,
            },
            KeyRaw::In(k) => KeyStep {
                key: k.key,
                viewport: k.viewport,
            },
        }
    }
}

/// A screenshot step: `screenshot: home`, or
/// `screenshot: {name: viewer, viewport: doc_viewer}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "ShotRaw")]
pub struct ShotStep {
    /// File name stem.
    pub name: String,
    /// Which window (`None`: the main window).
    pub viewport: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ShotRaw {
    Plain(String),
    In(ShotInRaw),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShotInRaw {
    name: String,
    #[serde(default)]
    viewport: Option<String>,
}

impl From<ShotRaw> for ShotStep {
    fn from(r: ShotRaw) -> ShotStep {
        match r {
            ShotRaw::Plain(name) => ShotStep {
                name,
                viewport: None,
            },
            ShotRaw::In(s) => ShotStep {
                name: s.name,
                viewport: s.viewport,
            },
        }
    }
}

/// One step. Steps that act on a window take an optional `viewport`
/// (`main`, the default, or `doc_viewer`); a step without one acts on the
/// main window exactly as before A02.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Resize a window (points).
    Resize {
        /// Width.
        width: f32,
        /// Height.
        height: f32,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Move a window's outer top-left corner (desktop points).
    Move {
        /// Left edge.
        x: f32,
        /// Top edge.
        y: f32,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Set the app zoom.
    SetZoom {
        /// 80 to 200.
        percent: u16,
    },
    /// Click an element.
    Click(TargetIn),
    /// Click an element if one matches right now; otherwise do nothing
    /// (for steps that depend on an earlier run's state, such as a dirty
    /// window that may or may not prompt).
    ClickIfPresent(TargetIn),
    /// Focus an element.
    Focus(TargetIn),
    /// Turn the mouse wheel over an element until what scrolls there (a
    /// form's body) reaches its start or end.
    Scroll {
        /// Where the pointer goes.
        target: Target,
        /// `start` or `end`.
        to: ScrollTo,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Move the pointer over an element without clicking (its tooltip
    /// shows after egui's delay; assert it with a later step).
    Hover(TargetIn),
    /// Type text. `{theme}` in the text becomes the theme being run, so a
    /// record a scenario creates gets a name of its own in each run (lists
    /// whose names stay taken after a record is made inactive).
    Type {
        /// The text.
        text: String,
        /// Focus this first.
        #[serde(default)]
        target: Option<Target>,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Paste text as one clipboard paste (a spreadsheet block: tabs between
    /// cells, a line per row). `{theme}` is replaced as in `type`.
    Paste {
        /// The text.
        text: String,
        /// Focus this first.
        #[serde(default)]
        target: Option<Target>,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Press a key chord.
    Key(KeyStep),
    /// Run a command id.
    Command(String),
    /// Answer the next file or folder window the app opens (none is shown
    /// in a controlled run): `paths` chosen, or `cancel: true`.
    DialogAnswer {
        /// The chosen paths (`{scenario_dir}` works here).
        #[serde(default)]
        paths: Vec<String>,
        /// Cancel instead.
        #[serde(default)]
        cancel: bool,
    },
    /// Pause.
    Wait {
        /// Milliseconds.
        ms: u64,
    },
    /// An element exists.
    AssertExists(TargetIn),
    /// No element matches.
    AssertNotExists(TargetIn),
    /// How many elements match.
    AssertCount {
        /// What to count.
        target: Target,
        /// Expected number.
        count: usize,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// An element's label equals or contains text.
    AssertText {
        /// Which element.
        target: Target,
        /// Exact label.
        #[serde(default)]
        equals: Option<String>,
        /// Label contains (case-insensitive).
        #[serde(default)]
        contains: Option<String>,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// An input's value equals text.
    AssertValue {
        /// Which element.
        target: Target,
        /// Expected value.
        equals: String,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// An element has keyboard focus.
    AssertFocused(TargetIn),
    /// An element is enabled.
    AssertEnabled(TargetIn),
    /// An element is disabled.
    AssertDisabled(TargetIn),
    /// A value in `state` (dotted path such as `window.width`; a number
    /// picks an array element, `screen.rows.0.date`) equals this.
    AssertState {
        /// Dotted path.
        key: String,
        /// Expected JSON value.
        equals: Value,
    },
    /// No element is cut off by the window edge, drawn over another, or
    /// showing text cut inside its own text field.
    AssertNoClipping {
        /// Points of slack (default 1).
        #[serde(default)]
        tolerance: Option<f32>,
        /// Areas that scroll by design (the rail, the tab strip): elements
        /// centred inside are not counted.
        #[serde(default)]
        ignore: Vec<Bounds>,
        /// Which window.
        #[serde(default)]
        viewport: Option<String>,
    },
    /// Save a screenshot named `<name>-<theme>.png`; one of another window
    /// is `<name>-<viewport>-<theme>.png`, so it never overwrites the main
    /// window's picture.
    Screenshot(ShotStep),
}

impl Step {
    /// The window this step acts on, when it names one.
    #[must_use]
    pub fn viewport(&self) -> Option<&String> {
        match self {
            Step::Resize { viewport, .. }
            | Step::Move { viewport, .. }
            | Step::Scroll { viewport, .. }
            | Step::Type { viewport, .. }
            | Step::Paste { viewport, .. }
            | Step::AssertCount { viewport, .. }
            | Step::AssertText { viewport, .. }
            | Step::AssertValue { viewport, .. }
            | Step::AssertNoClipping { viewport, .. } => viewport.as_ref(),
            Step::Click(t)
            | Step::ClickIfPresent(t)
            | Step::Focus(t)
            | Step::Hover(t)
            | Step::AssertExists(t)
            | Step::AssertNotExists(t)
            | Step::AssertFocused(t)
            | Step::AssertEnabled(t)
            | Step::AssertDisabled(t) => t.viewport.as_ref(),
            Step::Key(k) => k.viewport.as_ref(),
            Step::Screenshot(s) => s.viewport.as_ref(),
            Step::SetZoom { .. }
            | Step::Command(_)
            | Step::DialogAnswer { .. }
            | Step::Wait { .. }
            | Step::AssertState { .. } => None,
        }
    }

    /// Short description for reports.
    #[must_use]
    pub fn describe(&self) -> String {
        let at = in_window(self.viewport());
        match self {
            Step::Resize { width, height, .. } => format!("resize{at} to {width}x{height}"),
            Step::Move { x, y, .. } => format!("move{at} to {x},{y}"),
            Step::SetZoom { percent } => format!("zoom {percent}%"),
            Step::Click(t) => format!("click {}", t.describe()),
            Step::ClickIfPresent(t) => format!("click {} if present", t.describe()),
            Step::Focus(t) => format!("focus {}", t.describe()),
            Step::Scroll { target, to, .. } => {
                format!("scroll {}{at} to the {to:?}", target.describe())
            }
            Step::Hover(t) => format!("hover over {}", t.describe()),
            Step::Type { text, .. } => format!("type {} characters{at}", text.chars().count()),
            Step::Paste { text, .. } => {
                format!("paste {} line(s){at}", text.lines().count().max(1))
            }
            Step::Key(k) => format!("key {}{at}", k.key),
            Step::Command(c) => format!("command {c}"),
            Step::DialogAnswer { paths, cancel } => {
                if *cancel || paths.is_empty() {
                    "answer the next file window: cancel".into()
                } else {
                    // Paths can be owner data: only how many.
                    format!("answer the next file window with {} path(s)", paths.len())
                }
            }
            Step::Wait { ms } => format!("wait {ms} ms"),
            Step::AssertExists(t) => format!("expect {} to exist", t.describe()),
            Step::AssertNotExists(t) => format!("expect {} not to exist", t.describe()),
            Step::AssertCount { target, count, .. } => {
                format!("expect {count} of {}{at}", target.describe())
            }
            Step::AssertText { target, .. } => {
                format!("expect text of {}{at}", target.describe())
            }
            Step::AssertValue { target, .. } => {
                format!("expect value of {}{at}", target.describe())
            }
            Step::AssertFocused(t) => format!("expect {} to be focused", t.describe()),
            Step::AssertEnabled(t) => format!("expect {} to be enabled", t.describe()),
            Step::AssertDisabled(t) => format!("expect {} to be disabled", t.describe()),
            Step::AssertState { key, .. } => format!("expect state {key}"),
            Step::AssertNoClipping { .. } => format!("expect nothing clipped{at}"),
            Step::Screenshot(s) => format!("screenshot {}{at}", s.name),
        }
    }
}

/// A scenario that could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    /// The file could not be read.
    #[error("the scenario file could not be read: {0}")]
    Io(#[from] std::io::Error),
    /// The YAML is wrong.
    #[error("the scenario is not valid: {0}")]
    Parse(String),
}

impl Scenario {
    /// Parse YAML text.
    ///
    /// # Errors
    /// [`ScenarioError::Parse`] with the YAML error and its position.
    pub fn parse(text: &str) -> Result<Scenario, ScenarioError> {
        let s: Scenario =
            serde_yaml_ng::from_str(text).map_err(|e| ScenarioError::Parse(e.to_string()))?;
        if s.steps.is_empty() {
            return Err(ScenarioError::Parse(
                "a scenario needs at least one step".into(),
            ));
        }
        if let Some(bad) = s
            .themes
            .iter()
            .find(|t| !matches!(t.as_str(), "light" | "dark"))
        {
            return Err(ScenarioError::Parse(format!(
                "theme `{bad}` must be light or dark"
            )));
        }
        for (i, step) in s.steps.iter().chain(&s.compare).enumerate() {
            if let Some(v) = step.viewport()
                && !is_viewport(v)
            {
                return Err(ScenarioError::Parse(format!(
                    "step {}: viewport `{v}` must be main or doc_viewer",
                    i + 1
                )));
            }
        }
        Ok(s)
    }

    /// The `compare` steps as a scenario of their own (light theme only);
    /// `None` when the scenario has none.
    #[must_use]
    pub fn compare_run(&self) -> Option<Scenario> {
        (!self.compare.is_empty()).then(|| Scenario {
            name: self.name.clone(),
            description: self.description.clone(),
            themes: vec!["light".into()],
            steps: self.compare.clone(),
            compare: Vec::new(),
        })
    }

    /// Read a file.
    ///
    /// # Errors
    /// I/O or parse errors.
    pub fn load(path: &Path) -> Result<Scenario, ScenarioError> {
        let text = std::fs::read_to_string(path)?;
        Scenario::parse(&with_scenario_dir(&text, path))
    }
}

/// A step that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepFailure {
    /// Position in the scenario (from 1).
    pub step: usize,
    /// What the step was.
    pub what: String,
    /// Why it failed.
    pub message: String,
}

/// The result for one theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeOutcome {
    /// `light` or `dark`.
    pub theme: String,
    /// Steps that passed.
    pub steps_passed: usize,
    /// The failure, if any (the run stops there for this theme).
    pub failure: Option<StepFailure>,
}

/// The result of a whole scenario.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// One per theme.
    pub outcomes: Vec<ThemeOutcome>,
    /// Screenshots written.
    pub screenshots: Vec<PathBuf>,
}

impl Report {
    /// True when every theme passed every step.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.outcomes.is_empty() && self.outcomes.iter().all(|o| o.failure.is_none())
    }

    /// Plain-text summary.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = String::new();
        for o in &self.outcomes {
            match &o.failure {
                None => {
                    let _ = writeln!(out, "{}: passed ({} steps)", o.theme, o.steps_passed);
                }
                Some(f) => {
                    let _ = writeln!(
                        out,
                        "{}: FAILED at step {} ({}): {}",
                        o.theme, f.step, f.what, f.message
                    );
                }
            }
        }
        for s in &self.screenshots {
            let _ = writeln!(out, "screenshot: {}", s.display());
        }
        out
    }
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn call(t: &mut dyn Transport, method: &str, params: Value) -> Result<Value, String> {
    t.call(method, params).map_err(|e| match e {
        ClientError::Remote(b) => b.message,
        other => other.to_string(),
    })
}

/// `params` with `viewport` added when the step names a window. A step
/// without one sends exactly what it sent before A02.
fn at(mut params: Value, viewport: Option<&String>) -> Value {
    if let (Some(v), Some(map)) = (viewport, params.as_object_mut()) {
        map.insert("viewport".into(), Value::String(v.clone()));
    }
    params
}

fn snapshot(t: &mut dyn Transport, viewport: Option<&String>) -> Result<Snapshot, String> {
    let v = call(t, "inspect", at(json!({}), viewport))?;
    serde_json::from_value(v).map_err(|_| "the element tree could not be read".to_string())
}

/// A value at a dotted path; a number picks an array element
/// (`screen.rows.0.date`).
fn dotted<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, key| match cur {
        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => cur.get(key),
    })
}

/// Retry `check` against fresh snapshots of one window until it passes or
/// time runs out.
fn eventually(
    t: &mut dyn Transport,
    viewport: Option<&String>,
    check: impl Fn(&Snapshot) -> Result<(), String>,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let result = snapshot(t, viewport).and_then(|s| check(&s));
        match result {
            Ok(()) => return Ok(()),
            Err(m) if start.elapsed() >= ASSERT_TIMEOUT => return Err(m),
            Err(_) => std::thread::sleep(ASSERT_POLL),
        }
    }
}

/// Wait until `target` resolves in its window.
fn appears(
    t: &mut dyn Transport,
    target: &Target,
    viewport: Option<&String>,
) -> Result<(), String> {
    eventually(t, viewport, |s| {
        s.resolve(target).map(drop).map_err(|e| e.to_string())
    })
}

fn expect_node(
    t: &mut dyn Transport,
    target: &TargetIn,
    ok: impl Fn(&crate::inspect::Node) -> Result<(), String>,
) -> Result<(), String> {
    expect_node_in(t, &target.target, target.viewport.as_ref(), ok)
}

fn expect_node_in(
    t: &mut dyn Transport,
    target: &Target,
    viewport: Option<&String>,
    ok: impl Fn(&crate::inspect::Node) -> Result<(), String>,
) -> Result<(), String> {
    eventually(t, viewport, |s| {
        let node = s.resolve(target).map_err(|e| e.to_string())?;
        ok(node)
    })
}

/// How long `click_if_present` waits for its element.
const PRESENCE_WAIT: Duration = Duration::from_millis(600);

/// Clicks `target` if it appears within [`PRESENCE_WAIT`]; otherwise does
/// nothing (a dialog that only some runs raise).
fn click_if_present(t: &mut dyn Transport, target: &TargetIn) -> Result<(), String> {
    let viewport = target.viewport.as_ref();
    let start = Instant::now();
    loop {
        if !snapshot(t, viewport)?.find_all(&target.target).is_empty() {
            return call(t, "click", at(json!({"target": target.target}), viewport)).map(drop);
        }
        if start.elapsed() >= PRESENCE_WAIT {
            return Ok(());
        }
        std::thread::sleep(ASSERT_POLL);
    }
}

/// `text` with `{scenario_dir}` replaced by the folder of the scenario file
/// (absolute, forward slashes), so a step can type the path of a fixture
/// file kept next to the scenarios.
fn with_scenario_dir(text: &str, path: &Path) -> String {
    if !text.contains("{scenario_dir}") {
        return text.to_owned();
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    text.replace(
        "{scenario_dir}",
        &dir.display().to_string().replace('\\', "/"),
    )
}

/// `text` with `{theme}` replaced by `theme`.
fn themed(text: &str, theme: &str) -> String {
    text.replace("{theme}", theme)
}

/// The PNG for a screenshot: `<name>-<theme>.png` for the main window,
/// `<name>-<viewport>-<theme>.png` for another, so the two never overwrite
/// each other.
fn shot_path(out_dir: &Path, name: &str, viewport: Option<&String>, theme: &str) -> PathBuf {
    match viewport {
        Some(v) if v != MAIN_VIEWPORT => {
            out_dir.join(format!("{}-{}-{theme}.png", safe_name(name), safe_name(v)))
        }
        _ => out_dir.join(format!("{}-{theme}.png", safe_name(name))),
    }
}

fn clipping(s: &Snapshot, tol: f32, ignore: &[Bounds]) -> Result<(), String> {
    // Parts wider than their room: cut by a panel, not the window edge.
    let wide = s.overflowing(ignore);
    if !wide.is_empty() {
        let names: Vec<String> = wide
            .iter()
            .take(5)
            .map(|o| {
                format!(
                    "{} (x {:.0} to {:.0})",
                    o.what,
                    o.bounds.x,
                    o.bounds.x + o.bounds.width
                )
            })
            .collect();
        return Err(format!(
            "{} part(s) are wider than the room they were given: {}",
            wide.len(),
            names.join(", ")
        ));
    }
    let clipped = s.clipped_excluding(tol, ignore);
    let overlaps = s.overlapping(tol, ignore);
    let truncated = s.truncated_inputs(tol, ignore);
    if clipped.is_empty() && overlaps.is_empty() && truncated.is_empty() {
        Ok(())
    } else if clipped.is_empty() && overlaps.is_empty() {
        let names: Vec<String> = truncated
            .iter()
            .take(5)
            .map(|n| format!("{} \"{}\"", n.role, n.label.as_deref().unwrap_or("")))
            .collect();
        Err(format!(
            "{} text field(s) are too narrow for their text: {}",
            truncated.len(),
            names.join(", ")
        ))
    } else if clipped.is_empty() {
        let names: Vec<String> = overlaps
            .iter()
            .take(5)
            .map(|(a, b)| {
                format!(
                    "{} \"{}\" / {} \"{}\"",
                    a.role,
                    a.label.as_deref().unwrap_or(""),
                    b.role,
                    b.label.as_deref().unwrap_or("")
                )
            })
            .collect();
        Err(format!(
            "{} pair(s) of elements are drawn over each other: {}",
            overlaps.len(),
            names.join(", ")
        ))
    } else {
        let names: Vec<String> = clipped
            .iter()
            .take(5)
            .map(|n| format!("{} \"{}\"", n.role, n.label.as_deref().unwrap_or("")))
            .collect();
        Err(format!(
            "{} element(s) are cut off by the window edge: {}",
            clipped.len(),
            names.join(", ")
        ))
    }
}

#[allow(clippy::too_many_lines)]
fn run_step(
    t: &mut dyn Transport,
    step: &Step,
    theme: &str,
    out_dir: &Path,
    shots: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let vp = step.viewport();
    match step {
        Step::Resize { width, height, .. } => call(
            t,
            "resize",
            at(json!({"width": width, "height": height}), vp),
        )
        .map(drop),
        Step::Move { x, y, .. } => call(t, "move", at(json!({"x": x, "y": y}), vp)).map(drop),
        Step::SetZoom { percent } => call(t, "set_zoom", json!({"percent": percent})).map(drop),
        Step::Click(target) => {
            // The element may still be appearing.
            appears(t, &target.target, vp)?;
            call(t, "click", at(json!({"target": target.target}), vp)).map(drop)
        }
        Step::ClickIfPresent(target) => click_if_present(t, target),
        Step::Focus(target) => {
            appears(t, &target.target, vp)?;
            call(t, "focus", at(json!({"target": target.target}), vp)).map(drop)
        }
        Step::Scroll { target, to, .. } => {
            appears(t, target, vp)?;
            call(t, "scroll", at(json!({"target": target, "to": to}), vp)).map(drop)
        }
        Step::Hover(target) => {
            appears(t, &target.target, vp)?;
            call(t, "hover", at(json!({"target": target.target}), vp)).map(drop)
        }
        Step::Type { text, target, .. } => {
            let text = themed(text, theme);
            call(t, "type", at(json!({"text": text, "target": target}), vp)).map(drop)
        }
        Step::Paste { text, target, .. } => {
            let text = themed(text, theme);
            call(t, "paste", at(json!({"text": text, "target": target}), vp)).map(drop)
        }
        Step::Key(k) => call(t, "key", at(json!({"key": k.key}), vp)).map(drop),
        Step::Command(c) => call(t, "command", json!({"id": c})).map(drop),
        Step::DialogAnswer { paths, cancel } => call(
            t,
            "dialog_answer",
            json!({"paths": paths, "cancel": cancel}),
        )
        .map(drop),
        Step::Wait { ms } => {
            std::thread::sleep(Duration::from_millis(*ms));
            Ok(())
        }
        Step::AssertExists(target) => appears(t, &target.target, vp),
        Step::AssertNotExists(target) => eventually(t, vp, |s| {
            let n = s.find_all(&target.target).len();
            if n == 0 {
                Ok(())
            } else {
                Err(format!(
                    "{n} element(s) match {} but none should",
                    target.describe()
                ))
            }
        }),
        Step::AssertCount { target, count, .. } => eventually(t, vp, |s| {
            let n = s.find_all(target).len();
            if n == *count {
                Ok(())
            } else {
                Err(format!(
                    "found {n} of {}, expected {count}",
                    target.describe()
                ))
            }
        }),
        Step::AssertText {
            target,
            equals,
            contains,
            ..
        } => {
            if equals.is_none() && contains.is_none() {
                return Err("assert_text needs equals or contains".into());
            }
            expect_node_in(t, target, vp, |n| {
                let label = n.label.as_deref().unwrap_or("");
                if let Some(e) = equals
                    && label != e
                {
                    return Err(format!("text is \"{label}\", expected \"{e}\""));
                }
                if let Some(c) = contains
                    && !label.to_lowercase().contains(&c.to_lowercase())
                {
                    return Err(format!("text \"{label}\" does not contain \"{c}\""));
                }
                Ok(())
            })
        }
        Step::AssertValue { target, equals, .. } => expect_node_in(t, target, vp, |n| {
            let v = n.value.as_deref().unwrap_or("");
            if v == equals {
                Ok(())
            } else {
                Err(format!("value is \"{v}\", expected \"{equals}\""))
            }
        }),
        Step::AssertFocused(target) => expect_node(t, target, |n| {
            if n.focused {
                Ok(())
            } else {
                Err("it does not have focus".into())
            }
        }),
        Step::AssertEnabled(target) => expect_node(t, target, |n| {
            if n.enabled {
                Ok(())
            } else {
                Err("it is disabled".into())
            }
        }),
        Step::AssertDisabled(target) => expect_node(t, target, |n| {
            if n.enabled {
                Err("it is enabled".into())
            } else {
                Ok(())
            }
        }),
        Step::AssertState { key, equals } => {
            let start = Instant::now();
            loop {
                let state = call(t, "state", json!({}))?;
                let got = dotted(&state, key).cloned().unwrap_or(Value::Null);
                if &got == equals {
                    return Ok(());
                }
                if start.elapsed() >= ASSERT_TIMEOUT {
                    return Err(format!("state {key} is {got}, expected {equals}"));
                }
                std::thread::sleep(ASSERT_POLL);
            }
        }
        Step::AssertNoClipping {
            tolerance, ignore, ..
        } => {
            let tol = tolerance.unwrap_or(1.0);
            eventually(t, vp, |s| clipping(s, tol, ignore))
        }
        Step::Screenshot(shot) => {
            let path = shot_path(out_dir, &shot.name, vp, theme);
            // Let the last input finish drawing.
            std::thread::sleep(Duration::from_millis(150));
            call(
                t,
                "screenshot",
                at(json!({"path": path.display().to_string()}), vp),
            )?;
            shots.push(path);
            Ok(())
        }
    }
}

/// Run a scenario once per theme, saving screenshots under `out_dir`.
pub fn run(t: &mut dyn Transport, scenario: &Scenario, out_dir: &Path) -> Report {
    let mut report = Report::default();
    if let Err(e) = std::fs::create_dir_all(out_dir) {
        report.outcomes.push(ThemeOutcome {
            theme: String::new(),
            steps_passed: 0,
            failure: Some(StepFailure {
                step: 0,
                what: "prepare".into(),
                message: format!("the output folder could not be created: {e}"),
            }),
        });
        return report;
    }
    for theme in &scenario.themes {
        let mut outcome = ThemeOutcome {
            theme: theme.clone(),
            steps_passed: 0,
            failure: None,
        };
        if let Err(message) = call(t, "set_theme", json!({"theme": theme})) {
            outcome.failure = Some(StepFailure {
                step: 0,
                what: "set theme".into(),
                message,
            });
            report.outcomes.push(outcome);
            continue;
        }
        // Every theme starts at 100% zoom: a theme that stopped mid-zoom
        // would otherwise leave the next theme's `resize` (sent in points,
        // which the window multiplies by the zoom) at twice the size.
        if let Err(message) = call(t, "set_zoom", json!({"percent": 100})) {
            outcome.failure = Some(StepFailure {
                step: 0,
                what: "reset zoom".into(),
                message,
            });
            report.outcomes.push(outcome);
            continue;
        }
        for (i, step) in scenario.steps.iter().enumerate() {
            match run_step(t, step, theme, out_dir, &mut report.screenshots) {
                Ok(()) => outcome.steps_passed += 1,
                Err(message) => {
                    // Keep a picture of what was on screen, and of the
                    // other window when the step was aimed at it.
                    let path = out_dir.join(format!("failure-{theme}.png"));
                    if call(t, "screenshot", json!({"path": path.display().to_string()})).is_ok() {
                        report.screenshots.push(path);
                    }
                    if let Some(v) = step.viewport().filter(|v| *v != MAIN_VIEWPORT) {
                        let path = shot_path(out_dir, "failure", Some(v), theme);
                        let params = at(json!({"path": path.display().to_string()}), Some(v));
                        if call(t, "screenshot", params).is_ok() {
                            report.screenshots.push(path);
                        }
                    }
                    outcome.failure = Some(StepFailure {
                        step: i + 1,
                        what: step.describe(),
                        message,
                    });
                    break;
                }
            }
        }
        report.outcomes.push(outcome);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::fixture::snapshot as fixture_snapshot;

    /// Answers `inspect` with a fixed tree and records the other calls.
    struct Fake {
        calls: Vec<(String, Value)>,
        snapshot: Snapshot,
        state: Value,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                calls: vec![],
                snapshot: fixture_snapshot(),
                state: json!({"palette_open": false, "window": {"width": 800.0}}),
            }
        }
    }

    impl Transport for Fake {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
            self.calls.push((method.to_string(), params));
            match method {
                "inspect" => Ok(serde_json::to_value(&self.snapshot).unwrap()),
                "state" => Ok(self.state.clone()),
                _ => Ok(json!({})),
            }
        }
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn parses_yaml_with_defaults() {
        let s = Scenario::parse(
            "name: x\nsteps:\n  - key: Ctrl+K\n  - click: {label: Save}\n  - assert_exists: {role: tab}\n  - screenshot: home\n  - assert_no_clipping: {}\n  - assert_text: {target: {label: Save}, equals: Save}\n  - set_zoom: {percent: 200}\n",
        )
        .unwrap();
        assert_eq!(s.themes, vec!["light", "dark"]);
        assert_eq!(s.steps.len(), 7);
    }

    #[test]
    fn dialog_answers_parse_and_describe_no_path() {
        let s = Scenario::parse(
            "name: x\nsteps:\n  - dialog_answer: {paths: ['C:/Owner/Scans']}\n  - dialog_answer: {cancel: true}\n",
        )
        .unwrap();
        assert_eq!(
            s.steps[0],
            Step::DialogAnswer {
                paths: vec!["C:/Owner/Scans".into()],
                cancel: false,
            }
        );
        assert!(!s.steps[0].describe().contains("Owner"));
        assert!(s.steps[1].describe().contains("cancel"));
        assert!(crate::tools::is_method("dialog_answer"));
    }

    #[test]
    fn scroll_sends_the_target_and_the_end() {
        let s = Scenario::parse(
            "name: x\nthemes: [light]\nsteps:\n  - scroll: {target: {id: '2'}, to: end}\n",
        )
        .unwrap();
        assert_eq!(
            s.steps[0],
            Step::Scroll {
                target: Target {
                    id: Some("2".into()),
                    ..Target::default()
                },
                to: ScrollTo::End,
                viewport: None,
            }
        );
        let mut fake = Fake::new();
        let out = dir();
        let report = run(&mut fake, &s, out.path());
        assert!(report.passed(), "{}", report.summary());
        let sent = fake
            .calls
            .iter()
            .find(|(m, _)| m == "scroll")
            .map(|(_, p)| p.clone())
            .expect("a scroll call");
        assert_eq!(sent["to"], "end");
        assert_eq!(sent["target"]["id"], "2");
        assert!(
            Scenario::parse("name: x\nsteps:\n  - scroll: {target: {id: '2'}, to: middle}\n")
                .is_err()
        );
    }

    #[test]
    fn rejects_bad_scenarios_with_a_message() {
        assert!(Scenario::parse("name: x\nsteps: []\n").is_err());
        assert!(Scenario::parse("name: x\nsteps:\n  - explode: {}\n").is_err());
        assert!(Scenario::parse("name: x\nthemes: [blue]\nsteps:\n  - key: A\n").is_err());
        assert!(Scenario::parse("name: x\nextra: 1\nsteps:\n  - key: A\n").is_err());
    }

    #[test]
    fn compare_steps_run_once_in_light() {
        let s = Scenario::parse("name: x\nsteps:\n  - key: A\n").unwrap();
        assert!(s.compare.is_empty() && s.compare_run().is_none());
        let s = Scenario::parse(
            "name: x\nsteps:\n  - key: A\ncompare:\n  - key: Ctrl+G\n  - assert_state: {key: screen.screen, equals: register}\n",
        )
        .unwrap();
        let c = s.compare_run().expect("compare steps");
        assert_eq!(c.themes, vec!["light"]);
        assert_eq!(c.steps.len(), 2);
        assert_eq!(c.compare.len(), 0);
        let mut fake = Fake::new();
        fake.state = json!({"screen": {"screen": "register"}});
        let out = dir();
        let report = run(&mut fake, &c, out.path());
        assert!(report.passed(), "{}", report.summary());
        assert_eq!(report.outcomes.len(), 1);
    }

    #[test]
    fn runs_every_theme_and_names_screenshots_by_theme() {
        let out = dir();
        let s = Scenario::parse(
            "name: x\nsteps:\n  - assert_exists: {role: tab, label: Home, index: 0}\n  - assert_count: {target: {role: button}, count: 2}\n  - assert_text: {target: {id: '2'}, equals: Save}\n  - assert_enabled: {id: '2'}\n  - assert_no_clipping: {}\n  - assert_state: {key: window.width, equals: 800.0}\n  - assert_state: {key: palette_open, equals: false}\n  - click: {id: '2'}\n  - key: Enter\n  - screenshot: home page\n",
        )
        .unwrap();
        let mut fake = Fake::new();
        let report = run(&mut fake, &s, out.path());
        assert!(report.passed(), "{}", report.summary());
        assert_eq!(report.outcomes.len(), 2);
        let names: Vec<_> = report
            .screenshots
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["home_page-light.png", "home_page-dark.png"]);
        let themes: Vec<_> = fake
            .calls
            .iter()
            .filter(|(m, _)| m == "set_theme")
            .map(|(_, p)| p["theme"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(themes, vec!["light", "dark"]);
        // Each theme resets the zoom to 100% right after its theme, before
        // any step (a resize is in points, so it follows the zoom).
        for (i, (m, _)) in fake.calls.iter().enumerate() {
            if m == "set_theme" {
                let next = &fake.calls[i + 1];
                assert_eq!(next.0, "set_zoom");
                assert_eq!(next.1["percent"], 100);
            }
        }
    }

    #[test]
    fn a_failed_assertion_stops_that_theme_and_keeps_a_picture() {
        let out = dir();
        let s = Scenario::parse(
            "name: x\nthemes: [light]\nsteps:\n  - key: A\n  - assert_text: {target: {id: '2'}, equals: Nope}\n  - key: B\n",
        )
        .unwrap();
        let mut fake = Fake::new();
        let start = Instant::now();
        let report = run(&mut fake, &s, out.path());
        assert!(start.elapsed() >= ASSERT_TIMEOUT, "assertions retry first");
        assert!(!report.passed());
        let f = report.outcomes[0].failure.as_ref().unwrap();
        assert_eq!(f.step, 2);
        assert!(f.message.contains("expected \"Nope\""), "{}", f.message);
        assert_eq!(report.outcomes[0].steps_passed, 1);
        // The third step never ran.
        assert_eq!(fake.calls.iter().filter(|(m, _)| m == "key").count(), 1);
        assert!(
            report
                .screenshots
                .iter()
                .any(|p| p.ends_with("failure-light.png"))
        );
    }

    #[test]
    fn clipping_assertion_fails_when_content_is_cut() {
        let out = dir();
        let s = Scenario::parse("name: x\nthemes: [light]\nsteps:\n  - assert_no_clipping: {}\n")
            .unwrap();
        let mut fake = Fake::new();
        fake.snapshot
            .tree
            .children
            .push(crate::inspect::fixture::node(
                "9",
                "button",
                Some("Cut off"),
                Some((780.0, 10.0, 60.0, 24.0)),
            ));
        let report = run(&mut fake, &s, out.path());
        let f = report.outcomes[0].failure.as_ref().unwrap();
        assert!(f.message.contains("Cut off"), "{}", f.message);
    }

    /// A02 review F4: a part the app reports wider than its room fails the
    /// check even though the window edge cuts nothing; an ignored region
    /// lets it pass.
    #[test]
    fn clipping_assertion_fails_on_a_part_wider_than_its_room() {
        let out = dir();
        let s = Scenario::parse("name: x\nthemes: [light]\nsteps:\n  - assert_no_clipping: {}\n")
            .unwrap();
        let mut fake = Fake::new();
        fake.snapshot.overflow.push(crate::inspect::Overflow {
            what: "document viewer".into(),
            bounds: Bounds {
                x: 500.0,
                y: 100.0,
                width: 290.0,
                height: 300.0,
            },
        });
        let report = run(&mut fake, &s, out.path());
        let f = report.outcomes[0].failure.as_ref().unwrap();
        assert!(f.message.contains("document viewer"), "{}", f.message);
        assert!(f.message.contains("wider than the room"), "{}", f.message);
        let s = Scenario::parse(
            "name: x\nthemes: [light]\nsteps:\n  - assert_no_clipping: {ignore: [{x: 400, y: 0, width: 400, height: 600}]}\n",
        )
        .unwrap();
        let report = run(&mut fake, &s, out.path());
        assert!(report.passed(), "{}", report.summary());
    }

    #[test]
    fn paste_sends_the_block_whole_and_never_describes_it() {
        let out = dir();
        let yaml = "name: x\nthemes: [dark]\nsteps:\n  - paste: {text: \"Date\\tMemo\\n2026-11-04\\tLunch {theme}\\n\"}\n  - paste: {text: \"12.50\", target: {id: '2'}}\n";
        let s = Scenario::parse(yaml).unwrap();
        assert_eq!(s.steps[0].describe(), "paste 2 line(s)");
        let mut fake = Fake::new();
        let report = run(&mut fake, &s, out.path());
        assert!(report.passed(), "{}", report.summary());
        let pastes: Vec<_> = fake.calls.iter().filter(|(m, _)| m == "paste").collect();
        assert_eq!(pastes.len(), 2);
        assert_eq!(pastes[0].1["text"], "Date\tMemo\n2026-11-04\tLunch dark\n");
        assert_eq!(pastes[0].1["target"], Value::Null);
        assert_eq!(pastes[1].1["target"]["id"], "2");
        let p: crate::actions::PasteParams = serde_json::from_value(pastes[0].1.clone()).unwrap();
        assert!(!format!("{p:?}").contains("Lunch"));
    }

    #[test]
    fn dotted_paths() {
        let v = json!({"a": {"b": 3}});
        assert_eq!(dotted(&v, "a.b"), Some(&json!(3)));
        assert_eq!(dotted(&v, "a.c"), None);
        let v = json!({"rows": [{"d": 1}, {"d": 2}]});
        assert_eq!(dotted(&v, "rows.1.d"), Some(&json!(2)));
        assert_eq!(dotted(&v, "rows.2.d"), None);
        assert_eq!(dotted(&v, "rows.x"), None);
    }

    /// A02 D5: steps aimed at the pop-out window carry `viewport` on every
    /// call they make (the inspect polls too); steps without one send
    /// exactly what they sent before.
    #[test]
    fn steps_aimed_at_another_window_send_its_viewport() {
        let yaml = "name: x\nthemes: [light]\nsteps:\n  - click: {role: button, label: Save, viewport: doc_viewer}\n  - key: {key: F6, viewport: doc_viewer}\n  - key: F6\n  - assert_exists: {label: Save, viewport: doc_viewer}\n  - assert_text: {target: {id: '2'}, equals: Save, viewport: doc_viewer}\n  - move: {x: 40, y: 50, viewport: doc_viewer}\n  - resize: {width: 500, height: 400, viewport: doc_viewer}\n  - screenshot: {name: viewer, viewport: doc_viewer}\n  - screenshot: viewer\n  - assert_no_clipping: {viewport: doc_viewer}\n  - click: {id: '2'}\n";
        let s = Scenario::parse(yaml).unwrap();
        assert_eq!(
            s.steps[0],
            Step::Click(TargetIn {
                target: Target {
                    role: Some("button".into()),
                    label: Some("Save".into()),
                    ..Target::default()
                },
                viewport: Some("doc_viewer".into()),
            })
        );
        assert_eq!(
            s.steps[2],
            Step::Key(KeyStep {
                key: "F6".into(),
                viewport: None,
            })
        );
        assert_eq!(
            s.steps[0].describe(),
            "click role button, label \"Save\" in the doc_viewer window"
        );
        assert_eq!(s.steps[2].describe(), "key F6");
        let mut fake = Fake::new();
        let out = dir();
        let report = run(&mut fake, &s, out.path());
        assert!(report.passed(), "{}", report.summary());
        let calls = &fake.calls;
        let vp_of = |p: &Value| p.get("viewport").and_then(Value::as_str).map(str::to_owned);
        let sent = |m: &str| -> Vec<Option<String>> {
            calls
                .iter()
                .filter(|(n, _)| n == m)
                .map(|(_, p)| vp_of(p))
                .collect()
        };
        let dv = Some("doc_viewer".to_string());
        assert_eq!(sent("click"), vec![dv.clone(), None]);
        assert_eq!(sent("key"), vec![dv.clone(), None]);
        assert_eq!(sent("move"), vec![dv.clone()]);
        assert_eq!(sent("resize"), vec![dv.clone()]);
        assert_eq!(sent("screenshot"), vec![dv.clone(), None]);
        // Every inspect poll before the last (main) click asked for the
        // pop-out's tree; the last one for the main window's.
        let inspects = sent("inspect");
        assert_eq!(inspects.last(), Some(&None));
        assert!(inspects[..inspects.len() - 1].iter().all(|v| *v == dv));
        // The two screenshots never share a file.
        let names: Vec<_> = report
            .screenshots
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["viewer-doc_viewer-light.png", "viewer-light.png"]
        );
    }

    #[test]
    fn an_unknown_viewport_is_refused_when_the_scenario_is_read() {
        let e = Scenario::parse("name: x\nsteps:\n  - key: {key: A, viewport: popup}\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("popup") && e.contains("doc_viewer"), "{e}");
        assert!(
            Scenario::parse(
                "name: x\nsteps:\n  - key: A\ncompare:\n  - click: {id: '1', viewport: x}\n"
            )
            .is_err()
        );
        // Typos in a step's own fields are still refused.
        assert!(Scenario::parse("name: x\nsteps:\n  - click: {lable: Save}\n").is_err());
        assert!(Scenario::parse("name: x\nsteps:\n  - key: {key: A, window: main}\n").is_err());
        assert!(Scenario::parse("name: x\nsteps:\n  - screenshot: {name: a, extra: 1}\n").is_err());
    }

    /// A failure in a step aimed at the pop-out keeps a picture of both
    /// windows.
    #[test]
    fn a_failure_in_the_other_window_pictures_it_too() {
        let out = dir();
        let s = Scenario::parse(
            "name: x\nthemes: [light]\nsteps:\n  - assert_text: {target: {id: '2'}, equals: Nope, viewport: doc_viewer}\n",
        )
        .unwrap();
        let mut fake = Fake::new();
        let report = run(&mut fake, &s, out.path());
        assert!(!report.passed());
        let names: Vec<_> = report
            .screenshots
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["failure-light.png", "failure-doc_viewer-light.png"]
        );
    }

    #[test]
    fn typed_text_names_the_theme() {
        assert_eq!(themed("Leaf Removal {theme}", "dark"), "Leaf Removal dark");
        assert_eq!(themed("plain", "light"), "plain");
        let p = Path::new("some/where/s.yaml");
        let out = with_scenario_dir("type: {scenario_dir}/x.csv", p);
        assert!(out.ends_with("some/where/x.csv"), "{out}");
        assert!(!out.contains('\\'));
        assert_eq!(with_scenario_dir("plain", p), "plain");
    }
}
