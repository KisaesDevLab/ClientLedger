//! What every form's save-conflict flow shares (P33 D4 step 11, Q-P33r):
//! the read of their version and the open dialog ([`ConflictFlow`]), what a
//! read means ([`read_of`]), the line projections (expense and item lines
//! in one `lines` list, so they travel together in Keep mine), and the
//! words the dialog shows for ids and amounts ([`Words`]).
//!
//! Make Journal Entry and Transfer Funds are the two templates (with lines
//! and header-only); the other forms follow them with these pieces.
//! Amounts are compared as the engine's exact strings and never added up.

use std::sync::Arc;

use cl_api::ApiError;
use cl_core::MoneyStyle;
use cl_model::forms::SaveIntent;
use cl_model::forms::item_lines::{ItemLine, ItemLines};
use cl_model::forms::lines::{FormLine, FormLines};
use eframe::egui;
use serde_json::{Value, json};

use crate::company::Task;
use crate::forms::item_lines_grid::ItemLists;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog, FieldText, LineText};
use crate::screens::register::lists::RegisterLists;

/// A conflict waiting for the owner: the dialog, their record as read, and
/// what the save was for.
pub(crate) struct OpenConflict<R> {
    pub dialog: ConflictDialog,
    pub theirs: R,
    pub intent: SaveIntent,
}

/// A form's conflict state: their version being read, then the dialog.
pub(crate) struct ConflictFlow<R> {
    pub reading: Option<(Task<R>, SaveIntent)>,
    pub open: Option<OpenConflict<R>>,
}

impl<R> Default for ConflictFlow<R> {
    fn default() -> Self {
        ConflictFlow {
            reading: None,
            open: None,
        }
    }
}

impl<R> std::fmt::Debug for ConflictFlow<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConflictFlow")
            .field("reading", &self.reading.is_some())
            .field("open", &self.open.is_some())
            .finish()
    }
}

impl<R> ConflictFlow<R> {
    /// Their version is being read.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.reading.is_some()
    }

    /// The dialog is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// For the control channel: the dialog's counts, or null.
    #[must_use]
    pub fn state(&self) -> Value {
        self.open.as_ref().map_or(Value::Null, |c| c.dialog.state())
    }

    /// Their version, once read.
    pub fn poll(&mut self) -> Option<(Result<R, ApiError>, SaveIntent)> {
        let (mut task, intent) = self.reading.take()?;
        if let Some(r) = task.try_take() {
            Some((r, intent))
        } else {
            self.reading = Some((task, intent));
            None
        }
    }

    /// Draws the open dialog; the owner's choice, with the conflict taken.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<(OpenConflict<R>, ConflictChoice)> {
        let choice = self.open.as_mut()?.dialog.show(ctx)?;
        self.open.take().map(|open| (open, choice))
    }

    /// Esc while the dialog is open: Back.
    pub fn take_back(&mut self) -> Option<(OpenConflict<R>, ConflictChoice)> {
        self.open.take().map(|open| (open, ConflictChoice::Back))
    }

    /// Another record or a new form: nothing pending.
    pub fn clear(&mut self) {
        self.reading = None;
        self.open = None;
    }
}

/// What reading their version found.
pub(crate) enum Read<R> {
    /// Their record.
    Theirs(R),
    /// Deleted elsewhere: never saved again ("No longer exists").
    Gone,
    /// Anything else: the form says it changed elsewhere (Reload).
    Reload,
}

/// What a read of their version means for the form.
#[must_use]
pub(crate) fn read_of<R>(r: Result<R, ApiError>) -> Read<R> {
    match r {
        Ok(v) => Read::Theirs(v),
        Err(ApiError::NotFound { .. }) => Read::Gone,
        Err(_) => Read::Reload,
    }
}

/// The notice for a record deleted elsewhere (i18n key).
pub(crate) const GONE: &str = "lan.conflict.gone";

/// An expense line (account, amount, memo, name, billable, tag).
#[must_use]
pub(crate) fn expense_line(l: &FormLine) -> Value {
    json!({
        "kind": "expense",
        "account": l.account.map(|a| a.to_string()),
        "amount": l.amount.map(cl_core::Money::to_wire),
        "memo": l.memo.trim(),
        "name": l.customer.map(|c| c.to_string()),
        "billable": l.billable,
        "tag": l.tag.map(|t| t.to_string()),
    })
}

