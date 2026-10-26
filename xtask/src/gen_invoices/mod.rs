//! `cargo xtask gen-invoices` (A03 step 12, plan D6): the synthetic bills
//! and receipts the reading accuracy harness scores against.
//!
//! For each layout (`layouts.rs`) it writes ten seeded documents: a PDF with
//! embedded subset fonts (most with `ToUnicode` maps, a few without), its
//! ground truth (`NN.json`), a flatbed "scan" (`NN.scan.jpg`, JPEG q75-90) and a phone
//! "photo" (`NN.photo.jpg`), into `target/docread-synth/{seen,holdout}/`.
//! All values are invented. Held-out layouts and the held-out font are only
//! written with `--holdout` (review S15).
//!
//! A full run also writes `stress/`: the largest documents an owner can
//! legitimately send (ten text pages, ten scanned pages, a 12-megapixel
//! photo, a 600 dpi scan) for the measured limits (step 12a).
//!
//! `--samples` instead refreshes the dozen committed examples in
//! `fixtures/docs/read/` (IBM Plex only, so no fetched font is committed).
//!
//! Picture work is slow in a debug build, so a debug `cargo xtask` runs
//! itself again optimised (profile `docread-bench`).

// Page geometry and pixel code: short coordinate names (x, y, w, h)
// read best here.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

mod doc;
mod fonts;
mod layouts;
mod pdf;
mod render;
mod rng;
mod scan;
mod values;

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The default seed: the set is the same on every machine.
pub const SEED: u64 = 0x00A0_3C0D_E5EE_D001;
/// Documents per layout (plan D6).
pub const DOCS_PER_LAYOUT: u32 = 10;
/// The environment variable that marks the optimised re-run.
const OPTIMISED: &str = "CL_XTASK_OPTIMISED";

// Command-line switches, one bool each.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    out: Option<PathBuf>,
    holdout: bool,
    scans: bool,
    samples: bool,
    stress_only: bool,
    only: Option<Vec<String>>,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        out: None,
        holdout: false,
        scans: true,
        samples: false,
        stress_only: false,
        only: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--holdout" => o.holdout = true,
            "--no-scans" => o.scans = false,
            "--samples" => o.samples = true,
            "--stress-only" => o.stress_only = true,
            "--out" => o.out = Some(PathBuf::from(it.next().context("--out needs a folder")?)),
            "--layouts" => {
                o.only = Some(
                    it.next()
                        .context("--layouts needs ids")?
                        .split(',')
                        .map(str::to_string)
                        .collect(),
                );
            }
            other => bail!(
                "unknown option `{other}`\nusage: cargo xtask gen-invoices [--holdout] [--no-scans] [--samples] [--stress-only] [--out DIR] [--layouts id,id]"
            ),
        }
    }
    Ok(o)
}

