# Receipt inbox

The receipt inbox collects the photos and PDFs your phone, scanner or email saves into your intake folders, so you can turn each one into a bill or check, or attach it to a record. Open it from **Vendors > Receipt Inbox**. The menu shows how many files are waiting.

## Your folders

The inbox lists the files from all of the company's intake folders. To add or change folders, choose **Set up folders...** in the inbox, or open **Edit > Preferences > Document intake**. See [Document intake folders](document-intake.md) for how.

Use **Folder** at the top to see one folder or **All folders**. A folder with a problem shows a label with the reason. Choose **Test** on it to look again, or **Set up...** if this computer hasn't confirmed it yet.

If you had a receipt inbox folder from an earlier version, nothing was lost and nothing moved. It became your first intake folder, named **Receipts**. Files that were already filed stay in the `Filed` folder. The inbox doesn't list them.

## What you see

The inbox lists PDF, PNG, JPEG and TIFF files. Choose **Check now** to look again. Each file shows its state:

- **Ready**: you can use it.
- **Still arriving**: the file is still being saved. Wait a moment.
- **Copying in** and **Copied in**: ClientLedger is copying it into the company.
- **On an open form**: you opened a form with this file. Save that form to file it, or close the form to use the file here again.
- **Same as a file copied before**: see "Duplicates" below.
- **Too large (10 MB limit)**: save a smaller copy.
- **Save as JPG or PDF instead**: iPhone photos (HEIC) can't be used. Set the phone to save JPG or PDF.
- **Stored online only**: your sync app hasn't downloaded the file to this computer. Open the sync app and keep the file on this computer.
- **Older file, not copied in**: it was in the folder before you chose "Only files added from now on". Choose **Bring in** to use it.

If the folders hold more files than fit in the list, the inbox says how many more there are.

## Using a file

Pick a file to preview it. Multi-page TIFF files show page by page; a few kinds of TIFF can't be shown (see Document viewer). Then choose one of:

- **New bill**: opens Enter Bill with the file attached.
- **Add to a bill...**: attaches it to a bill you already entered.
- **New check**: opens Write Check with the file attached.
- **Add to a check...**: attaches it to a check you already wrote.
- **Add to a bank transaction...**: attaches it to a deposit, payment or other bank or card transaction.
- **Discard**: you don't need the file. After you confirm, the original moves to the `Discarded` folder inside its intake folder, and ClientLedger's copy is removed from the company. Nothing is deleted from your folder.

For the "Add to" choices, find the record in the list and choose **Attach**.

## Duplicates

If a file is exactly the same as one that came in before, the inbox flags it once ("Same as a file copied on" and the date) and does not copy it in. Choose **Discard** to put it away, or **Copy anyway** to copy it in as usual.

## Where files go

Files leave the inbox only after you finish with them. The original then moves inside its intake folder:

- into `Processed/` in a subfolder for the year and month, like `Processed/2026-10/`, once the bill, check or record it is attached to is saved,
- into `Discarded/` if you discard it.

If the folder is set to **Leave originals where they are**, the original stays and the inbox no longer lists it.

- While a new bill or check is unsaved, the file stays in the inbox. It moves when you save.
- Nothing is deleted, and nothing is overwritten.
- If a file changes while it's being attached, it isn't moved.
- If ClientLedger can't confirm that a file was attached, the file stays in the inbox. Check the record before trying again.
- If a file can't be moved yet, it stays where it is and ClientLedger tries again shortly.
