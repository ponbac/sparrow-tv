mod support;

use chrono::{DateTime, FixedOffset, Offset, TimeDelta, TimeZone, Utc};
use sparrow_core::{ChannelQuery, GuideProgramme, GuideWindowQuery, PageLimit, PageRequest};
use sparrow_tv_guide::time::{
    GuideWindow, ProgrammeCellAppearance, ProgrammeCellContent, ProgrammeFocus, ProgrammePlacement,
    ProgrammeStep, Timeline, elapsed_fraction, minutes_left, next_after, next_full_local_hours,
    programme_at, timeline_day_label,
};

#[test]
fn time_chips_offer_six_strictly_future_local_hours() {
    let now = Utc.with_ymd_and_hms(2026, 1, 9, 12, 15, 0).unwrap();
    let hours = next_full_local_hours(now, &chrono_tz::Asia::Kathmandu);
    assert_eq!(
        hours,
        (1..=6)
            .map(|hour| now + TimeDelta::hours(hour))
            .collect::<Vec<_>>()
    );
}

#[test]
fn time_chips_skip_local_hours_lost_to_spring_forward() {
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 5, 15, 0).unwrap();
    let labels: Vec<_> = next_full_local_hours(now, &chrono_tz::America::New_York)
        .into_iter()
        .map(|instant| {
            instant
                .with_timezone(&chrono_tz::America::New_York)
                .format("%H:%M")
                .to_string()
        })
        .collect();
    assert_eq!(
        labels,
        ["01:00", "03:00", "04:00", "05:00", "06:00", "07:00"]
    );
}

#[test]
fn time_chips_include_both_instants_of_a_repeated_local_hour() {
    let zone = chrono_tz::America::New_York;
    let now = Utc.with_ymd_and_hms(2026, 11, 1, 4, 15, 0).unwrap();
    let hours = next_full_local_hours(now, &zone);
    let label =
        |instant: DateTime<Utc>| instant.with_timezone(&zone).format("%H:%M %z").to_string();
    assert_eq!(label(hours[0]), "01:00 -0400");
    assert_eq!(label(hours[1]), "01:00 -0500");
    assert_eq!(hours[1] - hours[0], TimeDelta::hours(1));
    assert_eq!(next_full_local_hours(hours[0], &zone)[0], hours[1]);
}

#[test]
fn time_chips_realign_after_a_half_hour_clock_change() {
    let zone = chrono_tz::Australia::Lord_Howe;
    let now = Utc.with_ymd_and_hms(2026, 10, 3, 14, 0, 0).unwrap();
    let hours = next_full_local_hours(now, &zone);
    assert_eq!(hours[0], now + TimeDelta::minutes(30));
    assert_eq!(hours[1], now + TimeDelta::hours(2));
}

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

#[test]
fn timeline_day_follows_local_midnight_and_the_offset_at_each_instant() {
    let utc = |day, hour, minute| {
        Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    };
    let daylight = FixedOffset::west_opt(4 * 3600).unwrap();
    let standard = FixedOffset::west_opt(5 * 3600).unwrap();
    let now = utc(1, 3, 45).with_timezone(&daylight);
    for (visible, offset, label) in [
        (utc(1, 3, 30), daylight, "Today"),
        (utc(1, 4, 0), daylight, "Tomorrow"),
        (utc(1, 6, 30), standard, "Tomorrow"),
        // At this instant the previous offset would label it Monday, before midnight locally.
        (utc(2, 4, 30), standard, "Tomorrow"),
        (utc(2, 5, 0), standard, "Monday"),
    ] {
        assert_eq!(
            timeline_day_label(visible.with_timezone(&offset), now),
            label
        );
    }
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
            full_width_fraction: 5.0 / 120.0,
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
            full_width_fraction: 0.5,
            cut_at_start: false,
        })
    );

    timeline.reveal(&programmes[4]);
    assert_eq!(timeline.starts_at(), at(360));
    assert_eq!(timeline.ends_at(), at(480));
}

