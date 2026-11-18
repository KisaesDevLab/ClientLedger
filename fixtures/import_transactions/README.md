# Spreadsheet transaction import fixtures (P31 D5)

Synthetic files for Import Transactions from a Spreadsheet (Q-125). Names
and accounts are the Ozark Ridge sample company's; dates are after its
closing date (09/30/2026).

- `good.csv`: 2 invoices (one of two lines), 2 bills (one with a negative
  line), 1 balanced journal entry. Hand totals: invoice 7001 = 1562.50,
  invoice 7002 = 480.00, bill OMS-5521 = 685.25, bill HF-2209 = 200.00,
  JE-1031 = 89.99 each side.
- `bad-rows.csv`: one problem per group, rows 2-9 (row 1 is the header):
  missing customer (2), unknown type (3), day-first date (4), five decimal
  places (5), unbalanced entry off by 0.01 (6), exponent amount (8), no
  amount (9).