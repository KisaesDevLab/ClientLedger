//! The reading pipeline (WP-A; plan D3 "Pipeline"): words → layout →
//! fields (template first, then rules, per field) → kind → lines →
//! validate. After field extraction, steps only add checks; none writes an
//! amount (AC3).
//!
//! **Consumer:** the worker calls [`read`] once per job with every page's
//! words (text layer or OCR, already mapped to original-page boxes).
//! The signature of [`read`] and [`ReadInput`] is fixed for WP-B; WP-A
//! fills in the body. The vendor/card match and the AI gate run later, in
//! the desktop.
//!
//! Checks are ordered pages, header fields, kind, totals, then lines, and
//! capped at `MAX_CHECKS`, so a document with hundreds of failing lines
//! still yields a reading the desktop accepts.

use crate::model::{
    Check, CheckCode, DocKind, MAX_CHECKS, PageSummary, PageWords, Reader, Reading, RuleId,
    Template, WordSource,
};
use crate::protocol::JobHint;
use crate::{fields, kind, layout, lines, templates, validate};
use cl_core::CalendarDate;

/// Everything the rules need for one document.
#[derive(Debug, Clone, Copy)]
pub struct ReadInput<'a> {
    /// The pages that were read, in order (at most `model::MAX_PAGES`).
    pub pages: &'a [PageWords],
    /// Pages in the document, including any not read.
    pub page_count: u16,
    /// 0-based pages whose words could not be had (OCR failed or timed
    /// out); each gets a `PageNotRead` check.
    pub unread_pages: &'a [u16],
    /// The company's vendor layout templates; the reading names the one
    /// applied by index.
    pub templates: &'a [Template],
    pub hint: JobHint,
    /// The owner's "today".
    pub today: CalendarDate,
}

/// Read one document's words into a [`Reading`].
#[must_use]
pub fn read(input: &ReadInput<'_>) -> Reading {
    let ocr = input.pages.iter().any(|p| p.source == WordSource::Ocr);
    let mut r = Reading::empty(
        if ocr { Reader::Ocr } else { Reader::TextPdf },
        input.page_count,
    );
    r.pages = input
        .pages
        .iter()
        .map(|p| PageSummary {
            page: p.page,
            source: Some(p.source),
            words: u32::try_from(p.words.len()).unwrap_or(u32::MAX),
        })
        .chain(input.unread_pages.iter().map(|&page| PageSummary {
            page,
            source: None,
            words: 0,
        }))
        .collect();
    r.pages.sort_by_key(|p| p.page);
    r.words = input.pages.to_vec();
    let mut checks: Vec<Check> = Vec::new();
    for &page in input.unread_pages {
        checks.push(page_check(CheckCode::PageNotRead, Some(page)));
    }
    if input.page_count > crate::model::MAX_PAGES {
        checks.push(page_check(CheckCode::PagesSkipped, None));
    }

    let layout = layout::analyze(input.pages);
    let fingerprint = templates::fingerprint(input.pages);
    let matched = fingerprint
        .as_ref()
        .and_then(|f| templates::best_match(f, input.templates));
    let template = matched.map(|i| &input.templates[i]);

    let extracted = fields::extract(&layout, input.pages, template, input.today);
    checks.extend(extracted.checks);
    let (kind, kind_confidence) = kind::classify(&extracted.header, &extracted.signals, input.hint);
    if kind == DocKind::Unknown {
        checks.push(Check {
            code: CheckCode::KindUnclear,
            rule: RuleId::new("kind.unclear"),
            field: None,
            line: None,
            expected: None,
            found: None,
            difference: None,
            spot: None,
        });
    }
    let (items, line_checks) = lines::extract(
        &layout,
        input.pages,
        template.and_then(|t| t.line_table.as_ref()),
    );
    checks.extend(validate::check(&extracted.header, &items));
    checks.extend(line_checks);
    checks.truncate(MAX_CHECKS);

    r.kind = kind;
    r.kind_confidence = kind_confidence;
    r.header = extracted.header;
    r.lines = items;
    r.checks = checks;
    r.fingerprint = fingerprint;
    if extracted.template_used {
        r.reader = Reader::Template;
        r.template = matched.and_then(|i| u16::try_from(i).ok());
    }
    r
}

