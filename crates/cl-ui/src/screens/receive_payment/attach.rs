//! Attachments on a saved payment (Q-P21w, the mockup's Attach), as Create
//! Invoice keeps them (`invoice/attach.rs`): Attach picks a file (PDF, PNG,
//! JPEG or TIFF, at most 10 MB, checked here first and read off the
//! frame), uploads it to the payment (`attachable_type:
//! customer_payment`, keyed so a retry stores it once), and the side
//! panel lists the files; Open shows one in the document viewer. Files are
//! owner data: names are shown, never logged.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::{
    ApiError, AttachTo, Attachment, AttachmentId, IdempotencyKey, OrphanLabel, PageRequest, TxnType,
};
use cl_core::ids::TransactionId;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::ReceivePaymentScreen;
use super::engine::{self, Job};
use crate::company::CompanyEvent;
use crate::i18n::t;
use crate::screens::write_check::attach::{
    FileRead, poll_file_read, read_file, read_in_background,
};
use crate::theme::Tokens;

/// The payment's files and the work in flight.
#[derive(Default)]
pub(crate) struct PaymentAttach {
    picking: Option<Receiver<Vec<PathBuf>>>,
    reading: Option<Receiver<FileRead>>,
    upload: Option<(String, IdempotencyKey, Job<Attachment>)>,
    list_job: Option<Job<Vec<Attachment>>>,
    open_job: Option<Job<(String, Vec<u8>)>>,
    /// The files on the open payment.
    pub(crate) list: Vec<Attachment>,
    pub(crate) problem: Option<&'static str>,
    of: Option<TransactionId>,
    /// The key of an upload whose outcome is unknown, reused when the same
    /// file is tried again.
    retry: Option<(String, IdempotencyKey)>,
    /// Files attached in this window (control channel).
    pub(crate) attached: u32,
}

impl PaymentAttach {
    pub(crate) fn busy(&self) -> bool {
        self.picking.is_some() || self.reading.is_some() || self.upload.is_some()
    }

    pub(crate) fn state_json(&self) -> Value {
        json!({
            "files": self.list.len(),
            "busy": self.busy(),
            "attached": self.attached,
            "problem": self.problem,
        })
    }
}

fn target(id: TransactionId) -> AttachTo {
    AttachTo::Txn(TxnType::CustomerPayment, id)
}

