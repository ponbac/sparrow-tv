mod support;

use std::{collections::HashSet, fmt::Write};

use chrono::{FixedOffset, TimeZone, Utc};
use sparrow_core::{CatalogAccess, CoreError};
use sparrow_tv_guide::{
    GuidePageRequest, GuideWindow, PageUpdate,
    now_focus::{Direction, NowFocus},
    now_screen::{
        LaneFocus, NowRead, NowScreen, ScreenAction, ScreenFocus, ScreenKey, ScreenNotice,
        ToolbarFocus,
    },
};

#[tokio::test]
async fn focus_reveals_only_the_pixels_needed_to_clear_the_bottom_margin() {
    let fixture = support::open(catalog(30), None).await;
    let mut screen = open_screen(fixture.core(), HashSet::new());
    screen.set_viewport_height(558.);

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
    screen.set_viewport_height(440.);
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
    assert_eq!(screen.press(ScreenKey::Activate), ScreenAction::Unavailable);
    arrow(&mut screen, Direction::Down);
    arrow(&mut screen, Direction::Left);
    assert_eq!(screen.focus(), ScreenFocus::GroupLane(LaneFocus::Phase));
    assert_eq!(
        screen.press(ScreenKey::Activate),
        ScreenAction::OpenTimeline
    );
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

    screen.press(ScreenKey::Activate);
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
    screen.press(ScreenKey::Activate);
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
    screen.press(ScreenKey::Activate);
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
    screen.press(ScreenKey::Activate);
    finish_reads(&mut screen, core);

    let next = Utc.with_ymd_and_hms(2026, 1, 9, 12, 31, 0).unwrap();
    screen.set_window(GuideWindow::at(
        next,
        FixedOffset::east_opt(0).unwrap(),
        false,
    ));
    finish_reads(&mut screen, core);

    arrow(&mut screen, Direction::Left);
    arrow(&mut screen, Direction::Left);
    screen.press(ScreenKey::Activate);
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
    screen.press(ScreenKey::Activate);
    assert_eq!(screen.families()[0].title(), "Fixture Two");
    let _pending = guide_request(&mut screen);

    arrow(&mut screen, Direction::Right);
    screen.press(ScreenKey::Activate);
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
    screen.press(ScreenKey::Activate);
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
    screen.set_viewport_height(558.);
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

    screen.press(ScreenKey::Activate);
    let retry = guide_request(&mut screen);
    let replacement = support::open(catalog(10), None).await;
    let result = replacement.core().guide_window(retry.query().clone());
    assert!(matches!(result, Err(CoreError::StaleCursor { .. })));
    assert_eq!(
        screen.guide_completed(retry, result),
        PageUpdate::ReconcileGeneration
    );
    assert!(matches!(&screen.take_reads()[..], [NowRead::Status]));

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
    screen.set_window(next_window);
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
}

fn catalog(count: usize) -> String {
    let mut m3u = "#EXTM3U\n".to_owned();
    for channel in 1..=count {
        let group = ["Local", "World", ""][channel % 3];
        writeln!(m3u, "#EXTINF:-1 group-title=\"{group}\",Fixture {channel}\nhttps://fixture.invalid/{channel}.ts").unwrap();
    }
    m3u
}

fn open_screen(core: &CatalogAccess, exclusions: HashSet<String>) -> NowScreen {
    let instant = Utc.with_ymd_and_hms(2026, 1, 9, 12, 1, 0).unwrap();
    let window = GuideWindow::at(instant, FixedOffset::east_opt(0).unwrap(), false);
    let mut screen = NowScreen::new(window, exclusions);
    screen.catalog_status(&core.status());
    finish_reads(&mut screen, core);
    screen
}

fn finish_reads(screen: &mut NowScreen, core: &CatalogAccess) {
    loop {
        let reads = screen.take_reads();
        if reads.is_empty() {
            return;
        }
        for read in reads {
            match read {
                NowRead::Status => screen.catalog_status(&core.status()),
                NowRead::Groups(request) => {
                    let result = core.list_groups(request.query().clone());
                    assert_eq!(
                        screen.groups_completed(request, result),
                        PageUpdate::Applied
                    );
                }
                NowRead::Guide(request) => complete_guide(screen, core, request),
            }
        }
    }
}

fn guide_request(screen: &mut NowScreen) -> GuidePageRequest {
    let mut reads = screen.take_reads();
    assert_eq!(reads.len(), 1);
    let Some(NowRead::Guide(request)) = reads.pop() else {
        panic!("expected a Guide page read")
    };
    request
}

fn complete_guide(screen: &mut NowScreen, core: &CatalogAccess, request: GuidePageRequest) {
    let result = core.guide_window(request.query().clone());
    assert_eq!(screen.guide_completed(request, result), PageUpdate::Applied);
}

fn channel(index: usize) -> ScreenFocus {
    ScreenFocus::Board(NowFocus::Channel(index))
}

fn arrow(screen: &mut NowScreen, direction: Direction) {
    screen.press(ScreenKey::Arrow(direction));
}
