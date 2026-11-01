//! The demo's invoice print layouts (P20, U-130 played for `--demo`):
//! "Standard" (the default) and "Simple" (no quantity or rate), by the
//! engine's rules: names unique (any case), exactly one default, archived
//! never deleted, the default can't be archived, an archived layout can't
//! become the default. Clients keep a default layout. Synthetic data.

use std::collections::BTreeMap;

use cl_api::{ApiError, InvoiceLayout, InvoiceLayoutId, LayoutDraft};
use cl_core::ids::ContactId;
use serde_json::json;

use super::DemoState;

/// Layouts and each client's default layout.
#[derive(Clone, Debug, Default)]
pub struct DemoLayouts {
    /// The layouts, default first.
    pub layouts: Vec<InvoiceLayout>,
    /// Clients' default layouts.
    pub clients: BTreeMap<ContactId, InvoiceLayoutId>,
    writes: u64,
}

fn layout(id: &str, name: &str, default: bool, qty: bool) -> Option<InvoiceLayout> {
    serde_json::from_value(json!({
        "id": id, "name": name, "isDefault": default, "isArchived": false,
        "accentColor": "#1f6feb", "logoUrl": null, "showShipTo": true, "showPoNumber": true,
        "showTerms": true, "footerText": null, "footerOverrides": {},
        "columns": {"item": false, "description": true, "serviceDate": true, "quantity": qty, "rate": qty},
        "resolvedFooters": {"invoice": "", "cash_sale": "", "credit_memo": "", "statement": ""},
        "invoiceCount": 0, "createdAt": "2026-10-31T10:00:00.000Z", "updatedAt": "2026-10-31T10:00:00.000Z",
    }))
    .ok()
}

impl DemoLayouts {
    /// Standard (default) and Simple.
    #[must_use]
    pub fn seeded() -> DemoLayouts {
        DemoLayouts {
            layouts: [
                layout(
                    "e1000000-0000-4000-8000-000000000001",
                    "Standard",
                    true,
                    true,
                ),
                layout(
                    "e1000000-0000-4000-8000-000000000002",
                    "Simple",
                    false,
                    false,
                ),
            ]
            .into_iter()
            .flatten()
            .collect(),
            ..DemoLayouts::default()
        }
    }
}

fn refused(status: u16, code: &str) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code}}),
    )
}

fn stamp(state: &mut DemoState) -> Option<cl_core::Timestamp> {
    state.layouts.writes += 1;
    let n = i64::try_from(state.layouts.writes).unwrap_or(0);
    cl_core::Timestamp::from_unix_millis(1_793_100_000_000 + n).ok()
}

/// `GET /company/invoice-layouts[?includeArchived=true]`.
#[must_use]
pub fn list(state: &DemoState, archived: bool) -> Vec<InvoiceLayout> {
    state
        .layouts
        .layouts
        .iter()
        .filter(|l| archived || !l.is_archived)
        .cloned()
        .collect()
}

fn name_taken(state: &DemoState, name: &str, except: Option<InvoiceLayoutId>) -> bool {
    state
        .layouts
        .layouts
        .iter()
        .any(|l| Some(l.id) != except && l.name.trim().eq_ignore_ascii_case(name.trim()))
}

fn apply(l: &mut InvoiceLayout, d: &LayoutDraft) {
    l.name = d.name.trim().to_string();
    l.show_ship_to = d.show_ship_to;
    l.show_po_number = d.show_po_number;
    l.show_terms = d.show_terms;
    let footer = d.footer_text.trim();
    l.footer_text = (!footer.is_empty()).then(|| footer.to_string());
    l.resolved_footers.invoice = footer.to_string();
    l.columns = d.columns;
}

/// `POST /company/invoice-layouts`.
///
/// # Errors
/// 409 `INVOICE_LAYOUT_NAME_IN_USE`; `Local(InvalidValue)` for a blank name.
pub fn create(state: &mut DemoState, d: &LayoutDraft) -> Result<InvoiceLayout, ApiError> {
    if d.name.trim().is_empty() {
        return Err(ApiError::Local(cl_api::LocalValidation::InvalidValue));
    }
    if name_taken(state, &d.name, None) {
        return Err(refused(409, "INVOICE_LAYOUT_NAME_IN_USE"));
    }
    let n = state.layouts.layouts.len() + 1;
    let mut l = layout(
        &format!("e1000000-0000-4000-8000-{n:012}"),
        &d.name,
        false,
        true,
    )
    .ok_or(ApiError::Internal)?;
    apply(&mut l, d);
    l.updated_at = stamp(state);
    state.layouts.layouts.push(l.clone());
    Ok(l)
}

