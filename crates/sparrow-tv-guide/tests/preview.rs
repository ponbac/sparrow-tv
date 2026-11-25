mod support;

use std::fmt::Write;

use chrono::{Offset, TimeDelta, TimeZone, Utc};
use sparrow_core::{ChannelQuality, ChannelQuery, GuideWindowQuery, PageLimit, PageRequest};
use sparrow_tv_guide::{
    GuidePages, GuideWindow, PageUpdate, VariantPreferences,
    preview::{
        DescriptionPage, PreviewProgramme, PreviewTiming, following_programmes, preview_timing,
        select_programme,
    },
};

#[tokio::test]
async fn preview_selects_the_focused_programme_and_its_schedule_source() {
    let m3u = r#"#EXTM3U
#EXTINF:-1 tvg-id="sd" group-title="Film",Cinema SD
https://fixture.invalid/cinema-sd.ts
#EXTINF:-1 tvg-id="hd" group-title="Film",Cinema HD
https://fixture.invalid/cinema-hd.ts
#EXTINF:-1 tvg-id="fhd" group-title="Film",Cinema FHD
https://fixture.invalid/cinema-fhd.ts
"#;
    let epg = r#"<tv>
      <channel id="sd"><display-name>Cinema SD</display-name></channel>
      <channel id="hd"><display-name>Cinema HD</display-name></channel>
      <programme channel="sd" start="20260901200000 +0000" stop="20260901210000 +0000"><title>Live</title></programme>
      <programme channel="sd" start="20260901210000 +0000" stop="20260901220000 +0000"><title>Focused</title></programme>
      <programme channel="sd" start="20260901213000 +0000" stop="20260901221500 +0000"><title>Overlap</title></programme>
      <programme channel="sd" start="20260901220000 +0000" stop="20260901223000 +0000"><title>Adjacent</title></programme>
      <programme channel="sd" start="20260901223000 +0000" stop="20260901230000 +0000"><title>Second</title></programme>
      <programme channel="sd" start="20260901230000 +0000" stop="20260901233000 +0000"><title>Third</title></programme>
      <programme channel="sd" start="20260901233000 +0000" stop="20260902000000 +0000"><title>Fourth</title></programme>
      <programme channel="hd" start="20260901210000 +0000" stop="20260901220000 +0000"><title>Alternate</title></programme>
    </tv>"#;
    let fixture = support::open(m3u.into(), Some(epg.into())).await;
    let core = fixture.core();
    let at = |hour, minute, second| {
        Utc.with_ymd_and_hms(2026, 9, 1, hour, minute, second)
            .unwrap()
    };
    let now = at(20, 58, 30);
    let mut pages = GuidePages::new(
        core.status().generation().unwrap(),
        None,
        GuideWindow::at(now, Utc.fix(), true),
        Default::default(),
    );
    let request = pages.request_next().unwrap();
    let data = core.guide_window(request.query().clone());
    assert_eq!(pages.complete(request, data), PageUpdate::Applied);
    let [family] = pages.families() else {
        panic!("the fixture supplies one Channel with three Quality Variants");
    };
    let mut preferences = VariantPreferences::default();

    let (source, programme) = select_programme(family, &preferences, None, now).unwrap();
    assert_eq!(source.channel().name(), "Cinema SD");
    assert_eq!(programme.title(), "Live");
    assert_eq!(
        select_programme(family, &preferences, Some(usize::MAX), now)
            .unwrap()
            .1
            .title(),
        "Live"
    );

    let (source, programme) = select_programme(family, &preferences, Some(1), now).unwrap();
    assert_eq!(programme.title(), "Focused");
    assert_eq!(
        preview_timing(programme, now),
        PreviewTiming::Upcoming {
            in_minutes: 2,
            duration_minutes: 60,
        }
    );
    assert_eq!(
        preview_timing(programme, at(21, 0, 0)),
        PreviewTiming::Live {
            elapsed_fraction: 0.,
            minutes_left: 60,
        }
    );
    assert_eq!(
        preview_timing(programme, at(21, 30, 0)),
        PreviewTiming::Live {
            elapsed_fraction: 0.5,
            minutes_left: 30,
        }
    );
    assert_eq!(
        preview_timing(programme, at(22, 0, 0)),
        PreviewTiming::Ended {
            duration_minutes: 60,
        }
    );
    assert_eq!(
        following_programmes(source, programme)
            .map(|programme| programme.title())
            .collect::<Vec<_>>(),
        ["Adjacent", "Second", "Third"]
    );

    preferences.choose("Film", "Cinema", ChannelQuality::Hd);
    let (source, programme) = select_programme(family, &preferences, None, now).unwrap();
    assert_eq!(source.channel().name(), "Cinema HD");
    assert_eq!(programme.title(), "Alternate");
    assert!(following_programmes(source, programme).next().is_none());
    assert!(select_programme(family, &preferences, None, at(22, 0, 0)).is_none());
}

#[tokio::test]
async fn description_finds_the_focused_duplicate_on_the_next_schedule_page() {
    let m3u = "#EXTM3U\n#EXTINF:-1 tvg-id=\"preview\",Preview\nhttps://fixture.invalid/a.ts\n";
    let mut epg =
        String::from(r#"<tv><channel id="preview"><display-name>Preview</display-name></channel>"#);
    let title_prefix = "a".repeat(256);
    let chosen_description = format!("Chosen {}", "🌍".repeat(300));
    for index in 0..5 {
        let description = if index == 4 {
            &chosen_description
        } else {
            "Earlier"
        };
        write!(
            epg,
            r#"<programme channel="preview" start="20260901200000 +0000" stop="20260901210000 +0000"><title>{title_prefix}{index}</title><desc>{description}</desc></programme>"#,
        )
        .unwrap();
    }
    epg.push_str("</tv>");
    let fixture = support::open(m3u.into(), Some(epg)).await;
    let core = fixture.core();
    let from = Utc.with_ymd_and_hms(2026, 9, 1, 20, 0, 0).unwrap();
    let query = GuideWindowQuery::new(
        from,
        from + TimeDelta::hours(3),
        ChannelQuery::all(PageRequest::first(PageLimit::new(1).unwrap())),
    )
    .unwrap();
    let guide = core.guide_window(query).unwrap();
    let row = &guide.items()[0];
    let slots = row.programmes();
    assert_eq!(slots.len(), 5);
    assert!(slots[4].title_truncated());
    assert_eq!(slots[4].title(), slots[0].title());
    let chosen = PreviewProgramme::for_row(guide.generation(), row, &slots[4]).unwrap();
    let earlier = PreviewProgramme::for_row(guide.generation(), row, &slots[0]).unwrap();
    assert!(chosen != earlier);

    let lookup = chosen.description(from + TimeDelta::minutes(30));
    let first = core.schedule(lookup.query()).unwrap();
    let DescriptionPage::Next(lookup) = lookup.accept(&first) else {
        panic!("the fifth occurrence belongs to the second schedule page");
    };
    let second = core.schedule(lookup.query()).unwrap();
    let DescriptionPage::Finished(Some(description)) = lookup.accept(&second) else {
        panic!("the second page contains the focused occurrence");
    };
    assert_eq!(description, format!("Chosen {}", "🌍".repeat(254)));
}
