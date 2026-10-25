//! Intake folders of the open company (A01, engine U-93, capability
//! `intake-settings-v1`): `GET` and `PUT /api/v1/company/intake-settings`.
//!
//! - The list is stored in the engine (`companies.intake_settings`) so every
//!   computer that opens the company sees the same folders (Q-A01a). Which
//!   folders this computer actually watches is a local confirmation, kept
//!   outside the engine (Q-A01b).
//! - `PUT` replaces the whole list and names the `updatedAt` it last read
//!   (`null` before the first save). A different stored value is 409
//!   `STALE` (`Conflict { Stale }`), nothing changed. The token is opaque:
//!   it is echoed back exactly as the engine sent it, never parsed or
//!   reformatted.
//! - Without `intake-settings-v1` nothing is sent and both calls answer
//!   `Local(Unsupported)`; the P17 single folder keeps working (Q-A01q).
//! - Folder names and paths are owner data: `Debug` of every type here goes
//!   through [`IntakeFolder`]'s redacting `Debug`, and nothing here logs.

use cl_core::intake::IntakeFolder;
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{INTAKE_SETTINGS_GET, INTAKE_SETTINGS_PUT};
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// Most folders a company may have (the engine refuses more with 400
/// `VALIDATION`; checked here first so nothing is sent).
pub const INTAKE_FOLDERS_MAX: usize = 20;

/// The company's intake folders as the engine stores them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntakeSettings {
    /// Every folder, in the order saved.
    pub folders: Vec<IntakeFolder>,
    /// The list's own change token (`None` before the first save). Opaque:
    /// pass it back unchanged as `expected` to [`Intake::put`].
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// The `PUT` body. `expectedUpdatedAt` is always written, `null` included
/// (the engine's schema is strict and the key is required).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PutBody<'a> {
    folders: &'a [IntakeFolder],
    expected_updated_at: Option<&'a str>,
}

/// Intake-folder operations of a session (`session.intake()`).
#[derive(Clone, Copy, Debug)]
pub struct Intake<'a>(pub(crate) &'a Session);

impl Session {
    /// Intake folders of the open company.
    #[must_use]
    pub fn intake(&self) -> Intake<'_> {
        Intake(self)
    }
}

