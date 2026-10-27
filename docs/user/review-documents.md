# Review Documents

Review Documents is the list of bills and receipts that have arrived in your intake folders and are waiting for you to enter them. Open it from **Vendors > Review Documents**, or from **Home** when it says "N documents to review". You can also open it from the [receipt inbox](receipt-inbox.md).

Nothing is entered on its own. You open each document, check what ClientLedger read, and save it in the usual form.

## The list and its statuses

Each row shows the kind of document (Bill, Card charge, Check, Expense, or Not sure yet) and a status in words:

- **Waiting to be read**: ClientLedger hasn't read it yet.
- **Ready**: it was read and nothing looks wrong.
- **Check these fields**: some values weren't printed clearly. Look at them against the document.
- **Possible duplicate**: a record with the same amount and a close date is already in your books. See "Attach a receipt to something already in your books" below.
- **Couldn't read**: the document can't be read. Open it and enter it by hand.

A document from a **Vendor statements** intake folder is a vendor's statement. Opening it takes you to Match Vendor Statement instead of a bill form.

The list shows 50 documents a page. Use **Filter** to narrow it by kind, status or number, and **Refresh** to look again.

## Keys

| Key | What it does |
| --- | --- |
| `J` | Next document |
| `K` | Previous document |
| `Enter` | Open the document in its form |
| `Delete` | Discard the document (you're asked first) |

The letters don't work while you're typing in the Filter box. Pressing Delete asks "Discard this document?" Choose **Discard** or **Keep it**. A discarded document is never entered, and its file moves to the `Discarded` folder inside your intake folder. Nothing is deleted from your computer.

There is no key in the list that enters a document. You always enter it from its form, after you've seen it.

## Read all

Choose **Read all** to have this computer read every document that is still **Waiting to be read**. It shows "Reading 3 of 12..." and you can choose **Cancel reading** to stop; documents not read yet stay waiting. Reading happens on your computer and nothing is sent online. A document that is open in a form is skipped.

Documents in a folder set to read automatically are read as they arrive. See [Document intake folders](document-intake.md).

## Enter a document from its form

1. Select a row and press `Enter`. The document opens in Enter Bill, Record Card Charge or Write Check, with the document beside the form.
2. Read values are marked. Check each against the document. Choose **Accept these values** (`Alt+A`), or change a value (editing a value accepts it). You can't save while read values are waiting for your Accept.
3. Choose **Save & Close** or **Save & New**.

If ClientLedger can't tell what the document is, it asks "What is this document?" Pick the form. There is no default.

Closing the form without saving asks if you want to discard your changes. The document stays in the list. `PgUp` and `PgDn` in the form move to the previous or next document.

To throw a document away from its form, choose **Discard document...**.

If the document changed on another computer while you had it open, you can choose **Keep my values and enter** or **Reload**. If it was already entered or discarded on another computer, nothing is entered a second time.

### Enter this as

If the document opened in the wrong form, use **Enter this as:** to switch to a bill, a card charge, an expense paid from a bank or cash account, or a check. Values you accepted or changed go with you; the rest still need Accept.

The switch only changes the form on your screen. The document itself keeps its old kind in the list until you save it, so if you switch and then close the form without saving, the list still shows the original kind.

## Attach a receipt to something already in your books

Sometimes you already entered the purchase, for example from your bank feed, and the receipt only needs to go with it. When you save a document, ClientLedger looks for records with the same amount, dated within 7 days, that are in your books. If it finds any, it asks "Is this already in your books?" and lists them beside the document's date, payee, amount and account, marked "close match" or "possible match".

- Pick the record that is this purchase, then choose **Attach to selected**. Nothing is selected to begin with, so pressing `Enter` can't attach by accident.
- You're asked once more: "Attach this document to the check of 3/14 for $25.00? It won't be entered as a new one. This can't be undone." Choose **Attach** to go ahead, or Cancel.
- If none of them is this purchase, choose **Enter as new**.

**Attaching can't be undone.** The receipt moves onto that record, and the document leaves the list. If you attach by mistake, remove the file from that record (in its attachments) and enter the purchase by hand.

Attaching adds the receipt to the record. It doesn't change the record's amount, date or accounts, and it doesn't enter anything new.

Unpaid bills with the same reference are shown for information only.

## Compare the document with the form

Beside the form, the document shows where each value was read.

- Click a field and the page turns to the part it was read from, outlined. Click an outline and the field is selected.
- **Highlights** turns the outlines on or off.
- Outline styles show how sure ClientLedger was: **Sure**, **Check this** (marked with a "?"), or **Not found on the page** (dashed).

### Click to fill

If a value is wrong or missing, click its field. The document says "Click a word to fill: Amount due." Click the word on the page and the field is filled. For dates and text, `Shift`+click adds the next words.

An amount fills only from one amount. If the word isn't one amount, you see "That isn't one amount" and nothing changes. A filled field counts as something you entered.

On a scan or photo, drag a box around the words. ClientLedger reads just that part, on your computer. "Reading that part of the page..." is shown while it works. If the box is too small, drag a larger one.

In the pop-out document window, `Alt+Right` and `Alt+Left` move from word to word and `Enter` fills the field.

### Check list

**Check list** shows every field beside the part of the page it was read from. For each, choose **Looks right** once you've compared it, or **Find on page** to jump to it and its field.

## Ask AI to help

If AI suggestions are turned on for your company, the banner offers **Ask AI to help** once the page has been read. Nothing is sent until you click it, and AI never sets an amount that isn't printed on the document. See [Reading documents](document-reading.md) for the online switches.

## "Filled from <vendor>'s saved layout"

The banner over the form says where the values came from:

- **Filled from <vendor>'s saved layout**: you've entered documents from this vendor before, and ClientLedger used what it learned about where their numbers are printed.
- **Filled from <file> in your receipt inbox**: it read the document without a saved layout.

In both cases, check each value against the document, then save. **Clear and type it myself** empties the fields. ClientLedger saves or updates a vendor's layout only after you enter a document, never after an attach.

## The card's last four digits

For a receipt paid by card, ClientLedger can tell which card from the last four digits printed on it. If the receipt shows digits and your card account doesn't have them yet, you'll see **Remember card ending 1234**, already checked. Saving the document also saves the digits on that card account, so the next receipt picks the card by itself. Uncheck it if you don't want that.

If the account already has different digits, they aren't changed and you see a note. If two cards have the same digits, or none matches, pick the card yourself.

## Documents on more than one computer

If another computer already has the same document waiting, this computer's row says it's being entered elsewhere. **Discard** on this computer only moves this computer's file to the `Discarded` folder; the document stays waiting on the other computer.

## When something doesn't work

- "This company's books can't keep documents for review yet. Update ClientLedger to use Review Documents." Your books are on an older engine.
- "Attaching to a record needs a newer version of the bookkeeping engine. You can still enter it as new."
- "You don't have permission to enter this kind of document." Ask an Admin.
