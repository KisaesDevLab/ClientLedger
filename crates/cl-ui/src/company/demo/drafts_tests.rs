//! Tests of the demo document drafts, and the parity runner over
//! `fixtures/docs/draft-cases.json` (T16).
//!
//! The runner's argument vocabulary (the live runner reads the same):
//! - `upload_draft_file {content, draftKey?}`: a synthetic one-line PDF
//!   (`%PDF-1.4`, a comment line holding `content`, `%%EOF`) uploaded as a
//!   draft file; files naming the same `draftKey` label share a key,
//!   otherwise each gets its own. 201.
//! - `claim {file, kindHint, key}`; `get {draft}`; `discard {draft}`;
//!   `list {status, limit?}` (`expect.total` checks the engine's total).
//! - `store_reading {draft, expectedUpdatedAt, kind, docTotal?, docDate?,
//!   docReference?, vendor?}`; `store_failure {draft, expectedUpdatedAt,
//!   reason}`.
//! - `convert {draft, expectedUpdatedAt, kind, date, amount, vendor?,
//!   reference?, acknowledge?, closingPassword?: "right" | "wrong", key}`:
//!   one line of `amount` to the company's equipment rental expense
//!   account; a card charge from the credit card, a cash expense and a
//!   check from checking (a check prints later, so it has no number).
//! - `attach {draft, expectedUpdatedAt, transaction, key}`;
//!   `link_attachment {file, transaction}` (`POST /attachments/:id/link`);
//!   `plain_relink {file, transaction}` (a plain save moving the file's
//!   draft key onto the record, as `POST /bills/:id/attach-draft`).
//! - `post_expense {date, amount, vendor?, payFrom: "card", reference?}`;
//!   `void {transaction}`; `set_closing_date {date}`;
//!   `search_amount {amount, start, end}` (`expect.count`).
//! - `vendor: "story"` is the company's story vendor; `key` values are
//!   labels (one idempotency key per label in a case).
//! - `expect.code` is compared when it is not null; `expect.file`
//!   (`"draft"` or `"moved"`) checks where the step's file (`args.file`,
//!   else the draft's own file) is afterwards.

use std::collections::HashMap;
use std::str::FromStr as _;

use cl_api::{
    AttachTo, AttachmentId, BillDraft, BillLineDraft, CardChargeDraft, CheckDraft, DraftKey,
    DraftReader, DraftReading, DraftRefusal, SplitLine,
};
use serde_json::{Value, json};

use super::*;

const SECTIONS: [&str; 7] = [
    "claim",
    "store",
    "discard",
    "convert",
    "attach",
    "guard",
    "amount_filter",
];