impl Intake<'_> {
    fn supported(self) -> Result<(), ApiError> {
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::INTAKE_SETTINGS)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// The company's folders and their change token.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `intake-settings-v1` (nothing sent);
    /// `NoCompanyOpen`; `Forbidden` without company-settings view
    /// permission; `Decode`; any [`ApiError`].
    pub async fn get(&self) -> Result<IntakeSettings, ApiError> {
        self.supported()?;
        let call = Call::at(Method::Get, INTAKE_SETTINGS_GET.route());
        let reply = self.0.send(&INTAKE_SETTINGS_GET, call, None).await?;
        decode(&reply, "intake settings")
    }

    /// Replaces the whole list. `expected` is the `updated_at` last read,
    /// exactly as the engine sent it (`None` before the first save).
    /// Returns the stored list with its new token.
    ///
    /// Not in an engine idempotency set (pre-handler retries only): a
    /// repeated full replace either matches or is `STALE`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `intake-settings-v1` and
    /// `Local(InvalidValue)` for more than [`INTAKE_FOLDERS_MAX`] folders
    /// (nothing sent); `Conflict { Stale }` when the list changed since it
    /// was read (409 `STALE`, nothing changed: read it again);
    /// `Validation` (400, with the field path, such as
    /// `folders.0.path`); `Forbidden` without company-settings update
    /// permission; any [`ApiError`].
    pub async fn put(
        &self,
        folders: &[IntakeFolder],
        expected: Option<&str>,
    ) -> Result<IntakeSettings, ApiError> {
        self.supported()?;
        if folders.len() > INTAKE_FOLDERS_MAX {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = json_body(&PutBody {
            folders,
            expected_updated_at: expected,
        })?;
        let call = Call::at(Method::Put, INTAKE_SETTINGS_PUT.route()).json_body(body);
        let reply = self.0.send(&INTAKE_SETTINGS_PUT, call, None).await?;
        decode(&reply, "intake settings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConflictReason;
    use crate::resources::testkit::{company_session, json, requests, session_with_caps};
    use cl_core::ids::IntakeFolderId;
    use cl_core::intake::IntakeKind;
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const ROUTE: &str = "/api/v1/company/intake-settings";
    const ID: &str = "7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a6b";
    /// An odd-looking token: more digits than JavaScript prints, an offset.
    /// It must come back byte for byte.
    const TOKEN: &str = "2026-10-25T17:55:01.123456+00:00";

    fn folder() -> IntakeFolder {
        IntakeFolder {
            id: IntakeFolderId::parse(ID).expect("id"),
            name: "Canary Plumbing bills".into(),
            path: r"\\canary-server\Scans\Canary".into(),
            default_kind: IntakeKind::Bills,
            auto_read: false,
            keep_originals: true,
        }
    }

    fn stored(token: &str) -> String {
        format!(
            r#"{{"folders":[{{"id":"{ID}","name":"Canary Plumbing bills",
                "path":"\\\\canary-server\\Scans\\Canary","defaultKind":"bills",
                "autoRead":false,"keepOriginals":true}}],"updatedAt":"{token}"}}"#
        )
    }

    async fn served(server: &MockServer) -> Session {
        session_with_caps(server, &[Capabilities::INTAKE_SETTINGS]).await
    }

    #[tokio::test]
    async fn get_reads_folders_and_keeps_the_token_exact() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(json(200, &stored(TOKEN)))
            .mount(&server)
            .await;
        let s = served(&server).await;
        let got = s.intake().get().await.expect("get");
        assert_eq!(got.folders, vec![folder()]);
        assert_eq!(got.updated_at.as_deref(), Some(TOKEN));
        let gets = requests(&server, "GET").await;
        let get = gets.iter().find(|r| r.url.path() == ROUTE).expect("sent");
        assert!(get.headers.get("x-company-id").is_some(), "company scoped");
    }

    #[tokio::test]
    async fn a_new_company_has_no_folders_and_no_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(json(200, r#"{"folders":[],"updatedAt":null}"#))
            .mount(&server)
            .await;
        let got = served(&server).await.intake().get().await.expect("get");
        assert_eq!(got, IntakeSettings::default());
    }

    #[tokio::test]
    async fn put_echoes_the_token_and_sends_null_before_the_first_save() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(ROUTE))
            .respond_with(json(200, &stored("2026-10-25T18:00:00.000Z")))
            .mount(&server)
            .await;
        let s = served(&server).await;
        let saved = s.intake().put(&[folder()], Some(TOKEN)).await.expect("put");
        assert_eq!(
            saved.updated_at.as_deref(),
            Some("2026-10-25T18:00:00.000Z")
        );
        s.intake().put(&[], None).await.expect("first save");

        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 2);
        let first: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(first["expectedUpdatedAt"], TOKEN, "echoed byte for byte");
        assert!(
            String::from_utf8_lossy(&puts[0].body).contains(TOKEN),
            "not re-encoded"
        );
        assert_eq!(first["folders"][0]["path"], r"\\canary-server\Scans\Canary");
        assert_eq!(first["folders"][0]["keepOriginals"], true);
        assert_eq!(
            first.as_object().map(serde_json::Map::len),
            Some(2),
            "only folders and expectedUpdatedAt: {first}"
        );
        let second: Value = serde_json::from_slice(&puts[1].body).expect("json");
        assert!(
            second
                .as_object()
                .is_some_and(|o| o.get("expectedUpdatedAt") == Some(&Value::Null)),
            "the key is present and null: {second}"
        );
        assert_eq!(second["folders"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_stale_put_is_a_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(ROUTE))
            .respond_with(json(
                409,
                r#"{"error":{"message":"Intake folders were changed elsewhere","code":"STALE",
                    "details":{"updatedAt":"2026-10-25T18:00:00.000Z"}}}"#,
            ))
            .mount(&server)
            .await;
        let e = served(&server)
            .await
            .intake()
            .put(&[folder()], Some(TOKEN))
            .await
            .expect_err("stale");
        assert!(
            matches!(
                e,
                ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(requests(&server, "PUT").await.len(), 1, "never retried");
    }

    #[tokio::test]
    async fn a_refused_folder_is_a_validation_error() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(ROUTE))
            .respond_with(json(
                400,
                r#"{"error":{"message":"Validation failed","code":"VALIDATION",
                    "details":[{"path":["folders",0,"path"],"message":"Folder must not use a device path"}]}}"#,
            ))
            .mount(&server)
            .await;
        let e = served(&server)
            .await
            .intake()
            .put(&[folder()], None)
            .await
            .expect_err("validation");
        assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    }

    #[tokio::test]
    async fn without_the_capability_nothing_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(json(200, &stored(TOKEN)))
            .mount(&server)
            .await;
        // Learned capabilities without intake-settings-v1, and none at all.
        for s in [
            session_with_caps(&server, &["attachments-company-v1"]).await,
            company_session(&server),
        ] {
            assert_eq!(
                s.intake().get().await,
                Err(ApiError::Local(LocalValidation::Unsupported))
            );
            assert_eq!(
                s.intake().put(&[folder()], Some(TOKEN)).await,
                Err(ApiError::Local(LocalValidation::Unsupported))
            );
        }
        let sent = server.received_requests().await.expect("recorded");
        assert!(
            sent.iter().all(|r| r.url.path() != ROUTE),
            "no intake request without the capability"
        );
    }

    #[tokio::test]
    async fn more_than_twenty_folders_are_refused_locally() {
        let server = MockServer::start().await;
        let s = served(&server).await;
        let many: Vec<IntakeFolder> = (0..=INTAKE_FOLDERS_MAX)
            .map(|i| IntakeFolder {
                id: IntakeFolderId::parse(&format!("7f3a2c10-5b6d-4e8f-9a0b-{i:012}")).expect("id"),
                name: format!("Folder {i}"),
                path: format!(r"C:\Scans\{i}"),
                ..folder()
            })
            .collect();
        assert_eq!(
            s.intake().put(&many, None).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
        assert!(requests(&server, "PUT").await.is_empty());
    }

    #[test]
    fn debug_never_shows_a_name_or_path() {
        let settings = IntakeSettings {
            folders: vec![folder()],
            updated_at: Some(TOKEN.into()),
        };
        for text in [format!("{settings:?}"), format!("{settings:#?}")] {
            assert!(!text.contains("canary"), "{text}");
            assert!(!text.contains("Canary"), "{text}");
            assert!(!text.contains("Scans"), "{text}");
            assert!(text.contains(ID), "{text}");
        }
    }
}