impl ReceivePaymentScreen {
    /// Lists the open payment's files (on open and after an upload).
    pub(crate) fn list_attachments(&mut self) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.attach = PaymentAttach::default();
            return;
        };
        if self.attach.of != Some(id) {
            self.attach.list.clear();
            self.attach.problem = None;
        }
        self.attach.of = Some(id);
        self.attach.list_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::checks::attachments(d, target(id))),
            move |s| async move {
                Ok(s.attachments()
                    .list(target(id), PageRequest::first_max())
                    .await?
                    .data)
            },
        ));
        self.poll_attach();
    }

    /// Attach…: the file picker (a saved payment only).
    pub(crate) fn start_attach(&mut self, ctx: &egui::Context) {
        if self.editing.is_none() {
            self.notice = Some("payment.attach.save_first");
            return;
        }
        if self.attach.busy() {
            return;
        }
        self.attach.problem = None;
        self.attach.picking = Some(crate::screens::write_check::attach::browse_for_file(ctx));
    }

    /// Uploads `name`/`bytes` to the open payment (the control channel and
    /// the picker both end here).
    pub(crate) fn upload_attachment(&mut self, name: String, bytes: Vec<u8>) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.attach.problem = Some("payment.attach.save_first");
            return;
        };
        if let Err(k) = crate::screens::write_check::attach::check_bytes(&bytes) {
            self.attach.problem = Some(k);
            return;
        }
        // The same file tried again keeps its key (stored once).
        let key = match self.attach.retry.take() {
            Some((n, k)) if n == name => k,
            _ => IdempotencyKey::new(),
        };
        let (n2, b2, tag) = (name.clone(), bytes.clone(), name.clone());
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            OrphanLabel {
                what: "orphan.payment_attach",
                key: Some(key.clone()),
            },
            move |d| crate::company::demo::checks::upload(d, target(id), &n2, &b2),
            {
                let key = key.clone();
                move |s| async move { s.attachments().upload(target(id), &name, &bytes, key).await }
            },
        );
        self.attach.upload = Some((tag, key, job));
        self.poll_attach();
    }

    /// Opens a file full size in the document viewer.
    pub(crate) fn open_attachment(&mut self, a: &Attachment) {
        let (id, name): (AttachmentId, String) = (a.id, a.file_name.clone());
        let n2 = name.clone();
        self.attach.open_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::checks::download(d, id).map(|b| (n2, b)),
            move |s| async move { Ok((name, s.attachments().download(id).await?)) },
        ));
        self.poll_attach();
    }

    pub(crate) fn poll_attach(&mut self) {
        if let Some(rx) = &self.attach.picking
            && let Ok(paths) = rx.try_recv()
        {
            self.attach.picking = None;
            if let Some(p) = paths.into_iter().next() {
                match read_in_background(&p, read_file) {
                    Ok(rx) => self.attach.reading = Some(rx),
                    Err(k) => self.attach.problem = Some(k),
                }
            }
        }
        match poll_file_read(&mut self.attach.reading) {
            Some(Ok((name, bytes))) => self.upload_attachment(name, bytes),
            Some(Err(k)) => self.attach.problem = Some(k),
            None => {}
        }
        if let Some((n, k, mut job)) = self.attach.upload.take() {
            match job.take() {
                None => self.attach.upload = Some((n, k, job)),
                Some(Ok(_)) => {
                    self.attach.attached += 1;
                    self.list_attachments();
                }
                Some(Err(e)) => {
                    if crate::screens::register::keeps_key(&e) {
                        self.attach.retry = Some((n, k));
                    }
                    self.attach.problem = Some(match e {
                        ApiError::Local(cl_api::LocalValidation::AttachmentTooLarge) => {
                            "check.attach.too_large"
                        }
                        ApiError::Local(_) => "check.attach.wrong_type",
                        _ => "invoice.attach.failed",
                    });
                }
            }
        }
        if let Some(r) = engine::poll(&mut self.attach.list_job) {
            match r {
                Ok(l) => self.attach.list = l,
                Err(_) => self.attach.problem = Some("invoice.attach.list_failed"),
            }
        }
        match engine::poll(&mut self.attach.open_job) {
            Some(Ok((name, bytes))) => self.link.push(CompanyEvent::OpenWith(
                cl_model::CommandId::new(crate::screens::document_view::KIND),
                Box::new(crate::windows::OpenPayload::Document(
                    crate::widgets::pdf_viewer::DocSource {
                        name,
                        bytes: Arc::from(bytes),
                    },
                )),
            )),
            Some(Err(_)) => self.attach.problem = Some("invoice.attach.open_failed"),
            None => {}
        }
    }

    /// The side panel's Attachments card (saved payments only).
    pub(crate) fn attach_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.editing.is_none() {
            return;
        }
        if self.attach.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut open = None;
        let files = self.attach.list.clone();
        let problem = self.attach.problem;
        let busy = self.attach.busy();
        crate::screens::card_charge::kit::panel_card(
            ui,
            tokens,
            &t("invoice.attach.heading"),
            |ui| {
                if files.is_empty() {
                    ui.label(RichText::new(t("invoice.attach.none")).color(tokens.text_secondary));
                }
                for f in &files {
                    ui.horizontal(|ui| {
                        ui.label(&f.file_name);
                        if ui.small_button(t("invoice.attach.open")).clicked() {
                            open = Some(f.clone());
                        }
                    });
                }
                if busy {
                    ui.label(
                        RichText::new(t("invoice.attach.sending")).color(tokens.text_secondary),
                    );
                }
                if let Some(p) = problem {
                    ui.label(RichText::new(t(p)).color(tokens.warn_text));
                }
            },
        );
        if let Some(f) = open {
            self.open_attachment(&f);
        }
    }
}
