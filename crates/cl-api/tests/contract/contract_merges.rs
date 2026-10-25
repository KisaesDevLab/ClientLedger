//! Contact and tag merges under the closing date (U-47,
//! `merge-closing-date-v1`, Q-231, Q-P11aa), in tenant B so no other test
//! sees the lock. A merge that rewrites a transaction dated on or before the
//! closing date is refused without the closing-date password (nothing
//! moves) and goes through with it. The tag merge moves a line tag too
//! (Q-P11t). A wrong password (403) is covered by the cl-api unit tests:
//! the engine counts wrong closing-date passwords per user (10 in 15
//! minutes) and the run's other closed-period tests use that budget.

use cl_api::{
    ApiError, Basis, Capabilities, ContactFields, ContactType, JeLine, Method, NewContact,
    NewJournalEntry, Session, Side, TagFields,
};
use cl_core::{Money, Secret};
use serde_json::json;

use crate::common::{date, key, line, m, pair, post};
use crate::support::{Harness, unique};

const PASSWORD: &str = "closing-merge-1";

/// Clears the closing date (and the password once set), also when the test
/// panics, so later tests in tenant B see an open period.
struct Unlock<'a> {
    h: &'a Harness,
    s: Session,
    password_set: bool,
}

impl Drop for Unlock<'_> {
    fn drop(&mut self) {
        if self.password_set {
            let _ = self.h.run(self.s.set_closing_password(
                Some(&Secret::new(PASSWORD)),
                None,
                key(),
            ));
        }
        let _ = self.h.run(self.s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({"lockDate": null})),
        ));
    }
}

pub fn closed_period_needs_the_password_u47(h: &Harness) {
    if !h.require_cap(
        Capabilities::MERGE_CLOSING_DATE,
        "U-47 merge-closing-date-v1",
    ) || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15")
    {
        return;
    }
    let s = h.s3();
    let mut unlock = Unlock {
        h,
        s: s.clone(),
        password_set: false,
    };
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
    let contact = |name: &str| {
        h.run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, unique(name)),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("contact")
        .record
        .id
    };
    let tag = |name: &str| {
        h.run(s.tags().create(
            &TagFields {
                name: unique(name),
                color: None,
                group_id: None,
                description: None,
                tag_number: None,
            },
            key(),
        ))
        .expect("tag")
        .record
        .id
    };
    let (dup, kept) = (contact("Merge dup"), contact("Merge kept"));
    let (old_tag, new_tag) = (tag("Merge old"), tag("Merge new"));
    let (cash, supplies) = pair(h, &s);
    let entry = NewJournalEntry::new(
        date(2026, 1, 10),
        Some(unique("closed merge")),
        Basis::Both,
        vec![
            JeLine {
                contact: Some(dup),
                tag: Some(old_tag),
                ..line(supplies.id, Side::Debit(m("7.0000")))
            },
            line(cash.id, Side::Credit(m("7.0000"))),
        ],
    )
    .expect("balanced");
    let posted = post(h, &s, &entry).record.id;
    let line_contact = || {
        h.run(s.transactions().get(posted))
            .expect("entry")
            .record
            .lines
            .iter()
            .find(|l| l.account_id == supplies.id)
            .and_then(|l| l.contact_id)
    };

    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-01-31"})),
    ))
    .expect("set the closing date");
    let pw = Secret::new(PASSWORD);
    assert!(
        h.run(s.set_closing_password(None, Some(&pw), key()))
            .expect("set password")
    );
    unlock.password_set = true;
    let needs_password = |e: &ApiError| {
        assert!(
            matches!(e, ApiError::ClosedPeriod(c)
                if c.password_required == Some(true) && c.lock_date == Some(date(2026, 1, 31))),
            "{e:?}"
        );
    };

    // Contacts: the closed-period entry's line names the duplicate.
    let e = h
        .run(s.contacts().merge(dup, kept, None, key()))
        .expect_err("needs the password");
    needs_password(&e);
    assert_eq!(line_contact(), Some(dup), "nothing moved");
    h.run(s.contacts().merge(dup, kept, Some(&pw), key()))
        .expect("merge with the password");
    assert_eq!(line_contact(), Some(kept), "the closed-period line moved");

    // Tags: the same entry carries the tag on a line.
    let e = h
        .run(s.tags().merge(old_tag, new_tag, None, key()))
        .expect_err("needs the password");
    needs_password(&e);
    assert!(h.run(s.tags().get(old_tag)).is_ok(), "source kept");
    h.run(s.tags().merge(old_tag, new_tag, Some(&pw), key()))
        .expect("merge with the password");
    assert!(h.run(s.tags().get(old_tag)).is_err(), "source deleted");
    let moved = h.run(s.tags().usage(new_tag)).expect("usage");
    assert_eq!(moved.transaction_lines, 1, "the line tag moved");
}
