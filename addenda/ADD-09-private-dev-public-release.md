# ADDENDUM: private dev repo, public release repo (desktop apps)

Generic addendum for any Kisaes Rust/egui desktop app (ClientLedger, ClientLedger Trial Balance, Vibe Files, Vibe Filer Desktop, Vibe Notes). Drop it into the app's repo next to the existing build plan and run it as its own set of phases. It adds to the plan; it does not edit phases already built.

Revision 2, 2026-11-21: owner decisions folded in (see "Decisions").

## Fill in before running

| Placeholder | Meaning | Value for this repo (proposed) |
|---|---|---|
| `{APP}` | Product name as shown to users | ClientLedger |
| `{BIN}` | Binary / asset base name, no spaces | ClientLedger |
| `{DEV_REPO}` | Source repo (becomes private) | KisaesDevLab/ClientLedger |
| `{PUB_REPO}` | New public release repo | KisaesDevLab/clientledger-releases |
| `{PUB_DIR}` | Local checkout of the public repo | C:\Projects\clientledger-releases |
| `{SUPPORT_EMAIL}` | Address for support and source requests | (owner to supply) |

### Notes for this repo

- This is ADD-09. Phase ids R0 to R8 are local to this addendum.
- Reconcile with `addenda/ADD-04-winget.md` in R0: a winget manifest needs a public installer URL, which becomes the `{PUB_REPO}` release asset.
- `CLAUDE.md`, `PHASES.md`, `STATE.md`, `QUESTIONS.md` and `addenda/` are tracked in git today, so R7 will report them as visible to anyone given read access.

One public repo per app. A shared repo would break the "latest release" URLs, which only track one release line.

## Decisions

| Topic | Decision |
|---|---|
| Source access | Source is available on request by email to `{SUPPORT_EMAIL}`. A request is fulfilled by granting read access to the private `{DEV_REPO}`. |
| Packaging | Inno Setup installer plus a portable zip. Unsigned for now; the script signs automatically once a certificate is configured. |
| Updates | Manual only. Help > Check for updates opens the public releases page in the browser. The app makes no network call for updates. |
| Support | Public issues are open on `{PUB_REPO}`. |
| Platforms | Windows x64 only. Other platforms can be attached later with `-ExtraAssets`. |
| Website links | Sites link once to a stable URL. The release script does not touch any website. |
| Betas | Versions with a pre-release suffix (for example `0.9.0-beta.1`) publish as GitHub pre-releases and never become the "latest" download. |

## Outcome

- All source, history and build-plan files live only in `{DEV_REPO}`, which is private.
- `{PUB_REPO}` is public and contains no source: a README, licence text, changelog, issue templates, and GitHub Releases holding the installer, portable zip and checksums.
- Releases are built and published from the local machine with one script. No GitHub Actions.
- Nothing in the shipped binary or the app's UI points at the private repo.

## Rules for the agent

1. Never push source, tests, build-plan files, or git history to `{PUB_REPO}`. Its tracked files are limited to the allowlist in R1.
2. `{PUB_DIR}` is a separate sibling git repo. Not a submodule, not a subtree, not a second remote on `{DEV_REPO}`.
3. Do not change any repo's visibility, create repos on GitHub, or add collaborators. Those are owner steps (R6, R7). Prepare everything and stop.
4. Do not publish a real release without the owner running the script. Use `-DryRun` for every test.
5. Add no network call to the app. The update item only opens a browser URL.
6. Conventional commits, STATE.md updated per phase, anything undecided goes to QUESTIONS.md.

## Phases

### R0: inventory (read-only)

Record in STATE.md:

