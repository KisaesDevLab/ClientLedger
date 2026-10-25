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
    built("file.backup", "Back Up Company", Some(Menu::File), 1, None),
    cmd("file.restore", "Restore a Backup", Some(Menu::File), 1, None, Scope::Global),
    built("file.export_all", "Export All Data", Some(Menu::File), 1, None),
    screen("file.full_backup", "Full Database Backup (Utilities)", Some(Menu::File), 1, None, "P34"),
    built("file.delete_company", "Delete Company", Some(Menu::File), 2, None),
    cmd("file.print", "Print", Some(Menu::File), 3, ctrl('P'), Scope::Global),
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
    cmd("bill.accept_read", "Accept These Values", None, 7, alt('A'), Scope::Form),
    // Document preview (P17): the source-document card and its full-size tab
    cmd("viewer.zoom_in", "Zoom In on the Document", None, 8, key(Mods::CMD, Key::Equals), Scope::Viewer),
    cmd("viewer.zoom_out", "Zoom Out on the Document", None, 8, key(Mods::CMD, Key::Minus), Scope::Viewer),
    cmd("viewer.fit", "Fit the Document", None, 8, key(Mods::CMD, Key::Digit(0)), Scope::Viewer),
    cmd("viewer.page_next", "Next Page", None, 8, key(Mods::NONE, Key::PageDown), Scope::Viewer),
    cmd("viewer.page_prev", "Previous Page", None, 8, key(Mods::NONE, Key::PageUp), Scope::Viewer),
    // Lists
    built("lists.chart_of_accounts", "Chart of Accounts", Some(Menu::Lists), 0, ctrl('A')),
    built("lists.items", "Item List", Some(Menu::Lists), 0, None),
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
    built("clients.center", "Customers & Jobs", Some(Menu::Clients), 0, None),
    screen("clients.create_invoice", "Create Invoice", Some(Menu::Clients), 1, ctrl('I'), "P20"),
    screen("clients.receive_payment", "Receive Payment", Some(Menu::Clients), 1, ctrl('R'), "P21"),
    screen("clients.sales_receipt", "Sales Receipt", Some(Menu::Clients), 1, ctrl_shift('R'), "P20"),
    built("clients.daily_sales", "Daily Sales Summary", Some(Menu::Clients), 1, None),
    screen("clients.credit_memo", "Credit Memo or Refund", Some(Menu::Clients), 2, None, "P22"),
    screen("clients.statements", "Statements", Some(Menu::Clients), 2, None, "P22"),
    // Vendors
    built("vendors.center", "Vendors", Some(Menu::Vendors), 0, None),
    built("vendors.enter_bill", "Enter Bill", Some(Menu::Vendors), 1, ctrl('B')),
    screen("vendors.pay_bills", "Pay Bills", Some(Menu::Vendors), 1, ctrl_shift('B'), "P18"),
    built("vendors.write_check", "Write Check", Some(Menu::Vendors), 1, ctrl('W')),
    built("vendors.card_charge", "Record Card Charge", Some(Menu::Vendors), 1, ctrl_shift('C')),
    built("vendors.personal_funds", "Paid with Personal Funds", Some(Menu::Vendors), 1, None),
    // P17: the receipt inbox (the menu shows its count, Q-P17t) and the
    // vendor statement match; no shortcut
    built("nav.receipt_inbox", "Receipt Inbox", Some(Menu::Vendors), 2, None),
    built("nav.statement_match", "Match Vendor Statement\u{2026}", Some(Menu::Vendors), 2, None),
    // P17: a document in its own tab ("Open full size"); no menu entry
    built("nav.document_view", "Document Preview", None, 2, None),
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
        // Sales Receipt comes with invoices (Q-259).
        assert_eq!(find("clients.sales_receipt").unwrap().arrives, Some("P20"));
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
            // Ctrl+Shift+D is free again (Q-P16n: F9).
            assert_ne!(
                km.resolve(focus, Shortcut::new(Mods::CMD_SHIFT, Key::Letter('D')))
                    .map(CommandId::as_str),
                Some("window.toggle_details")
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
            ("bill.accept_read", alt('A'), Scope::Form),
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
            Some("bill.accept_read")
        );
        assert_eq!(
            km.resolve(Scope::Global, Shortcut::new(Mods::ALT, Key::Letter('A'))),
            None
        );
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
