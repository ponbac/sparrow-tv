use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    mem::size_of,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use static_assertions::assert_not_impl_any;
use url::Url;

use crate::{
    domain::{
        CatalogGeneration, ChannelId, ChannelQuery, CoreError, PageCursor, PageLimit, PageRequest,
        SearchRequest, SearchTerm, SourceConfiguration, SourceConfigurationInput,
    },
    m3u::ParsedChannel,
    xmltv::{ParsedGuide, ParsedProgramme},
};

use super::{CatalogProgramme, ChannelCatalog, ScheduleOverlapIndex};

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
        let expected = catalog
            .channels
            .iter()
            .map(|channel| channel.id.as_str().to_owned())
            .collect::<Vec<_>>();
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
        let expected = catalog
            .channels
            .iter()
            .map(|channel| channel.id.as_str().to_owned())
            .collect::<Vec<_>>();
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
        let forward_ids = forward
            .channels
            .iter()
            .map(|channel| channel.id.as_str().to_owned())
            .collect::<HashSet<_>>();
        let reversed_ids = reversed
            .channels
            .iter()
            .map(|channel| channel.id.as_str().to_owned())
            .collect::<HashSet<_>>();

        prop_assert_eq!(forward_ids.len(), duplicate_count);
        prop_assert_eq!(forward_ids, reversed_ids);
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
    let mut rank = |field| {
        matcher
            .field_rank(field, &super::never_cancelled)
            .expect("an uncancelled rank succeeds")
    };

    assert_eq!(rank("news"), Some(Exact));
    assert_eq!(rank("news tonight"), Some(Prefix));
    assert_eq!(rank("newsroom evening"), Some(Prefix));
    assert_eq!(rank("evening news bulletin"), Some(Token));
    assert_eq!(rank("goodnews bulletin"), Some(Substring));
    assert_eq!(rank("weather bulletin"), None);
}

#[test]
fn search_fields_share_one_arena_instead_of_retaining_per_document_strings() {
    let index = search_index(&[("Alpha", Some("One")), ("Beta", None)]);

    assert_eq!(index.arena, "alphaonebeta");
    assert_eq!(index.documents.len(), 2);
    assert_eq!(size_of::<super::SearchDocument>(), size_of::<usize>() * 4);
    assert_eq!(index.documents[0].primary.get(&index.arena), "alpha");
    assert_eq!(
        index.documents[0]
            .secondary()
            .map(|field| field.get(&index.arena)),
        Some("one")
    );
    assert!(index.documents[1].secondary().is_none());
}

#[test]
fn search_observes_cancellation_while_emitting_ranked_matches() {
    let documents = search_index(&[("News", None), ("News", None), ("News", None)]);
    let term = SearchTerm::parse("news").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);
    let checkpoints = Cell::new(0_usize);

    let result = super::ranked_selection(&documents, &mut matcher, usize::MAX, &|| {
        let next = checkpoints.get() + 1;
        checkpoints.set(next);
        next > documents.len()
    });

    assert!(matches!(result, Err(CoreError::Cancelled)));
    assert_eq!(checkpoints.get(), documents.len() + 1);
}

#[test]
fn search_observes_cancellation_inside_one_pathological_field() {
    let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
    let documents = search_index(&[(&oversized_field, None)]);
    let term = SearchTerm::parse("needle").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);
    let checkpoints = Cell::new(0_u8);

    let result = super::ranked_selection(&documents, &mut matcher, usize::MAX, &|| {
        let next = checkpoints.get() + 1;
        checkpoints.set(next);
        next >= 3
    });

    assert!(matches!(result, Err(CoreError::Cancelled)));
    assert_eq!(checkpoints.get(), 3);
}

