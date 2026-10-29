# Pay Bills

Use Pay Bills when you want to pay several bills from vendors at once, by check or by credit card. Open it from **Vendors > Pay Bills**, from **Home** (under Money out), or press `Ctrl+Shift+B`.

Paying here marks each bill as paid and lowers what you owe (your accounts payable). To record a bill first, use Enter Bill (see Enter Bill). If you only want to write one check that isn't for a bill, use Write Check.

## Choose which bills to show

At the top of the screen:

- **Show bills**: **Due on or before** a date (it starts a week from today), or **All bills**.
- **Vendor**: all vendors, or just one. When a vendor has credits you haven't used, the list shows "Credit available" with the amount.
- **Sort by**: due date, vendor or amount due.
- **Take discounts I qualify for**: leave it ticked to have early-payment discounts applied for you (see below).

## Tick the bills to pay

Each row is one bill. Tick the box in the **Pay** column, or move to the row and press `Space`. The box at the top of the column ticks every bill in the list.

When you tick a bill, ClientLedger fills in the rest for you:

- **Discount**: the early-payment discount the bill's terms give for your payment date, if there is one. The books work it out.
- **Credits**: any open credits you have with that vendor, up to what the bill needs.
- **Amt. to pay**: the amount due, less the discount and the credits.

You can change any of these. If a filter hides a bill you ticked, it stays ticked but isn't paid until you show it again. The screen tells you how many are hidden.

### Paying part of a bill

To pay only part, type a smaller number in **Amt. to pay**. The rest of the bill stays open and you can pay it later. A bill can't be paid more than it owes.

### Set discount

Select a bill and click **Set discount** to change the discount.

1. Enter the **Discount amount**, or click **Use suggested**.
2. Choose the **Discount account**, the account your discounts are recorded in.
3. Click **OK**.

You pick the discount account once. ClientLedger remembers it for the next time. If you take a discount and haven't picked one yet, you'll be asked first.

### Set credits

Select a bill and click **Set credits** to choose which of the vendor's credits to use, and how much of each. A credit can't be used for more than it has left. If the vendor has no credits, the screen says so.

**Go to bill** opens the selected bill.

## Totals

The **Totals** row counts only the bills you ticked, in every column, and says how many ("4 of 5 selected"). A bill you left unticked isn't in any total.

## How you pay

In the **Payment** box, check the **Payment date** and choose the **Method**.

**Check**

1. Choose the account to pay **From**.
2. Under **Check numbers**, choose how the checks are numbered:
   - **To be printed** (the default): the checks wait in Print Forms and get their numbers when you print them.
   - **Assign numbers now**: enter the **First check number**. You get one check per vendor, numbered in order. A number already used in that account is refused; enter the next free one.

**Credit card**

1. Choose the credit card account to pay **From**.
2. Add a **Reference** if you want, such as the card's confirmation number.

There are no check numbers. The amount you owe on the card goes up.

Next to **Pay from**, the balance shows before and after, for example "84,210.55 → 77,633.02". It is a preview from your books; after you pay, the real balance is read again.

## Pay the bills

Click **Pay 4 bills** (the button shows the count and the total) or press `Alt+S`. The screen then says how many bills it paid and how many payments it made. If something needs fixing first, such as no bill ticked or no account chosen, the screen tells you what.

**Clear selections** unticks everything.

## Looking at a bill payment

Open a payment from a register, where it appears as BILLPMT. It opens as a read-only page: vendor, date, check number ("To print" if not printed yet), the account it was paid from, the total, and each bill with what was paid, the discount and the credit.

A bill payment can't be edited. To change one:

1. Click **Void**, enter a reason, and click **Void payment**. The bills it paid are owed again.
2. Open Pay Bills and pay them again, the way you want.

A payment dated on or before your closing date can be voided only with the closing-date password, and the void is recorded in the audit trail. If the payment is part of a finished bank reconciliation, undo the reconciliation first.

## If the bills changed

- **"Some of these bills changed since the list was shown"**: someone paid or edited a bill after you opened Pay Bills, or you did in another window. The list has been brought up to date. Check the amounts, then pay again.
- **"We couldn't tell whether the payment went through"**: the connection dropped as you paid. Don't pay again yet. Click **Look again**. Bills that were paid are gone from the list. If the screen says the payment went through, nothing was paid twice. If the bills are still there, pay them again.

If a window says discounts or check numbering "needs a books update", your company is on an older engine that can't do that part yet. Nothing was paid. Choose **To be printed**, or set the discount to zero.
