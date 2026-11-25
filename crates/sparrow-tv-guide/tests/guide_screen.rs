mod support;

use std::{collections::HashSet, fmt::Write};

use chrono::{DateTime, FixedOffset, TimeDelta, TimeZone, Utc};
use sparrow_core::{CatalogAccess, CoreError};
use sparrow_tv_guide::{
    GuidePageRequest, GuideWindow, PageUpdate,
    guide_screen::{
        GuideMode, GuideRead, GuideScreen, LaneFocus, ScreenAction, ScreenFocus, ScreenKey,
        ScreenNotice, ToolbarFocus,
    },
    now_focus::{Direction, NowFocus},
    timeline_focus::TimelineFocus,
};

#[tokio::test]
async fn focus_reveals_only_the_pixels_needed_to_clear_the_bottom_margin() {
    let fixture = support::open(catalog(30), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    screen.set_surface_height(1080.);

    for _ in 0..4 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(screen.focus(), channel(12));
    // A 558-unit board leaves the fifth row 10 units from its bottom edge.
    // The 24-unit focus margin requires a 14-unit scroll, exposing part of row six.
    assert_eq!(screen.scroll_offset(), 14.);
    assert_eq!(screen.visible_families(0), 0..18);

    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.scroll_offset(), 14.);
}

#[tokio::test]
async fn near_tail_prefetch_and_held_down_keep_navigation_and_viewport_in_sync() {
    let fixture = support::open(catalog(160), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    screen.set_surface_height(962.);
    assert_eq!(screen.families().len(), 40);
    assert_eq!(screen.focus(), channel(0));

    for _ in 0..10 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(screen.focus(), channel(30));
    assert!(screen.take_reads().is_empty());
    arrow(&mut screen, Direction::Down);
    let continuation = guide_request(&mut screen);
    assert!(continuation.query().channels().page().cursor().is_some());

    for _ in 0..12 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(screen.focus(), channel(39));
    assert_eq!(screen.first_visible_row(), 10);
    assert!(screen.take_reads().is_empty());

    complete_guide(&mut screen, fixture.core(), continuation);
    assert_eq!(screen.focus(), channel(63));
    assert_eq!(screen.first_visible_row(), 18);
    assert_eq!(screen.visible_families(2), 48..75);
    assert!(screen.take_reads().is_empty());
}

#[tokio::test]
async fn group_focus_guards_prefetch_and_activation_reuses_the_loaded_group() {
    let fixture = support::open(catalog(160), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    for _ in 0..11 {
        arrow(&mut screen, Direction::Down);
    }
    let continuation = guide_request(&mut screen);
    complete_guide(&mut screen, core, continuation);
    assert_eq!(screen.families().len(), 140);
    for _ in 0..12 {
        arrow(&mut screen, Direction::Up);
    }
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.focus(), ScreenFocus::Toolbar(ToolbarFocus::Sources));
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::Toolbar(ToolbarFocus::Search));
    assert_eq!(activate(&mut screen), ScreenAction::Unavailable);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    arrow(&mut screen, Direction::Right);

    let ungrouped = screen
        .groups()
        .iter()
        .position(|group| group.name().is_empty())
        .unwrap();
    for _ in 0..=ungrouped {
        arrow(&mut screen, Direction::Right);
    }
    assert_eq!(
        screen.focus(),
        ScreenFocus::GroupLane(LaneFocus::Group(ungrouped))
    );
    assert!(screen.selected_group().is_none());
    let stale_ticket = screen.prefetch_ticket().unwrap();
    arrow(&mut screen, Direction::Left);
    arrow(&mut screen, Direction::Right);
    screen.prefetch(stale_ticket);
    assert!(screen.take_reads().is_empty());

    screen.prefetch(screen.prefetch_ticket().unwrap());
    let prefetched = guide_request(&mut screen);
    assert_eq!(prefetched.query().channels().group(), Some(""));
    complete_guide(&mut screen, core, prefetched);
    assert!(screen.selected_group().is_none());
    assert_eq!(screen.families().len(), 140);

    activate(&mut screen);
    assert_eq!(
        screen.selected_group().map(|group| group.as_str()),
        Some("")
    );
    assert_eq!(screen.families().len(), 40);
    assert!(
        screen
            .families()
            .iter()
            .all(|family| family.group().is_empty())
    );
    assert!(screen.take_reads().is_empty());

    for _ in 0..=ungrouped {
        arrow(&mut screen, Direction::Left);
    }
    activate(&mut screen);
    assert!(screen.selected_group().is_none());
    assert_eq!(screen.families().len(), 140);
    assert!(screen.take_reads().is_empty());
}

