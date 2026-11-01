//! The command registry: every action the owner can take from a menu, the
//! left rail, the command palette (Ctrl+K) or the keyboard.
//!
//! One table feeds all of them (CLAUDE.md UI standards), so a menu item, a
//! rail entry, a palette row and a shortcut can never disagree. A command that
//! opens a window carries `opens`; the window manager keys windows by command
//! id. A command whose screen is built in a later phase carries `arrives`, and
//! the shell opens a "coming in a later release" page for it until then.
//!
//! `label` is the English catalog text. `cl-ui::i18n` resolves `cmd.<id>`
//! through it so a later locale can override it without touching this table.

use std::fmt;

use crate::keymap::{Key, Mods, Scope, Shortcut};

/// A stable command id such as `vendors.write_check`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(&'static str);

impl CommandId {
    /// Wrap an id.
    #[must_use]
    pub const fn new(id: &'static str) -> CommandId {
        CommandId(id)
    }

    /// The id text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommandId({})", self.0)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The menu-bar menus (DESIGN.md window anatomy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Menu {
    /// File.
    File,
    /// Edit.
    Edit,
    /// Lists.
    Lists,
    /// Clients.
    Clients,
    /// Vendors.
    Vendors,
    /// Banking.
    Banking,
    /// Payroll (import only).
    Payroll,
    /// Reports.
    Reports,
    /// Company.
    Company,
    /// Window (tabs, zoom, theme).
    Window,
    /// Help.
    Help,
}

impl Menu {
    /// Menus in bar order.
    pub const ALL: &'static [Menu] = &[
        Menu::File,
        Menu::Edit,
        Menu::Lists,
        Menu::Clients,
        Menu::Vendors,
        Menu::Banking,
        Menu::Payroll,
        Menu::Reports,
        Menu::Company,
        Menu::Window,
        Menu::Help,
    ];

    /// English title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::Lists => "Lists",
            Menu::Clients => "Customers",
            Menu::Vendors => "Vendors",
            Menu::Banking => "Banking",
            Menu::Payroll => "Payroll",
            Menu::Reports => "Reports",
            Menu::Company => "Company",
            Menu::Window => "Window",
            Menu::Help => "Help",
        }
    }
}

/// One registered command.
#[derive(Debug, Clone, Copy)]
pub struct Command {
    /// Stable id.
    pub id: CommandId,
    /// English label.
    pub label: &'static str,
    /// Menu it appears in, if any.
    pub menu: Option<Menu>,
    /// Entries with different groups get a separator between them.
    pub group: u8,
    /// Keyboard shortcut, if any.
    pub shortcut: Option<Shortcut>,
    /// Where the shortcut is active.
    pub scope: Scope,
    /// Listed in the palette. Always true in the shipped table.
    pub palette: bool,
    /// Opens a window (tab) keyed by this command id.
    pub opens: bool,
    /// Phase that builds the real screen; `None` when it exists now.
    pub arrives: Option<&'static str>,
}

#[allow(clippy::unnecessary_wraps)] // table helpers: `Option` matches the field type
const fn ctrl(c: char) -> Option<Shortcut> {
    Some(Shortcut::new(Mods::CMD, Key::Letter(c)))
}
#[allow(clippy::unnecessary_wraps)]
const fn ctrl_shift(c: char) -> Option<Shortcut> {
    Some(Shortcut::new(Mods::CMD_SHIFT, Key::Letter(c)))
}
#[allow(clippy::unnecessary_wraps)]
const fn alt(c: char) -> Option<Shortcut> {
    Some(Shortcut::new(Mods::ALT, Key::Letter(c)))
}
#[allow(clippy::unnecessary_wraps)]
const fn key(mods: Mods, k: Key) -> Option<Shortcut> {
    Some(Shortcut::new(mods, k))
}

const fn cmd(
    id: &'static str,
    label: &'static str,
    menu: Option<Menu>,
    group: u8,
    shortcut: Option<Shortcut>,
    scope: Scope,
) -> Command {
    Command {
        id: CommandId::new(id),
        label,
        menu,
        group,
        shortcut,
        scope,
        palette: true,
        opens: false,
        arrives: None,
    }
}

/// A command that opens a window whose screen arrives in `phase`.
const fn screen(
    id: &'static str,
    label: &'static str,
    menu: Option<Menu>,
    group: u8,
    shortcut: Option<Shortcut>,
    phase: &'static str,
) -> Command {
    let mut c = cmd(id, label, menu, group, shortcut, Scope::Global);
    c.opens = true;
    c.arrives = Some(phase);
    c
}

/// A command that opens a window whose screen exists now.
const fn built(
    id: &'static str,
    label: &'static str,
    menu: Option<Menu>,
    group: u8,
    shortcut: Option<Shortcut>,
) -> Command {
    let mut c = cmd(id, label, menu, group, shortcut, Scope::Global);
    c.opens = true;
    c
}

/// Rail entries, by command id (DESIGN.md window anatomy item 3).
pub struct RailLayout {
    /// "Workspace" section.
    pub rail_workspace: &'static [&'static str],
    /// "Tools" section.
    pub rail_tools: &'static [&'static str],
}

/// The left rail. Accountant tools are post-beta and have no entry.
pub const ALL: RailLayout = RailLayout {
    rail_workspace: &[
        "nav.home",
        "clients.center",
        "vendors.center",
        "banking.feed",
        "lists.chart_of_accounts",
        "reports.center",
    ],
    rail_tools: &[
        "banking.import_file",
        "payroll.import",
        "company.send_to_accountant",
        "company.audit_trail",
    ],
};

