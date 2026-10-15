# CLAUDE.md — ClientLedger

ClientLedger is a free, open-source (MIT), fully offline, native, cross-platform desktop bookkeeping app for **US small business owners**. It gives QuickBooks Desktop switchers the workflow they know — form-first entry, two-line registers, keyboard-driven everything — on top of the existing **Vibe MyBooks engine**. When the owner's accountant needs the books, ClientLedger exports a transfer file the accountant imports into MyBooks Online (the firm's browser-based MyBooks).

- **UI:** Rust + egui (eframe), the same approach as PdfCraft. Windows x64, macOS, Linux from one codebase.
- **Engine:** the existing MyBooks TypeScript engine (public repo, PolyForm Internal Use 1.0.0 plus a ClientLedger redistribution grant), compiled to a single executable with Node SEA and run locally as a sidecar process.
- **Database:** PostgreSQL 16, bundled with the app and managed by ClientLedger.
- **Network:** none required. The only online features are optional and owner-configured: AI suggestions with the owner's own Anthropic or OpenAI key (or fully offline with a local Ollama model), emailing invoices through the owner's SMTP server, and the manual update check.
- **License and repo:** ClientLedger code is MIT; the repo is public; outside pull requests are not accepted. No license keys, activation, or seat limits.
- **Owner:** KisaesDevLab (Kurt). Branding: "ClientLedger — powered by Vibe MyBooks". The ClientLedger name and ClientLedger.net belong to this desktop app; MyBooks markets under its own name (Q-120).

Read these before every session, in order:

1. `STATE.md` — where the build is and what is next
2. `PHASES.md` (index) and the milestone file in `phases/` that contains the phase you are working on — read only that phase in detail
3. `QUESTIONS.md` — open questions and the default assumptions you must follow
4. `docs/ARCHITECTURE.md`, `docs/DESIGN.md` — when the phase touches those areas; for any screen, also `docs/design/README.md` and that screen's mockup
5. `UPSTREAM.md` — engine changes requested from or made in the MyBooks engine
6. `docs/AGENTS.md` — which subagent and model to use for each job and phase tier

---

## Non-negotiables

1. **Offline first.** Every feature except those explicitly marked *online-only* must work with networking disabled. Tests run with network blocked by default.
2. **Permissive licenses only.** Rust dependencies must be MIT or Apache-2.0 (dual MIT/Apache preferred). Transitive dependencies may also use the permissive licenses on the `cargo-deny` allowlist in `deny.toml` (see QUESTIONS Q-07). Never add GPL, LGPL, AGPL, MPL, SSPL, BSL, or "source-available" crates. Fonts and assets are audited separately in `ATTRIBUTION.md`.
3. **No telemetry.** No analytics, crash upload, or phone-home. Logs stay on the machine. The app makes no outbound connection unless the owner turned on AI or email, or clicked "Check for updates".
4. **AI never touches amounts.** AI may suggest accounts, payees, tags, and read document fields; every amount comes from the user, the bank file, or the source document, and every AI suggestion requires a user accept.
5. **Original design.** The app is familiar in workflow, not in look. Do not copy Intuit icons, the QuickBooks Home-page flowchart artwork, screen layouts pixel-for-pixel, color schemes, or names. Use the tokens in `docs/DESIGN.md`. Mention QuickBooks only in import/migration text and the non-affiliation notice.
6. **Money is exact.** Never use floating point for money. The engine is the source of truth; the UI uses `rust_decimal` for display and input parsing only, and sends strings/minor units exactly as the engine contract specifies.
7. **The engine owns accounting rules.** Posting, balances, closing-date enforcement, sales tax math, inventory costing, and audit trail live in the engine. The UI never writes to Postgres directly and never re-derives a balance the engine already reports.
8. **Closed periods are locked.** Any edit that would change a transaction dated on or before the closing date requires the closing-date password flow and is recorded in the audit trail.
9. **Nothing destructive without a backup.** Restores, imports, migrations, and app upgrades take an automatic verified backup first.
10. **Public repo hygiene.** Never commit secrets, real client data, real bank files, real QBD exports, or private URLs. CI for pull requests needs no secrets.
11. **Secrets stay local.** AI keys, SMTP passwords, and the Postgres superuser password live in the OS keychain. They never appear in logs, backups, transfer files, screenshots, or crash dialogs.
12. **Write for owners.** Labels, setup, and errors use plain language a business owner understands; accounting jargon gets a tooltip. Beta excludes accountant-only tools (see PHASES.md M7).