#[tokio::test]
async fn displays_compatible_rows_while_the_selected_group_loads() {
    let fixture = support::open(catalog(160), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    arrow(&mut screen, Direction::Up);
    arrow(&mut screen, Direction::Right);
    let first_group = screen.groups()[0].name().to_owned();
    activate(&mut screen);
    assert_eq!(screen.selected_group().unwrap().as_str(), first_group);
    let request = guide_request(&mut screen);
    assert_eq!(
        request.query().channels().group(),
        Some(first_group.as_str())
    );
    assert!(!screen.families().is_empty());
    assert!(
        screen
            .families()
            .iter()
            .all(|family| family.group() == first_group)
    );
    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), channel(0));
    complete_guide(&mut screen, fixture.core(), request);
    assert_eq!(screen.focus(), channel(0));
}

#[tokio::test]
async fn replacing_retained_rows_keeps_quality_focus_on_the_same_channel() {
    let fixture = support::open(three_groups_with_variants(), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());

    arrow(&mut screen, Direction::Up);
    arrow(&mut screen, Direction::Right);
    arrow(&mut screen, Direction::Right);
    activate(&mut screen);
    finish_reads(&mut screen, core);

    let next = Utc.with_ymd_and_hms(2026, 1, 9, 12, 31, 0).unwrap();
    screen.set_clock(next, local_offset());
    finish_reads(&mut screen, core);

    arrow(&mut screen, Direction::Left);
    arrow(&mut screen, Direction::Left);
    activate(&mut screen);
    let all = guide_request(&mut screen);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Right);
    assert_eq!(
        screen.focus(),
        ScreenFocus::Board(NowFocus::Variant { family: 0, chip: 0 })
    );
    let focused_number = screen.families()[0].number();

    complete_guide(&mut screen, core, all);
    assert_eq!(screen.families()[0].title(), "Fixture One");
    assert_eq!(
        screen.focus(),
        ScreenFocus::Board(NowFocus::Variant { family: 1, chip: 0 })
    );
    assert_eq!(screen.preview_family().unwrap().number(), focused_number);
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), channel(1));
}

#[tokio::test]
async fn successive_pending_group_selections_use_the_last_completed_rows() {
    let fixture = support::open(three_groups_with_variants(), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    arrow(&mut screen, Direction::Up);
    arrow(&mut screen, Direction::Right);
    arrow(&mut screen, Direction::Right);
    activate(&mut screen);
    assert_eq!(screen.families()[0].title(), "Fixture Two");
    let _pending = guide_request(&mut screen);

    arrow(&mut screen, Direction::Right);
    activate(&mut screen);
    assert_eq!(screen.selected_group().unwrap().as_str(), "C");
    assert_eq!(
        screen.families().first().map(|family| family.title()),
        Some("Fixture Three")
    );
}

fn three_groups_with_variants() -> String {
    concat!(
        "#EXTM3U\n",
        "#EXTINF:-1 group-title=\"A\",Fixture One\nhttps://fixture.invalid/one.ts\n",
        "#EXTINF:-1 group-title=\"B\",Fixture Two SD\nhttps://fixture.invalid/two-sd.ts\n",
        "#EXTINF:-1 group-title=\"B\",Fixture Two HD\nhttps://fixture.invalid/two-hd.ts\n",
        "#EXTINF:-1 group-title=\"C\",Fixture Three\nhttps://fixture.invalid/three.ts\n",
    )
    .to_owned()
}

#[tokio::test]
async fn preview_keeps_the_last_board_channel_until_the_group_excludes_it() {
    let fixture = support::open(catalog(20), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    arrow(&mut screen, Direction::Right);
    let preview = screen.preview_family().unwrap();
    assert_eq!(preview.number(), screen.families()[1].number());
    let preview_number = preview.number();
    let other_group = screen
        .groups()
        .iter()
        .position(|group| group.name() != preview.group())
        .unwrap();

    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.preview_family().unwrap().number(), preview_number);
    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.preview_family().unwrap().number(), preview_number);

    arrow(&mut screen, Direction::Left);
    arrow(&mut screen, Direction::Down);
    for _ in 0..=other_group {
        arrow(&mut screen, Direction::Right);
    }
    activate(&mut screen);
    finish_reads(&mut screen, fixture.core());
    assert_eq!(
        screen.preview_family().unwrap().number(),
        screen.families()[0].number()
    );
    assert_ne!(screen.preview_family().unwrap().number(), preview_number);
}

