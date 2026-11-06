# ADD-05 — VibeMyFirm Community License: tiers, keys and Pro gating

**Status:** Active (added 2026-11-06, during the build) · **Draft — license texts received; waiting on the engine addendum** · Phases L1–L5 run with packaging (P35/P36), L5 with P39 · Decisions Q-336–Q-344 (supersede Q-303, amend Q-01, Q-133, Q-134, Q-316)

Owner request, 2026-11-06: the build-plan addendum "ClientLedger: VibeMyFirm Community License Implementation" (kept verbatim below under *Owner brief*), with the owner's answers to a Q&A the same day. This addendum never rewrites completed phases.

## Source files

- `licensing/LICENSE-VibeMyFirm-Community-1.0.md` — received 2026-11-06; becomes `LICENSE.md` in L1 (Sections 5–9 and 18 drive the UI copy).
- `licensing/KISAES-GL-ENGINE-LICENSE-AND-GRANT.md` — received 2026-11-06 (Part A grant, Part B notice).
- The engine OEM License Agreement — received 2026-11-06; **not committed here** (an engine-repo document with fee terms; the engine notice points to it by URL).
- **Still waiting:** `PROMPT-kisaes-gl-engine.md` (the engine addendum: `GET /license` contract and the host-signature mechanism). Items needing it stay PENDING.

## What the license texts settle (read 2026-11-06)

- **Own-Entity Use has no fee and no key** (§5.1: no limit on users or entities) — matches Q-337. Pro and Developer use need a valid key (§5.2, §5.3).
- **Pro key term** is "the term stated in the Commercial Terms" (§6.2); the brief says Pro keys are perpetual (`exp: null`). The app accepts `exp: null` and also honours a dated `exp` if a key carries one (never locking data; Q-344).
- **Developer builds** (§7.2): never include the engine; About states "Based on software © Kisaes LLC, licensed under the VibeMyFirm Community License."; no Kisaes marks as the brand; no embedded keys; no bypass of key checks.
- **Engine** (§8, grant Part B.3): the engine inside an Official Build is licensed to the user under the Community License; the engine checks the signature of the host that launches it, and an unsigned or third-party host runs it in **internal mode** (no distribution, no remote users). Official Build = built, signed and published by Kisaes (§18).
- **Required Notice** for source headers (L1): `Required Notice: Copyright © 2026 Kisaes LLC. Licensed under the VibeMyFirm Community License 1.0, https://vibemyfirm.com/licenses/community-1.0`.
- **Engine name:** "Kisaes GL Engine", repository `kisaes-gl-engine` (the submodule is `KisaesDevLab/Vibe-MyBooks` today; the rename is engine-side and follows the engine addendum).

## Owner answers (2026-11-06)

| ID | Decision |
| --- | --- |
| Q-336 | The Community License 1.0 **replaces PolyForm Internal Use 1.0.0** (Q-303). The brief's "Remove MIT" means "remove the current license": `LICENSE` (PolyForm) → `LICENSE.md` (Community License) + `NOTICE.md`; crate `license` fields, cargo-deny, the installer acceptance screen (Q-134) and the winget Agreements (ADD-04) follow. |
| Q-337 | **Keys only for Pro and Developer.** Business use needs **no key and no registration**: no licensing.kisaes.com call, no "unregistered" reminder. The brief's L2 Business registration flow is dropped. No key = Business tier. |
| Q-338 | Supersedes CLAUDE.md "No license keys, activation, or seat limits" only as far as Q-337: optional paid Pro/Developer keys, verified offline; still **no activation, seat or company counting, expiry, kill switch or data lock**. |
| Q-339 | **Schedule:** L1–L5 run with packaging — L1–L4 inside P35/P36, L5 (signing, release notes) with P39. P24 and the rest of the beta track are not interrupted. |
| Q-340 | Pro unlocks only the post-beta accountant tools (brief Decision 4). Every beta feature, LAN multi-user and unlimited companies stay in Business. |
| Q-341 | Network calls stay user-started only (non-negotiable 3): Buy (opens the browser), Refresh key (Pro/Developer), and the engine download in developer builds. |
| Q-343 | **LAN and the engine's internal mode (default, to confirm):** workstations of the owner's own Company on the LAN are internal users, not "remote users"; Official Builds are signed, so the engine runs in its full mode; unsigned development and CI builds must still run LAN tests in internal mode. Confirm against the engine addendum before P33. |
| Q-344 | **Key expiry (default, to confirm):** keys with `exp: null` never expire; a key with a dated `exp` that has passed turns Pro features off again (banner only), never locks data or bookkeeping. |
| Q-342 | Engine naming ("Kisaes GL Engine" in About) and the `GET /license` contract follow the engine addendum once uploaded; until then About shows the engine version only (PENDING). |

