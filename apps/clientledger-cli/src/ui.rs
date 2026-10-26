//! `clientledger-cli ui` and `clientledger-cli tools`: drive a running app
//! through its control channel (P07).
//!
//! The app must have been started with `--control <path>`. Output is JSON for
//! scripts; scenario runs print a plain summary. Nothing here prints the
//! control token.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use cl_automation::actions::Target;
use cl_automation::client::{Client, ClientError, OwnerCheck, Transport};
use cl_automation::scenario::{Scenario, run};
use serde_json::{Value, json};

use crate::host::EXIT_USAGE;

/// Usage text for `ui`.
pub const USAGE: &str = "\
usage: clientledger-cli ui --control <path> <method> [arguments]

Start the app with `clientledger --control <path>` first.

methods:
  state                          window size, theme, zoom, tabs, dialogs
  inspect [target]               the element tree, or elements matching a target
  click <target>                 click an element
  focus <target>                 move keyboard focus to an element
  hover <target>                 move the pointer over an element (no click)
  type <text> [target]           type text (focus the target first, if given)
  type --stdin [target]          read the text from standard input instead
  key <chord>                    press a key, for example Ctrl+K or Escape
  command <id>                   run a command by id, for example help.shortcuts
  screenshot <file.png>          save the window as a PNG
  resize <width> <height>        set the window size in points
  move <x> <y>                   move the window to a desktop position (points)
  theme <light|dark|auto>        switch the colour theme
  zoom <80..200>                 set the app zoom in percent
  quit                           ask the app to quit (unsaved work prompts)
  call <method> [json]           call any method from `clientledger-cli tools`
  scenario <file.yaml> [--out <folder>] [--compare]
                                 run a scenario in light and dark (--compare:
                                 only its compare steps, light)
  wait [--timeout <seconds>]     wait until the app answers

Do not put secrets (passwords, keys) in command-line arguments: other programs
can read them. Scenarios and scripts must not type real secrets; use --stdin
if a test must type something private.

target flags: --id <id> --role <role> --label <exact text>
              --contains <text> --index <n>

--viewport <main|doc_viewer>   which window inspect, click, focus, hover,
                               type, key, screenshot, resize and move act on
                               (default main; doc_viewer is the pop-out
                               document viewer)

exit status: 0 done, 1 failed or a scenario step failed, 2 usage error";

/// Exit status when the call or scenario failed.
const EXIT_FAILED: u8 = 1;

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n\n{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

/// Subcommands that act on one window and take `--viewport`.
const WINDOW_METHODS: &[&str] = &[
    "inspect",
    "click",
    "focus",
    "hover",
    "type",
    "key",
    "screenshot",
    "resize",
    "move",
];

/// Split the target flags out of `args`; returns the rest.
fn take_target(args: &[String]) -> Result<(Target, Vec<String>), String> {
    let mut target = Target::default();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--id" => target.id = Some(value("--id")?),
            "--role" => target.role = Some(value("--role")?),
            "--label" => target.label = Some(value("--label")?),
            "--contains" => target.label_contains = Some(value("--contains")?),
            "--index" => {
                let v = value("--index")?;
                target.index = Some(v.parse().map_err(|_| format!("`{v}` is not a number"))?);
            }
            _ => rest.push(a.clone()),
        }
    }
    Ok((target, rest))
}

/// Text for `type --stdin`: at most 64 KiB, one trailing newline removed.
fn read_text(reader: &mut dyn std::io::Read) -> Result<String, String> {
    let mut text = String::new();
    std::io::Read::take(reader, 64 * 1024)
        .read_to_string(&mut text)
        .map_err(|_| "standard input could not be read as text".to_string())?;
    if let Some(t) = text.strip_suffix('\n') {
        text = t.strip_suffix('\r').unwrap_or(t).to_string();
    }
    Ok(text)
}

/// Split `--viewport <name>` out of `args`.
fn take_viewport(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut viewport = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--viewport" {
            let v = it.next().ok_or("--viewport needs main or doc_viewer")?;
            if !cl_automation::actions::is_viewport(v) {
                return Err(format!("unknown viewport `{v}`; use main or doc_viewer"));
            }
            viewport = Some(v.clone());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((viewport, rest))
}

/// Build the method and parameters for a subcommand. `--viewport` is added
/// to the parameters of the methods that act on one window.
fn request(sub: &str, args: &[String]) -> Result<(String, Value), String> {
    let (viewport, args) = take_viewport(args)?;
    let (method, mut params) = request_in(sub, &args)?;
    if let Some(v) = viewport {
        if !WINDOW_METHODS.contains(&sub) {
            return Err(format!("`{sub}` does not take --viewport"));
        }
        if let Some(map) = params.as_object_mut() {
            map.insert("viewport".into(), Value::String(v));
        }
    }
    Ok((method, params))
}