#[tokio::test]
async fn retries_a_failed_read_and_reconciles_a_cursor_rejected_by_core() {
    let fixture = support::open(catalog(160), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    screen.set_surface_height(1080.);
    for _ in 0..14 {
        arrow(&mut screen, Direction::Down);
    }
    let request = guide_request(&mut screen);
    assert_eq!(
        screen.guide_completed(request, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(screen.focus(), ScreenFocus::Board(NowFocus::Retry));
    assert!(screen.retry_available());
    assert!(screen.visible_families(0).contains(&39));
    assert_eq!(screen.footer_top(), 456.);

    activate(&mut screen);
    let retry = guide_request(&mut screen);
    let replacement = support::open(catalog(10), None).await;
    let result = replacement.core().guide_window(retry.query().clone());
    assert!(matches!(result, Err(CoreError::StaleCursor { .. })));
    assert_eq!(
        screen.guide_completed(retry, result),
        PageUpdate::ReconcileGeneration
    );
    assert!(matches!(&screen.take_reads()[..], [GuideRead::Status]));

    screen.catalog_status(&replacement.core().status());
    assert!(screen.families().is_empty());
    finish_reads(&mut screen, replacement.core());
    assert_eq!(screen.families().len(), 10);
    assert_eq!(screen.focus(), channel(0));
    assert!(!screen.retry_available());
}

#[tokio::test]
async fn clock_rollover_returns_focused_retry_to_the_last_retained_channel() {
    let fixture = support::open(catalog(50), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    for _ in 0..14 {
        arrow(&mut screen, Direction::Down);
    }
    let failed = guide_request(&mut screen);
    assert_eq!(
        screen.guide_completed(failed, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(screen.focus(), ScreenFocus::Board(NowFocus::Retry));

    let old_window = screen.displayed_window();
    let next_window = GuideWindow::at(
        old_window.starts_at() + chrono::TimeDelta::minutes(30),
        FixedOffset::east_opt(0).unwrap(),
        false,
    );
    screen.set_clock(next_window.starts_at(), local_offset());
    assert_eq!(screen.focus(), channel(39));
    assert_eq!(screen.displayed_window(), old_window);
    assert!(!screen.retry_available());

    let catch_up = guide_request(&mut screen);
    complete_guide(&mut screen, fixture.core(), catch_up);
    assert_eq!(screen.focus(), channel(39));
    assert_eq!(screen.displayed_window(), next_window);
}

#[tokio::test]
async fn hidden_groups_produce_a_distinct_notice_without_a_phantom_board_target() {
    let fixture = support::open(catalog(160), None).await;
    let exclusions = ["Local", "World", ""]
        .map(str::to_owned)
        .into_iter()
        .collect();
    let mut screen = open_screen(fixture.core(), exclusions);
    assert_eq!(screen.notice(), Some(ScreenNotice::AllGroupsHidden));
    assert!(screen.groups().is_empty());
    assert!(screen.families().is_empty());
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));

    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    activate(&mut screen);
    assert_eq!(screen.mode(), GuideMode::Timeline);
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Down);
    assert_eq!(
        screen.focus(),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
    );
    arrow(&mut screen, Direction::Right);
    assert_eq!(
        screen.focus(),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
    );
    for _ in 0..3 {
        arrow(&mut screen, Direction::Up);
    }
    assert_eq!(screen.focus(), ScreenFocus::Toolbar(ToolbarFocus::Sources));
    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
}

#[tokio::test]
async fn timeline_rail_waits_for_empty_loading_board_and_toolbar_can_reach_retry() {
    let fixture = support::open(three_groups_with_variants(), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Down);
    activate(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Down);
    activate(&mut screen);
    assert_eq!(screen.selected_group().unwrap().as_str(), "B");
    assert_eq!(screen.notice(), Some(ScreenNotice::Loading));
    let loading = guide_request(&mut screen);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Down);
    assert_eq!(
        screen.focus(),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
    );
    arrow(&mut screen, Direction::Right);
    assert_eq!(
        screen.focus(),
        ScreenFocus::GroupLane(LaneFocus::ChooseGroups)
    );

    assert_eq!(
        screen.guide_completed(loading, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    for _ in 0..6 {
        arrow(&mut screen, Direction::Up);
    }
    assert_eq!(screen.focus(), ScreenFocus::Toolbar(ToolbarFocus::Sources));
    arrow(&mut screen, Direction::Down);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Retry)
    );
}

#[tokio::test]
async fn timeline_toggle_extends_the_query_once_and_keeps_the_same_focus() {
    let fixture = support::open(catalog(25), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    let first = screen.displayed_window();
    assert_eq!(first.ends_at() - first.starts_at(), TimeDelta::hours(3));
    assert_eq!(screen.mode(), GuideMode::Now);

    open_timeline(&mut screen);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    assert_eq!(screen.timeline().unwrap().starts_at(), first.starts_at());
    let expanded = guide_request(&mut screen);
    assert_eq!(expanded.query().starts_at(), first.starts_at());
    assert_eq!(
        expanded.query().ends_at() - expanded.query().starts_at(),
        TimeDelta::hours(8)
    );
    assert_eq!(screen.displayed_window(), first);
    complete_guide(&mut screen, core, expanded);

    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
    arrow(&mut screen, Direction::Right);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(0))
    );
    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    activate(&mut screen);
    assert_eq!(screen.mode(), GuideMode::Now);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    assert!(screen.take_reads().is_empty());

    let now = clock() + TimeDelta::minutes(30);
    screen.set_clock(now, local_offset());
    let rollover = guide_request(&mut screen);
    assert_eq!(
        rollover.query().ends_at() - rollover.query().starts_at(),
        TimeDelta::hours(8)
    );
    complete_guide(&mut screen, core, rollover);
    screen.press(ScreenKey::Activate, now, local_offset());
    assert_eq!(
        screen.timeline().unwrap().starts_at(),
        screen.displayed_window().starts_at()
    );
    assert!(screen.take_reads().is_empty());
}

#[tokio::test]
async fn timeline_keeps_programme_preview_and_returns_through_the_rail_and_toolbar() {
    let fixture = support::open(timeline_catalog(), Some(timeline_epg())).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 0));
    let family = screen.preview_family().unwrap().number();
    assert_eq!(screen.preview_cell(), Some(0));

    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 1));
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 2));
    assert!(screen.timeline().unwrap().starts_at() > screen.displayed_window().starts_at());
    assert!(screen.take_reads().is_empty());
    assert_eq!(screen.preview_cell(), Some(2));
    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    arrow(&mut screen, Direction::Up);
    assert_eq!(screen.focus(), ScreenFocus::Toolbar(ToolbarFocus::Sources));
    assert_eq!(screen.preview_family().unwrap().number(), family);
    assert_eq!(screen.preview_cell(), Some(2));
    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), programme(0, 2));
    arrow(&mut screen, Direction::Down);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(1))
    );
    assert_eq!(screen.preview_cell(), None);
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 2));

    arrow(&mut screen, Direction::Up);
    activate(&mut screen);
    assert_eq!(screen.preview_cell(), Some(2));
    activate(&mut screen);
    assert_eq!(screen.preview_cell(), Some(2));
    assert_eq!(
        screen.timeline().unwrap().starts_at(),
        screen.displayed_window().starts_at()
    );
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 0));
    arrow(&mut screen, Direction::Left);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Variant { family: 0, chip: 1 })
    );
    arrow(&mut screen, Direction::Left);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Variant { family: 0, chip: 0 })
    );
    arrow(&mut screen, Direction::Right);
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 0));
}

