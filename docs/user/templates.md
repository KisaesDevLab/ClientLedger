# Templates

A template is a form you fill in once and reuse: the rent check, the monthly software charge, the owner's insurance entry. ClientLedger can remind you when one is due, or enter it for you.

## Save as Template

On Write Check, Make Deposit, Transfer Funds, Record Card Charge, Make Journal Entry or Daily Sales Summary, fill in the form as usual and choose **Save as Template** (`Ctrl+M`). The template keeps everything except the date and the check or entry number; those are filled in each time it is used.

In the Save as Template window:

1. Give it a **Name** and, if you like, a **Group**. A group is just a folder, such as "Monthly bills"; choose **New group…** to make one.
2. Under **On its dates**, choose what happens:
   - **Remind me**: it is listed in Templates Due on each date, for you to enter.
   - **Enter automatically**: ClientLedger enters it on each date by itself.
   - **Don't remind me**: no schedule; you use it whenever you like.
3. For Remind me and Enter automatically, set **How often** (with **Every** for "every 2 weeks" and so on), the **Next date**, and when it **Ends**: never, on a date, or after a number of times. The window shows the next five dates. A monthly template on the 29th, 30th or 31st uses the last day of shorter months.
4. Press **Save**.

A Daily Sales Summary template can remind you but can't be entered automatically, because its amounts change every day. A deposit that includes customer payments can't be a template. An automatic schedule can't start more than about a year back.

## The Templates list

Open **Lists > Templates**. Templates are listed under their groups with how often they run, the next date, the mode and the amount.

- **Use** (or `Enter`) opens the template's form, filled in, dated today. Change anything, then save. If something on the template is no longer active, the form says so and leaves that field empty.
- **Enter now** enters it at once on its next date. A template without a schedule asks which date to use. (A Daily Sales Summary template is entered with **Use**, since its amounts change.)
- **Edit** changes the name, group and schedule.
- **Delete** removes the template; transactions already entered from it stay.
- **New group** adds a group.

To change what a template records, choose **Use**, change the form, then **Save as Template** and choose **Replace**. If someone changed the template in another window meanwhile, ClientLedger tells you instead of overwriting it.

Schedules made in MyBooks Online are listed but can only be deleted here.

See [Templates Due](templates-due.md) for what happens when templates come due.