/// An item line (item, description, quantity, unit, rate, amount, account,
/// name, billable, tag).
#[must_use]
pub(crate) fn item_line(l: &ItemLine) -> Value {
    json!({
        "kind": "item",
        "item": l.item.map(|i| i.to_string()),
        "description": l.description.trim(),
        "qty": l.qty.map(cl_core::Quantity::to_wire),
        "unit": l.unit.map(|u| u.to_string()),
        "rate": l.rate.map(cl_core::Money::to_wire),
        "amount": l.amount.map(cl_core::Money::to_wire),
        "account": l.account.map(|a| a.to_string()),
        "name": l.customer.map(|c| c.to_string()),
        "billable": l.billable,
        "tag": l.tag.map(|t| t.to_string()),
    })
}

/// The used expense lines, then the used item lines, as one list.
#[must_use]
pub(crate) fn lines_value(lines: &FormLines, items: Option<&ItemLines>) -> Vec<Value> {
    lines
        .used()
        .map(expense_line)
        .chain(items.into_iter().flat_map(|i| i.used().map(item_line)))
        .collect()
}

/// The ids in a form's values as the owner reads them: accounts, names,
/// tags and items from the form's own lists, money in the owner's style.
#[derive(Clone)]
pub(crate) struct Words {
    lists: Arc<RegisterLists>,
    items: Option<Arc<ItemLists>>,
    style: MoneyStyle,
    /// Header fields that hold an amount.
    money: &'static [&'static str],
    /// Other ids the form names itself (sales tax items, layouts).
    names: Arc<std::collections::HashMap<String, String>>,
}

/// A wire code in words, for the fields that hold one (terms, payment
/// method, charge or credit); `None` for anything else.
#[must_use]
pub(crate) fn code_text(field: &str, code: &str) -> Option<String> {
    let key = |k: String| crate::i18n::has(&k).then(|| crate::i18n::t(&k));
    match field {
        "terms" => cl_model::lists::contacts::Terms::from_wire(code)
            .map(|x| crate::i18n::t(x.label().as_str())),
        "paymentMethod" => key(format!("payment.method.{code}"))
            .or_else(|| key(format!("sales_receipt.method.{code}"))),
        "kind" => key(format!("card.mode.{code}")),
        _ => None,
    }
}

/// A quantity as the owner reads it: no trailing zeros after the point.
fn qty_text(wire: &str) -> String {
    if wire.contains('.') {
        wire.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        wire.to_string()
    }
}

impl Words {
    /// Words from `lists` (and `items`), amounts in `style`.
    #[must_use]
    pub fn new(
        lists: Arc<RegisterLists>,
        items: Option<Arc<ItemLists>>,
        style: MoneyStyle,
        money: &'static [&'static str],
    ) -> Words {
        Words {
            lists,
            items,
            style,
            money,
            names: Arc::default(),
        }
    }

    /// These words, also naming `names` (id text to name).
    #[must_use]
    pub fn with_names(mut self, names: std::collections::HashMap<String, String>) -> Words {
        self.names = Arc::new(names);
        self
    }

    /// An id's name (account, name, tag or item), if it is one.
    #[must_use]
    pub fn name(&self, id: &str) -> Option<String> {
        if let Some(n) = self.names.get(id) {
            return Some(n.clone());
        }
        if let Ok(a) = cl_core::ids::AccountId::parse(id)
            && let Some(t) = self.lists.account_text(a)
        {
            return Some(t.to_string());
        }
        if let Ok(c) = cl_core::ids::ContactId::parse(id)
            && let Some(t) = self.lists.name_text(c)
        {
            return Some(t.to_string());
        }
        if let Ok(t) = cl_core::ids::TagId::parse(id)
            && let Some(e) = self.lists.tag_entries().iter().find(|e| e.id == t)
        {
            return Some(e.name.clone());
        }
        let (Ok(i), Some(items)) = (cl_core::ids::ItemId::parse(id), self.items.as_ref()) else {
            return None;
        };
        items
            .names()
            .into_iter()
            .find(|(k, _)| *k == i)
            .map(|(_, n)| n)
    }

    /// An amount as the owner reads it.
    #[must_use]
    pub fn amount(&self, wire: &str) -> Option<String> {
        cl_core::Money::parse_wire(wire)
            .ok()
            .map(|m| cl_core::report::display_amount(m.as_decimal(), self.style))
    }

    /// One header field's value in words (`None`: the dialog's own).
    #[must_use]
    pub fn field(&self, field: &str, v: &Value) -> Option<String> {
        match v {
            Value::String(s) if self.money.contains(&field) => self.amount(s),
            Value::String(s) => code_text(field, s).or_else(|| self.name(s)),
            Value::Array(ids) => {
                let names: Vec<String> = ids
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|id| self.name(id).unwrap_or_else(|| id.to_string()))
                    .collect();
                Some(names.join(", "))
            }
            _ => None,
        }
    }

    /// One line in words: account or item, quantity and rate, amount,
    /// memo or description.
    #[must_use]
    pub fn line(&self, v: &Value) -> String {
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
        let what = [text("item"), text("account"), text("name")]
            .into_iter()
            .find(|s| !s.is_empty())
            .and_then(|id| self.name(id))
            .unwrap_or_default();
        let (qty, rate) = (text("qty"), text("rate"));
        let priced = match (qty.is_empty(), rate.is_empty()) {
            (false, false) => format!(
                "{} \u{00d7} {}",
                qty_text(qty),
                self.amount(rate).unwrap_or_else(|| rate.to_string())
            ),
            (false, true) => qty_text(qty),
            (true, false) => self.amount(rate).unwrap_or_else(|| rate.to_string()),
            (true, true) => String::new(),
        };
        let amount = self.amount(text("amount")).unwrap_or_default();
        let memo = [text("memo"), text("description")]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        [what.as_str(), priced.as_str(), amount.as_str(), memo]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{00b7} ")
    }

    /// The dialog's line words.
    pub fn line_text(&self) -> LineText {
        let w = self.clone();
        Box::new(move |v| w.line(v))
    }

    /// The dialog's field words.
    pub fn field_text(&self) -> FieldText {
        let w = self.clone();
        Box::new(move |f, v| w.field(f, v))
    }

    /// The dialog over `conflict` in these words.
    #[must_use]
    pub fn dialog(&self, conflict: cl_model::lan::Conflict) -> ConflictDialog {
        ConflictDialog::new(conflict, self.line_text()).with_field_text(self.field_text())
    }
}

