mod support;

use chrono::{Offset, TimeZone, Utc};
use sparrow_core::ChannelQuality::{Fhd, Hd, Sd, Uhd};
use sparrow_tv_guide::{GuidePages, GuideWindow, VariantPreferences};

const CATALOG: &str = r#"#EXTM3U
#EXTINF:-1 tvg-id="sweden-sd" group-title="Sweden",Regional News SD
https://fixture.invalid/sweden/sd.ts
#EXTINF:-1 tvg-id="sweden-hd" group-title="Sweden",Regional News HD
https://fixture.invalid/sweden/hd.ts
#EXTINF:-1 tvg-id="sweden-fhd" group-title="Sweden",Regional News FHD
https://fixture.invalid/sweden/fhd.ts
#EXTINF:-1 tvg-id="norway-sd" group-title="Norway",Regional News SD
https://fixture.invalid/norway/sd.ts
#EXTINF:-1 tvg-id="norway-fhd" group-title="Norway",Regional News FHD
https://fixture.invalid/norway/fhd.ts
"#;

const GUIDE: &str = r#"<tv>
  <channel id="sweden-sd"><display-name>Sweden SD</display-name></channel>
  <channel id="sweden-hd"><display-name>Sweden HD</display-name></channel>
  <programme channel="sweden-sd" start="20260901200000 +0000" stop="20260901210000 +0000">
    <title>Regional Report</title>
  </programme>
  <programme channel="sweden-hd" start="20260901200000 +0000" stop="20260901210000 +0000">
    <title>Regional Update</title>
  </programme>
</tv>"#;

#[tokio::test]
async fn quality_choice_uses_the_normalized_family_title_in_its_own_group() {
    let pages = load_pages(None).await;
    let [sweden, norway] = pages.families() else {
        panic!("the fixture provides two guide rows");
    };
    let selected_quality = |family: &sparrow_tv_guide::GuideFamily,
                            choices: &VariantPreferences| {
        family
            .preferred_variant(choices)
            .channel()
            .variant()
            .expect("fixture quality variant")
            .quality()
    };
    let mut choices = VariantPreferences::default();
    assert_eq!(selected_quality(sweden, &choices), Fhd);

    choices.choose("Sweden", "  ＲＥＧＩＯＮＡＬ\u{a0} NEWS\u{feff}", Sd);
    assert_eq!(selected_quality(sweden, &choices), Sd);
    assert_eq!(selected_quality(norway, &choices), Fhd);

    choices.choose("Sweden", "regional news", Uhd);
    assert_eq!(selected_quality(sweden, &choices), Fhd);
}

#[tokio::test]
async fn programme_data_follows_the_preferred_variant_then_catalog_order() {
    let pages = load_pages(Some(GUIDE)).await;
    let [sweden, norway] = pages.families() else {
        panic!("the fixture provides two guide rows");
    };
    let mut choices = VariantPreferences::default();
    assert_eq!(sweden.programmes(&choices)[0].title(), "Regional Report");
    assert!(norway.programmes(&choices).is_empty());

    choices.choose("Sweden", sweden.title(), Hd);
    assert_eq!(sweden.programmes(&choices)[0].title(), "Regional Update");
}

async fn load_pages(epg: Option<&str>) -> GuidePages {
    let fixture = support::open(CATALOG.into(), epg.map(str::to_owned)).await;
    let catalog = fixture.core();
    let generation = catalog.status().generation().expect("fixture generation");
    let now = Utc.with_ymd_and_hms(2026, 9, 1, 20, 15, 0).unwrap();
    let window = GuideWindow::at(now, Utc.fix(), false);
    let mut pages = GuidePages::new(generation, None, window, Default::default());
    let request = pages.request_next().expect("initial guide request");
    let result = catalog.guide_window(request.query().clone());
    let _ = pages.complete(request, result);
    pages
}
