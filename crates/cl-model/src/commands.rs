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
            Menu::Clients => "Clients",
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
    built("file.portable_copy", "Make Portable Copy", Some(Menu::File), 1, None),
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
    // Lists
    built("lists.chart_of_accounts", "Chart of Accounts", Some(Menu::Lists), 0, ctrl('A')),
    built("lists.items", "Item List", Some(Menu::Lists), 0, None),
    built("lists.tags", "Tags", Some(Menu::Lists), 0, None),
    built("lists.terms", "Terms", Some(Menu::Lists), 1, None),
    built("lists.payment_methods", "Payment Methods", Some(Menu::Lists), 1, None),
    built("lists.import_export", "Import or Export a List", Some(Menu::Lists), 2, None),
    // List windows (Chart of Accounts first; the other lists use the same keys)
    cmd("list.new", "New List Entry", None, 3, ctrl('N'), Scope::Window),
    cmd("list.toggle_active", "Make Inactive or Active Again", None, 3, ctrl_shift('I'), Scope::Window),
    cmd("list.show_inactive", "Include Inactive Entries", None, 3, ctrl_shift('H'), Scope::Window),
    cmd("list.quick_report", "QuickReport", None, 3, ctrl_shift('Q'), Scope::Window),
    cmd("list.merge", "Merge Into Another Entry", None, 3, None, Scope::Window),
    // Register windows (P12; Ctrl+F, Ctrl+E, Ctrl+D and Ctrl+Shift+Q above apply too)
    cmd("register.one_line", "One-Line Display", None, 4, None, Scope::Window),
    // Clients
    built("clients.center", "Clients & Jobs", Some(Menu::Clients), 0, None),
    screen("clients.create_invoice", "Create Invoice", Some(Menu::Clients), 1, ctrl('I'), "P20"),
    screen("clients.receive_payment", "Receive Payment", Some(Menu::Clients), 1, ctrl('R'), "P21"),
    screen("clients.sales_receipt", "Sales Receipt", Some(Menu::Clients), 1, ctrl_shift('R'), "P14"),
    screen("clients.credit_memo", "Credit Memo or Refund", Some(Menu::Clients), 2, None, "P22"),
    screen("clients.statements", "Statements", Some(Menu::Clients), 2, None, "P22"),
    // Vendors
    built("vendors.center", "Vendors", Some(Menu::Vendors), 0, None),
    screen("vendors.enter_bill", "Enter Bill", Some(Menu::Vendors), 1, ctrl('B'), "P17"),
    screen("vendors.pay_bills", "Pay Bills", Some(Menu::Vendors), 1, ctrl_shift('B'), "P18"),
    screen("vendors.write_check", "Write Check", Some(Menu::Vendors), 1, ctrl('W'), "P13"),
    screen("vendors.card_charge", "Record Card Charge", Some(Menu::Vendors), 1, ctrl_shift('C'), "P14"),
    // Banking
    built("banking.register", "Open Register", Some(Menu::Banking), 0, ctrl('G')),
    screen("banking.deposit", "Make Deposit", Some(Menu::Banking), 0, ctrl('D'), "P14"),
    screen("banking.transfer", "Transfer Funds", Some(Menu::Banking), 0, ctrl('T'), "P14"),
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
    screen("company.journal_entry", "Make Journal Entry", Some(Menu::Company), 0, ctrl('J'), "P15"),
    screen("company.closing_date", "Set Closing Date", Some(Menu::Company), 1, None, "P29"),
    screen("company.users", "Users and Permissions", Some(Menu::Company), 1, None, "P29"),
    screen("company.send_to_accountant", "Send to My Accountant", Some(Menu::Company), 2, None, "P32"),
    screen("company.audit_trail", "Audit Trail", Some(Menu::Company), 2, None, "P29"),
    // Window
    screen("nav.home", "Home", Some(Menu::Window), 0, alt('H'), "P16"),
    cmd("window.next", "Next Window", Some(Menu::Window), 1, key(Mods::CMD, Key::Tab), Scope::Global),
    cmd("window.prev", "Previous Window", Some(Menu::Window), 1, key(Mods::CMD_SHIFT, Key::Tab), Scope::Global),
    cmd("window.pop_out", "Move Window to Its Own Window", Some(Menu::Window), 1, ctrl_shift('O'), Scope::Window),
    cmd("window.zoom_in", "Zoom In", Some(Menu::Window), 2, key(Mods::CMD, Key::Equals), Scope::Global),
    cmd("window.zoom_out", "Zoom Out", Some(Menu::Window), 2, key(Mods::CMD, Key::Minus), Scope::Global),
    cmd("window.zoom_reset", "Actual Size", Some(Menu::Window), 2, key(Mods::CMD, Key::Digit(0)), Scope::Global),
    cmd("window.theme", "Switch Light, Dark, or Automatic Colors", Some(Menu::Window), 3, ctrl_shift('L'), Scope::Global),
    // Help
    cmd("help.palette", "Go to", Some(Menu::Help), 0, ctrl('K'), Scope::Global),
    cmd("help.current", "Help for This Screen", Some(Menu::Help), 0, key(Mods::NONE, Key::F(1)), Scope::Global),
    cmd("help.shortcuts", "Keyboard Shortcuts", Some(Menu::Help), 0, key(Mods::CMD, Key::Slash), Scope::Global),
    cmd("help.privacy", "Privacy Notice", Some(Menu::Help), 1, None, Scope::Global),
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
pub const DEV_COMMANDS: &[Command] = &[built(
    "dev.grid_demo",
    "Grid Demo (developer)",
    None,
    0,
    None,
)];

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
            assert!(c.opens && c.arrives.is_none());
        }
        assert!(COMMANDS.iter().all(|c| !c.id.as_str().starts_with("dev.")));
        assert_eq!(find_dev("dev.grid_demo").map(|c| c.opens), Some(true));
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
            "file.portable_copy",
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