/// Entry point.
#[allow(clippy::too_many_lines)]
pub fn gen_invoices(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    if cfg!(debug_assertions)
        && std::env::var(OPTIMISED).as_deref() != Ok("1")
        && (o.scans || o.samples)
    {
        return rerun_optimised(args);
    }
    let root = crate::engine::repo_root()?;
    if o.samples {
        return samples(&root);
    }
    let out = o
        .out
        .clone()
        .unwrap_or_else(|| root.join("target").join("docread-synth"));
    let fonts = fonts::Fonts::load(&root, false)?;
    let all = layouts::all();
    let chosen: Vec<layouts::Layout> = all
        .iter()
        .filter(|l| o.holdout || !l.holdout)
        .filter(|l| {
            o.only
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|i| i == l.id))
        })
        .copied()
        .collect();
    if chosen.is_empty() {
        bail!("no layout matches");
    }
    let renderer = if o.scans {
        Some(scan::Renderer::new(&root)?)
    } else {
        None
    };
    if o.stress_only {
        let r = renderer
            .as_ref()
            .context("pictures are needed for stress documents")?;
        let _ = std::fs::remove_dir_all(out.join("stress"));
        let n = write_stress(&out, &fonts, r, &rng::Rng::new(SEED))?;
        println!("gen-invoices: {n} stress documents in {}", out.display());
        return Ok(());
    }
    // A fresh set: an old held-out folder must not linger after a seen-only run.
    for part in ["seen", "holdout"] {
        let _ = std::fs::remove_dir_all(out.join(part));
    }
    std::fs::create_dir_all(&out)?;
    let seed = rng::Rng::new(SEED);
    let written = std::sync::atomic::AtomicUsize::new(0);
    let errors = std::sync::Mutex::new(Vec::<String>::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .clamp(1, 4);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(l) = chosen.get(i) else { break };
                    match write_layout(l, &out, &fonts, renderer.as_ref(), &seed) {
                        Ok(n) => {
                            written.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(e) => errors
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(format!("{}: {e:#}", l.id)),
                    }
                }
            });
        }
    });
    let errors = errors
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    let _ = std::fs::remove_dir_all(out.join("stress"));
    if let (Some(r), None) = (renderer.as_ref(), &o.only) {
        let n = write_stress(&out, &fonts, r, &seed)?;
        written.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
    std::fs::write(
        out.join("roster.json"),
        serde_json::to_vec_pretty(&doc::roster(&all))?,
    )?;
    std::fs::write(
        out.join("set.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "seed": format!("{SEED:#x}"),
            "holdout": o.holdout,
            "scans": o.scans,
            "layouts": chosen.iter().map(|l| l.id).collect::<Vec<_>>(),
        }))?,
    )?;
    println!(
        "gen-invoices: {} files for {} layouts in {}",
        written.into_inner(),
        chosen.len(),
        out.display()
    );
    Ok(())
}

fn rerun_optimised(args: &[String]) -> Result<()> {
    println!("gen-invoices: building xtask optimised (profile docread-bench) for picture work");
    let status =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "run",
                "--quiet",
                "--profile",
                "docread-bench",
                "-p",
                "xtask",
                "--",
                "gen-invoices",
            ])
            .args(args)
            .env(OPTIMISED, "1")
            .status()
            .context("run cargo")?;
    if !status.success() {
        bail!("gen-invoices failed ({status})");
    }
    Ok(())
}

/// Write one layout's documents; returns the number of files.
fn write_layout(
    l: &layouts::Layout,
    out: &Path,
    fonts: &fonts::Fonts,
    renderer: Option<&scan::Renderer>,
    seed: &rng::Rng,
) -> Result<usize> {
    let dir = out
        .join(if l.holdout { "holdout" } else { "seen" })
        .join(l.id);
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for seq in 1..=DOCS_PER_LAYOUT {
        let d = doc::Doc::new(l, seq, seed);
        let pages = render::pages(&d, fonts);
        let bytes = pdf::write(&pages, fonts, d.tounicode, "Synthetic test document")?;
        let stem = format!("{seq:02}");
        let mut truth = d.truth();
        truth["pages"] = serde_json::json!(pages.len());
        // Pictures are of the first page; a longer document has none.
        let pictures = renderer.is_some() && pages.len() == 1;
        truth["pictures"] = serde_json::json!(pictures);
        std::fs::write(dir.join(format!("{stem}.pdf")), &bytes)?;
        std::fs::write(
            dir.join(format!("{stem}.json")),
            serde_json::to_vec_pretty(&truth)?,
        )?;
        n += 2;
        if let (Some(r), true) = (renderer, pictures) {
            let mut rng = seed.fork(&format!("{}/{seq}/pictures", l.id));
            let scan_page = r.first_page(&bytes, 300.0)?;
            let s = scan::Degrade::scan(&mut rng).apply(&scan_page, &mut rng);
            let sq = u8::try_from(rng.range(75, 90)).unwrap_or(80);
            std::fs::write(dir.join(format!("{stem}.scan.jpg")), scan::jpeg(&s, sq)?)?;
            let photo_page = r.first_page(&bytes, 200.0)?;
            let p = scan::Degrade::photo(&mut rng).apply(&photo_page, &mut rng);
            let q = u8::try_from(rng.range(40, 70)).unwrap_or(60);
            std::fs::write(dir.join(format!("{stem}.photo.jpg")), scan::jpeg(&p, q)?)?;
            n += 2;
        }
    }
    Ok(n)
}