/// Every command. Order is menu order within each menu.
#[rustfmt::skip]
pub const COMMANDS: &[Command] = &[
    // File
    built("file.open_company", "Open or Switch Company", Some(Menu::File), 0, ctrl('O')),
    built("file.new_company", "New Company", Some(Menu::File), 0, None),
    cmd("file.close_company", "Close Company", Some(Menu::File), 0, None, Scope::Global),
    // P19 (Q-P19ao): close the company the normal way and open the practice company.
    cmd("file.open_practice", "Open Practice Company", Some(Menu::File), 0, None, Scope::Global),
    built("file.backup", "Back Up Company", Some(Menu::File), 1, None),
    cmd("file.restore", "Restore a Backup", Some(Menu::File), 1, None, Scope::Global),
    built("file.export_all", "Export All Data", Some(Menu::File), 1, None),
    screen("file.full_backup", "Full Database Backup (Utilities)", Some(Menu::File), 1, None, "P34"),
    built("file.delete_company", "Delete Company", Some(Menu::File), 2, None),
    cmd("file.print", "Print", Some(Menu::File), 3, ctrl('P'), Scope::Global),
    // P19: the print queues and the printer's alignment (no shortcut)
    built("file.print_forms", "Print Forms", Some(Menu::File), 3, None),
    built("file.printer_setup", "Printer Setup", Some(Menu::File), 3, None),
    cmd("file.close_window", "Close Window", Some(Menu::File), 4, key(Mods::CMD, Key::F(4)), Scope::Global),
    cmd("file.quit", "Quit ClientLedger", Some(Menu::File), 5, ctrl('Q'), Scope::Global),
    // Edit
    cmd("edit.undo", "Undo", Some(Menu::Edit), 0, ctrl('Z'), Scope::Form),
    cmd("edit.redo", "Redo", Some(Menu::Edit), 0, ctrl('Y'), Scope::Form),
    cmd("edit.find", "Find / Go To", Some(Menu::Edit), 1, ctrl('F'), Scope::Global),
    cmd("edit.edit_transaction", "Edit Transaction", Some(Menu::Edit), 1, ctrl('E'), Scope::Window),
    cmd("edit.void_transaction", "Void Transaction", Some(Menu::Edit), 1, None, Scope::Window),
    cmd("edit.open_list", "Open List for This Field", Some(Menu::Edit), 1, ctrl('L'), Scope::Form),
    built("edit.preferences", "Preferences", Some(Menu::Edit), 2, key(Mods::CMD, Key::Comma)),
    // Form and grid commands (not in a menu; listed in the palette and overlay)
    cmd("form.save_new", "Save & New", None, 0, alt('N'), Scope::Form),
    cmd("form.save_close", "Save & Close", None, 0, alt('S'), Scope::Form),
    cmd("form.revert", "Revert Field, Then Form", None, 0, key(Mods::NONE, Key::Escape), Scope::Form),
    cmd("form.next_record", "Next Record", None, 1, key(Mods::NONE, Key::PageDown), Scope::Form),
    cmd("form.prev_record", "Previous Record", None, 1, key(Mods::NONE, Key::PageUp), Scope::Form),
    cmd("form.insert_line", "Insert Line", None, 2, key(Mods::CMD, Key::Insert), Scope::Form),
    cmd("form.delete_line", "Delete Line", None, 2, key(Mods::CMD, Key::Delete), Scope::Form),
    cmd("form.delete_item", "Delete Transaction", None, 2, ctrl('D'), Scope::Form),
    cmd("form.attach", "Attach\u{2026}", None, 3, None, Scope::Form),
    cmd("form.history", "History", None, 3, None, Scope::Form),
    // Save as Template (P15, Q-P15k): every form that can be a template
    cmd("form.save_as_template", "Save as Template", None, 3, ctrl('M'), Scope::Form),
    // Write Check (P13; Page Up/Down, Ctrl+F, Ctrl+P, Alt+N and Alt+S above apply too)
    cmd("check.print_later", "Print Later", None, 4, None, Scope::Form),
    // Bulk Entry (P15A, Q-P15Ah; Alt+S and Alt+N above save all)
    cmd("batch.next_problem", "Next Row That Needs Attention", None, 4, key(Mods::NONE, Key::F(8)), Scope::Form),
    cmd("batch.prev_problem", "Previous Row That Needs Attention", None, 4, key(Mods::SHIFT, Key::F(8)), Scope::Form),
    cmd("batch.next_section", "Next Section", None, 4, key(Mods::CMD, Key::PageDown), Scope::Form),
    cmd("batch.prev_section", "Previous Section", None, 4, key(Mods::CMD, Key::PageUp), Scope::Form),
    // Bulk Entry columns and selection actions (P16, Q-274; one undo step each)
    cmd("batch.columns", "Columns\u{2026}", None, 4, None, Scope::Form),
    cmd("batch.set_account", "Set Account", None, 5, None, Scope::Grid),
    cmd("batch.set_name", "Set Name", None, 5, None, Scope::Grid),
    cmd("batch.set_tag", "Set Tag", None, 5, None, Scope::Grid),
    cmd("batch.set_date", "Set Date", None, 5, None, Scope::Grid),
    cmd("batch.set_memo", "Set Memo", None, 5, None, Scope::Grid),
    cmd("batch.set_print_later", "Set Print Later", None, 5, None, Scope::Grid),
    cmd("batch.delete_rows", "Delete Rows", None, 5, None, Scope::Grid),
    // Grids that opted in to grid commands (P16, Q-272, Q-274). The Grid
    // scope sits inside Form, so Ctrl+D copies down here and deletes the
    // transaction in a form.
    cmd("grid.copy_down", "Copy Down", None, 6, ctrl('D'), Scope::Grid),
    cmd("grid.copy_down_below", "Copy to Rows Below", None, 6, None, Scope::Grid),
    cmd("grid.select_all", "Select All Rows", None, 6, ctrl('A'), Scope::Grid),
    cmd("grid.clear_selection", "Clear Selection", None, 6, None, Scope::Grid),
    // Enter Bill (P17; Page Up/Down, Alt+N, Alt+S, Ctrl+M and Attach above apply too)
    cmd("bill.kind_toggle", "Switch Bill or Vendor Credit", None, 7, None, Scope::Form),
    cmd("bill.read_document", "Read with AI", None, 7, None, Scope::Form),
    // A03: read the shown document on this computer (no AI, nothing sent)
    cmd("bill.read_local", "Read Document", None, 7, None, Scope::Form),
    // A04: Accept on every form that takes a reading (Enter Bill, Record
    // Card Charge, Write Check); was `bill.accept_read` before A04.
    cmd("form.accept_read", "Accept These Values", None, 7, alt('A'), Scope::Form),
    // A04 draft mode (D2, D3): a document's form. Attach to selected and
    // Enter as new act only while the attach dialog is up; Enter there
    // never attaches (Q-A04e).
    cmd("form.attach_selected", "Attach to Selected Transaction", None, 7, alt('T'), Scope::Form),
    cmd("form.enter_as_new", "Enter as New", None, 7, alt('E'), Scope::Form),
    cmd("form.discard_document", "Discard Document\u{2026}", None, 7, None, Scope::Form),
    // Create Invoice (P20; Page Up/Down, Alt+N, Alt+S, Ctrl+P and Attach above apply too).
    // In a form Ctrl+Shift+B shadows Pay Bills. Email is Ctrl+Shift+M, not the
    // plan's Ctrl+E: Ctrl+E is Edit Transaction in the register entry row,
    // which is Form scope too (P12; Q-P20aa).
    cmd("invoice.add_billable", "Add Billable Costs\u{2026}", None, 7, ctrl_shift('U'), Scope::Form),
    cmd("invoice.email", "Email\u{2026}", None, 7, ctrl_shift('M'), Scope::Form),
    // Receive Payment (P21; Alt+N, Alt+S, Page Up/Down apply too). Record
    // bounced check has no shortcut: the toolbar and the Edit menu.
    cmd("payment.unapply_all", "Un-apply All", None, 7, alt('U'), Scope::Form),
    cmd("payment.discounts_credits", "Discounts & Credits\u{2026}", None, 7, alt('D'), Scope::Form),
    cmd("payment.bounced_check", "Record Bounced Check\u{2026}", None, 7, None, Scope::Form),
    // Credit memo toolbar (P22, no shortcut).
    cmd("credit_memo.apply", "Apply Credit\u{2026}", None, 7, None, Scope::Form),
    cmd("credit_memo.refund", "Refund\u{2026}", None, 7, None, Scope::Form),
    // Document preview (P17): the source-document card and its full-size tab
    cmd("viewer.zoom_in", "Zoom In on the Document", None, 8, key(Mods::CMD, Key::Equals), Scope::Viewer),
    cmd("viewer.zoom_out", "Zoom Out on the Document", None, 8, key(Mods::CMD, Key::Minus), Scope::Viewer),
    cmd("viewer.fit", "Fit the Document", None, 8, key(Mods::CMD, Key::Digit(0)), Scope::Viewer),
    cmd("viewer.page_next", "Next Page", None, 8, key(Mods::NONE, Key::PageDown), Scope::Viewer),
    cmd("viewer.page_prev", "Previous Page", None, 8, key(Mods::NONE, Key::PageUp), Scope::Viewer),
    // Rotate is view only and has no shortcut (A02, Q-A02b, Q-A02m: Ctrl+R
    // would shadow Receive Payment while the page has focus).
    cmd("viewer.rotate_cw", "Rotate the Page Right", None, 8, None, Scope::Viewer),
    cmd("viewer.rotate_ccw", "Rotate the Page Left", None, 8, None, Scope::Viewer),
    // A04 compare tools (D4): step through the page's words and fill the
    // armed field from one (Q-A04c). Unmodified arrows stay unbound.
    cmd("viewer.next_word", "Next Word on the Page", None, 8, key(Mods::ALT, Key::Right), Scope::Viewer),
    cmd("viewer.prev_word", "Previous Word on the Page", None, 8, key(Mods::ALT, Key::Left), Scope::Viewer),
    cmd("viewer.fill_word", "Fill the Field From This Word", None, 8, key(Mods::NONE, Key::Enter), Scope::Viewer),
    cmd("viewer.toggle_highlights", "Show or Hide Highlights", None, 8, None, Scope::Viewer),
    // A04 Review Documents queue (D6): the list's own keys. Alt+S is not
    // bound here (Q-A04b): accepting happens in the form.
    cmd("queue.next", "Next Document", None, 9, key(Mods::NONE, Key::Letter('J')), Scope::List),
    cmd("queue.prev", "Previous Document", None, 9, key(Mods::NONE, Key::Letter('K')), Scope::List),
    cmd("queue.open", "Open the Document", None, 9, key(Mods::NONE, Key::Enter), Scope::List),
    cmd("queue.discard", "Discard the Document\u{2026}", None, 9, key(Mods::NONE, Key::Delete), Scope::List),
    cmd("queue.read_all", "Read All Documents", None, 9, None, Scope::List),
    // Lists
    built("lists.chart_of_accounts", "Chart of Accounts", Some(Menu::Lists), 0, ctrl('A')),
    built("lists.items", "Item List", Some(Menu::Lists), 0, None),
    built("lists.price_levels", "Price Levels", Some(Menu::Lists), 0, None),
    built("lists.tags", "Tags", Some(Menu::Lists), 0, None),
    built("lists.terms", "Terms", Some(Menu::Lists), 1, None),
    built("lists.payment_methods", "Payment Methods", Some(Menu::Lists), 1, None),
    built("lists.templates", "Templates", Some(Menu::Lists), 1, None),
    // Templates Due (P15): the catch-up runner opens it; no menu entry (the
    // palette still finds it, like every command)
    built("lists.templates_due", "Templates Due", None, 1, None),
    built("lists.import_export", "Import or Export a List", Some(Menu::Lists), 2, None),
    // List windows (Chart of Accounts first; the other lists use the same keys)
    cmd("list.new", "New List Entry", None, 3, ctrl('N'), Scope::Window),
    cmd("list.toggle_active", "Make Inactive or Active Again", None, 3, ctrl_shift('I'), Scope::Window),
    cmd("list.show_inactive", "Include Inactive Entries", None, 3, ctrl_shift('H'), Scope::Window),
    cmd("list.quick_report", "Account or Transaction Report", None, 3, ctrl_shift('Q'), Scope::Window),
    cmd("list.merge", "Merge Into Another Entry", None, 3, None, Scope::Window),
    // Delete the selected entry, after a confirmation (P15: Templates). A
    // text field keeps its own Delete key.
    cmd("list.delete", "Delete List Entry", None, 3, key(Mods::NONE, Key::Delete), Scope::Window),
    // Register windows (P12; Ctrl+F, Ctrl+E, Ctrl+D and Ctrl+Shift+Q above apply too)
    cmd("register.one_line", "One-Line Display", None, 4, None, Scope::Window),
    // Clients
    built("clients.center", "Customers & Projects", Some(Menu::Clients), 0, None),
    built("clients.create_invoice", "Create Invoice", Some(Menu::Clients), 1, ctrl('I')),
    built("clients.receive_payment", "Receive Payment", Some(Menu::Clients), 1, ctrl('R')),
    built("clients.sales_receipt", "Sales Receipt", Some(Menu::Clients), 1, ctrl_shift('R')),
    built("clients.daily_sales", "Daily Sales Summary", Some(Menu::Clients), 1, None),
    built("clients.credit_memo", "Credit Memo or Refund", Some(Menu::Clients), 2, None),
    built("clients.statements", "Statements", Some(Menu::Clients), 2, None),
    // P22: the Refund screen (D5) and Assess Finance Charges (D7), no
    // shortcut.
    built("clients.refund", "Refund a Credit", Some(Menu::Clients), 2, None),
    built("clients.finance_charges", "Assess Finance Charges", Some(Menu::Clients), 2, None),
    built("clients.outbox", "Outbox", Some(Menu::Clients), 2, None),
    // Vendors
    built("vendors.center", "Vendors", Some(Menu::Vendors), 0, None),
    built("vendors.enter_bill", "Enter Bill", Some(Menu::Vendors), 1, ctrl('B')),
    built("vendors.pay_bills", "Pay Bills", Some(Menu::Vendors), 1, ctrl_shift('B')),
    built("vendors.write_check", "Write Check", Some(Menu::Vendors), 1, ctrl('W')),
    built("vendors.card_charge", "Record Card Charge", Some(Menu::Vendors), 1, ctrl_shift('C')),
    built("vendors.personal_funds", "Paid with Personal Funds", Some(Menu::Vendors), 1, None),
    // P17: the receipt inbox (the menu shows its count, Q-P17t) and the
    // vendor statement match; no shortcut
    built("nav.receipt_inbox", "Receipt Inbox", Some(Menu::Vendors), 2, None),
    built("nav.statement_match", "Match Vendor Statement\u{2026}", Some(Menu::Vendors), 2, None),
    // A04: the company-wide queue of documents waiting to be entered (D6);
    // Home and the receipt inbox open it too. No shortcut.
    built("vendors.review_documents", "Review Documents", Some(Menu::Vendors), 2, None),
    // P17: a document in its own tab ("Open full size"); no menu entry
    built("nav.document_view", "Document Preview", None, 2, None),
    // P18: a bill payment, read-only with Void (OpenRecord, Q-P18f); no
    // menu entry
    built("vendors.bill_payment", "Bill Payment", None, 2, None),
    // Banking
    built("banking.register", "Open Register", Some(Menu::Banking), 0, ctrl('G')),
    built("banking.deposit", "Make Deposit", Some(Menu::Banking), 0, ctrl('D')),
    built("banking.transfer", "Transfer Funds", Some(Menu::Banking), 0, ctrl('T')),
    built("banking.owner_money", "Pay Yourself or Put Money In", Some(Menu::Banking), 0, None),
    built("banking.loan_payment", "Loan Payment", Some(Menu::Banking), 0, None),
    // Bulk Entry (P15A): no global shortcut (Q-P15Ah)
    built("banking.bulk_entry", "Bulk Entry", Some(Menu::Banking), 0, None),
    screen("banking.reconcile", "Reconcile", Some(Menu::Banking), 1, ctrl_shift('E'), "P27"),
    screen("banking.import_file", "Import Bank File", Some(Menu::Banking), 2, None, "P25"),
    screen("banking.feed", "Bank Feed", Some(Menu::Banking), 2, None, "P26"),
    // Payroll
    screen("payroll.import", "Import Payroll", Some(Menu::Payroll), 0, None, "P30"),
    // Reports
    screen("reports.center", "Reports", Some(Menu::Reports), 0, None, "P28"),
    screen("reports.profit_loss", "Profit & Loss", Some(Menu::Reports), 1, None, "P28"),
    screen("reports.balance_sheet", "Balance Sheet", Some(Menu::Reports), 1, None, "P28"),
    screen("reports.trial_balance", "Trial Balance", Some(Menu::Reports), 1, ctrl_shift('T'), "P28"),
    // Company
    built("company.journal_entry", "Make Journal Entry", Some(Menu::Company), 0, ctrl('J')),
    // P19: logo, form footers and document numbering (no shortcut)
    built("company.information", "Company Information", Some(Menu::Company), 0, None),
    screen("company.closing_date", "Set Closing Date", Some(Menu::Company), 1, None, "P29"),
    screen("company.users", "Users and Permissions", Some(Menu::Company), 1, None, "P29"),
    screen("company.send_to_accountant", "Send to My Accountant", Some(Menu::Company), 2, None, "P32"),
    screen("company.audit_trail", "Audit Trail", Some(Menu::Company), 2, None, "P29"),
    // Window
    built("nav.home", "Home", Some(Menu::Window), 0, alt('H')),
    cmd("window.next", "Next Window", Some(Menu::Window), 1, key(Mods::CMD, Key::Tab), Scope::Global),
    cmd("window.prev", "Previous Window", Some(Menu::Window), 1, key(Mods::CMD_SHIFT, Key::Tab), Scope::Global),
    cmd("window.pop_out", "Move Window to Its Own Window", Some(Menu::Window), 1, ctrl_shift('O'), Scope::Window),
    // Collapse or expand a screen's detail panel (P16, Q-266)
    cmd("window.toggle_details", "Show or Hide Details", Some(Menu::Window), 1, key(Mods::NONE, Key::F(9)), Scope::Window),
    // The pop-out document viewer (A02, DESIGN.md keyboard map)
    cmd("view.toggle_doc_viewer", "Pop Out or Dock the Document", Some(Menu::Window), 1, ctrl_shift('D'), Scope::Global),
    cmd("view.focus_doc", "Move Between the Form and Its Document", Some(Menu::Window), 1, key(Mods::NONE, Key::F(6)), Scope::Window),
    cmd("window.zoom_in", "Zoom In", Some(Menu::Window), 2, key(Mods::CMD, Key::Equals), Scope::Global),
    cmd("window.zoom_out", "Zoom Out", Some(Menu::Window), 2, key(Mods::CMD, Key::Minus), Scope::Global),
    cmd("window.zoom_reset", "Actual Size", Some(Menu::Window), 2, key(Mods::CMD, Key::Digit(0)), Scope::Global),
    cmd("window.theme", "Switch Light, Dark, or Automatic Colors", Some(Menu::Window), 3, ctrl_shift('L'), Scope::Global),
    // Help
    cmd("help.palette", "Go to", Some(Menu::Help), 0, ctrl('K'), Scope::Global),
    cmd("help.current", "Help for This Screen", Some(Menu::Help), 0, key(Mods::NONE, Key::F(1)), Scope::Global),
    cmd("help.shortcuts", "Keyboard Shortcuts", Some(Menu::Help), 0, key(Mods::CMD, Key::Slash), Scope::Global),
    cmd("help.privacy", "Privacy Notice", Some(Menu::Help), 1, None, Scope::Global),
    // Opens the owner's Ko-fi page only when chosen (Q-P16a, Q-278)
    cmd("help.kofi", "Support ClientLedger on Ko-fi", Some(Menu::Help), 2, None, Scope::Global),
    cmd("help.about", "About ClientLedger", Some(Menu::Help), 2, None, Scope::Global),
];

