mod support;

use chrono::{DateTime, FixedOffset, Offset, TimeDelta, TimeZone, Utc};
use sparrow_core::{ChannelQuery, GuideProgramme, GuideWindowQuery, PageLimit, PageRequest};
use sparrow_tv_guide::time::{
    GuideWindow, ProgrammeFocus, ProgrammePlacement, ProgrammeStep, Timeline, elapsed_fraction,
    minutes_left, next_after, programme_at,
};

#[test]
fn windows_follow_local_half_hours_even_with_a_quarter_hour_offset() {
    let kathmandu = FixedOffset::east_opt((5 * 60 + 45) * 60).unwrap();
    let window = GuideWindow::at(at(17), kathmandu, false);
    let extended = GuideWindow::at(at(17), kathmandu, true);

    assert_eq!(window.starts_at(), at(15));
    assert_eq!(window.ends_at(), at(195));
    assert_eq!(GuideWindow::at(at(44), kathmandu, false), window);
    assert_eq!(
        GuideWindow::at(at(45), kathmandu, false).starts_at(),
        at(45)
    );
    assert_eq!(extended.starts_at(), window.starts_at());
    assert_eq!(extended.ends_at(), at(495));
}

#[test]
fn guide_window_advances_through_the_repeated_wall_clock_hour() {
    let first = Utc.with_ymd_and_hms(2026, 11, 1, 5, 10, 0).unwrap();
    let daylight = FixedOffset::west_opt(4 * 3600).unwrap();
    let standard = FixedOffset::west_opt(5 * 3600).unwrap();
    let first_window = GuideWindow::at(first, daylight, false);
    let second_window = GuideWindow::at(first + TimeDelta::hours(1), standard, false);

    assert_eq!(first_window.starts_at(), first - TimeDelta::minutes(10));
    assert_eq!(
        second_window.starts_at(),
        first_window.starts_at() + TimeDelta::hours(1)
    );
}

#[tokio::test]
async fn now_next_and_elapsed_use_half_open_slots_and_keep_schedule_gaps() {
    let programmes = load_programmes().await;
    let feature = &programmes[1];

    assert_eq!(
        programme_at(&programmes, at(4)).unwrap().title(),
        "Lead-out"
    );
    assert_eq!(programme_at(&programmes, at(5)), Some(feature));
    assert_eq!(programme_at(&programmes, at(60)), None);
    assert_eq!(next_after(&programmes, at(5)).unwrap().title(), "Bulletin");
    assert_eq!(next_after(&programmes, at(60)).unwrap().title(), "Bulletin");
    assert_eq!(next_after(&programmes, at(450)), None);

    assert_eq!(minutes_left(feature, at(15)), 45);
    assert_eq!(
        minutes_left(feature, at(15) - TimeDelta::milliseconds(1)),
        46
    );
    assert_eq!(minutes_left(feature, at(60)), 1);
    assert_eq!(
        elapsed_fraction(feature, at(32) + TimeDelta::seconds(30)),
        0.5
    );
}