/// The committed examples (plan D6 "about 12 samples"): IBM Plex layouts
/// only; PDFs, two scans and two photos, each with its truth.
fn samples(root: &Path) -> Result<()> {
    let fonts = fonts::Fonts::load(root, true)?;
    let renderer = scan::Renderer::new(root)?;
    let dir = root.join("fixtures").join("docs").join("read");
    std::fs::create_dir_all(&dir)?;
    let seed = rng::Rng::new(SEED).fork("samples");
    let all = layouts::all();
    let pick = |id: &str| all.iter().find(|l| l.id == id).copied();
    let plan: [(&str, u32, &str); 12] = [
        ("s01-service-classic", 1, "pdf"),
        ("s03-wholesale-grid", 1, "pdf"),
        ("s05-utility-electric", 1, "pdf"),
        ("s07-telecom", 1, "pdf"),
        ("s09-grocery-thermal", 1, "pdf"),
        ("s14-contractor-deposit", 1, "pdf"),
        ("s16-office-discount", 1, "pdf"),
        ("s18-professional", 1, "pdf"),
        ("s02-service-centered", 2, "scan"),
        ("s12-restaurant", 2, "scan"),
        ("s06-utility-water", 3, "photo"),
        ("s15-saas-card", 3, "photo"),
    ];
    for (id, seq, kind) in plan {
        let l = pick(id).with_context(|| format!("no layout {id}"))?;
        if !(fonts.has(l.font) && fonts.has(l.bold)) {
            bail!("sample {id} needs a font that is not IBM Plex");
        }
        let d = doc::Doc::new(&l, seq, &seed);
        let pages = render::pages(&d, &fonts);
        let bytes = pdf::write(&pages, &fonts, true, "Synthetic test document")?;
        let stem = format!("{id}-{seq:02}");
        let mut truth = d.truth();
        truth["tounicode"] = serde_json::json!(true);
        truth["pages"] = serde_json::json!(pages.len());
        let mut rng = seed.fork(&stem);
        let (name, data) = match kind {
            "pdf" => (format!("{stem}.pdf"), bytes),
            "scan" => {
                let page = renderer.first_page(&bytes, 200.0)?;
                let img = scan::Degrade::scan(&mut rng).apply(&page, &mut rng);
                (format!("{stem}.scan.jpg"), scan::jpeg(&img, 70)?)
            }
            _ => {
                let page = renderer.first_page(&bytes, 150.0)?;
                let img = scan::Degrade::photo(&mut rng).apply(&page, &mut rng);
                (format!("{stem}.photo.jpg"), scan::jpeg(&img, 60)?)
            }
        };
        std::fs::write(dir.join(&name), data)?;
        std::fs::write(
            dir.join(format!("{stem}.json")),
            serde_json::to_vec_pretty(&truth)?,
        )?;
        println!("gen-invoices: fixtures/docs/read/{name}");
    }
    Ok(())
}

