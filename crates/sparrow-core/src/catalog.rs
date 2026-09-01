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
    xmltv::ParsedGuide,
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
    source_channels: Arc<Vec<ParsedChannel>>,
    source_guide: Option<Arc<ParsedGuide>>,
    groups: Arc<[ChannelGroupView]>,
    channels: Box<[CatalogChannel]>,
    programmes: Box<[CatalogProgramme]>,
    channel_search: SearchIndex,
    programme_search: SearchIndex,
    group_ranges: HashMap<Arc<str>, Range<usize>>,
    schedule_ranges: HashMap<ChannelId, Range<usize>>,
    by_id: HashMap<ChannelId, usize>,
}

impl ChannelCatalog {
    pub(crate) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: Arc<Vec<ParsedChannel>>,
        guide: Option<Arc<ParsedGuide>>,
        generation: CatalogGeneration,
    ) -> Self {
        let mut occurrences = HashMap::<[u8; 32], u32>::new();
        let mut pending = Vec::with_capacity(parsed.len());

        for (source_index, channel) in parsed.iter().enumerate() {
            let seed = identity::seed(&channel.tvg_id, &channel.name, &channel.group);
            let occurrence = occurrences.entry(seed).or_default();
            let id = identity::channel_id(&configuration.fingerprint, &seed, *occurrence);
            *occurrence = occurrence.saturating_add(1);

            pending.push(PendingChannel {
                source_index,
                group_order: identity::normalize_identity_field(&channel.group),
                name_order: identity::normalize_identity_field(&channel.name),
                tvg_id: Arc::clone(&channel.tvg_id),
                id,
            });
        }

        pending.sort_unstable_by(|left, right| compare_channels(left, right, &parsed));

        let (programmes, schedule_ranges) = build_programmes(&pending, guide.as_deref());

        let mut channels = Vec::with_capacity(pending.len());
        let mut by_id = HashMap::with_capacity(pending.len());
        for channel in &pending {
            let index = channels.len();
            let previous = by_id.insert(channel.id.clone(), index);
            debug_assert!(previous.is_none());
            channels.push(CatalogChannel {
                id: channel.id.clone(),
                source_index: channel.source_index,
            });
        }

        let mut groups = Vec::new();
        let mut group_ranges = HashMap::new();
        let mut group_start = 0;
        while group_start < channels.len() {
            let group_name = Arc::clone(&parsed[channels[group_start].source_index].group);
            let mut group_end = group_start + 1;
            while group_end < channels.len()
                && parsed[channels[group_end].source_index].group == group_name
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

        let mut channel_search = SearchIndexBuilder::with_capacity(
            channels.len(),
            channels.iter().fold(0, |bytes, channel| {
                let source = &parsed[channel.source_index];
                bytes.saturating_add(source.name.len() + source.group.len())
            }),
        );
        for channel in &channels {
            let source = &parsed[channel.source_index];
            channel_search.push(&source.name, Some(&source.group));
        }
        let channel_search = channel_search.finish();
        let mut programme_search = SearchIndexBuilder::with_capacity(
            programmes.len(),
            programmes.iter().fold(0, |bytes, programme| {
                let source = &guide
                    .as_ref()
                    .expect("catalogued Programmes have a parsed EPG Source")
                    .programmes[programme.source_index];
                bytes.saturating_add(
                    source.title.len()
                        + source
                            .description
                            .as_ref()
                            .map_or(0, |description| description.len()),
                )
            }),
        );
        for programme in &programmes {
            let source = &guide
                .as_ref()
                .expect("catalogued Programmes have a parsed EPG Source")
                .programmes[programme.source_index];
            programme_search.push(&source.title, source.description.as_deref());
        }
        let programme_search = programme_search.finish();

        Self {
            generation,
            source_channels: parsed,
            source_guide: guide,
            groups: Arc::from(groups),
            channels: channels.into_boxed_slice(),
            programmes: programmes.into_boxed_slice(),
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
                0..self.channels.len(),
                query_hash(ALL_CHANNELS_QUERY_TAG, None),
            ),
            Some(group) => (
                self.group_ranges.get(group).cloned().unwrap_or(0..0),
                query_hash(GROUP_CHANNELS_QUERY_TAG, Some(group)),
            ),
        };
        Page::from_projection(
            self.generation,
            &self.channels,
            collection,
            query.page(),
            query_hash,
            |channel| self.channel_summary(channel),
        )
    }

    pub(crate) fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.by_id
            .get(id)
            .map(|index| self.channel_details(&self.channels[*index]))
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })
    }

    pub(crate) fn resolve_playback(
        &self,
        id: &ChannelId,
    ) -> Result<ResolvedPlaybackSource, CoreError> {
        self.by_id
            .get(id)
            .map(|index| {
                let source = self.source_channel(&self.channels[*index]);
                ResolvedPlaybackSource::new(SecretPlaybackLocation::new(Arc::clone(
                    &source.playback,
                )))
            })
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
        Page::from_projection(
            self.generation,
            &self.programmes,
            collection,
            query.page(),
            query_hash(SCHEDULE_QUERY_TAG, Some(query.channel_id().as_str())),
            |programme| self.programme_summary(programme),
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
        Page::from_selection_projection(
            self.generation,
            &self.channels,
            &matches,
            page,
            query_hash(CHANNEL_SEARCH_QUERY_TAG, Some(term.as_str())),
            |channel| self.channel_summary(channel),
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
        Page::from_selection_projection(
            self.generation,
            &self.programmes,
            &matches,
            page,
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(term.as_str())),
            |programme| self.programme_summary(programme),
        )
    }

    fn source_channel(&self, channel: &CatalogChannel) -> &ParsedChannel {
        &self.source_channels[channel.source_index]
    }

    fn channel_summary(&self, channel: &CatalogChannel) -> ChannelSummary {
        let source = self.source_channel(channel);
        ChannelSummary::new(
            channel.id.clone(),
            Arc::clone(&source.name),
            Arc::clone(&source.group),
        )
    }

    fn channel_details(&self, channel: &CatalogChannel) -> ChannelDetails {
        let source = self.source_channel(channel);
        ChannelDetails::new(
            channel.id.clone(),
            Arc::clone(&source.name),
            Arc::clone(&source.group),
        )
    }

    fn programme_summary(&self, programme: &CatalogProgramme) -> ProgrammeSummary {
        let source = &self
            .source_guide
            .as_ref()
            .expect("catalogued Programmes have a parsed EPG Source")
            .programmes[programme.source_index];
        ProgrammeSummary::new(
            programme.channel_id.clone(),
            Arc::clone(&source.title),
            source.description.as_ref().map(Arc::clone),
            source.starts_at,
            source.ends_at,
        )
    }
}

