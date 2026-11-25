use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    mem::size_of,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use static_assertions::assert_not_impl_any;

use crate::{
    domain::{
        CatalogGeneration, ChannelGroupFilter, ChannelId, ChannelQuery, CoreError, GuideProgramme,
        GuideWindowQuery, PageCursor, PageLimit, PageRequest, ScheduleQuery, SearchRequest,
        SearchTerm, SourceConfiguration, SourceConfigurationInput,
    },
    m3u::ParsedChannel,
    xmltv::{ParsedGuide, ParsedGuideChannel, ParsedProgramme},
};

use super::{
    ChannelCatalog,
    flat::{ChannelHot, FlatHeader, SECTION_COUNT, Section},
};

assert_not_impl_any!(ChannelCatalog: Clone);

proptest! {
    #[test]
    fn channel_cursor_round_trips_visit_every_sorted_item_once(
        channel_count in 1_usize..300,
        limit in 1_u16..=100,
    ) {
        let catalog = catalog(
            unique_channels(channel_count, false),
            generation(1),
        );
        let expected = catalog.channel_ids_in_order();
        let mut request = PageRequest::first(PageLimit::new(limit).expect("generated limit is valid"));
        let mut observed = Vec::new();

        loop {
            let page = catalog
                .channels_page(&ChannelQuery::all(request))
                .expect("generated page request is valid");
            prop_assert!(page.items().len() <= usize::from(limit));
            observed.extend(
                page.items()
                    .iter()
                    .map(|channel| channel.id().as_str().to_owned()),
            );
            let Some(next) = page.next() else {
                break;
            };
            let parsed = PageCursor::parse(next.as_str())
                .expect("generated cursors round-trip through their transport form");
            request = PageRequest::after(
                parsed,
                PageLimit::new(limit).expect("generated limit is valid"),
            );
        }

        prop_assert_eq!(observed, expected);
    }

    #[test]
    fn catalog_generations_are_positive_javascript_safe_integers(
        m3u_checksum in any::<[u8; 32]>(),
        epg_checksum in prop::option::of(any::<[u8; 32]>()),
    ) {
        let generation = configuration().catalog_generation(
            &m3u_checksum,
            epg_checksum.as_ref(),
        );

        prop_assert!((1..=CatalogGeneration::MAX_SAFE_INTEGER).contains(&generation.get()));
    }

    #[test]
    fn search_cursor_round_trips_visit_every_ranked_channel_once(
        channel_count in 1_usize..300,
        limit in 1_u16..=100,
    ) {
        let catalog = catalog(
            unique_channels(channel_count, false),
            generation(1),
        );
        let expected = catalog.channel_ids_in_order();
        let term = SearchTerm::parse("channel").expect("fixture term is valid");
        let programme_page = PageRequest::first(
            PageLimit::new(100).expect("fixture limit is valid"),
        );
        let mut channel_page = PageRequest::first(
            PageLimit::new(limit).expect("generated limit is valid"),
        );
        let mut observed = Vec::new();

        loop {
            let results = catalog
                .search(&SearchRequest::new(
                    term.clone(),
                    channel_page,
                    programme_page.clone(),
                ))
                .expect("generated search request is valid");
            prop_assert!(results.channels().items().len() <= usize::from(limit));
            prop_assert!(results.programmes().items().is_empty());
            observed.extend(
                results
                    .channels()
                    .items()
                    .iter()
                    .map(|channel| channel.id().as_str().to_owned()),
            );
            let Some(next) = results.channels().next() else {
                break;
            };
            channel_page = PageRequest::after(
                PageCursor::parse(next.as_str())
                    .expect("generated cursor round-trips through transport"),
                PageLimit::new(limit).expect("generated limit is valid"),
            );
        }

        prop_assert_eq!(observed, expected);
    }

    #[test]
    fn missing_provider_ids_are_stable_across_source_reordering(
        channel_count in 1_usize..200,
    ) {
        let forward = catalog(
            unique_channels(channel_count, false),
            generation(1),
        );
        let reversed = catalog(
            unique_channels(channel_count, true),
            generation(2),
        );

        prop_assert_eq!(ids_by_name(&forward), ids_by_name(&reversed));
    }

    #[test]
    fn indistinguishable_backup_entries_have_distinct_reorder_stable_id_sets(
        duplicate_count in 2_usize..100,
    ) {
        let forward = catalog(
            duplicate_channels(duplicate_count, false),
            generation(1),
        );
        let reversed = catalog(
            duplicate_channels(duplicate_count, true),
            generation(2),
        );
        let forward_ids = forward.channel_ids_in_order().into_iter().collect::<HashSet<_>>();
        let reversed_ids = reversed.channel_ids_in_order().into_iter().collect::<HashSet<_>>();

        prop_assert_eq!(forward_ids.len(), duplicate_count);
        prop_assert_eq!(forward_ids, reversed_ids);
    }

    #[test]
    fn mutated_flat_catalog_bytes_either_reject_or_query_without_panicking(
        mutations in prop::collection::vec((0_usize..20_000, any::<u8>()), 1..8),
        truncate_by in 0_usize..32,
    ) {
        let built = rich_catalog(generation(1));
        let mut bytes = built.as_bytes().to_vec();
        let len = bytes.len();
        for (index, value) in mutations {
            bytes[index % len] ^= value.max(1);
        }
        if truncate_by > 0 && truncate_by < bytes.len() {
            bytes.truncate(bytes.len() - truncate_by);
        }
        if let Some(reopened) =
            ChannelCatalog::from_cache_mapping(crate::ports::CatalogCacheMapping::new(bytes), generation(1))
        {
            let limit = PageLimit::new(10).unwrap();
            let _ = reopened.groups_page(&PageRequest::first(limit));
            if let Ok(page) = reopened.channels_page(&ChannelQuery::all(PageRequest::first(limit))) {
                for ch in page.items() {
                    let _ = reopened.channel(ch.id());
                    let _ = reopened.resolve_playback(ch.id());
                    let _ = reopened.schedule(&ScheduleQuery::new(
                        ch.id().clone(),
                        PageRequest::first(limit),
                    ));
                }
            }
            let term = SearchTerm::parse("news").unwrap();
            let _ = reopened.search(&SearchRequest::new(
                term.clone(),
                PageRequest::first(limit),
                PageRequest::first(limit),
            ));
            let _ = reopened.search_programmes(&term, &PageRequest::first(limit));
            let window = GuideWindowQuery::new(
                instant(0),
                instant(7200),
                ChannelQuery::all(PageRequest::first(limit)),
            )
            .unwrap();
            let _ = reopened.guide_window(&window);
        }
    }
}

