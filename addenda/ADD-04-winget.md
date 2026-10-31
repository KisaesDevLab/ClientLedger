# ADD-04 — Distribution through winget

**Status:** Active (added 2026-10-31, during the build) · **Part of the beta** · No A-phases of its own: built inside P35/P36 and switched on at P39 stage 2 · Decisions Q-317–Q-323 (amends Q-134)

Owner request, 2026-10-31: "Plan for distribution using winget". The owner answered a Q&A the same day (Q-317–Q-323).

ClientLedger becomes installable with the Windows Package Manager: `winget install KisaesLLC.ClientLedger`, upgraded with `winget upgrade`. Winget never rehosts files: the manifest in `microsoft/winget-pkgs` points at Kisaes LLC's own GitHub Release asset by URL and SHA-256 (Q-121), so only Kisaes LLC distributes ClientLedger and its engine (Q-31, Q-133, Q-303, Q-316). `winget upgrade` is started by the user, so the no-phone-home rule holds (non-negotiable 3).

This addendum adds work to phases not built yet. It never rewrites completed phases; where it changes planned behavior, it says so below.

## When to run it (scheduling rule)

1. **P35 (packaging)** builds the MSI requirements, the release workflow's winget job (off by default) and the manifest files below.
2. **P36 (updates and migrations)** adds the winget upgrade hint, the silent-upgrade test, and (with P33) the LAN version message.
3. **P39 stage 2 (public pre-release, Q-75)**, after the owner's sign-off: the first winget submission is made by hand, then the owner turns on `WINGET_ENABLED`; later releases go to winget automatically. Stage-1 beta testers install from the GitHub release. Preview builds never go to winget (Q-253).

## What changes for planned work

| Phase | Was | Becomes |
| --- | --- | --- |
| P35 step 2 (Windows installer) | MSI, per-user **and** per-machine | MSI, **per-machine only** (Q-318): Program Files, all users, needs admin; company data stays per user in `%LOCALAPPDATA%\ClientLedger` |
| P35 (license acceptance, Q-134) | Shown by the installer | Shown by the installer UI; silent installs (winget) carry it as winget **Agreements**, and the app shows it once on first start when the installer didn't record acceptance (Q-320) |
| P35 release workflow | Build, checksums, GitHub Release | Also: draft releases (published by the owner), immutable assets, and a winget job on `release: published` gated by the repository variable `WINGET_ENABLED` (Q-322) |
| P36 update check | Manual check against GitHub Releases | Same; when installed by winget, the "new version" message also shows `winget upgrade KisaesLLC.ClientLedger` |
| P33/P36 version check (PR-9) | Host and workstations must match | A mismatched workstation can't connect and shows the host's version and `winget install KisaesLLC.ClientLedger --version <x.y.z>` (Q-323) |
| P38 docs | Install from the download | Adds "Install with winget" to the user guide and README |
| P39 checklist | Publish GitHub Release, ClientLedger.net link | Adds: first manual winget submission; turn on `WINGET_ENABLED` |

## Fixed identifiers (never change)

- **MSI UpgradeCode:** `{5871208F-8AA3-4EC1-86A7-ABCF603A0765}`
- **Winget PackageIdentifier:** `KisaesLLC.ClientLedger` (Q-321); Publisher "Kisaes LLC"; Moniker `clientledger`.

## MSI requirements (P35: `packaging/windows/wix/*`, `xtask/src/package.rs`)

