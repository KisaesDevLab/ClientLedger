//! Export All Data (Q-56): every list and transaction with its lines as
//! CSV, from the engine's full export, in one zip with a manifest. The
//! CSV files open in Excel and every spreadsheet program; a native
//! workbook is deferred to the report engine (Q-P08f).

use std::path::{Path, PathBuf};

use cl_api::Session;
use serde::Serialize;

use crate::error::CompanyError;
use crate::zipfile;

/// What [`export_all`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportAllOutcome {
    /// The zip file.
    pub path: PathBuf,
    /// File names and row counts, in manifest order.
    pub files: Vec<(String, u64)>,
    /// Zip size.
    pub bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    app: &'a str,
    app_version: &'a str,
    exported_at: String,
    files: Vec<ManifestFile<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile<'a> {
    name: &'a str,
    row_count: u64,
}

const README: &str = "ClientLedger - Export All Data\n\n\
Each .csv file is plain text with a header row; amounts are exact decimal strings. \
Open them in Excel, LibreOffice, or any spreadsheet program (File > Open, or import as text with comma separators). \
Nothing in this zip contains passwords or keys.\n";

/// Exports the open company to `dest` (a `.zip` path).
///
/// # Errors
/// `Api` (needs an open company); `Io`.
pub async fn export_all(session: &Session, dest: &Path) -> Result<ExportAllOutcome, CompanyError> {
    let manifest = session.export_manifest().await?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut files = Vec::new();
    for f in &manifest.files {
        let text = session.export_file(&f.name).await?;
        entries.push((f.name.clone(), text.into_bytes()));
        files.push((f.name.clone(), f.row_count));
    }
    let m = Manifest {
        app: "ClientLedger",
        app_version: crate::APP_VERSION,
        exported_at: cl_settings::recent::now().to_wire(),
        files: manifest
            .files
            .iter()
            .map(|f| ManifestFile {
                name: &f.name,
                row_count: f.row_count,
            })
            .collect(),
    };
    entries.push((
        "manifest.json".into(),
        serde_json::to_vec_pretty(&m).map_err(|_| CompanyError::Metadata)?,
    ));
    entries.push(("README.txt".into(), README.as_bytes().to_vec()));
    let dest = dest.to_path_buf();
    let d2 = dest.clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<u64, CompanyError> {
        if let Some(dir) = d2.parent() {
            std::fs::create_dir_all(dir).map_err(CompanyError::io("create export folder"))?;
        }
        zipfile::write_stored(&d2, &entries).map_err(CompanyError::io("write export"))?;
        std::fs::metadata(&d2)
            .map(|m| m.len())
            .map_err(CompanyError::io("stat export"))
    })
    .await
    .map_err(|_| CompanyError::Io {
        what: "write export",
        source: std::io::Error::other("worker stopped"),
    })??;
    Ok(ExportAllOutcome {
        path: dest,
        files,
        bytes,
    })
}