#[test]
fn a_cursor_from_an_older_catalog_generation_returns_typed_invalidation() {
    let first = catalog(unique_channels(3, false), generation(1));
    let first_page = first
        .channels_page(&ChannelQuery::all(PageRequest::first(
            PageLimit::new(1).expect("fixture limit is valid"),
        )))
        .expect("first generation is queryable");
    let cursor = first_page.next().expect("more Channels remain").clone();
    let current_generation = generation(2);
    let current = catalog(unique_channels(3, true), current_generation);

    let error = current
        .channels_page(&ChannelQuery::all(PageRequest::after(
            cursor,
            PageLimit::new(1).expect("fixture limit is valid"),
        )))
        .expect_err("an older generation cursor is rejected");
    assert_eq!(
        error,
        CoreError::StaleCursor {
            current: current_generation,
        }
    );
}

#[test]
fn a_cached_catalog_keeps_channel_numbers_quality_variants_and_all_query_parity() {
    let built = rich_catalog(generation(1));
    let bytes = built.as_bytes().to_vec();
    let reopened = ChannelCatalog::from_cache_mapping(
        crate::ports::CatalogCacheMapping::new(bytes),
        generation(1),
    )
    .expect("the flat catalog bytes reopen cleanly");
    let limit = PageLimit::new(20).expect("fixture limit is valid");

    let groups_built = built.groups_page(&PageRequest::first(limit)).unwrap();
    let groups_reopened = reopened.groups_page(&PageRequest::first(limit)).unwrap();
    assert_eq!(groups_reopened.items(), groups_built.items());

    let query = ChannelQuery::all(PageRequest::first(limit));
    let expected = built.channels_page(&query).unwrap();
    assert_eq!(
        expected
            .items()
            .iter()
            .map(|channel| channel.number())
            .collect::<Vec<_>>(),
        [1, 2, 2, 2, 2]
    );
    let reopened_channels = reopened.channels_page(&query).unwrap();
    assert_eq!(reopened_channels.items(), expected.items());

    let group_query = ChannelQuery::in_group(
        ChannelGroupFilter::parse("Sport").unwrap(),
        PageRequest::first(limit),
    );
    assert_eq!(
        reopened.channels_page(&group_query).unwrap().items(),
        built.channels_page(&group_query).unwrap().items()
    );

    for ch in expected.items() {
        assert_eq!(
            reopened.channel(ch.id()).unwrap(),
            built.channel(ch.id()).unwrap()
        );
        assert_eq!(
            reopened
                .resolve_playback(ch.id())
                .unwrap()
                .location_for_adapter(),
            built
                .resolve_playback(ch.id())
                .unwrap()
                .location_for_adapter()
        );
        let sched = ScheduleQuery::new(ch.id().clone(), PageRequest::first(limit));
        assert_eq!(
            reopened.schedule(&sched).unwrap().items(),
            built.schedule(&sched).unwrap().items()
        );
        let sched_from = sched.with_from(instant(15));
        assert_eq!(
            reopened.schedule(&sched_from).unwrap().items(),
            built.schedule(&sched_from).unwrap().items()
        );
    }

    let window = GuideWindowQuery::new(instant(10), instant(60), query).unwrap();
    assert_eq!(
        reopened.guide_window(&window).unwrap().items(),
        built.guide_window(&window).unwrap().items()
    );

    let term = SearchTerm::parse("arena").unwrap();
    let search_req = SearchRequest::new(
        term.clone(),
        PageRequest::first(limit),
        PageRequest::first(limit),
    );
    let built_search = built.search(&search_req).unwrap();
    let reopened_search = reopened.search(&search_req).unwrap();
    assert_eq!(
        reopened_search.channels().items(),
        built_search.channels().items()
    );
    assert_eq!(
        reopened_search.programmes().items(),
        built_search.programmes().items()
    );
    assert_eq!(
        reopened
            .search_programmes(&term, &PageRequest::first(limit))
            .unwrap()
            .items(),
        built
            .search_programmes(&term, &PageRequest::first(limit))
            .unwrap()
            .items()
    );
}

#[test]
fn incremental_epg_and_m3u_rebuilds_are_byte_identical_to_full_from_parsed() {
    let config = configuration();
    let initial_channels = vec![
        parsed_channel_with_tvg("tvg.a", "Alpha HD", "News"),
        with_icon(
            parsed_channel_with_tvg("tvg.a", "Alpha FHD", "News"),
            "https://icons.example/alpha.png",
        ),
        parsed_channel_with_tvg("", "World News", "News"),
    ];
    let updated_channels = vec![
        with_icon(
            parsed_channel_with_tvg("tvg.a", "Alpha HD", "News"),
            "https://icons.example/alpha.png",
        ),
        with_icon(
            parsed_channel_with_tvg("tvg.unmatched", "New Match", "Sports"),
            "http://icons.example/new.jpg",
        ),
    ];
    let guide = ParsedGuide {
        channels: vec![
            ParsedGuideChannel {
                id: Arc::from("tvg.a"),
                display_names: vec![Arc::from("Alpha")],
            },
            ParsedGuideChannel {
                id: Arc::from("tvg.unmatched"),
                display_names: vec![Arc::from("New Match")],
            },
        ],
        programmes: vec![
            ParsedProgramme {
                guide_channel_id: Arc::from("tvg.unmatched"),
                title: Arc::from("Unmatched Initially"),
                description: Some(Arc::from("Later matched when M3U adds tvg.unmatched")),
                starts_at: instant(100),
                ends_at: instant(200),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("tvg.a"),
                title: Arc::from("Alpha Show"),
                description: Some(Arc::from("Description A")),
                starts_at: instant(50),
                ends_at: instant(150),
            },
        ],
    };

    // 1. M3U-only -> with_updated_guide must be byte-for-byte identical to full from_parsed.
    let m3u_only = ChannelCatalog::from_parsed(&config, &initial_channels, None, generation(1));
    let updated_epg = m3u_only.with_updated_guide(&guide, generation(2));
    let full_initial =
        ChannelCatalog::from_parsed(&config, &initial_channels, Some(&guide), generation(2));
    assert_eq!(updated_epg.as_bytes(), full_initial.as_bytes());

    // 2. Existing catalog (containing unmatched EPG programmes in cold tables) + new M3U
    // must match previously unmatched programmes and be byte-for-byte identical to full from_parsed.
    let rematched = ChannelCatalog::from_parsed_with_existing_guide(
        &config,
        &updated_channels,
        &updated_epg,
        generation(3),
    );
    let full_updated =
        ChannelCatalog::from_parsed(&config, &updated_channels, Some(&guide), generation(3));
    assert_eq!(rematched.as_bytes(), full_updated.as_bytes());
}