struct SearchIndex {
    // Normalized fields share one allocation; each document retains only offsets
    // instead of one or two independently allocated Strings.
    arena: String,
    documents: Box<[SearchDocument]>,
}

impl SearchIndex {
    #[cfg(test)]
    fn len(&self) -> usize {
        self.documents.len()
    }
}

struct SearchIndexBuilder {
    arena: String,
    documents: Vec<SearchDocument>,
}

impl SearchIndexBuilder {
    fn with_capacity(document_capacity: usize, byte_capacity: usize) -> Self {
        Self {
            arena: String::with_capacity(byte_capacity),
            documents: Vec::with_capacity(document_capacity),
        }
    }

    fn push(&mut self, primary: &str, secondary: Option<&str>) {
        let primary = self.push_field(primary);
        let secondary = secondary.map(|value| self.push_field(value));
        self.documents.push(SearchDocument::new(primary, secondary));
    }

    fn push_field(&mut self, value: &str) -> SearchField {
        let normalized = normalize_search_text(value);
        let start = self.arena.len();
        self.arena.push_str(&normalized);
        let end = self.arena.len();
        SearchField { start, end }
    }

    fn finish(self) -> SearchIndex {
        SearchIndex {
            arena: self.arena,
            documents: self.documents.into_boxed_slice(),
        }
    }
}

#[derive(Clone, Copy)]
struct SearchField {
    start: usize,
    end: usize,
}

struct SearchDocument {
    primary: SearchField,
    secondary_start: usize,
    secondary_end: usize,
}

impl SearchDocument {
    fn new(primary: SearchField, secondary: Option<SearchField>) -> Self {
        let (secondary_start, secondary_end) =
            secondary.map_or((usize::MAX, usize::MAX), |field| (field.start, field.end));
        Self {
            primary,
            secondary_start,
            secondary_end,
        }
    }

