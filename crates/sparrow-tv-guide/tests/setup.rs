mod support;

use sparrow_core::{CatalogGeneration, ChannelGroupView, Page, PageLimit, PageRequest};
use sparrow_tv_guide::{
    PageStatus,
    now_focus::Direction,
    setup::{
        CatalogProgress, DraftView, FailureReason, FoundSummary, Setup, SetupAction, SetupControl,
        SetupField, SetupGate, SetupKey, SetupMessage, SetupStep,
    },
};

const EMPTY: DraftView = DraftView {
    has_m3u: false,
    has_epg: false,
    busy: false,
};
const M3U: DraftView = DraftView {
    has_m3u: true,
    ..EMPTY
};
const BOTH: DraftView = DraftView {
    has_epg: true,
    ..M3U
};

fn arrow(direction: Direction) -> SetupKey {
    SetupKey::Arrow(direction)
}

fn first_launch() -> Setup {
    Setup::start(CatalogProgress::NotConfigured).expect("an empty device starts Setup")
}

fn at_guide_step(draft: DraftView) -> Setup {
    let mut setup = first_launch();
    setup.field_committed(SetupField::M3u, draft);
    assert_eq!(setup.press(SetupKey::Activate, draft), SetupAction::None);
    assert_eq!(setup.step(), SetupStep::Guide);
    setup
}

/// A real two-Channel catalog, for the replacement's generation and roster.
async fn replacement_roster() -> (Option<CatalogGeneration>, Page<ChannelGroupView>) {
    let replacement = support::open(
        "#EXTM3U\n\
         #EXTINF:-1 group-title=\"New\",One\nhttps://fixture.invalid/one.ts\n\
         #EXTINF:-1 group-title=\"New\",Two\nhttps://fixture.invalid/two.ts\n"
            .into(),
        None,
    )
    .await;
    let roster = replacement
        .core()
        .list_groups(PageRequest::first(PageLimit::new(50).unwrap()))
        .unwrap();
    (Some(roster.generation()), roster)
}

#[test]
fn a_window_decides_once_whether_setup_greets_the_viewer() {
    let mut populated = SetupGate::default();
    assert!(populated.observe(CatalogProgress::Ready).is_none());
    // Replacing sources from the Sources sheet publishes no catalog for a while.
    assert!(populated.observe(CatalogProgress::Loading).is_none());

    let mut empty = SetupGate::default();
    let setup = empty.observe(CatalogProgress::NotConfigured).unwrap();
    assert_eq!(setup.step(), SetupStep::Channels);
    assert!(empty.observe(CatalogProgress::NotConfigured).is_none());

    let relaunched = Setup::start(CatalogProgress::Failed(FailureReason::Refused)).unwrap();
    assert_eq!(relaunched.step(), SetupStep::Failed(FailureReason::Refused));
    assert_eq!(relaunched.focus(), SetupControl::TryAgain);
}

#[test]
fn step_one_edits_the_channel_source_then_moves_to_the_guide_source() {
    let mut setup = first_launch();
    assert_eq!(setup.focus(), SetupControl::Field);
    assert_eq!(
        setup.press(arrow(Direction::Right), EMPTY),
        SetupAction::None
    );
    assert_eq!(
        setup.focus(),
        SetupControl::Field,
        "no Clear without an address"
    );
    assert_eq!(
        setup.press(SetupKey::Activate, EMPTY),
        SetupAction::Edit(SetupField::M3u)
    );

    setup.press(arrow(Direction::Down), EMPTY);
    assert_eq!(setup.focus(), SetupControl::Next);
    assert_eq!(
        setup.press(SetupKey::Activate, EMPTY),
        SetupAction::Edit(SetupField::M3u),
        "Next without an address opens the keyboard"
    );

    setup.field_committed(SetupField::M3u, M3U);
    assert_eq!(setup.focus(), SetupControl::Next);
    assert_eq!(setup.press(SetupKey::Activate, M3U), SetupAction::None);
    assert_eq!(setup.step(), SetupStep::Guide);
    assert_eq!(setup.focus(), SetupControl::Skip);

    assert_eq!(setup.back(M3U), SetupAction::None);
    assert_eq!(setup.step(), SetupStep::Channels);
    assert_eq!(setup.back(M3U), SetupAction::Exit);
}