/// The largest documents an owner can legitimately send (step 12a): ten
/// text pages, ten scanned pages (picture-only PDF, every page through
/// OCR), a 12-megapixel colour phone photo and a 600 dpi scan. The
/// accuracy harness reads each on its own and records time and memory.
fn write_stress(
    out: &Path,
    fonts: &fonts::Fonts,
    renderer: &scan::Renderer,
    seed: &rng::Rng,
) -> Result<usize> {
    let dir = out.join("stress");
    std::fs::create_dir_all(&dir)?;
    let all = layouts::all();
    let letters: Vec<&layouts::Layout> = all
        .iter()
        .filter(|l| {
            !l.holdout && l.family != layouts::Family::Receipt && l.family != layouts::Family::Slip
        })
        .collect();
    let mut rng = seed.fork("stress");
    let mut text_pages = Vec::new();
    let mut scanned_pages = Vec::new();
    for i in 0..10u32 {
        let l = letters[usize::try_from(i).unwrap_or(0) % letters.len()];
        let d = doc::Doc::new(l, i + 1, seed);
        let mut pages = render::pages(&d, fonts);
        pages.truncate(1);
        let one = pdf::write(&pages, fonts, true, "Synthetic test document")?;
        let grey = renderer.first_page(&one, 300.0)?;
        let img = scan::Degrade::scan(&mut rng).apply(&grey, &mut rng);
        scanned_pages.push(pdf::Page {
            width: pages[0].width,
            height: pages[0].height,
            items: Vec::new(),
            image: Some(pdf::PageImage {
                jpeg: scan::jpeg(&img, 80)?,
                width: img.width(),
                height: img.height(),
            }),
        });
        text_pages.extend(pages);
    }
    std::fs::write(
        dir.join("ten-pages.pdf"),
        pdf::write(&text_pages, fonts, true, "Synthetic test document")?,
    )?;
    std::fs::write(
        dir.join("ten-pages-scanned.pdf"),
        pdf::write(&scanned_pages, fonts, true, "Synthetic test document")?,
    )?;
    let d = doc::Doc::new(letters[2], 1, seed);
    let one = pdf::write(
        &render::pages(&d, fonts)[..1],
        fonts,
        true,
        "Synthetic test document",
    )?;
    // 3024 px across a 612 pt page with a 5 % border: about 340 dpi.
    let page = renderer.first_page(&one, 340.0)?;
    let photo = scan::Degrade {
        margin: 0.05,
        ..scan::Degrade::photo(&mut rng)
    }
    .apply(&page, &mut rng);
    let rgb = image::imageops::resize(
        &image::DynamicImage::ImageLuma8(photo).to_rgb8(),
        3024,
        4032,
        image::imageops::FilterType::Triangle,
    );
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)
        .encode_image(&rgb)
        .context("encode photo")?;
    std::fs::write(dir.join("photo-12mp.jpg"), buf)?;
    let page = renderer.first_page(&one, 600.0)?;
    let big = scan::Degrade::scan(&mut rng).apply(&page, &mut rng);
    std::fs::write(dir.join("scan-600dpi.jpg"), scan::jpeg(&big, 85)?)?;
    // The same scan in colour (a flatbed set to colour by default).
    let colour = image::DynamicImage::ImageLuma8(big).to_rgb8();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85)
        .encode_image(&colour)
        .context("encode colour scan")?;
    std::fs::write(dir.join("scan-600dpi-colour.jpg"), buf)?;
    // The largest picture the worker accepts (A02 limits: 256 MiB decoded):
    // 8800 x 9600 RGB is 253 MB.
    let cap = image::imageops::resize(&colour, 8800, 9600, image::imageops::FilterType::Nearest);
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 60)
        .encode_image(&cap)
        .context("encode the largest picture")?;
    std::fs::write(dir.join("picture-at-decode-cap.jpg"), buf)?;
    Ok(6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        let o = parse(&["--holdout".into(), "--layouts".into(), "a,b".into()]).unwrap();
        assert!(o.holdout && o.scans && !o.samples);
        assert_eq!(o.only, Some(vec!["a".to_string(), "b".to_string()]));
        assert!(parse(&["--bogus".into()]).is_err());
        assert!(parse(&["--out".into()]).is_err());
    }

    #[test]
    fn a_text_pdf_round_trips_through_the_writer() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let fonts = fonts::Fonts::load(&root, true).unwrap();
        let l = layouts::all()[0];
        let d = doc::Doc::new(&l, 1, &rng::Rng::new(SEED));
        let pages = render::pages(&d, &fonts);
        let with = pdf::write(&pages, &fonts, true, "t").unwrap();
        let without = pdf::write(&pages, &fonts, false, "t").unwrap();
        assert!(with.starts_with(b"%PDF-"));
        let has = |b: &[u8], n: &[u8]| b.windows(n.len()).any(|w| w == n);
        assert!(has(&with, b"/ToUnicode") && !has(&without, b"/ToUnicode"));
        assert!(has(&with, b"/FontFile2") && has(&with, b"+IBMPlexSans"));
    }
}
