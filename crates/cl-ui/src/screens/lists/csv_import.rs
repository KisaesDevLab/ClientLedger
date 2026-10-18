//! Import or Export a List (P11 step 6).
//!
//! Export writes one list to a CSV file in a folder the owner picks: the
//! Chart of Accounts, clients and vendors, and items come from the engine's
//! export routes; tags are written here from the tag list (the engine has
//! no tag export route). Import reads the owner's CSV file
//! (`cl_import::csv`), suggests which column fills which field
//! (`cl_model::lists::csv_map`), lets the owner change that mapping, shows
//! the rows that will be left out and why, and sends the rest: accounts and
//! contacts through the engine's import routes, items one at a time through
//! the item save (the engine's item import waits for U-46). An import only
//! adds records (existing account numbers and item names are skipped, never
//! overwritten), so it needs no backup first (Q-P11x).
//!
//! Keyboard: Tab moves through the list choice, the path fields, the
//! column choices and the buttons; Enter in a path field runs it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{
    Account, AccountFields, AccountFilter, AccountImportRow, AccountType, ApiError, ConflictReason,
    ContactFields, ContactImportRow, ContactType, IdempotencyKey, ItemFields, ItemFilter,
    OrphanLabel, RequestGroup, Tag, TagFilter, TagGroup,
};
use cl_core::ids::{AccountId, TenantId};
use cl_import::csv;
use cl_model::lists::csv_map::{
    self, Built, CsvList, ItemLookups, Mapped, MappingProblem, RowProblem, Target,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::lists::accounts::model::api_type;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.import_export";

/// Largest file read, in bytes.
const FILE_MAX_BYTES: u64 = 20 * 1024 * 1024;
/// Left-out rows listed on screen (the count says how many in all).
const ISSUES_SHOWN: usize = 50;

/// The list being imported or exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListChoice {
    /// Chart of Accounts.
    Accounts,
    /// Clients and vendors.
    Contacts,
    /// Items.
    Items,
    /// Tags (export only).
    Tags,
}

impl ListChoice {
    const ALL: [ListChoice; 4] = [
        ListChoice::Accounts,
        ListChoice::Contacts,
        ListChoice::Items,
        ListChoice::Tags,
    ];

    fn csv(self) -> Option<CsvList> {
        match self {
            ListChoice::Accounts => Some(CsvList::Accounts),
            ListChoice::Contacts => Some(CsvList::Contacts),
            ListChoice::Items => Some(CsvList::Items),
            ListChoice::Tags => None,
        }
    }

    fn label(self) -> String {
        match self.csv() {
            Some(l) => t(l.label().as_str()),
            None => t("csv.list.tags"),
        }
    }

    fn key(self) -> &'static str {
        match self.csv() {
            Some(l) => l.key(),
            None => "tags",
        }
    }

    fn file_stem(self) -> &'static str {
        match self {
            ListChoice::Accounts => "chart-of-accounts",
            ListChoice::Contacts => "clients-and-vendors",
            ListChoice::Items => "items",
            ListChoice::Tags => "tags",
        }
    }
}

/// A file as read: its header and data rows with their lines.
#[derive(Debug, Clone, Default)]
struct Parsed {
    headers: Vec<String>,
    rows: Vec<(usize, Vec<String>)>,
}

/// Why a row was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Skip {
    /// The row's own problem, found before sending.
    Row(RowProblem),
    /// The engine already has it (an account number, an item name).
    Exists,
    /// A system account can't be changed.
    Other,
}

/// What an import did.
#[derive(Debug, Clone, Default)]
struct Outcome {
    imported: u64,
    skipped: Vec<(usize, Skip)>,
    /// The engine stopped the import part way (items only).
    stopped: Option<ApiError>,
}

/// Records an import needs to check against.
#[derive(Debug, Clone, Default)]
struct Lookups {
    accounts: Vec<Account>,
    item_names: Vec<String>,
}