fn request_in(sub: &str, args: &[String]) -> Result<(String, Value), String> {
    let (target, rest) = take_target(args)?;
    let need = |n: usize, what: &str| {
        if rest.len() == n {
            Ok(())
        } else {
            Err(format!("`{sub}` needs {what}"))
        }
    };
    match sub {
        "state" | "quit" => {
            need(0, "no arguments")?;
            Ok((sub.to_string(), json!({})))
        }
        "inspect" => {
            need(0, "only target flags")?;
            let p = if target.is_empty() {
                json!({})
            } else {
                json!({"target": target})
            };
            Ok(("inspect".into(), p))
        }
        "click" | "focus" | "hover" => {
            need(0, "target flags (--label, --role, --id)")?;
            if target.is_empty() {
                return Err(format!("`{sub}` needs a target"));
            }
            Ok((sub.to_string(), json!({"target": target})))
        }
        "type" if rest == ["--stdin"] => {
            let text = read_text(&mut std::io::stdin().lock())?;
            let p = if target.is_empty() {
                json!({"text": text})
            } else {
                json!({"text": text, "target": target})
            };
            Ok(("type".into(), p))
        }
        "type" => {
            need(1, "the text to type")?;
            let p = if target.is_empty() {
                json!({"text": rest[0]})
            } else {
                json!({"text": rest[0], "target": target})
            };
            Ok(("type".into(), p))
        }
        "key" => {
            need(1, "a key such as Ctrl+K")?;
            Ok(("key".into(), json!({"key": rest[0]})))
        }
        "command" => {
            need(1, "a command id")?;
            Ok(("command".into(), json!({"id": rest[0]})))
        }
        "screenshot" => {
            need(1, "a .png file name")?;
            Ok(("screenshot".into(), json!({"path": rest[0]})))
        }
        "resize" | "move" => {
            let (a, b) = if sub == "resize" {
                ("width", "height")
            } else {
                ("x", "y")
            };
            need(2, &format!("{a} and {b}"))?;
            let n = |s: &String| {
                s.parse::<f32>()
                    .map_err(|_| format!("`{s}` is not a number"))
            };
            Ok((sub.to_string(), json!({a: n(&rest[0])?, b: n(&rest[1])?})))
        }
        "theme" => {
            need(1, "light, dark, or auto")?;
            Ok(("set_theme".into(), json!({"theme": rest[0]})))
        }
        "zoom" => {
            need(1, "a percentage")?;
            let p: u16 = rest[0]
                .parse()
                .map_err(|_| format!("`{}` is not a percentage", rest[0]))?;
            Ok(("set_zoom".into(), json!({"percent": p})))
        }
        "call" => {
            if rest.is_empty() || rest.len() > 2 {
                return Err("`call` needs a method and optional JSON".into());
            }
            let params = match rest.get(1) {
                Some(j) => {
                    serde_json::from_str(j).map_err(|_| "the JSON is not valid".to_string())?
                }
                None => json!({}),
            };
            Ok((rest[0].clone(), params))
        }
        other => Err(format!("unknown ui method `{other}`")),
    }
}

/// The port must belong to the process named in the control file before the
/// token is sent (P07 review L3). Fails closed.
fn owner_check() -> &'static OwnerCheck {
    &|port, pid| cl_host::verify_listener(port, pid).map_err(|e| e.to_string())
}

fn report(e: &ClientError) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::from(EXIT_FAILED)
}

/// `clientledger-cli ui ...`
#[must_use]
pub fn run_ui(args: &[String]) -> ExitCode {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    // --control <path> may appear anywhere.
    let mut control: Option<PathBuf> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--control" {
            match it.next() {
                Some(p) => control = Some(PathBuf::from(p)),
                None => return usage_error("--control needs a path"),
            }
        } else {
            rest.push(a.clone());
        }
    }
    let Some(control) = control else {
        return usage_error("--control <path> is required");
    };
    let Some((sub, sub_args)) = rest.split_first() else {
        return usage_error("a method is required");
    };

    match sub.as_str() {
        "wait" => {
            let secs = match sub_args {
                [] => 30,
                [flag, v] if flag == "--timeout" => match v.parse() {
                    Ok(n) => n,
                    Err(_) => return usage_error("--timeout needs a number of seconds"),
                },
                _ => return usage_error("usage: wait [--timeout <seconds>]"),
            };
            match Client::connect_wait_with(
                &control,
                Duration::from_secs(secs),
                Some(owner_check()),
            ) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => report(&e),
            }
        }
        "scenario" => scenario(&control, sub_args),
        _ => {
            let (method, params) = match request(sub, sub_args) {
                Ok(r) => r,
                Err(m) => return usage_error(&m),
            };
            match Client::connect_with(&control, Some(owner_check()))
                .and_then(|mut c| c.call(&method, params))
            {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    ExitCode::SUCCESS
                }
                Err(e) => report(&e),
            }
        }
    }
}

