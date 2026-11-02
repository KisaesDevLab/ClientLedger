//! The sales tax setup editors (P23 plan §3 WP-2 screen; Q-P23d, Q-P23v):
//! a sales tax item (name, description, percent, agency), a group (1-10
//! items) and a tax code (up to 8 characters, taxable or not).

use cl_core::TaxRate;
use cl_core::ids::{SalesTaxAgencyId, SalesTaxItemId};

use crate::text::TextKey;

/// Longest item or group name (mirrors `cl_api::SALES_TAX_NAME_MAX`).
pub const NAME_MAX: usize = 100;
/// Items in a group.
pub const GROUP_MAX: usize = 10;
/// Longest tax code.
pub const CODE_MAX: usize = 8;

/// Why an editor can't save yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetupProblem {
    /// No name.
    NameRequired,
    /// Name over 100 characters.
    NameTooLong,
    /// The percent is not a rate (blank, letters, more than 4 decimals, 100
    /// or more).
    BadRate,
    /// An item needs the agency it is paid to.
    AgencyRequired,
    /// A group needs at least one item.
    GroupEmpty,
    /// A group holds at most 10 items.
    GroupTooLarge,
    /// No code.
    CodeRequired,
    /// Code over 8 characters.
    CodeTooLong,
}

impl SetupProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            SetupProblem::NameRequired => TextKey("sales_tax.problem.name_required"),
            SetupProblem::NameTooLong => TextKey("sales_tax.problem.name_too_long"),
            SetupProblem::BadRate => TextKey("sales_tax.problem.bad_rate"),
            SetupProblem::AgencyRequired => TextKey("sales_tax.problem.agency_required"),
            SetupProblem::GroupEmpty => TextKey("sales_tax.problem.group_empty"),
            SetupProblem::GroupTooLarge => TextKey("sales_tax.problem.group_too_large"),
            SetupProblem::CodeRequired => TextKey("sales_tax.problem.code_required"),
            SetupProblem::CodeTooLong => TextKey("sales_tax.problem.code_too_long"),
        }
    }
}

/// What a checked item or group editor sends (the UI maps it onto
/// `cl_api::SalesTaxItemDraft`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemPlan {
    /// A sales tax item.
    Item {
        /// Name (trimmed).
        name: String,
        /// Description (trimmed; `None` when blank).
        description: Option<String>,
        /// The rate.
        rate: TaxRate,
        /// The agency.
        agency: SalesTaxAgencyId,
    },
    /// A group.
    Group {
        /// Name (trimmed).
        name: String,
        /// Description.
        description: Option<String>,
        /// Its items, in the order shown.
        members: Vec<SalesTaxItemId>,
    },
}

fn check_name(name: &str) -> Result<String, SetupProblem> {
    let n = name.trim();
    if n.is_empty() {
        return Err(SetupProblem::NameRequired);
    }
    if n.chars().count() > NAME_MAX {
        return Err(SetupProblem::NameTooLong);
    }
    Ok(n.to_string())
}

