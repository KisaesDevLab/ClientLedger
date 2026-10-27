//! The optional readers (A03 step 21, plan D5, Q-A03j, Q-A03k): when the
//! reading made on this computer left fields empty or unsure, the owner may
//! ask the company's AI for help. Never automatic, never without the
//! owner's click.
//!
//! - **Where it runs.** A local Ollama model when the company's AI runs on
//!   Ollama (nothing leaves this computer or network; no cloud switch is
//!   needed). An Ollama cloud model (name ending `:cloud` or `-cloud`)
//!   runs on Ollama's servers: it takes the online route and its consent. An online provider only with the company's own consent for
//!   reading documents online: the page **text** needs
//!   `document_text_cloud`; page **pictures** need `document_images_cloud`
//!   as well, plus the engine's cloud-vision setting (which the desktop
//!   never turns on, Q-A03v). `receipt_ocr` ("Read bills and receipts")
//!   never stands in for either switch, but it must be on: it is the
//!   owner's switch for AI reading documents at all.
//! - **What is sent.** The words the local reader found, line by line, at
//!   most [`cl_api::ASSIST_MAX_TEXT_CHARS`] characters; a picture only to
//!   an online provider when allowed and the document itself is one PNG of
//!   at most 2 MiB and 4096 px on a side (A03 renders no pages, Q-A03u). A
//!   larger PNG is not shrunk here (only the worker decodes pictures): its
//!   text goes alone and the owner is told ([`Picture::TooLarge`]). Never
//!   logged.
//! - **What comes back** goes through the same checks as the local
//!   reading: `cl_docread::ai_gate::merge` runs **before** any form value
//!   exists, so an AI amount is kept only when a word on the page equals it
//!   (Q-A03j), and the rules' own values always win; a disagreement is a
//!   check, never a change. The vendor is matched on the desktop again
//!   (Q-A03o). Every value it fills is *read, not accepted* (non-negotiable
//!   4).

use cl_api::{AssistLine, AssistReader, AssistRequest, AssistSuggestion, DraftKind};
use cl_docread::ai_gate::{self, AiLine, AiReading, AiSource};
use cl_docread::layout;
use cl_docread::model::{CheckCode, Confidence, ConfidenceBucket, Found, Reading};
use cl_docread::vendor_match::{self, VendorCandidate};
use cl_settings::AiProvider;

use super::LocalReading;

/// The owner's and the engine's switches that decide whether, and where,
/// a second reading may run.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssistSwitches {
    /// The engine offers the assist (`document-assist-v1`).
    pub capability: bool,
    /// AI is on and the company agreed to the current AI notice.
    pub ai: bool,
    /// "Read bills and receipts" is on (`receipt_ocr`).
    pub reading: bool,
    /// The provider document reading uses.
    pub provider: Option<AiProvider>,
    /// The company's consent to send page text online.
    pub text_cloud: bool,
    /// The company's consent to send page pictures online.
    pub images_cloud: bool,
    /// The engine sends pictures to an online provider at all.
    pub cloud_vision: bool,
    /// The provider is Ollama but its model runs on Ollama's servers
    /// (`…:cloud`, `…-cloud`): the cloud route, with its consent (review
    /// S5).
    pub ollama_cloud: bool,
}

/// Where a second reading runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Ollama, on this computer or network.
    Local,
    /// The company's online provider; `images` when pictures may go too.
    Cloud {
        /// Page pictures may be sent.
        images: bool,
    },
}

/// Why no second reading is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoRoute {
    /// The engine has no assist.
    Unsupported,
    /// AI, document reading or a provider is off.
    Off,
    /// The provider is online and the company hasn't agreed to send
    /// documents to it.
    NeedsConsent,
}

/// Decides where a second reading may run.
///
/// # Errors
/// Why none may run.
pub fn route(s: AssistSwitches) -> Result<Route, NoRoute> {
    if !s.capability {
        return Err(NoRoute::Unsupported);
    }
    let Some(provider) = s.provider.filter(|_| s.ai && s.reading) else {
        return Err(NoRoute::Off);
    };
    if !provider.is_cloud() && !s.ollama_cloud {
        return Ok(Route::Local);
    }
    if !s.text_cloud {
        return Err(NoRoute::NeedsConsent);
    }
    Ok(Route::Cloud {
        images: s.images_cloud && s.cloud_vision,
    })
}

fn unsure<T>(f: Option<&Found<T>>) -> bool {
    f.is_none_or(|f| f.confidence.bucket() == ConfidenceBucket::Low)
}

/// True when the local reading left a main field empty or unsure: the
/// vendor (or no vendor matched), the date, the ref. no., the total, or the
/// lines.
#[must_use]
pub fn needs_help(local: &LocalReading) -> bool {
    let r = &local.reading;
    let h = &r.header;
    let total = h.total.as_ref().or(h.balance_due.as_ref());
    local.vendor.is_none()
        || unsure(h.vendor.as_ref())
        || unsure(h.date.as_ref())
        || unsure(h.reference.as_ref())
        || unsure(total)
        || r.checks.iter().any(|c| c.code == CheckCode::LinesNotRead)
        || r.lines
            .iter()
            .any(|l| l.amount.confidence.bucket() == ConfidenceBucket::Low)
}

