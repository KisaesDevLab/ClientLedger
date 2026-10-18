//! `cargo xtask grid-bench`: the ledger grid scroll benchmark (P10 plan §6,
//! S9). Builds and runs `crates/cl-ui/benches/grid_scroll.rs` optimised
//! (profile `grid-bench`), reads its `grid-bench ...` lines and checks the
//! CPU frame-time budget: p95 at most 8 ms with AccessKit off and 12 ms with
//! it on (60 fps leaves 16.6 ms per frame for everything else too). A run
//! over budget is retried once, because shared CI runners have noisy
//! neighbours; the deterministic work-count tests in
//! `cl-grid/tests/virtualization.rs` are the primary gate.

use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// p95 budget in microseconds, AccessKit off.
const BUDGET_OFF_US: u64 = 8_000;
/// p95 budget in microseconds, AccessKit on.
const BUDGET_ON_US: u64 = 12_000;
/// Runs allowed before the budget fails.
const ATTEMPTS: u32 = 2;

/// One measured run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// AccessKit was on.
    pub accesskit: bool,
    /// Frames measured.
    pub frames: u64,
    /// Median frame time.
    pub p50_us: u64,
    /// 95th percentile frame time.
    pub p95_us: u64,
    /// Slowest frame.
    pub max_us: u64,
}

impl Run {
    fn budget_us(&self) -> u64 {
        if self.accesskit {
            BUDGET_ON_US
        } else {
            BUDGET_OFF_US
        }
    }

    fn within_budget(&self) -> bool {
        self.p95_us <= self.budget_us()
    }
}

/// The `grid-bench ...` lines in the benchmark's output.
pub fn parse(output: &str) -> Vec<Run> {
    output
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("grid-bench ")?;
            let mut accesskit = None;
            let (mut frames, mut p50, mut p95, mut max) = (None, None, None, None);
            for field in rest.split_whitespace() {
                let (k, v) = field.split_once('=')?;
                match k {
                    "accesskit" => accesskit = Some(v == "on"),
                    "frames" => frames = v.parse().ok(),
                    "p50_us" => p50 = v.parse().ok(),
                    "p95_us" => p95 = v.parse().ok(),
                    "max_us" => max = v.parse().ok(),
                    _ => {}
                }
            }
            Some(Run {
                accesskit: accesskit?,
                frames: frames?,
                p50_us: p50?,
                p95_us: p95?,
                max_us: max?,
            })
        })
        .collect()
}

/// Problems with one attempt's runs; empty when both are within budget.
pub fn verdict(runs: &[Run]) -> Vec<String> {
    let mut out = Vec::new();
    for accesskit in [false, true] {
        match runs.iter().find(|r| r.accesskit == accesskit) {
            None => out.push(format!(
                "no result with AccessKit {}",
                if accesskit { "on" } else { "off" }
            )),
            Some(r) if !r.within_budget() => out.push(format!(
                "p95 {} us is over the {} us budget with AccessKit {}",
                r.p95_us,
                r.budget_us(),
                if accesskit { "on" } else { "off" }
            )),
            Some(_) => {}
        }
    }
    out
}

fn ms(us: u64) -> String {
    format!("{}.{:02}", us / 1000, (us % 1000) / 10)
}

pub fn grid_bench(args: &[String]) -> Result<()> {
    if let Some(a) = args.first() {
        bail!("grid-bench takes no arguments (got `{a}`)");
    }
    let mut last = Vec::new();
    for attempt in 1..=ATTEMPTS {
        let out = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "bench",
                "-p",
                "cl-ui",
                "--bench",
                "grid_scroll",
                "--profile",
                "grid-bench",
            ])
            .output()
            .context("running cargo bench")?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
            bail!("the grid benchmark failed ({})", out.status);
        }
        let runs = parse(&stdout);
        for r in &runs {
            println!(
                "grid-bench: AccessKit {:3}  {} frames  p50 {} ms  p95 {} ms  max {} ms  (budget p95 {} ms)",
                if r.accesskit { "on" } else { "off" },
                r.frames,
                ms(r.p50_us),
                ms(r.p95_us),
                ms(r.max_us),
                ms(r.budget_us()),
            );
        }
        last = verdict(&runs);
        if last.is_empty() {
            println!("grid-bench: within budget");
            return Ok(());
        }
        if attempt < ATTEMPTS {
            eprintln!("grid-bench: over budget on attempt {attempt}, retrying once");
        }
    }
    for v in &last {
        eprintln!("grid-bench: {v}");
    }
    bail!("the grid frame time is over budget")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "\
     Running benches/grid_scroll.rs
grid-bench accesskit=off frames=600 top_rows=229 p50_us=423 p95_us=670 max_us=1245
grid-bench accesskit=on frames=600 top_rows=229 p50_us=441 p95_us=12001 max_us=13843
";

    #[test]
    fn parses_both_runs() {
        let runs = parse(OUT);
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0],
            Run {
                accesskit: false,
                frames: 600,
                p50_us: 423,
                p95_us: 670,
                max_us: 1245
            }
        );
        assert!(runs[1].accesskit);
    }

    #[test]
    fn the_budget_is_checked_per_mode() {
        let runs = parse(OUT);
        let v = verdict(&runs);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("AccessKit on"));
        assert!(verdict(&runs[..1]).iter().any(|m| m.contains("no result")));
        let mut fine = runs;
        fine[1].p95_us = BUDGET_ON_US;
        assert_eq!(verdict(&fine), Vec::<String>::new());
    }

    #[test]
    fn malformed_lines_are_ignored() {
        assert_eq!(parse("grid-bench accesskit=on frames=x\nother"), Vec::new());
    }

    #[test]
    fn milliseconds_are_printed_without_floats() {
        assert_eq!(ms(670), "0.67");
        assert_eq!(ms(12_001), "12.00");
    }
}
