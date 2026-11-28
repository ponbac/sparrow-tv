mod support;

use std::{collections::HashSet, fmt::Write, sync::Arc};

use chrono::{Offset, TimeZone, Utc};
use sparrow_core::{CatalogAccess, ChannelQuality};
use sparrow_tv_guide::{
    GuidePages, GuideWindow, PageUpdate, VariantPreferences,
    search::{SearchSelection, SearchState, SearchStatus, SearchView},
};

const HIDDEN: &str = "Hidden";
const VISIBLE: &str = "Visible";
const EXACT_PROGRAMME_MATCH: &str = "Signal";

#[tokio::test]
async fn inline_search_filters_both_lanes_and_tunes_the_exact_programme_channel() {
    let mut m3u = String::from("#EXTM3U\n");
    let mut epg = String::from("<tv>");
    add(&mut m3u, 1, HIDDEN, "Signal Hidden");
    programme(&mut epg, 1, "Signal Hidden Report");
    add(&mut m3u, 2, VISIBLE, "Signal One FHD");
    programme(&mut epg, 2, EXACT_PROGRAMME_MATCH);
    add(&mut m3u, 3, VISIBLE, "Signal One SD");
    for id in 4..=15 {
        add(&mut m3u, id, VISIBLE, &format!("Signal Channel {id}"));
        programme(&mut epg, id, &format!("Signal Report {id}"));
    }
    epg.push_str("</tv>");
    let fixture = support::open(m3u, Some(epg)).await;
    let core = fixture.core();
    let generation = core.status().generation();
    let mut state = SearchState::new(generation);
    let mut choices = VariantPreferences::default();
    choices.choose(VISIBLE, "  ＳＩＧＮＡＬ\u{a0}ＯＮＥ", ChannelQuality::Sd);
    let choices = Arc::new(choices);
    state.set_preferences(HashSet::from([HIDDEN.into()]), choices);
    state.set_view(SearchView::Inline);
    let ticket = state.set_term("  ＳＩＧＮＡＬ ").unwrap();
    assert!(state.set_term("signal").is_none());
    let request = state.finish_debounce(ticket).unwrap();
    assert_eq!(state.complete(request.run(core)), PageUpdate::Applied);
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 8);
    assert_eq!(state.programmes().len(), 8);
    assert_eq!(state.hidden_count(), 2);
    assert!(
        state
            .programmes()
            .iter()
            .all(|hit| hit.channel().group() == VISIBLE)
    );
    assert!(state.channels().iter().all(|row| !row.is_hidden()));

    let family = &state.channels()[0];
    assert_eq!(family.title(), "Signal One");
    assert_eq!(family.channel().name(), "Signal One SD");
    let fhd = family.variants()[1].id().clone();
    let now = Utc.with_ymd_and_hms(2026, 9, 1, 20, 15, 0).unwrap();
    let mut guide = GuidePages::new(
        generation.unwrap(),
        None,
        GuideWindow::at(now, Utc.fix(), false),
        HashSet::from([HIDDEN.into()]),
    );
    let page = guide.request_next().unwrap();
    let result = core.guide_window(page.query().clone());
    assert_eq!(guide.complete(page, result), PageUpdate::Applied);
    assert_eq!(
        family
            .programme_on_now(&guide.families()[0], now)
            .unwrap()
            .title(),
        EXACT_PROGRAMME_MATCH
    );
    let result = state
        .programmes()
        .iter()
        .position(|hit| hit.title() == EXACT_PROGRAMME_MATCH)
        .expect("matching Programme is visible");
    let key = state.programme_key(result).unwrap();
    let tune = state.tune(SearchSelection::Programme(key)).unwrap();
    assert_eq!(tune.channel, fhd);
    assert_eq!(Some(tune.generation), generation);
    assert_eq!(state.view(), SearchView::Closed);

    populate(&mut state, core, SearchView::Inline, "signal");
    assert_eq!(state.channels()[0].channel().name(), "Signal One SD");
}

#[tokio::test]
async fn filtered_programme_selection_cannot_tune_the_channel_that_inherits_its_rank() {
    let mut m3u = String::from("#EXTM3U\n");
    add(&mut m3u, 1, HIDDEN, "Hidden News");
    add(&mut m3u, 2, VISIBLE, "Local News");
    let mut epg = String::from("<tv>");
    programme(&mut epg, 1, EXACT_PROGRAMME_MATCH);
    programme(&mut epg, 2, "Local Signal");
    epg.push_str("</tv>");
    let fixture = support::open(m3u, Some(epg)).await;
    let core = fixture.core();
    let mut state = SearchState::new(core.status().generation());
    populate(&mut state, core, SearchView::Inline, "signal");
    assert_eq!(state.programmes()[0].channel().group(), HIDDEN);
    assert_eq!(state.programmes()[1].channel().group(), VISIBLE);

    let removed = state.programme_key(0).unwrap();
    let retained = state.programme_key(1).unwrap();
    let retained_channel = state.programmes()[1].channel().id().clone();
    state.set_preferences(HashSet::from([HIDDEN.into()]), Arc::default());
    assert_eq!(state.programmes()[0].channel().group(), VISIBLE);
    assert_eq!(state.programme_index(removed), None);
    assert!(state.tune(SearchSelection::Programme(removed)).is_none());
    assert_eq!(state.programme_index(retained), Some(0));
    let tune = state.tune(SearchSelection::Programme(retained)).unwrap();
    assert_eq!(tune.channel, retained_channel);

    populate(&mut state, core, SearchView::Inline, "signal");
    assert_eq!(state.programmes()[0].channel().id(), &retained_channel);
    assert!(state.tune(SearchSelection::Programme(retained)).is_none());
}

