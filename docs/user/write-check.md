# Write Check

Use Write Check to record a check you wrote by hand, or one you want to print later. Open it from **Vendors > Write Check** or press `Ctrl+W`.

## Writing a check

1. Pick the **Bank account** the check comes from. The ending balance next to it is the balance your books report for that account.
2. Fill in **Pay to the order of**. Start typing and pick a name from the list. Press `Ctrl+L` to see the whole list.
3. ClientLedger proposes the next check **No.** You can type a different number.
4. Enter the **Amount**. It is written out in words below, like on a paper check.
5. On the **Expenses** lines, choose the account for each part of the check and enter its amount. You can also add a memo, a customer (tick **Billable** to bill them later) and a tag on each line.
6. Press **Save & New** (`Alt+N`) to write another check, or **Save & Close** (`Alt+S`).

**The lines must add up to the check amount.** Below the lines you see how much is left to assign, or that the lines match. ClientLedger won't save the check until they match to the cent.

When you have typed the check amount and pick the account on a line with no amount, the line gets what is left to assign: the whole check amount on the first line, then whatever the earlier lines leave. Type over it to change it, or press `Tab` to keep it. ClientLedger never changes an amount you typed.

When you pick a name you have paid before, ClientLedger copies the accounts, memos and tags from your last check to them. It never copies an amount. If you typed the check amount first, the first copied line gets what is left of it; the other amounts stay blank for you to fill in.

## Print later

Tick **Print later** if you will print the check from ClientLedger instead of writing it by hand. The number shows **To print**: the check gets its number when you print it. To print one check right away, click **Print** instead; you see a preview first. To reprint a check that was lost or spoiled, open it and click **Reprint check**. See [Printing checks](printing-checks.md).

## Open bills

If you have an unpaid bill from the same vendor, the panel on the right says so, for example: "You have an open bill from this vendor for 1,286.40, due 11/05." If this check pays that bill, use Pay Bills instead, so the bill shows as paid. Click **Go to Pay Bills** in the note to open it (see [Pay Bills](pay-bills.md)). The note doesn't stop you from saving the check.

## Account suggestions

A line with no account may get a suggested account, shown in grey with a note saying where it came from (one of your rules, or AI).

- **A suggestion is never used until you accept it.** Press `Tab` or click **Accept**. Anything else ignores it.
- Suggestions only fill in the account. Every amount comes from you.
- **AI is off until you turn it on** in **Edit > Preferences > AI suggestions**. You can use a model on this computer (Ollama) or your own Anthropic or OpenAI key. Untick **Bank Feed suggestions** there to stop AI account suggestions.

## Changing a saved check

Use **Prev** and **Next** (`Page Up` / `Page Down`) to move through your checks. If you have unsaved changes, ClientLedger asks whether to save them first.

- If the check is dated on or before your **closing date**, you need the closing-date password to change it.
- If the check was already **printed** and you change what is printed on it, ClientLedger asks whether to **Reprint** it or **Keep number** (keep it as a handwritten check with the same number).
- To cancel a check, use **Edit > Void Transaction**. The check stays in your books with a zero amount, and its number can be used again.
