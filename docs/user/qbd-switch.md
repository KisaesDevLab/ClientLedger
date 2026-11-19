# Switch from QuickBooks Desktop

**File › Import from QuickBooks Desktop** brings your old books into a new
ClientLedger company: your accounts, customers, vendors, items, your history
and the open invoices and bills. Then it compares the result with the reports
you exported and tells you whether everything matches, to the cent.

QuickBooks is named here only because it is where your files come from. Your
QuickBooks company stays as it is; nothing is changed in it.

Only the company's Admin can switch. Do it in a **new company with no
transactions**; ClientLedger stops if the company already has some. (Memorized
transactions and QuickBooks users aren't brought over. You set up Templates
and users fresh.)

## Before you start

Pick the **switch date**: the day your books move to ClientLedger, usually the
last day of a month. Close out the day's work in QuickBooks first, so the
reports below don't change while you export.

Export each report to a **CSV file** from QuickBooks (Reports, then the
report, then Excel, then Create New Worksheet, Export to a CSV file). The
wizard's first step lists them with their menu paths:

| Report | Date to use |
|---|---|
| Account List | none |
| Customer Contact List, Vendor Contact List, Other Names List | none |
| Item List | none |
| Opening Trial Balance | the day before your history starts (the wizard shows it) |
| Journal | all dates of your history; one file per year keeps files small |
| Trial Balance on the switch date | the switch date |
| Open Invoices | **Report Date** = the switch date |
| Unpaid Bills Detail | **Report Date** = the switch date |
| Inventory Valuation Summary | the switch date (only if you track inventory) |

Two things trip people up:

- **Use Report Date, not "Current".** The aging reports default to the
  current date. Open balances must be as of the switch date or the check at
  the end will show differences. The wizard stops you if a report is dated
  differently.
- **An empty report is fine.** If you have no unpaid bills, export it anyway
  and add it.

Each file can be up to 10 MB.

## The steps

1. **Export reports from QuickBooks.** The list above.
2. **Add the files.** Drag each file onto its card, or use *Add file*. The
   Journal card takes several files. Each card says how many rows were read;
   a file that isn't the report on its card is refused with a reason.
3. **Review matches.** Accounts in your files are matched to accounts in the
   new company by number, then by name. Accounts from your account list are
   created for you. Your QuickBooks chart is used: when a starter account in
   the new company has the same number as a QuickBooks account, the
   QuickBooks account takes that number over (the starter account gets the
   QuickBooks name and type; Undo puts it back). Every account matched by
   number to an account with another name is listed with both names and
   waits for your OK, or for you to choose another account. Anything that
   matches nothing needs an account chosen; nothing goes to a holding
   account.
4. **History and start date.** The switch date is filled in from your trial
   balance. Choose how much history to bring:
   - **Two years plus opening balances** (recommended): the two fiscal years
     before the switch date's year, with everything earlier as opening
     balances. Example: switching on October 27, 2026 with a January fiscal
     year brings history from January 1, 2024; balances before that come in
     as opening balances dated December 31, 2023.
   - **All history in the files.**
5. **Import.** ClientLedger takes a verified backup first. If no backup
   passphrase is set yet, this step asks for one (it is saved as File ›
   Back Up Company saves it). If the backup fails, nothing is imported.
   Closing the window while the backup is being made cancels the import. Then the import runs with progress; it can't
   be posted twice. If you lose touch with the books midway, choose **Check
   status** before doing anything else.
6. **Check that it matches.** See below.
7. **Spot-check a few items.** See below.

If your books are closed through a date and the import has transactions on or
before it, you're asked for the closing-date password (or, if none is set, to
confirm). The import is noted in the audit trail.

## Check that it matches

ClientLedger compares your new books with the trial balance, open invoices,
unpaid bills and inventory valuation you exported. Each difference shown is
the new books minus your report.

- **0.00 means the books agree with QuickBooks to the cent** for that report.
  When every difference is 0.00 you see "Everything matches" and can choose
  **Mark verified and continue**.
- A difference names the account (or invoice, bill or item) that is off. Use
  *Show only differences*, the tabs (Trial balance, A/R aging, A/P aging,
  Inventory), and *Save PDF* for your accountant. If anything is off, **Undo
  the import**, fix the file and import again. Mark verified is off until
  every difference is 0.00.

### The "before conversion" lines

Under the table are checks labelled *Receivables before conversion*,
*Payables before conversion* and *Inventory before conversion*. Your old
history was posted here as it happened, and the open invoices, unpaid bills
and inventory valuation then moved those balances back to the right
accounts. What is left on the temporary before-conversion accounts must be
**0.00** at the switch date. Anything else means the open invoices, bills or
valuation report don't agree with the trial balance: usually a report dated
the wrong day (Current instead of Report Date), or a missing file.

## Spot-check

After Mark verified, a short list asks you to look at a few things in both
programs and tick each one that matches: bank balances, the three largest
open invoices and bills, sales tax and payroll liabilities, net income year to
date, inventory, and a few sample transactions. Add a note to any item. The
list is saved with the import.

## Undo, and restoring the backup

**Undo this import** (on the Check step, or from **History**) removes
everything the import brought in and takes a backup first. It's refused when
something now depends on the import: a payment on an imported invoice, a
reconciled account, a deposit, or an edited transaction. The window lists
what is in the way in plain words.

If it can't be undone, choose **Restore the backup taken before the import**.
That creates a **new company** from the backup; the current one stays as it
is, so nothing is lost. Backups taken before imports are kept (the newest 10).

## What to know about taxes

- **1099s.** For the year you switch, file your 1099s from QuickBooks.
  Payments from before the switch came over as history, but the conversion
  year's 1099s are not prepared here.
- **Sales tax on cash basis.** If you report sales tax when customers pay
  (cash basis), a payment received in ClientLedger on an invoice that was
  imported is **not** included in the sales tax report. Report those
  collections from QuickBooks, or tell your accountant to adjust them.
  Invoices created in ClientLedger are not affected.

## Other programs

**File › Import from Another Accounting Program** does the same for Accounting Power,
QuickBooks Online, a QuickBooks Desktop chart and general ledger only, or any
program that can export our spreadsheet layout. The steps, the check and the
undo work the same way. From QuickBooks Online, export each report to Excel
(.xlsx); the Account List may also be a CSV file. To bring in just a
spreadsheet of invoices, bills or journal entries, use **File › Import
Transactions from a Spreadsheet**.
