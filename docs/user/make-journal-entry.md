# Make Journal Entry

A journal entry moves amounts between accounts directly, without a check, deposit or bill. Use it for corrections, depreciation, or entries your accountant gives you. Open it from **Company > Make Journal Entry** or press `Ctrl+J`.

## Debits and credits, simply

Every line puts an amount in one account, in either the **Debit** column or the **Credit** column, never both. The debits and the credits must add up to the same total. As a rule of thumb:

- A **debit** increases what you own (cash, equipment) and your expenses.
- A **credit** increases what you owe (loans, card balances), your income and your equity.

For example, paying 50.00 for fuel straight from checking is a debit of 50.00 to Fuel and a credit of 50.00 to Operating Checking.

## Make the entry

1. Check the **Date**. **Entry No.** suggests the next number; you can type another or leave it blank.
2. On each line pick an **Account** and type the amount under **Debit** or **Credit**. Typing in one column clears the other. Add a **Memo**, a **Name** or a **Tag** if you like.
3. Watch the line under the grid: "Debits $X · Credits $Y · Difference $Z". When you give the next blank line an account, it fills in the amount that makes the difference zero; you can type over it. **Balance on this line** puts that amount on the line you are on.
4. Press **Save & New** (`Alt+N`) or **Save & Close** (`Alt+S`). They stay off while debits and credits differ.

## Names on Accounts Receivable and Accounts Payable lines

A line on **Accounts Receivable** needs a customer in the **Name** column, and a line on **Accounts Payable** needs a vendor, so the amount shows up on the right customer or vendor balance. An entry can have only one line on Accounts Receivable or Accounts Payable.

## Adjusting entries

Tick **Adjusting entry** for an entry your accountant uses to correct or update the books at the end of a period. The entry is marked so it can be told apart from your everyday entries.

## Reversing entries

Type a **Reverse on** date (after the entry's date) and ClientLedger also enters the same entry on that date with the debits and credits swapped. This is handy for an accrual you want undone next month. The entry then shows "Reversed by the entry of DATE"; click it to open the reversal. Voiding or deleting the entry voids or deletes its reversal too.

## Change, void or delete

Find an entry with **Prev** and **Next**, change it and save. **Void** keeps it in your books with zero amounts; **Delete** removes it from registers and reports (both stay in the audit trail). If the entry is dated on or before your closing date, ClientLedger asks for the closing-date password first.

To reuse an entry, choose **Save as Template** (`Ctrl+M`); see [Templates](templates.md).
