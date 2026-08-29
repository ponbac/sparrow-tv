use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
    ops::Range,
    sync::Arc,
};

use url::Url;

use crate::{
    domain::{
        CatalogGeneration, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
        ChannelSummary, CoreError, CursorQueryHash, Page, PageRequest, ProgrammeSummary,
        ScheduleQuery, SearchRequest, SearchResults, SearchTerm, SourceConfiguration,
        normalize_search_text,
    },
    identity,
    m3u::ParsedChannel,
    xmltv::{ParsedGuide, ParsedProgramme},
};

const CURSOR_QUERY_DOMAIN: &[u8] = b"sparrow-page-query-v1\0";
const GROUPS_QUERY_TAG: u8 = 0;
const ALL_CHANNELS_QUERY_TAG: u8 = 1;
const GROUP_CHANNELS_QUERY_TAG: u8 = 2;
const SCHEDULE_QUERY_TAG: u8 = 3;
const CHANNEL_SEARCH_QUERY_TAG: u8 = 4;
const PROGRAMME_SEARCH_QUERY_TAG: u8 = 5;

pub(crate) struct ChannelCatalog {
    generation: CatalogGeneration,
    groups: Arc<[ChannelGroupView]>,
    summaries: Arc<[ChannelSummary]>,
    programmes: Arc<[ProgrammeSummary]>,
    records: Vec<ChannelRecord>,
    channel_search: Box<[SearchDocument]>,
    programme_search: Box<[SearchDocument]>,
    group_ranges: HashMap<Arc<str>, Range<usize>>,
    schedule_ranges: HashMap<ChannelId, Range<usize>>,
    by_id: HashMap<ChannelId, usize>,
}

