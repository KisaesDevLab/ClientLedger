# M6 — Ship beta (P35–P39)

**Outcome:** installers for Windows x64, macOS, and Linux; safe updates and migrations; accessibility, performance, fuzzing, and security hardening; bundled help; staged beta.

**Milestone exit:** **PR-9** (after P36) and **PR-10** (before P39 sign-off) process reviews with no unresolved Blocking items; owner sign-off recorded (STOP).

---

## P35 — Packaging

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | L | P00–P34 | rust-implementer (sonnet) | security-reviewer (opus) |

**Decisions:** Q-26–Q-28, Q-121, Q-123, Q-134, Q-158. **Files:** `packaging/{windows/wix/*,macos/*,linux/{appimage,deb,rpm}/*}`, `xtask/src/package.rs`, `.github/workflows/release.yml`.

**Steps**
1. Bundle app + engine executable + PostgreSQL + PDFium per target. — `build(package): bundle layout`
2. Windows x64 MSI (per-user and per-machine) with license acceptance (MIT, PolyForm Internal Use, Q-134). — `build(package): msi`
3. macOS DMG (arm64/x64 or universal) with license acceptance; notarization hooks. — `build(package): dmg`
4. Linux AppImage, `.deb`, `.rpm`; license shown on first launch for AppImage/tarball. — `build(package): linux`
5. Signing steps run only when secrets exist (Q-27); unsigned-install notes; SHA-256 checksums; artifacts to GitHub Releases (Q-121); names say "ClientLedger" (Q-123). — `ci: release workflow`

**Tests:** clean-VM installs with networking disabled (Windows 11, macOS current−1, Ubuntu 22.04/24.04) on minimum hardware (Q-158); uninstall keeps data unless chosen.

**Acceptance**
- [ ] Clean installs launch and create a company offline.
- [ ] Uninstall keeps company data by default.

---

## P36 — Updates and migrations

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P35 | rust-implementer (opus) | reviewer (opus), security-reviewer (opus) |

**Decisions:** Q-43, Q-46, Q-214–Q-216. **Files:** `crates/cl-host/src/{update_check.rs,migrate.rs}`, `docs/pg-major-upgrade.md`.

**Steps**
1. Manual update check against GitHub Releases; single channel; no reminders; verify downloaded installer checksums; changelog link in About (Q-214–Q-216). — `feat(update): check`
2. Upgrade flow: verified backup → install → per-company migrations on next open with progress → automatic rollback on failure. — `feat(update): migrations`
3. Backup and transfer compatibility matrix across versions (Q-43). — `test(update): compatibility`
4. Documented, tested PostgreSQL major-upgrade procedure. — `docs(pg): major upgrade`

**Tests:** upgrade from the previous build passes golden reports; forced migration failure restores the backup; older backups restore into the new version.

**Acceptance**
- [ ] Upgrade from the previous beta migrates the sample company and passes golden reports.
- [ ] Forced migration failure restores the pre-upgrade backup.

### Checkpoint PR-9 — Upgrades and migrations (fable)
Interrupted migrations, rollback, schema/version compatibility of backups and transfer files, LAN host and workstation version mismatch. Fix Blocking findings on `fix/PR-9`.

---

## P37 — Hardening

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | L | P35 | rust-implementer (sonnet) | security-reviewer (opus), ui-verifier |

**Decisions:** Q-111, Q-116, Q-158. **Files:** `xtask/src/gen_large.rs`, `crates/*/fuzz/*`, `docs/security-checklist.md`.

**Steps**
1. Accessibility: labels, focus order, contrast, screen-reader smoke test per OS. — `fix(a11y): audit fixes`
2. Performance: synthetic 5-year, 100k-transaction company; all DESIGN.md budgets on minimum hardware (Q-158). — `perf: budgets`
3. Fuzzing: OFX/QFX/QIF/CSV, backup and transfer manifests; 24 h per target. — `test(fuzz): long runs`
4. Security checklist sign-off (loopback defaults, tokens, control channel off by default, TLS pinning, keychain, no secrets in logs/backups/transfers, AI redaction). — `docs(security): checklist`

**Acceptance**
- [ ] All performance budgets met and recorded.
- [ ] 24 h fuzzing per target with no crashes or hangs.
- [ ] Security checklist has no open Critical/High items.

---

## P38 — Documentation

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P35 | docs-writer (sonnet) | — |

**Decisions:** Q-109, Q-132, Q-135, Q-137, Q-159–Q-161. **Files:** `docs/user/*`, `assets/help/*`, `assets/cheatsheet.pdf` source.

**Steps**
1. User guide (getting started, setup interview, daily work, banking and imports, reports, sales tax, inventory, payroll import, send to accountant, LAN setup — LAN only, Q-109, backups, troubleshooting, AI and privacy). — `docs(user): guide`
2. QBD switcher guide (exports, import, tie-out, keyboard differences, Classes → Tags). — `docs(user): qbd switcher`
3. Privacy notice and own-books scope note (Q-135, Q-137). — `docs(user): privacy and scope`
4. Bundle help in the app; F1 per screen; publish the same pages online (Q-159). — `feat(help): bundled help`
5. Printable keyboard cheat sheet from the keymap (Q-161). — `docs(user): cheat sheet`

**Acceptance**
- [ ] F1 opens a help page on every beta screen, offline.

---

## P39 — Beta release (STOP)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | S | P35–P38 | orchestrator + bookkeeper | **PR-10** process-reviewer (fable) |

**Decisions:** Q-35, Q-46, Q-47, Q-75, Q-120, Q-121. **Upstream:** U-10.

**Steps**
1. **PR-10** whole-app journeys on the sample company: setup → a month of work → reconcile → reports → close → send to accountant → restore. Fix Blocking findings.
2. Release checklist: version 0.x bump (Q-46); changelog from conventional commits; artifacts and checksums; release notes and known issues; ATTRIBUTION.md and NOTICE.md complete; engine relicensed to PolyForm Internal Use in `vibe-mybooks` (engine PR merged) and engine repo public (Q-35, U-10).
3. Log-bundle export (scrubbed) for GitHub issues (Q-47).
4. Owner actions: ClientLedger.net download link (Q-120, Q-121); stage-1 beta with the owner's clients, then public pre-release (Q-75).

**Acceptance**
- [ ] **STOP:** owner sign-off recorded in STATE.md before anything is published.
