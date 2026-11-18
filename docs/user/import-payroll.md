# Import payroll

ClientLedger doesn't run payroll. Your payroll service (Gusto, ADP, Paychex,
Square, OnPay, QuickBooks Payroll or any other) already did that. **Payroll ›
Import Payroll** brings the result into your books: one journal entry for
each pay date, with wages, payroll taxes, money owed and net pay on the right
accounts.

## Steps

1. **Choose file.** Pick your payroll service and the file you exported from
   it (CSV, TSV, TXT or Excel, up to 10 MB). Enter the pay period and the
   check date. For a file with several pay dates, use the first period's
   start, the last period's end and the last pay date.
2. **Match columns.** Only for files with one row per worker. The usual
   layout for your service is filled in. For any other CSV, choose which
   column holds each amount once; tick *Save this layout for next time* and
   the next file with the same columns skips this step. Only numbers and
   dates from the file are shown here, never names. If the file has a
   Department column, give each department a tag: its wages and employer
   costs get that tag.
3. **Accounts.** Choose the account for each kind of amount. *Net pay is paid
   from* should be your bank account: then each payroll withdrawal waits in
   the Bank Feed to be matched.
4. **Check.** The books read every row. A row that can't be used is named
   ("Row 7: Federal income tax withheld isn't a number"); fix the file or the
   columns.
5. **Review entries.** One entry per pay date, with its totals. An entry that
   doesn't balance says by how much; it usually means a deduction column
   isn't matched. If your company requires a tag on every line, Post tells
   you which line still needs one.
6. **Done.** The entries are posted. Worker names from the file are
   discarded once the entries are posted.

## History

The **History** tab lists every import: service, pay period, check date,
status and entries. **Reverse** voids a posted import's entries (it asks why,
and for the closing date password if the books are closed). **Discard**
removes an import that was never posted.

## Good to know

- The same file can't be posted twice. Reverse the first import in History
  if it was wrong.
- If payroll you already posted covers the same dates, you're asked before
  posting.
- People who can't see every account (some accounts are marked sensitive)
  can't import payroll, because payroll shows wages.
- Shortcuts: Alt+N goes to the next step, Alt+B back, Esc steps back.
