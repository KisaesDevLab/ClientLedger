//! The demo's Set Closing Date (P29 WP5): the open company's closing date
//! and the demo closing-date password, checked the way the engine does.

use cl_api::{ApiError, ClosingDateChange, ClosingPasswordStatus};
use cl_core::{CalendarDate, Secret};

use super::DemoState;

fn company(d: &mut DemoState) -> Option<&mut super::DemoCompany> {
    let open = d
        .open
        .or_else(|| d.companies.first().map(|c| c.tenant_id))?;
    d.companies.iter_mut().find(|c| c.tenant_id == open)
}

/// The closing date and whether a password is set.
#[must_use]
pub fn status(d: &mut DemoState) -> ClosingPasswordStatus {
    let set = d.closing_password.is_some();
    ClosingPasswordStatus {
        closing_date: company(d).and_then(|c| c.closed_through),
        set_by: None,
        set_at: None,
        password_set: set,
    }
}

fn check(d: &DemoState, given: Option<&Secret>) -> Result<(), ApiError> {
    match (&d.closing_password, given) {
        (None, _) => Ok(()),
        (Some(p), Some(g)) if g.expose() == p => Ok(()),
        (Some(_), Some(_)) => Err(ApiError::ClosingPasswordRejected),
        (Some(_), None) => Err(ApiError::ClosingPasswordRequired),
    }
}

/// Sets or clears the date (the first password goes with it).
///
/// # Errors
/// The engine's password refusals.
pub fn set_date(
    d: &mut DemoState,
    change: &ClosingDateChange,
) -> Result<Option<CalendarDate>, ApiError> {
    if d.closing_password.is_some() {
        check(d, change.closing_password.as_ref())?;
    } else if let Some(p) = &change.closing_password {
        d.closing_password = Some(p.expose().to_owned());
    }
    if let Some(c) = company(d) {
        c.closed_through = change.closing_date;
    }
    Ok(change.closing_date)
}

/// Changes the password.
///
/// # Errors
/// A wrong current password.
pub fn set_password(
    d: &mut DemoState,
    current: Option<&Secret>,
    new: Option<&Secret>,
) -> Result<bool, ApiError> {
    check(d, current)?;
    d.closing_password = new.map(|n| n.expose().to_owned());
    Ok(d.closing_password.is_some())
}
