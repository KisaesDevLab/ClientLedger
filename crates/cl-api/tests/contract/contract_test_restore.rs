//! P34 (U-296, pin `f52b64ae`): a test restore of a fresh backup passes
//! with the trial balance equal and creates no company; a wrong passphrase
//! fails the job.

use std::time::{Duration, Instant};

use cl_api::{ArchiveRequest, Capabilities, JobState, Session, TestRestoreRequest};
use cl_core::Secret;
use serde_json::json;

use crate::support::{Harness, unique};

fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(300);
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

pub fn test_restore_passes_and_leaves_no_company_u296(h: &Harness) {
    if !h.require_cap(Capabilities::TENANT_ARCHIVE_TEST_RESTORE, "U-296") {
        return;
    }
    let s = h.s3();
    let co = h.tenant_b();
    h.run(s.close_company()).expect("workspace");
    let passphrase = Secret::new(format!("contract-{}", unique("testrestore")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: false,
                transfer: false,
                as_of: None,
                host: json!({"app": "ClientLedger", "kind": "backup"}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("backup job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let path = st.export_result().expect("result").path.expect("path");
    let before = h.run(s.tenants().list()).expect("tenants").len();

    let job = h
        .run(s.tenants().test_restore(&TestRestoreRequest {
            path: path.clone(),
            passphrase: passphrase.clone(),
        }))
        .expect("test restore job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    assert_eq!(st.kind.as_deref(), Some("test-restore"));
    let r = st
        .test_restore_result()
        .expect("readable")
        .expect("present");
    assert!(r.passed, "{:?}", r.failures);
    assert_eq!(r.source_tenant_id, Some(co));
    let tb = r.trial_balance.as_ref().expect("trial balance compared");
    assert!(tb.rows_equal && tb.totals_equal);
    assert_eq!(tb.expected.debit_exact, tb.actual.debit_exact);
    assert!(tb.difference_exact.debit.is_zero() && tb.difference_exact.credit.is_zero());
    assert_eq!(
        r.verification_totals.as_ref().and_then(|v| v.matches),
        Some(true)
    );
    assert_eq!(
        h.run(s.tenants().list()).expect("tenants").len(),
        before,
        "no company was created"
    );

    let job = h
        .run(s.tenants().test_restore(&TestRestoreRequest {
            path,
            passphrase: Secret::new("a wrong passphrase 2026"),
        }))
        .expect("job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Failed);
    assert_eq!(st.error_code(), "ARCHIVE_PASSPHRASE_INVALID");
}
