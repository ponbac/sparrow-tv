use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use crate::{
    domain::{
        CatalogGeneration, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
        ChannelSummary, CoreError, CursorQueryHash, Page, PageRequest, ProgrammeSummary,
        ResolvedPlaybackSource, ScheduleQuery, SearchRequest, SearchResults, SearchTerm,
        SecretPlaybackLocation, SourceConfiguration, normalize_search_text,
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
const SEARCH_CANCELLATION_CHECKPOINT_BYTES: usize = 4 * 1024;

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
                playback: SecretPlaybackLocation::new(Arc::clone(&channel.playback)),
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
            let previous = by_id.insert(channel.id, index);
            debug_assert!(previous.is_none());
            summaries.push(summary);
            records.push(ChannelRecord {
                details,
                playback: channel.playback,
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
            let previous = group_ranges.insert(group_name, group_start..group_end);
            debug_assert!(previous.is_none());
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

    pub(crate) fn resolve_playback(
        &self,
        id: &ChannelId,
    ) -> Result<ResolvedPlaybackSource, CoreError> {
        self.by_id
            .get(id)
            .map(|index| ResolvedPlaybackSource::new(self.records[*index].playback.clone()))
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
        self.search_with_cancellation(request, &never_cancelled)
    }

    pub(crate) fn search_with_cancellation(
        &self,
        request: &SearchRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResults, CoreError> {
        let channel_page = self.search_channels_with_cancellation(
            request.term(),
            request.channels(),
            is_cancelled,
        )?;
        let programme_page = self.search_programmes_with_cancellation(
            request.term(),
            request.programmes(),
            is_cancelled,
        )?;

        Ok(SearchResults::new(channel_page, programme_page))
    }

    pub(crate) fn search_channels(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        self.search_channels_with_cancellation(term, page, &never_cancelled)
    }

    pub(crate) fn search_channels_with_cancellation(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        // Ranking allocates one small index record per match. Page construction
        // shallow-clones at most PageLimit read models, whose strings remain
        // shared with the immutable catalog through Arc.
        let matches = ranked_indices(&self.channel_search, term, is_cancelled)?;
        Page::from_selection(
            self.generation,
            &self.summaries,
            &matches,
            page,
            query_hash(CHANNEL_SEARCH_QUERY_TAG, Some(term.as_str())),
        )
    }

    pub(crate) fn search_programmes(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        self.search_programmes_with_cancellation(term, page, &never_cancelled)
    }

    pub(crate) fn search_programmes_with_cancellation(
        &self,
        term: &SearchTerm,
        page: &PageRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ProgrammeSummary>, CoreError> {
        let matches = ranked_indices(&self.programme_search, term, is_cancelled)?;
        Page::from_selection(
            self.generation,
            &self.programmes,
            &matches,
            page,
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(term.as_str())),
        )
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

    fn rank(
        &self,
        term: &SearchTerm,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<SearchRank>, CoreError> {
        let mut best: Option<SearchRank> = None;
        for (field_index, field) in std::iter::once(self.primary.as_str())
            .chain(self.secondary.as_deref())
            .enumerate()
        {
            let Some(category) = field_rank(field, term.as_str(), is_cancelled)? else {
                continue;
            };
            let candidate = SearchRank {
                category,
                field_index,
            };
            best = Some(best.map_or(candidate, |current| current.min(candidate)));
        }
        Ok(best)
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

fn ranked_indices(
    documents: &[SearchDocument],
    term: &SearchTerm,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<usize>, CoreError> {
    const SEARCH_FIELDS: usize = 2;
    const RANK_BUCKETS: usize = 4 * SEARCH_FIELDS;

    // SearchRank has a small finite range. Bucketing keeps the same total order
    // as sorting `(SearchRank, catalog index)` while giving cancellation a
    // checkpoint for every document and every emitted match.
    let mut buckets: [Vec<usize>; RANK_BUCKETS] = std::array::from_fn(|_| Vec::new());
    for (item_index, document) in documents.iter().enumerate() {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        if let Some(rank) = document.rank(term, is_cancelled)? {
            buckets[rank.bucket_index()].push(item_index);
        }
    }

    let match_count = buckets.iter().map(Vec::len).sum();
    let mut ranked = Vec::with_capacity(match_count);
    for bucket in buckets {
        // Items enter each bucket in catalog order, preserving the catalog index
        // as the deterministic final tie breaker without an uninterruptible sort.
        for item_index in bucket {
            if is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            ranked.push(item_index);
        }
    }
    Ok(ranked)
}

impl SearchRank {
    fn bucket_index(self) -> usize {
        let category = match self.category {
            MatchCategory::Exact => 0,
            MatchCategory::Prefix => 1,
            MatchCategory::Token => 2,
            MatchCategory::Substring => 3,
        };
        category * 2 + self.field_index
    }
}

fn never_cancelled() -> bool {
    false
}

fn field_rank(
    field: &str,
    term: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Option<MatchCategory>, CoreError> {
    if field == term {
        return Ok(Some(MatchCategory::Exact));
    }
    if field.starts_with(term) {
        return Ok(Some(MatchCategory::Prefix));
    }
    if contains_all_tokens(field, term, is_cancelled)? {
        return Ok(Some(MatchCategory::Token));
    }
    Ok(contains_substring(field, term, is_cancelled)?.then_some(MatchCategory::Substring))
}

fn contains_all_tokens(
    field: &str,
    term: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<bool, CoreError> {
    if is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let mut missing = term.split(' ').collect::<HashSet<_>>();
    let longest = missing.iter().map(|token| token.len()).max().unwrap_or(0);
    let mut token_start = 0;

    for (chunk_index, chunk) in field
        .as_bytes()
        .chunks(SEARCH_CANCELLATION_CHECKPOINT_BYTES)
        .enumerate()
    {
        if chunk_index != 0 && is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let chunk_start = chunk_index * SEARCH_CANCELLATION_CHECKPOINT_BYTES;
        for (offset, byte) in chunk.iter().enumerate() {
            if *byte != b' ' {
                continue;
            }
            let token_end = chunk_start + offset;
            if token_end - token_start <= longest {
                missing.remove(&field[token_start..token_end]);
                if missing.is_empty() {
                    return Ok(true);
                }
            }
            token_start = token_end + 1;
        }
    }

    if field.len() - token_start <= longest {
        missing.remove(&field[token_start..]);
    }
    Ok(missing.is_empty())
}

fn contains_substring(
    field: &str,
    term: &str,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<bool, CoreError> {
    if is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let needle = term.as_bytes();
    let mut fallback = vec![0; needle.len()];
    let mut prefix_length = 0;
    for index in 1..needle.len() {
        while prefix_length > 0 && needle[index] != needle[prefix_length] {
            prefix_length = fallback[prefix_length - 1];
        }
        if needle[index] == needle[prefix_length] {
            prefix_length += 1;
            fallback[index] = prefix_length;
        }
    }

    let mut matched = 0;
    for (chunk_index, chunk) in field
        .as_bytes()
        .chunks(SEARCH_CANCELLATION_CHECKPOINT_BYTES)
        .enumerate()
    {
        if chunk_index != 0 && is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        for byte in chunk {
            while matched > 0 && *byte != needle[matched] {
                matched = fallback[matched - 1];
            }
            if *byte == needle[matched] {
                matched += 1;
                if matched == needle.len() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
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
        let previous = ranges.insert(channel_id, start..end);
        debug_assert!(previous.is_none());
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
    playback: SecretPlaybackLocation,
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
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
        assert!(baseline.get() <= CatalogGeneration::MAX_SAFE_INTEGER);
    }

    #[test]
    fn search_ranking_is_exact_then_prefix_then_token_then_substring() {
        use super::MatchCategory::{Exact, Prefix, Substring, Token};

        let rank = |field, term| {
            super::field_rank(field, term, &super::never_cancelled)
                .expect("an uncancelled rank succeeds")
        };

        assert_eq!(rank("news", "news"), Some(Exact));
        assert_eq!(rank("news tonight", "news"), Some(Prefix));
        assert_eq!(rank("newsroom evening", "news"), Some(Prefix));
        assert_eq!(rank("evening news bulletin", "news"), Some(Token));
        assert_eq!(rank("goodnews bulletin", "news"), Some(Substring));
        assert_eq!(rank("weather bulletin", "news"), None);
    }

    #[test]
    fn search_observes_cancellation_while_emitting_ranked_matches() {
        let documents = [
            super::SearchDocument::new("News", None),
            super::SearchDocument::new("News", None),
            super::SearchDocument::new("News", None),
        ];
        let term = SearchTerm::parse("news").expect("fixture term is valid");
        let checkpoints = Cell::new(0_usize);

        let result = super::ranked_indices(&documents, &term, &|| {
            let next = checkpoints.get() + 1;
            checkpoints.set(next);
            next > documents.len()
        });

        assert_eq!(result, Err(CoreError::Cancelled));
        assert_eq!(checkpoints.get(), documents.len() + 1);
    }

    #[test]
    fn search_observes_cancellation_inside_one_pathological_field() {
        let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
        let documents = [super::SearchDocument::new(&oversized_field, None)];
        let term = SearchTerm::parse("needle").expect("fixture term is valid");
        let checkpoints = Cell::new(0_u8);

        let result = super::ranked_indices(&documents, &term, &|| {
            let next = checkpoints.get() + 1;
            checkpoints.set(next);
            next >= 3
        });

        assert_eq!(result, Err(CoreError::Cancelled));
        assert_eq!(checkpoints.get(), 3);
    }

    #[test]
    fn substring_matching_observes_cancellation_inside_one_pathological_field() {
        let oversized_field = "x".repeat(super::SEARCH_CANCELLATION_CHECKPOINT_BYTES * 3);
        let checkpoints = Cell::new(0_u8);

        let result = super::contains_substring(&oversized_field, "needle", &|| {
            let next = checkpoints.get() + 1;
            checkpoints.set(next);
            next >= 2
        });

        assert_eq!(result, Err(CoreError::Cancelled));
        assert_eq!(checkpoints.get(), 2);
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
