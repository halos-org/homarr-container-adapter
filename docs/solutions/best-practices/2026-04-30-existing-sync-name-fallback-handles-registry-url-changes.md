---
title: Existing name-fallback dedup in a registry-driven sync handles URL-shape changes without explicit migration code
date: 2026-04-30
category: best-practices
module: homarr-container-adapter
problem_type: best_practice
component: sync-loop
severity: medium
applies_when:
  - A registry-driven sync loop already does dedup by stable identifier (URL match, name fallback)
  - A planned change reshapes the registry URL (e.g., absolute → path-only, host substitution, scheme change)
  - The sync's update path unconditionally writes the new registry values to the matched record
  - The "migration" requirement is just "stored records pick up the new URL shape on next run"
tags:
  - sync-loop
  - dedup
  - migration
  - yagni
  - planning
  - homarr-container-adapter
---

# Existing name-fallback dedup in a registry-driven sync handles URL-shape changes without explicit migration code

## Context

A registry-driven sync (registry file → external service) is changing the URL shape of one or more entries — e.g., `https://<host>/<path>` → `/<path>`. The plan calls for a "migration unit" to rewrite stored records that still hold the old shape: load self-hostnames, build origin predicates, fetch existing records, match by old shape, write new shape, preserve admin overrides, gate with a one-shot idempotency flag.

Before writing any of that, **read the sync's current dedup path end-to-end**. If the sync already finds existing records by a stable identifier (a name, an ID, anything that survives the URL change) and the update path already writes the full registry record into the match, the migration is already happening as a side effect of normal sync.

## What we did

The sync's `add_registry_app` path was:

1. Try to find an existing record by `normalize_url(registry_url)`.
2. **On URL miss, fall back to name match** ("App not found by URL, but found by name — will update existing app").
3. Call `update_registry_app` on the match, overwriting `href`, `pingUrl`, icon, description from the registry.

When the registry's URL shape changes:

- URL match fails (different shape after normalization).
- Name match wins (names are the stable identifier across registry-shape changes).
- The full registry record overwrites the stored values.

The migration "just runs" on the next sync after the registry is updated. No new state, no probe, no hostname loader, no origin predicate, no idempotency flag.

## Net change

The originally-planned migration unit collapsed to **zero lines of code**. Closed by referencing the existing dedup path in the plan doc and verifying live that stored URLs flip on the next sync after the registry update lands.

## When this generalizes

Anywhere a sync already has multi-level dedup (try-strict-key, fall-back-to-stable-key) and an unconditional write-through update path. The pattern: **registry URL changes are not a migration; they're an input change that the existing sync absorbs on its next pass.**

## When this does NOT generalize

- The sync only matches by the URL itself (no stable secondary key) — old records become orphans, new records get duplicated. A real migration is required.
- The update path tries to preserve fields conditionally (e.g., "don't overwrite admin-edited pingUrl"). Then the existing sync is too lossy as a migration vehicle and you do need explicit logic. **First, ask whether that conditional preservation is actually a requirement** — if the answer is "admin customizations on registry-managed records are out of scope," drop the preservation and the migration evaporates.
- The registry change is destructive (record removed, not reshaped). Existing dedup won't trigger removal of orphans without separate cleanup logic.

## Anti-pattern this replaces

Designing a migration unit on the assumption that "the sync writes new records but doesn't reshape old ones" — without verifying that assumption against the actual sync code. Plans that bundle a hostname loader, an origin predicate, a runtime version probe, and a one-shot idempotency flag for what turns out to be a no-op.

## Lesson

Before specifying a migration, **trace one record end-to-end through the existing sync after the registry change**. If it already lands in the right state, the migration unit doesn't exist. The corollary: when reviewing plans, ask "what happens on the next sync if we ship the registry change alone?" before approving a parallel migration path.

## Related

- The Phase 2 / Phase 3 split in the plan was a planning artifact — Phase 3 turned out to be subsumed by Phase 2's existing sync semantics.
- Companion learning on the same project: [Skip APT Depends pins between sibling HaLOS packages](2026-04-30-skip-apt-depends-pins-sibling-halos-packages.md) — same theme of "the existing system already covers this; don't add a layer."