#[tokio::test]
async fn timeline_steps_past_a_sliver_at_the_start_and_pans_to_later_cells() {
    let programmes = load_programmes().await;
    let mut timeline = Timeline::new(GuideWindow::at(at(0), Utc.fix(), true));
    assert_eq!(timeline.ends_at(), at(120));

    let time_anchor = timeline.anchor(at(0), Some(&programmes[0]));
    assert_eq!(time_anchor, at(0));
    assert_eq!(timeline.cell_at(&programmes, time_anchor), Some(1));
    assert_eq!(timeline.cell_at(&programmes, at(64)), Some(2));
    assert_eq!(timeline.cell_at(&[], time_anchor), None);
    assert_eq!(
        timeline.step_programme(&programmes, 1, ProgrammeStep::Earlier),
        None
    );
    assert_eq!(
        timeline.step_programme(&programmes, 2, ProgrammeStep::Later),
        Some(3)
    );
    assert_eq!(
        timeline.placement(&programmes[0]),
        Some(ProgrammePlacement {
            left_fraction: 0.0,
            width_fraction: 5.0 / 120.0,
            cut_at_start: true,
        })
    );
    assert_eq!(timeline.placement(&programmes[3]), None);

    timeline.reveal(&programmes[3]);
    assert_eq!(timeline.starts_at(), at(120));
    assert_eq!(timeline.ends_at(), at(240));
    assert_eq!(timeline.playhead_fraction(at(119)), None);
    assert_eq!(timeline.playhead_fraction(at(180)), Some(0.5));
    assert_eq!(timeline.playhead_fraction(at(240)), Some(1.0));
    assert_eq!(timeline.playhead_fraction(at(241)), None);
    assert_eq!(
        timeline.step_programme(&programmes, 3, ProgrammeStep::Earlier),
        Some(2)
    );
    timeline.reveal(&programmes[2]);
    assert_eq!(timeline.starts_at(), at(60));
    assert_eq!(timeline.placement(&programmes[1]), None);
    assert_eq!(
        timeline.placement(&programmes[3]),
        Some(ProgrammePlacement {
            left_fraction: 0.75,
            width_fraction: 0.25,
            cut_at_start: false,
        })
    );

    timeline.reveal(&programmes[4]);
    assert_eq!(timeline.starts_at(), at(360));
    assert_eq!(timeline.ends_at(), at(480));
}

#[tokio::test]
async fn clock_reanchor_keeps_the_visible_time_and_restores_focus_within_the_same_row() {
    let programmes = load_programmes().await;
    let mut timeline = Timeline::new(GuideWindow::at(at(0), Utc.fix(), false));
    timeline.reveal(&programmes[3]);
    timeline.reveal(&programmes[2]);
    assert_eq!(timeline.starts_at(), at(60));

    let old_row = [
        programmes[0].clone(),
        programmes[2].clone(),
        programmes[2].clone(),
    ];
    let second_bulletin = ProgrammeFocus::at(&old_row, 2).unwrap();
    timeline.reanchor(GuideWindow::at(at(30), Utc.fix(), false));
    assert_eq!(
        timeline.restore_focus(&old_row[1..], &second_bulletin),
        Some(1)
    );
    assert_eq!(timeline.starts_at(), at(60));

    let expired = ProgrammeFocus::at(&programmes, 0).unwrap();
    assert_eq!(timeline.restore_focus(&programmes, &expired), Some(1));
    assert_eq!(timeline.starts_at(), at(30));
    assert_eq!(timeline.restore_focus(&[], &expired), None);
}

fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 20, 0, 0).unwrap() + TimeDelta::minutes(minutes)
}

async fn load_programmes() -> Vec<GuideProgramme> {
    let m3u = "#EXTM3U\n#EXTINF:-1 tvg-id=\"test\",Test\nhttp://127.0.0.1:9/live\n";
    let mut epg = "<tv><channel id=\"test\"><display-name>Test</display-name></channel>".to_owned();
    for (title, start, end) in [
        ("Lead-out", -55, 5),
        ("Feature", 5, 60),
        ("Bulletin", 65, 80),
        ("Afternoon", 150, 210),
        ("Far programme", 450, 470),
    ] {
        let start = at(start).format("%Y%m%d%H%M%S +0000");
        let end = at(end).format("%Y%m%d%H%M%S +0000");
        epg.push_str(&format!("<programme channel=\"test\" start=\"{start}\" stop=\"{end}\"><title>{title}</title></programme>"));
    }
    epg.push_str("</tv>");
    let fixture = support::open(m3u.to_owned(), Some(epg)).await;
    let channel_query = ChannelQuery::all(PageRequest::first(PageLimit::new(1).unwrap()));
    let window = GuideWindowQuery::new(at(0), at(480), channel_query).unwrap();
    let page = fixture.core().guide_window(window).unwrap();
    page.items()[0].programmes().to_vec()
}
