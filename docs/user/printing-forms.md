# Printing forms

Use this page to print invoices, sales receipts, credit memos and deposit summaries, and to set up your logo, footer text and numbering. For printing checks, see [Printing checks](printing-checks.md).

## Set up what prints (once)

Open **Company > Company Information**. This is what prints on your invoices, receipts, statements and checks.

### Logo

1. Click **Browse...** and choose a PNG or JPEG picture up to 2 MB.
2. You see a preview, and "Logo saved."

The logo prints on forms. It prints on checks only if you tick **Print company name and logo on this account's checks** in **File > Printer Setup**.

If a logo can't be used (for example a WebP picture, a file over 2 MB, or one that won't open), forms print without it and you see "Your logo can't be printed, so forms print without it." Choose a PNG or JPEG.

**Remove** stops ClientLedger from printing the logo on this computer until you choose a new one. The old picture stays stored in the books.

### Footer on forms

The footer is a line at the bottom of each form, such as payment instructions or a thank-you. It can be up to 1,000 characters.

- **Company footer** is used on every form.
- Each form type (invoice, sales receipt, credit memo, statement) has its own optional box, "Footer for Invoice (optional)" and so on. Leave it blank to use the company footer.

### Numbering

Under **Numbering**, each form type has a **Prefix** (no spaces, up to 20 characters, such as INV-) and a **Next number** (1 to 999,999,999). **Next form gets** shows what the next form will look like, for example INV-1043.

If another form already has that number, you see a warning, for example "1042 is already the number of another Invoice." Two forms with the same number are hard to tell apart, so choose a higher next number.

Click **Save** when you're done.

## Print forms together

You can print one form from its own screen, or mark forms **Print later** and print them together.

1. Open **File > Print Forms**.
2. Click the tab **Invoices, receipts and credit memos**.
3. Tick the forms to print. Each row shows the type, number, date, customer and amount.
4. Click **Preview 3 forms** (the number is how many you ticked).
5. In the preview, set **Copies** (starts at 1), choose the **Printer** from the list, and click **Print**.

ClientLedger remembers the last printer you used for forms on this computer.

If nothing is waiting, the tab says "No invoices, sales receipts or credit memos are marked Print later."

### Other buttons in the preview

- **Save PDF...** saves the forms as a file. It suggests the folder Documents\ClientLedger and your company name, with a name like `Invoice 1042 - Acme.pdf`. After that it remembers the last folder you used.
- **Envelopes** prints #10 envelopes with your company's return address and the customer's address, for the forms you ticked. The envelopes go through your PDF app, which sets the envelope size.
- **Back to the list** returns without printing.

The bill-to address on forms sits where it shows through a #10 double-window envelope.

## Print a deposit summary

On **Make Deposit**, click **Print**. A new or changed deposit is saved first. You see a preview, then click **Print**. You can also open a saved deposit from a register and print its summary the same way.

## Printing on Windows

On Windows, ClientLedger sends forms straight to the printer you chose, on US Letter paper, with your copies. If that doesn't work, the PDF opens on your screen so you can print it from there. If your PDF app took the file, the message says "Check that it printed."

On Mac and Linux, ClientLedger uses the system's normal print command.

## Try it on the practice company

To try printing without touching your real books, choose **File > Open Practice Company**. ClientLedger closes your current company (asking about any unsaved windows first) and opens the practice company, building one if this computer doesn't have one yet.
