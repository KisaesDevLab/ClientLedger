//! P29 WP5 (Q-155, U-229): only an Admin moves the closing date or changes
//! its password; a bookkeeper with company-settings rights is refused with
//! 403 `CLOSING_DATE_ADMIN_ONLY` and other settings still save.

use cl_api::Method;
use serde_json::json;

use crate::support::Harness;

/// U-229's capability.
const CLOSING_ADMIN: &str = "closing-date-admin-v1";

/// A bookkeeper with full company-settings rights cannot set the closing
/// date or the closing-date password; their other settings still save.
pub fn bookkeeper_cannot_move_the_closing_date(h: &Harness) {
    if !h.require_cap(CLOSING_ADMIN, "U-229") {
        return;
    }
    let s = &h.s1;
    let (user, bk) = h.bookkeeper();
    let r = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": { "company_settings": "full" } })),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
    let put = |path: &str, body: serde_json::Value| {
        h.run(bk.raw(Method::Put, path, &[], Some(&body)))
            .expect("raw")
    };
    let r = put(
        "/api/v1/company/settings",
        json!({ "lockDate": "2026-01-31", "closingPassword": "bk-tries-1" }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(
        r.body["error"]["code"], "CLOSING_DATE_ADMIN_ONLY",
        "{}",
        r.body
    );
    let r = put(
        "/api/v1/company/closing-password",
        json!({ "newPassword": "bk-tries-1" }),
    );
    assert_eq!(r.status, 403, "{}", r.body);
    // Other settings are still the bookkeeper's to change.
    let r = put(
        "/api/v1/company/settings",
        json!({ "dateFormat": "MM/DD/YYYY" }),
    );
    assert_eq!(r.status, 200, "{}", r.body);
}
