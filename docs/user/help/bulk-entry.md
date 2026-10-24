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

## Fill many rows at once

### Copy down

Click a cell and press **Ctrl+D** to copy its value into the same column of the other filled-in rows. You can also use the right-click menu (see below), which also has **Copy to rows below**. It is one undo step (`Ctrl+Z`).

Amounts, Debit, Credit and check or entry numbers are never copied. Copy down is refused, with a note on the status line, when:

- you are on the empty line at the bottom for a new entry: "This is the empty line for a new entry. Copy from a line that is filled in."
- the row has no amount yet: "Enter this line's amount first, then copy from it."
- the cell is empty: "This cell is empty, so there is nothing to copy."

### Set

Tick rows in the column at the left (or press Ctrl+A, below). A bar appears with **Set Date**, **Set Name**, **Set Account**, **Set Tag**, **Set Memo** and, for checks, **Set Print Later**. Pick the value, then choose **Apply**. Blank rows are skipped. Amounts and numbers can't be set. It is one undo step.

### Delete Rows

With rows selected, choose **Delete Rows**. It asks first. The rows aren't saved yet, so they are simply removed, and `Ctrl+Z` brings them back.

### Select all with Ctrl+A

If the cell you are in has text, the first **Ctrl+A** selects that text. Press it again to select all rows. If the cell is empty, the first press selects all rows. **Select All Rows** is also in the right-click menu. The empty line for a new entry isn't selected.

### Journal entries stay apart

Journal entry lines with the same date and Entry No. make one entry. Copy down or Set never moves a line into a different entry. If it would, those rows stay as they were and the status line says: "This would join two journal entries with the same date and Entry No., so N rows were left as they were." The rest of your change still applies.

## Menu and columns

**Shift+F10** or the **Menu** key opens the same menu as a right-click, for the cell you are on or for a column heading.

**Columns…** shows, hides and reorders the columns of the section's type. Untick a column to hide it. Hidden columns keep their values and still save. Select a column and press Alt+Up or Alt+Down to move it, or use the Up and Down buttons. **Reset to default** brings back the original set. **Done** closes the panel. The columns a type needs to save, such as the amount, can't be hidden; they say "Needed to save checks." and so on.

**F8** (next row that needs attention) shows a hidden column again if a row has a problem in it: "The Memo column is shown again: a row has a problem in it."

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