- current remote(s), default branch, and whether `{DEV_REPO}` is public today
- existing tags and any existing GitHub Releases on `{DEV_REPO}`
- how the app is packaged today, and whether an Inno Setup script already exists
- every place the source repo URL appears: `Cargo.toml` `repository`/`homepage`, About box, Help menu links, issue links, README badges, licence text, installer metadata
- any existing update-check code and what it calls
- any `.github/workflows` and what they publish
- bundled components with their own source exposure (for example a Node SEA engine, embedded SQL, embedded templates)
- build-plan and agent files that are tracked or present in git history (`CLAUDE.md`, `PHASES.md`, `STATE.md`, `QUESTIONS.md`, `KICKOFF_PROMPT.md`, addenda). These matter for R7.

**Accept:** STATE.md lists every URL occurrence with file and line, and every tracked or historical build-plan file.

### R1: scaffold the public release repo (local only)

Create `{PUB_DIR}` with `git init` and exactly these tracked files:

- `README.md`: what `{APP}` is; download links; Windows x64 requirement; how to verify the checksum; a note that the build is unsigned and Windows SmartScreen will warn ("More info" then "Run anyway"); how to request source (`{SUPPORT_EMAIL}`); where to report problems
- `LICENSE.md`: the licence that applies to the distributed binary (copy from `{DEV_REPO}`)
- `CHANGELOG.md`: user-facing, one section per version
- `SECURITY.md`: report vulnerabilities by email to `{SUPPORT_EMAIL}`, not as a public issue
- `.github/ISSUE_TEMPLATE/bug_report.yml` and `feature_request.yml`. Each template opens with a bold warning: do not attach or paste client data, tax documents, company files, backups, licence keys, or screenshots showing them. Ask for app version, Windows version, and steps to reproduce.
- `.github/ISSUE_TEMPLATE/config.yml`: blank issues disabled; a contact link to `{SUPPORT_EMAIL}` for anything involving client data
- `.gitignore`: ignore everything except the files above, so a stray copy cannot be committed

Download links in the README, because of the beta rule in R3:

- Stable: `https://github.com/{PUB_REPO}/releases/latest`
- All versions including betas: `https://github.com/{PUB_REPO}/releases`

**Accept:** `git -C {PUB_DIR} ls-files` shows only the allowlist. No file in it mentions `{DEV_REPO}`.

### R2: release build hygiene