#[tokio::test]
async fn desk_advances_all_hidden_pages_and_preserves_a_family_across_continuations() {
    let mut m3u = String::from("#EXTM3U\n");
    for id in 1..=41 {
        add(&mut m3u, id, HIDDEN, &format!("Signal Channel {id:03}"));
    }
    for id in 42..=79 {
        add(&mut m3u, id, VISIBLE, &format!("Signal Channel {id:03}"));
    }
    add(&mut m3u, 80, VISIBLE, "Signal Bridge SD");
    add(&mut m3u, 81, VISIBLE, "Signal Bridge FHD");
    let fixture = support::open(m3u, None).await;
    let core = fixture.core();
    let mut state = SearchState::new(core.status().generation());
    state.set_preferences(HashSet::from([HIDDEN.into()]), Arc::default());
    populate(&mut state, core, SearchView::Inline, "signal");
    assert_eq!(state.status(), SearchStatus::Hidden);
    assert_eq!(state.hidden_count(), 40);
    assert!(state.channels().is_empty());

    let timer = state.set_view(SearchView::Desk).unwrap();
    let first_page = state.finish_debounce(timer).unwrap();
    assert_eq!(state.complete(first_page.run(core)), PageUpdate::Applied);
    assert_eq!(state.hidden_count(), 40);
    assert_eq!(state.status(), SearchStatus::Loading);
    assert!(state.should_advance_hidden());
    state.set_include_hidden(true);
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 40);
    assert!(state.channels().iter().all(|row| row.is_hidden()));
    assert_eq!(state.hidden_count(), 0);
    assert!(!state.should_advance_hidden());

    let timer = state.set_view(SearchView::Inline).unwrap();
    assert!(!state.include_hidden());
    let inline = state.finish_debounce(timer).unwrap();
    assert_eq!(state.complete(inline.run(core)), PageUpdate::Applied);
    assert!(state.channels().is_empty());
    let timer = state.set_view(SearchView::Desk).unwrap();
    assert!(!state.include_hidden());
    let reopened = state.finish_debounce(timer).unwrap();
    assert_eq!(state.complete(reopened.run(core)), PageUpdate::Applied);
    assert!(state.channels().is_empty());
    assert!(state.should_advance_hidden());

    let continuation = state.request_more().unwrap();
    assert!(state.loading_more());
    assert!(state.request_more().is_none());
    assert_eq!(state.complete(continuation.run(core)), PageUpdate::Applied);
    assert_eq!(state.hidden_count(), 41);
    assert_eq!(state.channels().len(), 39);
    assert!(!state.should_advance_hidden());
    let bridge = &state.channels()[38];
    assert_eq!(bridge.channel().name(), "Signal Bridge SD");
    let number = bridge.channel().number();

    let final_page = state.request_more().unwrap();
    assert_eq!(state.complete(final_page.unavailable()), PageUpdate::Failed);
    assert_eq!(state.status(), SearchStatus::Failed);
    let retry = state.retry().unwrap();
    assert_eq!(state.complete(retry.run(core)), PageUpdate::Applied);
    assert_eq!(state.channels().len(), 39);
    let bridge = &state.channels()[38];
    assert_eq!(bridge.channel().number(), number);
    assert_eq!(bridge.channel().name(), "Signal Bridge FHD");
    assert!(!state.has_more());
    let best = bridge.channel().id().clone();
    assert_eq!(
        state
            .tune(SearchSelection::Channel(number))
            .unwrap()
            .channel,
        best
    );
}

#[tokio::test]
async fn tuning_a_hidden_desk_channel_clears_include_hidden_for_the_next_open() {
    let mut m3u = String::from("#EXTM3U\n");
    add(&mut m3u, 1, HIDDEN, "Hidden News");
    let fixture = support::open(m3u, None).await;
    let core = fixture.core();
    let mut state = SearchState::new(core.status().generation());
    state.set_preferences(HashSet::from([HIDDEN.into()]), Arc::default());
    populate(&mut state, core, SearchView::Desk, "news");
    assert!(state.channels().is_empty());
    state.set_include_hidden(true);
    let selected = state.channels()[0].channel().number();
    assert!(state.tune(SearchSelection::Channel(selected)).is_some());
    assert_eq!(state.view(), SearchView::Closed);

    populate(&mut state, core, SearchView::Desk, "news");
    assert!(!state.include_hidden());
    assert!(state.channels().is_empty());
}