#[tokio::test]
async fn timeline_clips_placement_but_keeps_the_actual_extent_and_elapsed_end() {
    let programmes = load_schedule(&[
        ("Left cut", -60, 60),
        ("Right cut", 115, 180),
        ("After the track", 120, 135),
    ])
    .await;
    let timeline = Timeline::new(GuideWindow::at(at(0), Utc.fix(), false));
    let left = &programmes[0];
    let right = &programmes[1];
    let after = &programmes[2];

    assert_eq!(
        timeline.placement(right),
        Some(ProgrammePlacement {
            left_fraction: 115.0 / 120.0,
            width_fraction: 5.0 / 120.0,
            full_width_fraction: 65.0 / 120.0,
            cut_at_start: false,
        })
    );
    assert_eq!(
        timeline
            .placement(right)
            .unwrap()
            .cell_appearance(1218., false),
        Some(ProgrammeCellAppearance {
            width: 653.75,
            content: ProgrammeCellContent::Text,
        })
    );
    assert_eq!(timeline.elapsed_fraction(left, at(30)), Some(0.5));
    assert_eq!(timeline.elapsed_fraction(right, at(117)), Some(2.0 / 65.0));
    assert_eq!(timeline.elapsed_fraction(right, at(0)), Some(0.0));
    assert_eq!(timeline.elapsed_fraction(right, at(185)), Some(1.0));
    assert_eq!(timeline.placement(after), None);
    assert_eq!(timeline.elapsed_fraction(after, at(125)), None);
}

#[test]
fn timeline_narrow_cells_draw_only_for_cut_tails_or_focus() {
    use ProgrammeCellContent::{Chevron, Text};

    // The six-unit gap makes 96 units of scheduled time the exact text cutoff.
    let cases = [
        (6., true, true, None),
        (95.5, false, false, None),
        (96., false, false, None),
        (96., true, false, Some((90., Chevron))),
        (96., false, true, Some((90., Chevron))),
        (96.5, false, false, Some((90.5, Text))),
    ];
    for (scheduled_width, cut_at_start, focused, expected) in cases {
        const TRACK_WIDTH: f32 = 192.;
        let fraction = scheduled_width / f64::from(TRACK_WIDTH);
        let placement = ProgrammePlacement {
            left_fraction: 0.,
            width_fraction: fraction,
            full_width_fraction: fraction,
            cut_at_start,
        };

        assert_eq!(
            placement.cell_appearance(TRACK_WIDTH, focused),
            expected.map(|(width, content)| ProgrammeCellAppearance { width, content }),
            "scheduled_width={scheduled_width}, cut={cut_at_start}, focused={focused}",
        );
    }
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
        timeline.exact_focus(&old_row[1..], &second_bulletin),
        Some(1)
    );
    assert_eq!(
        timeline.restore_focus(&old_row[1..], &second_bulletin),
        Some(1)
    );
    assert_eq!(timeline.starts_at(), at(60));

    let expired = ProgrammeFocus::at(&programmes, 0).unwrap();
    assert_eq!(timeline.exact_focus(&programmes, &expired), None);
    assert_eq!(timeline.restore_focus(&programmes, &expired), Some(1));
    assert_eq!(timeline.starts_at(), at(30));
    assert_eq!(timeline.restore_focus(&[], &expired), None);
}

fn at(minutes: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 20, 0, 0).unwrap() + TimeDelta::minutes(minutes)
}

async fn load_programmes() -> Vec<GuideProgramme> {
    load_schedule(&[
        ("Lead-out", -55, 5),
        ("Feature", 5, 60),
        ("Bulletin", 65, 80),
        ("Afternoon", 150, 210),
        ("Far programme", 450, 470),
    ])
    .await
}

async fn load_schedule(slots: &[(&str, i64, i64)]) -> Vec<GuideProgramme> {
    let m3u = "#EXTM3U\n#EXTINF:-1 tvg-id=\"test\",Test\nhttp://127.0.0.1:9/live\n";
    let mut epg = "<tv><channel id=\"test\"><display-name>Test</display-name></channel>".to_owned();
    for &(title, start, end) in slots {
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