/// The Import or Export a List window.
pub struct CsvScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    list: ListChoice,
    folder: String,
    export: Option<Task<String>>,
    exported: Option<PathBuf>,
    export_error: Option<String>,
    export_api_error: Option<ApiError>,
    file: String,
    parsed: Option<Parsed>,
    read_error: Option<String>,
    mapping: Vec<Option<Target>>,
    role: ContactType,
    default_income: Option<AccountId>,
    lookups: Option<Lookups>,
    lookups_task: Option<Task<Lookups>>,
    built: Built,
    problems: Vec<MappingProblem>,
    import: Option<Task<Outcome>>,
    outcome: Option<Outcome>,
    import_error: Option<ApiError>,
}

impl CsvScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, _tenant: Option<(TenantId, String)>) -> CsvScreen {
        let group = link.group().and_then(Result::ok);
        CsvScreen {
            folder: link.backup_dir.display().to_string(),
            link,
            group,
            list: ListChoice::Accounts,
            export: None,
            exported: None,
            export_error: None,
            export_api_error: None,
            file: String::new(),
            parsed: None,
            read_error: None,
            mapping: Vec::new(),
            role: ContactType::Customer,
            default_income: None,
            lookups: None,
            lookups_task: None,
            built: Built::default(),
            problems: Vec::new(),
            import: None,
            outcome: None,
            import_error: None,
        }
    }

    /// Chooses the list; a file already read is mapped afresh.
    pub fn set_list(&mut self, list: ListChoice) {
        if self.list == list {
            return;
        }
        self.list = list;
        self.exported = None;
        self.export_error = None;
        self.export_api_error = None;
        self.outcome = None;
        self.import_error = None;
        if let (Some(l), Some(p)) = (list.csv(), self.parsed.as_ref()) {
            self.mapping = csv_map::guess(l, &p.headers);
        }
        if list == ListChoice::Items {
            self.load_lookups();
        }
        self.rebuild();
    }

    // ---- export -------------------------------------------------------------

    /// Starts writing the chosen list to the folder.
    pub fn start_export(&mut self) {
        if self.export.is_some() {
            return;
        }
        self.exported = None;
        self.export_error = None;
        self.export_api_error = None;
        let list = self.list;
        if self.link.is_demo() {
            let text = demo_export(&self.link, list);
            self.export = Some(Task::ready(text));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let pending = group.read(&session, move |s| async move {
            match list {
                ListChoice::Accounts => s.accounts().export_csv().await,
                ListChoice::Contacts => s.contacts().export_csv(None).await,
                ListChoice::Items => s.items().export_csv().await,
                ListChoice::Tags => {
                    let tags = s.tags().list_all(&TagFilter::default()).await?;
                    let groups = s.tags().groups().await?;
                    Ok(tags_csv(&tags, &groups))
                }
            }
        });
        if let Ok(p) = pending {
            self.export = Some(Task::Pending(p));
        }
    }

    fn finish_export(&mut self, text: &str) {
        let dir = PathBuf::from(self.folder.trim());
        match write_export(&dir, self.list.file_stem(), text) {
            Ok(path) => {
                tracing::info!(list = self.list.key(), "list exported");
                self.exported = Some(path);
            }
            Err(e) => {
                tracing::warn!(list = self.list.key(), kind = ?e.kind(), "list export not written");
                self.export_error = Some(t("csv.export_failed"));
            }
        }
    }

    // ---- import -------------------------------------------------------------

    /// Reads the file named in the path field and suggests a mapping.
    pub fn read_file(&mut self) {
        self.parsed = None;
        self.read_error = None;
        self.outcome = None;
        self.import_error = None;
        let path = PathBuf::from(self.file.trim().trim_matches('"'));
        match read_csv(&path) {
            Ok(p) => {
                if let Some(l) = self.list.csv() {
                    self.mapping = csv_map::guess(l, &p.headers);
                }
                self.parsed = Some(p);
                if self.list == ListChoice::Items {
                    self.load_lookups();
                }
            }
            Err(msg) => self.read_error = Some(msg),
        }
        self.rebuild();
    }

    fn load_lookups(&mut self) {
        if self.lookups.is_some() || self.lookups_task.is_some() {
            return;
        }
        if let Some(mut d) = self.link.demo_state() {
            let accounts = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let item_names = d
                .items_mut()
                .iter()
                .map(|i| i.fields.name.clone())
                .collect();
            self.lookups_task = Some(Task::ready(Lookups {
                accounts,
                item_names,
            }));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            let items = s.items().list_all(&ItemFilter::default()).await?;
            Ok(Lookups {
                accounts,
                item_names: items.into_iter().map(|i| i.fields.name).collect(),
            })
        }) {
            self.lookups_task = Some(Task::Pending(p));
        }
    }

    fn income_accounts(&self) -> Vec<&Account> {
        self.lookups.as_ref().map_or_else(Vec::new, |l| {
            l.accounts
                .iter()
                .filter(|a| {
                    a.is_active
                        && matches!(
                            a.fields.account_type,
                            AccountType::Revenue | AccountType::OtherRevenue
                        )
                })
                .collect()
        })
    }

    /// Sets the field column `col` fills.
    pub fn set_mapping(&mut self, col: usize, target: Option<Target>) {
        if let Some(m) = self.mapping.get_mut(col) {
            *m = target;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let (Some(list), Some(p)) = (self.list.csv(), self.parsed.as_ref()) else {
            self.built = Built::default();
            self.problems.clear();
            return;
        };
        self.problems = csv_map::check_mapping(list, &self.mapping, p.rows.len());
        let incomes: Vec<(AccountId, String, Option<String>)> = self
            .income_accounts()
            .iter()
            .map(|a| (a.id, a.fields.name.clone(), a.fields.account_number.clone()))
            .collect();
        let names: Vec<String> = self
            .lookups
            .as_ref()
            .map(|l| l.item_names.iter().map(|n| n.to_lowercase()).collect())
            .unwrap_or_default();
        let resolve = |cell: &str| {
            incomes
                .iter()
                .find(|(id, name, number)| {
                    name.eq_ignore_ascii_case(cell)
                        || number.as_deref() == Some(cell)
                        || id.to_string() == cell.to_ascii_lowercase()
                })
                .map(|(id, _, _)| *id)
        };
        let taken = |n: &str| names.contains(&n.to_lowercase());
        let lookups = ItemLookups {
            income_account: &resolve,
            default_income: self.default_income,
            name_taken: &taken,
        };
        let items = (list == CsvList::Items && self.lookups.is_some()).then_some(&lookups);
        self.built = csv_map::build(list, &self.mapping, &p.rows, items);
    }

    fn can_import(&self) -> bool {
        self.list.csv().is_some()
            && self.parsed.is_some()
            && self.problems.is_empty()
            && !self.built.rows.is_empty()
            && self.import.is_none()
            && (self.list != ListChoice::Items || self.lookups.is_some())
    }

    /// Sends the ready rows.
    pub fn start_import(&mut self) {
        if !self.can_import() {
            return;
        }
        self.outcome = None;
        self.import_error = None;
        let left_out: Vec<(usize, Skip)> = self
            .built
            .issues
            .iter()
            .map(|i| (i.line, Skip::Row(i.problem)))
            .collect();
        let rows = self.built.rows.clone();
        let role = self.role;
        if self.link.is_demo() {
            let mut out = demo_import(&self.link, rows, role);
            out.skipped.splice(0..0, left_out);
            self.lookups = None;
            self.import = Some(Task::ready(out));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let label = OrphanLabel {
            what: "orphan.list_import",
            key: None,
        };
        let pending = group.write(&session, label, move |s| async move {
            let mut out = Outcome {
                skipped: left_out,
                ..Outcome::default()
            };
            let lines: Vec<usize> = rows.iter().map(|(l, _)| *l).collect();
            let mut accounts = Vec::new();
            let mut contacts = Vec::new();
            let mut items = Vec::new();
            for (line, m) in rows {
                match m {
                    Mapped::Account(a) => accounts.push(AccountImportRow {
                        account_number: a.number,
                        name: a.name,
                        account_type: api_type(a.kind),
                        detail_type: a.detail_type,
                    }),
                    Mapped::Contact(c) => contacts.push(ContactImportRow {
                        display_name: c.name,
                        company_name: c.company,
                        email: c.email,
                        phone: c.phone,
                    }),
                    Mapped::Item(i) => items.push((
                        line,
                        ItemFields {
                            name: i.name,
                            description: i.description,
                            unit_price: i.price,
                            income_account_id: i.income_account,
                            is_taxable: i.taxable,
                        },
                    )),
                }
            }
            if !accounts.is_empty() {
                let r = s
                    .accounts()
                    .import(&accounts, false, IdempotencyKey::new())
                    .await?;
                out.imported = r.imported + r.updated;
                for sk in r.skipped {
                    let idx = sk
                        .row
                        .and_then(|r| usize::try_from(r).ok())
                        .and_then(|r| r.checked_sub(1));
                    let line = idx.and_then(|i| lines.get(i).copied()).unwrap_or_default();
                    let why = if sk.reason.as_deref().is_some_and(|r| r.contains("system")) {
                        Skip::Other
                    } else {
                        Skip::Exists
                    };
                    out.skipped.push((line, why));
                }
            }
            if !contacts.is_empty() {
                let r = s
                    .contacts()
                    .import(&contacts, role, IdempotencyKey::new())
                    .await?;
                out.imported = r.imported;
            }
            for (line, fields) in items {
                match s.items().create(&fields, IdempotencyKey::new()).await {
                    Ok(_) => out.imported += 1,
                    Err(ApiError::Conflict {
                        reason: ConflictReason::AlreadyExists | ConflictReason::Unspecified,
                        ..
                    }) => out.skipped.push((line, Skip::Exists)),
                    Err(e) => {
                        out.stopped = Some(e);
                        break;
                    }
                }
            }
            out.skipped.sort_by_key(|(l, _)| *l);
            Ok(out)
        });
        if let Ok(p) = pending {
            self.import = Some(Task::Pending(p));
        }
    }

    fn poll(&mut self) {
        if let Some(r) = self.export.as_mut().and_then(Task::try_take) {
            self.export = None;
            match r {
                Ok(text) => self.finish_export(&text),
                Err(e) => self.export_api_error = Some(e),
            }
        }
        if let Some(r) = self.lookups_task.as_mut().and_then(Task::try_take) {
            self.lookups_task = None;
            match r {
                Ok(l) => {
                    self.lookups = Some(l);
                    if self.default_income.is_none() {
                        self.default_income = self.income_accounts().first().map(|a| a.id);
                    }
                    self.rebuild();
                }
                Err(e) => self.import_error = Some(e),
            }
        }
        if let Some(r) = self.import.as_mut().and_then(Task::try_take) {
            self.import = None;
            match r {
                Ok(o) => {
                    tracing::info!(
                        list = self.list.key(),
                        imported = o.imported,
                        left_out = o.skipped.len(),
                        "list imported"
                    );
                    // Names now taken: read them again before another import.
                    self.lookups = None;
                    self.outcome = Some(o);
                    self.parsed = None;
                    self.built = Built::default();
                    self.problems.clear();
                    if self.list == ListChoice::Items {
                        self.load_lookups();
                    }
                }
                Err(e) => self.import_error = Some(e),
            }
        }
    }

    // ---- drawing -------------------------------------------------------------

    fn list_choice_ui(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(t("csv.list_label"));
            let mut chosen = self.list;
            for l in ListChoice::ALL {
                ui.radio_value(&mut chosen, l, l.label());
            }
            self.set_list(chosen);
        });
    }

    fn export_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("csv.export_heading"));
            form::hint(ui, tokens, &t("csv.export_hint"));
            ui.add_space(4.0);
            let r = form::field_wide(ui, &t("csv.folder"), &mut self.folder);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            let label = t_with("csv.export_button", &[("list", &self.list.label())]);
            let clicked = ui
                .add_enabled(self.export.is_none(), egui::Button::new(label))
                .clicked();
            if clicked || enter {
                self.start_export();
            }
            if self.export.is_some() {
                form::progress_line(ui, tokens, &t("csv.exporting"));
            }
            if let Some(p) = &self.exported {
                ui.label(
                    RichText::new(t_with(
                        "csv.exported",
                        &[("path", &p.display().to_string())],
                    ))
                    .color(tokens.text_secondary),
                );
            }
            if let Some(e) = &self.export_error {
                ui.label(RichText::new(e).color(tokens.warn_text));
            }
            if let Some(e) = &self.export_api_error {
                form::inline_error(ui, tokens, e);
            }
        });
    }

    fn import_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("csv.import_heading"));
            if self.list == ListChoice::Tags {
                form::hint(ui, tokens, &t("csv.tags_export_only"));
                return;
            }
            form::hint(ui, tokens, &t("csv.import_hint"));
            ui.add_space(4.0);
            let r = form::field_wide(ui, &t("csv.file"), &mut self.file);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            if ui.button(t("csv.read_button")).clicked() || enter {
                self.read_file();
            }
            if let Some(e) = &self.read_error {
                ui.label(RichText::new(e).color(tokens.warn_text));
            }
            if let Some(e) = &self.import_error {
                form::inline_error(ui, tokens, e);
            }
            if self.parsed.is_some() {
                ui.add_space(8.0);
                self.mapping_ui(ui, tokens);
            }
            if let Some(o) = &self.outcome {
                ui.add_space(8.0);
                outcome_ui(ui, tokens, o);
            }
        });
    }

    fn mapping_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(list) = self.list.csv() else {
            return;
        };
        let Some(p) = self.parsed.as_ref() else {
            return;
        };
        let headers = p.headers.clone();
        let sample: Vec<String> = p.rows.first().map(|(_, r)| r.clone()).unwrap_or_default();
        let rows = p.rows.len();
        form::heading(ui, tokens, &t("csv.mapping_heading"));
        form::hint(
            ui,
            tokens,
            &t_with("csv.mapping_hint", &[("rows", &rows.to_string())]),
        );
        let mut change = None;
        egui::Grid::new("csv-mapping")
            .num_columns(3)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label(RichText::new(t("csv.col.column")).strong());
                ui.label(RichText::new(t("csv.col.field")).strong());
                ui.label(RichText::new(t("csv.col.sample")).strong());
                ui.end_row();
                for (i, h) in headers.iter().enumerate() {
                    let header = if h.trim().is_empty() {
                        t_with("csv.column_n", &[("n", &(i + 1).to_string())])
                    } else {
                        h.clone()
                    };
                    let l = ui.label(&header);
                    let current = self.mapping.get(i).copied().flatten();
                    let shown =
                        current.map_or_else(|| t("csv.field.skip"), |x| t(x.label().as_str()));
                    let mut pick = current;
                    let r = egui::ComboBox::from_id_salt(("csv-map", i))
                        .selected_text(shown)
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut pick, None, t("csv.field.skip"));
                            for target in list.targets() {
                                ui.selectable_value(
                                    &mut pick,
                                    Some(*target),
                                    t(target.label().as_str()),
                                );
                            }
                        });
                    r.response.labelled_by(l.id);
                    if pick != current {
                        change = Some((i, pick));
                    }
                    let value = sample.get(i).map(String::as_str).unwrap_or_default();
                    ui.label(RichText::new(value).color(tokens.text_secondary));
                    ui.end_row();
                }
            });
        if let Some((i, pick)) = change {
            self.set_mapping(i, pick);
        }
        ui.add_space(6.0);
        match list {
            CsvList::Contacts => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("csv.role_label"));
                    let mut role = self.role;
                    ui.radio_value(&mut role, ContactType::Customer, t("csv.role.client"));
                    ui.radio_value(&mut role, ContactType::Vendor, t("csv.role.vendor"));
                    ui.radio_value(&mut role, ContactType::Both, t("csv.role.both"));
                    self.role = role;
                });
                form::hint(ui, tokens, &t("csv.contacts_fields_note"));
            }
            CsvList::Items => self.income_ui(ui, tokens),
            CsvList::Accounts => form::hint(ui, tokens, &t("csv.accounts_note")),
        }
        ui.add_space(6.0);
        for pr in &self.problems {
            ui.label(RichText::new(mapping_problem_text(*pr)).color(tokens.warn_text));
        }
        let ready = self.built.rows.len();
        let left = self.built.issues.len();
        ui.label(t_with(
            "csv.ready",
            &[("ready", &ready.to_string()), ("left", &left.to_string())],
        ));
        for i in self.built.issues.iter().take(ISSUES_SHOWN) {
            ui.label(
                RichText::new(t_with(
                    "csv.line_problem",
                    &[
                        ("line", &i.line.to_string()),
                        ("problem", &row_problem_text(i.problem)),
                    ],
                ))
                .color(tokens.text_secondary),
            );
        }
        if left > ISSUES_SHOWN {
            ui.label(t_with(
                "csv.more_problems",
                &[("n", &(left - ISSUES_SHOWN).to_string())],
            ));
        }
        ui.add_space(6.0);
        let label = t_with("csv.import_button", &[("n", &ready.to_string())]);
        if ui
            .add_enabled(self.can_import(), egui::Button::new(label))
            .clicked()
        {
            self.start_import();
        }
        if self.import.is_some() {
            form::progress_line(ui, tokens, &t("csv.importing"));
        }
    }

    fn income_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.lookups.is_none() {
            form::progress_line(ui, tokens, &t("csv.loading_accounts"));
            return;
        }
        let choices: Vec<(AccountId, String)> = self
            .income_accounts()
            .iter()
            .map(|a| (a.id, a.fields.name.clone()))
            .collect();
        let shown = self
            .default_income
            .and_then(|id| choices.iter().find(|(c, _)| *c == id))
            .map_or_else(String::new, |(_, n)| n.clone());
        let mut pick = self.default_income;
        ui.horizontal(|ui| {
            let l = ui.label(t("csv.default_income"));
            let r = egui::ComboBox::from_id_salt("csv-income")
                .selected_text(shown)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (id, name) in &choices {
                        ui.selectable_value(&mut pick, Some(*id), name);
                    }
                });
            r.response.labelled_by(l.id);
        });
        if pick != self.default_income {
            self.default_income = pick;
            self.rebuild();
        }
    }
}

