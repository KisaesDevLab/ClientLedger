# Match a vendor statement

A vendor's statement lists what they think you owe. Match Vendor Statement checks it against your books, so you can spot a bill you never entered or one the vendor left off. Open it from **Vendors > Match Vendor Statement**.

## Steps

1. Choose the **Vendor**.
2. Enter the statement period: **From** and **To**.
3. Type or paste the statement's lines into **Statement lines**.
4. Click **Match**.

## Read a statement file

If you have the statement as a PDF or a picture, ClientLedger can read the lines for you instead of you typing them.

1. Choose **Read a statement...**, pick the file, then choose **Read file**. You can also drop the file on the window. Files that arrive in Review Documents, or in an intake folder of the kind **Vendor statements**, open here too.
2. ClientLedger fills in the vendor, the period and the lines. The statement shows beside the lines.
3. Check each line against the statement. Click a line to see its spot on the page. Every line says "Read from the document - check it" until you accept it.
4. Choose **Accept** (`Alt+A`) for a line, or **Accept all lines**. **Match** stays off until the lines are accepted.
5. Choose **Match**, as before. You can still change any line in the box. Once you do, the lines are yours.

The reading happens on this computer. It works offline with AI off, and nothing is sent anywhere. A statement can be up to 20 pages. If there are more than 2,000 lines, the rest are left out and a note says so.

**If the vendor looks wrong.** If the statement seems to be from a different vendor than the one you picked, you get a warning and nothing is changed. Check the vendor.

**If the statement's own figures don't agree.** ClientLedger checks the totals, the running balance, the dates and repeated lines. Anything that looks off is flagged. Nothing is changed to make the figures agree. Check the statement with your vendor.

**Ask AI to help.** This is optional, and only works if you turned AI on. It can suggest the vendor and the closing balance. Each suggestion waits for you to choose **Use this...**. AI never makes up lines or amounts.

**The balance line.** After Match you see "Statement says you owe X; your books say Y." This is for comparison only and nothing is posted. Entries made directly to Accounts Payable (the account that tracks what you owe) aren't in the books' figure.

**Keep the statement.** **Save this statement with the vendor** is on by default. It keeps the file with the vendor, named by its period, so you can open it again later. It is not saved if the statement names a different vendor.

**If it can't be read.** You see "Couldn't read this statement - type or paste the lines instead." Use the steps above, as before.

## The line format

Put one entry on each line, with the date, the ref. no., the amount and, if you like, the type: `invoice`, `credit` or `payment`.

- Paste straight from a spreadsheet, or type the parts with commas between them.
- **Date:** `10/31/2026` or `2026-10-31`.
- **Amount:** `1,330.00`, `-75.00` or `(75.00)`. Credits and payments can be negative.
- Leave out lines with a zero amount.
- If your first line is column headings, ClientLedger skips it and says so.

If a line can't be read, it is listed with the reason (for example "Line 3: ...") and Match waits until you fix it.

## The results

Nothing is changed in your books by matching. You see four sections:

- **Matched**: statement lines found in your books. Each shows how it matched: **Same ref. no. and amount**, or **Same amount, close date**.
- **On the statement, not in your books**: you may have missed entering these.
- **In your books, not on the statement**: entries the vendor didn't list.
- **Needs your choice**: more than one entry in your books could be that line.

**Nothing is picked for you.** For lines in "Needs your choice", compare the candidates with the statement and decide yourself.

## Creating what's missing

In "On the statement, not in your books", choose **Create bill** (or **Create credit** for a credit). Enter Bill opens with the statement's date, ref. no. and amount filled in. Add the account, check it, then save. Payments aren't entered from here.

## Good to know

- This compares line by line. The balance line is shown for comparison only, and vendor refunds aren't listed.
- If the vendor has more entries in the period than can be read at once, ClientLedger says some may be missing. Try a shorter period.
- On older engines, vendor credits in your books don't keep the vendor's ref. no., so credits are matched by amount and date only. The window tells you when this applies.
