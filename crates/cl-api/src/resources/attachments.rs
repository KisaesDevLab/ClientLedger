//! Files attached to a check (P13 WP2), a bill, a vendor credit or another
//! transaction (P17 WP2): upload, list, download, details, link, delete
//! and the unlinked files (`/api/v1/attachments`).
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
use crate::resources::operations::{
    ATTACHMENT_DELETE, ATTACHMENT_DOWNLOAD, ATTACHMENT_GET, ATTACHMENT_LINK, ATTACHMENTS_LIST,
    ATTACHMENTS_UNLINKED, ATTACHMENTS_UPLOAD,
};
use crate::resources::reconcile::{OutcomeMatcher, Probe};
use crate::resources::transactions::TxnType;
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
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
    /// A form that is not saved yet.
    Draft(DraftKey),
    /// A saved bill (attachable type `bill`, as the engine's own
    /// attach-draft uses).
    Bill(TransactionId),
    /// A saved vendor credit (`vendor_credit`).
    VendorCredit(TransactionId),
    /// Any other saved transaction, under its transaction type (a deposit,
    /// a journal entry, …).
    Txn(TxnType, TransactionId),
}

impl AttachTo {
    /// The engine's attachable type (`expense`, `draft`, `bill`,
    /// `vendor_credit`, or the transaction type).
    #[must_use]
    pub const fn attachable_type(self) -> &'static str {
        match self {
            AttachTo::Expense(_) => "expense",
            AttachTo::Draft(_) => "draft",
            AttachTo::Bill(_) => "bill",
            AttachTo::VendorCredit(_) => "vendor_credit",
            AttachTo::Txn(t, _) => match t {
                TxnType::Invoice => "invoice",
                TxnType::CustomerPayment => "customer_payment",
                TxnType::CashSale => "cash_sale",
                TxnType::Expense => "expense",
                TxnType::Deposit => "deposit",
                TxnType::Transfer => "transfer",
                TxnType::JournalEntry => "journal_entry",
                TxnType::CreditMemo => "credit_memo",
                TxnType::CustomerRefund => "customer_refund",
                TxnType::Bill => "bill",
                TxnType::VendorCredit => "vendor_credit",
                TxnType::BillPayment => "bill_payment",
                TxnType::DailySales => "daily_sales",
                TxnType::Aje => "aje",
            },
        }
    }

    /// The record (or draft) id it names.
    #[must_use]
    pub fn attachable_id(self) -> String {
        self.id()
    }

    fn kind(self) -> String {
        self.attachable_type().to_string()
    }

    fn id(self) -> String {
        match self {
            AttachTo::Expense(id)
            | AttachTo::Bill(id)
            | AttachTo::VendorCredit(id)
            | AttachTo::Txn(_, id) => id.to_string(),
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
    for (field, value) in [("attachableType", to.kind()), ("attachableId", to.id())] {
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
            ("attachable_type".into(), of.kind()),
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

    /// One file's details.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: AttachmentId) -> Result<Attachment, ApiError> {
        #[derive(Deserialize)]
        struct W {
            attachment: Attachment,
        }
        let call = Call::new(Method::Get, ATTACHMENT_GET.route(), format!("{BASE}/{id}"));
        let reply = self.0.send(&ATTACHMENT_GET, call, None).await?;
        decode::<W>(&reply, "attachment").map(|w| w.attachment)
    }

    /// Deletes a file. A lost reply is reconciled by reading the file back
    /// (gone = deleted).
    ///
    /// # Errors
    /// `NotFound`; `OutcomeUnknown`; any [`ApiError`].
    pub async fn delete(&self, id: AttachmentId, key: IdempotencyKey) -> Result<(), ApiError> {
        let s = &self.0.pinned();
        let path = format!("{BASE}/{id}");
        let matcher = OutcomeMatcher::Deleted {
            get: &ATTACHMENT_GET,
            path: path.clone(),
        };
        let probe = Probe::new(s, matcher, false).await;
        let mut call = Call::new(Method::Delete, ATTACHMENT_DELETE.route(), path);
        call.idempotency_key = Some(key);
        s.send(&ATTACHMENT_DELETE, call, Some(&probe)).await?;
        Ok(())
    }

    /// Moves a file onto a record or into a draft (`POST
    /// /attachments/{id}/link`; adopting an unlinked file into a draft, or
    /// attaching a vendor credit's files before `bill-lines-v1`). Linking
    /// again to the same place changes nothing.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn link(
        &self,
        id: AttachmentId,
        to: AttachTo,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let body = json_body(&serde_json::json!({
            "attachableType": to.kind(),
            "attachableId": to.id(),
        }))?;
        let mut call = Call::new(
            Method::Post,
            ATTACHMENT_LINK.route(),
            format!("{BASE}/{id}/link"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        self.0.send(&ATTACHMENT_LINK, call, None).await?;
        Ok(())
    }

    /// Files not linked to any record yet (drafts and receipts; the
    /// engine's newest 50).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn unlinked(&self) -> Result<Vec<Attachment>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<Attachment>,
        }
        let call = Call::at(Method::Get, ATTACHMENTS_UNLINKED.route());
        let reply = self.0.send(&ATTACHMENTS_UNLINKED, call, None).await?;
        decode::<W>(&reply, "unlinked attachments").map(|w| w.data)
    }
}

