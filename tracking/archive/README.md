# Archived tracking documents

This directory holds tracking artefacts that are no longer the
live source of truth but are kept verbatim for provenance.
Sessions browse these only when a question specifically
requires historical context (e.g. "why did we pick chitchat
over openraft?" answered in the April 13 audit).

## Contents

| File | Covers | Archived at |
|---|---|---|
| `HANDOFF-tier0-3.md` | Sessions 1 through 83 (Tier 0 fixes, Tier 1 test infrastructure, Tier 2 unified data plane + every protocol, Tier 3 cluster + observability planes). | Session 86 hygiene sweep. |
| `AUDIT-2026-04-10.md` | First honest maturity audit against MediaMTX, LiveKit, OvenMediaEngine, SRS, Ant Media, AWS KVS, Janus, Jitsi. Pre-session-30. | Session 86 hygiene sweep. |
| `AUDIT-2026-04-13.md` | Full competitive audit dated 2026-04-13. | Session 86 hygiene sweep. |
| `AUDIT-INTERNAL-2026-04-13.md` | Internal dead code, bug, and hardening review. All findings were closed by session 17. | Session 86 hygiene sweep. |
| `AUDIT-READINESS-2026-04-13.md` | CI, supply chain, doc drift, and Tier 1 progress inventory. | Session 86 hygiene sweep. |
| `notes-2026-04-10.md` | Loose planning notes from the April 10 kickoff. | Session 86 hygiene sweep. |

## Why archive instead of delete

The audit docs captured the project's self-understanding at
specific moments. Commit history preserves the lines but not
the narrative. These files are light (< 600 KB total) and
reading them is the fastest way to understand how the current
architecture was chosen.

## What remains live

The live tracking surface is:

* [`../HANDOFF.md`](../HANDOFF.md) -- Tier 4 session blocks
  (84 onward). Authoritative state of current work.
* [`../ROADMAP.md`](../ROADMAP.md) -- 18-24 month
  plan. Stable across tiers.
* [`../TIER_3_PLAN.md`](../TIER_3_PLAN.md) -- closed;
  retained for reference.
* [`../TIER_4_PLAN.md`](../TIER_4_PLAN.md) -- active.

Newer audits (session 72, 80, 85) live in the audit-shape
memory file at
`.claude/projects/.../memory/project_audit_findings.md`, not
here. That file is refreshed as the audit-shape findings
evolve; once a set of findings is superseded, the older
version is overwritten rather than archived.