fn page_check(code: CheckCode, page: Option<u16>) -> Check {
    Check {
        code,
        rule: RuleId::new("pipeline.pages"),
        field: None,
        line: None,
        expected: None,
        found: None,
        difference: None,
        spot: page.map(|page| crate::model::Spot {
            page,
            bbox: crate::model::Bbox::PAGE,
        }),
    }
}

/// Synthetic pages for the rules' tests: words placed on a letter-size
/// page in ten-thousandths, 80 units per character, 120 units high.
#[cfg(test)]
pub(crate) mod testdoc {
    use crate::model::{Bbox, Confidence, PageWords, Word, WordSource};

    pub const CHAR_W: u16 = 80;
    pub const WORD_H: u16 = 120;

    #[derive(Debug, Clone)]
    pub struct Doc {
        pages: Vec<Vec<Word>>,
        pub source: WordSource,
        pub confidence: Confidence,
    }

    impl Doc {
        pub fn new() -> Doc {
            Doc {
                pages: vec![Vec::new()],
                source: WordSource::TextLayer,
                confidence: Confidence::CERTAIN,
            }
        }

        /// Start a new page.
        pub fn page(&mut self) {
            self.pages.push(Vec::new());
        }

        pub fn word_box(&mut self, l: u16, t: u16, r: u16, b: u16, text: &str) {
            let bbox =
                Bbox::new(l.min(10_000), t.min(10_000), r.min(10_000), b.min(10_000)).unwrap();
            let confidence = self.confidence;
            self.pages.last_mut().unwrap().push(Word {
                text: text.into(),
                bbox,
                confidence,
            });
        }

        /// Left-aligned words from `x`.
        pub fn text(&mut self, x: u16, y: u16, s: &str) {
            let mut left = x;
            for w in s.split(' ').filter(|w| !w.is_empty()) {
                let width = CHAR_W * u16::try_from(w.chars().count()).unwrap();
                self.word_box(left, y, left + width, y + WORD_H, w);
                left += width + CHAR_W;
            }
        }

        /// One word right-aligned at `right`.
        pub fn amount(&mut self, right: u16, y: u16, s: &str) {
            let width = CHAR_W * u16::try_from(s.chars().count()).unwrap();
            self.word_box(right - width, y, right, y + WORD_H, s);
        }

        pub fn pages(&self) -> Vec<PageWords> {
            self.pages
                .iter()
                .enumerate()
                .map(|(i, words)| PageWords {
                    page: u16::try_from(i).unwrap(),
                    aspect: 12_941,
                    source: self.source,
                    words: words.clone(),
                })
                .collect()
        }
    }

    /// A one-page invoice: letterhead, reference, date, terms, a bill-to
    /// block, a line table at y = 4000 and a totals box. `lines` are
    /// (description, qty, rate, amount) as printed.
    pub fn invoice(
        reference: &str,
        date: &str,
        lines: &[(&str, &str, &str, &str)],
        subtotal: &str,
        tax: &str,
        total: &str,
    ) -> Doc {
        let mut d = Doc::new();
        d.text(500, 400, "Riverbend Supply Co.");
        d.text(7000, 400, "INVOICE");
        d.text(500, 600, "1200 Mill Road");
        d.text(500, 800, "Springfield, MO 65801");
        d.text(500, 1000, "(417) 555-0142");
        d.text(6000, 800, &format!("Invoice #: {reference}"));
        d.text(6000, 1000, &format!("Date: {date}"));
        d.text(6000, 1200, "Terms: Net 30");
        d.text(500, 1800, "Bill To:");
        d.text(500, 2000, "Ozark Ridge Farm");
        d.text(500, 2200, "PO Box 12");
        d.text(500, 4000, "Description");
        d.text(5500, 4000, "Qty");
        d.text(7000, 4000, "Rate");
        d.text(8900, 4000, "Amount");
        let mut y = 4200;
        for (desc, qty, rate, amount) in lines {
            d.text(500, y, desc);
            if !qty.is_empty() {
                d.amount(5800, y, qty);
            }
            if !rate.is_empty() {
                d.amount(7600, y, rate);
            }
            d.amount(9500, y, amount);
            y += 200;
        }
        y += 200;
        for (label, value) in [("Subtotal", subtotal), ("Sales Tax", tax), ("Total", total)] {
            if !value.is_empty() {
                d.text(6000, y, label);
                d.amount(9500, y, value);
                y += 200;
            }
        }
        d
    }
}

#[cfg(test)]
pub(crate) mod tests;
