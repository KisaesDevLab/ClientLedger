//! Files attached to a check (P13 WP2): upload, list and download
//! (`/api/v1/attachments`).
//!
//! - A check is an engine `expense`, so its files use attachable type
//!   `expense` (the type the engine's own check route links drafts to).
//! - Before a check is saved, files go to a **draft**: attachable type
//!   `draft` with a [`DraftKey`] the app makes. The check create then sends
//!   that key as `draftAttachmentId` and the engine moves every file of the
//!   draft onto the new check. The key is not an attachment id: one draft
//!   may hold several files.
//! - Files are checked here before anything is sent: at most 10 MB (the
//!   engine's limit), and a PDF, PNG or JPEG by their first bytes. The
//!   content type sent is the one the bytes show, never a guess from the
//!   name.
//! - File names and contents are owner data: never logged.

use std::fmt;
use std::str::FromStr;

use cl_core::Timestamp;
use cl_core::ids::{IdError, TransactionId, parse_uuid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{ATTACHMENT_DOWNLOAD, ATTACHMENTS_LIST, ATTACHMENTS_UPLOAD};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode};
use crate::wire::{Page, PageRequest};

/// The largest file the engine accepts (its upload limit).
pub const ATTACHMENT_MAX_BYTES: usize = 10 * 1024 * 1024;

/// The longest file name sent (the engine column holds 255 characters).
const NAME_MAX_CHARS: usize = 200;

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Uuid);

        impl $name {
            /// Strict parse (hyphenated UUID).
            ///
            /// # Errors
            /// See [`IdError`].
            pub fn parse(s: &str) -> Result<Self, IdError> {
                parse_uuid(s).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(&self.0.hyphenated())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

uuid_id!(
    /// One stored file.
    AttachmentId
);

uuid_id!(
    /// The draft a not-yet-saved check collects its files under. Sent on
    /// the check create as `draftAttachmentId`.
    DraftKey
);

impl DraftKey {
    /// A fresh draft (random).
    #[must_use]
    pub fn new() -> DraftKey {
        DraftKey(Uuid::new_v4())
    }
}

impl Default for DraftKey {
    fn default() -> Self {
        DraftKey::new()
    }
}

/// Where a file goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachTo {
    /// A saved check (or any other expense).
    Expense(TransactionId),
    /// A check that is not saved yet.
    Draft(DraftKey),
}

impl AttachTo {
    fn kind(self) -> &'static str {
        match self {
            AttachTo::Expense(_) => "expense",
            AttachTo::Draft(_) => "draft",
        }
    }

    fn id(self) -> String {
        match self {
            AttachTo::Expense(id) => id.to_string(),
            AttachTo::Draft(d) => d.to_string(),
        }
    }
}

/// The kinds of file a check takes, known by their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `%PDF-`.
    Pdf,
    /// The PNG signature.
    Png,
    /// `FF D8 FF`.
    Jpeg,
}

impl FileKind {
    /// What the bytes are, or `None` for anything else.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<FileKind> {
        if bytes.starts_with(b"%PDF-") {
            Some(FileKind::Pdf)
        } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
            Some(FileKind::Png)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(FileKind::Jpeg)
        } else {
            None
        }
    }

    /// The content type sent.
    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            FileKind::Pdf => "application/pdf",
            FileKind::Png => "image/png",
            FileKind::Jpeg => "image/jpeg",
        }
    }
}

/// Checks a file before upload: not empty, at most 10 MB, a PDF, PNG or
/// JPEG.
///
/// # Errors
/// `AttachmentTooLarge` or `AttachmentType`.
pub fn check_file(bytes: &[u8]) -> Result<FileKind, LocalValidation> {
    if bytes.is_empty() || bytes.len() > ATTACHMENT_MAX_BYTES {
        return Err(LocalValidation::AttachmentTooLarge);
    }
    FileKind::sniff(bytes).ok_or(LocalValidation::AttachmentType)
}

/// A stored file as the engine lists it (storage paths are not kept).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// Id.
    pub id: AttachmentId,
    /// The file's name.
    pub file_name: String,
    /// Size in bytes.
    #[serde(default)]
    pub file_size: Option<u64>,
    /// Content type.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// `expense`, `draft`, …
    pub attachable_type: String,
    /// The record (or draft) it belongs to.
    pub attachable_id: String,
    /// Uploaded.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

/// Never prints the file name (owner data).
impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("id", &self.id)
            .field("file_size", &self.file_size)
            .field("mime_type", &self.mime_type)
            .field("attachable_type", &self.attachable_type)
            .finish_non_exhaustive()
    }
}

/// A name safe inside a multipart header: no quotes, backslashes, path
/// separators or control characters; never empty; at most 200 characters.
fn safe_name(name: &str, kind: FileKind) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\' | '/'))
        .take(NAME_MAX_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        let ext = match kind {
            FileKind::Pdf => "pdf",
            FileKind::Png => "png",
            FileKind::Jpeg => "jpg",
        };
        format!("attachment.{ext}")
    } else {
        cleaned
    }
}

