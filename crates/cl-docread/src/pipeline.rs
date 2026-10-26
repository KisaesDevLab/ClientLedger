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

use crate::model::{
    Check, CheckCode, PageSummary, PageWords, Reader, Reading, RuleId, Template, WordSource,
};
use crate::protocol::JobHint;
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
///
/// Stub (step 2): carries the words and page summaries through and reports
/// unread and skipped pages; finds no fields. WP-A replaces the body.
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
    for &page in input.unread_pages {
        r.checks
            .push(page_check(CheckCode::PageNotRead, Some(page)));
    }
    if input.page_count > crate::model::MAX_PAGES {
        r.checks.push(page_check(CheckCode::PagesSkipped, None));
    }
    let _ = (input.templates, input.hint, input.today);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Confidence, Word};

    #[test]
    fn words_and_unread_pages_are_carried_through() {
        let pages = [PageWords {
            page: 0,
            aspect: 12_941,
            source: WordSource::Ocr,
            words: vec![Word {
                text: "x".into(),
                bbox: Bbox::PAGE,
                confidence: Confidence::MEDIUM,
            }],
        }];
        let r = read(&ReadInput {
            pages: &pages,
            page_count: 12,
            unread_pages: &[1],
            templates: &[],
            hint: JobHint::default(),
            today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
        });
        assert_eq!(r.reader, Reader::Ocr);
        assert_eq!(r.pages.len(), 2);
        assert_eq!(r.words, pages);
        let codes: Vec<_> = r.checks.iter().map(|c| c.code).collect();
        assert_eq!(codes, [CheckCode::PageNotRead, CheckCode::PagesSkipped]);
        r.validate(0).unwrap();
    }
}
