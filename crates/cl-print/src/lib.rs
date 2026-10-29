//! `cl-print`: checks and forms as PDF bytes (P19).
//!
//! Layouts are specs in millimetres on US Letter ([`layout`]); the renderer
//! ([`pdf`]) embeds IBM Plex and draws every text run exactly where the
//! layout resolved it. Amounts arrive as `rust_decimal::Decimal` parsed by
//! the caller from the engine's strings (Q-P19c); this crate formats them
//! and, for voucher stubs only, adds them up (Q-213). Never floating point
//! for money: `f32` is page geometry in millimetres.
//!
//! This crate makes bytes only. Sending a PDF to a printer or a viewer is
//! `cl-host::os_print` (Q-P19a).
#![forbid(unsafe_code)]

pub mod amount;
pub mod checks;
pub mod envelopes;
pub mod files;
pub mod fonts;
pub mod forms;
pub mod image;
pub mod layout;
pub mod pdf;
pub mod print_queue;

pub use amount::{amount_in_words, check_figures, format_amount, format_quantity};
pub use checks::{
    AlignmentOffset, CheckData, CheckLabels, CheckOptions, CheckStyle, StubBill, StubCredit,
    StubData, StubTotals, alignment_test, layout_checks,
};
pub use envelopes::{EnvelopeOptions, layout_envelope, layout_envelopes};
pub use files::{default_save_dir, pdf_file_name};
pub use fonts::Font;
pub use forms::{
    CompanyInfo, FormDoc, FormLabels, FormStyle, ItemLine, layout_form, resolve_footer,
};
pub use image::Logo;
pub use layout::{Align, Document, Page, PageSize, Rect, ResolvedPage, Run, TextBox, resolve};
pub use pdf::{render, render_resolved};
pub use print_queue::{
    Confirmation, DocKind, PrintQueue, PrintRun, QueueError, RunOutcome, RunState,
};

/// Why a document could not be laid out or rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrintError {
    /// A document without pages.
    #[error("there is nothing to print")]
    NoPages,
    /// An embedded font could not be read or subset (a build defect).
    #[error("a built-in font could not be used: {0}")]
    Font(String),
    /// The logo file could not be used.
    #[error("the logo could not be used: {0}")]
    Logo(String),
    /// A check amount below zero.
    #[error("a check cannot be printed for a negative amount")]
    NegativeAmount,
    /// A check amount with a fraction of a cent.
    #[error("the amount has a fraction of a cent, which a check cannot show")]
    SubCent,
    /// An amount too large to print or add up.
    #[error("the amount is too large to print")]
    AmountTooLarge,
    /// An alignment offset beyond 10 mm.
    #[error("the alignment adjustment must be within 10 mm in each direction")]
    OffsetOutOfRange,
    /// A form with more header fields than the template holds.
    #[error("the form has more header fields than fit")]
    TooManyFields,
}
