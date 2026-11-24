//! The allow-listed MCP tool table, grouped per form, and what is not a tool (plan D1).
//!
//! Each [`Tool`] is one form (or list, or reading surface) and one kind:
//! - [`Kind::Read`] tools only look (`readOnlyHint: true`);
//! - [`Kind::Change`] tools create, edit, void or inactivate, picked with
//!   an `action` argument (review N4: per form, not per verb, so an agent
//!   sees tens of tools, not hundreds);
//! - [`Kind::Send`] tools email (they reach the outside world and cannot
//!   be undone, Q-A11m).
//!
//! Every [`Action`] names the switch classes it needs (Q-327, Q-329), its
//! fixed engine steps (method, route template, stale class, step kind), and
//! whether it edits a record (Edit only on stamped routes, review S11).
//! The engine refuses anything its own allow-list (U-299) does not hold, so
//! a row here can only narrow, never widen, what an agent reaches.
//!
//! [`NOT_A_TOOL`] gives the reason every other command is not offered
//! (Q-A11f, Q-332); a unit test keeps the two tables covering
//! [`cl_model::commands::COMMANDS`].

/// A switch class (Q-327, Q-329). The engine derives the same classes from
/// its rules (U-299); these decide only what the bridge offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Look at records, lists and reports.
    Read,
    /// Make new records and list entries.
    Create,
    /// Change existing ones (and reconcile, match, attach: Q-333, Q-334).
    Edit,
    /// Void, delete or inactivate.
    DeleteVoid,
    /// Email (Q-329).
    SendPrint,
}

impl Class {
    /// Every class, in switch order.
    pub const ALL: [Class; 5] = [
        Class::Read,
        Class::Create,
        Class::Edit,
        Class::DeleteVoid,
        Class::SendPrint,
    ];

    /// The words the owner sees for the switch.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Class::Read => "Read",
            Class::Create => "Create",
            Class::Edit => "Edit",
            Class::DeleteVoid => "Delete/Void",
            Class::SendPrint => "Send & print",
        }
    }
}

/// What a tool does, for its MCP hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Changes nothing.
    Read,
    /// Creates, edits, voids or inactivates in the books.
    Change,
    /// Sends email out of the books.
    Send,
}

/// The engine's stale class for a route (`desktop/route-classes.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// A read (no class).
    Read,
    /// `create`.
    Create,
    /// `stale(updatedAt)`.
    UpdatedAt,
    /// `stale(version)`.
    Version,
    /// `state-guarded`.
    Guarded,
    /// `n-a`.
    NotApplicable,
}

impl Stamp {
    /// The class as the engine's `OpenAPI` `x-stale` writes it.
    #[must_use]
    pub const fn x_stale(self) -> Option<&'static str> {
        match self {
            Stamp::Read => None,
            Stamp::Create => Some("create"),
            Stamp::UpdatedAt => Some("stale(updatedAt)"),
            Stamp::Version => Some("stale(version)"),
            Stamp::Guarded => Some("state-guarded"),
            Stamp::NotApplicable => Some("n-a"),
        }
    }

    /// A stamped route: a change made since the agent read it is refused.
    #[must_use]
    pub const fn is_stamped(self) -> bool {
        matches!(self, Stamp::UpdatedAt | Stamp::Version)
    }
}

/// How the applier resumes a step (plan D2 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// A read; never applied.
    Read,
    /// A posting or list write: recorded in the step ledger (B3).
    Ledger,
    /// Set-semantics, safe to send again (ticks, `isActive`).
    SetIdem,
    /// An email: never sent again automatically.
    Email,
}

/// One fixed engine call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// HTTP method.
    pub method: &'static str,
    /// Route template, as in the engine's `OpenAPI` (`{id}` path params).
    pub template: &'static str,
    /// The route's stale class.
    pub stamp: Stamp,
    /// How it is resumed.
    pub kind: StepKind,
}

/// One action of a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    /// The `action` argument's value.
    pub name: &'static str,
    /// Every switch class it needs (a reconciliation with charges needs
    /// Edit and Create, Q-334).
    pub classes: &'static [Class],
    /// The engine calls, in order (a composite has several).
    pub steps: &'static [Step],
    /// Changes an existing record's fields (S11: only on stamped routes).
    pub record_edit: bool,
    /// What it does, for the tool description.
    pub what: &'static str,
}

impl Action {
    /// The class shown for it: the strongest it needs.
    #[must_use]
    pub fn class(&self) -> Class {
        self.classes.iter().copied().max().unwrap_or(Class::Read)
    }
}

/// One MCP tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tool {
    /// MCP name: `[a-z0-9_]`, at most 64 characters.
    pub name: &'static str,
    /// The windows it stands for (`cl_model::commands` ids).
    pub commands: &'static [&'static str],
    /// The in-window commands its actions stand for.
    pub verbs: &'static [&'static str],
    /// Its kind.
    pub kind: Kind,
    /// One line for the agent.
    pub description: &'static str,
    /// Its actions.
    pub actions: &'static [Action],
}