#[tokio::test]
async fn timeline_restores_the_next_programme_only_after_the_displayed_window_changes() {
    let fixture = support::open(timeline_catalog(), Some(timeline_epg())).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Right);
    assert_eq!(screen.focus(), programme(0, 0));
    let old_window = screen.displayed_window();
    let old_slot = screen.preview_family().unwrap().variants()[0].programmes()[0].starts_at();

    let next = clock() + TimeDelta::minutes(30);
    screen.set_clock(next, local_offset());
    assert_eq!(screen.displayed_window(), old_window);
    assert_eq!(screen.focus(), programme(0, 0));
    let rollover = guide_request(&mut screen);
    complete_guide(&mut screen, core, rollover);
    assert_eq!(screen.focus(), programme(0, 0));
    assert_eq!(screen.preview_cell(), Some(0));
    let restored = screen.preview_family().unwrap().variants()[0].programmes()[0].starts_at();
    assert_eq!(restored - old_slot, TimeDelta::minutes(30));
    assert_eq!(
        screen.timeline().unwrap().starts_at(),
        screen.displayed_window().starts_at()
    );
}

#[tokio::test]
async fn timeline_and_now_preserve_scroll_and_use_the_target_board_rows() {
    let fixture = support::open(catalog(35), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    screen.set_surface_height(1080.);
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Right);
    for _ in 0..12 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(12))
    );
    assert_eq!(screen.scroll_offset(), 515.);
    assert_eq!(screen.first_visible_row(), 6);
    assert_eq!(screen.visible_families(0), 6..14);
    assert_eq!(screen.visible_families(2), 4..16);
    arrow(&mut screen, Direction::Left);
    arrow(&mut screen, Direction::Up);
    activate(&mut screen);
    assert_eq!(screen.mode(), GuideMode::Now);
    assert_eq!(screen.scroll_offset(), 515.);
    assert_eq!(screen.first_visible_row(), 4);
    activate(&mut screen);
    assert_eq!(screen.mode(), GuideMode::Timeline);
    assert_eq!(screen.scroll_offset(), 515.);
    assert_eq!(screen.first_visible_row(), 6);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Right);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(5))
    );
    assert_eq!(screen.scroll_offset(), 391.);

    for _ in 5..12 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(screen.scroll_offset(), 515.);
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::All));
    activate(&mut screen);
    assert_eq!(screen.scroll_offset(), 515.);
    arrow(&mut screen, Direction::Down);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Group(0)));
    activate(&mut screen);
    assert!(screen.selected_group().is_some());
    assert!(!screen.families().is_empty());
    assert_eq!(screen.scroll_offset(), 0.);
    assert_eq!(screen.first_visible_row(), 0);
    finish_reads(&mut screen, core);
    assert_eq!(screen.scroll_offset(), 0.);
}

