# Reading documents

ClientLedger can read a vendor's bill or receipt for you and fill in the vendor, dates, ref. no. and amounts. It reads on your own computer. It works offline and with AI turned off, and nothing is sent anywhere.

You always check what it read before you save. ClientLedger never saves a bill on its own.

## Read document

1. Open **Vendors > Enter Bill** (`Ctrl+B`) and attach the vendor's bill or receipt. It shows in **Source document** next to the form.
2. Click **Read document**. You'll see "Reading the document on this computer…". A clear PDF takes a moment; a scan or photo takes a second or two a page.
3. ClientLedger fills only the fields that are **empty**. It never overwrites what you typed. Read values are marked, and values that weren't printed clearly are marked more strongly.
4. Check them against the document. Click **Accept these values** (`Alt+A`), or **Clear and type it myself**. Editing a read value also accepts it. You can't save while read values are waiting for your Accept.

What it reads:

- **PDFs that contain text**, such as bills a vendor emails you.
- **Scans and photos**: scanned PDFs, PNG, JPEG and TIFF pictures. The text is recognized on your computer.
- The **first 10 pages** of a document. If there are more, you'll see "Only the first 10 pages were read."

More things to know:

- **Vendor.** ClientLedger looks for the vendor among your vendors. If the name on the document isn't one of them, you'll see **Add as a new vendor?**. Nothing is added on its own.
- **Lines.** Each line gets the vendor's default account. Change it if you need to. If the lines can't be read clearly, you get one line for the document's total, and a note says "The lines couldn't be read, so one line has the document's total. Split it if you need to."
- **Balance due.** If a document shows a total and a smaller balance due (for example after a deposit), the bill's amount is the total, and a note shows both so you can change it.

## Checks that never change anything

After reading, ClientLedger checks that the figures agree. Problems are listed under "Check these on the document (nothing was changed):". For example:

- "The lines add up to $410.00, but the document's subtotal is $401.00."
- "The subtotal, tax and fees add up to $428.04, but the document's total is $482.04."
- "On line 2, quantity × price isn't the amount printed. The printed amount was used."
- "The date could also be read day first. Check the date."
- "An amount read as 801.00 could also be 80.00. 801.00 was used: check it on the document."
- "Pick the card this was paid with."

A check **never changes a value**. Every amount you see is the one printed on the document. If something doesn't add up, compare it with the document and fix the field yourself.

## Cancel

While a document is being read, **Cancel reading** stops it. Nothing is filled in. You can read it again or type the bill in. Closing the window or starting a new bill also stops a reading.

## "Couldn't read this document"

Sometimes a document can't be read. The form keeps working, so enter the bill by hand. The message tells you why when it can:

| Message | What to do |
| --- | --- |
| Couldn't read this document, enter the bill by hand. | The file was damaged, unusual, or took too long. Type the bill in. |
| …the PDF has a password. | Type the bill in, or ask the vendor for a copy without a password. |
| …it's too large. | Files over 10 MB aren't read. Type the bill in. |
| …it isn't a PDF or a picture ClientLedger reads. | Save the document as PDF, PNG or JPEG. |
| …it's a scan, and the scan reader isn't installed on this computer. | Text PDFs still read. Reinstall ClientLedger to get the scan reader back. |
| …the document reader isn't available on this computer. | Reinstall ClientLedger. |

Reading happens in a separate program with strict limits, kept apart from your books. If a file is damaged or built to cause trouble, that program is stopped and you see this message.

## Ask AI to help

If AI suggestions are on for your company, you can ask AI to help with what couldn't be read. When a reading leaves the vendor, date, ref. no., total or lines empty or unclear, the banner offers **Ask AI to help**. Nothing is sent until you click it.

- **AI on this computer (an Ollama model):** nothing leaves your computer, and you don't need to allow anything else.
- **An online AI service** (your own Anthropic or OpenAI key, or an Ollama model whose name ends in "cloud"): your company must first allow it in **Edit > Preferences > Reading documents online** (below). Until then the banner tells you where to allow it.

AI never sets an amount on its own. An amount AI suggests is used only when the same amount is printed on the document; otherwise you're told the two disagree. You still accept every value before saving.

## Reading documents online (Preferences)

**Edit > Preferences > Reading documents online** decides whether Ask AI to help may send a document to your online AI service. Only the company's owner can change it, and AI suggestions must be on first.

There are two switches, and **both are off** until you turn them on:

- **Send the document's text**: the words ClientLedger read from the page. Personal details your AI settings recognise, such as account and card numbers, are hidden first.
- **Also send pictures of the page**: only for a document that is itself a picture (PNG). A picture can help with a hard-to-read scan, but **a picture can't have anything hidden**: everything printed on it, including names, addresses and account numbers, goes to the AI service as it is. If your AI settings don't allow pictures to be sent, only the text is sent.

Each change to these switches is recorded in the company's audit trail. If you turn AI off, or agree to a new AI notice, both switches turn off again. ClientLedger tells you when that happens, and you can turn them back on here.

If your company's books are on an older engine, the screen says it needs a newer version, and nothing is sent online.

## Card ends in

For receipts paid by card, ClientLedger can tell which card was used from the last 4 digits printed on the receipt. To make this work, enter the digits on the card's account:

1. Open **Lists > Chart of Accounts** (`Ctrl+A`).
2. Open your credit card account to edit it, or create one with **New account**.
3. Type the card's last 4 digits in **Card ends in**, and save.

The field shows only on credit card accounts, and only when your books support it. If two card accounts have the same digits, or none match, you'll see "Pick the card this was paid with."