#[test]
fn equal_ranked_programme_search_hits_follow_channel_id_order_across_page_boundaries() {
    // Build channels whose catalog order differs from ChannelId lexicographic order,
    // all sharing the same EPG programme title so tie-breaking strictly exercises
    // ChannelId traversal order across page boundaries.
    let channels: Vec<ParsedChannel> = (0..12)
        .map(|i| parsed_channel_with_tvg("shared.tvg", &format!("Station {i:02}"), "Live"))
        .collect();
    let guide = ParsedGuide {
        channels: vec![ParsedGuideChannel {
            id: Arc::from("shared.tvg"),
            display_names: vec![Arc::from("Shared")],
        }],
        programmes: vec![ParsedProgramme {
            guide_channel_id: Arc::from("shared.tvg"),
            title: Arc::from("Evening News"),
            description: Some(Arc::from("Shared bulletin")),
            starts_at: instant(100),
            ends_at: instant(200),
        }],
    };
    let cat = ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));
    let catalog_order_ids = cat.channel_ids_in_order();
    let mut id_sorted = catalog_order_ids.clone();
    id_sorted.sort();
    assert_ne!(
        catalog_order_ids, id_sorted,
        "fixture must differ between catalog order and ChannelId order"
    );

    let term = SearchTerm::parse("evening news").unwrap();
    let limit = PageLimit::new(5).unwrap();
    let mut page_req = PageRequest::first(limit);
    let mut observed_ids = Vec::new();
    loop {
        let page = cat.search_programmes(&term, &page_req).unwrap();
        observed_ids.extend(
            page.items()
                .iter()
                .map(|p| p.channel_id().as_str().to_owned()),
        );
        let Some(next) = page.next() else {
            break;
        };
        page_req = PageRequest::after(next.clone(), limit);
    }
    assert_eq!(observed_ids, id_sorted);
}

#[test]
fn standalone_programme_search_deduplicates_shared_long_descriptions_in_page_arena() {
    let channels: Vec<ParsedChannel> = (0..60)
        .map(|i| parsed_channel_with_tvg("shared.long", &format!("Mirror {i:02}"), "HD"))
        .collect();
    let long_description = format!("shared-needle {}", "x".repeat(32 * 1024));
    let guide = ParsedGuide {
        channels: vec![ParsedGuideChannel {
            id: Arc::from("shared.long"),
            display_names: vec![Arc::from("Mirror")],
        }],
        programmes: vec![ParsedProgramme {
            guide_channel_id: Arc::from("shared.long"),
            title: Arc::from("Shared Feature"),
            description: Some(Arc::from(long_description.as_str())),
            starts_at: instant(100),
            ends_at: instant(200),
        }],
    };
    let cat = ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));
    let page = cat
        .search_programmes(
            &SearchTerm::parse("shared-needle").unwrap(),
            &PageRequest::first(PageLimit::new(60).unwrap()),
        )
        .unwrap();
    assert_eq!(page.items().len(), 60);
    // All 60 ProgrammeSummary descriptions point to the exact same slice in the
    // page-owned arena rather than allocating 60 separate 32 KiB copies.
    let first_ptr = page.items()[0].description().unwrap().as_ptr();
    for item in page.items() {
        assert_eq!(item.description(), Some(long_description.as_str()));
        assert_eq!(item.description().unwrap().as_ptr(), first_ptr);
    }
}

#[test]
fn compact_programme_projections_bound_ascii_and_multibyte_utf8_titles() {
    let channels = vec![parsed_channel_with_tvg(
        "titles.id",
        "Title Channel",
        "News",
    )];
    let exact_title = format!("exact-{}", "a".repeat(GuideProgramme::MAX_TITLE_BYTES - 6));
    assert_eq!(exact_title.len(), GuideProgramme::MAX_TITLE_BYTES);
    let long_ascii = format!("ascii-{}", "b".repeat(4_096));
    // Place a 4-byte emoji starting at byte 254 so byte 256 splits the scalar.
    let mut split_utf8 = format!("utf8-{}", "c".repeat(254 - 5));
    assert_eq!(split_utf8.len(), 254);
    split_utf8.push('🎉');
    split_utf8.push_str(&"d".repeat(512));

    let guide = ParsedGuide {
        channels: vec![ParsedGuideChannel {
            id: Arc::from("titles.id"),
            display_names: vec![Arc::from("Title Channel")],
        }],
        programmes: vec![
            ParsedProgramme {
                guide_channel_id: Arc::from("titles.id"),
                title: Arc::from("short-needle"),
                description: None,
                starts_at: instant(100),
                ends_at: instant(200),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("titles.id"),
                title: Arc::from(exact_title.as_str()),
                description: None,
                starts_at: instant(200),
                ends_at: instant(300),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("titles.id"),
                title: Arc::from(long_ascii.as_str()),
                description: None,
                starts_at: instant(300),
                ends_at: instant(400),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("titles.id"),
                title: Arc::from(split_utf8.as_str()),
                description: None,
                starts_at: instant(400),
                ends_at: instant(500),
            },
        ],
    };
    let cat = ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));

    let window = cat
        .guide_window(
            &GuideWindowQuery::new(
                instant(100),
                instant(500),
                ChannelQuery::all(PageRequest::first(PageLimit::new(10).unwrap())),
            )
            .unwrap(),
        )
        .unwrap();
    let progs = window.items()[0].programmes();
    assert_eq!(progs.len(), 4);
    assert_eq!(progs[0].title(), "short-needle");
    assert!(!progs[0].title_truncated());
    assert_eq!(progs[1].title(), exact_title);
    assert!(!progs[1].title_truncated());
    assert_eq!(
        progs[2].title(),
        &long_ascii[..GuideProgramme::MAX_TITLE_BYTES]
    );
    assert!(progs[2].title_truncated());
    assert_eq!(progs[3].title().len(), 254);
    assert_eq!(progs[3].title(), &split_utf8[..254]);
    assert!(progs[3].title_truncated());

    let search = cat
        .search(&SearchRequest::new(
            SearchTerm::parse("utf8").unwrap(),
            PageRequest::first(PageLimit::new(10).unwrap()),
            PageRequest::first(PageLimit::new(10).unwrap()),
        ))
        .unwrap();
    assert_eq!(search.programmes().items().len(), 1);
    let hit = &search.programmes().items()[0];
    assert_eq!(hit.title(), &split_utf8[..254]);
    assert!(hit.title_truncated());
}