fn mapping_problem_text(p: MappingProblem) -> String {
    match p {
        MappingProblem::Missing(f) | MappingProblem::Twice(f) => {
            t_with(p.key().as_str(), &[("field", &t(f.label().as_str()))])
        }
        MappingProblem::TooManyRows(max) => t_with(p.key().as_str(), &[("max", &max.to_string())]),
        MappingProblem::NoRows => t(p.key().as_str()),
    }
}

fn row_problem_text(p: RowProblem) -> String {
    match p {
        RowProblem::TooLong(f) => t_with(p.key().as_str(), &[("field", &t(f.label().as_str()))]),
        other => t(other.key().as_str()),
    }
}

fn skip_text(s: &Skip) -> String {
    match s {
        Skip::Row(p) => row_problem_text(*p),
        Skip::Exists => t("csv.skipped.exists"),
        Skip::Other => t("csv.skipped.system"),
    }
}

fn outcome_ui(ui: &mut Ui, tokens: &Tokens, o: &Outcome) {
    ui.label(
        RichText::new(t_with(
            "csv.imported",
            &[
                ("n", &o.imported.to_string()),
                ("left", &o.skipped.len().to_string()),
            ],
        ))
        .strong(),
    );
    for (line, why) in o.skipped.iter().take(ISSUES_SHOWN) {
        ui.label(
            RichText::new(t_with(
                "csv.line_problem",
                &[("line", &line.to_string()), ("problem", &skip_text(why))],
            ))
            .color(tokens.text_secondary),
        );
    }
    if let Some(e) = &o.stopped {
        form::inline_error(ui, tokens, e);
    }
}

