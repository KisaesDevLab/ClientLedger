//! The batch model's plain values (`cl_model::forms::batch`) mapped 1:1
//! onto the engine types (`cl_api` batch), and the engine's answers back.
//! cl-model may not depend on cl-api (layering), so the screen does it.
//!
//! Amounts go through [`BatchAmount`], which takes only positive whole
//! cents: anything else was already refused by the local checks, and is
//! refused again here (never rounded, non-negotiable 6).

use cl_api::{
    BatchAmount, BatchCaps, BatchField, BatchGroup, BatchKind, BatchRowIn, BatchRowProblem,
    BatchValidation, ColumnMapping, CsvField, FieldProblem, LocalValidation, ParsedRow,
    ProblemKind,
};
use cl_core::Secret;
use cl_model::forms::batch::{
    EngineProblem, EngineProblemKind, EngineVerdict, Field, Mapping, Outgoing, ParsedCells,
    SaveCaps, SectionKind,
};

/// The engine kind of a section type.
#[must_use]
pub(crate) fn batch_kind(k: SectionKind) -> BatchKind {
    match k {
        SectionKind::Checks => BatchKind::Check,
        SectionKind::Expenses => BatchKind::Expense,
        SectionKind::CardCharges => BatchKind::CardCharge,
        SectionKind::CardCredits => BatchKind::CardCredit,
        SectionKind::Deposits => BatchKind::Deposit,
        SectionKind::JournalEntries => BatchKind::JournalEntry,
    }
}

/// The engine's capabilities as the model's gate reads them.
#[must_use]
pub(crate) fn save_caps(c: BatchCaps) -> SaveCaps {
    SaveCaps {
        atomic: c.atomic,
        check: c.check,
        mixed: c.mixed,
        exact: c.exact,
        idempotent: c.idempotent,
        closing: c.closing,
    }
}

fn amount(m: Option<cl_core::Money>) -> Result<Option<BatchAmount>, LocalValidation> {
    m.map(BatchAmount::new).transpose()
}

/// The body's groups. `password` goes on every row dated on or before the
/// closing date (`batch-closing-v1`), in the body only.
///
/// # Errors
/// An amount that is not a positive whole number of cents (the local
/// checks refuse those first).
pub(crate) fn groups(
    out: &Outgoing,
    password: Option<&Secret>,
) -> Result<Vec<BatchGroup>, LocalValidation> {
    out.groups
        .iter()
        .map(|g| {
            let rows = g
                .rows
                .iter()
                .map(|r| {
                    Ok(BatchRowIn {
                        row_number: r.row_number,
                        date: r.date,
                        number: r.number.clone(),
                        print_later: r.print_later,
                        contact: r.contact,
                        contact_name: r.contact_name.clone(),
                        account: r.account,
                        account_name: r.account_name.clone(),
                        amount: amount(r.amount)?,
                        debit: amount(r.debit)?,
                        credit: amount(r.credit)?,
                        memo: r.memo.clone(),
                        tag: r.tag,
                        closing_password: if r.locked { password.cloned() } else { None },
                    })
                })
                .collect::<Result<Vec<_>, LocalValidation>>()?;
            Ok(BatchGroup {
                kind: batch_kind(g.kind),
                context: g.context,
                rows,
            })
        })
        .collect()
}

fn field(f: BatchField) -> Option<Field> {
    match f {
        BatchField::Date => Some(Field::Date),
        BatchField::Number => Some(Field::Number),
        BatchField::Contact => Some(Field::Contact),
        BatchField::Account => Some(Field::Account),
        BatchField::Amount => Some(Field::Amount),
        BatchField::Debit => Some(Field::Debit),
        BatchField::Credit => Some(Field::Credit),
        BatchField::Memo => Some(Field::Memo),
        BatchField::Tag => Some(Field::Tag),
        BatchField::Row => None,
    }
}

/// One engine problem as the model keeps it.
#[must_use]
pub(crate) fn problem(p: &FieldProblem) -> EngineProblem {
    EngineProblem {
        field: field(p.field),
        kind: match p.kind {
            ProblemKind::NotInLists => EngineProblemKind::NotInLists,
            ProblemKind::Inactive => EngineProblemKind::Inactive,
            ProblemKind::Required => EngineProblemKind::Required,
            ProblemKind::Invalid => EngineProblemKind::Invalid,
            ProblemKind::ClosedPeriod => EngineProblemKind::ClosedPeriod,
            ProblemKind::Unbalanced => EngineProblemKind::Unbalanced,
            ProblemKind::Other => EngineProblemKind::Other,
        },
        message: p.message.as_str().to_string(),
    }
}

