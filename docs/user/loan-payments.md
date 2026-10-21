# Loan payments

Use Loan Payment to record a payment on a loan, splitting it into interest and the part that pays down the loan (the principal). Open it from **Banking > Loan Payment**.

ClientLedger doesn't build payment schedules. Have your lender's statement ready and type the amounts from it.

1. Pick the **Loan account** and the **Lender**. The accounts from your last payment to this lender are filled in; the amounts never are.
2. Pick **Pay from** and how you paid: **Check** or **Online or automatic**.
3. Type the **Payment** and the **Interest** from the statement. **Principal** is worked out for you: the payment less the interest and any extra lines.
4. Add **Fees, escrow and other extra lines** if the statement shows them.
5. Optional: type the **Lender's balance after this payment** from the statement. After you save, ClientLedger compares it with the loan balance in your books and shows "Matches your lender's statement ✓", or the difference.
6. Press **Save & New** (`Alt+N`) or **Save & Close** (`Alt+S`).

If the interest and extra lines are more than the payment, ClientLedger won't save. Check the amounts on the statement.