## What changes for planned work

| Phase | Was | Becomes |
| --- | --- | --- |
| P35 packaging | PolyForm + engine commercial license acceptance (Q-134) | Community License 1.0 acceptance; NOTICE.md; L1 (legal files, About), L2 (key module, Help › License), L3 (honor prompt, Pro gating, Firm menu greyed), L4 (`--brand` developer builds, engine fetched at install) |
| P36 updates | Version check only | Major-version policy (key `major` vs app major; refused major opens read-only with backup/export, never deletes or locks data) |
| P39 release | Release checklist | L5: code-signed Windows and macOS artifacts in CI; release-notes template names the license change; fresh-install smoke on Windows 11 and macOS 14 |
| P45 accountant tools (post-beta) | Ungated | Behind `requires_pro()` |

## Phases (from the brief, with Q-337 applied)

- **L1 — Legal files and About.** Replace `LICENSE` with `LICENSE.md` + `NOTICE.md`; README "License" section (plain summary, link https://vibemyfirm.com/licenses/community-1.0); one-line Required Notice in source headers; About shows tier, firm, engine version and mode. Acceptance: nothing in the repo names PolyForm or MIT as ClientLedger's license; About renders with and without a key.
- **L2 — Key handling (Pro/Developer only).** `license` module: verify Ed25519-signed JWT offline with the compiled-in public key; claims `firm`, `tier`, `products` (must include `clientledger`), `issued`, `exp` (null), `major`; store in the app data dir with owner-only permissions; Help › License to enter, paste, refresh or remove. No key → Business. Acceptance: unit tests for valid, invalid, tampered, wrong-product and major-mismatch keys; the app works fully with no key except Pro features.
- **L3 — Honor prompt and Pro gating.** First-run screen and Help › License checkbox; banner at most once per session; Firm menu greyed with "Pro feature" tooltip without Pro/Developer; one `requires_pro()` helper; integration test asserts the gate list equals Decision 4 exactly. Acceptance: no key runs every beta bookkeeping workflow end to end.
- **L4 — Developer build path.** `--brand <config>` only with a Developer key at build time; attribution string injected; key never embedded; engine never bundled — the installer fetches it from the Kisaes release URL and verifies SHA-256 and signature.
- **L5 — Release.** Code signing in CI; release-notes template; fresh-install smoke tests.

## Questions to raise if found (brief)

- A beta feature unclear as Business vs Pro.
- The first-run wizard can't host the license screen without redesign.
- LAN mode exposes the engine on an interface the engine's internal-mode policy would reject (P33).

## Owner brief (verbatim UI copy; counsel-reviewed)

- First-run title: "How will you use ClientLedger?"
  - Option A: "For a business I own, control, or work for — free."
  - Option B: "For clients I'm paid to keep books for — requires a Pro license ($495 one-time per firm)."
  - Footer: "Full terms: VibeMyFirm Community License 1.0 (link). You can change this later in Help → License."
- About: "ClientLedger <version> — Licensed to <firm> (<Tier>). Engine: Kisaes GL Engine <version>, <mode>. © Kisaes LLC. VibeMyFirm Community License 1.0." (With no key: tier "Business", firm omitted — wording to confirm against the license text.)
- Pro banner: "Keeping books for clients needs a Pro license. Enter a key or buy one ($495, one-time)." Buttons: Enter key · Buy · Not now.

Non-goals: no per-seat or per-company counting; no remote kill switch, expiry or data lock; no engine changes in this repo.
