ClientLedger PREVIEW build
==========================

Version {version}, built from commit {commit} for {target}.

PLEASE READ FIRST
-----------------
* This is an early PREVIEW for testing. It is not finished and it is not
  signed. Do NOT keep your real books in it. Use made-up companies or the
  practice company only. A later version may not be able to open what you
  create here.
* It works without an internet connection. It sends nothing anywhere: no
  usage statistics, no crash reports.
* There is no installer yet. Everything the app needs is in this folder:
  the app itself, its books engine (engine/) and its own copy of the
  PostgreSQL database (pg/). Keep the folder together.

HOW TO RUN IT
-------------
Windows
  1. Unzip the whole folder somewhere you can write to, for example your
     Desktop or Documents (not inside the zip viewer).
  2. Double-click clientledger.exe.
  3. Windows may say "Windows protected your PC" because the preview is not
     signed. Click "More info", then "Run anyway". The same may happen once
     for the engine and the database programs.

macOS
  1. Unzip the folder, for example into your Downloads folder.
  2. macOS blocks programs from unknown developers that were downloaded.
     Open Terminal and run (drag the folder onto the Terminal window to type
     its path):
         xattr -dr com.apple.quarantine <the unzipped folder>
  3. Then start the app from Terminal:
         <the unzipped folder>/clientledger
     If macOS still says the app "cannot be opened", open System Settings >
     Privacy & Security, scroll down and click "Open Anyway".

Linux
  1. Unpack: tar -xzf <the .tar.gz file>
  2. Run ./clientledger inside the unpacked folder. The app needs a desktop
     session (X11 or Wayland) and the usual graphics libraries (Vulkan or
     OpenGL, libxkbcommon).

THE PRACTICE COMPANY
--------------------
The practice company is a made-up business built into the app. Two ways
to use it:

  - Start the app normally and click "Try the practice company" on the
    Open Company screen. It is built in the bundled database the first
    time (about a minute) and then works like a real company file: what
    you save is kept between launches. If it says the practice company is
    not set up yet, wait a few seconds for the database to start and click
    again.
  - Or run the app with --demo for a quick look without the database.
    Nothing you change is kept after you close the app.

WHERE YOUR TEST DATA GOES
-------------------------
Companies, settings, backups and logs are stored in:
  Windows: %LOCALAPPDATA%\ClientLedger
  macOS:   ~/Library/Application Support/ClientLedger
  Linux:   ~/.local/share/clientledger (or $XDG_DATA_HOME/clientledger)
The database password is kept in your system's password store (Windows
Credential Manager, macOS Keychain, or the Secret Service on Linux).

To keep a test run completely separate, start the app with
  --data-dir <an empty folder of your choice>
(the folder must be a full path, for example C:\ClientLedgerTest).
To remove everything, quit the app and delete the data folder.

REPORTING PROBLEMS
------------------
Open an issue at https://github.com/KisaesDevLab/ClientLedger/issues and
include the version and commit shown at the top of this file, your operating
system, and what you did. When the app shows an error, "Copy details" copies
a short error ID and log lines that contain no amounts, names or memo text;
paste them into the issue. Please do not attach real business data.

LICENSES
--------
ClientLedger's own code is MIT licensed (LICENSE). The books engine is the
Vibe MyBooks engine under the PolyForm Internal Use License 1.0.0 (see
NOTICE.md). Fonts and bundled programs, including PostgreSQL, are listed with
their licenses in ATTRIBUTION.md.

ClientLedger is not affiliated with Intuit or QuickBooks.