#[test]
fn parsed_channel_id_equals_and_hashes_identically_to_projections_from_different_pages() {
    let cat = catalog(unique_channels(10, false), generation(1));
    let page_one = cat
        .channels_page(&ChannelQuery::all(PageRequest::first(
            PageLimit::new(4).unwrap(),
        )))
        .unwrap();
    let page_two = cat
        .channels_page(&ChannelQuery::all(PageRequest::after(
            page_one.next().unwrap().clone(),
            PageLimit::new(4).unwrap(),
        )))
        .unwrap();
    let target_from_page_two = page_two.items()[0].id().clone();
    let target_parsed = ChannelId::parse(target_from_page_two.as_str()).unwrap();
    let target_from_details = cat.channel(&target_parsed).unwrap().id().clone();

    assert_eq!(target_parsed, target_from_page_two);
    assert_eq!(target_parsed, target_from_details);
    assert_eq!(target_from_page_two, target_from_details);
    assert_eq!(hash_of(&target_parsed), hash_of(&target_from_page_two));
    assert_eq!(hash_of(&target_parsed), hash_of(&target_from_details));
    assert_eq!(format!("{target_from_page_two:?}"), "ChannelId(<redacted>)");
}

#[test]
fn structural_validator_rejects_short_tables_flag_mismatch_bad_urls_and_corrupt_overlap() {
    let built = rich_catalog(generation(1));
    let base = built.as_bytes().to_vec();

    // 1. Short parallel table with contiguous sections (remove 1 `Span` = 8 bytes
    //    from ChannelSearchSpans and shift all remaining sections
    //    through `SECTION_COUNT` so the file stays contiguous through EOF and
    //    reaches the exact record-count check).
    let mut short_table = base.clone();
    let section_count = super::flat::SECTION_COUNT;
    let search_spans = Section::ChannelSearchSpans as usize;
    let sec18_off_pos = section_entry(search_spans);
    let sec18_off = u64::from_le_bytes(
        short_table[sec18_off_pos..sec18_off_pos + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    let sec18_len = u64::from_le_bytes(
        short_table[sec18_off_pos + 8..sec18_off_pos + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    assert!(sec18_len >= 8);
    short_table.drain(sec18_off + sec18_len - 8..sec18_off + sec18_len);
    short_table[sec18_off_pos + 8..sec18_off_pos + 16]
        .copy_from_slice(&((sec18_len - 8) as u64).to_le_bytes());
    for section_index in search_spans + 1..section_count {
        let pos = section_entry(section_index);
        let off = u64::from_le_bytes(short_table[pos..pos + 8].try_into().unwrap());
        short_table[pos..pos + 8].copy_from_slice(&(off - 8).to_le_bytes());
    }
    // Verify the mutated fixture passes section contiguity all the way to EOF
    // so failure must come from the record-count check on ChannelSearchSpans.
    let mut expected_cursor = size_of::<FlatHeader>() as u64;
    for section_index in 0..section_count {
        let pos = section_entry(section_index);
        let off = u64::from_le_bytes(short_table[pos..pos + 8].try_into().unwrap());
        let len = u64::from_le_bytes(short_table[pos + 8..pos + 16].try_into().unwrap());
        assert_eq!(
            off, expected_cursor,
            "section {section_index} must stay contiguous"
        );
        expected_cursor += len;
    }
    assert_eq!(expected_cursor as usize, short_table.len());
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(short_table),
            generation(1)
        )
        .is_none()
    );

    // 2. Unknown flag bit set in header.
    let mut bad_flags = base.clone();
    bad_flags[12..16].copy_from_slice(&0xFFFF_u32.to_le_bytes());
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(bad_flags),
            generation(1)
        )
        .is_none()
    );

    // 3. Clearing FLAG_HAS_EPG while EPG counts are non-zero.
    let mut cleared_epg_flag = base.clone();
    cleared_epg_flag[12..16].copy_from_slice(&0_u32.to_le_bytes());
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(cleared_epg_flag),
            generation(1)
        )
        .is_none()
    );

    // 4. Malformed playback URL with invalid port (would fail Url::parse at tune time).
    let mut bad_url = base.clone();
    let needle = b"https://media.fixture.invalid/channel";
    let pos = bad_url
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("playback URL exists in flat buffer");
    bad_url[pos..pos + needle.len()].copy_from_slice(b"https://media.fixture.in:99999/channe");
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(bad_url),
            generation(1)
        )
        .is_none()
    );

    // 5. Duplicate index in channels_by_id.
    let mut dup_by_id = base.clone();
    let sec6_off = section_offset(&dup_by_id, Section::ChannelsById);
    let first_idx = dup_by_id[sec6_off..sec6_off + 4].to_vec();
    dup_by_id[sec6_off + 4..sec6_off + 8].copy_from_slice(&first_idx);
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(dup_by_id),
            generation(1)
        )
        .is_none()
    );

    // 6. Underestimated schedule_overlap hiding an earlier long programme.
    let mut bad_overlap = base.clone();
    let sec9_off = section_offset(&bad_overlap, Section::ScheduleOverlap);
    // rich_catalog has programme 0 [0..100] followed by programme 1 [10..20];
    // setting overlap[1] = 1 instead of 0 underestimates the running max end.
    bad_overlap[sec9_off + 4..sec9_off + 8].copy_from_slice(&1_u32.to_le_bytes());
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(bad_overlap),
            generation(1)
        )
        .is_none()
    );

    // 7. Overlapping/aliased PlaybackSpans: aliasing channel 1 to
    // channel 0's playback span must be rejected to prevent parsing amplification.
    let mut aliased_playback = base.clone();
    let sec12_off = section_offset(&aliased_playback, Section::PlaybackSpans);
    let span0 = aliased_playback[sec12_off..sec12_off + 8].to_vec();
    aliased_playback[sec12_off + 8..sec12_off + 16].copy_from_slice(&span0);
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(aliased_playback),
            generation(1)
        )
        .is_none()
    );

    // 8. Overlapping/aliased ChannelNames (ChannelsHot -> ChannelHot.name_off/len):
    // aliasing channel 1's name span to channel 0 must be rejected.
    let mut aliased_name = base.clone();
    let sec4_off = section_offset(&aliased_name, Section::ChannelsHot);
    // name_off/name_len live at bytes 32..40 of each ChannelHot record.
    let stride = size_of::<ChannelHot>();
    let name0 = aliased_name[sec4_off + 32..sec4_off + 40].to_vec();
    aliased_name[sec4_off + stride + 32..sec4_off + stride + 40].copy_from_slice(&name0);
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(aliased_name),
            generation(1)
        )
        .is_none()
    );

    // 9. Trailing unreferenced bytes after final section must be rejected.
    let mut trailing = base.clone();
    trailing.push(0);
    assert!(
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(trailing),
            generation(1)
        )
        .is_none()
    );
}

