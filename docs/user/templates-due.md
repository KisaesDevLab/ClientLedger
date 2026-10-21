# Templates Due

When you open your company, at midnight and every hour while it is open, ClientLedger checks your scheduled [templates](templates.md). Templates set to **Enter automatically** are entered for every date that has come due, each date exactly once, even if the app was closed for a while. Then the **Templates Due** window opens if there is something new to show. While some still need you, the status bar says "2 templates need you"; click it to open the window.

The window has up to four lists:

- **Entered for you**: what was just entered automatically. **Open** shows it in its form.
- **Waiting for you**: reminders that are due, one row per date.
  - **Enter now** enters it with the template's amounts on that date.
  - **Open in form** opens it filled in, so you can check or change it before you save.
  - **Skip this one** leaves that date out; the next date stays scheduled.
- **Couldn't be entered**: dates that were due but not entered, with the reason in plain words. **Enter it now** opens the form for that date; **Dismiss** leaves the date out.
- A template that couldn't be checked says "we'll try again" and offers **Copy details** for a support request.

**Check now** looks again right away. Use the arrow keys to move, `Enter` for the row's main button and `Alt+O` to open it.

## Closed periods

A template is never entered automatically on or before your closing date. That date shows under **Couldn't be entered** with "This date is in a closed period." Choose **Enter it now**: the form opens on that date, and when you save, ClientLedger asks for the closing-date password, just as for any change to a closed period.

## Many dates at once

If a template has been due many times, for example a daily template after a long break, ClientLedger enters up to 31 dates of it at a time and keeps going until it is caught up. While it is still working through them, the row says "Many dates were due at once. The rest will be entered on the next run; nothing to do here." That row has no Skip or Dismiss, because there is nothing for you to do.
