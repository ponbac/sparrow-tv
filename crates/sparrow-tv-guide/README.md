# Shield Guide state and Tauri source map

Reference Tauri revision `973e0ad55de728ccb15eef25772d736e95f286c3`. Paths below are relative to the worktree. The Rust test names should describe the same visible state transitions. Use native typed models and deterministic timestamps.

| Rule | Tauri owner | Reference coverage |
| --- | --- | --- |
| Quality Variants fold in Channel Number order and keep the preferred variant or highest available quality | `app/src/features/guide/guide-families.ts` | `guide-families.test.ts` and the one-numbered-row cases in `programme-guide.test.tsx` |
| Programme data follows the selected variant and falls back to the first scheduled variant | `app/src/features/guide/guide-families.ts` | `guide-families.test.ts` |
| All filters hidden Channel Groups before folding and advances on excluded-only pages | `app/src/features/guide/board-group-roster.ts` | `board-group-roster.test.ts` and the hidden-group and pagination cases in `programme-guide.test.tsx` |
| Guide pages start at 40 raw Channels and append 100 using a forward cursor correlated with generation and window | `app/src/features/catalog/use-guide-catalog.ts` | `catalog-browser.test.tsx`, especially appending a correlated continuation, rejecting mismatched generations, and replacing an older parallel bootstrap |
| Now uses a 3-column cursor with no horizontal wrap and a partial last-row fallback | `app/src/features/guide/guide-navigation.ts` and `television-guide-rows.tsx` | `app/src/features/stage/television-navigation.test.tsx` |
| Down prefetch and bounded pending movement handle a late, failed, or variant-only continuation | `app/src/features/guide/guide-navigation.ts` | `app/src/features/stage/television-navigation.test.tsx`, including the pagination error and navigated-away case |
| Now and Next use half-open Programme slots and display Channel-only rows when EPG is unavailable | `app/src/features/guide/now-next.ts`, `programme-guide.tsx` | `now-next.test.ts` and `programme-guide.test.tsx` |
| Local half-hour anchoring, DST offsets, fixed 120-minute visible span, and 3h to 8h extension | `app/src/features/guide/guide-window.ts` and `app/src/features/catalog/catalog-browser.tsx` | `guide-window.test.ts` and the Now/Timeline swap case in `catalog-browser.test.tsx` |
| Timeline uses a focus-driven time anchor, may jump several half-hour ticks, and keeps same-row focus on clock re-anchor | `app/src/features/guide/guide-navigation.ts` | `app/src/features/stage/television-navigation.test.tsx`, including Timeline walk, sliver avoidance, and stable pan after re-anchor |
| The preview uses the focused Programme, then falls back to Now or Next. Description reads follow a settled Programme identity. | `app/src/features/guide/guide-preview.tsx` and `guide-preview-store.ts` | Preview and focus cases in `programme-guide.test.tsx` and `television-navigation.test.tsx` |

The crate tracks the active query and displayed rows separately. It retains compatible rows during same-generation group switches and waits for a new clock window to reach the old page depth. An ordinary continuation failure offers Retry. A stale cursor or wrong-generation result requires a new status read before paging restarts. Deferred Down uses the displayed board state and selection identity.

[GuideScreen](src/guide_screen.rs) owns both views' Toolbar, Channel Group and board focus, viewport reveal, the 150 ms first-page prefetch and a bounded cache of visited group windows. Focus on a Channel Group is independent of selection. Pending group changes project compatible rows from the last loaded page set and reconcile board focus by Channel Number and Quality Variant when the page settles. Up and Down between bands use logical targets. The Tauri screen chooses across bands using the measured horizontal positions of controls.

The [GPUI host](../sparrow-shield/src/guide_surface.rs) dispatches the reducer's typed reads on blocking workers, updates the local clock on minute boundaries, and reports focus using Channel Numbers and control IDs. The [Guide renderer](../sparrow-shield/src/guide_view.rs) shares the poster, preview and toolbar, and mounts only the visible board rows plus two lines of overscan. The reference geometry comes from `app/src/features/stage/television.css`.

The phase control switches between Now and Timeline. The first Timeline activation extends Guide reads from three to eight hours for the life of the screen. While replacement pages catch up, Timeline uses the displayed query window and its pan limits. A view switch preserves the vertical scroll offset within the new view's extent. Selecting a different Channel Group returns the board to the top. Playback, source editing, search, the group chooser and preference persistence follow in later slices.

The Rust cases live next to [Now navigation](src/now_focus.rs) and [Timeline movement](src/timeline_focus.rs), and in [screen navigation](tests/guide_screen.rs), [paging](tests/pages.rs), [Quality Variants](tests/families.rs), [preview](tests/preview.rs), and [time rules](tests/time.rs). `cargo test --locked -p sparrow-tv-guide` runs them on Linux. The integration fixtures use the installed catalog with generated loopback sources.

During the repeated hour when daylight saving time ends, native Guide uses the UTC offset of the current clock reading. JavaScript's local Date constructor chooses the earlier occurrence when that time is ambiguous. This keeps the native Guide's half-hour anchor on the current occurrence.
