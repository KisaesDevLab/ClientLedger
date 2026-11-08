# ADD-07 — The engine moves to its own repository (kisaes-gl-engine)

**Status:** Active (added 2026-11-07, during the build) · **Part of the beta** · Phase **A12** · Decisions Q-346–Q-349

Owner request, 2026-11-07: "How can we move the engine to its own repo at Kisaes Engine", answered in a Q&A the same day.

Today the engine lives inside `KisaesDevLab/Vibe-MyBooks` (packages `api`, `shared`, `worker`, `web`, plus Docker, installer and appliance files), and ClientLedger's `engine/` submodule points at that whole repository. After this addendum the engine has its own repository, and both MyBooks Online and ClientLedger pin it as a git submodule.

## Decisions (owner, 2026-11-07)

| ID | Decision |
| --- | --- |
| Q-346 | New repository **`KisaesDevLab/kisaes-gl-engine`** (the name the engine license and grant use). Public, like Vibe-MyBooks today. |
| Q-347 | It holds **`packages/api` and `packages/shared`** (with `openapi.json`, the migrations, the engine's tests and golden files). MyBooks Online keeps `web`, `worker`, Docker, the installer and the appliance files. |
| Q-348 | **Git submodule** for both consumers: Vibe-MyBooks and ClientLedger each pin an engine commit (no package registry; offline-friendly). |
| Q-349 | **Timing:** right after P26 merges (its engine PRs #235–#238 merged into Vibe-MyBooks first), before A05. |

## When to run it (scheduling rule)

A12 runs right after P26 and its phase-end QA and Q&A, before A05. Branches `phase/A12-engine-repo` (ClientLedger) and `clientledger/engine-repo` (Vibe-MyBooks); PRs `A12 — Engine in its own repository`.

## A12 — steps

1. **Freeze:** no open engine PRs (all P26 PRs merged). Record the Vibe-MyBooks commit the split starts from.
2. **Split with history:** `git filter-repo` keeps `packages/api`, `packages/shared`, `openapi.json`, the engine's own scripts, and the root workspace files the engine needs (`package.json`/`package-lock.json` reduced to the two packages, `tsconfig.base.json`). Engine license set at the root: `LICENSE` (PolyForm Internal Use 1.0.0), `ENGINE-GRANT.md` (Part A), `NOTICE.md` (Part B) from `licensing/KISAES-GL-ENGINE-LICENSE-AND-GRANT.md`, plus `CLA.md` if the owner supplies it (else a placeholder issue). Engine CI moved over: tests, typecheck, migration policy, license check, secret scan (with the golden-file allowlist), dependency audit. Create the repository (owner-approved outward action) and push.
3. **MyBooks Online:** in Vibe-MyBooks, remove `packages/api` and `packages/shared`, add the engine as a submodule at `engine/` (or `packages/engine`, whichever keeps the workspace simplest), point `web`, `worker`, Docker builds, the installer and CI at it, and keep the appliance (BullMQ) path working. One PR, reviewed; MyBooks Online's own tests and e2e green.
4. **ClientLedger:** repoint the `engine/` submodule URL to `kisaes-gl-engine` at the split commit (content identical to the last pin). Update `xtask` engine build/test paths if the layout changed, CI checkout, `UPSTREAM.md` (engine PRs now on kisaes-gl-engine; branch convention `clientledger/<feature>` unchanged), `CLAUDE.md`, `docs/ARCHITECTURE.md`, `NOTICE.md` and ATTRIBUTION.md links. Live contract tests and `cargo xtask ci` green.
5. **Checks:** the engine executable built from the new repo is byte-for-byte the same build input as before (same source tree hash for `packages/api` + `packages/shared`); all ClientLedger live contract tests pass; Vibe-MyBooks CI green; history and blame preserved for engine files.

## Risks

- Open PRs in Vibe-MyBooks at split time would have to be recreated — hence the freeze.
- Dependabot and security alerts move with the code; re-enable them on the new repo.
- MyBooks Online deploys must pull the submodule (`--recurse-submodules`) — update its Docker build and docs.
