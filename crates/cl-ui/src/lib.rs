//! The `ClientLedger` application: theme and fonts, the shell (title bar, menus,
//! rail, tabs, status bar), the window manager, dialogs, and the glue that
//! connects the supervised engine to the typed API client.
#![forbid(unsafe_code)]
// Drawing functions are long and linear by nature, brand names (ClientLedger,
// MyBooks, QuickBooks) appear throughout the docs, and the shell keeps several
// independent UI flags; splitting any of that to satisfy a count would hurt
// readability.
#![allow(
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::struct_excessive_bools
)]

pub mod about;
pub mod app;
pub mod automation;
pub mod engine_link;
pub mod errors;
pub mod fonts;
pub mod i18n;
pub mod icon;
pub mod input;
pub mod logging;
pub mod prefs;
pub mod shell;
pub mod theme;
pub mod widgets;
pub mod windows;
