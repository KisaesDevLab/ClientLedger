# Enter Bill

Use Enter Bill to record a bill from a vendor that you will pay later, or a credit a vendor owes you. Open it from **Vendors > Enter Bill** or press `Ctrl+B`.

A bill adds to what you owe (your accounts payable, or A/P). To pay it, use Pay Bills (**Vendors > Pay Bills** or `Ctrl+Shift+B`; see [Pay Bills](pay-bills.md)).

## Bill or vendor credit

At the top, choose **Bill** or **Vendor credit**.

- A **bill** is money you owe the vendor.
- A **vendor credit** is money the vendor owes you, for example for returned goods. It has no terms or due date.

A saved bill stays a bill and a saved credit stays a credit.

## Entering a bill

1. Choose the **Vendor**. Start typing and pick a name, or press `Ctrl+L` for the whole list. Add the vendor first if they aren't there.
2. Check the **Date** and enter the vendor's **Ref. no.** (their invoice number).
3. Choose the **Terms**. They set the **Bill due** date and, for early-payment terms such as "2% 10 Net 30", the **Discount date**. For custom terms, enter the number of days (and a discount percentage and days, if the vendor offers one).
4. Enter the **Amount due**, the total printed on the vendor's bill.
5. Enter the lines (see below). Add a **Memo** and **Tags** if you want.
6. Press **Save & New** (`Alt+N`) or **Save & Close** (`Alt+S`).

When you pick a vendor you have entered a bill for before, ClientLedger copies details from their last bill, such as the account and terms. Amounts are never copied. If you typed the amount due first, the first expense line gets what is left of it; the others stay blank for you to fill in.

### Changing the due and discount dates

The dates are worked out from the terms and the bill date. To use your own date, type it in **Bill due** or **Discount date**. Clear the field to go back to the terms. The discount date must fall between the bill date and the due date.

## Expenses and items

Lines go on one of two tabs. Each tab shows its own total.

- **Expenses** lines: pick an account, enter the amount, and add a memo if you like. On each line you can also pick a **Customer / Project**, tick **Billable** (so you can bill that customer for it later), and add a tag. A billable line needs a customer.
- **Items** lines: pick the item, then enter the **Qty** and the **Cost**. The line amount is quantity times cost, so you change the quantity or the cost, not the amount.

Use `Ctrl+Insert` to add a line and `Ctrl+Delete` to remove one. A bill can have up to 500 lines.

**The lines must add up to the amount due.** Below the lines you see "Lines total = amount due" with a check mark, or how much is left to assign, or how much the lines are over. ClientLedger won't save the bill until they match to the cent.

When you have typed the amount due and pick the account on an **Expenses** line with no amount, the line gets what is left to assign after the other lines (items included): the whole amount due on the first line, then whatever is left. Type over it to change it, or press `Tab` to keep it. ClientLedger never changes an amount you typed, and never changes an item line.

## Attachments and preview

Click **Attach** to add the vendor's bill (a PDF, PNG or JPEG) or a receipt. Each file can be up to 10 MB. The file is shown in **Source document** next to the form. Use the zoom and page buttons there to read it while you type (see Document viewer).

If the bill saves but the file can't be attached, the screen says so and offers **Try again**. Your bill is not lost.

## The vendor panel

When you choose a vendor, the panel at the side shows:

- **Open balance**: what you owe this vendor.
- **Default account** and whether they are a **1099 vendor**.
- **Open bills**: unpaid bills, with their due dates.
- **Unused credits**: vendor credits you haven't used yet.
- **Recent bills** from this vendor.

If a list is long, the panel says "Showing 5 of 12" and so on.

## Duplicate bill numbers

As you type a **Ref. no.**, ClientLedger checks the vendor's bills from the last 18 months. If another bill uses the same number, the panel names it (for example "Bill 1042 on 10/03/2026 already uses ref. no. 5521") and offers **Open that bill**.

When you save, a window asks about it. Choose **Save anyway** if it really is a different bill, or **Go back** to look first. If the check couldn't look at every bill, the panel tells you to look through the vendor's bills yourself before saving.

## Changing, voiding and deleting

Use **Prev** and **Next** (`Page Up` / `Page Down`) to move through saved bills.

- **Void** keeps the bill in your books with a zero amount. Delete removes it; the audit trail keeps a record of it. To start, open the saved bill. You'll be asked for a reason when you void.
- **A bill with payments or credits applied can't be voided or deleted**, and its vendor, date and amount can't change. Void those payments or credits first (see [Pay Bills](pay-bills.md) for voiding a bill payment).
- A bill that has already been billed to a customer on one of its lines can't change.
- A **vendor credit that is applied to bills, or refunded, can't be changed, voided or deleted.**
- A bill dated on or before your **closing date** needs the closing-date password to change, void or delete. The audit trail records the change.
- If someone changed the bill since you opened it, you'll see "Someone changed this bill since you opened it." Choose **Reload**, or **Keep my copy to compare**.

If a window says part of a bill "needs a books update", your company is on an older engine that can't save that part yet. Nothing was saved.

## Read document

ClientLedger can read the attached bill for you, on this computer. It works offline and with AI turned off, and nothing is sent anywhere.

1. Attach the bill's PDF or picture (a scan or a photo works too).
2. Click **Read document** on the **Source document** card. Nothing happens until you click. **Cancel reading** stops it.
3. ClientLedger fills the vendor, dates, ref. no., amounts and lines, but only into fields that are empty. Read values are marked. Values that weren't printed clearly are marked more strongly.
4. Check them. Click **Accept these values** (`Alt+A`), or **Clear and type it myself**. Editing a read value also accepts it.

You can't save while read values are still waiting for your Accept. If the document is from a vendor you haven't added, you'll see **Add as a new vendor?**, which opens your vendor list. Nothing is added on its own.

If the figures on the document don't agree (for example, the lines don't add up to the subtotal), the problems are listed under "Check these on the document (nothing was changed):". These checks never change a value. If the document can't be read at all, you'll see "Couldn't read this document, enter the bill by hand." The rest of the form keeps working.

### Ask AI to help

If AI suggestions are on and something couldn't be read clearly, the banner offers **Ask AI to help**. AI on this computer (Ollama) needs nothing more. An online AI service needs your company's permission first, in **Edit > Preferences > Reading documents online**. An amount AI suggests is used only when the same amount is printed on the document, and you still accept every value.

See **Reading documents** for the details: what is read, every message, the checks, the online switches, and the **Card ends in** field for card receipts.

If your company's books are on an older engine, you'll see **Read with AI** instead of Ask AI to help. It needs AI on, with **Read bills and receipts** ticked in **Edit > Preferences > AI suggestions**. It sends the document to your AI service, and a picture of a bill can't have private details removed first.
## If ClientLedger can't tell whether a save went through

Sometimes the connection to your books drops at the moment you save. The screen then says **We could not confirm the save**, and saving is paused so the bill isn't entered twice.

1. Choose **Check again**. ClientLedger looks for the bill and tells you whether it was saved.
2. If it still can't tell, look in the Accounts Payable register yourself (**Find** opens it).
3. Only save again if the bill isn't there. If it is there, saving again would enter it twice.