#[tokio::test]
async fn superseded_debounce_and_reads_cannot_replace_the_active_generation() {
    let mut m3u = String::from("#EXTM3U\n");
    add(&mut m3u, 1, VISIBLE, "Evening News");
    let first = support::open(m3u.clone(), None).await;
    add(&mut m3u, 2, VISIBLE, "Local Weather");
    let second = support::open(m3u, None).await;
    let generation = first.core().status().generation();
    let newer_generation = second.core().status().generation();
    assert_ne!(generation, newer_generation);

    let mut state = SearchState::new(generation);
    state.set_view(SearchView::Inline);
    let superseded_timer = state.set_term("signal").unwrap();
    let timer = state.set_term("news").unwrap();
    assert!(state.finish_debounce(superseded_timer).is_none());
    let delayed = state.finish_debounce(timer).unwrap();
    let completed = state
        .set_view(SearchView::Desk)
        .expect("the desk starts its own read");
    assert!(delayed.is_cancelled());
    assert_eq!(
        state.complete(delayed.run(first.core())),
        PageUpdate::Superseded,
    );

    let request = state.finish_debounce(completed).unwrap();
    assert_eq!(
        state.complete(request.run(second.core())),
        PageUpdate::ReconcileGeneration,
    );
    assert!(state.channels().is_empty());
    assert_eq!(state.status(), SearchStatus::ReconcileGeneration);
    let restarted = state.set_generation(newer_generation).unwrap();
    let request = state.finish_debounce(restarted).unwrap();
    assert_eq!(
        state.complete(request.run(second.core())),
        PageUpdate::Applied,
    );
    assert_eq!(state.channels()[0].channel().name(), "Evening News");

    assert!(state.set_term(&"é".repeat(129)).is_none());
    assert_eq!(state.status(), SearchStatus::Invalid);
    assert!(state.channels().is_empty());
}

#[tokio::test]
async fn refining_a_search_keeps_previous_results_visible_until_replacement_arrives() {
    let mut m3u = String::from("#EXTM3U\n");
    add(&mut m3u, 1, VISIBLE, "Manchester One");
    add(&mut m3u, 2, VISIBLE, "Manchester Two");
    add(&mut m3u, 3, VISIBLE, "Other News");
    let fixture = support::open(m3u, None).await;
    let core = fixture.core();
    let mut state = SearchState::new(core.status().generation());
    populate(&mut state, core, SearchView::Inline, "man");
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 2);

    // Typing another character keeps the existing rows visible during debounce and read.
    let ticket = state.set_term("manchester two").unwrap();
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 2);
    let request = state.finish_debounce(ticket).unwrap();
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 2);

    // Completing replaces the previous hits cleanly without mixing old and new rows.
    assert_eq!(state.complete(request.run(core)), PageUpdate::Applied);
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 1);
    assert_eq!(state.channels()[0].channel().name(), "Manchester Two");

    // Refining to a non-matching term keeps prior hits until the empty result arrives.
    let ticket = state.set_term("manchester zzz").unwrap();
    assert_eq!(state.status(), SearchStatus::Ready);
    assert_eq!(state.channels().len(), 1);
    let request = state.finish_debounce(ticket).unwrap();
    assert_eq!(state.complete(request.run(core)), PageUpdate::Applied);
    assert_eq!(state.status(), SearchStatus::Empty);
    assert!(state.channels().is_empty());

    // Clearing the term clears hits immediately.
    populate(&mut state, core, SearchView::Inline, "man");
    assert_eq!(state.channels().len(), 2);
    assert!(state.set_term("").is_none());
    assert_eq!(state.status(), SearchStatus::Idle);
    assert!(state.channels().is_empty());
}

fn populate(state: &mut SearchState, core: &CatalogAccess, view: SearchView, term: &str) {
    state.set_view(view);
    let ticket = state.set_term(term).unwrap();
    let request = state.finish_debounce(ticket).unwrap();
    assert_eq!(state.complete(request.run(core)), PageUpdate::Applied);
}

fn add(m3u: &mut String, id: u32, group: &str, name: &str) {
    writeln!(
        m3u,
        "#EXTINF:-1 tvg-id=\"channel-{id}\" group-title=\"{group}\",{name}\nhttps://fixture.invalid/{id}.ts",
    )
    .unwrap();
}

fn programme(epg: &mut String, id: u32, title: &str) {
    write!(
        epg,
        "<channel id=\"channel-{id}\"><display-name>Channel {id}</display-name></channel>\
         <programme channel=\"channel-{id}\" start=\"20260901200000 +0000\" stop=\"20260901210000 +0000\">\
         <title>{title}</title></programme>",
    )
    .unwrap();
}
