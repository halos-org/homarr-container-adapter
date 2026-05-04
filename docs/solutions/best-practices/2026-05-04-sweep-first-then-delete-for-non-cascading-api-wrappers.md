---
title: Sweep-first-then-delete for cascade wrappers around non-cascading APIs
date: 2026-05-04
category: best-practices
module: homarr-container-adapter
problem_type: best_practice
component: sync-loop
severity: high
applies_when:
  - You are wrapping an upstream `delete` API that does not cascade to dependent records (board items, comments, attachments, references).
  - The cascade wrapper does (a) a global delete and (b) one or more dependent-cleanup operations against systems where the dependent rows are looked up by the about-to-be-deleted key.
  - The cleanup operations can fail transiently (network, partial-success on one of N targets, auth expiry).
  - You want the next sync/run to be able to recover the failure without persisting an extra "pending sweep" marker.
tags:
  - cascade-delete
  - failure-recovery
  - retry-semantics
  - api-wrappers
  - homarr-container-adapter
---

# Sweep-first-then-delete for cascade wrappers around non-cascading APIs

## Context

The Homarr `app.delete` endpoint deletes a global app row but does **not** cascade to board items pointing at the deleted appId. The adapter wraps this with `delete_app_and_orphan_items(app_id, board_names)` to perform the cascade itself: `app.delete` + per-board sweep of orphan items.

The first draft did the global delete first, then iterated boards to sweep orphan items. The doc-comment justified it as "more correct semantics — the global app is gone before any board references it without a target." Code review found this ordering creates an unrecoverable failure window:

1. `app.delete` succeeds. Global row gone.
2. Sweep of board #2 of N fails (transient `board.saveBoard` 500). Wrapper returns `Err`.
3. Caller "keeps the state entry for retry" (this was the explicit retry mechanism the PR added).
4. Next sync: the cleanup loop refetches apps, can't find the row (it was deleted in step 1), takes the "app already gone, treat as success" branch, prunes the state entry **without re-sweeping**.
5. Orphan items on board #2 persist forever as "No app" placeholders — exactly the bug the cascade was created to prevent.

## Guidance

**Reverse the order: sweep dependents first, then delete the root.** The correct cascade sequence for a non-cascading API is:

```rust
pub async fn delete_app_and_orphan_items(
    &self,
    app_id: &str,
    board_names: &[&str],
) -> Result<()> {
    // Sweep every board first. Accumulate per-board errors so a failure
    // on board #2 of N does not strand orphans on boards #3..N.
    let mut first_error: Option<AdapterError> = None;
    for board_name in board_names {
        if let Err(e) = self.sweep_orphan_items(app_id, board_name).await {
            tracing::warn!(...);
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    if let Some(e) = first_error {
        // Leave the global app intact so the caller's retry path can
        // re-find it on the next sync and re-attempt the cascade.
        return Err(e);
    }

    // All boards swept clean — safe to delete the global row.
    self.delete_app(app_id).await
}
```

Properties:

- **Sweep is idempotent.** A sweep on a board with no orphan items is a no-op. The pre-delete sweep removes items pointing at the soon-to-be-deleted appId; on partial failure, a retry sees the surviving root, re-sweeps the boards (boards already swept are no-ops), and re-attempts the delete.
- **The root key stays valid until cleanup is fully done.** Future syncs can still look up the row via the existing matcher (`find_app_by_url` etc.) and re-enter the cascade. No "pending sweep" persistence required.
- **Per-board errors accumulate**, not short-circuit. A failure on one board doesn't strand orphans on remaining boards.

## Why This Matters

The naive ordering — delete-then-sweep — feels semantically cleaner ("remove the root, then garbage-collect references") but it makes partial failure unrecoverable: once the root key is gone, you can no longer look up the dependents from the root, and the retry path either (a) silently succeeds without re-sweeping, or (b) requires persisting the pending-sweep set in your own state, which is significant added complexity.

Sweep-first trades a **brief window** during which the root still exists but has no remaining references against a **fully recoverable failure mode**. The window is benign — a global app row with no board items just sits in the picker; users don't see anything wrong. The recoverable failure is the real win.

## When to Apply

- Any cascade wrapper around an API that doesn't natively cascade.
- Multi-step destructive operations where an intermediate failure would orphan resources.
- Idempotent sweep operations against a stable lookup key.

Do **not** apply when:

- The upstream API actually cascades (then your wrapper is a wrapper, not a cascade).
- The dependent cleanup is destructive in ways the root key doesn't gate (e.g., the sweep modifies records that don't reference the root). Then sweep-first leaves you in a partial-cleanup state without the safety net.
- Visibility of the root during the cleanup window is itself a problem (e.g., security: the row must not be reachable while orphan refs exist). In that case, accept the unrecoverable window or persist an explicit pending-sweep marker.

## Examples

**Anti-pattern (delete-first):**

```rust
self.delete_app(app_id).await?;        // root gone
for board in boards {
    self.sweep_orphan_items(...).await?; // can't recover after this point
}
```

If `sweep_orphan_items` fails on any iteration, the root is gone, the lookup key is gone, and the retry can't re-find the row to re-sweep.

**Correct (sweep-first):**

```rust
for board in boards {
    let _ = self.sweep_orphan_items(...).await; // accumulate errors
}
if first_error.is_some() { return Err(...); }   // root still intact
self.delete_app(app_id).await
```

## Related

- This solution: `homarr-container-adapter/src/homarr.rs::delete_app_and_orphan_items`
- Companion learning: [`unwrap_or_default()` on fetches in destructive paths is silent state loss](2026-05-04-unwrap-or-default-on-fetches-is-state-loss-in-destructive-paths.md)
- Companion learning: [Ship the cross-format identity helper before the URL-format migration](2026-05-04-ship-cross-format-identity-helper-before-url-migration.md)
- Adjacent learning from the original migration: [Existing name-fallback dedup absorbs URL-shape changes](2026-04-30-existing-sync-name-fallback-handles-registry-url-changes.md)