---

## How you work (autonomous phase loop)

0. **Run continuously (Q-112).** Keep going phase after phase without waiting for review. Stop only for items marked **STOP** in QUESTIONS.md or PHASES.md, or for irreversible actions.
1. Read `STATE.md`. Work on the phase marked **Next**. Do not skip ahead or start a later phase "while you're there". Create branch `phase/Pxx-short-name` from the latest `main`.
2. Before coding, list the phase's acceptance criteria as tasks. If the phase depends on an engine capability, run the **capability check** (below) first.
3. Build in small steps. Keep the workspace compiling and tests green at every commit.
4. When blocked by a decision: add it to `QUESTIONS.md` with a **default assumption**, follow the default, and continue. Never stop the run to wait for an answer unless the decision is irreversible (data loss, licensing, publishing a release).
5. When the phase is done: all acceptance criteria met, `cargo xtask ci` passes, screens verified through the control channel (from P07 on), docs updated. Then update `STATE.md` (mark done, set next phase, add a log entry with date and summary), commit, push, and open a pull request titled `Pxx — <phase title>` with the acceptance checklist and screenshots (Q-113).
6. Enable auto-merge on the PR; it merges when required CI is green (Q-115). Wait for the merge, update local `main`, and start the next phase. If CI fails, fix it on the same branch.
7. At the end of each milestone, add a milestone summary to `STATE.md` (what shipped, where screenshots are, questions added, upstream items pending) for the owner's asynchronous review — then continue.
8. Never mark a phase done with failing tests, skipped criteria, `todo!()` in shipped paths, or `#[ignore]`d tests that were not ignored before.

### Agents and models

The main session is the **orchestrator** and runs on Sonnet. It delegates to the subagents in `.claude/agents/` and routes models by phase tier as described in `docs/AGENTS.md` (Q-221):

- `haiku` — `explorer` (read-only search) and `bookkeeper` (STATE/QUESTIONS/UPSTREAM upkeep). Never production code or reviews.
- `sonnet` — `engine-scout`, `test-writer`, `ui-verifier`, `ci-fixer`, `docs-writer`, and implementers for Tier L/M phases and small engine changes.
- `opus` — `architect`, `reviewer`, `security-reviewer`, and implementers for Tier H phases and M/L or accounting-rule engine changes.
- `fable` — `process-reviewer` only: deep end-to-end process reviews at the 12 checkpoints in docs/AGENTS.md, the riskiest architect plans, and the last escalation step. Never for security review or writing code.

Pass the implementer's model per invocation according to the tier table. Reviews are fresh-eyes: never share the implementation transcript with `reviewer` or `security-reviewer`. Escalate after two failed attempts (sonnet → opus → re-plan → block that item and continue). Log the agents and models used in each phase's STATE.md entry.

### Commits

Conventional commits, small and focused:

```
feat(register): two-line row virtualization
fix(pg): retry initdb when data dir is on a network share
test(import): OFX 2.x multi-account fixtures
docs(state): complete P12, next P13
chore(deny): add Unicode-3.0 to allowlist (Q-07)
```

Scopes match crate or feature names (`host`, `pg`, `api`, `ui`, `grid`, `register`, `bills`, `import`, `reports`, `settings`, `automation`, `pack`, `engine`, `state`, `deny`). One phase may span many commits; the final commit of a phase is `docs(state): complete Pxx, next Pyy`.

### Engine capability check

Many screens depend on engine features (estimates, sales tax, inventory, reconcile, etc.). Before building a feature screen:

1. Search the engine's OpenAPI spec (`engine/openapi.json`, generated in P02) for the endpoints the screen needs.
2. If present: write contract tests in `crates/cl-api/tests/` that exercise them against a live local engine, then build the screen.
3. If missing or incomplete: make the engine change in the `engine/` submodule on branch `clientledger/<feature>`, following the engine's own conventions and tests, record it in `UPSTREAM.md` (what, why, files, migration, status), and open a pull request on the engine repo for the owner to review and merge (Q-29). Keep engine changes minimal and generic so MyBooks Online benefits too. If the change is large (Q-220), still build it upstream as its own engine PR, mark only the affected screen as blocked in `STATE.md`, build that screen against the documented contract with its control-channel tests pending, and keep going with other phases. Never silently move beta scope to post-beta.