#[test]
fn reopened_catalog_rejects_numbers_that_violate_the_family_index() {
    use std::mem::offset_of;

    let built = rich_catalog(generation(1));
    let base = built.as_bytes();
    let start = section_offset(base, Section::ChannelsHot);
    let reopen = |bytes| {
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(bytes),
            generation(1),
        )
    };
    assert!(reopen(base.to_vec()).is_some());

    let mut out_of_order = base.to_vec();
    let number = start + 4 * size_of::<ChannelHot>() + offset_of!(ChannelHot, number);
    out_of_order[number..number + 4].copy_from_slice(&1_u32.to_le_bytes());
    assert!(reopen(out_of_order).is_none());

    // Give the last variant of family 2 the same quality as its predecessor.
    let mut repeated_quality = base.to_vec();
    let quality = start + 4 * size_of::<ChannelHot>() + offset_of!(ChannelHot, quality);
    repeated_quality[quality] = repeated_quality[quality - size_of::<ChannelHot>()];
    assert!(reopen(repeated_quality).is_none());
}

#[test]
fn catalog_generation_covers_configuration_m3u_and_optional_epg_content() {
    let first_configuration = configuration();
    let other_configuration = SourceConfiguration::parse(SourceConfigurationInput::new(
        "https://other-provider.fixture.invalid/property.m3u",
        None::<String>,
    ))
    .expect("alternate fixture Source Configuration is valid");
    let baseline = first_configuration.catalog_generation(&[1; 32], None);

    assert_eq!(
        baseline,
        first_configuration.catalog_generation(&[1; 32], None)
    );
    assert_ne!(
        baseline,
        first_configuration.catalog_generation(&[2; 32], None)
    );
    assert_ne!(
        baseline,
        first_configuration.catalog_generation(&[1; 32], Some(&[3; 32]))
    );
    assert_ne!(
        baseline,
        other_configuration.catalog_generation(&[1; 32], None)
    );
    assert_ne!(baseline.get(), 0);
    assert!(baseline.get() <= CatalogGeneration::MAX_SAFE_INTEGER);
}

#[test]
fn search_ranking_is_exact_then_prefix_then_token_then_substring() {
    use super::MatchCategory::{Exact, Prefix, Substring, Token};

    let term = SearchTerm::parse("news").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);
    let mut rank = |field: &str| {
        matcher
            .field_rank(field.as_bytes(), &super::never_cancelled)
            .expect("an uncancelled rank succeeds")
    };

    assert_eq!(rank("news"), Some(Exact));
    assert_eq!(rank("news tonight"), Some(Prefix));
    assert_eq!(rank("newsroom evening"), Some(Prefix));
    assert_eq!(rank("evening news bulletin"), Some(Token));
    assert_eq!(rank("goodnews bulletin"), Some(Substring));
    assert_eq!(rank("weather bulletin"), None);

    let prefix = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES - 2);
    assert_eq!(rank(&format!("{prefix}newsroom")), Some(Substring));
    assert_eq!(rank(&format!("{prefix}newsroom news")), Some(Token));
}

#[test]
fn search_observes_cancellation_while_emitting_ranked_matches() {
    let cat = catalog(
        vec![
            parsed_channel("News", "G"),
            parsed_channel("News", "G"),
            parsed_channel("News", "G"),
        ],
        generation(1),
    );
    let term = SearchTerm::parse("news").expect("fixture term is valid");
    let checkpoints = Cell::new(0_usize);

    let result = cat.search_channels_with_cancellation(
        &term,
        &PageRequest::first(PageLimit::new(10).unwrap()),
        &|| {
            let next = checkpoints.get() + 1;
            checkpoints.set(next);
            next > 8
        },
    );

    assert!(matches!(result, Err(CoreError::Cancelled)));
}

#[test]
fn search_observes_cancellation_inside_one_pathological_field() {
    let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
    let cat = catalog(vec![parsed_channel(&oversized_field, "G")], generation(1));
    let term = SearchTerm::parse("needle").expect("fixture term is valid");
    let checkpoints = Cell::new(0_u8);

    let result = cat.search_channels_with_cancellation(
        &term,
        &PageRequest::first(PageLimit::new(10).unwrap()),
        &|| {
            let next = checkpoints.get() + 1;
            checkpoints.set(next);
            next >= 6
        },
    );

    assert!(matches!(result, Err(CoreError::Cancelled)));
}

