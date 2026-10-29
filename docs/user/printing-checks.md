# Printing checks

Use this page to print checks on paper from ClientLedger. For invoices, receipts and credit memos, see [Printing forms](printing-forms.md).

You always see a preview first. Nothing goes to the printer until you click **Print** in the preview.

## Before you print: choose your check stock

"Check stock" is the blank, pre-printed check paper you load in your printer. Open **File > Printer Setup**, pick the bank account, and choose its **Check style** to match your paper:

- **Voucher (check on top, two stubs)**: one check at the top of the page with two stubs below it. This is the style a new bank account starts with.
- **Standard (three checks per page)**: three checks on one sheet.
- **Z-fold (folds into a window envelope)**: one check per page that folds so the payee's address shows through a #10 window envelope.

All three are for US Letter paper (8.5 x 11 inches). The style is saved for each bank account, so you can use different paper for different accounts.

Also in Printer Setup:

- **Print company name and logo on this account's checks**: tick this if your stock doesn't already have your company name printed. It is off by default.
- The check style and this tick box are saved with the bank account. The alignment (next section) is saved on this computer, for each printer.

## Line up the printing

Every printer feeds paper a little differently, so the first time you use a printer, test it on plain paper.

1. Open **File > Printer Setup** and choose the bank account and the printer.
2. Click **Print alignment test...**. ClientLedger prints a sample check ("Sample Payee Name", memo "Alignment test") with a ruler marked in millimetres.
3. Hold the page over a blank check in front of a light, so you can see where each line would land.
4. If the printing sits too far over or too low, enter how far to move it in **Move right (mm)** and **Move down (mm)**. Use a minus sign to move left or up. You can move up to 10 millimetres either way.
5. Click **Save setup**, print the test again, and repeat until it fits.

Alignment can fix a printer that shifts the page. It can't fix check paper that is cut or spaced differently from the three styles above.

## Print one check now

On **Write Check**, click **Print** after you enter the check. ClientLedger saves it, takes the bank account's next check number, and shows the preview. Use **Print later** instead if you want to print it with other checks.

## Print a batch of checks

Checks saved with **Print later** wait in a list. Pay Bills puts its checks there too when **Check numbers** is set to **To be printed**.

1. Open **File > Print Forms** and stay on the **Checks** tab.
2. Choose the **Bank account**.
3. Enter the **First check number**: the number printed on the first blank check in your printer. Checks get numbers in order from there. If any number is already used on this account, ClientLedger tells you and offers the next free one. Click **Use** that number to accept it.
4. Tick the checks to print. Each row shows the date, payee and amount.
5. Click **Preview 5 checks** (the number is how many you ticked).
6. Load the check stock, pick the **Printer** from the list, and click **Print**.

Standard stock always starts at the first check on the sheet. If your sheet is partly used, load a fresh sheet, or the leftover spots on the last page stay blank.

You can also print from Pay Bills. After you pay, press `Ctrl+P` and ClientLedger previews the checks it made. Check numbers are recorded when you click **Print**, not before.

Other buttons in the preview:

- **Save PDF...** saves the pages as a file instead of sending them to a printer.
- **Envelopes** prints #10 envelopes with your return address and the payee's address.
- **Back to the list** returns without printing.

### What prints on a check

The date, the payee, the amount in figures and in words (in capitals, like `ONE THOUSAND TWO HUNDRED THIRTY-FOUR AND 56/100*****`), the memo, and the payee's address in the right place for a window envelope. Long amounts in words get smaller to fit.

The largest amount ClientLedger will print on a check is 9,999,999,999.99.

Stubs show the payee, date, memo and lines. For a bill payment, a stub lists the bills paid, with the discount and credit on each. The stub amounts must add up to the check amount. If they don't, the check isn't printed. Open the check and fix its lines.

## After printing: "Did all checks print correctly?"

When you click **Print**, ClientLedger asks, "Did all checks print correctly?", and says which check numbers were sent.

- **All printed correctly**: click this and the checks keep their numbers.
- **First check that didn't print right**: enter its number. That check, and every check after it, go back to the list to print again. Their old numbers are noted and can be used again. Click **Print those again**.
- **Decide later**: the checks stay printed with their numbers. Home shows "Printed checks are waiting for your answer" until you answer. Click **Answer** there.

Why every later check too? Checks print in order, so if a printer jams on check 1043, checks 1044 and up are on the wrong paper too.

## When a check prints wrong

1. Don't throw the check away yet. Mark it "VOID" by hand and keep it with your records.
2. Open **File > Print Forms**, answer the question as above, and choose the first check that went wrong.
3. Load fresh check stock and print again. The checks use the numbers on the stock you load, so enter the number printed on the next blank check as the first number.

If the problem is the position of the printing, go back to **Printer Setup** and adjust the alignment before you reprint.

## Reprint a lost or damaged check

If one check is lost, spoiled or never arrived:

1. Open the check from Write Check or from the bank register.
2. Click **Reprint check**. The check gets the account's next number, and you see a preview first.
3. Click **Print**.

The check's payment stays in your books. It is not voided and the bills it paid stay paid. The old number is recorded in the audit trail (the log of changes) and can be used again. A check that was written by hand can't be reprinted here.

If the lost check was already mailed, you may want to tell your bank to stop payment on the old number.

### Closed periods

A reprint changes the check number. If the check is dated on or before your closing date, ClientLedger asks for the closing-date password first, and the change is recorded in the audit trail.

## Printing on Windows

On Windows, ClientLedger sends the pages straight to the printer you chose, on US Letter paper. You don't need a PDF app.

If that doesn't work, the PDF opens on your screen so you can print it from there. The message says "Printing didn't start, so the PDF is open. Print it from there." If the PDF was handed to your PDF app and ClientLedger can't tell whether it printed, the message says "Check that it printed." Look at your printer before you answer "Did all checks print correctly?"

Envelopes print through your PDF app, which sets the envelope size.

If nothing works, click **Save PDF...** and print the file from another program.

On Mac and Linux, ClientLedger uses the system's normal print command.

## Messages you may see

- "One of those check numbers is already used on this account." Enter another first number, or click **Use** the number offered.
- "The checks waiting to print changed." Another window printed or changed them. Go back to the list and try again.
- "An amount is too large to print on a check."
- "A check amount can't be negative or have fractions of a cent."
- "Printing didn't work. Use Save PDF and print the file."

ClientLedger deletes its temporary print files each time it starts, because they hold payee names and amounts.
