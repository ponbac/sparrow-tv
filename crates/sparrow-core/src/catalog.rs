use std::{cmp::Ordering, collections::HashMap, ops::Range, sync::Arc};

use crate::{
    domain::{
        CatalogGeneration, ChannelDetails, ChannelGroupView, ChannelId, ChannelQuery,
        ChannelSummary, CoreError, CursorQueryHash, GuideProgramme, GuideWindowChannel,
        GuideWindowQuery, Page, PageRequest, ProgrammeSearchHit, ProgrammeSummary,
        ResolvedPlaybackSource, ScheduleQuery, SearchRequest, SearchResults, SearchTerm,
        SecretPlaybackLocation, SourceConfiguration,
    },
    identity,
    m3u::ParsedChannel,
    xmltv::{ParsedGuide, ParsedProgramme},
};

mod schedule;
mod search;

use schedule::{CatalogProgramme, ScheduleOverlapIndex, build_programmes};
#[cfg(test)]
use search::{MatchCategory, SEARCH_CANCELLATION_CHECKPOINT_BYTES, SearchDocument};
use search::{SearchIndex, SearchIndexBuilder, SearchMatcher, never_cancelled, ranked_selection};

const CURSOR_QUERY_DOMAIN: &[u8] = b"sparrow-page-query-v1\0";
const GROUPS_QUERY_TAG: u8 = 0;
const ALL_CHANNELS_QUERY_TAG: u8 = 1;
const GROUP_CHANNELS_QUERY_TAG: u8 = 2;
const SCHEDULE_QUERY_TAG: u8 = 3;
const CHANNEL_SEARCH_QUERY_TAG: u8 = 4;
const PROGRAMME_SEARCH_QUERY_TAG: u8 = 5;
const GUIDE_WINDOW_QUERY_TAG: u8 = 6;

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
    schedule_overlap_index: ScheduleOverlapIndex,
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
        let schedule_overlap_index = ScheduleOverlapIndex::build(&programmes, guide.as_deref());

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
            schedule_overlap_index,
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
        let query_hash = match query.group() {
            None => query_hash(ALL_CHANNELS_QUERY_TAG, None),
            Some(group) => query_hash(GROUP_CHANNELS_QUERY_TAG, Some(group)),
        };
        Page::from_projection(
            self.generation,
            &self.channels,
            self.channel_range(query.group()),
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

    pub(crate) fn guide_window(
        &self,
        query: &GuideWindowQuery,
    ) -> Result<Page<GuideWindowChannel>, CoreError> {
        let channels = query.channels();
        Page::from_projection(
            self.generation,
            &self.channels,
            self.channel_range(channels.group()),
            channels.page(),
            guide_window_query_hash(query),
            |channel| self.guide_window_channel(channel, query),
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
        let mut matcher = SearchMatcher::new(request.term());
        let channel_page =
            self.search_channels_with_matcher(request.channels(), &mut matcher, is_cancelled)?;
        let programme_page = self.search_programmes_with_matcher(
            request.programmes(),
            &mut matcher,
            is_cancelled,
            |catalog, programme| catalog.programme_search_hit(programme),
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
        let mut matcher = SearchMatcher::new(term);
        self.search_channels_with_matcher(page, &mut matcher, is_cancelled)
    }

    fn search_channels_with_matcher(
        &self,
        page: &PageRequest,
        matcher: &mut SearchMatcher<'_>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Page<ChannelSummary>, CoreError> {
        Page::from_bounded_selection_projection(
            self.generation,
            &self.channels,
            page,
            query_hash(CHANNEL_SEARCH_QUERY_TAG, Some(matcher.term)),
            |prefix_len| ranked_selection(&self.channel_search, matcher, prefix_len, is_cancelled),
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
        let mut matcher = SearchMatcher::new(term);
        self.search_programmes_with_matcher(
            page,
            &mut matcher,
            is_cancelled,
            |catalog, programme| catalog.programme_summary(programme),
        )
    }

    fn search_programmes_with_matcher<T>(
        &self,
        page: &PageRequest,
        matcher: &mut SearchMatcher<'_>,
        is_cancelled: &dyn Fn() -> bool,
        project: impl Fn(&Self, &CatalogProgramme) -> T,
    ) -> Result<Page<T>, CoreError> {
        Page::from_bounded_selection_projection(
            self.generation,
            &self.programmes,
            page,
            query_hash(PROGRAMME_SEARCH_QUERY_TAG, Some(matcher.term)),
            |prefix_len| {
                ranked_selection(&self.programme_search, matcher, prefix_len, is_cancelled)
            },
            |programme| project(self, programme),
        )
    }

    fn source_channel(&self, channel: &CatalogChannel) -> &ParsedChannel {
        &self.source_channels[channel.source_index]
    }

    fn channel_range(&self, group: Option<&str>) -> Range<usize> {
        match group {
            None => 0..self.channels.len(),
            Some(group) => self.group_ranges.get(group).cloned().unwrap_or(0..0),
        }
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
        let source = self.source_programme(programme);
        ProgrammeSummary::new(
            programme.channel_id.clone(),
            Arc::clone(&source.title),
            source.description.as_ref().map(Arc::clone),
            source.starts_at,
            source.ends_at,
        )
    }

    fn programme_search_hit(&self, programme: &CatalogProgramme) -> ProgrammeSearchHit {
        let channel_index = self
            .by_id
            .get(&programme.channel_id)
            .expect("catalogued Programmes always reference a catalogued Channel");
        let source = self.source_programme(programme);
        ProgrammeSearchHit::new(
            self.channel_summary(&self.channels[*channel_index]),
            Arc::clone(&source.title),
            source.starts_at,
            source.ends_at,
        )
    }

    fn guide_programme(&self, programme: &CatalogProgramme) -> GuideProgramme {
        let source = self.source_programme(programme);
        GuideProgramme::new(Arc::clone(&source.title), source.starts_at, source.ends_at)
    }

    fn source_programme(&self, programme: &CatalogProgramme) -> &ParsedProgramme {
        &self
            .source_guide
            .as_ref()
            .expect("catalogued Programmes have a parsed EPG Source")
            .programmes[programme.source_index]
    }

    fn guide_window_channel(
        &self,
        channel: &CatalogChannel,
        query: &GuideWindowQuery,
    ) -> GuideWindowChannel {
        let channel_id = &channel.id;
        let starts_at = query.starts_at();
        let ends_at = query.ends_at();
        let schedule = self
            .schedule_ranges
            .get(channel_id)
            .cloned()
            .unwrap_or(0..0);
        let first_possible_overlap =
            self.schedule_overlap_index
                .first_possible_overlap(&schedule, |source_index| {
                    self.source_guide
                        .as_ref()
                        .expect("catalogued Programmes have a parsed EPG Source")
                        .programmes[source_index]
                        .ends_at
                        <= starts_at
                });
        let programmes = self.programmes[first_possible_overlap..schedule.end]
            .iter()
            .take_while(|programme| self.source_programme(programme).starts_at < ends_at)
            .filter(|programme| self.source_programme(programme).ends_at > starts_at)
            .map(|programme| self.guide_programme(programme))
            .take(GuideWindowChannel::MAX_PROGRAMMES + 1)
            .collect::<Vec<_>>();
        GuideWindowChannel::new(self.channel_summary(channel), programmes)
    }
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

fn guide_window_query_hash(query: &GuideWindowQuery) -> CursorQueryHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CURSOR_QUERY_DOMAIN);
    hasher.update(&[GUIDE_WINDOW_QUERY_TAG]);
    match query.channels().group() {
        Some(group) => {
            hasher.update(&[1]);
            hasher.update(&(group.len() as u64).to_le_bytes());
            hasher.update(group.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    for instant in [query.starts_at(), query.ends_at()] {
        hasher.update(&instant.timestamp().to_le_bytes());
        hasher.update(&instant.timestamp_subsec_nanos().to_le_bytes());
    }
    CursorQueryHash::new(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests;