#[test]
fn substring_matching_observes_cancellation_inside_one_pathological_field() {
    let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
    let term = SearchTerm::parse("needle").expect("fixture term is valid");
    let matcher = super::SearchMatcher::new(&term);
    let checkpoints = Cell::new(0_u8);

    let result = matcher.contains_substring(&oversized_field, &|| {
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
            .field_rank("alpha only", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        None
    );
    assert_eq!(
        matcher
            .field_rank("beta only", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        None
    );
    assert_eq!(
        matcher
            .field_rank("beta then alpha", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        Some(Token)
    );

    let duplicate_term = SearchTerm::parse("alpha alpha").expect("fixture term is valid");
    let mut duplicate_matcher = super::SearchMatcher::new(&duplicate_term);
    assert_eq!(
        duplicate_matcher
            .field_rank("one alpha token", &super::never_cancelled)
            .expect("an uncancelled rank succeeds"),
        Some(Token),
        "duplicate query tokens retain set semantics"
    );
}

#[test]
fn ranked_selection_keeps_only_the_requested_prefix_and_counts_every_match() {
    let documents = search_index(&[
        ("News", None),
        ("Other", Some("News")),
        ("Newsroom", None),
        ("Other", Some("Newsroom")),
        ("Daily News", None),
        ("Other", Some("Daily News")),
        ("Goodnews", None),
        ("Other", Some("Goodnews")),
        ("News", None),
    ]);
    let term = SearchTerm::parse("news").expect("fixture term is valid");
    let mut matcher = super::SearchMatcher::new(&term);

    let selection = super::ranked_selection(&documents, &mut matcher, 5, &super::never_cancelled)
        .expect("an uncancelled ranking succeeds");

    assert_eq!(selection.total_len(), documents.documents.len());
    assert_eq!(selection.indices(), [0, 8, 1, 2, 3]);
}

#[test]
fn schedule_overlap_index_preserves_long_running_programmes_and_resets_per_channel() {
    let first_channel = fixture_channel_id('a');
    let second_channel = fixture_channel_id('b');
    let guide = ParsedGuide {
        channels: Vec::new(),
        programmes: vec![
            parsed_programme(0, 100),
            parsed_programme(10, 20),
            parsed_programme(20, 30),
            parsed_programme(0, 5),
            parsed_programme(10, 40),
        ],
    };
    let programmes = vec![
        CatalogProgramme {
            channel_id: first_channel.clone(),
            source_index: 0,
        },
        CatalogProgramme {
            channel_id: first_channel.clone(),
            source_index: 1,
        },
        CatalogProgramme {
            channel_id: first_channel,
            source_index: 2,
        },
        CatalogProgramme {
            channel_id: second_channel.clone(),
            source_index: 3,
        },
        CatalogProgramme {
            channel_id: second_channel,
            source_index: 4,
        },
    ];

    let index = ScheduleOverlapIndex::build(&programmes, Some(&guide));

    assert_eq!(&*index.prefix_max_end_sources, &[0, 0, 0, 3, 4]);
    let window_start = instant(50);
    assert_eq!(
        index.first_possible_overlap(&(0..3), |source_index| {
            guide.programmes[source_index].ends_at <= window_start
        }),
        0,
        "the earlier long-running Programme remains a possible overlap"
    );
    assert_eq!(
        index.first_possible_overlap(&(3..5), |source_index| {
            guide.programmes[source_index].ends_at <= window_start
        }),
        5,
        "prefix maxima reset at each Channel schedule boundary"
    );
}

#[test]
fn schedule_overlap_index_locates_deep_history_in_logarithmic_probes() {
    const PADDING: usize = 7;
    const SCHEDULE_LENGTH: usize = 4_096;
    const FIRST_POSSIBLE: usize = 4_000;
    let sources = std::iter::repeat_n(usize::MAX, PADDING)
        .chain(0..SCHEDULE_LENGTH)
        .collect::<Vec<_>>();
    let index = ScheduleOverlapIndex {
        prefix_max_end_sources: sources.into_boxed_slice(),
    };
    let probes = Cell::new(0_usize);

    let offset =
        index.first_possible_overlap(&(PADDING..PADDING + SCHEDULE_LENGTH), |source_index| {
            probes.set(probes.get() + 1);
            source_index < FIRST_POSSIBLE
        });

    assert_eq!(offset, PADDING + FIRST_POSSIBLE);
    assert!(
        probes.get() <= 14,
        "4,096 historical entries should require logarithmic probes, observed {}",
        probes.get()
    );
}

fn catalog(parsed: Vec<ParsedChannel>, generation: CatalogGeneration) -> ChannelCatalog {
    ChannelCatalog::from_parsed(&configuration(), Arc::new(parsed), None, generation)
}

fn search_index(fields: &[(&str, Option<&str>)]) -> super::SearchIndex {
    let mut index = super::SearchIndexBuilder::with_capacity(
        fields.len(),
        fields
            .iter()
            .map(|(primary, secondary)| primary.len() + secondary.map_or(0, |value| value.len()))
            .sum(),
    );
    for (primary, secondary) in fields {
        index.push(primary, *secondary);
    }
    index.finish()
}

fn generation(discriminator: u8) -> CatalogGeneration {
    configuration().catalog_generation(&[discriminator; 32], None)
}

fn fixture_channel_id(discriminator: char) -> ChannelId {
    ChannelId::parse(format!("ch1_{}", discriminator.to_string().repeat(64)))
        .expect("fixture Channel ID is canonical")
}

fn parsed_programme(starts_at: i64, ends_at: i64) -> ParsedProgramme {
    ParsedProgramme {
        guide_channel_id: Arc::from("fixture"),
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
            playback: Arc::new(
                Url::parse(&format!(
                    "https://media.fixture.invalid/channel/{index}?token=private-{index}"
                ))
                .expect("generated playback location is valid"),
            ),
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
            playback: Arc::new(
                Url::parse(&format!(
                    "https://media.fixture.invalid/backup/{index}?token=private-{index}"
                ))
                .expect("generated playback location is valid"),
            ),
        })
        .collect()
}

fn ids_by_name(catalog: &ChannelCatalog) -> BTreeMap<String, String> {
    catalog
        .channels
        .iter()
        .map(|channel| {
            (
                catalog.source_channel(channel).name.to_string(),
                channel.id.as_str().to_owned(),
            )
        })
        .collect()
}