fn find(state: &mut DemoState, id: InvoiceLayoutId) -> Result<&mut InvoiceLayout, ApiError> {
    state
        .layouts
        .layouts
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| refused(404, "INVOICE_LAYOUT_NOT_FOUND"))
}

fn check_stale(l: &InvoiceLayout, expected: &InvoiceLayout) -> Result<(), ApiError> {
    if l.updated_at != expected.updated_at {
        return Err(refused(409, "STALE"));
    }
    Ok(())
}

/// `PUT /company/invoice-layouts/{id}` (edit).
///
/// # Errors
/// 404, 409 `STALE`, 409 `INVOICE_LAYOUT_NAME_IN_USE`.
pub fn update(
    state: &mut DemoState,
    expected: &InvoiceLayout,
    d: &LayoutDraft,
) -> Result<InvoiceLayout, ApiError> {
    if name_taken(state, &d.name, Some(expected.id)) {
        return Err(refused(409, "INVOICE_LAYOUT_NAME_IN_USE"));
    }
    let t = stamp(state);
    let l = find(state, expected.id)?;
    check_stale(l, expected)?;
    apply(l, d);
    l.updated_at = t;
    Ok(l.clone())
}

/// `PUT /company/invoice-layouts/{id}` `{isArchived}`.
///
/// # Errors
/// 404, 409 `STALE`, 409 `INVOICE_LAYOUT_IS_DEFAULT`.
pub fn set_archived(
    state: &mut DemoState,
    expected: &InvoiceLayout,
    archived: bool,
) -> Result<InvoiceLayout, ApiError> {
    let t = stamp(state);
    let l = find(state, expected.id)?;
    check_stale(l, expected)?;
    if archived && l.is_default {
        return Err(refused(409, "INVOICE_LAYOUT_IS_DEFAULT"));
    }
    l.is_archived = archived;
    l.updated_at = t;
    Ok(l.clone())
}

/// `POST /company/invoice-layouts/{id}/default`.
///
/// # Errors
/// 404, 409 `INVOICE_LAYOUT_ARCHIVED`.
pub fn set_default(state: &mut DemoState, id: InvoiceLayoutId) -> Result<InvoiceLayout, ApiError> {
    if find(state, id)?.is_archived {
        return Err(refused(409, "INVOICE_LAYOUT_ARCHIVED"));
    }
    let t = stamp(state);
    for l in &mut state.layouts.layouts {
        let was = l.is_default;
        l.is_default = l.id == id;
        if was != l.is_default {
            l.updated_at = t;
        }
    }
    state.layouts.layouts.sort_by_key(|l| !l.is_default);
    find(state, id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::many_single_char_names, reason = "a short story")]
    fn layouts_follow_the_engine_rules() {
        let mut s = DemoState::default();
        let all = list(&s, false);
        assert_eq!(all.len(), 2);
        let (std, simple) = (all[0].clone(), all[1].clone());
        assert!(std.is_default);
        let e = set_archived(&mut s, &std, true).expect_err("default");
        assert_eq!(
            cl_api::invoice_refusal(&e),
            None,
            "a layout refusal, shown by Company Information"
        );
        let a = set_archived(&mut s, &simple, true).expect("archive");
        assert_eq!(list(&s, false).len(), 1);
        assert_eq!(list(&s, true).len(), 2);
        assert!(set_default(&mut s, a.id).is_err(), "archived");
        let u = set_archived(&mut s, &a, false).expect("unarchive");
        set_default(&mut s, u.id).expect("default");
        assert_eq!(list(&s, false)[0].id, u.id);
        let d = LayoutDraft {
            name: "standard".into(),
            ..LayoutDraft::of(&std)
        };
        assert!(create(&mut s, &d).is_err(), "names are unique in any case");
        let d = LayoutDraft {
            name: "Detailed".into(),
            ..d
        };
        assert_eq!(create(&mut s, &d).expect("new").name, "Detailed");
        assert!(
            update(&mut s, &std, &LayoutDraft::of(&std)).is_err(),
            "stale"
        );
    }
}
