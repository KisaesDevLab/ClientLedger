# Use ClientLedger on more than one computer

ClientLedger can share one company's books with other computers in your office. One computer, the **host**, keeps the books. The others use the host's books over your office network. Nothing goes over the internet.

## On the host computer

1. Open **Company > Office Network**.
2. Choose **Share my books with other computers (this is the host)**.
3. Pick the network connection to share on. Only private office networks are offered.
4. Press **Start sharing**, then restart ClientLedger. Sharing starts after the restart, and the pairing code shows then.

While you share, automatic sign-in is off. Everyone signs in with their own user name and password, so set up a user for each person first.

To keep sharing after you close ClientLedger, tick **Keep sharing after I close ClientLedger, and start sharing when I sign in to this computer**. The host has to be on, and someone has to be signed in to it, for the other computers to work.

If the other computers can't find the host, allow ClientLedger through the host's firewall for private networks.

## Adding another computer

1. On the host, press **Show a pairing code**. The code works for 10 minutes.
2. On the other computer, open ClientLedger and choose **Open a company on my office network** on the first screen (no account or company is needed on that computer), or open **Company > Office Network**. Choose **Use the books on another computer in the office**.
3. Pick the host from the list, or type its address. The host's Office Network window shows the address.
4. Type the pairing code and press **Pair with the host**.
5. Back on the host, check the name and press **Allow**.
6. Restart ClientLedger on the other computer, then sign in with your own user name and password from the host.

If the code is mistyped, the other computer says the code doesn't match. Type it again from the host's screen.

To stop a computer from using the books, press **Remove** next to it on the host. That computer then says it was removed; to use the books again, pair it again with a new code.

## Working together

- **Two people change the same thing.** If someone saved it while you were editing, ClientLedger shows what each of you changed:
  - **Keep mine** saves your changes on top of theirs, and lists anything of theirs it would undo.
  - **Use theirs** shows their version.
  - **Back** returns to your edit without saving.
- **The host stops answering.** Your open windows stay as you left them, and nothing is sent on its own. A save you press while the host is down is not sent and is not kept to send later. When the host is back, check each window and save it again.
- **The host is restoring or importing.** A banner says so. You can still look around, but saving waits until it's done. The host signs everyone else out while it works; afterwards a banner offers **Sign in again**.
- **Some things are done only on the host:** backing up, restoring, creating or deleting a company, imports, payroll import, and sending books to your accountant. The host makes a backup first.
- **AI and email settings** are set on the host. They can't be changed from the other computers.

All computers need the same version of ClientLedger. If they don't match, the other computers say so and won't save until you update them.
