---
title: Ship the cross-format identity helper before the URL-format migration
date: 2026-05-04
category: best-practices
module: homarr-container-adapter
problem_type: best_practice
component: sync-loop
severity: high
applies_when:
  - You are planning a URL-format change for resources synced into a destination system (absolute → path-only, scheme flip, host substitution, port removal, prefix change).
  - Records in the destination are keyed or matched by the URL value being changed.
  - The existing identity comparison (e.g., `normalize_url`) does not span both URL forms — it returns inequality for two URLs that refer to the same logical resource.
  - The sync has any code path that **deletes** records on a "no longer current" decision (stale-cleanup, removal of webapps the source no longer reports, garbage collection of orphans).
tags:
  - url-migration
  - planning
  - sequencing
  - sync-loop
  - homarr-container-adapter
---

# Ship the cross-format identity helper before the URL-format migration

## Context

The path-only-URL migration in this adapter shipped in the wrong order:

1. The first PR taught the adapter to **emit** path-only URLs (`/signalk-server/<webapp>/`) instead of absolute (`https://<host>/signalk-server/<webapp>/`).
2. On a device that had previously synced under the absolute form, the next sync ran with the new emit logic but the same `normalize_url` identity comparison built on `url::Url::parse`.
3. `url::Url::parse` rejects path-only inputs and returns the input unchanged, so `normalize_url("/signalk-server/foo/") != normalize_url("https://host/signalk-server/foo/")` — the two forms of the same logical URL look different to identity.
4. The adapter's stale-Signal-K-webapp cleanup loop saw every absolute URL in `state.discovered_apps` as "no longer current" (because `current_sk_urls` now held the path-only forms), called `delete_app` on each, and the per-app sync loop then re-created the same webapps under fresh appIds — leaving the prior board items as orphans pointing at deleted appIds. Every device that had previously been synced grew one orphan per Signal K webapp on the next sync after the upgrade.

The fix had to land both a focused identity helper (`signalk_webapp_identity`) that returns the canonical sub-path for both URL forms **and** a re-ordering of sync vs. cleanup so that updates run before deletions. None of that work would have been needed if the identity helper had shipped one release earlier.

## Guidance

**Before changing a URL format, ship the identity helper that spans both forms. The format change comes second.** Concretely, a URL-format migration is a two-PR sequence:

**PR 1 — identity helper.**
- Add a `<resource>_identity(url) -> Option<String>` (or equivalent) that returns the same canonical key for both the old and the new URL form.
- Wire it into every site that compares URLs: dedup matchers (`find_app_by_url` and friends), stale-detection sets, lookup fallbacks, prune predicates.
- Keep the helper narrow — only the URL family being migrated. Do not widen `normalize_url` or any general-purpose URL helper, because the blast radius of a global change is too large to analyze.
- Tests: every URL pair `(old_form, new_form)` for the migrated resource maps to the same identity. Pairs of different resources do not collide. The helper rejects URLs outside the migration scope.
- Ship and let it bake on devices for at least one release. Nothing functionally changes — the helper just exists alongside `normalize_url` as a second-tier match.

**PR 2 — format change.**
- Change the producer to emit the new URL form.
- The destination-side sync, sitting on the identity helper from PR 1, finds existing records via the new tier even though they were stored under the old form, updates them in place, preserves their stable IDs.
- No migration logic needed — the existing sync absorbs the format change as an input change.

If the producer change has already shipped (or the identity gap is discovered after the fact), the recovery PR has to land both at once, plus any cleanup logic for whatever orphan damage the first sync caused. That recovery work is harder and more invasive than the prevention.

## Why This Matters

A URL-format migration looks like a "just rename the field" change but is actually a stored-state migration: the existing records' identifiers (the URLs they're keyed or matched by) need to keep matching the new producer output. The identity comparison is the load-bearing piece — every call site that asks "is this the same URL as that one?" is implicitly part of the migration.

Most migration plans get this right when records are matched by **stable secondary keys** (name, container ID, etc.) — the existing dedup absorbs URL changes via the name fallback. See the companion learning [Existing name-fallback dedup absorbs URL-shape changes](2026-04-30-existing-sync-name-fallback-handles-registry-url-changes.md). But that companion learning explicitly warns:

> The sync only matches by the URL itself (no stable secondary key) — old records become orphans, new records get duplicated. A real migration is required.

That warning was correct, and this PR's bug was exactly that scenario for Signal K webapps: they have empty `container_id`, so the name fallback was their only stable-secondary-key path, and the cleanup pass ran *before* sync (so the name-fallback's update path never got the chance — the row was already deleted by `delete_app`). The two failure modes interact:

1. URL identity doesn't span the format transition.
2. Cleanup runs before sync.
3. Therefore: cleanup deletes the row by URL mismatch before sync's name-fallback can update it; sync then creates a new row; old board items become orphans.

Shipping the identity helper first cuts the chain at step 1 — even if cleanup ran first, it would correctly classify the row as "still current under the new format" and skip the delete.

## When to Apply

This sequencing is required when **all** of these hold:

- The destination identity comparison does not naturally span the format change. Test by hand: do the old and new URL form for one logical resource compare equal under your existing `normalize_url`/equivalent? If no, you need a helper.
- The sync has any code path that decides "delete" on a URL-mismatch. Cleanup, garbage-collection, stale-removal, expired-record sweeps — all qualify.
- The producer change is visible to a destination that holds prior records under the old form (i.e., it's not a greenfield deploy).

If the destination has stable secondary keys for every record and the sync's update path absorbs URL changes via those keys (and there is no URL-mismatch-triggered delete path), the format change can ship as a single PR. Verify by tracing one record end-to-end through the existing sync after the format change — if it lands in the right state, you're safe.

## Examples

**This adapter's identity helper (correct, narrow):**

```rust
/// Return the canonical SK-webapp path identity for a URL, or None
/// if the URL is not a Signal K webapp URL. Spans absolute and
/// path-only URL forms.
pub fn signalk_webapp_identity(url: &str) -> Option<String> {
    let rest = url.split(SIGNALK_PATH_PREFIX).nth(1)?;
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return None; // SK Server tile — not a webapp
    }
    Some(format!("/{}", trimmed))
}
```

Wired into `find_app_by_url` as a second-tier match (after `normalize_url`) and into stale-detection sets. Identity is webapp-specific; static-registry apps and unrelated URLs return `None`.

**Why not widen `normalize_url`?** It's a general-purpose helper used by every URL comparison in the codebase, including non-SK paths (Cockpit, Grafana, etc.). Teaching it to span absolute↔path-only would require re-analyzing every call site. The narrow `<resource>_identity` helper has a known blast radius (only the SK-aware sites layer it on top of `normalize_url`).

## Related

- This solution: `homarr-container-adapter/src/signalk.rs::signalk_webapp_identity`, `src/homarr.rs::find_app_by_url` (SK fallback tier)
- Adjacent learning from the original migration: [Existing name-fallback dedup absorbs URL-shape changes](2026-04-30-existing-sync-name-fallback-handles-registry-url-changes.md) — the warning at the bottom of that doc names the exact failure mode this learning prevents.
- Companion learning: [Sweep-first-then-delete for cascade wrappers](2026-05-04-sweep-first-then-delete-for-non-cascading-api-wrappers.md) — failure-recovery semantics for the cleanup-on-stale path that interacted with the identity gap.