- Per-machine install to `C:\Program Files\ClientLedger\`, with the preview archive's layout (`xtask/src/preview.rs`): `clientledger.exe`, `clientledger-cli.exe`, `clientledger-docread.exe`, `engine\`, `pg\`, `pdfium\`, `ocr\`, LICENSE, NOTICE.md, ATTRIBUTION.md. The app already finds these next to its executable (`cl_host::find_engine_exe`, `cl_pg::PgBinaries::discover`, `find_ocr_dir`).
- `MajorUpgrade` with the UpgradeCode above; downgrades refused; ProductVersion from the workspace version (SemVer 0.x, Q-46).
- Silent install works: `msiexec /i ClientLedger-<v>-x64.msi /quiet /norestart`. The license dialog sets a public property (e.g. `LICENSEACCEPTED=1`) written to HKLM; the app reads it.
- Before replacing files, the installer stops a running ClientLedger cleanly through the single-instance endpoint (the `clientledger-cli host stop` path), with Restart Manager as the fallback, so an open company's database shuts down properly.
- Uninstall removes program files only; `%LOCALAPPDATA%\ClientLedger` is kept.
- Runtime check: confirm whether the bundled PostgreSQL, PDFium and tesseract need the VC++ runtime. Preferred: ship its DLLs app-local so the install works offline; otherwise declare `Microsoft.VCRedist.2015+.x64` as a winget dependency and bundle it in the MSI for non-winget installs.
- Signing runs only when signing secrets exist (Q-27). Unsigned for now (Q-319).

## Release workflow (P35: `.github/workflows/release.yml`)

1. A tag `v0.*` on `main` builds the MSI on `windows-latest` (`cargo xtask package`) and the other platforms (P35), writes SHA-256 checksums, and creates a **draft** GitHub Release. Nothing is public until the owner publishes it (P39 STOP).
2. On `release: published`, a Windows job runs only when the repository variable `WINGET_ENABLED` is `true`:
   `wingetcreate update KisaesLLC.ClientLedger --version <v> --urls <msi url> --submit --token ${{ secrets.WINGET_TOKEN }}`.
   `WINGET_TOKEN` is a fine-grained token for a fork of `microsoft/winget-pkgs`. Pull-request CI stays secret-free (non-negotiable 10).
3. Release assets are never re-uploaded under the same version (winget pins the SHA-256); a fix is a new version.

## Winget manifest (P35: `packaging/winget/`, schema 1.9, for the first manual submission)

- `KisaesLLC.ClientLedger.yaml` (version), `KisaesLLC.ClientLedger.installer.yaml`, `KisaesLLC.ClientLedger.locale.en-US.yaml`.
- **Installer:** `InstallerType: wix`, `Scope: machine`, `Architecture: x64`, `InstallerUrl` (GitHub Release asset), `InstallerSha256`, `ProductCode`, `AppsAndFeaturesEntries` (UpgradeCode, DisplayName ClientLedger, Publisher Kisaes LLC), `MinimumOSVersion: 10.0.19045.0` (Q-28), `ElevationRequirement: elevatesSelf`, `InstallModes: [interactive, silent, silentWithProgress]`, `UpgradeBehavior: install`, `Dependencies` only if the runtime check needs it.
- **Locale:** `PackageName: ClientLedger`, `Publisher: Kisaes LLC`, `PublisherUrl` (ClientLedger.net), `ShortDescription`, `Moniker: clientledger`, `Tags` (bookkeeping, accounting, small-business, offline), `License: PolyForm Internal Use 1.0.0`, `LicenseUrl` (repository LICENSE), `Agreements` (the internal-use terms and the engine's commercial license included with ClientLedger; `AgreementUrl` → NOTICE.md), `ReleaseNotesUrl` (the GitHub release), `Copyright: Copyright 2026 Kisaes LLC dba KisaesDevLab`.
- First version: validated with `winget validate`, submitted by hand (`wingetcreate new` or the files above) at P39 stage 2. Later versions: automatic (Q-322).

## Decisions (added to QUESTIONS.md)

| ID | Topic | Decision |
| --- | --- | --- |
| Q-317 | When ClientLedger appears in winget | Owner 2026-10-31: first submission at P39 stage 2 (public pre-release, Q-75), after the owner's sign-off; previews never (Q-253). |
| Q-318 | Install scope | Owner 2026-10-31: per-machine only (Program Files, all users, admin); amends P35 step 2's per-user and per-machine MSI. |
| Q-319 | Signing for winget | Owner 2026-10-31: unsigned for now (Q-27); if Microsoft's winget validation rejects or holds the unsigned MSI, set up Azure Trusted Signing and resubmit signed. |
| Q-320 | License in silent installs | Owner 2026-10-31: winget shows the license and Agreements (accepted with `--accept-package-agreements`); the app shows the license once on first start when the installer didn't record acceptance. Amends Q-134. |
| Q-321 | Winget package identifier | Owner 2026-10-31: `KisaesLLC.ClientLedger` (permanent); Publisher Kisaes LLC; moniker `clientledger`. |
| Q-322 | Submitting new versions | Owner 2026-10-31: automatic PR to `microsoft/winget-pkgs` from each published GitHub release (gated by `WINGET_ENABLED`); the first version by hand. |
| Q-323 | LAN version mismatch | Owner 2026-10-31: a workstation on another version than the host can't connect and shows the host's version and the exact `winget install … --version` command. |

## Verification (P35/P36 acceptance additions)

- `winget validate packaging/winget/` passes.
- In Windows Sandbox with networking off and `LocalManifestFiles` enabled: `winget install --manifest packaging/winget --accept-package-agreements` installs per-machine silently; the app starts, shows the license once, opens the practice company; `winget list` shows `KisaesLLC.ClientLedger` at the right version.
- A second manifest with a higher version: `winget upgrade` stops the running app, upgrades silently, and the next company open backs up and migrates (non-negotiable 9, P36).
- `winget uninstall` removes program files and keeps `%LOCALAPPDATA%\ClientLedger`.
- Release workflow dry run on a test tag and a fork: draft release with the MSI and SHA-256; no winget PR while `WINGET_ENABLED` is off; with it on (fork target), `wingetcreate` opens a correct PR.
- LAN: a mismatched workstation shows the host version and the winget command.

## Notes for later phases

- **P35:** the items above; record any runtime-dependency finding in ATTRIBUTION.md.
- **P36:** winget upgrade hint; silent-upgrade test.
- **P38:** "Install with winget" in the user guide and README.
- **P39:** first manual submission, then `WINGET_ENABLED=true`; if validation flags the unsigned MSI, Q-319 applies.
