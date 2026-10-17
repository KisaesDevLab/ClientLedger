//! `clientledger-cli ui` and `clientledger-cli tools`: drive a running app
//! through its control channel (P07).
//!
//! The app must have been started with `--control <path>`. Output is JSON for
//! scripts; scenario runs print a plain summary. Nothing here prints the
//! control token.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use cl_automation::actions::Target;
use cl_automation::client::{Client, ClientError, Transport};
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
  type <text> [target]           type text (focus the target first, if given)
  key <chord>                    press a key, for example Ctrl+K or Escape
  command <id>                   run a command by id, for example help.shortcuts
  screenshot <file.png>          save the window as a PNG
  resize <width> <height>        set the window size in points
  theme <light|dark|auto>        switch the colour theme
  zoom <80..200>                 set the app zoom in percent
  quit                           ask the app to quit (unsaved work prompts)
  call <method> [json]           call any method from `clientledger-cli tools`
  scenario <file.yaml> [--out <folder>]   run a scenario in light and dark
  wait [--timeout <seconds>]     wait until the app answers

target flags: --id <id> --role <role> --label <exact text>
              --contains <text> --index <n>

exit status: 0 done, 1 failed or a scenario step failed, 2 usage error";

/// Exit status when the call or scenario failed.
const EXIT_FAILED: u8 = 1;

fn usage_error(message: &str) -> ExitCode {
    eprintln!("error: {message}\n\n{USAGE}");
    ExitCode::from(EXIT_USAGE)
}

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

/// Build the method and parameters for a subcommand.
fn request(sub: &str, args: &[String]) -> Result<(String, Value), String> {
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
        "click" | "focus" => {
            need(0, "target flags (--label, --role, --id)")?;
            if target.is_empty() {
                return Err(format!("`{sub}` needs a target"));
            }
            Ok((sub.to_string(), json!({"target": target})))
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
        "resize" => {
            need(2, "width and height")?;
            let n = |s: &String| {
                s.parse::<f32>()
                    .map_err(|_| format!("`{s}` is not a number"))
            };
            Ok((
                "resize".into(),
                json!({"width": n(&rest[0])?, "height": n(&rest[1])?}),
            ))
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
            match Client::connect_wait(&control, Duration::from_secs(secs)) {
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
            match Client::connect(&control).and_then(|mut c| c.call(&method, params)) {
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            match it.next() {
                Some(p) => out = Some(PathBuf::from(p)),
                None => return usage_error("--out needs a folder"),
            }
        } else if file.is_none() {
            file = Some(a);
        } else {
            return usage_error("usage: scenario <file.yaml> [--out <folder>]");
        }
    }
    let Some(file) = file else {
        return usage_error("usage: scenario <file.yaml> [--out <folder>]");
    };
    let scenario = match Scenario::load(std::path::Path::new(file)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(EXIT_FAILED);
        }
    };
    let out = out.unwrap_or_else(|| PathBuf::from("target/ui-check").join(&scenario.name));
    let mut client = match Client::connect_wait(control, Duration::from_secs(60)) {
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
    fn simple_methods() {
        assert_eq!(request("theme", &s(&["dark"])).unwrap().0, "set_theme");
        assert_eq!(request("zoom", &s(&["200"])).unwrap().1["percent"], 200);
        let (_, p) = request("type", &s(&["hello", "--role", "text_input"])).unwrap();
        assert_eq!(p["text"], "hello");
        assert_eq!(p["target"]["role"], "text_input");
        assert_eq!(request("inspect", &s(&[])).unwrap().1, json!({}));
    }
}