#[tokio::test]
async fn timeline_deferred_down_reaches_retry_then_settles_over_the_next_page() {
    let fixture = support::open(catalog(60), None).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    screen.set_surface_height(1080.);
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Right);
    for _ in 0..41 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(39))
    );
    let pending = guide_request(&mut screen);
    assert_eq!(
        screen.guide_completed(pending, Err(CoreError::NotConfigured)),
        PageUpdate::Failed
    );
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Retry)
    );
    assert!(screen.retry_available());
    assert!(screen.visible_families(0).contains(&39));
    activate(&mut screen);
    let retry = guide_request(&mut screen);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Down);
    complete_guide(&mut screen, core, retry);
    assert_eq!(
        screen.focus(),
        ScreenFocus::TimelineBoard(TimelineFocus::Channel(41))
    );
    assert!(!screen.retry_available());
}

#[tokio::test]
async fn timeline_deferred_down_tracks_the_same_programme_across_a_split_family() {
    let screen = complete_split_family_down(
        r#"<programme channel="hd" start="20260109120000 +0000" stop="20260109123000 +0000"><title>Earlier</title></programme>
           <programme channel="hd" start="20260109123000 +0000" stop="20260109130000 +0000"><title>Origin</title></programme>"#,
    )
    .await;
    assert_eq!(screen.focus(), programme(40, 1));
}

#[tokio::test]
async fn timeline_deferred_down_cancels_when_a_successor_takes_the_programme_index() {
    let screen = complete_split_family_down(
        r#"<programme channel="hd" start="20260109130000 +0000" stop="20260109133000 +0000"><title>Successor</title></programme>"#,
    )
    .await;
    assert_eq!(screen.focus(), programme(39, 0));
}

async fn complete_split_family_down(hd_programmes: &str) -> GuideScreen {
    let mut m3u = catalog(39);
    m3u.push_str(
        r#"#EXTINF:-1 tvg-id="sd" group-title="Guide",City News SD
https://fixture.invalid/news-sd.ts
#EXTINF:-1 tvg-id="hd" group-title="Guide",City News HD
https://fixture.invalid/news-hd.ts
#EXTINF:-1 tvg-id="next" group-title="Guide",Next Channel
https://fixture.invalid/next.ts
"#,
    );
    let epg = format!(
        r#"<tv>
<channel id="sd"><display-name>City News SD</display-name></channel>
<channel id="hd"><display-name>City News HD</display-name></channel>
<channel id="next"><display-name>Next Channel</display-name></channel>
<programme channel="sd" start="20260109123000 +0000" stop="20260109130000 +0000"><title>Origin</title></programme>
{hd_programmes}
<programme channel="next" start="20260109120000 +0000" stop="20260109123000 +0000"><title>First</title></programme>
<programme channel="next" start="20260109123000 +0000" stop="20260109130000 +0000"><title>Matching time</title></programme>
</tv>"#,
    );
    let fixture = support::open(m3u, Some(epg)).await;
    let core = fixture.core();
    let mut screen = open_screen(core, HashSet::new());
    screen.set_surface_height(1080.);
    open_timeline(&mut screen);
    finish_reads(&mut screen, core);
    arrow(&mut screen, Direction::Right);
    for _ in 0..39 {
        arrow(&mut screen, Direction::Down);
    }
    assert_eq!(screen.focus(), programme(39, 0));
    assert_eq!(screen.families()[39].variants().len(), 1);
    let continuation = guide_request(&mut screen);
    arrow(&mut screen, Direction::Down);
    complete_guide(&mut screen, core, continuation);
    assert_eq!(screen.families()[39].variants().len(), 2);
    screen
}