fn scenario(control: &std::path::Path, args: &[String]) -> ExitCode {
    let mut file: Option<&String> = None;
    let mut out: Option<PathBuf> = None;
    let mut compare = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--compare" {
            compare = true;
        } else if a == "--out" {
            match it.next() {
                Some(p) => out = Some(PathBuf::from(p)),
                None => return usage_error("--out needs a folder"),
            }
        } else if file.is_none() {
            file = Some(a);
        } else {
            return usage_error("usage: scenario <file.yaml> [--out <folder>] [--compare]");
        }
    }
    let Some(file) = file else {
        return usage_error("usage: scenario <file.yaml> [--out <folder>] [--compare]");
    };
    let scenario = match Scenario::load(std::path::Path::new(file)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_FAILED);
        }
    };
    // `--compare`: only the scenario's `compare` steps (the screen's main
    // view for `ui-check --compare`); nothing to do when it has none.
    let scenario = if compare {
        match scenario.compare_run() {
            Some(s) => s,
            None => return ExitCode::SUCCESS,
        }
    } else {
        scenario
    };
    let out = out.unwrap_or_else(|| PathBuf::from("target/ui-check").join(&scenario.name));
    let mut client =
        match Client::connect_wait_with(control, Duration::from_secs(60), Some(owner_check())) {
            Ok(c) => c,
            Err(e) => return report(&e),
        };
    let result = run(&mut client, &scenario, &out);
    print!("{}", result.summary());
    if result.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FAILED)
    }
}

/// `clientledger-cli tools`: the method table as JSON, for a future MCP
/// server (Q-16).
#[must_use]
pub fn run_tools() -> ExitCode {
    let tools = cl_automation::tools::tools();
    println!(
        "{}",
        serde_json::to_string_pretty(&tools).unwrap_or_default()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn click_builds_a_target() {
        let (m, p) = request(
            "click",
            &s(&["--role", "tab", "--label", "Home", "--index", "1"]),
        )
        .unwrap();
        assert_eq!(m, "click");
        assert_eq!(p["target"]["role"], "tab");
        assert_eq!(p["target"]["label"], "Home");
        assert_eq!(p["target"]["index"], 1);
    }

    #[test]
    fn mistakes_are_usage_errors() {
        assert!(request("click", &s(&[])).is_err());
        assert!(request("key", &s(&[])).is_err());
        assert!(request("zoom", &s(&["big"])).is_err());
        assert!(request("resize", &s(&["1"])).is_err());
        assert!(request("explode", &s(&[])).is_err());
        assert!(request("click", &s(&["--index", "x", "--label", "a"])).is_err());
        assert!(request("call", &s(&["inspect", "{bad"])).is_err());
    }

    #[test]
    fn stdin_text_loses_one_trailing_newline() {
        let crlf = "abc".to_string()
            + "
";
        let two = "a".to_string()
            + "

";
        assert_eq!(read_text(&mut crlf.as_bytes()).unwrap(), "abc");
        assert_eq!(
            read_text(&mut two.as_bytes()).unwrap(),
            "a
"
        );
        assert_eq!(read_text(&mut "".as_bytes()).unwrap(), "");
    }

    #[test]
    fn simple_methods() {
        assert_eq!(request("theme", &s(&["dark"])).unwrap().0, "set_theme");
        assert_eq!(request("zoom", &s(&["200"])).unwrap().1["percent"], 200);
        let (_, p) = request("type", &s(&["hello", "--role", "text_input"])).unwrap();
        assert_eq!(p["text"], "hello");
        assert_eq!(p["target"]["role"], "text_input");
        assert_eq!(request("inspect", &s(&[])).unwrap().1, json!({}));
    }

    /// A02 D5: `--viewport` aims a window method at the pop-out viewer.
    #[test]
    fn viewport_flag() {
        let (m, p) = request(
            "click",
            &s(&["--label", "Dock", "--viewport", "doc_viewer"]),
        )
        .unwrap();
        assert_eq!(m, "click");
        assert_eq!(p["viewport"], "doc_viewer");
        assert_eq!(p["target"]["label"], "Dock");
        let (m, p) = request("move", &s(&["--viewport", "doc_viewer", "40", "-50"])).unwrap();
        assert_eq!(m, "move");
        assert_eq!(p, json!({"x": 40.0, "y": -50.0, "viewport": "doc_viewer"}));
        let (_, p) = request("key", &s(&["F6"])).unwrap();
        assert!(p.get("viewport").is_none());
        assert!(request("key", &s(&["F6", "--viewport", "popup"])).is_err());
        assert!(request("key", &s(&["F6", "--viewport"])).is_err());
        assert!(request("theme", &s(&["dark", "--viewport", "doc_viewer"])).is_err());
        assert!(request("move", &s(&["1"])).is_err());
    }
}
