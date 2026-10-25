//! `cargo xtask ui-check <screen> [--compare] [--no-build]`.
//!
//! Builds the app and the CLI, launches the app with a temporary control
//! file, runs `crates/cl-automation/scenarios/<screen>.yaml` through
//! `clientledger-cli ui scenario` (light and dark; PNGs land in
//! `target/ui-check/<screen>/`), and stops only the app it started.
//!
//! With `--compare` it also sizes the window to the mockup (1440 x 940),
//! runs the scenario's optional `compare:` steps (which bring the screen's
//! main view back when the scenario closed it), saves the app's light
//! screenshot, and writes `compare.html` next to the
//! reference PNG from `docs/design/screenshots/` for the ui-verifier review
//! (Q-226). The app runs `--demo`, which shows the synthetic Ozark Ridge
//! sample at the mockups' story date (11/04/2026) until the real sample
//! company arrives in P08.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

/// Mockup size in points (docs/design/README.md).
const MOCKUP_SIZE: (u32, u32) = (1440, 940);

/// Parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// Screen name, for example `shell` or `home`.
    pub screen: String,
    /// Also produce the side-by-side page.
    pub compare: bool,
    /// Skip `cargo build`.
    pub build: bool,
}

/// Parse the arguments after `ui-check`.
///
/// # Errors
/// A plain message for a missing screen or an unknown option.
pub fn plan(args: &[String]) -> Result<Plan> {
    let mut screen = None;
    let mut compare = false;
    let mut build = true;
    for a in args {
        match a.as_str() {
            "--compare" => compare = true,
            "--no-build" => build = false,
            s if s.starts_with("--") => bail!("unknown ui-check option `{s}`"),
            s => {
                if screen.replace(s.to_string()).is_some() {
                    bail!("ui-check takes one screen name");
                }
            }
        }
    }
    let screen = screen.context("usage: cargo xtask ui-check <screen> [--compare] [--no-build]")?;
    if !screen
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("the screen name may only use letters, digits, - and _");
    }
    Ok(Plan {
        screen,
        compare,
        build,
    })
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

fn exe(name: &str) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    target_dir().join("debug").join(file)
}

/// Find the reference PNG for a screen: `Home.png` for `home`,
/// `WriteCheck.png` for `write-check`. A scenario that covers one panel of
/// a screen (`preferences-ai`) falls back to the screen's mockup
/// (`Preferences.png`) by dropping its last `-part`.
#[must_use]
pub fn find_mockup(dir: &Path, screen: &str) -> Option<PathBuf> {
    /// Scenarios named apart from their mockup (P17: `bill` is Enter Bill).
    const ALIASES: &[(&str, &str)] = &[("bill", "EnterBill")];
    find_mockup_exact(dir, screen)
        .or_else(|| {
            let (head, _) = screen.rsplit_once('-')?;
            find_mockup_exact(dir, head)
        })
        .or_else(|| {
            let (_, mockup) = ALIASES.iter().find(|(s, _)| *s == screen)?;
            find_mockup_exact(dir, mockup)
        })
}

fn find_mockup_exact(dir: &Path, screen: &str) -> Option<PathBuf> {
    let want: String = screen
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|e| e == "png")
                && p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.to_ascii_lowercase() == want)
        })
}

