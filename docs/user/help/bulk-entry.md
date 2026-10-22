# Bulk Entry

Bulk Entry lets you type (or paste) many checks, expenses, card charges,
card credits, deposits and journal entries in one grid, check them, and
save them all at once. Open it from **Banking > Bulk Entry** or the command
palette (Ctrl+K, then type "Bulk").

## Sections

A batch is made of sections. Each section holds one type of entry on one
account, for example "Checks · 1010 Operating Checking" or "Deposits ·
1010 Operating Checking". Journal entries need no account.

- **Add section** adds a section; **Remove section** removes the one shown
  (it asks first when the section has rows).
- Pick the section's **Type** and its **Bank account** or **Credit card**
  above the grid. The type can change only while the section is empty.
- **Ctrl+Page Down** and **Ctrl+Page Up** move between sections.

Each type has its own columns. Checks: Date, No., Print later, Pay to,
Account, Amount, Memo, Tag. Deposits: Date, Ref no., Received from, From
account, Amount, Memo, Tag. Journal entries: Date, Entry No., Account,
Debit, Credit, Name, Memo, Tag. Lines with the same date and Entry No.
make one journal entry; its debits and credits must be equal.

The grid works like every other grid: Tab moves across cells, Enter
records the row, Ctrl+Insert inserts a row, Ctrl+Delete deletes one, and
the date keys (T, +, -, …) work in the Date column. New check rows get the
next check number.

## Checking rows

A row is checked as soon as you leave it: first by ClientLedger (missing
cells, amounts, journal entries that don't balance, check numbers used
twice), then a moment later by the books. A row that needs attention is
tinted and says why under it; the status bar tells you when any row needs
attention. **F8** goes to the next one and **Shift+F8** to the previous
one. **Check all rows** checks everything now.

A name or account that isn't in your lists shows "Not in your lists". Pick
one from the list, or tick **Add new names to the list** to add new names
when you save. Accounts are never added for you.

## Account suggestions

When you pick a name and leave the Account empty, ClientLedger suggests an
account: the name's usual account, then your rules, then the AI (only when
you turned AI on). The suggestion is shown greyed out; press **Tab** to
accept it. **Accept all shown suggestions** accepts them all after asking.
A suggestion never fills in an amount.

## Paste from a spreadsheet, or import a CSV file

Copy rows in your spreadsheet and paste them into the grid (Ctrl+V), or use
**Import CSV file…** (10 MB at most). Match the columns in the panel that
opens; tick **Remember for this company** to use the same choices next
time for a file with the same column names. Amounts are rounded to the
cent. Tags are not imported. A batch holds at most 2,000 rows.

## Save all

**Save all** (Alt+S) is on only when every row passes. Everything is saved
in one step: either every row is in your books, or none is. If any row is
dated on or before your closing date, ClientLedger asks for the
closing-date password once for all of them. **Save all & New** (Alt+N)
saves and starts a fresh batch.

After saving you see a summary: the rows saved per type, the totals per
account, and a link to open each register. If the save could not be
confirmed (for example the connection dropped), choose Save all again: the
books never save the same batch twice.

Closing the window with unsaved rows asks whether to save them. Rows are
not kept after you close the app.
