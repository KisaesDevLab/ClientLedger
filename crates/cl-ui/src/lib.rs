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
pub mod account_names;
pub mod app;
pub mod automation;
pub mod company;
pub mod data_bus;
pub mod engine_link;
pub mod errors;
pub mod fonts;
pub mod forms;
pub mod grid_env;
pub mod grid_form;
pub mod i18n;
pub mod icon;
pub mod input;
pub mod layout;
pub mod list_field;
pub mod logging;
pub mod prefs;
pub mod screens;
pub mod shell;
pub mod tag_names;
pub mod templates_runner;
pub mod theme;
pub mod widgets;
pub mod windows;
