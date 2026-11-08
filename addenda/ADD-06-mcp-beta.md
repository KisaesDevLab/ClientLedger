# ADD-06 — MCP bookkeeper moves into the beta

**Status:** Active (added 2026-11-07, during the build) · **Part of the beta** · Phase **A11** (was post-beta P50) · Checkpoint **PR-A3** · Decisions Q-16 (amended), Q-324–Q-334, Q-345

Owner request, 2026-11-07: "Move MCP to after P34."

This addendum moves planned work; it never rewrites completed phases. The MCP bookkeeper keeps the full scope and decisions written for P50 in `phases/M7-post-beta.md` (Q-324–Q-334); only its place in the schedule changes.

Models: opus for all work (Tier H), architect plan required, engine-implementer on opus at effort high, combined reviewer plus **security-reviewer** (opus), and a process review at PR-A3 (opus, xhigh).

## When to run it (scheduling rule)

1. **A11 runs right after P34 (Backup scheduling, recovery and data integrity) merges, before P35 (Packaging).** Its dependencies are all built by then: P07 (control channel and tool table), P29 (users, permissions, closing date, audit trail), P33 (LAN multi-user with TLS pinning) and every beta form.
2. **Checkpoint PR-A3** (opus, xhigh, plus security-reviewer) runs right after A11 merges, before P35: MCP switches and review queue, undo restoring the trial balance exactly, off-limits tools refused for every role, LAN pairing, no secrets exposed, amounts only with writes on (non-negotiable 4 as amended by Q-324).
3. Branch `phase/A11-mcp-bookkeeper`; PR `A11 — MCP bookkeeper`; final commit `docs(state): complete A11, next PR-A3`.

## What changes for planned work

| Phase | Was | Becomes |
| --- | --- | --- |
| P50 (M7 post-beta) | MCP bookkeeper, last post-beta phase (Q-16) | Built in the beta as **A11**, after P34; P50 is withdrawn from M7 |
| P35 packaging | — | Ships the MCP server binary/endpoint (off by default per company, Q-328) and lists it in the installer's components and ATTRIBUTION.md |
| P37 hardening | — | Includes the MCP local socket and LAN pairing in its security pass |
| P38 documentation | — | Adds an owner help page "Let an AI assistant keep your books (MCP)" |
| PR-10 (before P39) | — | Whole-app journeys include one MCP journey with review on |
| CLAUDE.md non-negotiable 4 | Exception names "the post-beta MCP bookkeeper (P50)" | Same exception, now A11 in the beta |

## A11 — MCP bookkeeper

Scope, acceptance and tests exactly as written for P50 in `phases/M7-post-beta.md` (row P50) and decisions Q-324–Q-334. The architect drafts the detailed write-up into `target/plans/A11.md` as the first step.
