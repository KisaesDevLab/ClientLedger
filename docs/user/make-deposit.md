# Make Deposit

Use Make Deposit to record money you took to the bank. Open it from **Banking > Make Deposit** or press `Ctrl+D`.

## Payments waiting to be deposited

When a customer pays you, the payment doesn't go straight into your bank account in your books. It waits in an account called **Payments Clearing** (other programs call it "Undeposited Funds") until you record the trip to the bank. That way one bank deposit with several checks in it shows as one deposit, just like on your bank statement.

1. Pick the bank account under **Deposit to** and check the **Date**.
2. Under **Payments to deposit**, tick each payment you took to the bank, or press **Select all**. A payment is always deposited whole.
3. The **Deposit total** shows below. It should match the deposit on your bank statement.
4. Press **Save & New** (`Alt+N`) or **Save & Close** (`Alt+S`).

If you imported a bank file, **Bank deposits waiting** on the right lists the deposits the bank reported. Click one and ClientLedger ticks the payments that add up to it. "Bank deposit … ✓" means the totals match.

## Other deposits

Use the **Other deposits** lines for money that isn't a customer payment, like a refund or a loan. Pick the account and type the amount.

- **Fees.** If the bank or card processor took a fee out of the deposit, add a line for it with a minus sign, for example `-12.50`, on an account such as Bank and Merchant Fees.
- **Cash back.** If you kept some cash from the deposit, choose where it goes under **Cash back** and type the amount. Cash back must be less than the rest of the deposit.

## Refund from a vendor

Press **Refund from a vendor** when a vendor paid you back.

- **Something I already paid for** adds a line that takes the refund off what you spent. Pick the vendor and type the amount.
- **A vendor credit or overpayment** uses up a credit the vendor owes you.

## Changing or voiding a deposit

Use **Prev** and **Next** (`Page Up`, `Page Down`) to look at earlier deposits, or press `Ctrl+E` on a deposit in a register. **Summary** shows what went into a saved deposit.

A deposit that took payments from Payments to deposit can't be changed: void it and make it again. When you void it, its payments go back to the list of payments waiting to be deposited.
