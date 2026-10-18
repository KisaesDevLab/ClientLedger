//! Every user-visible string goes through here (CLAUDE.md coding standards).
//!
//! English only for now; the point is that a later locale replaces one table
//! instead of hunting through widgets. Keys are dotted (`shell.rail.workspace`).
//! Command labels come from the `cl-model` command table (`cmd.<id>` resolves
//! to the registered English label), and owner-facing error keys from `cl-core`
//! and `cl-api` (`error.*`, `fix.*`) live in this catalog.
//!
//! An unknown key returns the key itself so a missing string is visible, never
//! a panic; the `every_key_used_in_the_source_is_in_the_catalog` test makes it
//! a build failure instead.

use std::collections::HashMap;
use std::sync::OnceLock;

use cl_model::commands;

/// Translate `key`. `cmd.<command id>` returns the command's label.
#[must_use]
pub fn t(key: &str) -> String {
    if let Some(id) = key.strip_prefix("cmd.")
        && let Some(c) = commands::find(id)
    {
        return c.label.to_string();
    }
    match catalog().get(key) {
        Some(text) => (*text).to_string(),
        None => key.to_string(),
    }
}

/// Translate `key` and replace each `{name}` with its value.
#[must_use]
pub fn t_with(key: &str, args: &[(&str, &str)]) -> String {
    let mut text = t(key);
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// True when `key` has an entry (used by tests and by error presentation).
#[must_use]
pub fn has(key: &str) -> bool {
    key.strip_prefix("cmd.")
        .is_some_and(|id| commands::find(id).is_some())
        || catalog().contains_key(key)
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| ENGLISH.iter().copied().collect())
}