impl Tool {
    /// The action called `name`.
    #[must_use]
    pub fn action(&self, name: &str) -> Option<&'static Action> {
        self.actions.iter().find(|a| a.name == name)
    }
}

/// The tool called `name`.
#[must_use]
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Every (tool, action, step) in the table, for checks against the
/// engine's `MCP_AGENT_RULES` (U-299).
pub fn routes() -> impl Iterator<Item = (&'static Tool, &'static Action, &'static Step)> {
    TOOLS.iter().flat_map(|t| {
        t.actions
            .iter()
            .flat_map(move |a| a.steps.iter().map(move |s| (t, a, s)))
    })
}

/// The read-only tools the host answers itself (no engine route).
pub const HOST_TOOLS: [&str; 1] = ["mcp_status"];

// ----- table helpers ------------------------------------------------------

const fn step(method: &'static str, template: &'static str, stamp: Stamp, kind: StepKind) -> Step {
    Step {
        method,
        template,
        stamp,
        kind,
    }
}
const fn get(template: &'static str) -> Step {
    step("GET", template, Stamp::Read, StepKind::Read)
}
const fn post_new(template: &'static str) -> Step {
    step("POST", template, Stamp::Create, StepKind::Ledger)
}
const fn put_u(template: &'static str) -> Step {
    step("PUT", template, Stamp::UpdatedAt, StepKind::Ledger)
}
const fn void_u(template: &'static str) -> Step {
    step("POST", template, Stamp::UpdatedAt, StepKind::Ledger)
}
const fn guarded(method: &'static str, template: &'static str) -> Step {
    step(method, template, Stamp::Guarded, StepKind::Ledger)
}
const fn email(template: &'static str) -> Step {
    step("POST", template, Stamp::NotApplicable, StepKind::Email)
}

const READ: &[Class] = &[Class::Read];
const CREATE: &[Class] = &[Class::Create];
const EDIT: &[Class] = &[Class::Edit];
const VOID: &[Class] = &[Class::DeleteVoid];
const SEND: &[Class] = &[Class::SendPrint];

const fn act(
    name: &'static str,
    classes: &'static [Class],
    steps: &'static [Step],
    what: &'static str,
) -> Action {
    Action {
        name,
        classes,
        steps,
        record_edit: false,
        what,
    }
}
const fn edit(steps: &'static [Step]) -> Action {
    Action {
        name: "edit",
        classes: EDIT,
        steps,
        record_edit: true,
        what: "change one (send its updatedAt; a change made since is refused)",
    }
}
const fn read_tool(
    name: &'static str,
    commands: &'static [&'static str],
    description: &'static str,
    actions: &'static [Action],
) -> Tool {
    Tool {
        name,
        commands,
        verbs: &[],
        kind: Kind::Read,
        description,
        actions,
    }
}
const fn change_tool(
    name: &'static str,
    commands: &'static [&'static str],
    verbs: &'static [&'static str],
    description: &'static str,
    actions: &'static [Action],
) -> Tool {
    Tool {
        name,
        commands,
        verbs,
        kind: Kind::Change,
        description,
        actions,
    }
}
const fn send_tool(
    name: &'static str,
    commands: &'static [&'static str],
    verbs: &'static [&'static str],
    description: &'static str,
    actions: &'static [Action],
) -> Tool {
    Tool {
        name,
        commands,
        verbs,
        kind: Kind::Send,
        description,
        actions,
    }
}

const LIST: &str = "list";
const GET: &str = "get";

macro_rules! reads {
    ($list:expr, $one:expr) => {
        &[
            act(LIST, READ, &[get($list)], "list them, 200 to a page"),
            act(GET, READ, &[get($one)], "read one by id"),
        ]
    };
    (one $one:expr) => {
        &[act(GET, READ, &[get($one)], "read one by id")]
    };
}

// The transactions router (`/transactions`) carries checks' reads, card
// charges, personal funds, journal entries, transfers, owner money, loan
// payments and deposits.
const TXN_LIST: &str = "/api/v1/transactions";
const TXN_ONE: &str = "/api/v1/transactions/{id}";
const TXN_VOID: Step = void_u("/api/v1/transactions/{id}/void");

macro_rules! txn_change {
    ($create:expr) => {
        &[
            act("create", CREATE, &[post_new($create)], "make a new one"),
            edit(&[put_u(TXN_ONE)]),
            act("void", VOID, &[TXN_VOID], "void one (kept, with zero amounts)"),
        ]
    };
}