/// The side-by-side page. Pure, so it is unit tested.
#[must_use]
pub fn compare_html(screen: &str, app_png: &str, mockup_png: &str) -> String {
    let screen = html_escape(screen);
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
<title>ui-check compare: {screen}</title>\n\
<style>\n\
body{{font:14px system-ui,sans-serif;margin:16px;background:#f5f4f0;color:#1d2321}}\n\
.row{{display:flex;gap:16px;align-items:flex-start}}\n\
figure{{margin:0;flex:1 1 0;min-width:0}}\n\
img{{width:100%;border:1px solid #c9c6bd;background:#fff}}\n\
figcaption{{font-weight:600;margin-bottom:6px}}\n\
</style></head><body>\n\
<h1>{screen}: app (left) and mockup (right)</h1>\n\
<p>Review by structure, not pixels: missing or extra elements, order, labels, \
token colors, and states (docs/design/README.md). List every deviation in the PR \
and QUESTIONS.md.</p>\n\
<div class=\"row\">\n\
<figure><figcaption>App, light theme</figcaption><img src=\"{app}\" alt=\"App screenshot\"></figure>\n\
<figure><figcaption>Mockup</figcaption><img src=\"{mock}\" alt=\"Mockup\"></figure>\n\
</div></body></html>\n",
        app = html_escape(app_png),
        mock = html_escape(mockup_png),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Stops the app this run started, and only that process.
struct AppGuard {
    child: Child,
    /// Removed after the app is stopped (field drops follow `Drop::drop`).
    _tmp: tempfile::TempDir,
}

impl Drop for AppGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn make_private_dir() -> Result<tempfile::TempDir> {
    // Created atomically with a random name and owner-only permissions (0700
    // on Unix). Under the user's temp folder, which on Windows is inside
    // %LOCALAPPDATA%, as the control file rules require.
    tempfile::Builder::new()
        .prefix("clientledger-ui-check-")
        .tempdir()
        .context("could not create the temporary folder")
}

fn cli(control: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(exe("clientledger-cli"))
        .args(["ui", "--control"])
        .arg(control)
        .args(args)
        .status()
        .context("could not start clientledger-cli")?;
    if !status.success() {
        bail!("`clientledger-cli ui {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Entry point.
///
/// # Errors
/// Build failures, a scenario that fails, or a missing scenario.
pub fn ui_check(args: &[String]) -> Result<()> {
    let plan = plan(args)?;
    let scenario = root()
        .join("crates/cl-automation/scenarios")
        .join(format!("{}.yaml", plan.screen));
    let has_scenario = scenario.is_file();
    if !has_scenario && !plan.compare {
        bail!(
            "no scenario at {} (use --compare to only compare against the mockup)",
            scenario.display()
        );
    }
    let out = target_dir().join("ui-check").join(&plan.screen);
    std::fs::create_dir_all(&out)?;

    if plan.build {
        let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "-p", "clientledger", "-p", "clientledger-cli"])
            .status()
            .context("could not start cargo")?;
        if !status.success() {
            bail!("the build failed");
        }
    }

    let tmp = make_private_dir()?;
    let tmp_path = tmp.path().to_path_buf();
    let control = tmp_path.join("control.json");
    let (w, h) = if plan.compare {
        MOCKUP_SIZE
    } else {
        (1280, 820)
    };
    let child = Command::new(exe("clientledger"))
        .arg("--demo")
        .arg("--data-dir")
        .arg(tmp_path.join("data"))
        .arg("--control")
        .arg(&control)
        .args(["--theme", "light", "--size", &format!("{w}x{h}")])
        // Scenarios name synthetic fixture files relative to the repository
        // root (`document-view` attaches `fixtures/docs/two_pages.pdf`).
        .current_dir(root())
        .stdin(Stdio::null())
        .spawn()
        .context("could not start the app (is a display available?)")?;
    // From here on the guard stops exactly this process.
    let _guard = AppGuard { child, _tmp: tmp };

    cli(&control, &["wait", "--timeout", "90"])?;

    let mut failure = None;
    if has_scenario {
        let out_s = out.to_string_lossy().to_string();
        let sc = scenario.to_string_lossy().to_string();
        if let Err(e) = cli(&control, &["scenario", &sc, "--out", &out_s]) {
            failure = Some(e);
        }
    }

    if plan.compare {
        cli(&control, &["theme", "light"])?;
        cli(&control, &["zoom", "100"])?;
        cli(
            &control,
            &[
                "resize",
                &MOCKUP_SIZE.0.to_string(),
                &MOCKUP_SIZE.1.to_string(),
            ],
        )?;
        // The scenario may have closed its screen: its `compare` steps
        // bring the main view back before the screenshot.
        if has_scenario {
            let out_s = out.to_string_lossy().to_string();
            let sc = scenario.to_string_lossy().to_string();
            cli(&control, &["scenario", &sc, "--compare", "--out", &out_s])?;
        }
        let app_png = out.join("app-light.png");
        cli(&control, &["screenshot", &app_png.to_string_lossy()])?;
        let mockups = root().join("docs/design/screenshots");
        let mockup = find_mockup(&mockups, &plan.screen).with_context(|| {
            format!(
                "no mockup PNG for `{}` in docs/design/screenshots",
                plan.screen
            )
        })?;
        std::fs::copy(&mockup, out.join("mockup.png"))?;
        std::fs::write(
            out.join("compare.html"),
            compare_html(&plan.screen, "app-light.png", "mockup.png"),
        )?;
        println!("compare page: {}", out.join("compare.html").display());
    }
    println!("screenshots: {}", out.display());
    match failure {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn plan_parses() {
        let p = plan(&s(&["shell"])).unwrap();
        assert_eq!(
            p,
            Plan {
                screen: "shell".into(),
                compare: false,
                build: true
            }
        );
        let p = plan(&s(&["home", "--compare", "--no-build"])).unwrap();
        assert!(p.compare && !p.build);
        assert!(plan(&s(&[])).is_err());
        assert!(plan(&s(&["a", "b"])).is_err());
        assert!(plan(&s(&["--nope", "a"])).is_err());
        assert!(plan(&s(&["../x"])).is_err());
    }

    #[test]
    fn mockup_lookup_ignores_case_and_separators() {
        let dir = std::env::temp_dir().join(format!("xt-mock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("WriteCheck.png"), b"x").unwrap();
        std::fs::write(dir.join("Home.png"), b"x").unwrap();
        assert!(find_mockup(&dir, "home").unwrap().ends_with("Home.png"));
        assert!(
            find_mockup(&dir, "write-check")
                .unwrap()
                .ends_with("WriteCheck.png")
        );
        assert!(find_mockup(&dir, "nope").is_none());
        std::fs::write(dir.join("Preferences.png"), b"x").unwrap();
        assert!(
            find_mockup(&dir, "preferences-ai")
                .unwrap()
                .ends_with("Preferences.png"),
            "a panel scenario falls back to its screen's mockup"
        );
        assert!(find_mockup(&dir, "nope-ai").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compare_page_shows_both_images_and_escapes() {
        let h = compare_html("home", "app-light.png", "mockup.png");
        assert!(h.contains("src=\"app-light.png\"") && h.contains("src=\"mockup.png\""));
        assert!(compare_html("a<b", "x\"y.png", "m.png").contains("a&lt;b"));
    }
}