/// The English catalog.
#[rustfmt::skip]
pub const ENGLISH: &[(&str, &str)] = &[
    // App
    ("app.name", "ClientLedger"),
    ("app.tagline", "powered by Vibe MyBooks"),
    // Theme
    ("theme.light", "Light"),
    ("theme.dark", "Dark"),
    ("theme.auto", "Automatic"),
    // Menus
    ("menu.menu", "Menu"),
    ("menu.file", "File"),
    ("menu.edit", "Edit"),
    ("menu.lists", "Lists"),
    ("menu.clients", "Clients"),
    ("menu.vendors", "Vendors"),
    ("menu.banking", "Banking"),
    ("menu.payroll", "Payroll"),
    ("menu.reports", "Reports"),
    ("menu.company", "Company"),
    ("menu.window", "Window"),
    ("menu.help", "Help"),
    // Shell
    ("shell.no_company", "No company open"),
    ("shell.closed_through", "Books closed through {date}"),
    ("shell.closed_short", "Closed through {date}"),
    ("shell.go_to", "Go to"),
    ("shell.go_to_hint", "Go to a screen or command"),
    ("shell.go_to_label", "Go to a screen or command"),
    ("shell.rail.workspace", "Workspace"),
    ("shell.rail.tools", "Tools"),
    ("shell.rail.open_windows", "Open windows"),
    ("shell.rail.toggle", "Navigation"),
    ("shell.empty", "Nothing is open. Press Alt+H for Home, or Ctrl+K to go to anything."),
    ("shell.popped_out", "This window is open on its own."),
    ("shell.bring_back", "Bring it back here"),
    // Tabs
    ("tabs.unsaved", "unsaved changes"),
    ("tabs.label_dirty", "{title}, unsaved changes"),
    ("tabs.close", "Close {title}"),
    ("tabs.close_menu", "Close"),
    ("tabs.pop_out", "Move to its own window"),
    ("tabs.dock", "Bring back as a tab"),
    // Windows
    ("window.placeholder.coming", "This screen is not available yet. It arrives in a later release."),
    ("window.placeholder.home", "Home arrives in a later release."),
    ("window.prompt.title", "Unsaved changes"),
    ("window.prompt.message", "\"{title}\" has changes that are not saved. What would you like to do?"),
    ("window.prompt.save", "Save"),
    ("window.prompt.discard", "Discard changes"),
    ("window.prompt.keep", "Keep editing"),
    // Status bar
    ("status.not_signed_in", "Not signed in"),
    ("status.engine.starting", "Books engine: starting"),
    ("status.engine.running", "Books engine: running"),
    ("status.engine.restarting", "Books engine: restarting"),
    ("status.engine.failed", "Books engine stopped:"),
    ("status.engine.not_found", "the engine program was not found"),
    ("status.fault.database", "the database stopped working and could not be restarted"),
    ("status.fault.engine", "the engine stopped working and could not be restarted"),
    ("status.engine.stopped", "Books engine: stopped"),
    ("status.hint.go_to", "go to"),
    ("status.hint.next_window", "next window"),
    ("status.hint.shortcuts", "shortcuts"),
    ("status.hint.help", "help"),
    ("toast.not_here", "That does not apply to this screen."),
    ("toast.help_later", "Help for this screen is not available yet."),
    // Palette and overlay
    ("palette.hint", "Go to a screen or command"),
    ("palette.none", "Nothing matches. Try fewer letters."),
    ("shortcuts.title", "Keyboard shortcuts"),
    ("shortcuts.intro", "The keys that work right here. Press Esc to close."),
    ("shortcuts.scope.form", "In this form or grid"),
    ("shortcuts.scope.window", "In this window"),
    ("shortcuts.scope.global", "Anywhere"),
    ("shortcuts.fixed", "Always the same"),
    ("shortcuts.dates", "In date fields"),
    ("shortcuts.bank_feed", "In the Bank Feed"),
    ("shortcuts.close", "Close"),
    // Company lifecycle (P08)
    ("toast.open_company_first", "Open a company first."),
    ("open.tab", "Open Company"),
    ("open.title", "Open a company"),
    ("open.subtitle", "Your books stay on this computer. Nothing is sent anywhere unless you turn it on."),
    ("open.recent", "Recent companies"),
    ("open.start", "Start something new"),
    ("open.open", "Open"),
    ("open.browse", "Browse for a company\u{2026}"),
    ("open.saved_in", "Backups in {folder}"),
    ("open.empty", "No companies yet. Make a new one, restore a backup, or try the practice company."),
    ("open.new", "New company"),
    ("open.new_sub", "A short interview sets up your accounts"),
    ("open.qbd", "Switch from QuickBooks Desktop"),
    ("open.qbd_sub", "Bring your books over and check they match"),
    ("open.restore", "Restore a backup"),
    ("open.restore_sub", "From a .clbackup file"),
    ("open.lan", "Open a company on my office network"),
    ("open.lan_sub", "Another computer here is sharing it"),
    ("open.practice", "Try the practice company"),
    ("open.practice_sub", "Explore without touching real books"),
    ("open.practice_later", "The practice company is not set up on this computer yet."),
    ("open.footer.offline", "Works fully offline"),
    ("open.footer.not_affiliated", "Not affiliated with Intuit or QuickBooks"),
    ("open.probing", "Checking the books engine\u{2026}"),
    ("open.engine_not_ready", "The books engine is not ready yet. The status bar shows how it is doing."),
    ("open.opening", "Opening\u{2026}"),
    ("open.wait_for_saves", "A save is still finishing. Try again in a moment."),
    ("open.save_first", "Save or close your open forms first."),
    ("open.meta.opened", "Last opened {when}"),
    ("open.meta.backed_up", "Backed up {when}"),
    ("open.meta.never_backed_up", "Not backed up yet"),
    ("open.meta.transactions", "{n} transactions"),
    ("open.meta.sample", "Made-up data to try things safely \u{b7} Reset any time"),
    ("open.meta.new", "Just created"),
    ("open.setup.title", "Set up ClientLedger"),
    ("open.setup.intro", "This creates the owner account for this computer. You sign in with it; nothing is sent anywhere."),
    ("open.setup.name", "Your name"),
    ("open.setup.email", "Email (your sign-in)"),
    ("open.setup.password", "Password"),
    ("open.setup.confirm", "Password again"),
    ("open.setup.password_rule", "At least 12 characters. No one can reset it for you except the recovery key you get at setup."),
    ("open.setup.password_short", "The password needs at least 12 characters."),
    ("open.setup.password_mismatch", "The two passwords are not the same."),
    ("open.setup.go", "Create owner account"),
    ("open.setup.working", "Setting up\u{2026}"),
    ("open.signin.title", "Sign in"),
    ("open.signin.go", "Sign in"),
    ("open.signin.working", "Signing in\u{2026}"),
    ("open.restore.title", "Restore a backup"),
    ("open.restore.intro", "The backup is restored as a new company first and checked by the books engine. Your current company is kept until you confirm."),
    ("open.restore.file", "Backup file (.clbackup)"),
    ("open.restore.passphrase", "Backup passphrase"),
    ("open.restore.passphrase_hint", "Leave blank to use the passphrase saved on this computer. A file from another computer needs its passphrase."),
    ("open.restore.need_file", "Enter the path of the backup file."),
    ("open.restore.go", "Restore"),
    ("open.restore.cancel", "Cancel"),
    ("open.restore.working", "Restoring and checking\u{2026}"),
    ("open.restore.done", "Restored and verified: {name}. Open it from the list."),
    ("open.restore.done_unverified", "Restored: {name}. The engine did not report a verification."),
    ("open.restore.kept_previous", "The company it came from is still here, renamed with \"before restore\". Delete it when you are sure."),
    ("interview.tab", "New Company"),
    ("interview.title", "New company"),
    ("interview.intro", "A few plain questions. You can change any of this later in Preferences."),
    ("interview.page.names", "Names"),
    ("interview.page.year", "Fiscal year"),
    ("interview.page.chart", "Accounts"),
    ("interview.page.features", "Features"),
    ("interview.page.security", "Security and look"),
    ("interview.page.review", "Review"),
    ("interview.names.intro", "What is the business called?"),
    ("interview.names.name", "Business name"),
    ("interview.names.legal", "Legal name (if different)"),
    ("interview.names.legal_hint", "The legal name goes on forms; the business name is what you see every day."),
    ("interview.names.entity", "How is it organized?"),
    ("interview.entity.sole_prop", "Sole proprietor"),
    ("interview.entity.smllc", "Single-member LLC"),
    ("interview.entity.partnership", "Partnership"),
    ("interview.entity.s_corp", "S corporation"),
    ("interview.entity.c_corp", "C corporation"),
    ("interview.entity.unsure", "Not sure yet"),
    ("interview.year.intro", "Most small businesses use the calendar year."),
    ("interview.year.start", "Fiscal year starts in"),
    ("interview.year.calendar", "Months or retail periods?"),
    ("interview.calendar.standard", "Standard months"),
    ("interview.calendar.standard_sub", "January, February, March\u{2026} the usual choice"),
    ("interview.calendar.445", "4-4-5 weeks"),
    ("interview.calendar.445_sub", "Quarters of 4, 4 and 5 weeks (retail and restaurants)"),
    ("interview.calendar.454", "4-5-4 weeks"),
    ("interview.calendar.454_sub", "Quarters of 4, 5 and 4 weeks"),
    ("interview.calendar.544", "5-4-4 weeks"),
    ("interview.calendar.544_sub", "Quarters of 5, 4 and 4 weeks"),
    ("interview.calendar.note", "Retail periods change how reports group dates. Kept on this computer until the books engine stores it."),
    ("interview.chart.intro", "Pick the starter list of accounts closest to your business. You can add, rename or remove accounts any time."),
    ("interview.chart.count", "{n} accounts"),
    ("interview.features.intro", "Turn on only what you need; you can change it later."),
    ("interview.features.sales_tax", "I collect sales tax"),
    ("interview.features.tax_rate", "Default sales tax rate (%)"),
    ("interview.features.tax_rate_hint", "For example 8.25. Rates by location come later."),
    ("interview.features.inventory", "I track inventory (products I buy and resell)"),
    ("interview.features.basis", "How should reports count income?"),
    ("interview.basis.accrual", "Accrual (when invoiced or billed)"),
    ("interview.basis.accrual_sub", "The usual choice; every report has a cash toggle too"),
    ("interview.basis.cash", "Cash (when money moves)"),
    ("interview.basis.cash_sub", "Income when paid, expenses when you pay"),
    ("interview.security.closing", "Closing-date password"),
    ("interview.security.closing_intro", "Once you close a period, changing an entry in it needs this password. Optional; you can set it later."),
    ("interview.security.closing_password", "Closing-date password"),
    ("interview.security.closing_confirm", "Password again"),
    ("interview.security.closing_unavailable", "The books engine on this computer cannot take a closing-date password yet. You can set one after the next engine update."),
    ("interview.security.colour", "Company color"),
    ("interview.security.colour_intro", "A stripe in the title bar so you always know which company is open."),
    ("interview.colour.swatch", "Color {n}"),
    ("interview.review.same", "Same as the business name"),
    ("interview.review.yes", "Yes"),
    ("interview.review.no", "No"),
    ("interview.review.set", "Set"),
    ("interview.review.not_set", "Not set"),
    ("interview.next", "Next"),
    ("interview.back", "Back"),
    ("interview.create", "Create company"),
    ("interview.creating", "Creating the company and its accounts\u{2026}"),
    ("interview.done.title", "Your company is ready"),
    ("interview.done.body", "{n} accounts were set up. Next, keep your recovery key somewhere safe, then enter what you had on your start date."),
    ("interview.done.recovery", "Show my recovery key"),
    ("interview.done.opening", "Enter opening balances"),
    ("interview.done.later", "Go to Home"),
    ("setup.issue.name_required", "The business needs a name."),
    ("setup.issue.name_too_long", "That name is too long (255 characters at most)."),
    ("setup.issue.fiscal_month", "Pick the month the fiscal year starts in."),
    ("setup.issue.tax_rate", "The tax rate should be a percentage between 0 and 100."),
    ("setup.issue.closing_password_short", "The closing-date password needs at least 12 characters, and both entries must match."),
    ("setup.issue.closing_password_unavailable", "The books engine cannot take a closing-date password yet; leave it blank for now."),
    ("recovery.tab", "Recovery Key"),
    ("recovery.title", "Your recovery key"),
    ("recovery.intro", "If you forget the owner password, this key resets it. It is shown once. ClientLedger keeps only a scrambled copy, so it cannot show it again."),
    ("recovery.your_key", "Recovery key"),
    ("recovery.key_label", "Recovery key"),
    ("recovery.show", "Show"),
    ("recovery.hide", "Hide"),
    ("recovery.copy", "Copy"),
    ("recovery.copied", "Copied. Paste it somewhere safe, then clear your clipboard."),
    ("recovery.save_path", "Save to file"),
    ("recovery.save", "Save"),
    ("recovery.saved", "Saved to {path}. Move that file somewhere safe."),
    ("recovery.print_later", "Printing arrives with form printing; until then save or copy the key."),
    ("recovery.confirm", "I saved my recovery key somewhere safe"),
    ("recovery.done", "Done"),
    ("recovery.close", "Close"),
    ("recovery.working", "Making your recovery key\u{2026}"),
    ("recovery.unavailable", "The books engine on this computer does not make recovery keys yet. After the next engine update, Users and Permissions can make one."),
    ("opening.tab", "Opening Balances"),
    ("opening.title", "Opening balances"),
    ("opening.intro", "Enter what you had on the day your books start. Everything posts as one opening entry; invoices and bills become open items."),
    ("opening.start", "Start date"),
    ("opening.start_hint", "The first day ClientLedger keeps your books. Balances are as of this date."),
    ("opening.start_date", "Books start on"),
    ("opening.bank", "Bank and card balances"),
    ("opening.balance", "Balance"),
    ("opening.pick_account", "Pick an account"),
    ("opening.sign_hint", "Enter what the statement says. A credit card or loan balance is what you owe."),
    ("opening.add_row", "Add a line"),
    ("opening.remove_row", "Remove"),
    ("opening.invoices", "Open invoices (clients who owe you)"),
    ("opening.invoices_hint", "One line per unpaid invoice as of the start date."),
    ("opening.bills", "Unpaid bills (what you owe vendors)"),
    ("opening.bills_hint", "One line per bill you had not paid yet."),
    ("opening.other", "Other balances"),
    ("opening.other_hint", "Equipment, vehicles, loans, owner money put in."),
    ("opening.client", "Client"),
    ("opening.vendor", "Vendor"),
    ("opening.date", "Date"),
    ("opening.number", "Number"),
    ("opening.amount", "Amount"),
    ("opening.doc_line", "Opening balance"),
    ("opening.doc_memo", "Opening balance as of the start date"),
    ("opening.post", "Post opening balances"),
    ("opening.skip", "Skip for now"),
    ("opening.posting", "Posting\u{2026}"),
    ("opening.issue.start_date", "Enter the start date as MM/DD/YYYY."),
    ("opening.issue.account", "Line {row}: pick an account."),
    ("opening.issue.amount", "Line {row}: that is not an amount."),
    ("opening.issue.doc_amount", "{what}, line {row}: the amount must be more than zero."),
    ("opening.issue.doc_name", "{what}, line {row}: enter who it is for."),
    ("opening.issue.nothing", "Nothing to post yet. Enter at least one balance, invoice or bill."),
    ("opening.toast.posted", "Opening balances posted."),
    ("opening.review.title", "Balance sheet on your start date"),
    ("opening.review.summary", "Posted: {lines} lines in the opening entry, {invoices} invoices, {bills} bills."),
    ("opening.review.no_report", "The balance sheet will show on Reports."),
    ("opening.review.assets", "What you have"),
    ("opening.review.liabilities", "What you owe"),
    ("opening.review.equity", "Equity"),
    ("opening.review.total", "Total"),
    ("opening.review.done", "Done"),
    ("backup.title", "Back Up Company"),
    ("backup.portable_title", "Make Portable Copy"),
    ("backup.export_title", "Export All Data"),
    ("backup.intro", "A .clbackup file with everything in this company, locked with your backup passphrase. Keep the last 10 and one a month for a year."),
    ("backup.portable_intro", "A .clbackup to carry to another computer. Portable copies are never cleaned up automatically."),
    ("backup.export_intro", "Every list and every transaction with its lines as CSV files in one zip. They open in Excel or any spreadsheet program."),
    ("backup.company", "Company: {name}"),
    ("backup.folder", "Save in folder"),
    ("backup.passphrase_title", "Set a backup passphrase"),
    ("backup.passphrase_intro", "Backups are locked with a passphrase that stays in this computer's keychain. Write it down: a lost passphrase makes backups unreadable."),
    ("backup.passphrase", "Backup passphrase (12+ characters)"),
    ("backup.passphrase_confirm", "Passphrase again"),
    ("backup.no_company", "Open a company first."),
    ("backup.go", "Start"),
    ("backup.progress", "Working\u{2026}"),
    ("backup.progress_percent", "Working\u{2026} {p}%"),
    ("backup.progress_copy", "Copying the file\u{2026}"),
    ("backup.progress_verify", "Checking the file\u{2026}"),
    ("backup.done", "Backed up to {path}"),
    ("backup.export_done", "Exported to {path}"),
    ("backup.toast", "Backup finished."),
    ("delete.tab", "Delete Company"),
    ("delete.title", "Delete company"),
    ("delete.intro", "This removes {name} from this computer after taking one last backup."),
    ("delete.warning", "A backup is taken first and kept. Everything else about this company goes away."),
    ("delete.type_name", "Type the company name exactly"),
    ("delete.closing_password", "Closing-date password (if one is set)"),
    ("delete.closing_hint", "Needed only when this company has a closing-date password."),
    ("delete.unavailable", "The books engine on this computer cannot delete companies safely yet. After the next engine update this works; your data is untouched."),
    ("delete.go", "Back up and delete"),
    ("delete.working", "Backing up, then deleting\u{2026}"),
    ("delete.done", "{name} was deleted."),
    ("delete.done_with_backup", "Deleted. The last backup is at {path}"),
    ("error.company.file", "A file could not be read or written."),
    ("fix.company.file", "Check the folder exists and you can write to it, then try again."),
    ("error.company.settings", "The list of companies on this computer could not be saved."),
    ("error.company.keychain", "The keychain on this computer could not be used."),
    ("fix.company.keychain", "Unlock the keychain or sign in to your computer account again, then try again."),
    ("error.company.setup", "Something in the setup needs a look."),
    ("fix.company.setup", "Check the highlighted answers."),
    ("error.company.no_passphrase", "No backup passphrase is set yet."),
    ("fix.company.no_passphrase", "Set one on the Back Up screen; it is shown once."),
    ("error.company.export_failed", "The books engine could not finish the backup."),
    ("error.company.not_a_backup", "That file is not a ClientLedger backup."),
    ("fix.company.not_a_backup", "Pick a .clbackup file made by ClientLedger."),
    ("error.company.tampered", "This backup file changed after it was written."),
    ("fix.company.tampered", "Use another copy of the backup. A changed file is never restored."),
    ("error.company.copy_mismatch", "The backup file did not copy correctly."),
    ("fix.company.copy_mismatch", "Try again, or back up to a different folder."),
    ("error.company.name_mismatch", "The name you typed does not match."),
    ("fix.company.name_mismatch", "Type the company name exactly as shown, including capitals."),
    ("error.company.unknown", "That company is not on this computer."),
    ("error.company.unavailable", "The books engine on this computer cannot do this yet."),
    ("fix.company.unavailable", "It arrives with the next engine update."),
    // About and privacy
    ("about.title", "About ClientLedger"),
    ("about.version", "Version"),
    ("about.close", "Close"),
    ("about.h.license", "License"),
    ("about.license.body", "ClientLedger is free, open-source software under the MIT license. The source code is public. There are no license keys, activation, or seat limits."),
    ("about.h.engine", "The accounting engine"),
    ("about.engine.body", "ClientLedger keeps your books with the Vibe MyBooks engine, which comes inside official ClientLedger releases from Kisaes LLC. You may use it for the internal business operations of you and your company, including keeping your own books. You may not copy or pass the engine on to others. Keeping books for other people or organizations as a service is not internal use and needs a separate license from Kisaes LLC. Write to licensing@kisaes.com with questions."),
    ("about.h.components", "Bundled components"),
    ("about.components.body", "PostgreSQL (PostgreSQL License) stores your books on this computer. IBM Plex Sans and IBM Plex Mono (SIL Open Font License 1.1) draw the text. Other libraries keep their own licenses, listed in ATTRIBUTION.md with the app."),
    ("about.font_license.show", "Show the font license"),
    ("about.h.privacy", "Privacy"),
    ("about.h.affiliation", "Not affiliated with Intuit"),
    ("about.affiliation.body", "ClientLedger is an independent project. It is not affiliated with, endorsed by, or sponsored by Intuit Inc. QuickBooks is a trademark of Intuit Inc., mentioned only to describe import and migration features."),
    ("privacy.title", "Privacy notice"),
    ("privacy.body", "Your books stay on this computer. ClientLedger collects no analytics and sends no crash reports. Logs stay on this computer."),
    ("privacy.h.online", "When it goes online"),
    ("privacy.online.body", "ClientLedger connects to the internet only when you turn on a feature that needs it: AI suggestions with your own key (or a local model that never leaves your computer), emailing invoices through your own mail server, or when you choose Check for updates."),
    ("privacy.h.secrets", "Keys and passwords"),
    ("privacy.secrets.body", "AI keys, mail passwords, and the database password are kept in your operating system keychain. They never appear in logs, backups, files you send, or error reports."),
    // Errors
    ("error.dialog.title", "Something went wrong"),
    ("error.dialog.id", "Error ID:"),
    ("error.dialog.copy", "Copy details"),
    ("error.dialog.show_details", "Show what will be copied"),
    ("error.dialog.details_label", "These details contain no amounts, names, or passwords."),
    ("error.dialog.copied", "Copied"),
    ("error.dialog.close", "Close"),
    ("error.internal", "Something went wrong inside ClientLedger."),
    ("fix.internal.report", "Try again. If it keeps happening, copy the details and report it on GitHub."),
    ("error.money.format", "That is not an amount ClientLedger can read."),
    ("fix.money.format", "Use digits, for example 1,250.00."),
    ("error.money.too_precise", "That amount has more than two decimal places."),
    ("fix.money.too_precise", "Round it to cents."),
    ("error.money.too_large", "That amount is too large."),
    ("fix.money.too_large", "Check for an extra digit."),
    ("error.money.bad_weights", "These amounts cannot be split that way."),
    ("fix.money.bad_weights", "Check the split percentages."),
    ("error.money.not_whole_cents", "That amount is not in whole cents."),
    ("fix.money.not_whole_cents", "Round it to cents."),
    ("error.amount.empty", "Enter an amount."),
    ("fix.amount.empty", "Type the amount, for example 125.00."),
    ("error.amount.invalid", "That is not an amount ClientLedger can read."),
    ("fix.amount.invalid", "Use digits, for example 1,250.00."),
    ("error.amount.ambiguous", "It is not clear which mark is the decimal point."),
    ("fix.amount.ambiguous", "Write the amount with a period for cents, for example 1250.00."),
    ("error.amount.too_many_decimals", "That amount has more than two decimal places."),
    ("fix.amount.too_many_decimals", "Round it to cents."),
    ("error.amount.too_large", "That amount is too large."),
    ("fix.amount.too_large", "Check for an extra digit."),
    ("error.amount.sign_conflict", "The minus sign and the parentheses disagree."),
    ("fix.amount.sign_conflict", "Use one or the other."),
    ("error.date.format", "That is not a date ClientLedger can read."),
    ("fix.date.format", "Type it as month/day/year, for example 11/04/2026."),
    ("error.date.no_such_day", "There is no such day on the calendar."),
    ("fix.date.no_such_day", "Check the day and month."),
    ("error.date.out_of_range", "That date is too far in the past or future."),
    ("fix.date.out_of_range", "Use a date between 1900 and 2199."),
    ("error.fiscal.bad_month", "That is not a month of the year."),
    ("fix.fiscal.bad_month", "Pick a month from 1 to 12."),
    ("error.api.validation", "The books engine did not accept that entry."),
    ("fix.api.validation", "Check the highlighted fields and try again."),
    ("error.api.local", "Something in this entry needs fixing before it can be saved."),
    ("error.api.conflict_stale", "Someone or something else changed this record while you were editing it."),
    ("fix.api.reload", "Reload it to see the latest version, then make your change again."),
    ("error.api.conflict_in_use", "This record is in use, so it cannot be changed that way."),
    ("error.api.conflict_exists", "A record with that name already exists."),
    ("error.api.busy", "ClientLedger is busy right now."),
    ("fix.api.try_again_soon", "Wait a moment and try again."),
    ("error.api.conflict", "This change conflicts with something already in your books."),
    ("error.api.closed_period", "That date falls in a period you have closed."),
    ("fix.api.closed_period", "Pick a date after the closing date, or ask an administrator to reopen the period."),
    ("error.api.offline", "That needs the internet, and ClientLedger is working offline."),
    ("fix.api.offline", "Turn off Work offline in Preferences if you want to use it."),
    ("error.api.feature_off", "That feature is not turned on."),
    ("fix.api.feature_off", "Turn it on in Preferences."),
    ("error.api.bad_credentials", "The email or password is not right."),
    ("error.api.session_expired", "Your sign-in has timed out."),
    ("fix.api.sign_in", "Sign in again."),
    ("error.api.tfa_invalid", "That code is not right."),
    ("error.api.locked", "This account is locked for a little while after too many tries."),
    ("fix.api.wait", "Wait a few minutes, then try again."),
    ("error.api.deactivated", "This account has been turned off."),
    ("error.api.forbidden", "You do not have permission to do that."),
    ("error.api.not_found", "That record could not be found. It may have been deleted."),
    ("error.api.unsupported", "That is not available in ClientLedger."),
    ("error.api.no_company", "No company is open."),
    ("fix.api.open_company", "Open a company first (File, Open or Switch Company)."),
    ("error.api.too_large", "That file or entry is too large."),
    ("error.api.engine_unavailable", "The books engine is not answering."),
    ("error.api.outcome_unknown", "ClientLedger could not tell whether that was saved."),
    ("fix.api.check_again", "Look for it in the register before entering it again, so it is not entered twice."),
    ("error.api.internal", "The books engine had a problem."),
    ("fix.api.report", "Try again. If it keeps happening, copy the details and report it on GitHub."),
    ("error.api.saved_unreadable", "Saved. ClientLedger could not show the result."),
    ("fix.api.reload_to_see", "Reload to see it. Do not enter it again."),
    ("error.api.stale_scope", "That result belonged to a company you have since left."),
    ("error.api.cancelled", "That was cancelled."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_come_back_as_the_key() {
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn command_labels_resolve() {
        assert_eq!(t("cmd.vendors.write_check"), "Write Check");
        assert!(has("cmd.help.about"));
        assert!(!has("cmd.not.a.command"));
    }

    #[test]
    fn placeholders_are_replaced() {
        assert_eq!(
            t_with("no.such.{x}", &[("x", "y")]),
            "no.such.{x}".replace("{x}", "y")
        );
    }

    fn source_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                source_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn every_key_used_in_the_source_is_in_the_catalog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        source_files(&root, &mut files);
        let mut missing = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            // Only code before the tests module counts.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for call in ["t(\"", "t_with(\""] {
                let mut rest = code;
                while let Some(at) = rest.find(call) {
                    let before = rest[..at].chars().last();
                    rest = &rest[at + call.len()..];
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue; // part of a longer name such as `assert(`
                    }
                    let key = rest.split('"').next().unwrap_or("");
                    if !key.is_empty() && !has(key) {
                        missing.push(format!("{}: {key}", f.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn catalog_has_no_duplicate_keys_and_no_empty_text() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in ENGLISH {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert_ne!(*v, "", "{k}");
        }
    }
}