---

## Repository layout

> **Before P00 merges**, the crates, `xtask` and git repo don't exist yet; P00 creates them. Remove this note in the P00 PR.

```
ClientLedger/
├── CLAUDE.md  PHASES.md  STATE.md  QUESTIONS.md  UPSTREAM.md
├── phases/                   # detailed phase files, one per milestone (M0–M7)
├── .claude/
│   ├── agents/               # subagent definitions (models, tools, effort)
│   └── settings.json         # max 4 concurrent subagents
├── LICENSE                   # MIT — ClientLedger code only
├── NOTICE.md                 # engine/ is PolyForm Internal Use + ClientLedger grant (Q-31)
├── CONTRIBUTING.md           # issues welcome; pull requests not accepted (Q-32)
├── ATTRIBUTION.md            # every bundled font, icon, binary, and license
├── Cargo.toml                # workspace
├── deny.toml                 # cargo-deny license allowlist + advisories + bans
├── rust-toolchain.toml
├── engine/                   # git submodule: public MyBooks engine repo (pinned tag), NOT MIT
├── crates/
│   ├── cl-core/              # shared types: Money, ids, dates, fiscal calendars, errors (no I/O)
│   ├── cl-api/               # typed client for the local engine API + contract tests
│   ├── cl-pg/                # bundled PostgreSQL lifecycle: initdb, start/stop, backup/restore
│   ├── cl-host/              # supervisor: launches pg + engine, health, logs, single-instance lock
│   ├── cl-company/           # company file model: open/new/backup/restore/portable package
│   ├── cl-settings/          # local preferences + keychain secrets (AI keys, SMTP, pg password)
│   ├── cl-import/            # OFX/QFX/QIF/CSV parsers, CSV mapping profiles, dedupe keys (Q-11)
│   ├── cl-model/             # UI-side form state, validation, keyboard maps, undo stacks (no egui)
│   ├── cl-grid/              # the ledger grid widget (virtualized, two-line rows, inline edit)
│   ├── cl-ui/                # egui app: shell, screens, theme, fonts
│   ├── cl-automation/        # control channel, widget inspection, screenshot, tool table
│   ├── cl-print/             # PDF generation for checks/forms/reports + OS print hand-off
│   └── cl-lan/               # LAN host/client mode: discovery, TLS pinning, sessions
├── apps/
│   ├── clientledger/         # desktop binary (eframe)
│   └── clientledger-cli/     # headless CLI: company ops, import, automation `ui` commands
├── xtask/                    # build helpers: fetch pg binaries, build engine exe, package
├── assets/                   # fonts (IBM Plex, OFL), icons (original), sample company data
├── fixtures/                 # bank files, QBD report exports, golden reports (synthetic only)
├── packaging/                # msi/wix, macOS dmg + notarization, AppImage/deb/rpm
└── docs/
    ├── AGENTS.md             # subagents, model routing, phase tiers, escalation
    ├── ARCHITECTURE.md
    ├── DESIGN.md
    ├── design/               # owner-approved mockups (HTML + PNG) and README; read-only for Claude Code
    ├── KEYBOARD.md           # generated from cl-model keymap in P06
    └── user/                 # user guide, QBD switcher guide (P38)
```

Crate layering rules (enforced by a test in `xtask` from P00):

- `cl-core` depends on nothing internal.
- `cl-model`, `cl-api`, `cl-pg`, `cl-settings`, `cl-import` may depend only on `cl-core`.
- `cl-grid` depends on `cl-core`, `cl-model`, egui.
- `cl-ui` may depend on everything except `apps/*`.
- Nothing below `cl-ui` depends on egui except `cl-grid`.
- Only `cl-pg`, `cl-host`, and `cl-company` may spawn processes or touch the database directory.

---

## Commands

All tasks run through `cargo xtask` so no extra tooling is needed:

