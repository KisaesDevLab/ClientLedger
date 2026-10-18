//! Developer-only screens (`--demo` only, P10 plan §2.12, S8). They are
//! listed in `cl_model::commands::DEV_COMMANDS`, built only by the shell's
//! `make_any` when it runs with `--demo`, and never saved in layouts.

pub mod grid_demo;
pub mod lines;
pub mod register;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_model::CommandId;

use crate::company::CompanyLink;
use crate::windows::Screen;

/// The developer screen for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    today: CalendarDate,
    link: Option<&Arc<CompanyLink>>,
) -> Option<Box<dyn Screen>> {
    match kind.as_str() {
        grid_demo::KIND => Some(Box::new(grid_demo::GridDemo::new(
            today,
            link.map(Arc::clone),
        ))),
        _ => None,
    }
}
