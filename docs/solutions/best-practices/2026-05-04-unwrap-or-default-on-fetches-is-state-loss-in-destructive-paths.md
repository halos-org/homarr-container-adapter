---
title: "`unwrap_or_default()` on fetch results is silent state loss in destructive code paths"
date: 2026-05-04
category: best-practices
module: homarr-container-adapter
problem_type: best_practice
component: sync-loop
severity: high
applies_when:
  - A fetch (HTTP GET, DB query, file read) returns `Result<Vec<T>>` or similar.
  - The result drives a decision about whether to mutate persistent state — delete a row, prune a state-file entry, advance a watermark, send a destructive message.
  - `unwrap_or_default()` (or equivalent: `.ok().unwrap_or_default()`, `.unwrap_or_else(|_| vec![])`) is being used to convert the `Result` into a plain value.
tags:
  - error-handling
  - destructive-paths
  - state-loss
  - api-wrappers
  - homarr-container-adapter
---

# `unwrap_or_default()` on fetch results is silent state loss in destructive code paths

## Context

Three sites in a single PR's first draft used `unwrap_or_default()` on fetch results that drove destructive decisions:

1. `sweep_orphan_items()` — `let board_items = self.get_board_items(name).await.unwrap_or_default();` — a transient fetch failure produced an empty `Vec`, so `partition_items_by_app_id` reported `orphan_count == 0`, the cascade returned `Ok(())`, and the caller pruned `state.discovered_apps[url]` while the orphan items on the unfetched board persisted.
2. Cleanup-loop refetch — `let apps_after_sync = client.get_all_apps().await.unwrap_or_default();` — every stale URL then missed the lookup, fell into the "app already gone" branch, returned `Ok`, and the entire stale-state set was pruned in one shot.
3. `get_board_items()` itself — internally returned `Ok(vec![])` on non-success HTTP status, propagating the same pattern down one layer.

In all three cases a single transient failure (network blip, Homarr 5xx, auth expiry mid-sync) became a **permanent data inconsistency**: the adapter forgot it ever discovered records, but the records and their orphan dependents remained in the destination system with no future trigger to clean them up.

## Guidance

**`unwrap_or_default()` belongs in read-only or additive code paths only. In destructive paths, propagate the error.**

The litmus test for any fetch site:

> "If this fetch returned `Err`, would the code below it do something different than if it returned `Ok(vec![])`?"

- **No** → `unwrap_or_default()` is fine. The empty case is genuine and equivalent.
- **Yes** → propagate the error with `?` (or handle it explicitly). The "I got nothing" branch must be reachable only when there is genuinely nothing.

```rust
// ❌ Wrong — destructive path treats fetch failure as "nothing to do"
async fn sweep_orphan_items(&self, app_id: &str, board: &str) -> Result<()> {
    let items = self.get_board_items(board).await.unwrap_or_default();
    let (filtered, orphan_count) = partition_items_by_app_id(items, app_id);
    if orphan_count == 0 {
        return Ok(()); // "no orphans" — but maybe we never saw the board
    }
    self.save_board(board, filtered).await
}

// ✅ Correct — fetch failure aborts the destructive operation
async fn sweep_orphan_items(&self, app_id: &str, board: &str) -> Result<()> {
    let items = self.get_board_items(board).await?;
    let (filtered, orphan_count) = partition_items_by_app_id(items, app_id);
    if orphan_count == 0 {
        return Ok(()); // genuinely no orphans — verified by a successful fetch
    }
    self.save_board(board, filtered).await
}
```

The same applies one layer up. If a wrapper like `get_board_items` returns `Ok(vec![])` on a 4xx/5xx HTTP response (a common "tolerant" pattern), every caller inherits the silent-state-loss footgun. Make the wrapper return `Err` on non-success status and let each caller choose its own tolerance via `.unwrap_or_default()` if it's a non-destructive path.

## Why This Matters

`unwrap_or_default()` is one of Rust's most innocuous-looking footguns because the type system is happy: the fetch returns `Result<Vec<T>>`, you have a `Vec<T>` afterwards, the borrow checker has no opinions. But the runtime semantics — "the fetch failed, but I'm going to act as though the answer was empty" — silently approve destructive branches that should never have been entered.

The same pattern applies to:

- **Database queries.** `query_all().await.unwrap_or_default()` followed by `delete_orphans_not_in(results)` is the same bug at SQL layer.
- **File reads.** `fs::read_to_string(path).unwrap_or_default()` followed by `truncate_log_to(&content)`.
- **Channel receives.** `try_recv().ok().unwrap_or_default()` driving a state machine transition.

In all cases the question is the same: does my code distinguish "I checked and there's nothing" from "I couldn't check"?

## When to Apply

Audit `unwrap_or_default`, `unwrap_or_else(|_| Default::default())`, and `.ok().unwrap_or_default()` at code review time. For each occurrence, ask:

1. What is the type of the wrapped `Result`?
2. What does the code below the unwrap do with the value?
3. Is that behavior correct when the fetch succeeded with an empty result?
4. Is it **also** correct when the fetch failed?

If (3) and (4) diverge, the unwrap is wrong. Replace with `?` propagation, an explicit `match`, or — when the read is genuinely best-effort — a `let-else` with logged warning and an explicit early return.

For a destructive operation that **must** decide based on a fetch, a refetch failure is a **hard stop**: skip the destructive branch entirely (with a warning) and let the next run retry. Do not advance state on uncertainty.

## Examples

**Cleanup-loop bail-out (correct):**

```rust
let mut apps_after_sync = match client.get_all_apps().await {
    Ok(apps) => apps,
    Err(e) => {
        warn!(
            "Skipping stale-SK cleanup: failed to refetch apps ({}). \
             State entries retained for retry.",
            e
        );
        return; // do not iterate stale_urls — every iteration would prune state
    }
};
```

**Wrapper returns `Err` on non-success HTTP (correct):**

```rust
async fn get_board_items(&self, board_name: &str) -> Result<Vec<Value>> {
    let response = self.get(&url).await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default(); // ← OK here
        return Err(AdapterError::HomarrApi(format!(
            "board.getBoardByName('{}') returned {}: {}",
            board_name, status, text
        )));
    }
    // ...parse and return items
}
```

Note: `unwrap_or_default()` on `response.text().await` inside the error-formatting branch is fine — we're already returning `Err`, so a failure to read the error body just produces a less-detailed error message, not a destructive decision.

## Related

- This solution: `homarr-container-adapter/src/homarr.rs::sweep_orphan_items`, `get_board_items`; `src/main.rs::cleanup_stale_signalk_webapps`
- Companion learning: [Sweep-first-then-delete for cascade wrappers](2026-05-04-sweep-first-then-delete-for-non-cascading-api-wrappers.md) — the same orphan-creation chain this PR fixed
- Companion learning: [Ship cross-format identity helper before URL migration](2026-05-04-ship-cross-format-identity-helper-before-url-migration.md)
