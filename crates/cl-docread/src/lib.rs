//! Local document reading (A03, ADD-01): from a page's words to a
//! [`model::Reading`] of a bill or receipt, with no network and no AI.
//!
//! **Frozen in A03 step 2** (plan §3): this file, [`model`] and [`protocol`].
//! Every other module is a documented stub whose public signatures later
//! work packages fill in; the module owner may refine a stub's signatures
//! until a consumer in another work package uses them (noted per module).
//!
//! | Module | Owner | Used by |
//! | --- | --- | --- |
//! | [`model`], [`protocol`] | frozen | everyone |
//! | [`pipeline`] | WP-A | the worker (WP-B) |
//! | [`layout`], [`amount`], [`dates`] | WP-A (step 8) | rules; `amount` also the desktop |
//! | [`fields`], [`kind`] | WP-A (step 9) | [`pipeline`] |
//! | [`lines`], [`validate`] | WP-A (step 10) | [`pipeline`] |
//! | [`vendor_match`], [`card`], [`templates`], [`dupes`], [`ai_gate`] | WP-A (step 11) | the desktop (WP-G), A04 |
//! | [`register_match`], [`correct`] | A04 WP-R (step 4) | the desktop (A04 F2), A05 |
//! | [`tsv`], [`ocr_geom`] | WP-B (step 5) | the worker |
//! | `pdf_text`, `preprocess` (feature `worker`) | WP-B (steps 4, 5) | the worker |
//!
//! Rules: no I/O, no clock (callers pass "today"), no floats. Only
//! `pdf_text`, `preprocess` and `ocr_geom` may use floating point, for page
//! geometry, and they may not touch money (`cargo xtask nofloat`). Amounts
//! are [`cl_core::Money`]; the only document-amount parser is
//! [`amount::parse`]; after field extraction no step writes an amount
//! (AC3): checks only point at problems.
//!
//! Spawning processes, sockets and files are out of bounds here and in the
//! worker (plan D2 "Network"): `cl-host` owns every process.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod model;
pub mod protocol;

pub mod ai_gate;
pub mod amount;
pub mod card;
pub mod correct;
pub mod dates;
pub mod dupes;
pub mod fields;
pub mod kind;
pub mod layout;
pub mod lines;
pub mod ocr_geom;
#[cfg(feature = "worker")]
pub mod pdf_text;
pub mod pipeline;
#[cfg(feature = "worker")]
pub mod preprocess;
pub mod register_match;
pub mod templates;
pub mod tsv;
pub mod validate;
pub mod vendor_match;

pub use model::{
    Bbox, Check, CheckCode, Confidence, DocKind, Found, Header, Line, PageWords, Reading, RuleId,
    Spot, Template, Word,
};
pub use pipeline::{ReadInput, read};