fn blank_none(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The item editor: what the owner typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemEditor {
    /// Name.
    pub name: String,
    /// Description (printed as the tax label).
    pub description: String,
    /// The percent as typed (`8.875`).
    pub percent: String,
    /// The agency.
    pub agency: Option<SalesTaxAgencyId>,
    /// The item is used on documents: a change applies to new documents
    /// only (the editor shows the note, Q-P23v).
    pub in_use: bool,
}

impl ItemEditor {
    /// An editor filled from a saved item.
    #[must_use]
    pub fn from_saved(
        name: &str,
        description: Option<&str>,
        rate: TaxRate,
        agency: Option<SalesTaxAgencyId>,
        in_use: bool,
    ) -> ItemEditor {
        ItemEditor {
            name: name.to_string(),
            description: description.unwrap_or_default().to_string(),
            percent: rate.percent_input(),
            agency,
            in_use,
        }
    }

    /// The rate typed, if it is one.
    #[must_use]
    pub fn rate(&self) -> Option<TaxRate> {
        TaxRate::parse_percent(&self.percent).ok()
    }

    /// The rate as owners read it (`8.875%`), for the list and the note.
    #[must_use]
    pub fn rate_label(&self) -> Option<String> {
        self.rate().map(TaxRate::format_percent)
    }

    /// True when the note "applies to new documents only" shows: the item
    /// is in use and its rate or agency differs from `saved`.
    #[must_use]
    pub fn shows_forward_note(&self, saved: &ItemEditor) -> bool {
        self.in_use && (self.rate() != saved.rate() || self.agency != saved.agency)
    }

    /// The plan to send.
    ///
    /// # Errors
    /// The first [`SetupProblem`].
    pub fn plan(&self) -> Result<ItemPlan, SetupProblem> {
        let name = check_name(&self.name)?;
        let rate = self.rate().ok_or(SetupProblem::BadRate)?;
        let agency = self.agency.ok_or(SetupProblem::AgencyRequired)?;
        Ok(ItemPlan::Item {
            name,
            description: blank_none(&self.description),
            rate,
            agency,
        })
    }
}

/// The group editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupEditor {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    members: Vec<SalesTaxItemId>,
}

impl GroupEditor {
    /// An editor with these members (duplicates dropped).
    #[must_use]
    pub fn new(name: &str, members: &[SalesTaxItemId]) -> GroupEditor {
        let mut g = GroupEditor {
            name: name.to_string(),
            ..GroupEditor::default()
        };
        for m in members {
            g.add(*m);
        }
        g
    }

    /// The items, in order.
    #[must_use]
    pub fn members(&self) -> &[SalesTaxItemId] {
        &self.members
    }

    /// Adds an item (false when it is already in or the group is full).
    pub fn add(&mut self, item: SalesTaxItemId) -> bool {
        if self.members.contains(&item) || self.members.len() >= GROUP_MAX {
            return false;
        }
        self.members.push(item);
        true
    }

    /// Takes an item out.
    pub fn remove(&mut self, item: SalesTaxItemId) {
        self.members.retain(|m| *m != item);
    }

    /// The plan to send.
    ///
    /// # Errors
    /// The first [`SetupProblem`].
    pub fn plan(&self) -> Result<ItemPlan, SetupProblem> {
        let name = check_name(&self.name)?;
        if self.members.is_empty() {
            return Err(SetupProblem::GroupEmpty);
        }
        if self.members.len() > GROUP_MAX {
            return Err(SetupProblem::GroupTooLarge);
        }
        Ok(ItemPlan::Group {
            name,
            description: blank_none(&self.description),
            members: self.members.clone(),
        })
    }
}

/// The tax code editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeEditor {
    /// The code (`Res`).
    pub code: String,
    /// Its name (`Resale`).
    pub name: String,
    /// Lines with it are taxed.
    pub taxable: bool,
    /// `Tax` or `Non`: read only.
    pub is_system: bool,
}

/// What a checked code editor sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodePlan {
    /// The code (trimmed).
    pub code: String,
    /// Its name (trimmed; the code when blank).
    pub name: String,
    /// Taxed.
    pub taxable: bool,
}

impl CodeEditor {
    /// The plan to send (`None` for a system code: nothing to save).
    ///
    /// # Errors
    /// The first [`SetupProblem`].
    pub fn plan(&self) -> Result<Option<CodePlan>, SetupProblem> {
        if self.is_system {
            return Ok(None);
        }
        let code = self.code.trim();
        if code.is_empty() {
            return Err(SetupProblem::CodeRequired);
        }
        if code.chars().count() > CODE_MAX {
            return Err(SetupProblem::CodeTooLong);
        }
        let name = blank_none(&self.name).unwrap_or_else(|| code.to_string());
        if name.chars().count() > NAME_MAX {
            return Err(SetupProblem::NameTooLong);
        }
        Ok(Some(CodePlan {
            code: code.to_string(),
            name,
            taxable: self.taxable,
        }))
    }
}