/// Look a command up by id.
#[must_use]
pub fn find(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id.as_str() == id)
}

/// Developer-only commands (P10 plan §2.12, S8). They are not in
/// [`COMMANDS`], so no menu, keymap, palette or `KEYBOARD.md` sees them;
/// the shell looks here only when it runs with `--demo`.
pub const DEV_COMMANDS: &[Command] = &[
    built("dev.grid_demo", "Grid Demo (developer)", None, 0, None),
    // Write Check (P13): there is no check printing before P19, so the
    // control-channel scenario marks the demo check in front as printed to
    // reach the printed-check question.
    dev_action("dev.check_mark_printed", "Mark Check Printed (developer)"),
    // A03: the demo engine has no `document-drafts-v1`; the Chart of
    // Accounts scenario turns its "Card ends in" storage on and off.
    dev_action(
        "dev.demo_card_digits",
        "Demo Card Digits On/Off (developer)",
    ),
    // P18: the Pay Bills scenario runs light and dark in one app; this
    // voids the demo's bill payments so the second run sees the same list.
    // P19: the print scenarios run light and dark in one app; this puts
    // the demo's print queue back.
    dev_action("dev.demo_print_reset", "Demo Print Reset (developer)"),
    dev_action(
        "dev.demo_pay_bills_reset",
        "Reset Demo Bill Payments (developer)",
    ),
];