    fn rank(
        &self,
        arena: &str,
        term: &SearchTerm,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<SearchRank>, CoreError> {
        let mut best: Option<SearchRank> = None;
        for (field_index, field) in std::iter::once(self.primary.get(arena))
            .chain(self.secondary().map(|field| field.get(arena)))
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

    fn secondary(&self) -> Option<SearchField> {
        (self.secondary_start != usize::MAX).then_some(SearchField {
            start: self.secondary_start,
            end: self.secondary_end,
        })
    }
}

impl SearchField {
    fn get(self, arena: &str) -> &str {
        &arena[self.start..self.end]
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
    index: &SearchIndex,
    term: &SearchTerm,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Vec<usize>, CoreError> {
    const SEARCH_FIELDS: usize = 2;
    const RANK_BUCKETS: usize = 4 * SEARCH_FIELDS;

    // SearchRank has a small finite range. Bucketing keeps the same total order
    // as sorting `(SearchRank, catalog index)` while giving cancellation a
    // checkpoint for every document and every emitted match.
    let mut buckets: [Vec<usize>; RANK_BUCKETS] = std::array::from_fn(|_| Vec::new());
    for (item_index, document) in index.documents.iter().enumerate() {
        if is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        if let Some(rank) = document.rank(&index.arena, term, is_cancelled)? {
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
    source_index: usize,
    group_order: String,
    name_order: String,
    tvg_id: Arc<str>,
    id: ChannelId,
}

struct CatalogChannel {
    id: ChannelId,
    source_index: usize,
}

fn compare_channels(
    left: &PendingChannel,
    right: &PendingChannel,
    parsed: &[ParsedChannel],
) -> Ordering {
    let left_source = &parsed[left.source_index];
    let right_source = &parsed[right.source_index];
    left.group_order
        .cmp(&right.group_order)
        .then_with(|| left_source.group.cmp(&right_source.group))
        .then_with(|| left.name_order.cmp(&right.name_order))
        .then_with(|| left_source.name.cmp(&right_source.name))
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
) -> (Vec<CatalogProgramme>, HashMap<ChannelId, Range<usize>>) {
    let Some(guide) = guide else {
        return (Vec::new(), HashMap::new());
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
    for (source_index, programme) in guide.programmes.iter().enumerate() {
        let Some(channel_ids) = matched_channels.get(&programme.guide_channel_id) else {
            continue;
        };
        for channel_id in channel_ids {
            pending.push(CatalogProgramme {
                channel_id: channel_id.clone(),
                source_index,
            });
        }
    }
    pending.sort_unstable_by(|left, right| compare_programmes(left, right, guide));

    let programmes = pending;
    let mut ranges = HashMap::new();
    let mut start = 0;
    while start < programmes.len() {
        let channel_id = programmes[start].channel_id.clone();
        let mut end = start + 1;
        while end < programmes.len() && programmes[end].channel_id == channel_id {
            end += 1;
        }
        let previous = ranges.insert(channel_id, start..end);
        debug_assert!(previous.is_none());
        start = end;
    }

    (programmes, ranges)
}

struct CatalogProgramme {
    channel_id: ChannelId,
    source_index: usize,
}

fn compare_programmes(
    left: &CatalogProgramme,
    right: &CatalogProgramme,
    guide: &ParsedGuide,
) -> Ordering {
    let left_source = &guide.programmes[left.source_index];
    let right_source = &guide.programmes[right.source_index];
    left.channel_id
        .as_str()
        .cmp(right.channel_id.as_str())
        .then_with(|| left_source.starts_at.cmp(&right_source.starts_at))
        .then_with(|| left_source.ends_at.cmp(&right_source.ends_at))
        .then_with(|| left_source.title.cmp(&right_source.title))
        .then_with(|| left_source.description.cmp(&right_source.description))
        .then_with(|| left.source_index.cmp(&right.source_index))
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        collections::{BTreeMap, HashSet},
        mem::size_of,
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
        let documents = search_index(&[(&oversized_field, None)]);
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
        ChannelCatalog::from_parsed(&configuration(), Arc::new(parsed), None, generation)
    }

    fn search_index(fields: &[(&str, Option<&str>)]) -> super::SearchIndex {
        let mut index = super::SearchIndexBuilder::with_capacity(
            fields.len(),
            fields
                .iter()
                .map(|(primary, secondary)| {
                    primary.len() + secondary.map_or(0, |value| value.len())
                })
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
}
