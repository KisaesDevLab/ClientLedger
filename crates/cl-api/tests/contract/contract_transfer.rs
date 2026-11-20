//! P32 (U-282 to U-284, pin `dd11f2b4`): the verification totals preview,
//! a transfer export (the `options.transfer` echo, the totals in the
//! manifest, format 2 on the envelope), and the desktop refusing to import
//! a transfer file (`TRANSFER_NOT_A_BACKUP`).

use std::time::{Duration, Instant};

use cl_api::{ArchiveRequest, Capabilities, ImportRequest, JobState, Session};
use cl_core::Secret;
use serde_json::json;

use crate::support::{Harness, unique};

fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let st = h.run(s.tenants().archive_job(job)).expect("job");
        match st.status {
            JobState::Succeeded | JobState::Failed => return st,
            JobState::Queued | JobState::Running => {
                assert!(Instant::now() < deadline, "job did not finish");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// The engine's local date (never after its `CURRENT_DATE` + 1).
fn today() -> cl_core::CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    cl_core::CalendarDate::from_ymd(1970, 1, 1)
        .and_then(|d| d.add_days(i64::try_from(days).expect("days")))
        .expect("today")
}

pub fn transfer_export_preview_and_refusal_u284(h: &Harness) {
    if !h.require_cap(Capabilities::TENANT_ARCHIVE_TRANSFER, "U-284")
        || !h.require_cap(Capabilities::ARCHIVE_VERIFICATION_TOTALS, "U-283")
        || !h.require_cap(Capabilities::TENANT_ARCHIVE_V2, "U-282")
    {
        return;
    }
    let s = h.s3();
    let co = h.tenant_b();
    let as_of = today();
    let preview = h
        .run(s.tenants().verification_totals(co, as_of, true))
        .expect("preview");
    assert_eq!(preview.definition, cl_api::TOTALS_DEFINITION);
    assert_eq!(preview.as_of, as_of);
    assert!(!preview.companies.is_empty(), "the company's money");
    assert_eq!(
        preview.total_assets.as_decimal(),
        preview.total_liabilities.as_decimal() + preview.total_equity.as_decimal(),
        "A = L + E exactly"
    );
    h.run(s.close_company()).expect("workspace");
    let passphrase = Secret::new(format!("contract-{}", unique("transfer")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: true,
                omit_document_drafts: true,
                transfer: true,
                as_of: Some(as_of),
                host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "transfer", "name": "x"}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("transfer job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let result = st.export_result().expect("export result");
    assert!(result.transfer_honoured(), "S3: options.transfer echoed");
    let totals = result.verification_totals().expect("totals in the file");
    assert_eq!(totals.as_of, as_of);
    assert_eq!(totals.total_assets, preview.total_assets);
    assert_eq!(totals.counts.transactions, preview.counts.transactions);
    // `manifestSummary` has no `omitted` list (the manifest does; the
    // engine's own suites check it).
    let path = result.path.clone().expect("path");
    // Format 2 on the plaintext envelope.
    let bytes = std::fs::read(&path).expect("read");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"formatVersion\":2"), "format 2");
    // The desktop engine refuses to import a transfer file.
    let renamed = path.with_extension("clbackup");
    std::fs::copy(&path, &renamed).expect("copy");
    let job = h
        .run(s.tenants().import_archive(&ImportRequest {
            path: renamed.clone(),
            passphrase,
            scratch: false,
            reset_document_drafts: false,
        }))
        .expect("import job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Failed);
    assert_eq!(st.error_code(), "TRANSFER_NOT_A_BACKUP");
    let _ = std::fs::remove_file(&renamed);
    let _ = std::fs::remove_file(&path);
}
