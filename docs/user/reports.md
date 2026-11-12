# Reports

Reports show what your books say: how much you made, what you own and owe, who owes you, and where the money went. Open them from **Reports** in the menu or the left rail. Profit & Loss opens first.

## Picking a report

The list on the left groups the reports:

- **Company**: Profit & Loss (also by month, by fiscal period, by client and by tag group), Balance Sheet, Trial Balance, General Ledger, Budget vs. Actual.
- **Who owes what**: A/R Aging, Open Invoices, A/P Aging, Unpaid Bills and the balance reports for clients and vendors.
- **Sales & spending**: Sales by Client, Expenses by Vendor, Sales Tax Liability, the 1099 reports and more.
- **Memorized**: reports you saved with your own settings.

Type in **Find a report** to narrow the list. A report shown in grey says **Needs an engine update**: it arrives with a later update of the accounting engine.

## Report Builder reports

Under **Report Builder** in the report list are ready-made reports built from your books, such as Profit & Loss and Transaction Detail by Account. They open in the same window, with the same dates, Accrual or Cash, printing and exporting.

- Click **+** beside a line to open it, or **Collapse** (it reads **Expand all** when everything is closed) to open or close everything at once.
- A long list stops at a **Load more** button: click it for the next lines.
- Click an underlined amount to see the transactions behind it in a new tab, with their total at the bottom. Click a transaction's amount to open it.
- A part of the report your role isn't allowed to see says so, and totals that need it show a dash (—).
- If the books change while you look at the lines behind an amount, the report says so: click **Refresh**.
- A report that takes too long stops after 30 seconds. Try fewer months or open fewer lines.

## Build or change a report

Open the **Report Builder** in two ways:

- **New report…** under **Report Builder** in the report list asks five questions: what to report on (sales, purchases, who owes you, all accounts and more), how to group it, which columns, which dates, and a name. The preview on the right fills in as you answer. Press **Alt+N** for the next question and **Alt+B** to go back. At the end, **Run report**, **Memorize**, or **Open in Advanced** to keep changing it.
- **Edit in Report Builder** on a report's toolbar opens that report's rows and columns in the Report Builder, with the same dates and Accrual or Cash. Filters aren't carried over; add them as filters on a row. A report that can't be changed in the Report Builder yet says so.

In **Advanced**, the report is a list of rows on the left, the selected row's settings in the middle and a live preview on the right.

- **Rows**: a *section* adds up transactions (for example all accounts whose type is Income); a *group* puts rows under one heading with a total; a *formula* adds and subtracts rows above it (Income − Cost of goods sold = Gross profit); *text*, *blank rows* and *page breaks* help with layout. A report has up to 200 rows.
- Keys in the row list: **Insert** adds a row, **Delete** removes one, **F2** renames it, **Alt+↑/↓** moves it, **Tab** puts it inside the group above and **Shift+Tab** takes it out. **F6** and **Shift+F6** move between the row list, the settings and the preview.
- **Columns** tab: a column per month, quarter or year; a comparison with the period before or the same period last year ($ and % change); a "% of" column; the transaction columns to show; and your own column headings.
- Every figure comes from your books, the same as when you run the report. The preview shows up to 50 transactions under each line; **Run report** (or **Ctrl+Enter** to refresh the preview) shows them all.
- **Ctrl+Z** undoes a change and **Ctrl+Y** redoes it. Something that needs fixing shows under **Problems to fix**, with **Go to row**; the report runs again once it is fixed.
- **Memorize** (**Ctrl+M**) saves the report under its name. **Save changes** updates a memorized report you made (or any, if you are an owner). If someone else changed it meanwhile, you can keep yours as a copy or load theirs.

## Changing what a report shows

- **Dates**: pick a range like *Last month* or *Year to date*, or type your own **From** and **To**. Reports that show a balance take one **As of** date.
- **Columns**: one total, or a column per month, fiscal period, quarter or year, per client, or per tag of a tag group.
- **Accrual** or **Cash**: accrual counts income when you invoice and expenses when you get the bill; cash counts them when the money moves. Reports where it makes no difference say so.
- **% of income** shows each line as a share of your total income. **Year to date** adds a column from the start of your fiscal year. **Prior period** and **Prior year** add a comparison and the change.
- **Customize** changes the title and the run time on the header, and can split each account by the tags of a tag group (each tag on its own line, plus the lines with no tag and the account's total).

The report runs again right away. **Collapse** folds the sub-accounts and sections and then reads **Expand all**; **Refresh** (`F5`) runs it again.

## Seeing the transactions behind a number

Click any underlined amount to open the transactions behind it in a new tab. The total there matches the amount you clicked. Click one of a transaction's amounts to open its form. If the transaction is on or before your closing date, ClientLedger asks for the closing-date password before you can change it.

## Saving, printing and sending

- **Memorize** (`Ctrl+M`) saves the report with its settings under a name you choose. It shows under **Memorized**; if you picked a range like *Last month*, the dates move with the calendar.
- **Print** (`Ctrl+P`), **Save PDF**, **Export Excel** and **Export CSV** use the figures on screen. Excel and CSV files keep exact amounts.
- A report too wide for an upright page prints sideways (landscape), in smaller type if needed; a very wide one continues its columns on the next pages, each repeating the row names. Amounts and dates are never cut off.
- **Email** sends the report as a PDF, Excel or CSV file through your own mail server (set it up in **Preferences > Email**).

## Asking a question

If AI is on for your company (**Preferences > AI**, *Ask a question in Reports*), type a question in **Ask**, like *How did October compare with the year so far?*, and press Enter. ClientLedger picks the report and its settings and shows them after **Ran as**, so you can check or change them. The AI only chooses a report and dates; every number comes from your books. If no report fits, it says so and leaves your report as it was.

## Budgets

See [Budgets](budgets.md).
