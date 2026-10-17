//! Command-line options for the desktop app. Few and plain: the owner never
//! needs them; they exist for screenshots, tests and support.

use std::path::PathBuf;

use cl_ui::theme::ThemeChoice;

/// Parsed options.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    /// Show the synthetic Ozark Ridge sample and do not start the engine.
    pub demo: bool,
    /// Do not start the engine or take the single-instance lock.
    pub no_engine: bool,
    /// Engine executable to run instead of the usual search.
    pub engine_exe: Option<PathBuf>,
    /// App data folder to use instead of the per-user default.
    pub data_dir: Option<PathBuf>,
    /// Theme override for this run.
    pub theme: Option<ThemeChoice>,
    /// Zoom override for this run, in percent.
    pub zoom: Option<u16>,
    /// Window size override in points.
    pub size: Option<[f32; 2]>,
    /// Start the control channel and write its port and token here (tests and
    /// scripts only; absent means no listener at all).
    pub control: Option<PathBuf>,
    /// Print usage and exit.
    pub help: bool,
}

/// Usage text.
pub const USAGE: &str = "\
usage: clientledger [options]

  --demo               show the sample company from the design mockups (synthetic data, no engine)
  --no-engine          open the window without starting the books engine
  --engine-exe <path>  use this engine program
  --data-dir <path>    keep app data in this folder
  --theme <name>       light, dark, or auto, for this run
  --zoom <percent>     80 to 200, for this run
  --size <WxH>         window size in points, for example 1280x820
  --control <path>     start the local control channel for scripts and tests; writes a
                       private file at <path> (off unless given)
  -h, --help           show this help";

/// Parse the arguments after the program name.
///
/// # Errors
/// A plain message for an unknown option or a bad value.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--demo" => out.demo = true,
            "--no-engine" => out.no_engine = true,
            "-h" | "--help" => out.help = true,
            "--control" => out.control = Some(PathBuf::from(value("--control")?)),
            "--engine-exe" => out.engine_exe = Some(PathBuf::from(value("--engine-exe")?)),
            "--data-dir" => out.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--theme" => {
                out.theme = Some(match value("--theme")?.as_str() {
                    "light" => ThemeChoice::Light,
                    "dark" => ThemeChoice::Dark,
                    "auto" => ThemeChoice::Auto,
                    other => return Err(format!("unknown theme `{other}`")),
                });
            }
            "--zoom" => {
                let v = value("--zoom")?;
                let z: u16 = v
                    .parse()
                    .map_err(|_| format!("`{v}` is not a zoom percentage"))?;
                if !(80..=200).contains(&z) {
                    return Err("zoom must be between 80 and 200".into());
                }
                out.zoom = Some(z);
            }
            "--size" => {
                let v = value("--size")?;
                let bad = || format!("`{v}` is not a size like 1280x820");
                let (w, h) = v.split_once('x').ok_or_else(bad)?;
                let (w, h): (u16, u16) =
                    (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
                out.size = Some([f32::from(w), f32::from(h)]);
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Args, String> {
        parse(a.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn no_arguments_is_the_normal_start() {
        assert_eq!(p(&[]).unwrap(), Args::default());
        assert_eq!(p(&[]).unwrap().control, None);
    }

    #[test]
    fn flags_and_values() {
        let a = p(&[
            "--demo",
            "--theme",
            "dark",
            "--zoom",
            "150",
            "--size",
            "1440x940",
            "--data-dir",
            "x",
            "--control",
            "c.json",
        ])
        .unwrap();
        assert!(a.demo && !a.no_engine);
        assert_eq!(a.theme, Some(ThemeChoice::Dark));
        assert_eq!(a.zoom, Some(150));
        assert_eq!(a.size, Some([1440.0, 940.0]));
        assert_eq!(a.data_dir, Some(PathBuf::from("x")));
        assert_eq!(a.control, Some(PathBuf::from("c.json")));
    }

    #[test]
    fn mistakes_get_plain_messages() {
        assert!(p(&["--nope"]).unwrap_err().contains("unknown option"));
        assert!(p(&["--zoom"]).unwrap_err().contains("needs a value"));
        assert!(
            p(&["--zoom", "500"])
                .unwrap_err()
                .contains("between 80 and 200")
        );
        assert!(p(&["--zoom", "x"]).unwrap_err().contains("not a zoom"));
        assert!(
            p(&["--theme", "blue"])
                .unwrap_err()
                .contains("unknown theme")
        );
        assert!(p(&["--size", "wide"]).unwrap_err().contains("not a size"));
    }

    #[test]
    fn help_is_recognised() {
        assert!(p(&["-h"]).unwrap().help);
        assert!(p(&["--help"]).unwrap().help);
    }
}