/// Header tags as sorted id texts (their order never counts as a change).
#[must_use]
pub(crate) fn tag_ids(tags: &[cl_core::ids::TagId]) -> Vec<String> {
    let mut v: Vec<String> = tags.iter().map(ToString::to_string).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::rowkey::RowKeyGen;

    #[test]
    fn lines_are_content_only_and_keep_exact_amounts() {
        let mut keys = RowKeyGen::new();
        let line = FormLine {
            amount: cl_core::Money::parse_wire("10.50").ok(),
            memo: " feed ".into(),
            ..FormLine::blank(keys.next_key())
        };
        let other = FormLine {
            key: keys.next_key(),
            ..line.clone()
        };
        assert_eq!(
            expense_line(&line),
            expense_line(&other),
            "row keys never count"
        );
        let wire = cl_core::Money::parse_wire("10.50").expect("m").to_wire();
        assert_eq!(expense_line(&line)["amount"], wire.as_str());
        assert_eq!(expense_line(&line)["memo"], "feed");
    }

    #[test]
    fn codes_read_as_words_and_lines_show_quantity_and_rate() {
        assert_eq!(code_text("terms", "net_30").as_deref(), Some("Net 30"));
        assert_eq!(code_text("paymentMethod", "cash").as_deref(), Some("Cash"));
        assert_eq!(code_text("kind", "credit").as_deref(), Some("Credit"));
        assert_eq!(code_text("memo", "cash"), None);
        assert_eq!(code_text("terms", "made_up"), None);
        let w = Words::new(Arc::default(), None, MoneyStyle::default(), &[])
            .with_names([("L1".to_string(), "Simple".to_string())].into());
        assert_eq!(w.field("layout", &json!("L1")).as_deref(), Some("Simple"));
        let line = w.line(&json!({"qty": "2.5000", "rate": "40.00", "description": "Mulch"}));
        assert!(line.starts_with("2.5 \u{00d7} "), "{line}");
        assert!(line.contains("40.00"), "{line}");
        assert!(line.ends_with("Mulch"), "{line}");
    }

    #[test]
    fn a_missing_record_is_gone_and_anything_else_reloads() {
        assert!(matches!(
            read_of::<()>(Err(ApiError::NotFound { code: None })),
            Read::Gone
        ));
        assert!(matches!(
            read_of::<()>(Err(ApiError::NoCompanyOpen)),
            Read::Reload
        ));
        assert!(matches!(read_of(Ok(1)), Read::Theirs(1)));
    }
}