/// The page text as the local reader found it: one text line per line, a
/// blank line between pages, at most [`cl_api::ASSIST_MAX_TEXT_CHARS`]
/// characters.
#[must_use]
pub fn page_text(reading: &Reading) -> String {
    let lay = layout::analyze(&reading.words);
    let mut out = String::new();
    let mut page = None;
    for line in &lay.lines {
        let words = &reading.words[line.page_index].words;
        if page.is_some_and(|p| p != line.page_index) {
            out.push('\n');
        }
        page = Some(line.page_index);
        out.push_str(&layout::join(line.words.iter().map(|&w| &words[w])));
        out.push('\n');
    }
    out.chars().take(cl_api::ASSIST_MAX_TEXT_CHARS).collect()
}

/// Whether the document's picture goes with a second reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// No picture may go here, or the document isn't a PNG.
    NotSent,
    /// The PNG goes with the text.
    Sent,
    /// Pictures may go, but this PNG is over the engine's limits (2 MiB, or
    /// [`cl_api::ASSIST_MAX_IMAGE_SIDE_PX`] on a side): only the text goes,
    /// and the owner is told. The desktop never decodes or shrinks it (A03
    /// D5: pictures are decoded only in the reading worker).
    TooLarge,
}

impl Picture {
    /// The control channel's name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Picture::NotSent => "not_sent",
            Picture::Sent => "sent",
            Picture::TooLarge => "too_large",
        }
    }
}

/// Whether the document's picture goes, on this route.
#[must_use]
pub fn picture(route: Route, document: &[u8]) -> Picture {
    // A local model reads the text only (an Ollama text model can't take
    // pictures, and the words are already this computer's own reading).
    if !matches!(route, Route::Cloud { images: true }) || !cl_api::is_png(document) {
        return Picture::NotSent;
    }
    // The size is read from the PNG's header; nothing is decoded here.
    if document.len() <= cl_api::ASSIST_MAX_IMAGE_BYTES && cl_api::png_fits(document) {
        Picture::Sent
    } else {
        Picture::TooLarge
    }
}

/// What to send: the page text and, when pictures may go and the document
/// is one PNG within the engine's limits, that picture. `kind`: what the
/// owner is entering it as (Enter Bill: a bill; A04's draft forms: the
/// form's kind).
#[must_use]
pub fn request(
    local: &LocalReading,
    route: Route,
    document: &[u8],
    kind: DraftKind,
) -> AssistRequest {
    let picture = picture(route, document) == Picture::Sent;
    AssistRequest {
        text: page_text(&local.reading),
        page_images: if picture {
            vec![document.to_vec()]
        } else {
            Vec::new()
        },
        kind_hint: Some(kind),
    }
}

fn ai_line(l: &AssistLine) -> AiLine {
    AiLine {
        description: l.description.clone(),
        quantity: l.quantity,
        unit_price: l.unit_price,
        amount: l.amount,
    }
}

/// The answer in the gate's shape.
#[must_use]
pub fn ai_reading(s: &AssistSuggestion) -> AiReading {
    AiReading {
        vendor: s.vendor.clone(),
        reference: s.vendor_invoice_number.clone(),
        date: s.bill_date,
        due_date: s.due_date,
        subtotal: s.subtotal,
        tax: s.tax,
        total: s.total,
        amount_paid: None,
        lines: s.lines.iter().map(ai_line).collect(),
        confidence: s
            .confidence_bp
            .and_then(Confidence::new)
            .unwrap_or(Confidence::NONE),
    }
}

/// The local reading with the AI's answer merged under the gate, and the
/// vendor matched again when the local reading had none.
#[must_use]
pub fn merge(
    local: &LocalReading,
    answer: &AssistSuggestion,
    vendors: &[VendorCandidate],
) -> LocalReading {
    let source = match answer.reader {
        AssistReader::SelfHosted => AiSource::Ollama,
        AssistReader::Cloud => AiSource::Cloud,
    };
    let mut reading = ai_gate::merge(local.reading.clone(), &ai_reading(answer), source);
    let vendor = local.vendor.or_else(|| {
        let m = vendor_match::match_vendor(&reading.header, reading.fingerprint.as_ref(), vendors);
        // A name that won over a past layout is checked (review S4).
        if let Some(check) = m
            .as_ref()
            .and_then(|m| vendor_match::layout_check(m, &reading.header, vendors))
            && reading.checks.len() < cl_docread::model::MAX_CHECKS
        {
            reading.checks.push(check);
        }
        m
    });
    LocalReading {
        reading,
        vendor,
        card: local.card,
    }
}

#[cfg(test)]
#[path = "assist_tests.rs"]
mod tests;
