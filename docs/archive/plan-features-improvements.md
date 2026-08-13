# Plan — features.md accuracy + discoverability

**Status:** done · **Priority:** product positioning · **Depends on:** latency P0/P1 done  
**Related:** [features.md](features.md), [status.md](status.md), [plan-latency-competitors.md](plan-latency-competitors.md)

## Goal

1. Keep [features.md](features.md) **truthful** vs main (no stale ❌ for shipped work).  
2. Surface latency / safety knobs in TUI (`/tools`, `/info`, `/theme`) so matrix claims are **user-visible**.  
3. Track remaining gaps that still move FEATURES cells (OAuth, memory, etc.) without re-opening dropped swarm.

## Diagnosis (2026-08-05 audit)

| FEATURES claim (old) | Reality | Fix |
|----------------------|---------|-----|
| Mouse-interactive ❌ | HitArea, stop, scrollbar, slash hover | ✅★ |
| Resume “plain only” | TUI `/sessions` `/resume` `/continue` | ✅ |
| Loop protection only max turns | + doom-loop 3× | ✅★ |
| No latency section | Full stack shipped | new §10 |
| OpenCode “Go/TS” | TS/Effect mono-repo | wording |
| LOC ~24k | ~50k crate .rs | update |
| Slash list incomplete | missing `/sessions` `/resume` `/rename`… | inventory |
| `/tools` ignores core profile | listed full ToolExecutor | use agent profile |
| Theme dialog stub | DialogKind only, no paint/confirm | full picker |

## Tasks

1. [x] Rewrite FEATURES.md (date, mouse, resume, doom-loop, core tools, latency §, inventory).  
2. [x] `/tools` uses active agent tool profile + shows profile name + count.  
3. [x] `/info` shows `tool_profile`, `prompt_cache`, `model_fast`, title source, cache usage.  
4. [x] status.md tracks this plan + latency plan.  
5. [x] TUI `/theme` + Theme dialog (list, Enter/click apply, `:theme` colon cmd).  
6. [x] README latency blurb + open-plans table.

## Acceptance

- FEATURES mouse / resume / loop / latency rows match code on main.  
- `/tools` under default config lists ~core set, not full github/web dump.  
- `/info` mentions tool_profile and cache fields when present.  
- `/theme` opens a working picker; `/theme nord` applies by name.  
- status.md notes FEATURES refresh + this plan.

## Non-goals

- Swarm, OAuth implementation, semantic memory, desktop/IDE.  
- Fabricating competitor benchmarks.
