//! Shared helpers for the resource stub tests (test builds only).

use cl_core::ids::{CompanyId, TenantId};
use wiremock::{MockServer, Request, ResponseTemplate};

use crate::session::tests::session_on;
use crate::session::{Scope, Session};

pub(crate) const COMPANY: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
pub(crate) const TENANT: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

/// A session with a company open, talking to `server`.
pub(crate) fn company_session(server: &MockServer) -> Session {
    session_on(
        server,
        Scope::Company {
            tenant_id: TenantId::parse(TENANT).expect("tenant"),
            company_id: CompanyId::parse(COMPANY).expect("company"),
        },
    )
}

pub(crate) fn json(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
}

/// Received requests with `method`.
pub(crate) async fn requests(server: &MockServer, method: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == method)
        .collect()
}

/// A company session whose client learned `caps` from `/desktop/info`.
pub(crate) async fn session_with_caps(server: &MockServer, caps: &[&str]) -> Session {
    use wiremock::matchers::{method, path};
    wiremock::Mock::given(method("GET"))
        .and(path("/api/v1/desktop/info"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"mode":"desktop","initialized":true,"capabilities":{}}}"#,
                serde_json::to_string(caps).expect("caps")
            ),
        ))
        .mount(server)
        .await;
    let s = company_session(server);
    s.client().desktop_info().await.expect("desktop info");
    s
}

/// The query string of a request.
pub(crate) fn query(r: &Request) -> String {
    r.url.query().unwrap_or("").to_string()
}