/// A validate answer: one verdict per row number.
#[must_use]
pub(crate) fn verdicts(v: &BatchValidation) -> Vec<(u32, EngineVerdict)> {
    v.rows
        .iter()
        .map(|r| {
            let verdict = if r.ok && r.problems.is_empty() {
                EngineVerdict::Ok {
                    new_contact: r.new_contact,
                    account: r.account,
                }
            } else {
                EngineVerdict::Problems(r.problems.iter().map(problem).collect())
            };
            (r.row_number, verdict)
        })
        .collect()
}

/// A refused save's rows.
#[must_use]
pub(crate) fn refused(rows: &[BatchRowProblem]) -> Vec<(u32, EngineProblem)> {
    rows.iter()
        .map(|r| (r.row_number, problem(&r.problem)))
        .collect()
}

/// The CSV field a grid field is read from.
#[must_use]
pub(crate) const fn csv_field(f: Field) -> Option<CsvField> {
    match f {
        Field::Date => Some(CsvField::Date),
        Field::Number => Some(CsvField::RefNo),
        Field::Contact => Some(CsvField::Payee),
        Field::Account => Some(CsvField::Account),
        Field::Memo => Some(CsvField::Memo),
        Field::Amount => Some(CsvField::Amount),
        Field::Debit => Some(CsvField::Debit),
        Field::Credit => Some(CsvField::Credit),
        Field::PrintLater | Field::Tag => None,
    }
}

/// The mapping as `parse-csv` takes it (always sent explicitly).
#[must_use]
pub(crate) fn column_mapping(m: &Mapping) -> ColumnMapping {
    let mut out = ColumnMapping::default();
    for (f, col) in m {
        if let Some(c) = csv_field(*f) {
            out.0.insert(c, *col);
        }
    }
    out
}

/// One parsed row as the model reads it.
#[must_use]
pub(crate) fn cells(r: &ParsedRow) -> ParsedCells {
    ParsedCells {
        date: r.date.clone(),
        number: r.ref_no.clone(),
        contact: r.contact_name.clone(),
        account: r.account_name.clone(),
        memo: r.memo.clone(),
        amount: r.amount.clone(),
        debit: r.debit.clone(),
        credit: r.credit.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{CalendarDate, Money};
    use cl_model::forms::batch::{BulkBatch, RowDraft, RowKeyGen, Section};

    #[test]
    fn rows_map_one_to_one_with_the_password_on_locked_rows_only() {
        let bank = crate::company::demo::account_id(1010);
        let mut keys = RowKeyGen::new();
        let mut s = Section::new(SectionKind::Checks, Some(bank));
        for (d, a) in [(10, "12.35"), (25, "7.00")] {
            let mut r = RowDraft::new(keys.next_key());
            r.date = CalendarDate::from_ymd(2026, 9, d).ok();
            r.amount = Money::parse_wire(a).ok();
            r.account_text = "6100".into();
            s.rows.push(r);
        }
        let b = BulkBatch::new(s);
        let closing = CalendarDate::from_ymd(2026, 9, 15).ok();
        let out = b.outgoing(None, closing);
        let pw = Secret::new("demo-close");
        let g = groups(&out, Some(&pw)).expect("groups");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].kind, BatchKind::Check);
        assert_eq!(
            g[0].rows[0].amount.map(BatchAmount::to_wire).as_deref(),
            Some("12.35")
        );
        assert!(g[0].rows[0].closing_password.is_some(), "09/10 is closed");
        assert!(g[0].rows[1].closing_password.is_none(), "09/25 is open");
        assert_eq!(g[0].rows[1].account_name.as_deref(), Some("6100"));
        for k in SectionKind::ALL {
            assert_eq!(batch_kind(k).wire(), k.engine_kind());
        }
    }

    #[test]
    fn a_sub_cent_amount_is_refused_never_rounded() {
        let mut keys = RowKeyGen::new();
        let mut s = Section::new(
            SectionKind::Expenses,
            Some(crate::company::demo::account_id(1010)),
        );
        let mut r = RowDraft::new(keys.next_key());
        r.date = CalendarDate::from_ymd(2026, 11, 1).ok();
        r.amount = Money::parse_wire("2.505").ok();
        r.memo = "x".into();
        s.rows.push(r);
        let out = BulkBatch::new(s).outgoing(None, None);
        assert_eq!(groups(&out, None), Err(LocalValidation::NotWholeCents));
    }
}