fn cases() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/docs/draft-cases.json");
    let text = std::fs::read_to_string(path).expect("draft-cases.json");
    serde_json::from_str(&text).expect("json")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn date(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

#[test]
fn the_shared_cases_keep_their_shape() {
    let v = cases();
    let ops: Vec<&str> = v["_schema"]["ops"]
        .as_array()
        .expect("ops")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut names = std::collections::HashSet::new();
    for section in SECTIONS {
        for case in v[section].as_array().expect(section) {
            let name = case["name"].as_str().expect("name");
            assert!(names.insert(name.to_owned()), "{name} twice");
            assert!(
                matches!(case["owner"].as_str(), Some("WP-D" | "WP-E")),
                "{name}"
            );
            for step in case["steps"].as_array().expect("steps") {
                let op = step["op"].as_str().expect("op");
                assert!(ops.contains(&op), "{name}: {op}");
                assert!(step["expect"]["status"].is_u64(), "{name}: {op}");
            }
        }
    }
    let keys: Vec<&String> = v.as_object().expect("object").keys().collect();
    assert_eq!(keys.len(), SECTIONS.len() + 2, "{keys:?}");
}

// ------------------------------------------------------------- the runner

/// One case's world: a fresh demo engine, the step labels and the keys.
struct Runner {
    s: DemoState,
    tenant: TenantId,
    labels: HashMap<String, Value>,
    keys: HashMap<String, IdempotencyKey>,
    draft_keys: HashMap<String, DraftKey>,
}

/// What a step answered: the engine's status and code, the draft it
/// returned, and its result for later labels.
struct Outcome {
    status: u16,
    code: Option<String>,
    draft: Option<DocumentDraft>,
    label: Value,
    total: Option<u64>,
    count: Option<usize>,
}

impl Outcome {
    fn ok(status: u16, label: Value) -> Outcome {
        Outcome {
            status,
            code: None,
            draft: None,
            label,
            total: None,
            count: None,
        }
    }

    fn draft(status: u16, d: DocumentDraft) -> Outcome {
        let label = json!({"id": d.id, "updatedAt": d.updated_at});
        Outcome {
            draft: Some(d),
            ..Outcome::ok(status, label)
        }
    }

    fn err(e: &ApiError) -> Outcome {
        let (status, code) = status_of(e);
        Outcome {
            code,
            ..Outcome::ok(status, Value::Null)
        }
    }
}

/// The HTTP status and code an [`ApiError`] stands for.
fn status_of(e: &ApiError) -> (u16, Option<String>) {
    let code = |c: &str| Some(c.to_string());
    match e {
        ApiError::DocumentDraft(r) => match r {
            DraftRefusal::AlreadyClaimed { .. } => (409, code(cl_api::DOCUMENT_ALREADY_CLAIMED)),
            DraftRefusal::Closed { .. } => (409, code(cl_api::DOCUMENT_DRAFT_CLOSED)),
            DraftRefusal::PossibleDuplicate { .. } => (409, code(cl_api::POSSIBLE_DUPLICATE)),
            DraftRefusal::NotADraftFile => (422, code(cl_api::ATTACHMENT_NOT_DRAFT)),
            DraftRefusal::TemplateLimit => (409, code(cl_api::TEMPLATE_LIMIT)),
        },
        ApiError::Conflict { code: c, .. } => (409, c.as_ref().map(|c| c.as_str().to_string())),
        ApiError::Validation(v) => {
            let c = v.code().map(|c| c.as_str().to_string());
            let status = if c.as_deref() == Some("VALIDATION_ERROR") {
                400
            } else {
                422
            };
            (status, c)
        }
        ApiError::ClosedPeriod(_) => (423, code("TB_PERIOD_LOCKED")),
        ApiError::ClosingPasswordRejected => (403, code("CLOSING_PASSWORD_INVALID")),
        ApiError::NotFound { code: c } => (404, c.as_ref().map(|c| c.as_str().to_string())),
        ApiError::IdempotencyKeyReused { .. } => (422, code("IDEMPOTENCY_KEY_REUSED")),
        ApiError::Forbidden { code: c } => (403, c.as_ref().map(|c| c.as_str().to_string())),
        other => panic!("not an engine answer: {other:?}"),
    }
}

impl Runner {
    fn new() -> Runner {
        let s = DemoState::new();
        let tenant = s.open.expect("open company");
        Runner {
            s,
            tenant,
            labels: HashMap::new(),
            keys: HashMap::new(),
            draft_keys: HashMap::new(),
        }
    }

    /// A `$label` or `$label.field` reference, else the text itself.
    fn text(&self, v: &Value) -> String {
        let s = v.as_str().expect("a text argument");
        let Some(r) = s.strip_prefix('$') else {
            return s.to_string();
        };
        let (label, field) = r.split_once('.').unwrap_or((r, "id"));
        self.labels
            .get(label)
            .and_then(|l| l[field].as_str())
            .unwrap_or_else(|| panic!("unknown reference {s}"))
            .to_string()
    }

    fn arg(&self, args: &Value, k: &str) -> String {
        self.text(&args[k])
    }

    fn opt(&self, args: &Value, k: &str) -> Option<String> {
        (!args[k].is_null()).then(|| self.text(&args[k]))
    }

    fn key(&mut self, args: &Value) -> IdempotencyKey {
        let label = args["key"].as_str().unwrap_or("").to_string();
        self.keys
            .entry(label)
            .or_insert_with(IdempotencyKey::new)
            .clone()
    }

    fn draft_id(&self, args: &Value) -> DocumentDraftId {
        DocumentDraftId::parse(&self.arg(args, "draft")).expect("draft id")
    }

    fn file_id(&self, args: &Value) -> AttachmentId {
        AttachmentId::parse(&self.arg(args, "file")).expect("file id")
    }

    fn txn_id(&self, args: &Value) -> TransactionId {
        TransactionId::parse(&self.arg(args, "transaction")).expect("transaction id")
    }

    fn account(&mut self, number: &str) -> AccountId {
        self.s
            .chart
            .get_or_insert_with(super::super::chart)
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(number))
            .map(|a| a.id)
            .unwrap_or_else(|| panic!("account {number}"))
    }

    fn vendor(&mut self, args: &Value) -> Option<ContactId> {
        let name = match args["vendor"].as_str()? {
            "story" => super::super::bills::STORY_VENDOR,
            other => panic!("vendor {other}"),
        };
        self.s
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == name)
            .map(|c| c.id)
    }

    fn the_target(&self, id: TransactionId) -> AttachTo {
        let kind = self
            .s
            .ledger
            .transactions()
            .into_iter()
            .find(|t| DemoLedger::id_of(t) == id)
            .map(|t| t.kind)
            .expect("transaction");
        AttachTo::Txn(engine_type(&kind), id)
    }

    fn body(&mut self, args: &Value) -> DemoConvertBody {
        let kind = args["kind"].as_str().expect("kind");
        let date = date(&self.arg(args, "date"));
        let amount = money(&self.arg(args, "amount"));
        let reference = self.opt(args, "reference");
        let vendor = self.vendor(args);
        let expense = self.account("6210");
        let line = SplitLine {
            account: expense,
            amount,
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        };
        match kind {
            "bill" => DemoConvertBody::Bill(BillDraft {
                vendor: vendor.expect("a bill names its vendor"),
                date,
                vendor_invoice_number: reference,
                terms: None,
                due_date: None,
                memo: None,
                amount_due: amount,
                lines: vec![BillLineDraft::expense(expense, amount)],
                received: true,
                draft_attachment: None,
            }),
            "card_charge" | "cash_expense" => {
                let pay_from = if kind == "card_charge" {
                    self.account("2050")
                } else {
                    self.account("1010")
                };
                let d = CardChargeDraft {
                    pay_from,
                    date,
                    number: reference,
                    payee: vendor,
                    memo: None,
                    lines: vec![line],
                };
                if kind == "card_charge" {
                    DemoConvertBody::CardCharge(d)
                } else {
                    DemoConvertBody::CashExpense(d)
                }
            }
            "check" => DemoConvertBody::Check(CheckDraft {
                bank: self.account("1010"),
                date,
                payee: vendor,
                payee_name: "Synthetic payee".into(),
                check_number: None,
                print_later: true,
                payee_address: None,
                memo: None,
                printed_memo: None,
                tag_ids: Vec::new(),
                draft_attachment: None,
                amount,
                lines: vec![line],
            }),
            other => panic!("kind {other}"),
        }
    }

    fn password(args: &Value) -> Option<Secret> {
        match args["closingPassword"].as_str() {
            None => None,
            Some("right") => Some(Secret::new(super::super::register::DEMO_CLOSING_PASSWORD)),
            Some(_) => Some(Secret::new("not the password")),
        }
    }

    fn upload(&mut self, args: &Value) -> Outcome {
        let content = args["content"].as_str().expect("content");
        let key = match args["draftKey"].as_str() {
            Some(label) => *self
                .draft_keys
                .entry(label.to_string())
                .or_insert_with(DraftKey::new),
            None => DraftKey::new(),
        };
        let bytes = format!("%PDF-1.4\n% {content}\n%%EOF\n");
        let a = super::super::checks::upload(
            &mut self.s,
            AttachTo::Draft(key),
            "synthetic.pdf",
            bytes.as_bytes(),
        )
        .expect("upload");
        Outcome::ok(201, json!({"id": a.id}))
    }

    fn post_expense(&mut self, args: &Value) -> Outcome {
        assert_eq!(args["payFrom"].as_str(), Some("card"), "payFrom");
        let d = CardChargeDraft {
            pay_from: self.account("2050"),
            date: date(&self.arg(args, "date")),
            number: self.opt(args, "reference"),
            payee: self.vendor(args),
            memo: None,
            lines: vec![SplitLine {
                account: self.account("6210"),
                amount: money(&self.arg(args, "amount")),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        };
        let id = post_expense(&mut self.s, &d, "card_charge").expect("posted");
        Outcome::ok(201, json!({"id": id}))
    }

    fn run(&mut self, op: &str, args: &Value) -> Outcome {
        let tenant = self.tenant;
        let draft_outcome = |status: u16, r: Result<DocumentDraft, ApiError>| match r {
            Ok(d) => Outcome::draft(status, d),
            Err(e) => Outcome::err(&e),
        };
        match op {
            "upload_draft_file" => self.upload(args),
            "claim" => {
                let file = self.file_id(args);
                let hint: DraftKind =
                    serde_json::from_value(args["kindHint"].clone()).expect("kindHint");
                let key = self.key(args);
                draft_outcome(201, create(&mut self.s, tenant, file, hint, key))
            }
            "get" => {
                let id = self.draft_id(args);
                draft_outcome(200, get(&self.s, tenant, id))
            }
            "list" => {
                let statuses = args["status"]
                    .as_str()
                    .unwrap_or("")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| serde_json::from_value(json!(s)).expect("status"))
                    .collect();
                let q = DraftListQuery {
                    statuses,
                    limit: args["limit"]
                        .as_u64()
                        .and_then(|l| u32::try_from(l).ok())
                        .unwrap_or(50),
                    ..DraftListQuery::default()
                };
                match list(&self.s, tenant, &q) {
                    Ok(p) => Outcome {
                        total: Some(p.total),
                        ..Outcome::ok(200, Value::Null)
                    },
                    Err(e) => Outcome::err(&e),
                }
            }
            "store_reading" => {
                let id = self.draft_id(args);
                let expected = self.arg(args, "expectedUpdatedAt");
                let reading = DraftReading {
                    kind: serde_json::from_value(args["kind"].clone()).expect("kind"),
                    reader: DraftReader::Ocr,
                    fields: serde_json::Map::new(),
                    lines: Vec::new(),
                    checks: Vec::new(),
                    suggested_contact: self.vendor(args),
                    doc_reference: self.opt(args, "docReference"),
                    doc_total: self.opt(args, "docTotal").map(|t| money(&t)),
                    doc_date: self.opt(args, "docDate").map(|d| date(&d)),
                };
                draft_outcome(
                    200,
                    store_reading(&mut self.s, tenant, id, &expected, &reading),
                )
            }
            "store_failure" => {
                let id = self.draft_id(args);
                let expected = self.arg(args, "expectedUpdatedAt");
                let reason = self.arg(args, "reason");
                draft_outcome(
                    200,
                    store_failure(&mut self.s, tenant, id, &expected, &reason),
                )
            }
            "discard" => {
                let id = self.draft_id(args);
                draft_outcome(200, discard(&mut self.s, tenant, id))
            }
            "convert" => {
                let id = self.draft_id(args);
                let conversion = DemoConversion {
                    body: self.body(args),
                    expected_updated_at: self.arg(args, "expectedUpdatedAt"),
                    acknowledge: args["acknowledge"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .map(|t| TransactionId::parse(&self.text(t)).expect("ack"))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                let password = Self::password(args);
                let key = self.key(args);
                match convert(&mut self.s, tenant, id, &conversion, password.as_ref(), key) {
                    Ok(c) => Outcome::draft(201, c.draft),
                    Err(e) => Outcome::err(&e),
                }
            }
            "attach" => {
                let id = self.draft_id(args);
                let txn = self.txn_id(args);
                let expected = self.arg(args, "expectedUpdatedAt");
                let key = self.key(args);
                match attach(&mut self.s, tenant, id, txn, &expected, key) {
                    Ok(a) => Outcome::draft(200, a.draft),
                    Err(e) => Outcome::err(&e),
                }
            }
            "link_attachment" => {
                let file = self.file_id(args);
                let to = self.the_target(self.txn_id(args));
                match super::super::checks::link_attachment(&mut self.s, file, to) {
                    Ok(_) => Outcome::ok(200, Value::Null),
                    Err(e) => Outcome::err(&e),
                }
            }
            "plain_relink" => {
                let file = self.file_id(args);
                let to = self.the_target(self.txn_id(args));
                let a = super::super::checks::find_attachment(&self.s, file).expect("file");
                let key = DraftKey::from_str(&a.attachable_id).expect("on a draft key");
                super::super::checks::link_draft(&mut self.s, key, to);
                Outcome::ok(200, Value::Null)
            }
            "post_expense" => self.post_expense(args),
            "void" => {
                let id = self.txn_id(args);
                self.s.ledger.void(id).expect("void");
                Outcome::ok(200, Value::Null)
            }
            "set_closing_date" => {
                let d = date(&self.arg(args, "date"));
                let c = self
                    .s
                    .companies
                    .iter_mut()
                    .find(|c| c.tenant_id == tenant)
                    .expect("company");
                c.closed_through = Some(d);
                Outcome::ok(200, Value::Null)
            }
            "search_amount" => {
                let q = AmountSearch {
                    amount: money(&self.arg(args, "amount")),
                    start: date(&self.arg(args, "start")),
                    end: date(&self.arg(args, "end")),
                    account: None,
                };
                let rows = search_amount(&mut self.s, &q).expect("search");
                Outcome {
                    count: Some(rows.len()),
                    ..Outcome::ok(200, Value::Null)
                }
            }
            other => panic!("op {other} is not played by the demo runner"),
        }
    }

    /// Where the step's file is: `args.file`, else the draft's own file.
    fn file_place(&self, args: &Value, draft: Option<DocumentDraftId>) -> &'static str {
        let file = if args["file"].is_null() {
            draft
                .and_then(|id| self.s.drafts.find(self.tenant, id))
                .and_then(|r| r.draft.attachment_id)
                .expect("the draft's file")
        } else {
            self.file_id(args)
        };
        let a = super::super::checks::find_attachment(&self.s, file).expect("file");
        if a.attachable_type == "draft" {
            "draft"
        } else {
            "moved"
        }
    }
}

#[test]
fn the_demo_engine_answers_every_shared_case() {
    let v = cases();
    let mut ran = 0;
    for section in SECTIONS {
        for case in v[section].as_array().expect(section) {
            let name = case["name"].as_str().expect("name");
            let mut r = Runner::new();
            for (i, step) in case["steps"].as_array().expect("steps").iter().enumerate() {
                let op = step["op"].as_str().expect("op");
                let args = &step["args"];
                let expect = &step["expect"];
                let at = format!("{name} step {i} ({op})");
                let out = r.run(op, args);
                assert_eq!(
                    u64::from(out.status),
                    expect["status"].as_u64().expect("status"),
                    "{at}: code {:?}",
                    out.code
                );
                if let Some(code) = expect["code"].as_str() {
                    assert_eq!(out.code.as_deref(), Some(code), "{at}");
                }
                if let Some(s) = expect["draft_status"].as_str() {
                    let got = out.draft.as_ref().map(|d| d.status.as_str());
                    assert_eq!(got, Some(s), "{at}");
                }
                if let Some(t) = expect["total"].as_u64() {
                    assert_eq!(out.total, Some(t), "{at}");
                }
                if let Some(c) = expect["count"].as_u64() {
                    assert_eq!(out.count.map(|c| c as u64), Some(c), "{at}");
                }
                if let Some(place) = expect["file"].as_str() {
                    let draft = (!args["draft"].is_null()).then(|| r.draft_id(args));
                    assert_eq!(r.file_place(args, draft), place, "{at}");
                }
                if let Some(label) = step["as"].as_str() {
                    r.labels.insert(label.to_string(), out.label);
                }
            }
            ran += 1;
        }
    }
    assert!(ran >= 37, "only {ran} cases");
}

// ------------------------------------------------------ direct unit tests

fn world() -> (DemoState, TenantId) {
    let s = DemoState::new();
    let t = s.open.expect("open");
    (s, t)
}

fn upload(s: &mut DemoState, key: DraftKey, content: &str) -> AttachmentId {
    let bytes = format!("%PDF-1.4\n% {content}\n%%EOF\n");
    super::super::checks::upload(s, AttachTo::Draft(key), "x.pdf", bytes.as_bytes())
        .expect("upload")
        .id
}

fn card(s: &mut DemoState, amount: &str, vendor: Option<ContactId>) -> DemoConversion {
    let mut r = Runner {
        s: std::mem::take(s),
        tenant: TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("t"),
        labels: HashMap::new(),
        keys: HashMap::new(),
        draft_keys: HashMap::new(),
    };
    let mut body = r.body(&json!({"kind": "card_charge", "date": "2026-11-03", "amount": amount}));
    if let DemoConvertBody::CardCharge(d) = &mut body {
        d.payee = vendor;
    }
    *s = r.s;
    DemoConversion {
        body,
        expected_updated_at: String::new(),
        acknowledge: Vec::new(),
    }
}

#[test]
fn without_drafts_every_call_is_unsupported_and_changes_nothing() {
    assert!(!DemoDrafts::default().supported());
    assert!(DemoState::new().drafts.supported());
    let (mut s, tenant) = world();
    s.drafts.enabled = false;
    let file = upload(&mut s, DraftKey::new(), "off");
    let id = DocumentDraftId::parse("00000000-0000-4000-8000-000000000003").expect("id");
    let un = |e: ApiError| matches!(e, ApiError::Local(LocalValidation::Unsupported));
    let key = IdempotencyKey::new;
    assert!(un(
        create(&mut s, tenant, file, DraftKind::Bill, key()).expect_err("no")
    ));
    assert!(un(
        list(&s, tenant, &DraftListQuery::default()).expect_err("no")
    ));
    assert!(un(get(&s, tenant, id).expect_err("no")));
    assert!(un(duplicates(&mut s, tenant, id).expect_err("no")));
    assert!(un(
        store_failure(&mut s, tenant, id, "t", "gave_up").expect_err("no")
    ));
    assert!(un(discard(&mut s, tenant, id).expect_err("no")));
    let c = card(&mut s, "1.00", None);
    assert!(un(
        convert(&mut s, tenant, id, &c, None, key()).expect_err("no")
    ));
    let txn = TransactionId::parse("00000000-0000-4000-8000-000000000004").expect("t");
    assert!(un(
        attach(&mut s, tenant, id, txn, "t", key()).expect_err("no")
    ));
    let q = AmountSearch {
        amount: money("1.00"),
        start: date("2026-01-01"),
        end: date("2026-12-31"),
        account: None,
    };
    assert!(un(search_amount(&mut s, &q).expect_err("no")));
    assert_eq!(s.drafts.requests, 0);
    assert!(s.drafts.rows.is_empty());
}

#[test]
fn tokens_always_move_forward_and_drafts_are_per_company() {
    let (mut s, tenant) = world();
    let f1 = upload(&mut s, DraftKey::new(), "a");
    let d = create(&mut s, tenant, f1, DraftKind::Bill, IdempotencyKey::new()).expect("d");
    let t2 = touch(&mut s, tenant, d.id).expect("touch");
    let t3 = touch(&mut s, tenant, d.id).expect("touch");
    assert!(d.updated_at < t2 && t2 < t3, "ISO ms strings sort in time");
    let other = s.companies[1].tenant_id;
    assert!(matches!(
        get(&s, other, d.id),
        Err(ApiError::NotFound { .. })
    ));
    assert_eq!(
        list(&s, other, &DraftListQuery::default())
            .expect("list")
            .total,
        0
    );
    // The same bytes in another company are that company's to claim.
    let f2 = upload(&mut s, DraftKey::new(), "a");
    assert!(create(&mut s, other, f2, DraftKind::Bill, IdempotencyKey::new()).is_ok());
}

#[test]
fn the_summary_view_leaves_the_reading_out_and_bounds_are_checked() {
    let (mut s, tenant) = world();
    let f = upload(&mut s, DraftKey::new(), "b");
    let d = create(&mut s, tenant, f, DraftKind::Bill, IdempotencyKey::new()).expect("d");
    assert_eq!(d.fields, Some(json!({})));
    let q = DraftListQuery {
        summary: true,
        ..DraftListQuery::default()
    };
    let page = list(&s, tenant, &q).expect("list");
    assert!(page.drafts[0].fields.is_none() && page.drafts[0].lines.is_none());
    for bad in [
        DraftListQuery {
            limit: 0,
            ..DraftListQuery::default()
        },
        DraftListQuery {
            limit: 201,
            ..DraftListQuery::default()
        },
        DraftListQuery {
            offset: 10_001,
            ..DraftListQuery::default()
        },
    ] {
        assert_eq!(list(&s, tenant, &bad), Err(invalid()));
    }
    let mut fields = serde_json::Map::new();
    fields.insert("total".into(), json!(12.5));
    let reading = DraftReading {
        kind: DraftKind::Bill,
        reader: DraftReader::TextPdf,
        fields,
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: None,
        doc_reference: Some(" REF ".into()),
        doc_total: Some(money("-3.00")),
        doc_date: None,
    };
    let before = s.drafts.requests;
    assert_eq!(
        store_reading(&mut s, tenant, d.id, &d.updated_at, &reading),
        Err(invalid()),
        "a fractional JSON number is never stored"
    );
    assert_eq!(s.drafts.requests, before, "nothing sent");
    let ok = DraftReading {
        fields: serde_json::Map::new(),
        ..reading
    };
    let stored = store_reading(&mut s, tenant, d.id, &d.updated_at, &ok).expect("stored");
    assert_eq!(stored.doc_reference.as_deref(), Some("REF"));
    assert_eq!(stored.doc_total, None, "a negative total is left out");
    assert_eq!(
        store_failure(&mut s, tenant, d.id, &stored.updated_at, "Bad Reason"),
        Err(invalid())
    );
}

#[test]
fn a_lost_reply_resent_under_its_key_posts_once() {
    let (mut s, tenant) = world();
    let f = upload(&mut s, DraftKey::new(), "c");
    let d = create(
        &mut s,
        tenant,
        f,
        DraftKind::CardCharge,
        IdempotencyKey::new(),
    )
    .expect("d");
    let mut c = card(&mut s, "17.17", None);
    c.expected_updated_at.clone_from(&d.updated_at);
    let before = s.ledger.transactions().len();
    let key = IdempotencyKey::new();
    s.drafts.busy_next = true;
    assert!(matches!(
        convert(&mut s, tenant, d.id, &c, None, key.clone()),
        Err(ApiError::Conflict {
            reason: cl_api::ConflictReason::Busy,
            ..
        })
    ));
    s.drafts.lose_next_reply = true;
    let lost = convert(&mut s, tenant, d.id, &c, None, key.clone()).expect_err("lost");
    assert!(matches!(lost, ApiError::OutcomeUnknown { keyed: true, .. }));
    let again = convert(&mut s, tenant, d.id, &c, None, key).expect("replayed");
    assert_eq!(s.ledger.transactions().len(), before + 1, "posted once");
    assert_eq!(again.draft.status, DraftStatus::Converted);
    assert_eq!(again.draft.converted_type.as_deref(), Some("expense"));
    // A fresh key after the convert: closed, naming the record.
    match convert(&mut s, tenant, d.id, &c, None, IdempotencyKey::new()) {
        Err(ApiError::DocumentDraft(DraftRefusal::Closed { transaction })) => {
            assert_eq!(transaction, Some(again.transaction));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_possible_duplicate_names_the_record_and_other_drafts() {
    let (mut s, tenant) = world();
    let vendor = s
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == super::super::bills::STORY_VENDOR)
        .map(|c| c.id);
    let mut posted = card(&mut s, "81.81", vendor);
    let DemoConvertBody::CardCharge(p) = &mut posted.body else {
        panic!("card")
    };
    p.date = date("2026-11-01");
    let t1 = post_expense(&mut s, p, "card_charge").expect("posted");
    // Another live draft of the vendor with the same total.
    let f0 = upload(&mut s, DraftKey::new(), "other");
    let other = create(
        &mut s,
        tenant,
        f0,
        DraftKind::CardCharge,
        IdempotencyKey::new(),
    )
    .expect("other");
    let reading = DraftReading {
        kind: DraftKind::CardCharge,
        reader: DraftReader::Ocr,
        fields: serde_json::Map::new(),
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: vendor,
        doc_reference: None,
        doc_total: Some(money("81.81")),
        doc_date: Some(date("2026-11-02")),
    };
    store_reading(&mut s, tenant, other.id, &other.updated_at, &reading).expect("stored");
    let f = upload(&mut s, DraftKey::new(), "dup");
    let d = create(
        &mut s,
        tenant,
        f,
        DraftKind::CardCharge,
        IdempotencyKey::new(),
    )
    .expect("d");
    let mut c = card(&mut s, "81.81", vendor);
    c.expected_updated_at.clone_from(&d.updated_at);
    let before = s.ledger.transactions().len();
    match convert(&mut s, tenant, d.id, &c, None, IdempotencyKey::new()) {
        Err(ApiError::DocumentDraft(DraftRefusal::PossibleDuplicate {
            matches,
            draft_matches,
        })) => {
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].source, MatchSource::Transaction(t1));
            assert_eq!(matches[0].reason, MatchReason::AmountDate);
            assert_eq!(matches[0].total, Some(money("81.81")));
            assert_eq!(draft_matches.len(), 1);
            assert_eq!(draft_matches[0].source, MatchSource::Draft(other.id));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.ledger.transactions().len(), before, "nothing posted");
    // The duplicates endpoint of the other draft names the record too.
    let dupes = duplicates(&mut s, tenant, other.id).expect("dupes");
    assert!(
        dupes
            .iter()
            .any(|m| m.source == MatchSource::Transaction(t1))
    );
    c.acknowledge = vec![t1];
    let done = convert(&mut s, tenant, d.id, &c, None, IdempotencyKey::new()).expect("posted");
    assert_eq!(
        done.draft_matches.len(),
        1,
        "the other draft, informational"
    );
}

#[test]
fn plain_saves_leave_a_live_drafts_file_and_attach_takes_it() {
    let (mut s, tenant) = world();
    let key = DraftKey::new();
    let f = upload(&mut s, key, "guard");
    let d = create(&mut s, tenant, f, DraftKind::Bill, IdempotencyKey::new()).expect("d");
    // A bill saved with the same draft key keeps its hands off the file.
    let vendor = s
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == super::super::bills::STORY_VENDOR)
        .map(|c| c.id)
        .expect("vendor");
    let expense = super::super::account_id(6210);
    let bill = BillDraft {
        vendor,
        date: date("2026-11-03"),
        vendor_invoice_number: None,
        terms: None,
        due_date: None,
        memo: None,
        amount_due: money("12.00"),
        lines: vec![BillLineDraft::expense(expense, money("12.00"))],
        received: true,
        draft_attachment: Some(key),
    };
    let b = super::super::bills::create_bill(&mut s, &bill, None).expect("bill");
    assert!(super::super::checks::attachments(&s, AttachTo::Bill(b)).is_empty());
    assert_eq!(
        super::super::checks::attachments(&s, AttachTo::Draft(key)).len(),
        1
    );
    let a = attach(
        &mut s,
        tenant,
        d.id,
        b,
        &d.updated_at,
        IdempotencyKey::new(),
    )
    .expect("attached");
    assert_eq!(a.draft.kind, DraftKind::Bill, "kind unchanged");
    assert_eq!(a.draft.converted_type.as_deref(), Some("bill"));
    assert_eq!(
        super::super::checks::attachments(&s, AttachTo::Bill(b)).len(),
        1
    );
    // A second draft for the same record: its file is already there.
    assert_eq!(file_type(AttachTo::Txn(TxnType::Aje, b)), "journal_entry");
}