/// `multipart/form-data` with the two text fields first, then the file.
/// Returns the boundary and the body. The boundary is random and checked
/// not to occur in the file.
fn multipart(to: AttachTo, name: &str, kind: FileKind, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = loop {
        let b = format!("cl-{}", Uuid::new_v4().simple());
        if !bytes.windows(b.len()).any(|w| w == b.as_bytes()) {
            break b;
        }
    };
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (field, value) in [
        ("attachableType", to.kind().to_string()),
        ("attachableId", to.id()),
    ] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {}\r\n\r\n",
            kind.mime()
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// File operations of a session (`session.attachments()`).
#[derive(Clone, Copy, Debug)]
pub struct Attachments<'a>(pub(crate) &'a Session);

impl Session {
    /// Files on records of the open company.
    #[must_use]
    pub fn attachments(&self) -> Attachments<'_> {
        Attachments(self)
    }
}

impl Attachments<'_> {
    /// Uploads a file to a saved expense or to a draft. The file is checked
    /// first (nothing is sent when it is refused).
    ///
    /// # Errors
    /// `Local(AttachmentTooLarge | AttachmentType)` (nothing sent);
    /// `Validation` when the engine refuses the file; any [`ApiError`].
    pub async fn upload(
        &self,
        to: AttachTo,
        file_name: &str,
        bytes: &[u8],
        key: IdempotencyKey,
    ) -> Result<Attachment, ApiError> {
        #[derive(Deserialize)]
        struct W {
            attachment: Attachment,
        }
        let kind = check_file(bytes).map_err(ApiError::Local)?;
        let (boundary, body) = multipart(to, &safe_name(file_name, kind), kind, bytes);
        let mut call =
            Call::at(Method::Post, ATTACHMENTS_UPLOAD.route()).multipart_body(&boundary, body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&ATTACHMENTS_UPLOAD, call, None).await?;
        decode::<W>(&reply, "attachment")
            .map(|w| w.attachment)
            .map_err(saved_unreadable)
    }

    /// Uploads a file to a check that is not saved yet.
    ///
    /// # Errors
    /// As [`Attachments::upload`].
    pub async fn upload_draft(
        &self,
        draft: DraftKey,
        file_name: &str,
        bytes: &[u8],
        key: IdempotencyKey,
    ) -> Result<Attachment, ApiError> {
        self.upload(AttachTo::Draft(draft), file_name, bytes, key)
            .await
    }

    /// Uploads a file to a saved check.
    ///
    /// # Errors
    /// As [`Attachments::upload`].
    pub async fn attach(
        &self,
        check: TransactionId,
        file_name: &str,
        bytes: &[u8],
        key: IdempotencyKey,
    ) -> Result<Attachment, ApiError> {
        self.upload(AttachTo::Expense(check), file_name, bytes, key)
            .await
    }

    /// One page of the files on a check (or a draft).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(
        &self,
        of: AttachTo,
        page: PageRequest,
    ) -> Result<Page<Attachment>, ApiError> {
        let mut call = Call::at(Method::Get, ATTACHMENTS_LIST.route());
        call.query = vec![
            ("attachable_type".into(), of.kind().into()),
            ("attachable_id".into(), of.id()),
        ];
        call.query.extend(page.query());
        let reply = self.0.send(&ATTACHMENTS_LIST, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// A file's bytes (at most the client's reply cap, 32 MB).
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn download(&self, id: AttachmentId) -> Result<Vec<u8>, ApiError> {
        let call = Call::new(
            Method::Get,
            ATTACHMENT_DOWNLOAD.route(),
            ATTACHMENT_DOWNLOAD.route().replace("{id}", &id.to_string()),
        );
        let reply = self.0.send(&ATTACHMENT_DOWNLOAD, call, None).await?;
        Ok(reply.body.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ATT: &str = "5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const CHECK: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const PDF: &[u8] = b"%PDF-1.4\n%canary\n";

    fn reply(kind: &str, id: &str) -> String {
        format!(
            r#"{{"attachment":{{"id":"{ATT}","tenantId":"x","fileName":"receipt.pdf","filePath":"/secret/path",
            "fileSize":17,"mimeType":"application/pdf","attachableType":"{kind}","attachableId":"{id}",
            "ocrTotal":null,"createdAt":"2026-10-20T10:00:00.000Z"}}}}"#
        )
    }

    #[test]
    fn files_are_checked_by_size_and_first_bytes() {
        assert_eq!(check_file(PDF), Ok(FileKind::Pdf));
        assert_eq!(
            check_file(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0]),
            Ok(FileKind::Png)
        );
        assert_eq!(check_file(&[0xff, 0xd8, 0xff, 0xe0]), Ok(FileKind::Jpeg));
        assert_eq!(check_file(b""), Err(LocalValidation::AttachmentTooLarge));
        assert_eq!(
            check_file(b"MZ\x90\0"),
            Err(LocalValidation::AttachmentType)
        );
        assert_eq!(check_file(b"GIF89a"), Err(LocalValidation::AttachmentType));
        let mut big = b"%PDF-".to_vec();
        big.resize(ATTACHMENT_MAX_BYTES, b' ');
        assert_eq!(check_file(&big), Ok(FileKind::Pdf), "exactly 10 MB is fine");
        big.push(b' ');
        assert_eq!(check_file(&big), Err(LocalValidation::AttachmentTooLarge));
    }

    #[test]
    fn names_are_made_safe_for_the_header() {
        assert_eq!(safe_name("a\"b\r\n.pdf", FileKind::Pdf), "ab.pdf");
        assert_eq!(
            safe_name("C:\\Users\\me\\scan.png", FileKind::Png),
            "scan.png"
        );
        assert_eq!(safe_name("../../etc/x.jpg", FileKind::Jpeg), "x.jpg");
        assert_eq!(safe_name("  ", FileKind::Jpeg), "attachment.jpg");
        assert_eq!(
            safe_name(&"é".repeat(300), FileKind::Pdf).chars().count(),
            200
        );
    }

    #[tokio::test]
    async fn upload_sends_multipart_with_the_sniffed_type() {
        let server = MockServer::start().await;
        let draft = DraftKey::new();
        Mock::given(method("POST"))
            .and(path("/api/v1/attachments"))
            .respond_with(json(201, &reply("draft", &draft.to_string())))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let a = s
            .attachments()
            .upload_draft(draft, "Receipt \"Oct\".pdf", PDF, IdempotencyKey::new())
            .await
            .expect("upload");
        assert_eq!(a.id, AttachmentId::parse(ATT).expect("id"));
        assert_eq!(a.attachable_type, "draft");
        let shown = format!("{a:?}");
        assert!(
            !shown.contains("receipt") && !shown.contains("secret"),
            "{shown}"
        );

        let posts = requests(&server, "POST").await;
        let ct = posts[0]
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .expect("content type")
            .to_string();
        let boundary = ct
            .strip_prefix("multipart/form-data; boundary=")
            .expect("multipart");
        let body = String::from_utf8_lossy(&posts[0].body).into_owned();
        assert!(body.starts_with(&format!("--{boundary}\r\n")));
        assert!(body.contains("name=\"attachableType\"\r\n\r\ndraft\r\n"));
        assert!(body.contains(&format!("name=\"attachableId\"\r\n\r\n{draft}\r\n")));
        assert!(body.contains("name=\"file\"; filename=\"Receipt Oct.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\n%canary\n\r\n"));
        assert!(body.ends_with(&format!("\r\n--{boundary}--\r\n")));
        assert!(posts[0].headers.get("idempotency-key").is_some());
    }

    #[tokio::test]
    async fn refused_files_send_nothing() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let check = TransactionId::parse(CHECK).expect("id");
        let mut big = b"%PDF-".to_vec();
        big.resize(ATTACHMENT_MAX_BYTES + 1, b' ');
        let e = s
            .attachments()
            .attach(check, "big.pdf", &big, IdempotencyKey::new())
            .await
            .expect_err("too big");
        assert_eq!(e, ApiError::Local(LocalValidation::AttachmentTooLarge));
        let e = s
            .attachments()
            .attach(check, "x.exe", b"MZ\x90\0", IdempotencyKey::new())
            .await
            .expect_err("type");
        assert_eq!(e, ApiError::Local(LocalValidation::AttachmentType));
        assert!(server.received_requests().await.expect("r").is_empty());
    }

    #[tokio::test]
    async fn list_and_download_an_expense_file() {
        let server = MockServer::start().await;
        let one = reply("expense", CHECK);
        let row = one
            .strip_prefix(r#"{"attachment":"#)
            .and_then(|r| r.strip_suffix('}'))
            .expect("row");
        Mock::given(method("GET"))
            .and(path("/api/v1/attachments"))
            .respond_with(json(200, &format!(r#"{{"data":[{row}],"total":1}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/attachments/{ATT}/download")))
            .respond_with(ResponseTemplate::new(200).set_body_raw(PDF.to_vec(), "application/pdf"))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let check = TransactionId::parse(CHECK).expect("id");
        let page = s
            .attachments()
            .list(
                AttachTo::Expense(check),
                PageRequest::new(50, 0).expect("p"),
            )
            .await
            .expect("list");
        assert_eq!(page.total, 1);
        assert_eq!(page.data[0].file_name, "receipt.pdf");
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            format!("attachable_type=expense&attachable_id={CHECK}&limit=50&offset=0")
        );
        let bytes = s
            .attachments()
            .download(page.data[0].id)
            .await
            .expect("download");
        assert_eq!(bytes, PDF);
    }
}