const BASE: &str = "/api/v1/attachments";

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

    #[tokio::test]
    async fn bills_credits_and_other_records_link_get_delete_and_unlinked() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/attachments/{ATT}/link")))
            .respond_with(json(200, r#"{"linked":true}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/attachments/{ATT}")))
            .respond_with(json(200, &reply("bill", CHECK)))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/v1/attachments/{ATT}")))
            .respond_with(json(200, r#"{"message":"Attachment deleted"}"#))
            .mount(&server)
            .await;
        let one = reply("draft", CHECK);
        let row = one
            .strip_prefix(r#"{"attachment":"#)
            .and_then(|r| r.strip_suffix('}'))
            .expect("row");
        Mock::given(method("GET"))
            .and(path("/api/v1/attachments/unlinked"))
            .respond_with(json(200, &format!(r#"{{"data":[{row}]}}"#)))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let id = AttachmentId::parse(ATT).expect("id");
        let txn = TransactionId::parse(CHECK).expect("id");
        for to in [
            AttachTo::Bill(txn),
            AttachTo::VendorCredit(txn),
            AttachTo::Txn(TxnType::Deposit, txn),
            AttachTo::Draft(DraftKey::new()),
        ] {
            s.attachments()
                .link(id, to, IdempotencyKey::new())
                .await
                .expect("link");
        }
        let kinds: Vec<String> = requests(&server, "POST")
            .await
            .iter()
            .map(|r| {
                let v: serde_json::Value = serde_json::from_slice(&r.body).expect("json");
                v["attachableType"].as_str().unwrap_or("").to_string()
            })
            .collect();
        assert_eq!(kinds, ["bill", "vendor_credit", "deposit", "draft"]);
        for t in [
            TxnType::Invoice,
            TxnType::CustomerPayment,
            TxnType::CashSale,
            TxnType::Expense,
            TxnType::Deposit,
            TxnType::Transfer,
            TxnType::JournalEntry,
            TxnType::CreditMemo,
            TxnType::CustomerRefund,
            TxnType::Bill,
            TxnType::VendorCredit,
            TxnType::BillPayment,
            TxnType::DailySales,
            TxnType::Aje,
        ] {
            assert_eq!(AttachTo::Txn(t, txn).attachable_type(), t.as_str());
        }
        let a = s.attachments().get(id).await.expect("get");
        assert_eq!(a.attachable_type, "bill");
        assert_eq!(s.attachments().unlinked().await.expect("unlinked").len(), 1);
        s.attachments()
            .delete(id, IdempotencyKey::new())
            .await
            .expect("delete");
        assert_eq!(requests(&server, "DELETE").await.len(), 1);
    }
}