#[test]
fn clear_sits_beside_a_filled_field_and_returns_focus_to_it() {
    let mut setup = first_launch();
    setup.press(arrow(Direction::Up), M3U);
    assert_eq!(setup.focus(), SetupControl::Field);
    setup.press(arrow(Direction::Right), M3U);
    assert_eq!(setup.focus(), SetupControl::Clear);
    assert_eq!(
        setup.press(SetupKey::Activate, M3U),
        SetupAction::Clear(SetupField::M3u)
    );
    setup.field_cleared(SetupField::M3u);
    assert_eq!(setup.focus(), SetupControl::Field);

    // A busy draft takes no edits or clears.
    let busy = DraftView { busy: true, ..M3U };
    setup.press(arrow(Direction::Right), busy);
    assert_eq!(setup.press(SetupKey::Activate, busy), SetupAction::None);
}

#[test]
fn skip_saves_without_the_guide_source_and_load_saves_both() {
    let mut setup = at_guide_step(M3U);
    assert_eq!(
        setup.press(SetupKey::Activate, M3U),
        SetupAction::Save { include_epg: false }
    );

    let mut setup = at_guide_step(BOTH);
    assert_eq!(
        setup.focus(),
        SetupControl::Load,
        "a prefilled guide source loads"
    );
    assert_eq!(
        setup.press(SetupKey::Activate, BOTH),
        SetupAction::Save { include_epg: true }
    );
    setup.press(arrow(Direction::Left), BOTH);
    assert_eq!(
        setup.press(SetupKey::Activate, BOTH),
        SetupAction::Save { include_epg: false }
    );
}

#[test]
fn found_opens_with_the_first_catalog_but_the_guide_waits_for_the_save() {
    let mut setup = at_guide_step(BOTH);
    setup.save_started();
    assert_eq!(setup.step(), SetupStep::Loading);

    // A failure published before the replacement belongs to the old attempt.
    setup.catalog(CatalogProgress::Failed(FailureReason::Unreachable));
    assert_eq!(setup.step(), SetupStep::Loading);
    setup.catalog(CatalogProgress::Loading);
    setup.catalog(CatalogProgress::Ready);
    assert_eq!(setup.step(), SetupStep::Found);
    assert!(setup.summary().is_none());

    setup.roster(None, Some(PageStatus::Loading), &[]);
    assert!(setup.summary().is_none());
    setup.roster(None, Some(PageStatus::Failed), &[]);
    assert_eq!(setup.summary(), Some(&FoundSummary::Uncounted));
    assert!(!setup.guide_ready(), "the guide source is still loading");
    assert_eq!(setup.press(SetupKey::Activate, EMPTY), SetupAction::None);
    assert_eq!(setup.back(EMPTY), SetupAction::None);

    setup.save_succeeded(CatalogProgress::Ready, None);
    assert!(setup.guide_ready());
    assert_eq!(
        setup.press(SetupKey::Activate, EMPTY),
        SetupAction::OpenGuide
    );
    assert_eq!(setup.back(EMPTY), SetupAction::OpenGuide);
}

#[test]
fn a_coalesced_ready_catalog_shows_the_count_while_the_save_finishes() {
    let mut setup = at_guide_step(BOTH);
    setup.save_started();
    setup.catalog(CatalogProgress::Ready);
    assert_eq!(setup.step(), SetupStep::Found);
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert!(setup.summary().is_some());
    assert!(!setup.guide_ready());
    setup.save_succeeded(CatalogProgress::Ready, None);
    assert!(setup.guide_ready());
}

