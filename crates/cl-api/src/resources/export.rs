//! Export All Data (Q-56): `GET /api/v1/export/full` lists the CSV files
//! the engine produces (`accounts.csv`, `contacts.csv`, `items.csv`,
//! `tags.csv`, `transactions.csv`, `journal_lines.csv`) with row counts;
//! `GET /api/v1/export/full/{fileName}` returns one file as CSV text with
//! numerics as decimal strings. `cl-company` zips them.

use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{EXPORT_FULL, EXPORT_FULL_FILE};
use crate::session::{Session, decode};

/// One exported file.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    /// File name (`accounts.csv`).
    pub name: String,
    /// Data rows.
    pub row_count: u64,
}

/// `GET /api/v1/export/full`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportManifest {
    /// The files.
    pub files: Vec<ExportFile>,
}

fn file_name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.to_ascii_lowercase().ends_with(".csv")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        && !name.contains("..")
}

impl Session {
    /// The export manifest of the open company (all dates).
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn export_manifest(&self) -> Result<ExportManifest, ApiError> {
        let reply = self
            .send(
                &EXPORT_FULL,
                Call::at(Method::Get, EXPORT_FULL.route()),
                None,
            )
            .await?;
        decode(&reply, "export manifest")
    }

    /// One exported CSV file as text.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a name the engine would not serve
    /// (nothing sent); `NotFound`; any [`ApiError`].
    pub async fn export_file(&self, name: &str) -> Result<String, ApiError> {
        if !file_name_ok(name) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::new(
            Method::Get,
            EXPORT_FULL_FILE.route(),
            EXPORT_FULL_FILE.route().replace("{fileName}", name),
        );
        let reply = self.send(&EXPORT_FULL_FILE, call, None).await?;
        Ok(String::from_utf8_lossy(&reply.body).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn manifest_and_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/export/full"))
            .respond_with(json(
                200,
                r#"{"files":[{"name":"accounts.csv","rowCount":12}],"startDate":null,"endDate":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/export/full/accounts.csv"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("id,name\n1,Cash\n", "text/csv"))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let m = s.export_manifest().await.expect("manifest");
        assert_eq!(m.files[0].row_count, 12);
        let text = s.export_file("accounts.csv").await.expect("file");
        assert!(text.starts_with("id,name"));
        let e = s.export_file("../x.csv").await.expect_err("traversal");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    }
}