/// A developer-only command the window in front handles (opens nothing).
const fn dev_action(id: &'static str, label: &'static str) -> Command {
    let mut c = cmd(id, label, None, 0, None, Scope::Form);
    c.palette = false;
    c
}

/// Look a developer-only command up by id.
#[must_use]
pub fn find_dev(id: &str) -> Option<&'static Command> {
    DEV_COMMANDS.iter().find(|c| c.id.as_str() == id)
}

/// Commands in a menu, in table order.
pub fn in_menu(menu: Menu) -> impl Iterator<Item = &'static Command> {
    COMMANDS.iter().filter(move |c| c.menu == Some(menu))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_commands_stay_out_of_the_shipped_table() {
        for c in DEV_COMMANDS {
            assert!(c.id.as_str().starts_with("dev."), "{}", c.id);
            assert!(find(c.id.as_str()).is_none(), "{} is shipped", c.id);
            assert!(c.shortcut.is_none() && c.menu.is_none(), "{}", c.id);
            assert!(c.arrives.is_none(), "{}", c.id);
            assert!(c.opens || !c.palette, "{} is in the palette", c.id);
        }
        assert!(COMMANDS.iter().all(|c| !c.id.as_str().starts_with("dev.")));
        assert_eq!(find_dev("dev.grid_demo").map(|c| c.opens), Some(true));
        assert_eq!(
            find_dev("dev.check_mark_printed").map(|c| (c.opens, c.scope)),
            Some((false, Scope::Form))
        );
        assert_eq!(
            find_dev("dev.demo_card_digits").map(|c| (c.opens, c.scope)),
            Some((false, Scope::Form))
        );
        assert!(find_dev("nav.home").is_none());
    }

    #[test]
    fn find_and_menu_listing() {
        assert_eq!(find("vendors.write_check").unwrap().label, "Write Check");
        assert!(find("nope").is_none());
        assert!(in_menu(Menu::Vendors).count() >= 5);
    }

    #[test]
    fn ids_are_dotted_and_lowercase() {
        for c in COMMANDS {
            let s = c.id.as_str();
            assert!(s.contains('.'), "{s}");
            assert!(
                s.chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch == '.' || ch == '_'),
                "{s}"
            );
        }
    }

    #[test]
    fn screens_name_the_phase_that_builds_them() {
        // Screens that exist today (P08); everything else names its phase.
        let built_now = [
            "file.open_company",
            "file.new_company",
            "file.backup",
            "file.export_all",
            "file.delete_company",
            "edit.preferences",
            "lists.chart_of_accounts",
            "lists.items",
            "lists.tags",
            "lists.terms",
            "lists.payment_methods",
            "lists.import_export",
            "clients.center",
            "vendors.center",
            "banking.register",
            "vendors.write_check",
            // P14
            "banking.deposit",
            "banking.transfer",
            "clients.daily_sales",
            "vendors.card_charge",
            "banking.owner_money",
            "vendors.personal_funds",
            "banking.loan_payment",
            // P15
            "company.journal_entry",
            "lists.templates",
            "lists.templates_due",
            // P15A
            "banking.bulk_entry",
            // P16
            "nav.home",
            // P17
            "vendors.enter_bill",
            "nav.receipt_inbox",
            "nav.statement_match",
            "nav.document_view",
            // A04
            "vendors.review_documents",
            // P18
            "vendors.pay_bills",
            "vendors.bill_payment",
            // P19
            "file.print_forms",
            "file.printer_setup",
            "company.information",
            // P20
            "clients.create_invoice",
            "clients.outbox",
            "lists.price_levels",
            // P21
            "clients.receive_payment",
            // P22
            "clients.sales_receipt",
            "clients.credit_memo",
            "clients.refund",
            "clients.statements",
            "clients.finance_charges",
        ];
        for c in COMMANDS.iter().filter(|c| c.opens) {
            if built_now.contains(&c.id.as_str()) {
                assert!(c.arrives.is_none(), "{} is built", c.id);
                continue;
            }
            let p = c.arrives.expect("window commands name a phase");
            assert!(p.starts_with('P'), "{p}");
        }
    }

    #[test]
    fn register_keys() {
        use crate::keymap::Keymap;
        let km = Keymap::new(COMMANDS).unwrap();
        let at = |focus: Scope, mods: Mods, c: char| {
            km.resolve(focus, Shortcut::new(mods, Key::Letter(c)))
                .map(CommandId::as_str)
        };
        // In the register grid or entry row (Form scope).
        assert_eq!(at(Scope::Form, Mods::CMD, 'D'), Some("form.delete_item"));
        assert_eq!(
            at(Scope::Form, Mods::CMD, 'E'),
            Some("edit.edit_transaction")
        );
        assert_eq!(at(Scope::Form, Mods::CMD, 'F'), Some("edit.find"));
        assert_eq!(
            at(Scope::Form, Mods::CMD_SHIFT, 'Q'),
            Some("list.quick_report")
        );
        assert_eq!(at(Scope::Form, Mods::CMD, 'G'), Some("banking.register"));
        // Outside a form Ctrl+D still makes a deposit.
        assert_eq!(at(Scope::Global, Mods::CMD, 'D'), Some("banking.deposit"));
        let void = find("edit.void_transaction").unwrap();
        assert_eq!(void.label, "Void Transaction");
        assert_eq!(
            (void.menu, void.shortcut, void.scope),
            (Some(Menu::Edit), None, Scope::Window)
        );
        let one = find("register.one_line").unwrap();
        assert_eq!(one.label, "One-Line Display");
        assert_eq!(
            (one.menu, one.shortcut, one.scope),
            (None, None, Scope::Window)
        );
        let reg = find("banking.register").unwrap();
        assert!(reg.opens && reg.arrives.is_none(), "the register is built");
    }

    #[test]
    fn write_check_commands() {
        use crate::keymap::Keymap;
        let wc = find("vendors.write_check").unwrap();
        assert!(
            wc.opens && wc.arrives.is_none(),
            "Write Check is built (P13)"
        );
        for (id, label) in [
            ("form.attach", "Attach\u{2026}"),
            ("form.history", "History"),
            ("check.print_later", "Print Later"),
        ] {
            let c = find(id).unwrap();
            assert_eq!(c.label, label);
            assert_eq!((c.menu, c.shortcut, c.scope), (None, None, Scope::Form));
            assert!(!c.opens);
        }
        // The keys the form reuses still resolve in a form.
        let km = Keymap::new(COMMANDS).unwrap();
        let at = |mods: Mods, k: Key| {
            km.resolve(Scope::Form, Shortcut::new(mods, k))
                .map(CommandId::as_str)
        };
        assert_eq!(at(Mods::NONE, Key::PageUp), Some("form.prev_record"));
        assert_eq!(at(Mods::NONE, Key::PageDown), Some("form.next_record"));
        assert_eq!(at(Mods::CMD, Key::Letter('F')), Some("edit.find"));
        assert_eq!(at(Mods::CMD, Key::Letter('P')), Some("file.print"));
        assert_eq!(at(Mods::ALT, Key::Letter('N')), Some("form.save_new"));
        assert_eq!(at(Mods::ALT, Key::Letter('S')), Some("form.save_close"));
    }

    #[test]
    fn p14_commands() {
        use crate::keymap::Keymap;
        for (id, label, menu, shortcut) in [
            ("banking.deposit", "Make Deposit", Menu::Banking, ctrl('D')),
            (
                "banking.transfer",
                "Transfer Funds",
                Menu::Banking,
                ctrl('T'),
            ),
            (
                "vendors.card_charge",
                "Record Card Charge",
                Menu::Vendors,
                ctrl_shift('C'),
            ),
            // The new helpers have no shortcut (Q-P14x).
            (
                "clients.daily_sales",
                "Daily Sales Summary",
                Menu::Clients,
                None,
            ),
            (
                "banking.owner_money",
                "Pay Yourself or Put Money In",
                Menu::Banking,
                None,
            ),
            (
                "vendors.personal_funds",
                "Paid with Personal Funds",
                Menu::Vendors,
                None,
            ),
            ("banking.loan_payment", "Loan Payment", Menu::Banking, None),
        ] {
            let c = find(id).unwrap();
            assert_eq!(c.label, label, "{id}");
            assert_eq!(
                (c.menu, c.shortcut, c.scope),
                (Some(menu), shortcut, Scope::Global),
                "{id}"
            );
            assert!(c.opens && c.arrives.is_none(), "{id} is built (P14)");
        }
        // Sales Receipt came with credit memos (Q-259, Q-P22e).
        assert!(find("clients.sales_receipt").unwrap().opens);
        assert_eq!(find("clients.sales_receipt").unwrap().arrives, None);
        assert!(Keymap::new(COMMANDS).is_ok(), "no new key clashes");
    }

    #[test]
    fn p15_commands() {
        use crate::keymap::Keymap;
        let je = find("company.journal_entry").unwrap();
        assert_eq!(je.label, "Make Journal Entry");
        assert_eq!((je.menu, je.shortcut), (Some(Menu::Company), ctrl('J')));
        assert!(
            je.opens && je.arrives.is_none(),
            "Make Journal Entry is built"
        );
        let list = find("lists.templates").unwrap();
        assert_eq!(list.label, "Templates");
        assert_eq!((list.menu, list.shortcut), (Some(Menu::Lists), None));
        assert!(list.opens && list.arrives.is_none() && list.palette);
        // The catch-up runner opens Templates Due; it has no menu entry.
        let due = find("lists.templates_due").unwrap();
        assert_eq!(due.label, "Templates Due");
        assert_eq!((due.menu, due.shortcut), (None, None));
        assert!(due.opens && due.arrives.is_none() && due.palette);
        assert!(in_menu(Menu::Lists).all(|c| c.id.as_str() != "lists.templates_due"));
        // Save as Template is a form command on Ctrl+M (Q-P15k).
        let save = find("form.save_as_template").unwrap();
        assert_eq!(save.label, "Save as Template");
        assert_eq!(
            (save.menu, save.shortcut, save.scope),
            (None, ctrl('M'), Scope::Form)
        );
        assert!(!save.opens);
        let km = Keymap::new(COMMANDS).unwrap();
        assert_eq!(
            km.resolve(Scope::Form, Shortcut::new(Mods::CMD, Key::Letter('M')))
                .map(CommandId::as_str),
            Some("form.save_as_template")
        );
    }

    #[test]
    fn p15a_bulk_entry_command() {
        let c = find("banking.bulk_entry").unwrap();
        assert_eq!(c.label, "Bulk Entry");
        // Banking menu and palette; no global shortcut (Q-P15Ah).
        assert_eq!(
            (c.menu, c.shortcut, c.scope),
            (Some(Menu::Banking), None, Scope::Global)
        );
        assert!(c.opens && c.arrives.is_none() && c.palette);
        // Its own keys, in the form scope (Q-P15Ah).
        let km = crate::keymap::Keymap::new(COMMANDS).unwrap();
        for (id, mods, k) in [
            ("batch.next_problem", Mods::NONE, Key::F(8)),
            ("batch.prev_problem", Mods::SHIFT, Key::F(8)),
            ("batch.next_section", Mods::CMD, Key::PageDown),
            ("batch.prev_section", Mods::CMD, Key::PageUp),
        ] {
            let c = find(id).unwrap();
            assert_eq!(c.scope, Scope::Form, "{id}");
            assert_eq!(
                km.resolve(Scope::Form, Shortcut::new(mods, k))
                    .map(CommandId::as_str),
                Some(id)
            );
        }
    }

    #[test]
    fn p16_commands() {
        use crate::keymap::Keymap;
        let details = find("window.toggle_details").unwrap();
        assert_eq!(details.label, "Show or Hide Details");
        assert_eq!(
            (details.menu, details.shortcut, details.scope),
            (
                Some(Menu::Window),
                Some(Shortcut::new(Mods::NONE, Key::F(9))),
                Scope::Window
            )
        );
        assert!(!details.opens && details.palette);
        for (id, label, shortcut, scope) in [
            ("grid.copy_down", "Copy Down", ctrl('D'), Scope::Grid),
            (
                "grid.copy_down_below",
                "Copy to Rows Below",
                None,
                Scope::Grid,
            ),
            ("grid.select_all", "Select All Rows", ctrl('A'), Scope::Grid),
            ("grid.clear_selection", "Clear Selection", None, Scope::Grid),
            ("batch.columns", "Columns\u{2026}", None, Scope::Form),
            ("batch.set_account", "Set Account", None, Scope::Grid),
            ("batch.set_name", "Set Name", None, Scope::Grid),
            ("batch.set_tag", "Set Tag", None, Scope::Grid),
            ("batch.set_date", "Set Date", None, Scope::Grid),
            ("batch.set_memo", "Set Memo", None, Scope::Grid),
            (
                "batch.set_print_later",
                "Set Print Later",
                None,
                Scope::Grid,
            ),
            ("batch.delete_rows", "Delete Rows", None, Scope::Grid),
        ] {
            let c = find(id).unwrap();
            assert_eq!(c.label, label, "{id}");
            assert_eq!(
                (c.menu, c.shortcut, c.scope),
                (None, shortcut, scope),
                "{id}"
            );
            assert!(c.palette && !c.opens && c.arrives.is_none(), "{id}");
        }
        let km = Keymap::new(COMMANDS).unwrap();
        for focus in [Scope::Window, Scope::Form, Scope::Grid] {
            assert_eq!(
                km.resolve(focus, Shortcut::new(Mods::NONE, Key::F(9)))
                    .map(CommandId::as_str),
                Some("window.toggle_details")
            );
            // Ctrl+Shift+D left Details (Q-P16n: F9); the document viewer
            // took it in A02.
            assert_eq!(
                km.resolve(focus, Shortcut::new(Mods::CMD_SHIFT, Key::Letter('D')))
                    .map(CommandId::as_str),
                Some("view.toggle_doc_viewer")
            );
        }
        // Ko-fi in the Help menu too (Q-P16a): no shortcut, opens nothing.
        let kofi = find("help.kofi").unwrap();
        assert_eq!(kofi.label, "Support ClientLedger on Ko-fi");
        assert_eq!(
            (kofi.menu, kofi.shortcut, kofi.scope),
            (Some(Menu::Help), None, Scope::Global)
        );
        assert!(!kofi.opens && kofi.palette && kofi.arrives.is_none());
        assert!(in_menu(Menu::Help).any(|c| c.id.as_str() == "help.kofi"));
    }

    #[test]
    fn p17_commands() {
        use crate::keymap::Keymap;
        // Enter Bill is built (the placeholder is gone, Q-P06g, Q-P16k).
        let bill = find("vendors.enter_bill").unwrap();
        assert_eq!(
            (bill.menu, bill.shortcut, bill.scope),
            (Some(Menu::Vendors), ctrl('B'), Scope::Global)
        );
        assert!(bill.opens && bill.arrives.is_none() && bill.palette);
        for id in ["nav.receipt_inbox", "nav.statement_match"] {
            let c = find(id).unwrap();
            assert_eq!((c.menu, c.shortcut), (Some(Menu::Vendors), None), "{id}");
            assert!(c.opens && c.arrives.is_none() && c.palette, "{id}");
        }
        let doc = find("nav.document_view").unwrap();
        assert_eq!((doc.menu, doc.shortcut), (None, None));
        assert!(doc.opens && doc.arrives.is_none());
        for (id, shortcut, scope) in [
            ("bill.kind_toggle", None, Scope::Form),
            ("bill.read_document", None, Scope::Form),
            // A03: Read document (local) beside Read with AI.
            ("bill.read_local", None, Scope::Form),
            ("form.accept_read", alt('A'), Scope::Form),
            ("viewer.zoom_in", key(Mods::CMD, Key::Equals), Scope::Viewer),
            ("viewer.zoom_out", key(Mods::CMD, Key::Minus), Scope::Viewer),
            ("viewer.fit", key(Mods::CMD, Key::Digit(0)), Scope::Viewer),
            (
                "viewer.page_next",
                key(Mods::NONE, Key::PageDown),
                Scope::Viewer,
            ),
            (
                "viewer.page_prev",
                key(Mods::NONE, Key::PageUp),
                Scope::Viewer,
            ),
        ] {
            let c = find(id).unwrap();
            assert_eq!(
                (c.menu, c.shortcut, c.scope),
                (None, shortcut, scope),
                "{id}"
            );
            assert!(c.palette && !c.opens && c.arrives.is_none(), "{id}");
        }
        // Alt+A was free in a form before P17.
        let km = Keymap::new(COMMANDS).unwrap();
        assert_eq!(
            km.resolve(Scope::Form, Shortcut::new(Mods::ALT, Key::Letter('A')))
                .map(CommandId::as_str),
            Some("form.accept_read")
        );
        assert_eq!(
            km.resolve(Scope::Global, Shortcut::new(Mods::ALT, Key::Letter('A'))),
            None
        );
    }

    #[test]
    fn a02_viewer_commands() {
        use crate::keymap::Keymap;
        let toggle = find("view.toggle_doc_viewer").unwrap();
        assert_eq!(
            (toggle.menu, toggle.shortcut, toggle.scope),
            (Some(Menu::Window), ctrl_shift('D'), Scope::Global)
        );
        let focus = find("view.focus_doc").unwrap();
        assert_eq!(
            (focus.menu, focus.shortcut, focus.scope),
            (
                Some(Menu::Window),
                key(Mods::NONE, Key::F(6)),
                Scope::Window
            )
        );
        for c in [toggle, focus] {
            assert!(c.palette && !c.opens && c.arrives.is_none(), "{}", c.id);
        }
        // Rotate: buttons and the palette only (Q-A02m).
        for id in ["viewer.rotate_cw", "viewer.rotate_ccw"] {
            let c = find(id).unwrap();
            assert_eq!((c.menu, c.shortcut, c.scope), (None, None, Scope::Viewer));
            assert!(c.palette && !c.opens, "{id}");
        }
        let km = Keymap::new(COMMANDS).expect("no new key clashes");
        let at = |focus: Scope, mods: Mods, k: Key| {
            km.resolve(focus, Shortcut::new(mods, k))
                .map(CommandId::as_str)
        };
        for scope in Scope::ALL {
            assert_eq!(
                at(*scope, Mods::CMD_SHIFT, Key::Letter('D')),
                Some("view.toggle_doc_viewer"),
                "{scope:?}"
            );
        }
        assert_eq!(at(Scope::Global, Mods::NONE, Key::F(6)), None);
        for scope in [Scope::Window, Scope::Form, Scope::Grid, Scope::Viewer] {
            assert_eq!(
                at(scope, Mods::NONE, Key::F(6)),
                Some("view.focus_doc"),
                "{scope:?}"
            );
        }
        // Ctrl+R stays Receive Payment while the page has focus.
        assert_eq!(
            at(Scope::Viewer, Mods::CMD, Key::Letter('R')),
            Some("clients.receive_payment")
        );
        // The viewer's own keys still win inside it.
        assert_eq!(
            at(Scope::Viewer, Mods::NONE, Key::PageDown),
            Some("viewer.page_next")
        );
    }

    #[test]
    fn a04_commands() {
        use crate::keymap::Keymap;
        let review = find("vendors.review_documents").unwrap();
        assert_eq!(review.label, "Review Documents");
        assert_eq!(
            (review.menu, review.shortcut, review.scope),
            (Some(Menu::Vendors), None, Scope::Global)
        );
        assert!(review.opens && review.arrives.is_none() && review.palette);
        for (id, shortcut, scope) in [
            ("queue.next", key(Mods::NONE, Key::Letter('J')), Scope::List),
            ("queue.prev", key(Mods::NONE, Key::Letter('K')), Scope::List),
            ("queue.open", key(Mods::NONE, Key::Enter), Scope::List),
            ("queue.discard", key(Mods::NONE, Key::Delete), Scope::List),
            ("queue.read_all", None, Scope::List),
            (
                "viewer.next_word",
                key(Mods::ALT, Key::Right),
                Scope::Viewer,
            ),
            ("viewer.prev_word", key(Mods::ALT, Key::Left), Scope::Viewer),
            (
                "viewer.fill_word",
                key(Mods::NONE, Key::Enter),
                Scope::Viewer,
            ),
            ("viewer.toggle_highlights", None, Scope::Viewer),
            ("form.accept_read", alt('A'), Scope::Form),
        ] {
            let c = find(id).unwrap();
            assert_eq!(
                (c.menu, c.shortcut, c.scope),
                (None, shortcut, scope),
                "{id}"
            );
            assert!(c.palette && !c.opens && c.arrives.is_none(), "{id}");
        }
        // The old id is gone (every form handles `form.accept_read`).
        assert!(find("bill.accept_read").is_none());
        for mode in [
            crate::forms::EnterMode::SaveAndNew,
            crate::forms::EnterMode::MoveBetweenFields,
        ] {
            assert!(Keymap::with_mode(COMMANDS, mode).is_ok(), "{mode:?}");
        }
    }

    #[test]
    fn design_md_shortcuts_are_bound() {
        use crate::keymap::Keymap;
        let km = Keymap::new(COMMANDS).unwrap();
        let want: &[(&str, Mods, Key)] = &[
            ("help.palette", Mods::CMD, Key::Letter('K')),
            ("window.next", Mods::CMD, Key::Tab),
            ("window.prev", Mods::CMD_SHIFT, Key::Tab),
            ("nav.home", Mods::ALT, Key::Letter('H')),
            ("vendors.write_check", Mods::CMD, Key::Letter('W')),
            ("clients.create_invoice", Mods::CMD, Key::Letter('I')),
            ("clients.receive_payment", Mods::CMD, Key::Letter('R')),
            ("clients.sales_receipt", Mods::CMD_SHIFT, Key::Letter('R')),
            ("vendors.enter_bill", Mods::CMD, Key::Letter('B')),
            ("vendors.pay_bills", Mods::CMD_SHIFT, Key::Letter('B')),
            ("vendors.card_charge", Mods::CMD_SHIFT, Key::Letter('C')),
            ("banking.register", Mods::CMD, Key::Letter('G')),
            ("banking.reconcile", Mods::CMD_SHIFT, Key::Letter('E')),
            ("banking.transfer", Mods::CMD, Key::Letter('T')),
            ("company.journal_entry", Mods::CMD, Key::Letter('J')),
            ("reports.trial_balance", Mods::CMD_SHIFT, Key::Letter('T')),
            ("file.print", Mods::CMD, Key::Letter('P')),
            ("lists.chart_of_accounts", Mods::CMD, Key::Letter('A')),
            ("edit.find", Mods::CMD, Key::Letter('F')),
            ("edit.edit_transaction", Mods::CMD, Key::Letter('E')),
            ("edit.open_list", Mods::CMD, Key::Letter('L')),
            ("help.current", Mods::NONE, Key::F(1)),
            ("help.shortcuts", Mods::CMD, Key::Slash),
            ("view.toggle_doc_viewer", Mods::CMD_SHIFT, Key::Letter('D')),
            ("view.focus_doc", Mods::NONE, Key::F(6)),
        ];
        for (id, mods, k) in want {
            let c = find(id).unwrap();
            assert_eq!(c.shortcut, Some(Shortcut::new(*mods, *k)), "{id}");
            let focus = c.scope;
            assert_eq!(
                km.resolve(focus, Shortcut::new(*mods, *k))
                    .map(CommandId::as_str),
                Some(*id)
            );
        }
    }
}
