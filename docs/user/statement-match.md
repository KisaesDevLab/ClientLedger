# Match a vendor statement

A vendor's statement lists what they think you owe. Match Vendor Statement checks it against your books, so you can spot a bill you never entered or one the vendor left off. Open it from **Vendors > Match Vendor Statement**.

## Steps

1. Choose the **Vendor**.
2. Enter the statement period: **From** and **To**.
3. Type or paste the statement's lines into **Statement lines**.
4. Click **Match**.

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

- This compares line by line. It doesn't compare balances, and vendor refunds aren't listed.
- If the vendor has more entries in the period than can be read at once, ClientLedger says some may be missing. Try a shorter period.
- On older engines, vendor credits in your books don't keep the vendor's ref. no., so credits are matched by amount and date only. The window tells you when this applies.
