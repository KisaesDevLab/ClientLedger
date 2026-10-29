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

pub mod fonts;
pub mod image;
pub mod layout;
pub mod pdf;

pub use fonts::Font;
pub use image::Logo;
pub use layout::{Align, Document, Page, Rect, ResolvedPage, Run, TextBox, resolve};
pub use pdf::{render, render_resolved};

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
}
