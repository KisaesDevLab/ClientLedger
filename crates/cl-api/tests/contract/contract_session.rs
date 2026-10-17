//! Session behaviour against the live engine (AC5): refresh rotation, a
//! rotated-away cookie is refused, proactive and reactive refresh, and
//! single-flight refresh under concurrent requests.

use cl_api::{AccountFilter, ApiError, AuthFailure, Method, PageRequest};

use crate::support::Harness;

const REFRESH: &str = "/api/v1/auth/refresh";

fn refreshes(h: &Harness) -> u64 {
    h.observer.sent(Method::Post, REFRESH)
}

pub fn refresh_rotates_and_old_cookie_is_refused(h: &Harness) {
    let s = &h.s1;
    let old = s.refresh_cookie_for_test().expect("cookie");
    h.run(s.force_refresh()).expect("refresh 1");
    let first = s.refresh_cookie_for_test().expect("cookie");
    assert_ne!(first.expose(), old.expose(), "cookie not rotated");
    h.run(s.force_refresh()).expect("refresh 2");
    let second = s.refresh_cookie_for_test().expect("cookie");
    assert_ne!(second.expose(), first.expose(), "cookie not rotated");
    let e = h
        .run(h.client.refresh_with_cookie_for_test(&old))
        .expect_err("old cookie");
    assert!(matches!(e, ApiError::Unauthorized(_)), "{e:?}");
    // The session itself is unaffected.
    h.run(
        s.accounts()
            .list(&AccountFilter::default(), PageRequest::first_max()),
    )
    .expect("still signed in");
}

pub fn proactive_refresh_when_the_token_expires(h: &Harness) {
    let s = &h.s1;
    let before = refreshes(h);
    s.expire_access_for_test();
    h.run(
        s.accounts()
            .list(&AccountFilter::default(), PageRequest::first_max()),
    )
    .expect("list after expiry");
    assert_eq!(refreshes(h) - before, 1);
}

pub fn concurrent_requests_share_one_refresh(h: &Harness) {
    let s = &h.s1;
    let before = refreshes(h);
    s.expire_access_for_test();
    let filter = AccountFilter::default();
    let cfilter = cl_api::ContactFilter::default();
    let (accounts, contacts) = (s.accounts(), s.contacts());
    let (a, b, c) = h.run(async {
        tokio::join!(
            accounts.list(&filter, PageRequest::first_max()),
            contacts.list(&cfilter, PageRequest::first_max()),
            s.ledger_version(),
        )
    });
    a.expect("accounts");
    b.expect("contacts");
    c.expect("stamp");
    assert_eq!(refreshes(h) - before, 1, "refresh must be single-flight");
}

pub fn reactive_refresh_after_engine_401(h: &Harness) {
    let s = &h.s1;
    s.corrupt_access_for_test();
    let raw = h
        .run(s.raw(Method::Get, "/api/v1/accounts", &[], None))
        .expect("raw");
    assert_eq!(raw.status, 401);
    assert_eq!(raw.body["error"]["code"], "UNAUTHORIZED");
    let before = refreshes(h);
    h.run(
        s.accounts()
            .list(&AccountFilter::default(), PageRequest::first_max()),
    )
    .expect("refreshed and replayed");
    assert_eq!(refreshes(h) - before, 1);
}

/// Logging out ends the session: later calls are refused locally. Uses S2
/// (its conflict tests have run), so the 3-session cap never evicts S1.
pub fn logout_ends_the_session(h: &Harness) {
    let s = h.s2();
    let copy = s.clone();
    h.run(s.logout()).expect("logout");
    let e = h.run(copy.companies()).expect_err("ended");
    assert!(
        matches!(e, ApiError::Unauthorized(AuthFailure::SessionExpired)),
        "{e:?}"
    );
}
