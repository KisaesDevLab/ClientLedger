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
        && let Some(c) = commands::find(id).or_else(|| commands::find_dev(id))
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
    key.strip_prefix("cmd.").is_some_and(|id| {
        commands::find(id)
            .or_else(|| commands::find_dev(id))
            .is_some()
    }) || catalog().contains_key(key)
}

mod lists;

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| ENGLISH.iter().chain(lists::ENGLISH).copied().collect())
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
    ("orphan.lifecycle", "a company operation (open, create, back up, restore)"),
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
    ("open.practice_loading", "Building the practice company (about a minute)\u{2026}"),
    ("open.footer.offline", "Works fully offline"),
    ("open.footer.not_affiliated", "Not affiliated with Intuit or QuickBooks"),
    ("open.probing", "Getting ready\u{2026}"),
    ("open.engine_not_ready", "ClientLedger is still starting up. The status bar shows how it is doing."),
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
    ("open.restore.intro", "The backup is restored as a new company first and checked line by line. Your current company is kept until you confirm."),
    ("open.restore.file", "Backup file (.clbackup)"),
    ("open.restore.passphrase", "Backup passphrase"),
    ("open.restore.passphrase_hint", "Leave blank to use the passphrase saved on this computer. A file from another computer needs its passphrase."),
    ("open.restore.need_file", "Enter the path of the backup file."),
    ("open.restore.close_first", "Close the open company first (File, Close Company) so nothing is being saved while the backup comes in."),
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
    ("interview.calendar.note", "Retail periods change how reports group dates. For now this choice is saved on this computer only."),
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
    ("interview.security.closing_unavailable", "A closing-date password is not available in this version yet. You can set one after the next update."),
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
    ("setup.issue.closing_password_unavailable", "A closing-date password is not available in this version yet; leave it blank for now."),
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
    ("recovery.print_later", "You can print it later from Users and Permissions. For now, save it to a file or copy it."),
    ("recovery.confirm", "I saved my recovery key somewhere safe"),
    ("recovery.done", "Done"),
    ("recovery.close", "Close"),
    ("recovery.working", "Making your recovery key\u{2026}"),
    ("recovery.none_title", "No recovery key to show"),
    ("recovery.none_body", "Your recovery key was shown once when ClientLedger was set up. If you lost it, make a new one: the old key stops working the moment the new one is made."),
    ("recovery.rotate", "Make a new recovery key"),
    ("recovery.save_where", "Save it outside the ClientLedger folder (Documents is suggested) and never in a backup."),
    ("recovery.unavailable", "Recovery keys are not available in this version yet. After the next update, Users and Permissions can make one."),
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
    ("opening.other_hint", "Things you own (equipment, vehicles) and money you owe (loans, lines of credit), each as a positive number; and money you put in yourself."),
    ("opening.account", "Account"),
    ("opening.one_invoice", "invoice"),
    ("opening.many_invoices", "invoices"),
    ("opening.one_bill", "bill"),
    ("opening.many_bills", "bills"),
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
    ("opening.review.summary", "Posted: {lines} lines in the opening entry, {invoices}, {bills}."),
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
    ("delete.unavailable", "Deleting a company is not available in this version yet. After the next update this works; your data is untouched."),
    ("delete.go", "Back up and delete"),
    ("delete.working", "Backing up, then deleting\u{2026}"),
    ("delete.done", "{name} was deleted."),
    ("delete.done_with_backup", "Deleted. The last backup is at {path}"),
    ("error.company.file", "A file could not be read or written."),
    ("fix.company.file", "Check the folder exists and you can write to it, then try again."),
    ("error.company.settings", "The list of companies on this computer could not be saved."),
    ("error.company.keychain", "The keychain on this computer could not be used."),
    ("fix.company.keychain", "Unlock the keychain or sign in to your computer account again, then try again. On Linux a keyring service (GNOME Keyring or KDE Wallet, through Secret Service) must be installed and running."),
    ("error.company.setup", "Something in the setup needs a look."),
    ("fix.company.setup", "Check the highlighted answers."),
    ("error.company.no_passphrase", "No backup passphrase is set yet."),
    ("error.company.passphrase_short", "The backup passphrase needs at least 12 characters."),
    ("fix.company.passphrase_short", "Pick a longer passphrase; a short sentence works well."),
    ("error.company.passphrase_mismatch", "The two passphrase entries are not the same."),
    ("fix.company.passphrase_mismatch", "Type the same passphrase in both boxes."),
    ("error.company.passphrase", "That is not the passphrase this backup was made with."),
    ("fix.company.passphrase", "Type the passphrase that was set when the backup was made."),
    ("error.company.too_new", "This backup was made by a newer ClientLedger."),
    ("fix.company.too_new", "Update ClientLedger, then restore it."),
    ("error.company.job_failed", "The backup could not be finished."),
    ("fix.company.no_passphrase", "Set one on the Back Up screen; it is shown once."),
    ("error.company.not_a_backup", "That file is not a ClientLedger backup."),
    ("fix.company.not_a_backup", "Pick a .clbackup file made by ClientLedger."),
    ("error.company.tampered", "This backup file changed after it was written."),
    ("fix.company.tampered", "Use another copy of the backup. A changed file is never restored."),
    ("error.company.copy_mismatch", "The backup file did not copy correctly."),
    ("fix.company.copy_mismatch", "Try again, or back up to a different folder."),
    ("error.company.name_mismatch", "The name you typed does not match."),
    ("fix.company.name_mismatch", "Type the company name exactly as shown, including capitals."),
    ("error.company.unknown", "That company is not on this computer."),
    ("error.company.unavailable", "This is not available in this version yet."),
    ("fix.company.unavailable", "It arrives with the next update."),
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

    // Preferences (P09)
    ("prefs.title", "Preferences"),
    ("prefs.search", "Find a setting"),
    ("prefs.group.me", "Just for me"),
    ("prefs.group.company", "This company"),
    ("prefs.cat.look", "Look and feel"),
    ("prefs.cat.dates", "Dates and numbers"),
    ("prefs.cat.reminders", "Reminders"),
    ("prefs.cat.accounting", "Accounting"),
    ("prefs.cat.checks", "Checks and forms"),
    ("prefs.cat.sales_tax", "Sales tax"),
    ("prefs.cat.inventory", "Inventory"),
    ("prefs.cat.ai", "AI suggestions"),
    ("prefs.cat.email", "Email"),
    ("prefs.cat.inbox", "Receipt inbox"),
    ("prefs.cat.backups", "Backups"),
    ("prefs.cat.updates", "Updates"),
    ("prefs.save", "Save"),
    ("prefs.cancel", "Cancel"),
    ("prefs.saving", "Saving\u{2026}"),
    ("prefs.loading", "Reading the current settings\u{2026}"),
    ("prefs.saved", "Saved."),
    ("prefs.saved_waiting", "Saved. Your key stays in this computer's keychain; the online provider is not available yet and no online request is made."),
    ("prefs.saved_cap", "Saved. This month's cap is reached, so the online provider stays off until next month or a higher cap."),
    ("prefs.err.not_loaded", "Still reading the current settings; try again in a moment."),
    ("prefs.toast", "Preferences saved"),
    ("prefs.on", "On"),
    ("prefs.off", "Off"),
    ("prefs.err.number", "That needs to be a whole number in range."),
    ("prefs.err.rate", "Enter the tax rate as a percent with up to two decimals, such as 8.25."),
    // Just for me
    ("prefs.user.look_intro", "How ClientLedger looks on this computer."),
    ("prefs.user.theme", "Colors"),
    ("prefs.user.zoom", "Zoom"),
    ("prefs.user.zoom_hint", "Ctrl+Plus and Ctrl+Minus change this at any time."),
    ("prefs.user.start", "When a company opens, show"),
    ("prefs.user.start_home", "Home"),
    ("prefs.user.start_last", "The windows I had open"),
    ("prefs.user.start_feed", "Bank Feed"),
    ("prefs.user.dates_intro", "How dates and amounts are shown and typed."),
    ("prefs.user.date_format", "Dates"),
    ("prefs.user.date_mdy", "MM/DD/YYYY"),
    ("prefs.user.date_ymd", "YYYY-MM-DD"),
    ("prefs.user.negatives", "Negative amounts"),
    ("prefs.user.neg_minus", "-1,234.56"),
    ("prefs.user.neg_parens", "(1,234.56)"),
    ("prefs.user.warn_title", "Date warnings"),
    ("prefs.user.warn_intro", "Ask before saving a date far from today, so a typo in the year is caught."),
    ("prefs.user.warn_past", "Warn when a date is more than this many days in the past"),
    ("prefs.user.warn_future", "Warn when a date is more than this many days ahead"),
    ("prefs.user.reminders_intro", "What Home reminds you about."),
    ("prefs.user.bills_due", "Show bills due within this many days"),
    ("prefs.user.bills_due_hint", "Home lists bills that are overdue or due within this window."),
    ("prefs.user.backup_on_close", "Back up when I close a company"),
    ("prefs.user.backup_on_close_hint", "A backup is written to your backup folder each time you close or switch companies."),
    // Company
    ("prefs.company.accounting_intro", "Rules for this company's books. Your accountant can confirm these."),
    ("prefs.company.basis", "Basis"),
    ("prefs.company.accrual", "Accrual"),
    ("prefs.company.cash", "Cash"),
    ("prefs.company.basis_hint", "Accrual counts income when invoiced and expenses when billed; cash counts them when money moves. Reports can show either."),
    ("prefs.company.fy_month", "Fiscal year starts in"),
    ("prefs.company.closing_none", "No closing date is set."),
    ("prefs.company.closing_through", "Books are closed through {date}."),
    ("prefs.company.closing_hint", "Change the closing date and its password under Company > Set Closing Date."),
    ("prefs.company.account_numbers", "Use account numbers"),
    ("prefs.company.account_numbers_hint", "Shows each account's number before its name in the Chart of Accounts and in account lists, and sorts accounts by number."),
    ("prefs.company.checks_intro", "Check numbering, printing and form defaults."),
    ("prefs.company.checks_later", "Check options and default accounts are not available yet; they come with Write Check and printing."),
    ("prefs.company.finance_later", "Finance charge settings are not available yet; they come with Statements."),
    ("prefs.company.ten99_later", "1099 box mapping is not available yet; it comes with the Vendors list and the 1099 Summary report."),
    ("prefs.company.tax_intro", "Turn sales tax on when you charge it on invoices."),
    ("prefs.company.tax_on", "This company charges sales tax"),
    ("prefs.company.tax_rate", "Default rate (%)"),
    ("prefs.company.tax_hint", "Tax items, agencies and the Sales Tax report are not available yet; the default rate is used on new invoices."),
    ("prefs.company.inventory_intro", "Track quantities and average cost for the items you sell."),
    ("prefs.company.inventory_on", "This company tracks inventory"),
    ("prefs.company.inventory_hint", "Inventory items and reorder alerts are not available yet."),
    // AI
    ("prefs.ai.title", "AI suggestions"),
    ("prefs.ai.intro", "Off unless you turn it on. AI suggests accounts, payees, and tags, and reads bills \u{2014} it never sets an amount, and you accept every suggestion."),
    ("prefs.ai.where", "Where AI runs"),
    ("prefs.ai.provider_ollama", "On this computer (Ollama)"),
    ("prefs.ai.provider_anthropic", "Anthropic (your key)"),
    ("prefs.ai.provider_openai", "OpenAI (your key)"),
    ("prefs.ai.ollama_desc", "Works offline. Nothing leaves this computer. Needs Ollama installed."),
    ("prefs.ai.anthropic_desc", "Online. Sends the minimum needed, with names and numbers removed."),
    ("prefs.ai.openai_desc", "Online. Same limits as Anthropic."),
    ("prefs.ai.ollama_url", "Ollama address"),
    ("prefs.ai.ollama_model", "Model"),
    ("prefs.ai.key", "API key"),
    ("prefs.ai.key_saved", "A key is saved in your computer's keychain."),
    ("prefs.ai.key_missing", "No key saved yet. Paste your key above and Save."),
    ("prefs.ai.key_remove", "Remove key"),
    ("prefs.ai.waiting_engine", "Online providers are not available yet: your key stays in this computer's keychain and no online request is made."),
    ("prefs.ai.test", "Test"),
    ("prefs.ai.testing", "Testing\u{2026}"),
    ("prefs.ai.test_ok", "Connected \u{2713}"),
    ("prefs.ai.test_failed", "Could not connect. Check the address, the model, or your key."),
    ("prefs.ai.test_needs_key", "Save a key first, then test."),
    ("prefs.ai.needs_key", "Paste the provider's API key before turning an online provider on."),
    ("prefs.ai.needs_consent_again", "AI is on, but your agreement to the notice is missing or out of date for this choice: please read it and tick \"I agree\" before saving these AI changes."),
    ("prefs.ai.needs_consent", "Read the notice and tick \"I agree\" before turning an online provider on."),
    ("prefs.ai.notice_title", "Before anything goes online"),
    ("prefs.ai.notice_text", "With an online provider, ClientLedger sends short, redacted snippets (descriptions, payee names with numbers removed, document text) to the provider you chose, under your own account and key. Amounts are never sent and never set by AI. Account numbers, tax IDs, emails and phone numbers are removed first."),
    ("prefs.ai.notice_images", "A scanned bill or receipt is sent as an image, which cannot be redacted: only turn on \"Read bills and receipts\" with an online provider if that is acceptable to you."),
    ("prefs.ai.agree", "I agree. The date, my name and the notice version are recorded in the audit trail; the provider is noted in this company's settings."),
    ("prefs.ai.recorded_stale", "Agreement recorded on {when} for an older notice: please read and agree again."),
    ("prefs.ai.notice_local", "With Ollama nothing leaves this computer. Your agreement still records that AI is on for this company."),
    ("prefs.ai.ollama_local", "The address must be on this computer or your own network; the model must be installed in Ollama."),
    ("prefs.ai.err_ollama_url", "The Ollama address must be http or https, without a user name, on this computer or your own network."),
    ("prefs.ai.usage_meter", "Online AI usage this month"),
    ("prefs.ai.cap_unit", "tokens"),
    ("prefs.ai.recorded", "Agreement recorded on {when}."),
    ("prefs.ai.features", "What AI may help with"),
    ("prefs.ai.feat_feed", "Bank Feed suggestions"),
    ("prefs.ai.feat_feed_desc", "Suggest a payee and account for imported transactions"),
    ("prefs.ai.feat_docs", "Read bills and receipts"),
    ("prefs.ai.feat_docs_desc", "Fill vendor, date, ref. no., and total from a PDF or photo for you to check"),
    ("prefs.ai.feat_ask", "Ask a question in Reports"),
    ("prefs.ai.feat_ask_desc", "Pick the right report and dates; numbers always come from your books"),
    ("prefs.ai.feat_rules", "Suggest rules"),
    ("prefs.ai.feat_rules_desc", "Offer a Bank Feed rule after you accept the same suggestion a few times"),
    ("prefs.ai.usage", "Online AI usage"),
    ("prefs.ai.usage_of", "{used} of {cap} tokens this month"),
    ("prefs.ai.usage_nocap", "{used} tokens this month (no cap)"),
    ("prefs.ai.cap", "Monthly cap"),
    ("prefs.ai.estimate", "{calls} online calls this month, about ${dollars} (estimated, not a bill)."),
    ("prefs.ai.usage_note", "Only applies to Anthropic or OpenAI. Keys are kept in your computer's keychain and never appear in backups, transfer files, or logs."),
    // Email
    ("prefs.email.title", "Email"),
    ("prefs.email.intro", "Send invoices and statements through your own email server. Without it, documents go to the Outbox or a PDF."),
    ("prefs.email.host", "Server"),
    ("prefs.email.port", "Port"),
    ("prefs.email.security", "Security"),
    ("prefs.email.starttls", "STARTTLS (587)"),
    ("prefs.email.tls", "TLS (465)"),
    ("prefs.email.plain", "None (office server only)"),
    ("prefs.email.starttls_tip", "The usual choice: the connection starts plain and switches to TLS before anything is sent. Port 587."),
    ("prefs.email.tls_tip", "TLS from the first byte. Port 465."),
    ("prefs.email.plain_tip", "No encryption: only for a mail relay on this computer or your office network."),
    ("prefs.email.err_plain", "A plain connection is only allowed to a server on this computer or your office network."),
    ("prefs.email.err_user", "The user name has a character that cannot be sent."),
    ("prefs.email.err_from_name", "The From name has a character that cannot be sent."),
    ("prefs.email.err_security_port", "TLS uses port 465; STARTTLS uses another port, usually 587."),
    ("prefs.email.user", "User name"),
    ("prefs.email.password", "Password"),
    ("prefs.email.password_saved", "A password is saved in your computer's keychain."),
    ("prefs.email.password_missing", "No password saved yet."),
    ("prefs.email.password_remove", "Remove password"),
    ("prefs.email.from", "From address"),
    ("prefs.email.from_name", "From name"),
    ("prefs.email.test_title", "Try it"),
    ("prefs.email.test_to", "Send a test message to (optional)"),
    ("prefs.email.test", "Test connection"),
    ("prefs.email.testing", "Testing\u{2026}"),
    ("prefs.email.test_ok", "Connected \u{2713}"),
    ("prefs.email.test_failed", "Could not connect. Check the server, port, security, user name and password."),
    ("prefs.email.test_hint", "The test uses the password above without saving it anywhere else."),
    ("prefs.email.sends_later", "Sending invoices by email is not available yet; it comes with Create Invoice."),
    ("prefs.email.err_host", "Enter the mail server's address."),
    ("prefs.email.err_port", "The port must be a number from 1 to 65535."),
    ("prefs.email.err_from", "The From address must be an email address."),
    // Receipt inbox, backups, updates
    ("prefs.inbox.intro", "New photos and PDFs dropped into this folder appear for attaching to bills, checks, or bank items."),
    ("prefs.inbox.watch", "Watch a folder for receipts"),
    ("prefs.inbox.folder", "Folder"),
    ("prefs.inbox.folder_hint", "A folder your phone or scanner can save into, such as a synced Receipts folder."),
    ("prefs.inbox.err_absolute", "Enter the full path of the folder."),
    ("prefs.inbox.err_network", "A network folder cannot be watched; pick a folder on this computer or a synced one."),
    ("prefs.inbox.err_missing", "That folder does not exist yet."),
    ("prefs.inbox.later", "The inbox itself is not available yet; it comes with Enter Bill."),
    ("prefs.backups.intro", "Where backups go and when they are made."),
    ("prefs.backups.folder", "Backup folder"),
    ("prefs.backups.folder_hint", "Pick a folder on another drive or a synced folder, so a backup survives this computer."),
    ("prefs.backups.schedule_later", "Scheduled backups and the backup reminder are not available yet."),
    ("prefs.updates.intro", "ClientLedger never checks for updates by itself."),
    ("prefs.updates.manual_only", "Updates are checked only when you ask."),
    ("prefs.updates.version", "This is ClientLedger {v}."),
    ("prefs.updates.check", "Check for updates"),
    ("prefs.updates.later", "The check itself is not available yet; releases are on GitHub."),
    // Ledger grid (cl-grid and cl-model keys, written as `TextKey("...")`)
    ("grid.a11y.cell", "{column}, {row}"),
    ("grid.a11y.line", "line {n}"),
    ("grid.a11y.row_of", "row {n} of {total}"),
    ("grid.a11y.checked", "Checked"),
    ("grid.a11y.unchecked", "Not checked"),
    ("grid.a11y.cleared", "Cleared"),
    ("grid.a11y.reconciled", "Reconciled"),
    ("grid.a11y.loading", "Loading\u{2026}"),
    ("grid.a11y.locked", "Locked: this date is in a closed period"),
    ("grid.a11y.suggested", "Suggested. Press Tab to accept it, or type to replace it."),
    ("grid.a11y.row_problem", "This line has a problem"),
    ("grid.a11y.scrollbar", "Scroll rows"),
    ("grid.a11y.header_row", "Column headers"),
    ("grid.popup.loading", "Loading the list\u{2026}"),
    ("grid.popup.no_matches", "Nothing matches"),
    ("grid.popup.add_new", "Add \u{201c}{text}\u{201d}\u{2026}"),
    ("grid.popup.full_list", "All {column}"),
    ("grid.amount.empty", "Enter an amount."),
    ("grid.amount.invalid", "That is not an amount."),
    ("grid.amount.ambiguous", "It is not clear whether the comma or the period marks the cents"),
    ("grid.amount.ambiguous.fix", "Type the amount with a period for cents, such as 1234.50."),
    ("grid.amount.too_many_decimals", "Too many digits after the decimal point"),
    ("grid.amount.too_many_decimals.fix", "Type no more than two digits for cents."),
    ("grid.amount.too_large", "That amount is too large for this field."),
    ("grid.amount.sign_conflict", "Use a minus sign or parentheses for a negative amount, not both."),
    ("grid.amount.no_negative", "This column takes zero or more"),
    ("grid.amount.no_negative.fix", "Put a negative amount in the other column."),
    ("grid.calc.preview", "= {value}"),
    ("grid.calc.no_grouping", "Parentheses only mark a negative number here"),
    ("grid.calc.no_grouping.fix", "Work the calculation out in steps."),
    ("grid.calc.divide_by_zero", "Cannot divide by zero."),
    ("grid.calc.incomplete", "The calculation is not finished."),
    ("grid.calc.too_long", "The calculation is too long."),
    ("grid.calc.too_complex", "The calculation is too complex. Work it out in steps."),
    ("grid.date.empty", "Enter a date."),
    ("grid.date.invalid", "That is not a date. Type it as MM/DD/YYYY."),
    ("grid.date.no_such_day", "That day does not exist."),
    ("grid.date.out_of_range", "That date is too far in the past or the future."),
    ("grid.paste.one_value", "Paste one amount at a time."),
    ("grid.picker.not_in_list", "Not in the list. Pick an entry, or choose Add to create it."),
    ("grid.tags.not_in_list", "That tag is not in the list."),
    ("grid.tags.replaced", "Only one tag per group: the earlier tag was replaced."),
    ("grid.tags.duplicate", "That tag is already on this line."),
    ("grid.text.too_long", "That text is too long for this field."),
    ("grid.row.gone", "That line is no longer here."),
    (
        "grid.form.saving",
        "Saving. Wait a moment, then make your change again.",
    ),
    ("grid.cell.changed_underneath", "This cell changed while you were typing. Press Esc to see the new value."),
    // Grid demo (developer-only, --demo)
    ("dev.grid_demo.title", "Grid demo"),
    ("dev.grid_demo.synthetic", "A developer screen with made-up data. Nothing here is saved to a company."),
    ("dev.grid_demo.show_register", "Show 200,000 rows"),
    ("dev.grid_demo.show_bills", "Pay bills example"),
    ("dev.grid_demo.show_lines", "Show the expense lines"),
    ("dev.grid_demo.one_line", "One line"),
    ("dev.grid_demo.check_amount", "Check amount"),
    ("dev.grid_demo.lines_label", "Expense lines"),
    ("dev.grid_demo.register_label", "Register"),
    ("dev.grid_demo.bills_label", "Bills to pay"),
    ("dev.grid_demo.lines_total", "Lines total {total}"),
    ("dev.grid_demo.too_large", "too large to add up"),
    ("dev.grid_demo.matches", "\u{2713} matches the check amount"),
    ("dev.grid_demo.does_not_match", "does not match the check amount yet"),
    ("dev.grid_demo.save", "Save"),
    ("dev.grid_demo.save_new", "Save & New"),
    ("dev.grid_demo.saved_count", "Saved this session: {n}"),
    ("dev.grid_demo.bills_selected", "{n} selected"),
    ("dev.grid_demo.bad_value", "That value does not fit this column."),
    ("dev.grid_demo.need_account", "Pick an account for this line."),
    ("dev.grid_demo.billable_needs_job", "A billable line needs a Client:Job."),
    ("dev.grid_demo.not_balanced", "The lines total does not match the check amount."),
    ("dev.grid_demo.not_balanced.fix", "Change a line or the check amount so they match."),
    ("dev.grid_demo.col.account", "Account"),
    ("dev.grid_demo.col.account_memo", "Account \u{b7} Memo"),
    ("dev.grid_demo.col.amount", "Amount"),
    ("dev.grid_demo.col.memo", "Memo"),
    ("dev.grid_demo.col.service_date", "Service date"),
    ("dev.grid_demo.col.job", "Client:Job"),
    ("dev.grid_demo.col.billable", "Billable"),
    ("dev.grid_demo.col.tags", "Tags"),
    ("dev.grid_demo.col.date", "Date"),
    ("dev.grid_demo.col.number", "Number"),
    ("dev.grid_demo.col.type", "Type"),
    ("dev.grid_demo.col.payee", "Payee"),
    ("dev.grid_demo.col.payment", "Payment"),
    ("dev.grid_demo.col.mark", "\u{2713}"),
    ("dev.grid_demo.col.mark_help", "R = reconciled, \u{2713} = cleared"),
    ("dev.grid_demo.col.deposit", "Deposit"),
    ("dev.grid_demo.col.balance", "Balance"),
    ("dev.grid_demo.col.select", "Pay"),
    ("dev.grid_demo.col.vendor", "Vendor"),
    ("dev.grid_demo.col.due", "Due"),
    ("dev.grid_demo.col.amount_due", "Amount due"),
    ("dev.grid_demo.col.to_pay", "Amount to pay"),
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

    /// Every `TextKey("…")` written in cl-grid, cl-model and cl-ui code
    /// (outside test modules and comments) resolves (P10 step 15).
    #[test]
    fn every_text_key_in_grid_model_and_ui_is_in_the_catalog() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["../cl-grid/src", "../cl-model/src", "src"] {
            source_files(&here.join(dir), &mut files);
        }
        let mut missing = Vec::new();
        let mut found = 0;
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for line in code.lines() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let mut rest = line;
                while let Some(at) = rest.find("TextKey(\"") {
                    rest = &rest[at + "TextKey(\"".len()..];
                    let key = rest.split('"').next().unwrap_or("");
                    found += 1;
                    if !has(key) {
                        missing.push(format!("{}: {key}", f.display()));
                    }
                }
            }
        }
        assert!(found > 40, "the scan found only {found} keys");
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn grid_placeholders_are_the_ones_the_grid_fills() {
        for (key, names) in [
            ("grid.popup.add_new", &["{text}"][..]),
            ("grid.popup.full_list", &["{column}"]),
            ("grid.a11y.cell", &["{column}", "{row}"]),
            ("grid.a11y.row_of", &["{n}", "{total}"]),
        ] {
            for n in names {
                assert!(t(key).contains(n), "{key} lacks {n}");
            }
        }
    }

    #[test]
    fn catalog_has_no_duplicate_keys_and_no_empty_text() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in ENGLISH.iter().chain(lists::ENGLISH) {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert_ne!(*v, "", "{k}");
        }
    }
}