1. `Cargo.toml` release profile: `strip = true`, `debug = false`, `lto = true` (or `"thin"` if build time matters), `codegen-units = 1`. Keep any existing `panic` setting.
2. Remove local paths from the binary. Add `--remap-path-prefix` for the project directory and the cargo home (via `.cargo/config.toml` or the release script's `RUSTFLAGS`) so `C:\Projects\...` and `C:\Users\<name>\...` do not appear in panic messages or embedded strings.
3. Do not ship `.pdb` files. Archive them locally per version under an ignored `dist/symbols/` folder for crash diagnosis.
4. Embed the version from `Cargo.toml` and the short commit hash at build time, shown in the About box. The hash is a build identifier only; it must not be rendered as a link.
5. Confirm no secret is compiled in. A licence-verification public key is fine. Private keys, tokens, and API keys are not.
6. Add `tools/check-binary.ps1` that scans the built exe and the installer payload for: the Windows username, `C:\Projects`, `{DEV_REPO}`, `github.com/KisaesDevLab/` followed by anything other than the public repo name, and common token prefixes. Non-zero exit on any hit.
7. Record in STATE.md what bundled components remain readable after packaging. A Node SEA blob, for example, can be unpacked to JavaScript; stripping the Rust binary does not change that.

**Accept:** a release build passes `tools/check-binary.ps1`.

### R3: installer and release script

**Installer.** Add `installer/{BIN}.iss` (Inno Setup) if R0 found none:

- per-user install by default, no admin prompt unless the app needs one
- Start menu shortcut, optional desktop shortcut, uninstaller
- upgrade in place over an older version; never touch user data folders on install or uninstall
- publisher "Kisaes LLC"; publisher, support and update URLs all point at `{PUB_REPO}` or the product site
- licence page showing `LICENSE.md`
- version read from `Cargo.toml`, not hard-coded

**Script.** Add `tools/release.ps1` (PowerShell, run from the repo root on the build machine).

Parameters: `-Version` (defaults to the `Cargo.toml` version), `-DryRun`, `-SkipTests`, `-ExtraAssets <dir>`.

Steps, each failing fast with a plain message:

1. Preflight: clean working tree, on the default branch, tag `v{Version}` does not exist in either repo, `gh auth status` succeeds, Inno Setup compiler found, `{PUB_DIR}` exists and is clean, `CHANGELOG.md` in `{PUB_DIR}` has a section for this version.
2. Channel: a version with a pre-release suffix (`-beta.N`, `-rc.N`) is a beta. Anything else is stable.
3. Test: run the test suite unless `-SkipTests`.
4. Build: `cargo build --release` with the R2 flags.
5. Package: compile the installer, and build the portable zip (exe, licence, readme).
6. Sign: if `KISAES_SIGN_CERT` (thumbprint or PFX path) is set, sign the exe and the installer with `signtool` and a timestamp server. Otherwise print "UNSIGNED BUILD" and continue.
7. Check: run `tools/check-binary.ps1` on the final artifacts.
8. Assets, with fixed names so stable URLs never change:
   - `{BIN}-Setup-x64.exe`
   - `{BIN}-windows-x64.zip`
   - `SHA256SUMS.txt`
   - `latest.json`: `{ "version", "channel", "released_at", "notes_url", "assets": [ { "name", "sha256", "size" } ] }`. Nothing reads this today; it is there so a future updater needs no release-format change.
9. Publish (skipped under `-DryRun`, which prints the commands instead):
   - commit and push the `CHANGELOG.md` change in `{PUB_DIR}`
   - stable: `gh release create v{Version} --repo {PUB_REPO} --latest --title "{APP} {Version}" --notes-file <changelog section> <assets>`
   - beta: same command with `--prerelease` and without `--latest`
   - tag `v{Version}` in `{DEV_REPO}` and push the tag
10. Print the release URL and the SHA-256 of each asset.

**Beta rule and the stable URL.** GitHub's "latest" excludes pre-releases. So:

- `https://github.com/{PUB_REPO}/releases/latest/download/{BIN}-Setup-x64.exe` always returns the newest **stable** installer, and returns 404 until the first stable release exists.
- While an app has only beta releases, the website and README must link to `https://github.com/{PUB_REPO}/releases` instead. Switch the website to the stable URL once, at the first stable release.

The script prints a one-line reminder of this whenever it publishes a beta and no stable release exists yet.

**Accept:** `tools/release.ps1 -DryRun` completes on a clean tree and lists the exact assets and commands for both a beta version and a stable version. Running it on a dirty tree, or with a missing changelog section, stops before building.

### R4: Check for updates (manual, no network call)

- Add Help > Check for updates (or a button in About).
- It shows the running version and opens `https://github.com/{PUB_REPO}/releases` in the default browser. That page lists stable and beta builds, so it works before and after the first stable release.
- Remove or disable any existing background or startup update check found in R0.
- The app itself sends nothing. Status-bar or privacy wording about network use stays true.

**Accept:** with networking disabled, clicking the item still opens the browser and the app logs no connection attempt of its own.

### R5: repoint every link

Using the R0 list:

- `Cargo.toml` `repository` and `homepage`: the product page or `{PUB_REPO}`
- About box and Help menu: "Report a problem" opens `https://github.com/{PUB_REPO}/issues/new/choose`; "Contact support" uses `{SUPPORT_EMAIL}`
- installer metadata (publisher URL, support URL)
- licence and README wording about source: replace any link to the source repo with "Source code is available on request: email `{SUPPORT_EMAIL}`." Do not otherwise change licence terms; put any wording that looks inconsistent with a private repo into QUESTIONS.md for the owner.
- `{DEV_REPO}` README: note that releases are published from `tools/release.ps1` to `{PUB_REPO}`

**Accept:** `tools/check-binary.ps1` passes and a repo-wide search for `{DEV_REPO}` finds only developer documentation.

### R6: owner cutover checklist (the agent prepares, the owner does)

Write this into `RELEASING.md` and stop:

1. Create `{PUB_REPO}` on GitHub as public and empty, with Issues enabled. Add it as `origin` in `{PUB_DIR}` and push.
2. Run `tools/release.ps1 -DryRun`, then run it for real.
3. From a browser that is not signed in to GitHub, download the installer and check it against `SHA256SUMS.txt`.
4. Move any existing Releases off `{DEV_REPO}`. Once it is private they stop being downloadable by the public.
5. Point the product website at `{PUB_REPO}`: the releases page while the app is beta-only, the stable URL after the first stable release.
6. Make `{DEV_REPO}` private. Expect stars and watchers to be dropped and public forks to be detached.
7. From the signed-out browser, confirm `{DEV_REPO}` returns 404 and the `{PUB_REPO}` download still works.

### R7: source-request procedure

Fulfilling a request means giving someone read access to `{DEV_REPO}`. Read access shows everything in the repo: full git history, every branch, issues, pull requests, and any build-plan file that was ever committed. So before the first grant:

1. Using the R0 list, confirm build-plan and agent files are ignored by `.gitignore` and report whether they still exist in history. Scrubbing history is a separate owner-run task; do not rewrite history in this addendum.
2. Add `tools/check-history.ps1` that scans all commits for tracked secrets patterns and for the build-plan file names, and prints what a new reader would be able to see.
3. Write `SOURCE_REQUESTS.md` in `{DEV_REPO}` with the owner procedure:
   - the request arrives at `{SUPPORT_EMAIL}`; record name, firm, GitHub username, date, and licence tier in a local log (not in the repo)
   - run `tools/check-history.ps1` and review its output
   - invite the GitHub user as a collaborator with the **Read** role
   - reply with a short note: the licence terms still apply to the source, and access is to the repo as it stands
   - how to remove access later, and when (for example on licence termination)
4. Put any policy gaps in QUESTIONS.md, including the two listed under "Still open".

**Accept:** `SOURCE_REQUESTS.md` exists, `tools/check-history.ps1` runs clean or lists exactly what is exposed, and no collaborator was added by the agent.

### R8: document

`RELEASING.md` in `{DEV_REPO}`: prerequisites (`gh` signed in, Inno Setup, optional `KISAES_SIGN_CERT`), the one-command release, beta versus stable versions, how to attach other-platform artifacts with `-ExtraAssets`, how to pull a bad release (`gh release delete` and re-tag), where symbols are archived, and the R6 checklist.

## Acceptance for the whole addendum

- `{PUB_DIR}` tracks only the R1 allowlist, and its issue templates carry the no-client-data warning.
- A dry run produces installer, portable zip, checksums and `latest.json` with fixed names, for both a beta and a stable version.
- The built binary and installer contain no local paths, username, or private repo URL.
- The app makes no update-related network call; the menu item opens the releases page.
- No UI element links to `{DEV_REPO}`.
- `RELEASING.md` and `SOURCE_REQUESTS.md` exist, and the agent made no visibility change and added no collaborator.

## Still open (copy into QUESTIONS.md)

1. **Who may request source.** Anyone who emails, or only holders of a particular licence tier? The procedure in R7 records the tier but does not refuse anyone.
2. **Apps without a source-available licence.** Does the source-on-request wording apply to every desktop app, or only to the ones under the VibeMyFirm Community License?
3. **`{SUPPORT_EMAIL}`.** Which address goes in the README, licence wording and issue-template contact link?
4. **Signed metadata.** Not needed while updates are manual. Revisit if an app ever downloads its own updates.
