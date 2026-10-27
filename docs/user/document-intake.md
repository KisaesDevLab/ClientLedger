# Document intake folders

Intake folders are the folders where your scanner, phone or email saves bills and receipts. ClientLedger watches them and lists what it finds in **Vendors > Receipt Inbox**, ready to turn into a bill or attach to a record.

Each company has its own list of folders. Switch companies and you watch that company's folders.

- The list is saved with the company and is included in its backups.
- It is left out of transfer files (the file you give your accountant), so your folder paths are not shared.

## Who can change them

Only an Admin can add, rename, remove or test folders. Anyone else sees the list but can't change it, and is told to ask an Admin. For now, the person who owns the company file is the Admin.

## Adding a folder

1. Open **Edit > Preferences** (`Ctrl+,`) and choose **Document intake**.
2. Choose **Add folder**.
3. Type a **Name**, and type or paste the full path in **Folder**. There is no Browse button yet.
4. Pick a **Kind** and choose **Add**.
5. Choose **Test**, then **Save**.

A company can have up to 20 folders. Names must be different, and each folder can be in the list only once.

You can use:

- **A folder on this computer**, such as `C:\Scans\Bills`.
- **A mapped drive**, such as `Z:\Bills`.
- **A network share**, such as `\\server\share\Bills`.
- **A cloud-sync folder** from OneDrive, Dropbox or Google Drive for desktop. In your sync app, choose **Always keep on this device** for the folder. Files stored only online can't be read, and the inbox lists them as "Stored online only".

ClientLedger only opens folders on your computer or network. It does not log in to anything or connect to the internet to read them.

### Folders that are refused

ClientLedger tells you why when you type one of these:

- A path that doesn't start with a drive letter (`C:\`), `\\server` or `/`.
- Special Windows paths that start with `\\?\` or `\\.\`.
- A web address, or a web folder (WebDAV). Map the share as a drive instead.
- A path containing `..`. Spell the folder out in full.
- A whole drive. Pick a folder inside it.
- One of ClientLedger's own folders.
- A path that points to a file, not a folder.
- A path longer than 1,024 characters.
- A folder another company on this computer already uses.

## Test

**Test** checks that this computer can open the folder. It tells you how many files would come in, and flags:

- files over 10 MB,
- iPhone photos (HEIC),
- files stored only online,
- a folder where handled files can't be moved (no permission),
- a folder that looks like a cloud folder.

Test also checks that ClientLedger can move handled files into `Processed`. It does this by making and removing a small hidden test file in the folder. If you turn on **Leave originals where they are**, it skips this check.

## Set this company's folder for this computer

Saving the list doesn't start watching a folder on every computer. Each computer must first confirm the folder. That's because a path means different things on different computers: `Z:` might be a different drive, or not exist at all.

After you choose **Test** and it works, choose **Use this folder on this computer** (its tip reads "Set this company's folder for this computer"), then **Save**. The folder's status changes to "Set up on this computer".

You will need to do this:

- the first time you add a folder,
- after you restore a backup on a different computer,
- after you restore an older backup whose folder path was different,
- when a folder's path was changed on another computer.

Restoring a backup on the same computer keeps your confirmations, so the folders keep working.

If the folder already holds files, ClientLedger brings them in, oldest first. If there are more than 50, it asks first:

- **Bring in N files now** copies them all.
- **Only files added from now on** leaves the older ones alone. They show in the inbox as "Older file, not copied in", with a **Bring in** button.

## Kind

**Kind** says what new files in this folder are by default: **Bills**, **Receipts**, **Vendor statements**, or **Let ClientLedger decide**. Files in a **Vendor statements** folder open in Match Vendor Statement, where ClientLedger reads the lines for you to check (see "Match a vendor statement"). You can still choose any action for a file in the inbox.

## Leave originals where they are

Normally, when you finish with a file, the original moves into a subfolder of its intake folder (see "Where originals go"). Turn on **Leave originals where they are** if the folder is shared with other people or programs, or if ClientLedger isn't allowed to move files there. Originals then stay put and ClientLedger remembers they're handled.

## Read new documents automatically

This switch is saved with the folder, but reading doesn't work yet. It comes in a later update. Until then, new documents wait in the inbox for you.

## Folder health

When a folder has a problem, its row in Preferences, a label in the inbox, and a line on Home ("N intake folders need attention") say so. Here is what each message means.

| Message | What to do |
| --- | --- |
| Set this company's folder for this computer | Open **Edit > Preferences > Document intake**, choose **Test**, then **Use this folder on this computer** and **Save**. |
| Can't find the folder (drive unplugged, renamed, or sync paused) | Plug the drive in, check the folder wasn't renamed or moved, or resume your sync app. Then choose **Test**. |
| Can't reach the network folder | Check you are on the right network and the other computer or server is on. Then choose **Test**. |
| Not responding (still trying) | The folder is slow to answer. Wait, or check the network. |
| ClientLedger isn't allowed to read this folder | Ask whoever manages the computer or share to give you read access. |
| Handled files can't be moved to Processed (no permission) | Ask for write access, or turn on **Leave originals where they are**. Files stay where they are meanwhile. |
| This folder now points somewhere else | The drive letter or link now leads to a different place. Check it, then choose **Use this folder on this computer** again. |
| Another company on this computer already uses this folder | Two companies can't share one folder on the same computer. Change this company's folder. |
| This folder is slow to read | Information only. |
| This folder holds too many files: move handled files out | Move old files out of the folder. |

A problem is shown only after it happens twice in a row, so a brief sync pause doesn't raise an alarm. ClientLedger keeps trying again on its own.

## What files are taken

ClientLedger takes **PDF, JPG, PNG and TIFF** files. TIFF previews are not ready yet.

- **iPhone photos (HEIC)** are not taken. Set the phone to save as JPG or PDF. The inbox shows "Save as JPG or PDF instead".
- Files over **10 MB** are not taken. The inbox shows "Too large (10 MB limit)". Save a smaller copy.
- Hidden files, temporary files (such as `.tmp` or `.part`), `Thumbs.db`, `desktop.ini` and empty files are ignored.
- Shortcuts and links are skipped.

## When a file counts as arrived

ClientLedger looks in each folder about every 8 seconds. A file counts as arrived only after its size and date have stayed the same for at least 5 seconds and the file can be opened. A file still being saved shows "Still arriving". Then ClientLedger copies it into the company's attachments, and it shows as ready. This usually takes under 15 seconds.

## Where originals go

ClientLedger never deletes your originals. When you finish with a file, it moves the original inside the same intake folder:

- **Processed/2026-10/** (year and month) after you attach it to a saved record.
- **Discarded/** after you choose **Discard**.

Nothing is overwritten. If a name is already there, the new file gets `(2)` added. Don't put new files in these subfolders; ClientLedger doesn't watch them.

If you turned on **Leave originals where they are**, nothing moves.

## Removing a folder

Choose **Remove** next to the folder. Files in it stay where they are.