impl ChannelCatalog {
    pub(crate) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: &[ParsedChannel],
        guide: Option<&ParsedGuide>,
        generation: CatalogGeneration,
    ) -> Self {
        let mut occurrences = HashMap::<[u8; 32], u32>::new();
        let mut pending = Vec::with_capacity(parsed.len());

        for channel in parsed {
            let seed = identity::seed(&channel.tvg_id, &channel.name, &channel.group);
            let occurrence = occurrences.entry(seed).or_default();
            let id = identity::channel_id(&configuration.fingerprint, &seed, *occurrence);
            *occurrence = occurrence.saturating_add(1);

            let name = Arc::clone(&channel.name);
            let group = Arc::clone(&channel.group);
            pending.push(PendingChannel {
                group_order: identity::normalize_identity_field(&group),
                name_order: identity::normalize_identity_field(&name),
                tvg_id: Arc::clone(&channel.tvg_id),
                id,
                name,
                group,
                playback: SecretPlaybackLocation(Arc::clone(&channel.playback)),
            });
        }

        pending.sort_unstable_by(compare_channels);

        let (programmes, schedule_ranges) = build_programmes(&pending, guide);

        let mut summaries = Vec::with_capacity(pending.len());
        let mut records = Vec::with_capacity(pending.len());
        let mut by_id = HashMap::with_capacity(pending.len());
        for channel in pending {
            let summary = ChannelSummary::new(
                channel.id.clone(),
                Arc::clone(&channel.name),
                Arc::clone(&channel.group),
            );
            let details = ChannelDetails::new(channel.id.clone(), channel.name, channel.group);
            let index = records.len();
            debug_assert!(by_id.insert(channel.id, index).is_none());
            summaries.push(summary);
            records.push(ChannelRecord {
                details,
                _playback: channel.playback,
            });
        }

        let mut groups = Vec::new();
        let mut group_ranges = HashMap::new();
        let mut group_start = 0;
        while group_start < summaries.len() {
            let group_name: Arc<str> = Arc::from(summaries[group_start].group());
            let mut group_end = group_start + 1;
            while group_end < summaries.len() && summaries[group_end].group() == group_name.as_ref()
            {
                group_end += 1;
            }
            let channel_count = u32::try_from(group_end - group_start)
                .expect("the bounded M3U payload cannot contain more than u32::MAX Channels");
            groups.push(ChannelGroupView::new(
                Arc::clone(&group_name),
                channel_count,
            ));
            debug_assert!(
                group_ranges
                    .insert(group_name, group_start..group_end)
                    .is_none()
            );
            group_start = group_end;
        }

        let channel_search = summaries
            .iter()
            .map(|channel| SearchDocument::new(channel.name(), Some(channel.group())))
            .collect();
        let programme_search = programmes
            .iter()
            .map(|programme| SearchDocument::new(programme.title(), programme.description()))
            .collect();

        Self {
            generation,
            groups: Arc::from(groups),
            summaries: Arc::from(summaries),
            programmes,
            records,
            channel_search,
            programme_search,
            group_ranges,
            schedule_ranges,
            by_id,
        }
    }

    pub(crate) fn groups_page(
        &self,
        request: &PageRequest,
    ) -> Result<Page<ChannelGroupView>, CoreError> {
        Page::from_request(
            self.generation,
            Arc::clone(&self.groups),
            0..self.groups.len(),
            request,
            query_hash(GROUPS_QUERY_TAG, None),
        )
    }

    pub(crate) const fn generation(&self) -> CatalogGeneration {
        self.generation
    }

    pub(crate) fn channels_page(
        &self,
        query: &ChannelQuery,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        let (collection, query_hash) = match query.group() {
            None => (
                0..self.summaries.len(),
                query_hash(ALL_CHANNELS_QUERY_TAG, None),
            ),
            Some(group) => (
                self.group_ranges.get(group).cloned().unwrap_or(0..0),
                query_hash(GROUP_CHANNELS_QUERY_TAG, Some(group)),
            ),
        };
        Page::from_request(
            self.generation,
            Arc::clone(&self.summaries),
            collection,
            query.page(),
            query_hash,
        )
    }

    pub(crate) fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.by_id
            .get(id)
            .map(|index| self.records[*index].details.clone())
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })
    }

    pub(crate) fn schedule(
        &self,
        query: &ScheduleQuery,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        if !self.by_id.contains_key(query.channel_id()) {
            return Err(CoreError::ChannelNotFound {
                id: query.channel_id().clone(),
            });
        }
        let collection = self
            .schedule_ranges
            .get(query.channel_id())
            .cloned()
            .unwrap_or(0..0);
        Page::from_request(
            self.generation,
            Arc::clone(&self.programmes),
            collection,
            query.page(),
            query_hash(SCHEDULE_QUERY_TAG, Some(query.channel_id().as_str())),
        )
    }

    pub(crate) fn search(&self, request: &SearchRequest) -> Result<SearchResults, CoreError> {
        // Ranking allocates one small index record per match. Page construction
        // shallow-clones at most PageLimit read models, whose strings remain
        // shared with the immutable catalog through Arc.
        let channel_matches = ranked_indices(&self.channel_search, request.term());
        let channel_page = Page::from_selection(
            self.generation,
            &self.summaries,
            &channel_matches,
            request.channels(),
            query_hash(CHANNEL_SEARCH_QUERY_TAG, Some(request.term().as_str())),
        )?;

        let programme_matches = ranked_indices(&self.programme_search, request.term());
        let programme_page = Page::from_selection(
            self.generation,
            &self.programmes,
            &programme_matches,
            request.programmes(),
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(request.term().as_str())),
        )?;

        Ok(SearchResults::new(channel_page, programme_page))
    }
}

struct SearchDocument {
    primary: String,
    secondary: Option<String>,
}

impl SearchDocument {
    fn new(primary: &str, secondary: Option<&str>) -> Self {
        Self {
            primary: normalize_search_text(primary),
            secondary: secondary.map(normalize_search_text),
        }
    }