#[test]
fn an_old_catalog_that_recovers_during_a_save_gives_way_to_the_replacement() {
    let mut setup = Setup::start(CatalogProgress::Failed(FailureReason::Unreachable)).unwrap();
    setup.change_sources(M3U);
    setup.press(SetupKey::Activate, M3U);
    setup.save_started();
    // The previous source recovered just before the replacement took over.
    setup.catalog(CatalogProgress::Ready);
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert_eq!(setup.step(), SetupStep::Found);
    setup.catalog(CatalogProgress::Loading);
    assert_eq!(setup.step(), SetupStep::Loading);
    setup.save_succeeded(CatalogProgress::Failed(FailureReason::Refused), None);
    assert_eq!(setup.step(), SetupStep::Failed(FailureReason::Refused));
    assert!(setup.summary().is_none());
}

#[tokio::test]
async fn found_counts_the_replacement_even_when_loading_was_never_seen() {
    let (generation, roster) = replacement_roster().await;

    let mut setup = Setup::start(CatalogProgress::Failed(FailureReason::Unreachable)).unwrap();
    setup.change_sources(M3U);
    setup.save_started();
    // The old source recovers and is counted under its own generation.
    setup.catalog(CatalogProgress::Ready);
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert!(setup.summary().is_some());
    // The save reports the replacement's generation before the surface has
    // seen it, and the window missed the replacement's Loading status.
    setup.save_succeeded(CatalogProgress::Ready, generation);
    assert!(setup.summary().is_none(), "the old count is dropped");
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert!(setup.summary().is_none(), "the old roster no longer counts");
    assert_eq!(setup.press(SetupKey::Activate, EMPTY), SetupAction::None);
    setup.roster(generation, Some(PageStatus::Loading), &[]);
    assert!(!setup.guide_ready());
    setup.roster(generation, Some(PageStatus::Complete), roster.items());
    assert!(setup.guide_ready());
    assert!(matches!(
        setup.summary(),
        Some(FoundSummary::Counted { channels: 2, .. })
    ));
}

#[tokio::test]
async fn an_old_roster_that_finishes_after_the_save_never_counts() {
    let (generation, roster) = replacement_roster().await;
    let mut setup = Setup::start(CatalogProgress::Failed(FailureReason::Unreachable)).unwrap();
    setup.change_sources(M3U);
    setup.save_started();
    // The old source recovers, but its roster is still loading at save time.
    setup.catalog(CatalogProgress::Ready);
    setup.roster(None, Some(PageStatus::Loading), &[]);
    setup.save_succeeded(CatalogProgress::Ready, generation);
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert!(setup.summary().is_none(), "the old roster never counts");
    assert_eq!(setup.press(SetupKey::Activate, EMPTY), SetupAction::None);
    setup.roster(generation, Some(PageStatus::Complete), roster.items());
    assert!(setup.guide_ready());
}

#[test]
fn a_save_that_reports_back_before_any_status_still_reaches_found() {
    let mut setup = at_guide_step(M3U);
    setup.save_started();
    setup.save_succeeded(CatalogProgress::Ready, None);
    assert_eq!(setup.step(), SetupStep::Found);
}

#[test]
fn rejected_addresses_return_to_their_step() {
    let mut setup = at_guide_step(BOTH);
    setup.save_started();
    setup.save_failed(SetupMessage::InvalidM3u);
    assert_eq!(setup.step(), SetupStep::Channels);
    assert_eq!(setup.message(), Some(SetupMessage::InvalidM3u));
    assert_eq!(setup.focus(), SetupControl::Field);
    setup.field_committed(SetupField::M3u, BOTH);
    assert_eq!(setup.message(), None);

    let mut setup = at_guide_step(BOTH);
    setup.save_started();
    setup.save_failed(SetupMessage::SaveFailed);
    assert_eq!(setup.step(), SetupStep::Guide);
    assert_eq!(setup.message(), Some(SetupMessage::SaveFailed));
}