fn open_timeline(screen: &mut GuideScreen) {
    arrow(screen, Direction::Up);
    arrow(screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    assert_eq!(activate(screen), ScreenAction::None);
    assert_eq!(screen.mode(), GuideMode::Timeline);
}

fn programme(family: usize, cell: usize) -> ScreenFocus {
    ScreenFocus::TimelineBoard(TimelineFocus::Programme { family, cell })
}

fn timeline_catalog() -> String {
    r#"#EXTM3U
#EXTINF:-1 tvg-id="one" group-title="Guide",One HD
https://fixture.invalid/one-hd.ts
#EXTINF:-1 tvg-id="one" group-title="Guide",One FHD
https://fixture.invalid/one-fhd.ts
#EXTINF:-1 group-title="Empty",Two
https://fixture.invalid/two.ts
"#
    .into()
}

fn timeline_epg() -> String {
    r#"<?xml version="1.0"?>
<tv>
  <channel id="one"><display-name>One</display-name></channel>
  <programme channel="one" start="20260109120000 +0000" stop="20260109123000 +0000"><title>Morning</title></programme>
  <programme channel="one" start="20260109123000 +0000" stop="20260109130000 +0000"><title>Midday</title></programme>
  <programme channel="one" start="20260109135500 +0000" stop="20260109142000 +0000"><title>Later</title></programme>
</tv>"#.into()
}

fn catalog(count: usize) -> String {
    let mut m3u = "#EXTM3U\n".to_owned();
    for channel in 1..=count {
        let group = ["Local", "World", ""][channel % 3];
        writeln!(m3u, "#EXTINF:-1 group-title=\"{group}\",Fixture {channel}\nhttps://fixture.invalid/{channel}.ts").unwrap();
    }
    m3u
}

fn open_screen(core: &CatalogAccess, exclusions: HashSet<String>) -> GuideScreen {
    let mut screen = GuideScreen::new(clock(), local_offset(), exclusions);
    screen.catalog_status(&core.status());
    finish_reads(&mut screen, core);
    screen
}

fn finish_reads(screen: &mut GuideScreen, core: &CatalogAccess) {
    loop {
        let reads = screen.take_reads();
        if reads.is_empty() {
            return;
        }
        for read in reads {
            match read {
                GuideRead::Status => screen.catalog_status(&core.status()),
                GuideRead::Groups(request) => {
                    let result = core.list_groups(request.query().clone());
                    assert_eq!(
                        screen.groups_completed(request, result),
                        PageUpdate::Applied
                    );
                }
                GuideRead::Guide(request) => complete_guide(screen, core, request),
            }
        }
    }
}

fn guide_request(screen: &mut GuideScreen) -> GuidePageRequest {
    let mut reads = screen.take_reads();
    assert_eq!(reads.len(), 1);
    let Some(GuideRead::Guide(request)) = reads.pop() else {
        panic!("expected a Guide page read")
    };
    request
}

fn complete_guide(screen: &mut GuideScreen, core: &CatalogAccess, request: GuidePageRequest) {
    let result = core.guide_window(request.query().clone());
    assert_eq!(screen.guide_completed(request, result), PageUpdate::Applied);
}

fn channel(index: usize) -> ScreenFocus {
    ScreenFocus::Board(NowFocus::Channel(index))
}

fn arrow(screen: &mut GuideScreen, direction: Direction) {
    screen.press(ScreenKey::Arrow(direction), clock(), local_offset());
}

fn activate(screen: &mut GuideScreen) -> ScreenAction {
    screen.press(ScreenKey::Activate, clock(), local_offset())
}

fn clock() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 9, 12, 1, 0).unwrap()
}

fn local_offset() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}
