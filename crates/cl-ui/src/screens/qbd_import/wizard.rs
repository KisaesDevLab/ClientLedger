//! The wizard's requests: files in, the batch, the backup before every
//! commit and undo, the commit and undo themselves (one key per exact
//! body, never sent again on their own), the batch read while its job
//! runs, Check status, the closing-date password, the tie-out and verify.

use cl_api::imports::{
    AccountTarget, BatchKind, BatchOptions, BatchState, BatchStatus, CommitRequest, ContactKind,
    FileRole, ImportBatch, ImportBatchId, ImportKind, ImportRefusal, JobAccepted, NewBatch,
    SourceSystem, UploadOutcome, UploadRequest,
};
use cl_api::{ApiError, ClosedPeriodAuthorization, IdempotencyKey};
use cl_model::imports::wizard::{FileSlot, Step};

use super::{Held, ImportScreen, Job, Locked, Running, SlotFile, Trouble, poll, read_import_file};
use crate::screens::write_check::attach::{poll_file_read, read_in_background};

impl ImportScreen {
    // ---- Files -----------------------------------------------------------

    /// The slot named `key` of the current source.
    pub(crate) fn slot(&self, key: &str) -> Option<&'static FileSlot> {
        self.wizard.source.slots().iter().find(|s| s.key == key)
    }

    /// "Add file" on a card: the system's file window.
    pub(crate) fn browse(&mut self, ctx: &eframe::egui::Context, slot: &'static str) {
        use crate::widgets::file_picker::{IMPORT_FILES, PickUse, pick_files};
        let many = self.slot(slot).is_some_and(|s| s.multiple);
        self.pick_slot = Some(slot);
        self.pick_rx = Some(pick_files(ctx, PickUse::ImportFile, &[IMPORT_FILES], many));
    }

    /// A file dropped on (or chosen for) a card: read off the egui thread.
    pub(crate) fn take_path(&mut self, slot: &'static str, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut f = SlotFile::new(name);
        match read_in_background(path, read_import_file) {
            Ok(rx) => f.reading = Some(rx),
            Err(k) => f.problem = Some(k),
        }
        self.add_file(slot, f);
    }

    /// Bytes for a card (a dropped file's bytes, the demo's fixtures).
    pub(crate) fn take_file(&mut self, slot: &'static str, name: String, bytes: Vec<u8>) {
        let mut f = SlotFile::new(name);
        self.upload(slot, &mut f, bytes);
        self.add_file(slot, f);
    }

    fn add_file(&mut self, slot: &'static str, f: SlotFile) {
        let multiple = self.slot(slot).is_some_and(|s| s.multiple);
        let old: Vec<SlotFile> = if multiple {
            Vec::new()
        } else {
            self.uploads.remove(slot).unwrap_or_default()
        };
        for o in old {
            if let Some(id) = o.session()
                && self.batch.is_none()
            {
                self.discard(id);
            }
        }
        self.uploads.entry(slot).or_default().push(f);
        self.trouble = None;
        self.sync_files();
    }

    /// Removes one file from a card (its upload is discarded).
    pub(crate) fn remove_file(&mut self, slot: &'static str, i: usize) {
        if self.batch.is_some() || self.locked() {
            return;
        }
        let Some(list) = self.uploads.get_mut(slot) else {
            return;
        };
        if i >= list.len() || list[i].busy() {
            return;
        }
        let f = list.remove(i);
        if let Some(id) = f.session() {
            self.discard(id);
        }
        self.sync_files();
    }

    /// The upload of one file as its card's report (the engine's header
    /// check for that report).
    fn upload(&self, slot: &'static str, f: &mut SlotFile, bytes: Vec<u8>) {
        let Some(s) = self.slot(slot) else {
            return;
        };
        let (Some(kind), Some(source), Some(role)) = (
            ImportKind::from_wire(s.kind),
            SourceSystem::from_wire(self.wizard.source.engine_source()),
            FileRole::from_wire(s.role.as_wire()),
        ) else {
            f.problem = Some("imports.file.unreadable");
            return;
        };
        // Sources whose files carry no date get the wizard's (S5).
        let tb_report_date = (self.wizard.source.needs_tb_date()
            && kind == ImportKind::TrialBalance)
            .then(|| self.wizard.expected_date(s))
            .flatten();
        let req = UploadRequest {
            kind,
            source,
            role,
            bytes,
            file_name: f.name.clone(),
            contact_kind: s.contact_kind.and_then(ContactKind::from_wire),
            tb_column: None,
            tb_report_date,
            // Q-P31ac: a QBD switch (always into a new company) uses the
            // QuickBooks chart: a starter account with the same number
            // takes the QuickBooks name and type (Review matches shows each
            // one for the owner's OK; undo puts the old ones back).
            update_existing_coa: self.wizard.source == cl_model::imports::wizard::Source::Qbd
                && kind == ImportKind::Coa,
        };
        if let Err(e) = req.check() {
            f.problem = Some(match e {
                cl_api::LocalValidation::AttachmentTooLarge => "imports.file.too_large",
                cl_api::LocalValidation::AttachmentType => "imports.file.type",
                _ => "imports.file.unreadable",
            });
            return;
        }
        f.bytes = Some(std::sync::Arc::new(req.bytes.clone()));
        f.job = Some(self.write(
            "orphan.import_upload",
            None,
            {
                let req = req.clone();
                move |d| d.imports.upload(&req)
            },
            move |s| async move { s.imports().upload(&req).await },
        ));
    }

    pub(crate) fn poll_files(&mut self) {
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.pick_rx)
            && let Some(slot) = self.pick_slot.take()
        {
            for p in paths {
                self.take_path(slot, &p);
            }
        }
        let mut changed = false;
        let keys: Vec<&'static str> = self.uploads.keys().copied().collect();
        for slot in keys {
            let n = self.uploads.get(slot).map_or(0, Vec::len);
            for i in 0..n {
                let read = self
                    .uploads
                    .get_mut(slot)
                    .and_then(|v| poll_file_read(&mut v[i].reading));
                if let Some(r) = read {
                    changed = true;
                    let mut f = SlotFile::new(String::new());
                    if let Some(v) = self.uploads.get_mut(slot) {
                        std::mem::swap(&mut f, &mut v[i]);
                    }
                    match r {
                        Ok((name, bytes)) => {
                            f.name = name;
                            self.upload(slot, &mut f, bytes);
                        }
                        Err(k) => f.problem = Some(k),
                    }
                    if let Some(v) = self.uploads.get_mut(slot) {
                        std::mem::swap(&mut f, &mut v[i]);
                    }
                }
                let answer = self.uploads.get_mut(slot).and_then(|v| poll(&mut v[i].job));
                if let Some(r) = answer {
                    changed = true;
                    self.after_upload(slot, i, r);
                }
            }
        }
        if changed {
            self.sync_files();
        }
    }

    fn after_upload(&mut self, slot: &'static str, i: usize, r: Result<UploadOutcome, ApiError>) {
        let Some(f) = self.uploads.get_mut(slot).and_then(|v| v.get_mut(i)) else {
            return;
        };
        match r {
            Ok(o) => {
                tracing::info!(target: "imports", slot, rows = o.session.row_count, problems = o.problems.len(), "file read");
                f.outcome = Some(o);
            }
            Err(e) => {
                tracing::info!(target: "imports", slot, code = Trouble::of(&e).code(), "file refused");
                match import_refusal_of(&e) {
                    Some(r) => f.refusal = Some(r),
                    None => {
                        f.problem = Some(match e {
                            ApiError::Validation(_) => "imports.file.wrong_report",
                            ApiError::Local(cl_api::LocalValidation::Unsupported) => {
                                "imports.file.unsupported"
                            }
                            ApiError::EngineUnavailable { .. } => "imports.file.engine_down",
                            _ => "imports.file.unreadable",
                        });
                    }
                }
            }
        }
    }

    // ---- Import: batch, backup, commit -------------------------------------

    /// Import: the batch (once), a verified backup, then the commit. Every
    /// way in (Import, the closing-date password, Retry after a failure)
    /// comes here, so every commit has a backup just before it.
    pub(crate) fn start_import(&mut self) {
        if self.locked() || self.wizard.imported {
            return;
        }
        // QA-2: before the batch is made, so a missing passphrase leaves
        // nothing behind.
        if !self.passphrase_ready() {
            return;
        }
        self.trouble = None;
        self.failure = None;
        self.status_answer = None;
        match self.batch {
            Some(id) => self.start_backup(Running::Commit, id),
            None => self.create_batch(),
        }
    }

    fn create_batch(&mut self) {
        let Some(source) = SourceSystem::from_wire(self.wizard.source.engine_source()) else {
            return;
        };
        let sessions = self
            .uploads
            .values()
            .flatten()
            .filter_map(SlotFile::session)
            .collect();
        let dates = self.wizard.dates().ok();
        let options = BatchOptions {
            history_from: dates.and_then(|d| d.history_from),
            // E2c: the switch date the reference reports must carry (a QBD
            // conversion; other programs' files carry the owner's dates).
            conversion_date: self
                .wizard
                .source
                .is_conversion()
                .then_some(self.wizard.conversion_date)
                .flatten(),
            require_empty_company: self.wizard.source.requires_empty_company(),
            account_map: self
                .account_map
                .iter()
                .map(|(k, a)| (k.clone(), AccountTarget::Account(*a)))
                .collect(),
            ..BatchOptions::default()
        };
        let new = NewBatch {
            source,
            kind: BatchKind::Conversion,
            sessions,
            options,
        };
        self.batch_create_job = Some(self.write(
            "orphan.import_batch",
            None,
            {
                let new = new.clone();
                move |d| d.imports.create_batch(&new)
            },
            move |s| async move { s.imports().create_batch(&new).await },
        ));
    }

    pub(crate) fn after_batch_created(&mut self, r: Result<ImportBatch, ApiError>) {
        match r {
            Ok(b) => {
                tracing::info!(target: "imports", warnings = b.date_check.as_ref().map_or(0, |d| d.warnings.len()), "import batch made");
                self.batch = Some(b.id);
                self.date_check = b.date_check;
                self.start_backup(Running::Commit, b.id);
            }
            // Refused at create (dates, a picked account gone): no batch
            // exists and the files are free; Back fixes it.
            Err(e) => self.refused(&e),
        }
    }

    /// "Import again" after a failure the batch can't recover from (an
    /// account with no match, a missing valuation): its files are deleted
    /// and go up again, then the wizard returns to the step that fixes it.
    /// The next Import makes a new batch, a new backup and a new key.
    pub(crate) fn rebuild_after_failure(&mut self) {
        let Some(step) = self.failure.as_ref().and_then(super::rebuild_step) else {
            return;
        };
        if self.locked() || self.rebuild.is_some() {
            return;
        }
        if let Some(ImportRefusal::UnmappedAccount { keys, .. }) = &self.failure {
            self.unmapped_failed.extend(keys.0.iter().cloned());
        }
        tracing::info!(target: "imports", step = ?step, "import rebuilt after a failure");
        let ids: Vec<_> = self
            .uploads
            .values()
            .flatten()
            .filter_map(SlotFile::session)
            .collect();
        let jobs = ids
            .into_iter()
            .map(|id| {
                self.write(
                    "orphan.import_discard",
                    None,
                    move |d| d.imports.discard(id),
                    move |s| async move { s.imports().discard(id).await },
                )
            })
            .collect();
        self.rebuild = Some((jobs, step));
        self.batch = None;
        self.state = None;
        self.held = None;
        self.trouble = None;
        self.status_answer = None;
        self.date_check = None;
    }

    /// The deletes of a rebuild: once every one answered (a file already
    /// gone counts), the kept bytes go up again as new files.
    pub(crate) fn poll_rebuild(&mut self) {
        let Some((jobs, _)) = &mut self.rebuild else {
            return;
        };
        jobs.retain_mut(|j| j.take().is_none());
        if !jobs.is_empty() {
            return;
        }
        let Some((_, step)) = self.rebuild.take() else {
            return;
        };
        let slots: Vec<&'static str> = self.uploads.keys().copied().collect();
        for slot in slots {
            let old = self.uploads.remove(slot).unwrap_or_default();
            let mut again = Vec::new();
            for o in old {
                let mut f = SlotFile::new(o.name);
                match o.bytes {
                    Some(b) => self.upload(slot, &mut f, b.as_ref().clone()),
                    None => f.problem = Some("imports.file.add_again"),
                }
                again.push(f);
            }
            self.uploads.insert(slot, again);
        }
        self.failure = None;
        self.wizard.step = step;
        self.sync_files();
    }

    /// The verified backup before a commit or undo, off the egui thread.
    /// Anything that keeps it from starting is a failed backup: nothing is
    /// sent without one.
    pub(crate) fn start_backup(&mut self, what: Running, batch: ImportBatchId) {
        self.backup_error = None;
        // QA-2: no passphrase, no backup: the boxes say why (nothing sent).
        if !self.passphrase_ready() {
            self.trouble = Some(Trouble::BackupFailed);
            return;
        }
        self.waiting = Some((what, batch));
        if let Some(mut d) = self.link.demo_state() {
            let r = d.imports.backup().map(|r| {
                Ok(r.map_err(|()| cl_company::CompanyError::JobFailed {
                    code: "DEMO".into(),
                }))
            });
            self.backup_job = Some(Job::Done(r));
            return;
        }
        let unavailable =
            || cl_company::CompanyError::Api(ApiError::EngineUnavailable { permanent: false });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.backup_failed(unavailable());
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let default_dir = self.link.backup_dir.clone();
        let kind = super::backup_kind();
        let task = crate::company::spawn_lifecycle(&self.link, group, move || async move {
            let cl_api::Scope::Company { tenant_id, .. } = session.scope() else {
                return Err(cl_company::CompanyError::UnknownCompany);
            };
            let dest_dir = store
                .load()
                .ok()
                .and_then(|l| l.backup_folder)
                .unwrap_or(default_dir);
            let protected = super::protected_backups(&session).await;
            cl_company::backup(
                &session,
                &store,
                secrets.as_ref(),
                &cl_company::BackupRequest {
                    tenant_id,
                    kind,
                    dest_dir,
                    // See `backup_kind`: the before-import kind's own rule,
                    // never deleting a backup an import still needs.
                    apply_retention: super::BACKUP_RETENTION,
                    protected,
                },
                &cl_company::backup::silent(),
            )
            .await
            .map(|o| {
                o.record
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
        });
        match task {
            Ok(t) => self.backup_job = Some(Job::Running(t)),
            Err(_) => self.backup_failed(unavailable()),
        }
    }

    fn backup_failed(&mut self, e: cl_company::CompanyError) {
        tracing::warn!(target: "imports", detail = %cl_core::OwnerFacing::scrubbed_detail(&e), "backup before the import failed; nothing sent");
        self.waiting = None;
        self.password = None;
        self.confirm_closed_next = false;
        self.trouble = Some(Trouble::BackupFailed);
        // QA-2: the real cause is shown; no passphrase brings the boxes back.
        if matches!(e, cl_company::CompanyError::NoPassphrase) {
            self.need_passphrase = true;
        }
        self.backup_error = Some(e);
    }

    /// QA-2: a backup passphrase is set, or the one typed in the Import
    /// step's boxes is saved now (as Back Up saves it).
    pub(crate) fn passphrase_ready(&mut self) -> bool {
        if !self.need_passphrase {
            return true;
        }
        if self.passphrase.save(&self.link) {
            self.need_passphrase = false;
            return true;
        }
        false
    }

    pub(crate) fn after_backup(&mut self, r: Result<String, cl_company::CompanyError>) {
        match (r, self.waiting.take()) {
            (Ok(name), Some((what, batch))) if !name.trim().is_empty() => {
                tracing::info!(target: "imports", "backup made before the import");
                let held = Held {
                    what,
                    batch,
                    backup_ref: name,
                    password: self.password.take(),
                    confirm_closed: what == Running::Commit
                        && std::mem::take(&mut self.confirm_closed_next),
                    key: IdempotencyKey::new(),
                };
                if self.close_asked {
                    // QA-20: the close prompt is up; nothing is sent until
                    // Keep (Close drops the window and this with it).
                    tracing::info!(target: "imports", "send held while the close prompt is up");
                    self.parked = Some(held);
                } else {
                    self.send(held);
                }
            }
            (Ok(_), Some(_)) => self.backup_failed(cl_company::CompanyError::JobFailed {
                code: "NO_PATH".into(),
            }),
            (Ok(_), None) => {}
            (Err(e), _) => self.backup_failed(e),
        }
    }

    /// Sends `held` (the exact body and key); "Try again" sends it again.
    pub(crate) fn send(&mut self, held: Held) {
        let (id, backup, key) = (held.batch, held.backup_ref.clone(), held.key.clone());
        let password = held.password.clone();
        let confirm = held.confirm_closed;
        let what = held.what;
        self.last_key = Some(key.clone());
        self.held = Some(held);
        self.jobs_sent += 1;
        self.unsettled = false;
        self.trouble = None;
        let label = match what {
            Running::Commit => "orphan.import_commit",
            Running::Undo => "orphan.import_undo",
        };
        self.job_out = Some(self.write(
            label,
            Some(key.clone()),
            {
                let (backup, key, password) = (backup.clone(), key.clone(), password.clone());
                move |d| {
                    let closing = ImportScreen::demo_closing(d);
                    match what {
                        Running::Commit => d.imports.commit(
                            id,
                            &CommitRequest {
                                backup_ref: backup,
                                confirm_closed_period: confirm,
                                ..CommitRequest::default()
                            },
                            &key,
                            password.as_ref(),
                            closing,
                        ),
                        Running::Undo => {
                            d.imports
                                .undo(id, &backup, &key, password.as_ref(), closing)
                        }
                    }
                }
            },
            move |s| async move {
                let auth = password.map(ClosedPeriodAuthorization::with_password);
                match what {
                    Running::Commit => {
                        let req = CommitRequest {
                            backup_ref: backup,
                            closed_period: auth,
                            confirm_closed_period: confirm,
                        };
                        s.imports().commit(id, &req, key).await
                    }
                    Running::Undo => s.imports().undo(id, &backup, auth, key).await,
                }
            },
        ));
    }

    pub(crate) fn after_job_sent(&mut self, r: Result<JobAccepted, ApiError>) {
        let what = self.held.as_ref().map_or(Running::Commit, |h| h.what);
        match r {
            Ok(_) => {
                tracing::info!(target: "imports", what = ?what, "import job accepted");
                self.closing.saved();
                if let Some(h) = &mut self.held {
                    // Sent and taken: the password is not kept.
                    h.password = None;
                }
                self.running = Some(what);
                self.read_batch();
            }
            // Lost: it may have gone through. Check status or Try again
            // (the same body and key); nothing is sent on its own.
            Err(ApiError::OutcomeUnknown { .. }) => {
                self.unsettled = true;
                self.trouble = Some(Trouble::Unknown);
                self.closing.nothing_sent();
            }
            Err(e) => {
                self.held = None;
                self.refused(&e);
            }
        }
    }

    /// A refusal of create, commit or undo: the closing-date password
    /// prompt takes closed periods; the rest are shown in owner words.
    fn refused(&mut self, e: &ApiError) {
        let locked = match (self.held_what(), self.undo_pending) {
            (Running::Undo, Some(id)) => Locked::Undo(id),
            _ => Locked::Commit,
        };
        let closing = self
            .link
            .demo_state()
            .and_then(|d| ImportScreen::demo_closing(&d));
        // No closing-date password is set: the engine lets a commit in
        // once the owner confirms (plan §2a E1, `confirmClosedPeriod`).
        // Asked every time; an undo has no such path (the dialog explains).
        if let (ApiError::ClosedPeriod(c), Locked::Commit) = (e, locked)
            && c.can_override
            && c.password_required != Some(true)
        {
            tracing::info!(target: "imports", "closed period without a password: asking");
            self.confirm_closed_next = false;
            self.trouble = None;
            self.ask_closed = Some(super::ClosedAsk {
                date: c.lock_date.or(closing),
            });
            return;
        }
        if self.closing.failed(e, locked, true, closing) {
            // The button says what it does (UI review: not "Change").
            if matches!(locked, Locked::Undo(_)) {
                self.closing.set_ask("imports.closing.ask_undo", 1);
                self.closing
                    .set_verb(crate::forms::closing_prompt::Verb::UndoImport);
            } else {
                self.closing.set_ask("imports.closing.ask_import", 1);
                self.closing
                    .set_verb(crate::forms::closing_prompt::Verb::Import);
            }
            self.trouble = None;
            return;
        }
        tracing::info!(target: "imports", code = Trouble::of(e).code(), "import refused");
        self.trouble = Some(Trouble::of(e));
    }

    fn held_what(&self) -> Running {
        self.held
            .as_ref()
            .map(|h| h.what)
            .or_else(|| self.waiting.map(|w| w.0))
            .unwrap_or(if self.undo_pending.is_some() {
                Running::Undo
            } else {
                Running::Commit
            })
    }

    /// The closing-date password was given: a new backup and a new body.
    pub(crate) fn submit_password(&mut self) {
        let Some((locked, secret)) = self.closing.submit() else {
            return;
        };
        self.password = Some(secret);
        self.held = None;
        match locked {
            Locked::Commit => {
                if let Some(id) = self.batch {
                    self.start_backup(Running::Commit, id);
                } else {
                    self.closing.nothing_sent();
                }
            }
            Locked::Undo(id) => {
                self.undo_pending = Some(id);
                self.start_backup(Running::Undo, id);
            }
        }
    }

    /// "Import into the closed period": a new backup, then the commit
    /// with the owner's confirmation (a new key: a new body).
    pub(crate) fn confirm_closed(&mut self) {
        if self.ask_closed.take().is_none() || self.locked() {
            return;
        }
        let Some(id) = self.batch else {
            return;
        };
        self.held = None;
        self.confirm_closed_next = true;
        self.start_backup(Running::Commit, id);
    }

    /// Cancel on the closed-period question: nothing is sent.
    pub(crate) fn cancel_closed(&mut self) {
        self.ask_closed = None;
        self.confirm_closed_next = false;
    }

    pub(crate) fn cancel_password(&mut self) {
        self.closing.cancel();
        self.password = None;
        self.undo_pending = None;
    }

    /// "Try again" after a lost answer: the same body and key, on the
    /// owner's click only.
    pub(crate) fn try_again(&mut self) {
        if !self.unsettled || self.job_out.is_some() {
            return;
        }
        if let Some(h) = self.held.clone() {
            self.send(h);
        }
    }

    /// "Check status": reads the batch; its status settles a lost answer.
    pub(crate) fn check_status(&mut self) {
        if self.batch_job.is_some() {
            return;
        }
        self.read_batch();
    }

    pub(crate) fn read_batch(&mut self) {
        let Some(id) = self
            .held
            .as_ref()
            .map(|h| h.batch)
            .or(self.batch)
            .or(self.undo_pending)
        else {
            return;
        };
        self.batch_job = Some(self.read(
            move |d| d.imports.batch(id),
            move |s| async move { s.imports().batch(id).await },
        ));
    }

    pub(crate) fn after_batch_read(&mut self, r: Result<BatchState, ApiError>) {
        let st = match r {
            Ok(st) => st,
            Err(ApiError::NotFound { .. }) => {
                self.running = None;
                self.unsettled = false;
                self.trouble = Some(Trouble::Gone);
                return;
            }
            Err(e) => {
                // A read that failed changes nothing: the job may still
                // run (the next frame reads again).
                if self.running.is_none() {
                    self.trouble = Some(Trouble::of(&e));
                }
                return;
            }
        };
        let what = self.held_what();
        let status = st.batch.status;
        let failed = st.error.is_some() || status == BatchStatus::Failed;
        if st.batch.date_check.is_some() {
            self.date_check.clone_from(&st.batch.date_check);
        }
        self.state = Some(st);
        match (what, status) {
            (_, BatchStatus::Committing | BatchStatus::Undoing) => {
                // The job runs: a lost answer was taken after all.
                self.unsettled = false;
                if self.trouble == Some(Trouble::Unknown) {
                    self.trouble = None;
                }
                self.running = Some(what);
                self.status_answer = Some(true);
            }
            (Running::Commit, BatchStatus::Committed) => self.committed(),
            (Running::Undo, BatchStatus::Undone) => self.undone_now(),
            (Running::Commit, BatchStatus::Draft | BatchStatus::Failed) if self.unsettled => {
                // Not taken (yet): Try again (same key) stays offered.
                self.status_answer = Some(false);
                self.running = None;
            }
            (Running::Commit, BatchStatus::Failed) | (_, BatchStatus::Draft) if failed => {
                // The job failed: nothing was posted; the backup is kept.
                tracing::info!(target: "imports", code = self.state.as_ref().and_then(|s| s.error.as_ref()).map_or("none", |e| e.code.as_str()), "import job failed");
                self.running = None;
                self.held = None;
                self.failure = self.failure_refusal();
                self.trouble = Some(Trouble::Failed);
            }
            (Running::Undo, BatchStatus::Committed) if self.running.is_some() || self.unsettled => {
                // The undo ended without undoing (failed, or not taken).
                self.running = None;
                if self.unsettled && !failed {
                    self.status_answer = Some(false);
                } else {
                    // E1: a blocker found after the 202 lands as the
                    // batch's error (IMPORT_UNDO_BLOCKED); the restore is
                    // offered the same way.
                    self.unsettled = false;
                    self.held = None;
                    let refusal = self.failure_refusal();
                    self.trouble = Some(match refusal {
                        Some(r @ ImportRefusal::UndoBlocked { .. }) => Trouble::Refused(r),
                        _ => Trouble::Failed,
                    });
                }
            }
            _ => self.running = None,
        }
    }

    /// The last batch read's failure, when the screen explains its code.
    fn failure_refusal(&self) -> Option<ImportRefusal> {
        self.state
            .as_ref()
            .and_then(|s| s.error.as_ref())
            .and_then(cl_api::imports::BatchFailure::refusal)
    }

    fn committed(&mut self) {
        tracing::info!(target: "imports", source = self.wizard.source.key(), "import committed");
        // The accounts the import made (the spot-check's banks).
        self.load_chart();
        self.running = None;
        self.unsettled = false;
        self.held = None;
        self.trouble = None;
        self.status_answer = Some(true);
        self.wizard.imported = true;
        self.publish();
        if self.wizard.step == Step::Import {
            self.wizard.next(self.today);
            self.load_tieout();
        }
    }

    fn undone_now(&mut self) {
        tracing::info!(target: "imports", "import undone");
        self.running = None;
        self.unsettled = false;
        self.held = None;
        self.undo_pending = None;
        self.trouble = None;
        self.undone = true;
        self.publish();
        if self.view == super::View::History {
            self.load_history();
        }
    }

    /// The books changed: open registers, lists and Home read again.
    fn publish(&self) {
        let bus = self.link.bus();
        bus.publish(crate::data_bus::Change::Ledger(None));
        bus.publish(crate::data_bus::Change::Accounts);
        bus.publish(crate::data_bus::Change::Contacts);
        bus.publish(crate::data_bus::Change::Items);
    }

    // ---- Undo ------------------------------------------------------------

    /// "Undo this import" asks first.
    pub(crate) fn ask_undo(&mut self, id: ImportBatchId) {
        if self.allowed() && !self.locked() {
            self.confirm_undo = Some(id);
        }
    }

    /// The owner confirmed: a verified backup, then the undo.
    pub(crate) fn undo(&mut self) {
        let Some(id) = self.confirm_undo.take() else {
            return;
        };
        if self.locked() {
            return;
        }
        self.trouble = None;
        self.undo_pending = Some(id);
        self.held = None;
        self.start_backup(Running::Undo, id);
    }

    /// The batch an undo blocker belongs to, for the restore.
    pub(crate) fn blocked_backup(&self) -> Option<String> {
        let id = self.undo_pending.or(self.batch)?;
        self.history
            .iter()
            .flatten()
            .chain(self.state.as_ref().map(|s| &s.batch))
            .find(|b| b.id == id)
            .and_then(|b| b.backup_ref.clone())
    }

    /// Where the before-import backup `name` is on this computer, or `None`
    /// when it is gone (normal retention removes a verified batch's,
    /// Q-P31aa). Looked up once per name until History is read again.
    pub(crate) fn backup_path(&mut self, name: &str) -> Option<std::path::PathBuf> {
        if let Some(found) = self.backup_paths.get(name) {
            return found.clone();
        }
        let found = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| {
                l.companies
                    .iter()
                    .flat_map(|c| c.backups.iter())
                    .find(|b| {
                        b.path
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy() == name)
                    })
                    .map(|b| b.path.clone())
            })
            .or_else(|| Some(self.link.backup_dir.join(name)))
            // The demo's backups are simulated (no files).
            .filter(|p| self.link.is_demo() || p.is_file());
        self.backup_paths.insert(name.to_string(), found.clone());
        found
    }

    /// "Restore the backup taken before the import": Open Company's
    /// restore panel, with that file (it creates a new company). A file
    /// that is gone is said so in plain words (review S3), never handed on.
    pub(crate) fn restore_backup(&mut self, name: &str) {
        self.backup_paths.remove(name);
        let Some(path) = self.backup_path(name) else {
            self.restore_gone = true;
            return;
        };
        self.restore_gone = false;
        crate::screens::company::open::request_restore(path);
        self.link.push(crate::company::CompanyEvent::OpenWindow(
            cl_model::CommandId::new("file.open_company"),
        ));
    }

    // ---- History: Retry and the way back in ------------------------------

    /// The window's wizard on `b` (its program, its batch).
    fn take_batch(&mut self, b: &ImportBatch) {
        if self.locked() {
            return;
        }
        if let Some(source) = cl_model::imports::wizard::Source::ALL
            .into_iter()
            .find(|s| s.engine_source() == b.source.as_wire())
        {
            self.start_over();
            self.wizard.set_source(source);
        }
        self.batch = Some(b.id);
        self.view = super::View::Import;
    }

    /// Retry from History: a failed batch is imported again, a backup
    /// first and a new key (a new attempt, never the failed one's key).
    pub(crate) fn retry_batch(&mut self, b: &ImportBatch) {
        if !self.allowed() || b.status != BatchStatus::Failed || self.locked() {
            return;
        }
        // A batch whose options can't succeed (an account with no match, a
        // missing valuation) is not committed again: its files go in a new
        // import (the window has no bytes for them here).
        let failure = b
            .error
            .as_ref()
            .and_then(cl_api::imports::BatchFailure::refusal);
        if failure.as_ref().and_then(super::rebuild_step).is_some() {
            self.take_batch(b);
            self.batch = None;
            if let Some(ImportRefusal::UnmappedAccount { keys, .. }) = &failure {
                self.unmapped_failed.extend(keys.0.iter().cloned());
            }
            self.wizard.step = Step::Files;
            self.trouble = Some(Trouble::AddAgain);
            return;
        }
        self.take_batch(b);
        self.wizard.step = Step::Import;
        self.start_import();
    }

    /// "Check that it matches" from History: the batch's tie-out, or its
    /// spot-check once verified (the saved list).
    pub(crate) fn open_batch(&mut self, b: &ImportBatch) {
        if b.status != BatchStatus::Committed || self.locked() {
            return;
        }
        self.take_batch(b);
        self.wizard.imported = true;
        self.wizard.verified = b.is_verified();
        if self.wizard.verified && self.wizard.source.steps().contains(&Step::SpotCheck) {
            self.wizard.step = Step::SpotCheck;
            self.load_spot_check(b);
            // Back goes to the tie-out: read it (it may be gone once
            // verified; the step then says it is loading).
            self.load_tieout();
        } else {
            self.wizard.step = Step::TieOut;
            self.load_tieout();
        }
    }

    // ---- Tie-out and verify ----------------------------------------------

    pub(crate) fn load_tieout(&mut self) {
        let Some(id) = self.batch else {
            return;
        };
        self.tieout_job = Some(self.read(
            move |d| d.imports.tieout(id),
            move |s| async move { s.imports().tieout(id).await },
        ));
    }

    /// "Mark verified and continue": the engine recomputes and refuses
    /// unless every difference is zero.
    pub(crate) fn verify(&mut self) {
        let Some(id) = self.batch else {
            return;
        };
        self.verify_job = Some(self.write(
            "orphan.import_verify",
            None,
            move |d| d.imports.verify(id),
            move |s| async move { s.imports().verify(id).await },
        ));
    }

    /// Which job the close prompt speaks of.
    pub(crate) fn running_kind(&self) -> Running {
        self.running
            .or_else(|| self.waiting.map(|w| w.0))
            .or_else(|| self.held.as_ref().map(|h| h.what))
            .unwrap_or(Running::Commit)
    }

    // ---- Developer commands (scenarios, demo only) -----------------------

    pub(crate) fn demo_command(&mut self, rest: &str) -> bool {
        if let Some(name) = rest.strip_prefix("file_") {
            return self.demo_file(name);
        }
        let Some(mut d) = self.link.demo_state() else {
            return false;
        };
        match rest {
            "reset" => {
                d.imports.reset();
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = None;
                }
                drop(d);
                self.start_over();
                self.uploads.clear();
            }
            "backup_fail" => {
                d.imports.backup_mode = crate::company::demo::imports::DemoBackup::FailOnce;
            }
            "backup_hold" => {
                d.imports.backup_mode = crate::company::demo::imports::DemoBackup::HoldOnce;
            }
            "not_empty" => d.imports.company_empty = false,
            "job_hold" => d.imports.hold_jobs = true,
            "job_release" => d.imports.hold_jobs = false,
            "fail_commit" => d.imports.fail_next_commit = Some("IMPORT_INTERRUPTED"),
            "no_closing_password" => d.imports.closing_password_set = false,
            "differences_only" => {
                drop(d);
                if let Some(x) = self.tie.as_mut() {
                    x.view.differences_only = !x.view.differences_only;
                }
            }
            "confirm_closed" => {
                drop(d);
                self.confirm_closed();
            }
            "spot_tick_all" => {
                drop(d);
                for i in &mut self.spot.items {
                    i.done = true;
                }
                self.spot.changed_at = Some(std::time::Instant::now());
            }
            tab if tab.starts_with("tab_") => {
                drop(d);
                let want = &tab[4..];
                let Some(x) = self.tie.as_mut() else {
                    return false;
                };
                let Some(t) = x.tabs().into_iter().find(|t| t.key() == want) else {
                    return false;
                };
                x.tab = t;
            }
            "unmatched" => d.imports.unmatched = vec!["Old suspense".into()],
            "close_books" => {
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = cl_core::CalendarDate::from_ymd(2026, 9, 30).ok();
                }
            }
            "block_undo" => {
                let Some(id) = self.batch else {
                    return false;
                };
                return d
                    .imports
                    .add_blocker(
                        id,
                        cl_api::imports::UndoBlocker {
                            kind: "payment".into(),
                            id: None,
                            reason: Some("applied".into()),
                        },
                    )
                    .is_ok();
            }
            _ => return false,
        }
        true
    }

    /// Adds a compiled-in fixture file to its card (`<set>__<file>` with
    /// `/` as `__`, e.g. `qbd__shoal-creek__journal-2024.csv`; `all` adds
    /// the source's whole base set).
    fn demo_file(&mut self, name: &str) -> bool {
        if name == "all" {
            let set = self.demo_set();
            let mut ok = true;
            for (slot, file) in set {
                ok &= self.demo_add(slot, file);
            }
            return ok;
        }
        let Some((slot, file)) = name.split_once("__") else {
            return false;
        };
        let Some(s) = self.slot(slot) else {
            return false;
        };
        self.demo_add(s.key, &file.replace("__", "/"))
    }

    fn demo_add(&mut self, slot: &'static str, file: &str) -> bool {
        let Some(bytes) = crate::company::demo::imports::fixture_bytes(file) else {
            return false;
        };
        let name = file.rsplit('/').next().unwrap_or(file).to_string();
        self.take_file(slot, name, bytes.to_vec());
        true
    }

    /// The base fixture set of the current source, card by card.
    pub(crate) fn demo_set(&self) -> Vec<(&'static str, &'static str)> {
        use cl_model::imports::wizard::Source;
        match self.wizard.source {
            Source::Qbd => vec![
                ("qbd_account_list", "qbd/shoal-creek/account-list.csv"),
                ("qbd_customer_list", "qbd/shoal-creek/customer-list.csv"),
                ("qbd_vendor_list", "qbd/shoal-creek/vendor-list.csv"),
                ("qbd_other_names", "qbd/shoal-creek/other-names.csv"),
                ("qbd_item_list", "qbd/shoal-creek/item-list.csv"),
                (
                    "qbd_tb_opening",
                    "qbd/shoal-creek/tb-opening-2023-12-31.csv",
                ),
                ("qbd_journal", "qbd/shoal-creek/journal-2024.csv"),
                ("qbd_journal", "qbd/shoal-creek/journal-2025-2026.csv"),
                ("qbd_tb_reference", "qbd/shoal-creek/tb-2026-10-27.csv"),
                ("qbd_open_invoices", "qbd/shoal-creek/open-invoices.csv"),
                ("qbd_unpaid_bills", "qbd/shoal-creek/unpaid-bills.csv"),
                (
                    "qbd_inventory_valuation",
                    "qbd/shoal-creek/inventory-valuation.csv",
                ),
            ],
            Source::AccountingPower => vec![
                ("ap_coa", "imports/accounting-power/coa.csv"),
                ("ap_customers", "imports/accounting-power/customers.csv"),
                ("ap_vendors", "imports/accounting-power/vendors.csv"),
                ("ap_tb_opening", "imports/accounting-power/tb-opening.csv"),
                ("ap_gl", "imports/accounting-power/gl.csv"),
                (
                    "ap_tb_reference",
                    "imports/accounting-power/tb-reference.csv",
                ),
            ],
            Source::Qbo => vec![
                ("qbo_coa", "imports/quickbooks-online/coa.csv"),
                ("qbo_customers", "imports/quickbooks-online/customers.csv"),
                ("qbo_vendors", "imports/quickbooks-online/vendors.csv"),
                ("qbo_tb_opening", "imports/quickbooks-online/tb-opening.csv"),
                ("qbo_journal", "imports/quickbooks-online/gl.csv"),
                (
                    "qbo_tb_reference",
                    "imports/quickbooks-online/tb-reference.csv",
                ),
            ],
            Source::QbdGl => vec![
                ("qbd_account_list", "imports/qbd-gl/account-list.csv"),
                ("qbd_customer_list", "imports/qbd-gl/customer-list.csv"),
                ("qbd_vendor_list", "imports/qbd-gl/vendor-list.csv"),
                ("qbd_tb_opening", "imports/qbd-gl/tb-opening.csv"),
                ("qbd_journal", "imports/qbd-gl/journal.csv"),
                ("qbd_tb_reference", "imports/qbd-gl/tb-reference.csv"),
            ],
            Source::Generic => vec![
                ("generic_coa", "imports/generic/coa.csv"),
                ("generic_contacts", "imports/generic/contacts.csv"),
                ("generic_tb_opening", "imports/generic/tb-opening.csv"),
                ("generic_gl", "imports/generic/gl.csv"),
                ("generic_tb_reference", "imports/generic/tb-reference.csv"),
            ],
        }
    }
}

/// An import refusal a file's upload got (a re-import, S10).
fn import_refusal_of(e: &ApiError) -> Option<ImportRefusal> {
    cl_api::imports::import_refusal(e)
}