#[test]
fn substring_matching_observes_cancellation_inside_one_pathological_field() {
    let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
    let term = SearchTerm::parse("needle").expect("fixture term is valid");
    let matcher = super::SearchMatcher::new(&term);
    let checkpoints = Cell::new(0_u8);

    let result = matcher.contains_substring(oversized_field.as_bytes(), &|| {
        let next = checkpoints.get() + 1;
        checkpoints.set(next);
        next >= 2
    });

    assert_eq!(result, Err(CoreError::Cancelled));
    assert_eq!(checkpoints.get(), 2);
}

#[test]
fn compiled_search_matcher_clears_token_scratch_between_fields() {
    use super::MatchCategory::Token;

    let term = SearchTerm::parse("alpha beta").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);

    assert_eq!(
        matcher
            .field_rank(b"alpha only", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        None
    );
    assert_eq!(
        matcher
            .field_rank(b"beta only", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        None
    );
    assert_eq!(
        matcher
            .field_rank(b"beta then alpha", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        Some(Token)
    );

    let duplicate_term = SearchTerm::parse("alpha alpha").expect("fixture term is valid");
    let mut duplicate_matcher = super::SearchMatcher::new(&duplicate_term);
    assert_eq!(
        duplicate_matcher
            .field_rank(b"one alpha token", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        Some(Token),
        "duplicate query tokens retain set semantics"
    );
}

#[test]
fn ranked_selection_keeps_only_the_requested_prefix_and_counts_every_match() {
    let cat = catalog(
        vec![
            parsed_channel("News", "Other"),
            parsed_channel("Other", "News"),
            parsed_channel("Newsroom", "Other"),
            parsed_channel("Other", "Newsroom"),
            parsed_channel("Daily News", "Other"),
            parsed_channel("Other", "Daily News"),
            parsed_channel("Goodnews", "Other"),
            parsed_channel("Other", "Goodnews"),
            parsed_channel("News", "Other"),
        ],
        generation(1),
    );
    let term = SearchTerm::parse("news").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);

    let selection =
        super::ranked_channel_selection(&cat.flat, &mut matcher, 5, &super::never_cancelled)
            .expect("an uncancelled ranking succeeds");

    assert_eq!(selection.available_len(), 9);
    // Notice channels are clustered by first-seen group ("Other", "News", "Newsroom",
    // "Daily News", "Goodnews"), so check ranked order via channel names & groups.
    let first_five = cat
        .search_channels(&term, &PageRequest::first(PageLimit::new(5).unwrap()))
        .unwrap();
    let pairs: Vec<(&str, &str)> = first_five
        .items()
        .iter()
        .map(|c| (c.name(), c.group()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("News", "Other"),
            ("News", "Other"),
            ("Other", "News"),
            ("Newsroom", "Other"),
            ("Other", "Newsroom"),
        ]
    );
}

#[test]
fn schedule_overlap_index_preserves_long_running_programmes_and_resets_per_channel() {
    let channels = vec![
        parsed_channel_with_tvg("ch.a", "Channel A", "News"),
        parsed_channel_with_tvg("ch.b", "Channel B", "News"),
    ];
    let guide = ParsedGuide {
        channels: vec![
            ParsedGuideChannel {
                id: Arc::from("ch.a"),
                display_names: vec![],
            },
            ParsedGuideChannel {
                id: Arc::from("ch.b"),
                display_names: vec![],
            },
        ],
        programmes: vec![
            parsed_programme_for("ch.a", 0, 100),
            parsed_programme_for("ch.a", 10, 20),
            parsed_programme_for("ch.a", 20, 30),
            parsed_programme_for("ch.b", 0, 5),
            parsed_programme_for("ch.b", 10, 40),
        ],
    };
    let cat = ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));
    let ids = cat
        .channels_page(&ChannelQuery::all(PageRequest::first(
            PageLimit::new(10).unwrap(),
        )))
        .unwrap();
    let ch_a = ids.items()[0].id().clone();
    let ch_b = ids.items()[1].id().clone();

    let a_from_50 = cat
        .schedule(
            &ScheduleQuery::new(
                ch_a.clone(),
                PageRequest::first(PageLimit::new(10).unwrap()),
            )
            .with_from(instant(50)),
        )
        .unwrap();
    assert_eq!(a_from_50.items().len(), 1);
    assert_eq!(a_from_50.items()[0].ends_at(), instant(100));

    let b_from_50 = cat
        .schedule(
            &ScheduleQuery::new(ch_b, PageRequest::first(PageLimit::new(10).unwrap()))
                .with_from(instant(50)),
        )
        .unwrap();
    assert!(b_from_50.items().is_empty());

    // The short Programme ending at the filter boundary does not consume a
    // position. The continuation reaches the next match, then ends.
    let limit = PageLimit::new(1).unwrap();
    let first = cat
        .schedule(
            &ScheduleQuery::new(ch_a.clone(), PageRequest::first(limit)).with_from(instant(20)),
        )
        .unwrap();
    assert_eq!(first.items()[0].ends_at(), instant(100));
    let next = cat
        .schedule(
            &ScheduleQuery::new(
                ch_a,
                PageRequest::after(first.next().unwrap().clone(), limit),
            )
            .with_from(instant(20)),
        )
        .unwrap();
    assert_eq!(next.items()[0].ends_at(), instant(30));
    assert!(next.next().is_none());
}