/// Every tool. Order: transactions, lists, banking, documents, reports.
#[rustfmt::skip]
pub const TOOLS: &[Tool] = &[
    // ----- Vendors -------------------------------------------------------
    read_tool("bill_read", &["vendors.enter_bill"], "Read bills.", reads!("/api/v1/bills", "/api/v1/bills/{id}")),
    change_tool("bill_change", &[], &["edit.void_transaction"], "Enter, change or void a bill.", &[
        act("create", CREATE, &[post_new("/api/v1/bills")], "enter a new bill"),
        edit(&[put_u("/api/v1/bills/{id}")]),
        act("void", VOID, &[void_u("/api/v1/bills/{id}/void")], "void a bill"),
    ]),
    read_tool("vendor_credit_read", &[], "Read vendor credits.", reads!("/api/v1/vendor-credits", "/api/v1/vendor-credits/{id}")),
    change_tool("vendor_credit_change", &[], &["bill.kind_toggle"], "Enter, change or void a vendor credit.", &[
        act("create", CREATE, &[post_new("/api/v1/vendor-credits")], "enter a new vendor credit"),
        edit(&[put_u("/api/v1/vendor-credits/{id}")]),
        act("void", VOID, &[void_u("/api/v1/vendor-credits/{id}/void")], "void a vendor credit"),
    ]),
    read_tool("check_read", &["vendors.write_check"], "Read checks.", reads!("/api/v1/checks", TXN_ONE)),
    change_tool("check_change", &[], &[], "Write, change or void a check.", &[
        act("create", CREATE, &[post_new("/api/v1/checks")], "write a new check"),
        edit(&[put_u("/api/v1/checks/{id}")]),
        act("void", VOID, &[TXN_VOID], "void a check"),
    ]),
    read_tool("card_charge_read", &["vendors.card_charge"], "Read card charges.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("card_charge_change", &[], &[], "Record, change or void a card charge.", txn_change!(TXN_LIST)),
    read_tool("personal_funds_read", &["vendors.personal_funds"], "Read purchases paid with personal funds.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("personal_funds_change", &[], &[], "Record, change or void a purchase paid with personal funds.", txn_change!(TXN_LIST)),
    read_tool("pay_bills_read", &["vendors.pay_bills", "vendors.bill_payment"], "Read bill payments.", reads!("/api/v1/bill-payments", "/api/v1/bill-payments/{id}")),
    change_tool("pay_bills_change", &[], &[], "Pay bills (one payment per vendor) or void a bill payment.", &[
        act("create", CREATE, &[post_new("/api/v1/bill-payments")], "pay bills"),
        act("void", VOID, &[void_u("/api/v1/bill-payments/{id}/void")], "void a bill payment"),
    ]),
    read_tool("sales_tax_payment_read", &["vendors.pay_sales_tax"], "Read sales tax payments.", reads!("/api/v1/sales-tax/payments", "/api/v1/sales-tax/payments/{id}")),
    change_tool("sales_tax_payment_change", &[], &[], "Pay sales tax.", &[
        act("create", CREATE, &[post_new("/api/v1/sales-tax/payments")], "pay sales tax"),
    ]),
    read_tool("inventory_adjustment_read", &["vendors.adjust_inventory"], "Read inventory adjustments.", reads!(one "/api/v1/inventory/adjustments/{id}")),
    change_tool("inventory_adjustment_change", &[], &[], "Adjust, change or void an inventory quantity or value adjustment.", &[
        act("create", CREATE, &[post_new("/api/v1/inventory/adjustments")], "adjust quantity or value on hand"),
        edit(&[put_u("/api/v1/inventory/adjustments/{id}")]),
        act("void", VOID, &[void_u("/api/v1/inventory/adjustments/{id}/void")], "void an adjustment"),
    ]),
    change_tool("inventory_count_change", &["vendors.physical_count"], &[], "Enter a physical inventory count.", &[
        act("create", CREATE, &[post_new("/api/v1/inventory/counts")], "enter a count"),
    ]),
    // ----- Customers ------------------------------------------------------
    read_tool("invoice_read", &["clients.create_invoice"], "Read invoices.", reads!("/api/v1/invoices", "/api/v1/invoices/{id}")),
    change_tool("invoice_change", &[], &[], "Create, change or void an invoice.", &[
        act("create", CREATE, &[post_new("/api/v1/invoices")], "create a new invoice"),
        edit(&[put_u("/api/v1/invoices/{id}")]),
        act("void", VOID, &[void_u("/api/v1/invoices/{id}/void")], "void an invoice"),
    ]),
    send_tool("invoice_send", &[], &["invoice.email"], "Email an invoice. An email cannot be unsent.", &[
        act("email", SEND, &[email("/api/v1/invoices/{id}/email")], "email it now"),
    ]),
    read_tool("sales_receipt_read", &["clients.sales_receipt"], "Read sales receipts.", reads!(TXN_LIST, "/api/v1/sales-receipts/{id}")),
    change_tool("sales_receipt_change", &[], &[], "Record, change or void a sales receipt.", &[
        act("create", CREATE, &[post_new("/api/v1/sales-receipts")], "record a new sales receipt"),
        edit(&[put_u("/api/v1/sales-receipts/{id}")]),
        act("void", VOID, &[TXN_VOID], "void a sales receipt"),
    ]),
    send_tool("sales_receipt_send", &[], &[], "Email a sales receipt. An email cannot be unsent.", &[
        act("email", SEND, &[email("/api/v1/sales-receipts/{id}/email")], "email it now"),
    ]),
    read_tool("daily_sales_read", &["clients.daily_sales"], "Read daily sales summaries.", reads!("/api/v1/daily-sales/entries", "/api/v1/daily-sales/entries/{id}")),
    change_tool("daily_sales_change", &[], &[], "Enter and post, change or void a daily sales summary.", &[
        // Review S11: create is two steps, the draft then its posting.
        act("create", CREATE, &[
            post_new("/api/v1/daily-sales/entries"),
            guarded("POST", "/api/v1/daily-sales/entries/{id}/post"),
        ], "enter a summary and post it"),
        edit(&[put_u("/api/v1/daily-sales/entries/{id}")]),
        act("void", VOID, &[guarded("POST", "/api/v1/daily-sales/entries/{id}/void")], "void a posted summary"),
    ]),
    read_tool("credit_memo_read", &["clients.credit_memo"], "Read credit memos and refunds.", &[
        act(LIST, READ, &[get(TXN_LIST)], "list them, 200 to a page"),
        act(GET, READ, &[get("/api/v1/credit-memos/{id}")], "read a credit memo by id"),
        act("get_refund", READ, &[get("/api/v1/customer-refunds/{id}")], "read a refund by id"),
    ]),
    change_tool("credit_memo_change", &["clients.refund"], &["credit_memo.refund"], "Create, change or void a credit memo, or refund a credit.", &[
        act("create", CREATE, &[post_new("/api/v1/credit-memos")], "create a credit memo"),
        edit(&[put_u("/api/v1/credit-memos/{id}")]),
        act("refund", CREATE, &[post_new("/api/v1/customer-refunds")], "refund a credit"),
        act("void", VOID, &[TXN_VOID], "void a credit memo or refund"),
    ]),
    send_tool("credit_memo_send", &[], &[], "Email a credit memo. An email cannot be unsent.", &[
        act("email", SEND, &[email("/api/v1/credit-memos/{id}/email")], "email it now"),
    ]),
    read_tool("payment_read", &["clients.receive_payment"], "Read received payments.", reads!("/api/v1/payments", "/api/v1/payments/{id}")),
    change_tool("payment_change", &[], &[], "Receive, change or void a customer payment.", &[
        act("create", CREATE, &[post_new("/api/v1/payments/receive")], "receive a payment"),
        edit(&[put_u("/api/v1/payments/{id}")]),
        act("void", VOID, &[void_u("/api/v1/payments/{id}/void")], "void a payment"),
    ]),
    read_tool("finance_charge_read", &["clients.finance_charges"], "Read assessed finance charges.", &[
        act(LIST, READ, &[get("/api/v1/finance-charges")], "list them, 200 to a page"),
    ]),
    change_tool("finance_charge_change", &[], &[], "Assess finance charges.", &[
        act("create", CREATE, &[post_new("/api/v1/finance-charges/assess")], "assess finance charges"),
    ]),
    read_tool("bad_debt_recovery_read", &["clients.recover_bad_debt"], "Read bad-debt recoveries.", reads!("/api/v1/payments/write-offs", "/api/v1/payments/write-off-recoveries/{id}")),
    change_tool("bad_debt_recovery_change", &[], &[], "Record a recovered bad debt.", &[
        act("create", CREATE, &[post_new("/api/v1/payments/write-off-recoveries")], "record a recovery"),
    ]),
    read_tool("statement_read", &["clients.statements"], "Preview customer statements.", reads!("/api/v1/customer-statements", "/api/v1/customer-statements/{id}")),
    send_tool("statement_send", &[], &[], "Email a customer statement. An email cannot be unsent.", &[
        act("email", SEND, &[email("/api/v1/customer-statements/{id}/email")], "email it now"),
    ]),
    // ----- Banking and company --------------------------------------------
    read_tool("journal_entry_read", &["company.journal_entry"], "Read journal entries.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("journal_entry_change", &[], &[], "Make, change or void a journal entry.", txn_change!(TXN_LIST)),
    read_tool("deposit_read", &["banking.deposit"], "Read deposits.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("deposit_change", &[], &[], "Make, change or void a deposit.", txn_change!(TXN_LIST)),
    read_tool("transfer_read", &["banking.transfer"], "Read transfers between accounts.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("transfer_change", &[], &[], "Transfer funds, change or void a transfer.", txn_change!(TXN_LIST)),
    read_tool("owner_money_read", &["banking.owner_money"], "Read owner draws and contributions.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("owner_money_change", &[], &[], "Record, change or void an owner draw or contribution.", txn_change!(TXN_LIST)),
    read_tool("loan_payment_read", &["banking.loan_payment"], "Read loan payments.", reads!(TXN_LIST, TXN_ONE)),
    change_tool("loan_payment_change", &[], &[], "Record, change or void a loan payment.", txn_change!(TXN_LIST)),
    read_tool("register_read", &["banking.register"], "Read an account's register.", &[
        act(GET, READ, &[get("/api/v1/accounts/{id}/register")], "read an account's register"),
    ]),
    read_tool("bank_feed_read", &["banking.feed"], "Read the bank feed lines waiting for review.", &[
        act(LIST, READ, &[get("/api/v1/banking/feed")], "list the lines, 200 to a page"),
    ]),
    change_tool("bank_feed_change", &[], &["feed.accept", "feed.split", "feed.accept_selected", "feed.rule"], "Accept or split bank feed lines, or make a rule from one.", &[
        act("accept", CREATE, &[guarded("POST", "/api/v1/banking/feed/accept")], "accept lines (a split is one line with several parts)"),
        act("rule", EDIT, &[post_new("/api/v1/practice/conditional-rules")], "make a rule from a line"),
    ]),
    read_tool("reconcile_read", &["banking.reconcile"], "Read reconciliations and preview one before it starts.", &[
        act(LIST, READ, &[get("/api/v1/banking/reconciliations")], "list reconciliations"),
        act(GET, READ, &[get("/api/v1/banking/reconciliations/{id}")], "read one by id"),
        // U-300: read-only, stamped lines for the ticks (review S3).
        act("preview", READ, &[get("/api/v1/banking/reconciliations/preview")], "preview the lines to tick"),
    ]),
    change_tool("reconcile_change", &[], &["reconcile.tick", "reconcile.mark_all", "reconcile.unmark_all", "reconcile.finish"], "Reconcile an account: start, tick the cleared lines, finish with a difference of 0.00.", &[
        // Q-A11j, Q-334: one composite; charges post at Finish, so a
        // reconciliation with charges also needs Create.
        act("reconcile", EDIT, &[
            post_new("/api/v1/banking/reconciliations"),
            step("PUT", "/api/v1/banking/reconciliations/{id}/lines", Stamp::Guarded, StepKind::SetIdem),
            guarded("POST", "/api/v1/banking/reconciliations/{id}/complete"),
        ], "reconcile without service charges or interest"),
        act("reconcile_with_charges", &[Class::Create, Class::Edit], &[
            post_new("/api/v1/banking/reconciliations"),
            step("PUT", "/api/v1/banking/reconciliations/{id}/lines", Stamp::Guarded, StepKind::SetIdem),
            guarded("POST", "/api/v1/banking/reconciliations/{id}/complete"),
        ], "reconcile with service charge or interest entries"),
    ]),
    // ----- Lists ------------------------------------------------------------
    read_tool("account_read", &["lists.chart_of_accounts"], "Read the chart of accounts.", reads!("/api/v1/accounts", "/api/v1/accounts/{id}")),
    change_tool("account_change", &[], &["list.new", "list.toggle_active"], "Add, change or inactivate an account.", &[
        act("create", CREATE, &[post_new("/api/v1/accounts")], "add an account"),
        edit(&[put_u("/api/v1/accounts/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/accounts/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make an account inactive"),
    ]),
    read_tool("contact_read", &["clients.center", "vendors.center"], "Read customers, projects and vendors.", reads!("/api/v1/contacts", "/api/v1/contacts/{id}")),
    change_tool("contact_change", &[], &[], "Add, change or inactivate a customer, project or vendor.", &[
        act("create", CREATE, &[post_new("/api/v1/contacts")], "add one"),
        edit(&[put_u("/api/v1/contacts/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/contacts/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make one inactive"),
    ]),
    read_tool("item_read", &["lists.items"], "Read the item list.", reads!("/api/v1/items", "/api/v1/items/{id}")),
    change_tool("item_change", &[], &[], "Add, change or inactivate an item.", &[
        act("create", CREATE, &[post_new("/api/v1/items")], "add an item"),
        edit(&[put_u("/api/v1/items/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/items/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make an item inactive"),
    ]),
    read_tool("tag_read", &["lists.tags"], "Read tags.", reads!("/api/v1/tags", "/api/v1/tags/{id}")),
    change_tool("tag_change", &[], &[], "Add, change or inactivate a tag.", &[
        act("create", CREATE, &[post_new("/api/v1/tags")], "add a tag"),
        edit(&[put_u("/api/v1/tags/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/tags/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make a tag inactive"),
    ]),
    read_tool("price_level_read", &["lists.price_levels"], "Read price levels.", reads!("/api/v1/price-levels", "/api/v1/price-levels/{id}")),
    change_tool("price_level_change", &[], &[], "Add, change or inactivate a price level.", &[
        act("create", CREATE, &[post_new("/api/v1/price-levels")], "add a price level"),
        edit(&[put_u("/api/v1/price-levels/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/price-levels/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make a price level inactive"),
    ]),
    read_tool("unit_set_read", &["lists.unit_sets"], "Read unit sets.", reads!("/api/v1/unit-sets", "/api/v1/unit-sets/{id}")),
    change_tool("unit_set_change", &[], &[], "Add, change or inactivate a unit set.", &[
        act("create", CREATE, &[post_new("/api/v1/unit-sets")], "add a unit set"),
        edit(&[put_u("/api/v1/unit-sets/{id}")]),
        act("inactivate", VOID, &[step("PUT", "/api/v1/unit-sets/{id}", Stamp::UpdatedAt, StepKind::SetIdem)], "make a unit set inactive"),
    ]),
    read_tool("terms_read", &["lists.terms"], "Read payment terms.", &[
        act(LIST, READ, &[get("/api/v1/payment-terms")], "list the terms"),
    ]),
    // ----- Documents ------------------------------------------------------
    read_tool("document_read", &["nav.receipt_inbox", "vendors.review_documents", "nav.document_view"], "Read attached and waiting documents. Enter one with the form's own tool.", reads!("/api/v1/attachments", "/api/v1/attachments/{id}")),
    change_tool("document_change", &["nav.statement_match"], &["form.attach", "form.attach_selected"], "Attach a document to a record, or confirm a vendor statement line's match.", &[
        act("attach", EDIT, &[step("POST", "/api/v1/attachments/{id}/link", Stamp::NotApplicable, StepKind::Ledger)], "attach a document to a record"),
        act("match_statement_line", EDIT, &[guarded("POST", "/api/v1/banking/statement-lines/{lineId}/confirm")], "confirm a statement line's match"),
    ]),
    // ----- Reports, audit, status --------------------------------------------
    read_tool("report_run", &[
        "reports.center", "reports.profit_loss", "reports.balance_sheet", "reports.trial_balance",
        "reports.ar_aging", "reports.inventory_value", "reports.inventory_value_detail",
        "reports.stock_status", "reports.closing_date_exceptions", "vendors.sales_tax_owed",
        "vendors.items_to_reorder",
    ], "Run a report from the catalog.", &[
        act("catalog", READ, &[get("/api/v1/reports/catalog")], "list the reports this connection may run"),
        act("run", READ, &[get("/api/v1/reports/{report}")], "run one report"),
    ]),
    read_tool("audit_read", &["company.audit_trail"], "Read the audit trail.", &[
        act(LIST, READ, &[get("/api/v1/audit-log")], "list audit entries, 200 to a page"),
    ]),
    read_tool("mcp_status", &[], "Whether ClientLedger is open and what this connection may do.", &[
        act(GET, READ, &[], "read the status"),
        // U-300: the agent's own proposals.
        act("get_action", READ, &[get("/api/v1/mcp-actions/{id}")], "read one of this connection's changes and its state"),
    ]),
];

/// Commands that are not tools, with the reason (Q-A11f, Q-332). An entry
/// ending in `*` covers every command id with that prefix.
#[rustfmt::skip]
pub const NOT_A_TOOL: &[(&str, &str)] = &[
    // Q-332: never through MCP.
    ("company.users", "users and roles are never changed by MCP (Q-332)"),
    ("company.closing_date", "the closing date and its password are never available to MCP (Q-332)"),
    ("file.backup", "backups are never run by MCP (Q-332)"),
    ("file.backup_settings", "backups are never run by MCP (Q-332)"),
    ("file.restore", "restores are never run by MCP (Q-332)"),
    ("file.full_backup", "backups are never run by MCP (Q-332)"),
    ("file.delete_company", "company delete is never run by MCP (Q-332)"),
    // Q-A11f: never tools.
    ("banking.import_file", "imports are not MCP tools (Q-A11f)"),
    ("file.import_transactions", "imports are not MCP tools (Q-A11f)"),
    ("file.import_qbd", "imports are not MCP tools (Q-A11f)"),
    ("file.import_system", "imports are not MCP tools (Q-A11f)"),
    ("lists.import_export", "imports are not MCP tools (Q-A11f)"),
    ("payroll.import", "payroll import is not an MCP tool (Q-A11f)"),
    ("banking.bulk_entry", "bulk entry is not an MCP tool; use each form's tool (Q-A11f)"),
    ("lists.templates", "templates are not MCP tools (Q-A11f)"),
    ("lists.templates_due", "templates are not MCP tools (Q-A11f)"),
    ("company.information", "company information and settings are not MCP tools (Q-A11f)"),
    ("edit.preferences", "company information and settings are not MCP tools (Q-A11f)"),
    ("lists.sales_tax", "sales tax setup is company settings (Q-A11f)"),
    ("company.send_to_accountant", "send to accountant is not an MCP tool (Q-A11f)"),
    ("file.export_all", "exports are not MCP tools (Q-A11f)"),
    ("file.export_iif", "exports are not MCP tools (Q-A11f)"),
    ("file.verify_data", "verify data is not an MCP tool (Q-A11f)"),
    ("company.lan_setup", "LAN setup is not an MCP tool (Q-A11f)"),
    ("company.my_sign_in", "my sign-in is not an MCP tool (Q-A11f)"),
    // No engine write route for the list (reads come with `terms_read`).
    ("lists.payment_methods", "the engine has no payment method routes an agent may use"),
    // App-only windows and windows whose work another tool does.
    ("file.open_company", "the app opens companies; MCP works in the open one (Q-A11h)"),
    ("file.new_company", "the app makes companies"),
    ("file.close_company", "the app closes companies"),
    ("file.open_practice", "the app opens the practice company"),
    ("file.sign_out", "sign-in belongs to the person at the app"),
    ("file.lock", "sign-in belongs to the person at the app"),
    ("file.print", "printing is done by a person (Q-A11i)"),
    ("file.print_forms", "printing is done by a person (Q-A11i)"),
    ("file.printer_setup", "printer setup belongs to the person at the app"),
    ("file.close_window", "window management"),
    ("file.quit", "window management"),
    ("clients.outbox", "the outbox shows emails a person queued; sending goes through each form's send tool"),
    ("clients.tax_exemptions", "exemption certificates are company sales tax setup (Q-A11f)"),
    ("reports.memorized_list", "memorized reports belong to the person at the app; run reports with report_run"),
    ("reports.report_file", "report files are imports and exports (Q-A11f)"),
    ("reports.builder_editor", "the report builder belongs to the person at the app"),
    ("nav.home", "Home is a view of the app"),
    // In-window commands that are not actions of their own.
    ("edit.*", "editing keys inside a window; the change tools do the work"),
    ("form.*", "form keys inside a window; the change tools do the work"),
    ("batch.*", "bulk entry is not an MCP tool (Q-A11f)"),
    ("grid.*", "grid keys inside a window"),
    ("bill.*", "form keys inside Enter Bill; bill_change and document_read do the work"),
    ("check.*", "printing is done by a person (Q-A11i)"),
    ("invoice.*", "form keys inside Create Invoice; invoice_change does the work"),
    ("payment.*", "form keys inside Receive Payment; payment_change does the work"),
    ("credit_memo.*", "form keys inside a credit memo; credit_memo_change does the work"),
    ("viewer.*", "document viewer keys"),
    ("queue.*", "Review Documents keys; document_read does the work"),
    ("feed.*", "bank feed keys; bank_feed_change does the work"),
    ("reconcile.*", "reconcile keys; reconcile_change does the work"),
    ("builder.*", "report builder keys"),
    ("list.*", "list window keys; each list's change tool does the work"),
    ("register.*", "register display keys"),
    ("reports.*", "report window keys; report_run does the work"),
    ("window.*", "window management"),
    ("view.*", "window management"),
    ("help.*", "help belongs to the person at the app"),
];

/// Why `command` is not a tool, if it is not one.
#[must_use]
pub fn not_a_tool(command: &str) -> Option<&'static str> {
    NOT_A_TOOL
        .iter()
        .find(|(id, _)| match id.strip_suffix('*') {
            Some(prefix) => command.starts_with(prefix),
            None => *id == command,
        })
        .map(|(_, why)| *why)
}

/// The tools that stand for `command` (as its window or one of its verbs).
pub fn tools_for(command: &str) -> impl Iterator<Item = &'static Tool> + '_ {
    TOOLS
        .iter()
        .filter(move |t| t.commands.contains(&command) || t.verbs.contains(&command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::commands::COMMANDS;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_short_and_safe_for_every_client() {
        let mut seen = HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "{} twice", t.name);
            assert!(!t.name.is_empty() && t.name.len() <= 64, "{}", t.name);
            assert!(
                t.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{}",
                t.name
            );
            assert!(!t.actions.is_empty(), "{} has no action", t.name);
            let mut names = HashSet::new();
            for a in t.actions {
                assert!(names.insert(a.name), "{}.{} twice", t.name, a.name);
                assert!(!a.classes.is_empty(), "{}.{}", t.name, a.name);
            }
        }
    }

    #[test]
    fn every_command_is_a_tool_or_says_why_not() {
        for c in COMMANDS {
            let id = c.id.as_str();
            let tool = tools_for(id).next().is_some();
            let why = not_a_tool(id);
            assert!(tool || why.is_some(), "{id} is neither a tool nor in NOT_A_TOOL");
            // An exact NOT_A_TOOL entry and a tool contradict each other; a
            // prefix entry may cover verbs a tool names.
            let exact = NOT_A_TOOL.iter().any(|(n, _)| *n == id);
            assert!(!(tool && exact), "{id} is a tool and in NOT_A_TOOL");
        }
    }

    #[test]
    fn every_named_command_exists() {
        let ids: HashSet<&str> = COMMANDS.iter().map(|c| c.id.as_str()).collect();
        for t in TOOLS {
            for c in t.commands.iter().chain(t.verbs) {
                assert!(ids.contains(c), "{} names unknown command {c}", t.name);
            }
        }
        for (id, _) in NOT_A_TOOL {
            if let Some(prefix) = id.strip_suffix('*') {
                assert!(
                    ids.iter().any(|c| c.starts_with(prefix)),
                    "NOT_A_TOOL prefix {id} matches nothing"
                );
            } else {
                assert!(ids.contains(id), "NOT_A_TOOL names unknown command {id}");
            }
        }
    }

    #[test]
    fn q332_commands_are_never_tools() {
        for id in [
            "company.users",
            "company.closing_date",
            "file.backup",
            "file.restore",
            "file.delete_company",
        ] {
            assert!(tools_for(id).next().is_none(), "{id}");
            assert!(not_a_tool(id).is_some_and(|w| w.contains("Q-332")), "{id}");
        }
        // And no route reaches them.
        for (t, a, s) in routes() {
            for bad in [
                "/closing",
                "/users",
                "/backup",
                "/restore",
                "/mcp-connections",
                "/mcp-settings",
                "/permission",
                "/tb/",
            ] {
                assert!(!s.template.contains(bad), "{}.{} {}", t.name, a.name, s.template);
            }
        }
    }

    #[test]
    fn kinds_match_their_actions() {
        for t in TOOLS {
            for a in t.actions {
                match t.kind {
                    Kind::Read => {
                        assert_eq!(a.classes, READ, "{}.{}", t.name, a.name);
                        assert!(a.steps.iter().all(|s| s.method == "GET"), "{}.{}", t.name, a.name);
                    }
                    Kind::Change => {
                        assert!(!a.classes.contains(&Class::Read), "{}.{}", t.name, a.name);
                        assert!(!a.classes.contains(&Class::SendPrint), "{}.{}", t.name, a.name);
                        assert!(a.steps.iter().all(|s| s.method != "GET" && s.kind != StepKind::Email));
                    }
                    Kind::Send => {
                        assert_eq!(a.classes, SEND, "{}.{}", t.name, a.name);
                        assert!(a.steps.iter().all(|s| s.kind == StepKind::Email));
                    }
                }
                if t.kind != Kind::Read || t.name != "mcp_status" {
                    assert!(!a.steps.is_empty(), "{}.{} has no step", t.name, a.name);
                }
            }
        }
    }

    /// Review S11: Edit only on stamped routes. The Edit-class actions that
    /// are not record edits (they act on state-guarded or link routes, which
    /// carry their own checks) are listed by name.
    #[test]
    fn record_edits_use_only_stamped_routes() {
        const NOT_RECORD_EDITS: &[&str] = &[
            "bank_feed_change.rule",
            "reconcile_change.reconcile",
            "reconcile_change.reconcile_with_charges",
            "document_change.attach",
            "document_change.match_statement_line",
        ];
        for t in TOOLS {
            for a in t.actions {
                let key = format!("{}.{}", t.name, a.name);
                if a.record_edit {
                    assert!(a.classes == EDIT, "{key}");
                    assert!(
                        a.steps.iter().all(|s| s.stamp.is_stamped()),
                        "{key} edits on an unstamped route"
                    );
                } else if a.classes.contains(&Class::Edit) {
                    assert!(NOT_RECORD_EDITS.contains(&key.as_str()), "{key}");
                }
            }
        }
    }

    #[test]
    fn steps_have_consistent_kinds() {
        for (t, a, s) in routes() {
            let key = format!("{}.{} {} {}", t.name, a.name, s.method, s.template);
            assert!(s.template.starts_with("/api/v1/"), "{key}");
            match s.kind {
                StepKind::Read => assert_eq!(s.stamp, Stamp::Read, "{key}"),
                StepKind::Email => assert_eq!(s.stamp, Stamp::NotApplicable, "{key}"),
                StepKind::Ledger | StepKind::SetIdem => {
                    assert_ne!(s.stamp, Stamp::Read, "{key}");
                    assert_ne!(s.method, "GET", "{key}");
                }
            }
        }
    }

    /// The stale classes declared here are the engine's (`x-stale` in its
    /// `OpenAPI` export). Reads `engine/openapi.json`, or the file named by
    /// `CL_ENGINE_OPENAPI`; routes the pin does not have yet (U-300's
    /// `preview` and `mcp-actions`) are named.
    #[test]
    fn stale_classes_match_the_engines_export() {
        const NOT_AT_PIN: &[&str] = &[
            "/api/v1/banking/reconciliations/preview",
            "/api/v1/mcp-actions/{id}",
            "/api/v1/reports/{report}",
        ];
        let path = std::env::var_os("CL_ENGINE_OPENAPI").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engine/openapi.json"),
            std::path::PathBuf::from,
        );
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("NOTE: {} is not here (engine submodule not checked out)", path.display());
            return;
        };
        let spec: serde_json::Value = serde_json::from_str(&text).expect("openapi.json is JSON");
        for (t, a, s) in routes() {
            if NOT_AT_PIN.contains(&s.template) {
                continue;
            }
            let op = &spec["paths"][s.template][s.method.to_ascii_lowercase()];
            assert!(op.is_object(), "{}.{}: {} {} is not in the engine", t.name, a.name, s.method, s.template);
            assert_eq!(
                op.get("x-stale").and_then(serde_json::Value::as_str),
                s.stamp.x_stale(),
                "{}.{}: {} {}",
                t.name,
                a.name,
                s.method,
                s.template
            );
        }
    }
}
