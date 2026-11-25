mod support;

use std::{collections::HashSet, fmt::Write};

use chrono::{Duration, FixedOffset, TimeZone, Utc};
use sparrow_core::{CatalogAccess, ChannelGroupFilter, CoreError};
use sparrow_tv_guide::now_focus::{Direction, NowFocus, NowMove, NowNavigation};
use sparrow_tv_guide::{ChannelGroupPages, GuidePages, GuideWindow, PageStatus, PageUpdate};

#[tokio::test]
async fn folds_a_family_across_pages_and_retries_a_failed_continuation() {
    let fixture = support::open(split_family_m3u(), None).await;
    let core = fixture.core();
    let window = windows()[0];
    let mut pages = GuidePages::new(generation(core), None, window, HashSet::new());
    let first = pages.request_next().unwrap();
    assert_eq!(first.query().channels().page().limit().get(), 40);
    assert!(pages.request_next().is_none());
    let result = core.guide_window(first.query().clone());
    assert_eq!(pages.complete(first, result), PageUpdate::Applied);
    assert_eq!(pages.families().len(), 40);
    assert_eq!(pages.families()[39].variants().len(), 1);
    assert!(!pages.should_prefetch(31));
    assert!(pages.should_prefetch(32));

    let next = pages.request_next().unwrap();
    let requested_page = next.query().channels().page().clone();
    assert!(requested_page.cursor().is_some());
    assert_eq!(requested_page.limit().get(), 100);
    assert_eq!(
        pages.complete(next, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(pages.families().len(), 40);
    assert_eq!(pages.status(), PageStatus::Failed);

    let retry = pages.retry().unwrap();
    assert_eq!(*retry.query().channels().page(), requested_page);
    let result = core.guide_window(retry.query().clone());
    assert_eq!(pages.complete(retry, result), PageUpdate::Applied);
    assert_eq!(pages.status(), PageStatus::Complete);
    assert_eq!(pages.families().len(), 41);
    assert_eq!(pages.families()[39].title(), "City News");
    assert_eq!(pages.families()[39].variants().len(), 2);
    assert_eq!(pages.families()[40].title(), "Weekend Review");
}

#[tokio::test]
async fn clock_rollover_replaces_a_loaded_window_only_after_catching_up() {
    let fixture = support::open(split_family_m3u(), None).await;
    let core = fixture.core();
    let generation = generation(core);
    let [earlier, later] = windows();
    let mut pages = GuidePages::new(generation, None, earlier, HashSet::new());
    apply_next(&mut pages, core);
    apply_next(&mut pages, core);

    pages.select(generation, None, later);
    assert_eq!(pages.displayed_window(), earlier);
    assert_eq!(pages.displayed_status(), PageStatus::Complete);
    assert!(pages.needs_automatic_page());
    let replacement = pages.request_next().unwrap();
    assert_eq!(pages.status(), PageStatus::Loading);
    assert_eq!(
        NowNavigation::default().press(NowFocus::Channel(38), Direction::Down, 0, pages.board()),
        NowMove::Focus(NowFocus::Channel(40)),
    );
    let result = core.guide_window(replacement.query().clone());
    assert_eq!(pages.complete(replacement, result), PageUpdate::Applied);
    assert_eq!(pages.displayed_window(), earlier);
    assert_eq!(pages.displayed_status(), PageStatus::Complete);
    assert_eq!(pages.families().len(), 41);
    assert!(pages.needs_automatic_page());
    apply_next(&mut pages, core);
    assert_eq!(pages.displayed_window(), later);
    assert_eq!(pages.families().len(), 41);
    assert!(!pages.is_replacing_window());

    pages.select(generation, None, earlier);
    apply_next(&mut pages, core);
    assert_eq!(pages.displayed_window(), later);
    let continuation = pages.request_next().unwrap();
    assert_eq!(
        pages.complete(continuation, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(pages.displayed_window(), earlier);
    assert_eq!(pages.families().len(), 40);
    assert_eq!(pages.displayed_status(), PageStatus::Failed);

    pages.select(generation, None, later);
    assert_eq!(pages.displayed_window(), earlier);
    assert_eq!(pages.displayed_status(), PageStatus::Loading);
    assert!(pages.needs_automatic_page());
}

#[tokio::test]
async fn drops_obsolete_requests_and_never_shows_another_generation() {
    let fixture = support::open(split_family_m3u(), None).await;
    let core = fixture.core();
    let old_generation = generation(core);
    let [earlier, later] = windows();
    let mut pages = GuidePages::new(old_generation, None, earlier, HashSet::new());
    let original = pages.request_next().unwrap();
    pages.select(old_generation, None, later);
    pages.select(old_generation, None, earlier);
    let current = pages.request_next().unwrap();
    let old_result = core.guide_window(original.query().clone());
    assert_eq!(pages.complete(original, old_result), PageUpdate::Superseded);
    assert_eq!(pages.status(), PageStatus::Loading);
    let result = core.guide_window(current.query().clone());
    assert_eq!(pages.complete(current, result), PageUpdate::Applied);

    let changed = support::open(
        "#EXTM3U\n#EXTINF:-1,Replacement\nhttps://fixture.invalid/replaced.ts\n".into(),
        None,
    )
    .await;
    let changed_generation = generation(changed.core());
    assert_ne!(old_generation, changed_generation);
    let mut groups = ChannelGroupPages::new(old_generation);
    let old_group_request = groups.request_next().unwrap();
    groups.select(changed_generation);
    let old_groups = core.list_groups(old_group_request.query().clone());
    assert_eq!(
        groups.complete(old_group_request, old_groups),
        PageUpdate::Superseded
    );
    let continuation = pages.request_next().unwrap();
    let stale = changed.core().guide_window(continuation.query().clone());
    assert!(matches!(stale, Err(CoreError::StaleCursor { .. })));
    assert_eq!(
        pages.complete(continuation, stale),
        PageUpdate::ReconcileGeneration
    );
    assert!(
        pages.retry().is_none(),
        "a rejected cursor cannot be retried before status reconciliation"
    );
    pages.select(changed_generation, None, earlier);
    assert!(pages.families().is_empty());
    let request = pages.request_next().unwrap();
    let wrong_result = core.guide_window(request.query().clone());
    assert_eq!(
        pages.complete(request, wrong_result),
        PageUpdate::ReconcileGeneration
    );
    assert!(pages.families().is_empty());
    assert_eq!(pages.status(), PageStatus::ReconcileGeneration);
    assert!(pages.retry().is_none());
    pages.select(changed_generation, None, earlier);
    let retry = pages.request_next().unwrap();
    let refreshed = changed.core().guide_window(retry.query().clone());
    assert_eq!(pages.complete(retry, refreshed), PageUpdate::Applied);
    assert_eq!(pages.families()[0].title(), "Replacement");
}

#[tokio::test]
async fn keeps_all_distinct_from_ungrouped_and_skips_excluded_only_pages() {
    let mut playlist = "#EXTM3U\n".to_owned();
    for index in 0..40 {
        writeln!(playlist, "#EXTINF:-1 group-title=\"Hidden\",Hidden Channel {index}\nhttps://fixture.invalid/hidden/{index}.ts").unwrap();
    }
    writeln!(
        playlist,
        "#EXTINF:-1,Ungrouped Channel\nhttps://fixture.invalid/ungrouped.ts"
    )
    .unwrap();
    for index in (1..=100).rev() {
        writeln!(playlist, "#EXTINF:-1 group-title=\"Group {index:03}\",Channel {index}\nhttps://fixture.invalid/{index}.ts").unwrap();
    }
    let fixture = support::open(playlist, None).await;
    let core = fixture.core();
    let generation = generation(core);
    let window = windows()[0];
    let hidden = HashSet::from(["Hidden".to_owned()]);
    let mut pages = GuidePages::new(generation, None, window, hidden);
    apply_next(&mut pages, core);
    assert!(pages.families().is_empty());
    assert!(pages.needs_automatic_page());
    apply_next(&mut pages, core);
    assert_eq!(pages.families()[0].title(), "Ungrouped Channel");
    assert!(!pages.needs_automatic_page());

    pages.select(
        generation,
        Some(ChannelGroupFilter::parse("").unwrap()),
        window,
    );
    let ungrouped = pages.request_next().unwrap();
    assert_eq!(pages.families().len(), 1);
    assert_eq!(pages.families()[0].title(), "Ungrouped Channel");
    assert_eq!(ungrouped.query().channels().group(), Some(""));
    let result = core.guide_window(ungrouped.query().clone());
    assert_eq!(pages.complete(ungrouped, result), PageUpdate::Applied);
    assert_eq!(pages.families().len(), 1);

    let mut groups = ChannelGroupPages::new(generation);
    let request = groups.request_next().unwrap();
    assert_eq!(request.query().limit().get(), 100);
    let result = core.list_groups(request.query().clone());
    assert_eq!(groups.complete(request, result), PageUpdate::Applied);
    let failed_request = groups.request_next().unwrap();
    let page = failed_request.query().clone();
    assert_eq!(page.limit().get(), 100);
    assert_eq!(
        groups.complete(failed_request, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(groups.groups().len(), 100);
    let retry = groups.retry().unwrap();
    assert_eq!(*retry.query(), page);
    let result = core.list_groups(retry.query().clone());
    assert_eq!(groups.complete(retry, result), PageUpdate::Applied);
    assert_eq!(groups.status(), PageStatus::Complete);
    assert_eq!(groups.groups().len(), 102);
    assert_eq!(groups.groups()[0].name(), "Hidden");
    assert_eq!(groups.groups()[1].name(), "");
    assert_eq!(groups.groups()[2].name(), "Group 100");
    assert_eq!(groups.groups()[101].name(), "Group 001");
}

fn generation(core: &CatalogAccess) -> sparrow_core::CatalogGeneration {
    core.status().generation().unwrap()
}

#[tokio::test]
async fn replacement_pager_ignores_an_in_flight_read_from_the_old_owner() {
    let mut playlist = split_family_m3u();
    playlist.push_str(
        "#EXTINF:-1 group-title=\"World\",World News\nhttps://fixture.invalid/world/news.ts\n",
    );
    let fixture = support::open(playlist, None).await;
    let core = fixture.core();
    let generation = generation(core);
    let window = windows()[0];
    let exact_group = ChannelGroupFilter::parse("World").unwrap();
    let mut old = GuidePages::new(generation, Some(exact_group), window, HashSet::new());
    let stale_request = old.request_next().unwrap();
    drop(old);

    let mut replacement = GuidePages::new(generation, None, window, HashSet::new());
    let live_request = replacement.request_next().unwrap();
    let stale_result = core.guide_window(stale_request.query().clone());
    assert_eq!(
        replacement.complete(stale_request, stale_result),
        PageUpdate::Superseded
    );
    assert!(replacement.families().is_empty());
    let live_result = core.guide_window(live_request.query().clone());
    assert_eq!(
        replacement.complete(live_request, live_result),
        PageUpdate::Applied
    );
    assert_eq!(replacement.families()[0].title(), "Fixture Channel 1");
}

fn apply_next(pages: &mut GuidePages, core: &CatalogAccess) {
    let request = pages.request_next().unwrap();
    let result = core.guide_window(request.query().clone());
    assert_eq!(pages.complete(request, result), PageUpdate::Applied);
}

fn windows() -> [GuideWindow; 2] {
    let first = Utc.with_ymd_and_hms(2026, 11, 24, 9, 12, 0).unwrap();
    [first, first + Duration::minutes(30)]
        .map(|instant| GuideWindow::at(instant, FixedOffset::east_opt(0).unwrap(), false))
}

fn split_family_m3u() -> String {
    let mut playlist = "#EXTM3U\n".to_owned();
    for index in 1..40 {
        writeln!(playlist, "#EXTINF:-1 group-title=\"Local\",Fixture Channel {index}\nhttps://fixture.invalid/local/{index}.ts").unwrap();
    }
    playlist.push_str(
        "#EXTINF:-1 group-title=\"Local\",City News SD\nhttps://fixture.invalid/local/news-sd.ts\n\
         #EXTINF:-1 group-title=\"Local\",City News HD\nhttps://fixture.invalid/local/news-hd.ts\n\
         #EXTINF:-1 group-title=\"Local\",Weekend Review\nhttps://fixture.invalid/local/review.ts\n",
    );
    playlist
}