#[test]
fn schedule_overlap_index_locates_deep_history_in_logarithmic_probes() {
    const SCHEDULE_LENGTH: usize = 4_096;
    const FIRST_POSSIBLE: usize = 4_000;

    let channels = vec![
        parsed_channel_with_tvg("deep.a", "Deep History A", "News"),
        parsed_channel_with_tvg("deep.b", "Deep History B", "News"),
    ];
    let mut programmes = Vec::with_capacity(SCHEDULE_LENGTH * 2);
    for ch in ["deep.a", "deep.b"] {
        for i in 0..SCHEDULE_LENGTH {
            programmes.push(parsed_programme_for(
                ch,
                (i as i64) * 10,
                (i as i64 + 1) * 10,
            ));
        }
    }
    let guide = ParsedGuide {
        channels: vec![
            ParsedGuideChannel {
                id: Arc::from("deep.a"),
                display_names: vec![],
            },
            ParsedGuideChannel {
                id: Arc::from("deep.b"),
                display_names: vec![],
            },
        ],
        programmes,
    };
    let cat = ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));
    let mut schedules = [
        cat.flat.channel_schedule_range(0),
        cat.flat.channel_schedule_range(1),
    ];
    schedules.sort_by_key(|range| range.start);
    assert_eq!(schedules[0], 0..SCHEDULE_LENGTH);
    assert_eq!(schedules[1], SCHEDULE_LENGTH..SCHEDULE_LENGTH * 2);

    // Query right at the end of entry `FIRST_POSSIBLE - 1` on both the
    // zero-offset schedule and the nonzero-offset schedule.
    let target_instant = instant((FIRST_POSSIBLE as i64) * 10);
    for schedule in &schedules {
        let probes = Cell::new(0_usize);
        let offset = cat
            .flat
            .first_possible_overlap_with_probe(schedule, target_instant, || {
                probes.set(probes.get() + 1);
            });
        assert_eq!(offset, schedule.start + FIRST_POSSIBLE);
        assert!(
            probes.get() <= 14,
            "4,096 historical entries at offset {} should require logarithmic probes, observed {}",
            schedule.start,
            probes.get()
        );
    }
}

#[test]
fn quality_variants_share_the_first_icon_their_guide_row_names() {
    let built = catalog(
        vec![
            parsed_channel("Arena SD", "Sport"),
            with_icon(
                parsed_channel("Arena HD", "Sport"),
                "https://icons.example/arena-hd.png",
            ),
            with_icon(
                parsed_channel("Arena FHD", "Sport"),
                "https://icons.example/arena-fhd.png",
            ),
            with_icon(
                parsed_channel("Solo", "Sport"),
                "http://icons.example/shared.jpg",
            ),
            with_icon(
                parsed_channel("Other", "Sport"),
                "http://icons.example/shared.jpg",
            ),
            parsed_channel("Bare", "Sport"),
        ],
        generation(1),
    );
    let page = built
        .channels_page(&ChannelQuery::all(PageRequest::first(
            PageLimit::new(10).unwrap(),
        )))
        .unwrap();
    let icons: Vec<(&str, Option<&str>)> = page
        .items()
        .iter()
        .map(|channel| (channel.name(), channel.icon().map(|icon| icon.as_str())))
        .collect();
    assert_eq!(
        icons,
        vec![
            ("Arena SD", Some("https://icons.example/arena-hd.png")),
            ("Arena HD", Some("https://icons.example/arena-hd.png")),
            ("Arena FHD", Some("https://icons.example/arena-hd.png")),
            ("Solo", Some("http://icons.example/shared.jpg")),
            ("Other", Some("http://icons.example/shared.jpg")),
            ("Bare", None),
        ]
    );

    // A single variant fetched on its own reports its row's icon too.
    let details = built.channel(page.items()[2].id()).unwrap();
    assert_eq!(
        details.icon().map(|icon| icon.as_str()),
        Some("https://icons.example/arena-hd.png")
    );

    // Only the icons a row resolves to are stored, each once.
    assert_eq!(
        built.flat.as_bytes().len()
            - catalog(
                vec![
                    parsed_channel("Arena SD", "Sport"),
                    parsed_channel("Arena HD", "Sport"),
                    parsed_channel("Arena FHD", "Sport"),
                    parsed_channel("Solo", "Sport"),
                    parsed_channel("Other", "Sport"),
                    parsed_channel("Bare", "Sport"),
                ],
                generation(1),
            )
            .flat
            .as_bytes()
            .len(),
        2 * size_of::<super::flat::Span>()
            + "https://icons.example/arena-hd.png".len()
            + "http://icons.example/shared.jpg".len()
    );
}

#[test]
fn channel_icons_survive_a_mapped_reopen_and_reach_search_and_guide_rows() {
    let channels = vec![
        with_icon(
            parsed_channel_with_tvg("arena.tv", "Arena HD", "Sport"),
            "https://icons.example/arena.png",
        ),
        parsed_channel_with_tvg("news.tv", "News", "Sport"),
    ];
    let guide = ParsedGuide {
        channels: vec![ParsedGuideChannel {
            id: Arc::from("arena.tv"),
            display_names: vec![Arc::from("Arena")],
        }],
        programmes: vec![ParsedProgramme {
            title: Arc::from("Arena Live"),
            ..parsed_programme_for("arena.tv", 0, 100)
        }],
    };
    let built =
        ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation(1));
    let reopened = ChannelCatalog::from_cache_mapping(
        crate::ports::CatalogCacheMapping::new(built.as_bytes().to_vec()),
        generation(1),
    )
    .expect("a built catalog with icons validates");
    let limit = PageLimit::new(10).unwrap();

    let search = reopened
        .search(&SearchRequest::new(
            SearchTerm::parse("arena").unwrap(),
            PageRequest::first(limit),
            PageRequest::first(limit),
        ))
        .unwrap();
    assert_eq!(
        search.channels().items()[0]
            .icon()
            .map(|icon| icon.as_str()),
        Some("https://icons.example/arena.png")
    );
    let hit = &search.programmes().items()[0];
    assert_eq!(
        hit.channel().icon().map(|icon| icon.as_str()),
        Some("https://icons.example/arena.png")
    );

    let rows = reopened
        .guide_window(
            &GuideWindowQuery::new(
                instant(0),
                instant(3_600),
                ChannelQuery::all(PageRequest::first(limit)),
            )
            .unwrap(),
        )
        .unwrap();
    let icons: Vec<Option<&str>> = rows
        .items()
        .iter()
        .map(|row| row.channel().icon().map(|icon| icon.as_str()))
        .collect();
    assert_eq!(icons, vec![Some("https://icons.example/arena.png"), None]);
}

