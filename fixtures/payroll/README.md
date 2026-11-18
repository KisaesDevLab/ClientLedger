# Payroll import fixtures (P30, synthetic)

Every file here is invented. Worker names are `Test Worker NN`, the company is
"Synthetic Test Co", and every date is in 2025. No real export, client or
person is behind any row. The bytes are kept exactly (`-text` in
`.gitattributes`) so file hashes match on every OS.

Pay dates are at least 7 days apart and no two pay periods share a day, so no
two fixtures overlap even when they are imported into one company.

| File | Mode / provider key | Pay dates | Shape taken from | Verified against a real export? |
| --- | --- | --- | --- | --- |
| `gusto.csv` | A `gusto` | 2025-01-10, 2025-01-24 | The engine's Gusto fixture and the Gusto system template headers, plus `Pay Period Start`, `Pay Period End` and `Department` (Kitchen, Front, blank) | Header names follow the engine template; the layout is the one the engine was built from |
| `adp_run_gli.csv` | B `adp_run_gli` | 2025-02-14 | The engine's `adp-run-gli.csv` | No (Q-P30b) |
| `quickbooks_payroll.csv` | A `qbo_payroll` | 2025-03-14 (from the wizard) | The engine's QuickBooks Payroll system template: 3 report rows, then `Employee, Pay Period, Total Hours, Gross Pay, Federal Taxes, State Taxes, Net Pay` and a total row, **plus** employee and employer Social Security and Medicare columns the template leaves out | No (Q-P30b) |
| `paychex_flex_gls.csv` | B `paychex_flex_gls` | 2025-04-11 | The engine's `paychex-flex-gls.csv` | No (Q-P30b) |
| `square.csv` | A `square_payroll` | 2025-05-09 (from the wizard) | The engine's Square system template headers only; no date column | No (Q-P30b) |
| `onpay_gl_summary.csv` | B `onpay_gl_summary` | 2025-06-13 | The engine's `onpay-gl-summary.csv`; the Category column decides the side | No (Q-P30b) |
| `generic_period1.csv`, `generic_period2.csv` | A `custom` | 2025-07-11, 2025-07-25 | Invented generic headers (`Worker, Paid On, Wages, Fed WH, State WH, SS EE, Medicare EE, SS ER, Medicare ER, Take Home`) | Not a provider layout |
| `paychex_correction.csv` | B `paychex_flex_gls` | 2025-08-22 | `paychex_flex_gls.csv`'s layout with a correction pair: Debit -50.05 on one row, Credit -50.05 on another (P30 review S5). No golden: the contract test compares the engine to `payroll_expect` directly, and `payroll_fixtures.rs` pins that expectation by hand | Not a provider export |
| `bank_payroll.csv` | Bank CSV (`Date, Description, Amount`; money out is negative) | — | — | Three net-pay withdrawals (Gusto twice, ADP) and one Gusto tax withdrawal on 2025-01-12 that has no expected entry (U-249 deferred) |

Amounts use awkward cents on purpose: thousands separators, `$`, quoted
cells, and values such as `.x5` and `.x9` whose floating-point sums drift.

## Goldens (`expected/<file>.json`)

One file per payroll fixture: the provider key, the mode, the pay period and
check date the wizard sends, and per pay date the expected lines
(`lineType`, `department`, `side`, `amount`) with the entry's totals.

- Mode A lines follow the engine's summary rules (one entry per pay date,
  U-242; Square's "other" taxes credited to `other_tax_payable`, U-243).
  Gusto's expense lines are split by department (U-246); liability lines and
  net pay (`payroll_clearing`) stay one line per date.
- Mode B lines are one per file row, in file order; `lineType` is the row's
  description.

The goldens were written from the CSVs with exact decimals.
`crates/cl-api/tests/payroll_fixtures.rs` recomputes every one of them
independently from the raw CSV with `rust_decimal` and fails if they differ.
Nothing here was copied from the engine or the demo.