/// Reads and parses a CSV file: the first row is the header.
fn read_csv(path: &Path) -> Result<Parsed, String> {
    let meta = std::fs::metadata(path).map_err(|_| t("csv.read.missing"))?;
    if !meta.is_file() {
        return Err(t("csv.read.missing"));
    }
    if meta.len() > FILE_MAX_BYTES {
        return Err(t("csv.read.too_big"));
    }
    let bytes = std::fs::read(path).map_err(|_| t("csv.read.missing"))?;
    let rows = csv::parse(&csv::decode(&bytes)).map_err(|e| match e {
        csv::CsvError::Empty => t("csv.read.empty"),
        csv::CsvError::UnclosedQuote { line } => {
            t_with("csv.read.quote", &[("line", &line.to_string())])
        }
        csv::CsvError::TooManyRows
        | csv::CsvError::TooManyColumns { .. }
        | csv::CsvError::FieldTooLong { .. } => t("csv.read.too_big"),
    })?;
    let mut it = rows.into_iter();
    let headers = it.next().map(|r| r.fields).unwrap_or_default();
    Ok(Parsed {
        headers,
        rows: it.map(|r| (r.line, r.fields)).collect(),
    })
}

/// Writes `text` as `<stem>.csv` in `dir` (a number added when the name is
/// taken), with a byte-order mark so spreadsheet programs read UTF-8.
fn write_export(dir: &Path, stem: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut path = dir.join(format!("{stem}.csv"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).csv"));
        n += 1;
    }
    let mut body = String::with_capacity(text.len() + 3);
    body.push('\u{FEFF}');
    body.push_str(text);
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Tags as CSV: name, group, description, color, active.
fn tags_csv(tags: &[Tag], groups: &[TagGroup]) -> String {
    let group_name = |t: &Tag| {
        t.fields
            .group_id
            .and_then(|g| groups.iter().find(|x| x.id == g))
            .map(|g| g.fields.name.clone())
            .unwrap_or_default()
    };
    let mut rows = vec![vec![
        "Name".to_owned(),
        "Group".to_owned(),
        "Description".to_owned(),
        "Color".to_owned(),
        "Active".to_owned(),
    ]];
    let mut sorted: Vec<&Tag> = tags.iter().collect();
    sorted.sort_by_key(|t| (group_name(t).to_lowercase(), t.fields.name.to_lowercase()));
    for tag in sorted {
        rows.push(vec![
            csv::cell_text(&tag.fields.name),
            csv::cell_text(&group_name(tag)),
            csv::cell_text(tag.fields.description.as_deref().unwrap_or_default()),
            tag.fields.color.clone().unwrap_or_default(),
            tag.is_active.to_string(),
        ]);
    }
    csv::write(&rows)
}

/// The demo company's lists as CSV, in the engine's export layouts.
fn demo_export(link: &CompanyLink, list: ListChoice) -> String {
    let Some(mut d) = link.demo_state() else {
        return String::new();
    };
    match list {
        ListChoice::Accounts => {
            let chart = d.chart.get_or_insert_with(crate::company::demo::chart);
            let mut rows = vec![
                [
                    "Account Number",
                    "Name",
                    "Type",
                    "Detail Type",
                    "Balance",
                    "Active",
                    "System",
                ]
                .map(str::to_owned)
                .to_vec(),
            ];
            for a in chart.iter() {
                rows.push(vec![
                    a.fields.account_number.clone().unwrap_or_default(),
                    csv::cell_text(&a.fields.name),
                    a.fields.account_type.as_str().to_owned(),
                    a.fields.detail_type.clone().unwrap_or_default(),
                    a.balance.map(cl_core::Money::to_wire).unwrap_or_default(),
                    a.is_active.to_string(),
                    a.is_system.to_string(),
                ]);
            }
            csv::write(&rows)
        }
        ListChoice::Contacts => {
            let mut rows = vec![
                [
                    "Display Name",
                    "Type",
                    "Company",
                    "First Name",
                    "Last Name",
                    "Email",
                    "Phone",
                    "Active",
                ]
                .map(str::to_owned)
                .to_vec(),
            ];
            for c in d.contacts_mut().iter() {
                let f = &c.fields;
                rows.push(vec![
                    csv::cell_text(&f.display_name),
                    f.contact_type.as_str().to_owned(),
                    csv::cell_text(f.company_name.as_deref().unwrap_or_default()),
                    csv::cell_text(f.first_name.as_deref().unwrap_or_default()),
                    csv::cell_text(f.last_name.as_deref().unwrap_or_default()),
                    f.email.clone().unwrap_or_default(),
                    f.phone.clone().unwrap_or_default(),
                    c.is_active.to_string(),
                ]);
            }
            csv::write(&rows)
        }
        ListChoice::Items => {
            let mut rows = vec![
                [
                    "Name",
                    "Description",
                    "Unit Price",
                    "Income Account ID",
                    "Taxable",
                    "Active",
                ]
                .map(str::to_owned)
                .to_vec(),
            ];
            for i in d.items_mut().iter() {
                rows.push(vec![
                    csv::cell_text(&i.fields.name),
                    csv::cell_text(i.fields.description.as_deref().unwrap_or_default()),
                    i.fields
                        .unit_price
                        .map(cl_core::Money::to_wire)
                        .unwrap_or_default(),
                    i.fields.income_account_id.to_string(),
                    i.fields.is_taxable.to_string(),
                    i.is_active.to_string(),
                ]);
            }
            csv::write(&rows)
        }
        ListChoice::Tags => {
            let tags = d.tags_mut().clone();
            let groups = d.tag_groups_mut().clone();
            tags_csv(&tags, &groups)
        }
    }
}

/// Imports into the demo company at once (no engine).
fn demo_import(link: &CompanyLink, rows: Vec<(usize, Mapped)>, role: ContactType) -> Outcome {
    let mut out = Outcome::default();
    for (line, m) in rows {
        match m {
            Mapped::Account(a) => {
                let taken = a.number.as_ref().is_some_and(|n| {
                    link.demo_state().is_some_and(|mut d| {
                        d.chart
                            .get_or_insert_with(crate::company::demo::chart)
                            .iter()
                            .any(|x| x.fields.account_number.as_ref() == Some(n))
                    })
                });
                if taken {
                    out.skipped.push((line, Skip::Exists));
                    continue;
                }
                crate::screens::lists::accounts::demo_save(
                    link,
                    None,
                    None,
                    AccountFields {
                        name: a.name,
                        account_number: a.number,
                        account_type: api_type(a.kind),
                        detail_type: a.detail_type,
                        description: None,
                        parent_id: None,
                    },
                );
                out.imported += 1;
            }
            Mapped::Contact(c) => {
                let Ok(fields) = serde_json::from_value::<ContactFields>(json!({
                    "contactType": role.as_str(),
                    "displayName": c.name,
                    "companyName": c.company,
                    "email": c.email,
                    "phone": c.phone,
                })) else {
                    continue;
                };
                if let Some(mut d) = link.demo_state() {
                    let n = d.contacts_mut().len();
                    let contact = cl_api::Contact {
                        id: crate::company::demo::contacts::contact_id(
                            950_000 + u32::try_from(n).unwrap_or(0),
                        ),
                        fields,
                        is_active: true,
                        opening_balance: Some(cl_core::Money::ZERO),
                        opening_balance_date: None,
                        created_at: Some(crate::company::demo::DemoState::now()),
                        updated_at: Some(crate::company::demo::DemoState::now()),
                    };
                    d.contacts_mut().push(contact);
                    out.imported += 1;
                }
            }
            Mapped::Item(i) => {
                crate::screens::lists::items::demo_save(
                    link,
                    None,
                    ItemFields {
                        name: i.name,
                        description: i.description,
                        unit_price: i.price,
                        income_account_id: i.income_account,
                        is_taxable: i.taxable,
                    },
                );
                out.imported += 1;
            }
        }
    }
    out
}

impl Screen for CsvScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        egui::ScrollArea::vertical()
            .id_salt("csv-scroll")
            .show(ui, |ui| {
                form::heading(ui, &tokens, &self.title());
                ui.add_space(4.0);
                self.list_choice_ui(ui);
                ui.add_space(8.0);
                self.export_ui(ui, &tokens);
                ui.add_space(8.0);
                self.import_ui(ui, &tokens);
            });
    }

    fn state(&self) -> Value {
        json!({
            "screen": "import_export",
            "list": self.list.key(),
            "exporting": self.export.is_some(),
            "exported": self.exported.is_some(),
            "export_error": self.export_error.is_some() || self.export_api_error.is_some(),
            "file_read": self.parsed.is_some(),
            "read_error": self.read_error.is_some(),
            "columns": self.parsed.as_ref().map_or(0, |p| p.headers.len()),
            "mapped": self.mapping.iter().flatten().count(),
            "ready": self.built.rows.len(),
            "left_out": self.built.issues.len(),
            "problems": self.problems.len(),
            "can_import": self.can_import(),
            "importing": self.import.is_some(),
            "imported": self.outcome.as_ref().map(|o| o.imported),
            "skipped": self.outcome.as_ref().map(|o| o.skipped.len()),
            "import_error": self.import_error.is_some(),
        })
    }
}

#[cfg(test)]
mod tests;