#[test]
fn structural_validator_rejects_corrupt_channel_icons() {
    let built = catalog(
        vec![
            with_icon(
                parsed_channel("Alpha", "News"),
                "https://icons.example/alpha.png",
            ),
            with_icon(
                parsed_channel("Beta", "News"),
                "https://icons.example/beta.png",
            ),
        ],
        generation(1),
    );
    let base = built.as_bytes().to_vec();
    let reopen = |bytes: Vec<u8>| {
        ChannelCatalog::from_cache_mapping(
            crate::ports::CatalogCacheMapping::new(bytes),
            generation(1),
        )
    };
    assert!(reopen(base.clone()).is_some());

    // icon_index lives at bytes 48..52 of each ChannelHot record.
    let channels_hot = section_offset(&base, Section::ChannelsHot);
    let mut out_of_range = base.clone();
    out_of_range[channels_hot + 48..channels_hot + 52].copy_from_slice(&2_u32.to_le_bytes());
    assert!(reopen(out_of_range).is_none());

    // An icon that is no longer an absolute HTTP(S) location.
    let mut bad_scheme = base.clone();
    let needle = b"https://icons.example/beta.png";
    let pos = bad_scheme
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("icon exists in flat buffer");
    bad_scheme[pos..pos + 8].copy_from_slice(b"ftp://ab");
    assert!(reopen(bad_scheme).is_none());

    // Aliasing the second icon span onto the first.
    let icon_spans = section_offset(&base, Section::IconSpans);
    let mut aliased = base.clone();
    let span0 = aliased[icon_spans..icon_spans + 8].to_vec();
    aliased[icon_spans + 8..icon_spans + 16].copy_from_slice(&span0);
    assert!(reopen(aliased).is_none());
}

/// Byte position of a section's directory entry in the flat header.
fn section_entry(section_index: usize) -> usize {
    size_of::<FlatHeader>() - SECTION_COUNT * 16 + section_index * 16
}

fn section_offset(bytes: &[u8], section: Section) -> usize {
    let pos = section_entry(section as usize);
    u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap()) as usize
}

fn catalog(parsed: Vec<ParsedChannel>, generation: CatalogGeneration) -> ChannelCatalog {
    ChannelCatalog::from_parsed(&configuration(), &parsed, None, generation)
}

fn rich_catalog(generation: CatalogGeneration) -> ChannelCatalog {
    let channels = vec![
        parsed_channel_with_tvg("news.world", "World News", "Sport"),
        parsed_channel_with_tvg("arena.tv", "Arena SD", "Sport"),
        parsed_channel_with_tvg("arena.tv", "Arena HD", "Sport"),
        parsed_channel_with_tvg("arena.tv", "Arena FHD", "Sport"),
        parsed_channel_with_tvg("arena.tv", "Arena 4K", "Sport"),
    ];
    let guide = ParsedGuide {
        channels: vec![
            ParsedGuideChannel {
                id: Arc::from("arena.tv"),
                display_names: vec![Arc::from("Arena")],
            },
            ParsedGuideChannel {
                id: Arc::from("unused.tv"),
                display_names: vec![Arc::from("Unused")],
            },
        ],
        programmes: vec![
            ParsedProgramme {
                guide_channel_id: Arc::from("arena.tv"),
                title: Arc::from("Arena Marathon"),
                description: Some(Arc::from("Long running broadcast")),
                starts_at: instant(0),
                ends_at: instant(100),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("arena.tv"),
                title: Arc::from("Arena Short"),
                description: None,
                starts_at: instant(10),
                ends_at: instant(20),
            },
            ParsedProgramme {
                guide_channel_id: Arc::from("unused.tv"),
                title: Arc::from("Unused Programme"),
                description: Some(Arc::from("Unmatched")),
                starts_at: instant(30),
                ends_at: instant(40),
            },
        ],
    };
    ChannelCatalog::from_parsed(&configuration(), &channels, Some(&guide), generation)
}

fn generation(discriminator: u8) -> CatalogGeneration {
    configuration().catalog_generation(&[discriminator; 32], None)
}

fn parsed_programme_for(channel: &str, starts_at: i64, ends_at: i64) -> ParsedProgramme {
    ParsedProgramme {
        guide_channel_id: Arc::from(channel),
        title: Arc::from("Fixture"),
        description: None,
        starts_at: instant(starts_at),
        ends_at: instant(ends_at),
    }
}

fn instant(timestamp: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(timestamp, 0).expect("fixture timestamp is representable")
}

fn configuration() -> SourceConfiguration {
    SourceConfiguration::parse(SourceConfigurationInput::new(
        "https://provider.fixture.invalid/property.m3u",
        None::<String>,
    ))
    .expect("fixture Source Configuration is valid")
}

fn parsed_channel(name: &str, group: &str) -> ParsedChannel {
    parsed_channel_with_tvg("", name, group)
}

fn parsed_channel_with_tvg(tvg_id: &str, name: &str, group: &str) -> ParsedChannel {
    ParsedChannel {
        tvg_id: Arc::from(tvg_id),
        name: Arc::from(name),
        group: Arc::from(group),
        playback: Arc::from("https://media.fixture.invalid/channel"),
        icon: None,
    }
}

fn with_icon(channel: ParsedChannel, icon: &str) -> ParsedChannel {
    ParsedChannel {
        icon: Some(Arc::from(icon)),
        ..channel
    }
}

fn unique_channels(count: usize, reversed: bool) -> Vec<ParsedChannel> {
    let indices: Box<dyn Iterator<Item = usize>> = if reversed {
        Box::new((0..count).rev())
    } else {
        Box::new(0..count)
    };
    indices
        .map(|index| ParsedChannel {
            tvg_id: Arc::from(""),
            name: Arc::from(format!("Channel {index:04}")),
            group: Arc::from(format!("Group {:02}", index % 11)),
            playback: Arc::from(format!(
                "https://media.fixture.invalid/channel/{index}?token=private-{index}"
            )),
            icon: None,
        })
        .collect()
}

fn duplicate_channels(count: usize, reversed: bool) -> Vec<ParsedChannel> {
    let indices: Box<dyn Iterator<Item = usize>> = if reversed {
        Box::new((0..count).rev())
    } else {
        Box::new(0..count)
    };
    indices
        .map(|index| ParsedChannel {
            tvg_id: Arc::from(""),
            name: Arc::from("Duplicate"),
            group: Arc::from("News"),
            playback: Arc::from(format!(
                "https://media.fixture.invalid/backup/{index}?token=private-{index}"
            )),
            icon: None,
        })
        .collect()
}

fn ids_by_name(catalog: &ChannelCatalog) -> BTreeMap<String, String> {
    catalog.channel_names_and_ids().into_iter().collect()
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