    fn rank(&self, term: &SearchTerm) -> Option<SearchRank> {
        std::iter::once(self.primary.as_str())
            .chain(self.secondary.as_deref())
            .enumerate()
            .filter_map(|(field_index, field)| {
                field_rank(field, term.as_str()).map(|category| SearchRank {
                    category,
                    field_index,
                })
            })
            .min()
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct SearchRank {
    category: MatchCategory,
    field_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MatchCategory {
    Exact,
    Prefix,
    Token,
    Substring,
}

fn ranked_indices(documents: &[SearchDocument], term: &SearchTerm) -> Vec<usize> {
    let mut ranked = documents
        .iter()
        .enumerate()
        .filter_map(|(item_index, document)| document.rank(term).map(|rank| (rank, item_index)))
        .collect::<Vec<_>>();
    // The deterministic catalog item index is the final total-order tie breaker.
    ranked.sort_unstable();
    ranked
        .into_iter()
        .map(|(_, item_index)| item_index)
        .collect()
}

fn field_rank(field: &str, term: &str) -> Option<MatchCategory> {
    if field == term {
        return Some(MatchCategory::Exact);
    }
    if field.starts_with(term) {
        return Some(MatchCategory::Prefix);
    }
    if term
        .split(' ')
        .all(|query_token| field.split(' ').any(|token| token == query_token))
    {
        return Some(MatchCategory::Token);
    }
    field.contains(term).then_some(MatchCategory::Substring)
}

struct PendingChannel {
    group_order: String,
    name_order: String,
    tvg_id: Arc<str>,
    id: ChannelId,
    name: Arc<str>,
    group: Arc<str>,
    playback: SecretPlaybackLocation,
}

fn compare_channels(left: &PendingChannel, right: &PendingChannel) -> Ordering {
    left.group_order
        .cmp(&right.group_order)
        .then_with(|| left.group.cmp(&right.group))
        .then_with(|| left.name_order.cmp(&right.name_order))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.id.as_str().cmp(right.id.as_str()))
}

fn query_hash(tag: u8, discriminator: Option<&str>) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[tag]);
    if let Some(discriminator) = discriminator {
        hasher.update(&(discriminator.len() as u64).to_le_bytes());
        hasher.update(discriminator.as_bytes());
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

fn build_programmes(
    channels: &[PendingChannel],
    guide: Option<&ParsedGuide>,
) -> (Arc<[ProgrammeSummary]>, HashMap<ChannelId, Range<usize>>) {
    let Some(guide) = guide else {
        return (Arc::from([]), HashMap::new());
    };
    let mut guide_ids = HashSet::with_capacity(guide.channels.len());
    let mut guide_names = HashMap::<String, Option<Arc<str>>>::new();
    for channel in &guide.channels {
        guide_ids.insert(Arc::clone(&channel.id));
        for display_name in &channel.display_names {
            let normalized = identity::normalize_identity_field(display_name);
            if normalized.is_empty() {
                continue;
            }
            guide_names
                .entry(normalized)
                .and_modify(|candidate| {
                    if candidate.as_deref() != Some(channel.id.as_ref()) {
                        *candidate = None;
                    }
                })
                .or_insert_with(|| Some(Arc::clone(&channel.id)));
        }
    }

    let mut m3u_name_counts = HashMap::<String, usize>::new();
    for channel in channels {
        *m3u_name_counts
            .entry(channel.name_order.clone())
            .or_default() += 1;
    }

    let mut matched_channels = HashMap::<Arc<str>, Vec<ChannelId>>::new();
    for channel in channels {
        let exact_id = channel.tvg_id.trim();
        let guide_id = if exact_id.is_empty() {
            (m3u_name_counts.get(&channel.name_order) == Some(&1))
                .then(|| guide_names.get(&channel.name_order).and_then(Clone::clone))
                .flatten()
        } else {
            guide_ids.contains(exact_id).then(|| Arc::from(exact_id))
        };
        if let Some(guide_id) = guide_id {
            matched_channels
                .entry(guide_id)
                .or_default()
                .push(channel.id.clone());
        }
    }

    let mut pending = Vec::new();
    for (source_ordinal, programme) in guide.programmes.iter().enumerate() {
        let Some(channel_ids) = matched_channels.get(&programme.guide_channel_id) else {
            continue;
        };
        for channel_id in channel_ids {
            pending.push(PendingProgrammeSummary::new(
                channel_id.clone(),
                programme,
                source_ordinal,
            ));
        }
    }
    pending.sort_unstable_by(compare_programmes);

    let programmes = pending
        .into_iter()
        .map(PendingProgrammeSummary::into_summary)
        .collect::<Vec<_>>();
    let mut ranges = HashMap::new();
    let mut start = 0;
    while start < programmes.len() {
        let channel_id = programmes[start].channel_id().clone();
        let mut end = start + 1;
        while end < programmes.len() && programmes[end].channel_id() == &channel_id {
            end += 1;
        }
        debug_assert!(ranges.insert(channel_id, start..end).is_none());
        start = end;
    }

    (Arc::from(programmes), ranges)
}

struct PendingProgrammeSummary {
    channel_id: ChannelId,
    title: Arc<str>,
    description: Option<Arc<str>>,
    starts_at: chrono::DateTime<chrono::Utc>,
    ends_at: chrono::DateTime<chrono::Utc>,
    source_ordinal: usize,
}

impl PendingProgrammeSummary {
    fn new(channel_id: ChannelId, programme: &ParsedProgramme, source_ordinal: usize) -> Self {
        Self {
            channel_id,
            title: Arc::clone(&programme.title),
            description: programme.description.as_ref().map(Arc::clone),
            starts_at: programme.starts_at,
            ends_at: programme.ends_at,
            source_ordinal,
        }
    }

    fn into_summary(self) -> ProgrammeSummary {
        ProgrammeSummary::new(
            self.channel_id,
            self.title,
            self.description,
            self.starts_at,
            self.ends_at,
        )
    }
}

fn compare_programmes(left: &PendingProgrammeSummary, right: &PendingProgrammeSummary) -> Ordering {
    left.channel_id
        .as_str()
        .cmp(right.channel_id.as_str())
        .then_with(|| left.starts_at.cmp(&right.starts_at))
        .then_with(|| left.ends_at.cmp(&right.ends_at))
        .then_with(|| left.title.cmp(&right.title))
        .then_with(|| left.description.cmp(&right.description))
        .then_with(|| left.source_ordinal.cmp(&right.source_ordinal))
}

struct ChannelRecord {
    details: ChannelDetails,
    _playback: SecretPlaybackLocation,
}

struct SecretPlaybackLocation(Arc<Url>);

impl fmt::Debug for SecretPlaybackLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.0;
        formatter.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashSet},
        sync::Arc,
    };

    use proptest::prelude::*;
    use static_assertions::assert_not_impl_any;
    use url::Url;

    use crate::{
        domain::{
            CatalogGeneration, ChannelQuery, CoreError, PageCursor, PageLimit, PageRequest,
            SearchRequest, SearchTerm, SourceConfiguration, SourceConfigurationInput,
        },
        m3u::ParsedChannel,
    };

    use super::ChannelCatalog;

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
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
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
        fn search_cursor_round_trips_visit_every_ranked_channel_once(
            channel_count in 1_usize..300,
            limit in 1_u16..=100,
        ) {
            let catalog = catalog(
                unique_channels(channel_count, false),
                generation(1),
            );
            let expected = catalog
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
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
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
                .collect::<HashSet<_>>();
            let reversed_ids = reversed
                .summaries
                .iter()
                .map(|channel| channel.id().as_str().to_owned())
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
    }

    #[test]
    fn search_ranking_is_exact_then_prefix_then_token_then_substring() {
        use super::MatchCategory::{Exact, Prefix, Substring, Token};

        assert_eq!(super::field_rank("news", "news"), Some(Exact));
        assert_eq!(super::field_rank("news tonight", "news"), Some(Prefix));
        assert_eq!(super::field_rank("newsroom evening", "news"), Some(Prefix));
        assert_eq!(
            super::field_rank("evening news bulletin", "news"),
            Some(Token)
        );
        assert_eq!(
            super::field_rank("goodnews bulletin", "news"),
            Some(Substring)
        );
        assert_eq!(super::field_rank("weather bulletin", "news"), None);
    }

    fn catalog(parsed: Vec<ParsedChannel>, generation: CatalogGeneration) -> ChannelCatalog {
        ChannelCatalog::from_parsed(&configuration(), &parsed, None, generation)
    }

    fn generation(discriminator: u8) -> CatalogGeneration {
        configuration().catalog_generation(&[discriminator; 32], None)
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
            .summaries
            .iter()
            .map(|channel| (channel.name().to_owned(), channel.id().as_str().to_owned()))
            .collect()
    }
}