#[test]
fn a_failed_first_load_offers_retry_and_recovers_on_its_own() {
    let mut setup = at_guide_step(M3U);
    setup.save_started();
    setup.catalog(CatalogProgress::Loading);
    setup.save_succeeded(CatalogProgress::Failed(FailureReason::TimedOut), None);
    assert_eq!(setup.step(), SetupStep::Failed(FailureReason::TimedOut));
    assert_eq!(setup.back(M3U), SetupAction::Exit);

    assert_eq!(setup.press(SetupKey::Activate, M3U), SetupAction::Retry);
    setup.retry_started();
    setup.catalog(CatalogProgress::Failed(FailureReason::TimedOut));
    assert_eq!(
        setup.step(),
        SetupStep::Loading,
        "the old failure is ignored"
    );
    setup.retry_finished(CatalogProgress::Failed(FailureReason::Refused));
    assert_eq!(setup.step(), SetupStep::Failed(FailureReason::Refused));

    // A scheduled refresh can succeed without the viewer pressing anything.
    setup.catalog(CatalogProgress::Ready);
    assert_eq!(setup.step(), SetupStep::Found);
    setup.roster(None, Some(PageStatus::Complete), &[]);
    assert!(setup.guide_ready());
}

#[test]
fn change_sources_starts_again_from_the_prefilled_first_step() {
    let mut setup = Setup::start(CatalogProgress::Failed(FailureReason::Unreachable)).unwrap();
    setup.show_message(SetupMessage::RetryUnavailable);
    setup.press(arrow(Direction::Right), EMPTY);
    assert_eq!(setup.focus(), SetupControl::ChangeSources);
    assert_eq!(
        setup.press(SetupKey::Activate, EMPTY),
        SetupAction::ChangeSources
    );
    setup.change_sources(BOTH);
    assert_eq!(setup.step(), SetupStep::Channels);
    assert_eq!(setup.focus(), SetupControl::Next);
    assert_eq!(setup.message(), None, "a Failed message stays on Failed");
}

#[test]
fn the_viewers_steps_ignore_catalog_publications() {
    let mut setup = at_guide_step(M3U);
    for progress in [
        CatalogProgress::Loading,
        CatalogProgress::Failed(FailureReason::Storage),
        CatalogProgress::Ready,
        CatalogProgress::NotConfigured,
    ] {
        setup.catalog(progress);
        assert_eq!(setup.step(), SetupStep::Guide);
    }
}

#[tokio::test]
async fn catalog_progress_follows_the_real_first_launch() {
    let empty = support::open_unconfigured().await;
    assert_eq!(
        CatalogProgress::from_status(&empty.core().status()),
        CatalogProgress::NotConfigured
    );

    let unreachable = support::open_unreachable().await;
    assert_eq!(
        CatalogProgress::from_status(&unreachable.core().status()),
        CatalogProgress::Failed(FailureReason::Unreachable)
    );

    let m3u = "#EXTM3U\n\
        #EXTINF:-1 group-title=\"News\",News SD\nhttps://fixture.invalid/news-sd.ts\n\
        #EXTINF:-1 group-title=\"News\",News HD\nhttps://fixture.invalid/news-hd.ts\n\
        #EXTINF:-1 group-title=\"News\",Weather\nhttps://fixture.invalid/weather.ts\n\
        #EXTINF:-1 group-title=\"Sport\",Arena\nhttps://fixture.invalid/arena.ts\n\
        #EXTINF:-1,Loose\nhttps://fixture.invalid/loose.ts\n"
        .to_owned();
    let ready = support::open(m3u, None).await;
    assert_eq!(
        CatalogProgress::from_status(&ready.core().status()),
        CatalogProgress::Ready
    );
    let roster = ready
        .core()
        .list_groups(PageRequest::first(PageLimit::new(50).unwrap()))
        .unwrap();
    assert!(roster.next().is_none());
    // Quality Variants count as Channels, and untitled Channels form a group.
    assert_eq!(
        FoundSummary::from_roster(roster.items()),
        FoundSummary::Counted {
            channels: 5,
            groups: 3,
            first: vec![("News".into(), 3), ("Sport".into(), 1), (String::new(), 1)],
        }
    );
}