```
cargo xtask ci             # fmt --check, clippy -D warnings, test, deny check, layering test
cargo xtask test           # cargo test --workspace with network blocked (CL_OFFLINE_TESTS=1)
cargo xtask run            # build engine exe if stale, fetch pg if missing, run desktop app
cargo xtask engine         # build the engine single executable for the host platform
cargo xtask pg-fetch       # download + checksum-verify pinned PostgreSQL binaries
cargo xtask ui-check <screen>  # launch with control channel, run the screen's scenario, save screenshots
cargo xtask package        # platform installer for the host OS
```

---

## Coding standards

- Rust stable, edition 2024 (Q-117), `#![forbid(unsafe_code)]` in every crate except where a dependency boundary requires it (document why).
- `clippy::pedantic` warnings reviewed; `-D warnings` in CI.
- Errors: `thiserror` in libraries, `anyhow` only in binaries and xtask.
- Async: `tokio` in host, api, lan; the egui thread never blocks — all engine calls go through a request queue with results polled each frame.
- No `unwrap()`/`expect()` in non-test code except for invariants with a comment.
- Logging: `tracing`, rotating files in the app data dir kept 30 days or 100 MB, whichever comes first (Q-118), no PII in log messages (no amounts, names, or memo text at info level).
- Every user-visible string goes through `cl-ui::i18n` (English only for now) so localization is possible later.
- Tests: unit tests next to code; integration tests in `tests/`; UI checks as scripted control-channel scenarios in `crates/cl-automation/scenarios/`.
- Coverage (Q-116): ≥ 80% line coverage across the workspace, measured with `cargo-llvm-cov` (dev tool) including control-channel scenarios run against an instrumented build; CI fails below the threshold. Generated code and `xtask` are excluded.
- Errors shown to owners (Q-119): a plain-language message and suggested fix, plus "Copy details" that copies a scrubbed error ID and log excerpt (no amounts, names, or memo text) for a GitHub issue.
- Fixtures are synthetic. Never commit real client data, real bank files, or real QBD exports.
- `private-fixtures/` (git-ignored, Q-72) may hold the owner's anonymized real QBD exports and bank files. Tests under the `private` feature run against them only when present; never copy their contents into committed files, snapshots, logs, issues, or commit messages. CI never has them.

---

## UI standards (summary — full spec in docs/DESIGN.md)

- Every action reachable by keyboard; shortcuts registered in the single keymap in `cl-model::keymap` and shown in menus, tooltips, and the status bar.
- Forms: `Save & New` (Alt+N), `Save & Close` (Alt+S), `Clear`, Prev/Next record. Esc reverts the focused field, then closes the form if nothing is dirty; dirty forms prompt.
- Date fields accept the date keys in docs/DESIGN.md (+, −, T, M, H, Y, R, W, K).
- Name/account fields autofill as you type; Ctrl+L opens the full list.
- Grids: Tab/Shift+Tab across cells, Enter records the row, Ctrl+Insert inserts a line, Ctrl+Delete deletes a line, arrow keys move between rows.
- Multiple open windows as tabs; Ctrl+Tab cycles; unsaved windows show a dot.
- Light and dark themes from the same tokens; minimum contrast 4.5:1 for text.
- All interactive widgets expose AccessKit labels (required for the control channel and screen readers).
- Screens follow their mockup in `docs/design/` for layout, labels, and content; DESIGN.md and QUESTIONS.md decisions win on conflict (Q-226). Never edit `docs/design/`; never build elements marked PROPOSED.

---

## Definition of done (every phase)

- Acceptance criteria in the phase's `phases/` file all met and checked off in the phase's STATE.md log entry.
- `cargo xtask ci` green on the host OS; cross-platform CI green (from P00 on).
- New screens: control-channel scenario passes, screenshots saved to `target/ui-check/<screen>/`, light and dark; screens with a mockup also pass the `--compare` review, with deviations listed in the PR.
- New engine endpoints used: contract tests pass.
- Docs updated: KEYBOARD.md regenerated if shortcuts changed; ATTRIBUTION.md if assets/binaries added; UPSTREAM.md if engine changed.
- `STATE.md` updated; final commit `docs(state): complete Pxx, next Pyy`.
