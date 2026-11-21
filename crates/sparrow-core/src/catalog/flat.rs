//! Zero-copy flat processed catalog format (`sparrow-processed-catalog-v3`).
//!
//! All multi-byte fields use unaligned little-endian `zerocopy` wrappers so
//! both in-memory (`Owned(Vec<u8>)`) and `mmap`-backed (`Mapped(CatalogCacheMapping)`)
//! catalogs share one safe, zero-`unsafe` query implementation with hot, warm,
//! and cold sections separated in file order.

use std::{cmp::Ordering, collections::HashMap, mem::size_of, ops::Range};

use chrono::{DateTime, Datelike, Utc};
use url::Url;
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned,
    little_endian::{I64, U32, U64},
};

use crate::{
    domain::{CatalogGeneration, ChannelQuality, SourceConfiguration, normalize_search_text},
    identity,
    m3u::ParsedChannel,
    ports::{CatalogCacheMapping, SnapshotCandidate, SnapshotStore},
    xmltv::ParsedGuide,
};

use super::{
    schedule::{GuideSource, ScheduleBuildChannel, build_programmes},
    variants,
};

pub(crate) const FLAT_MAGIC: [u8; 12] = *b"SPARROWCAT\x03\x00";
const CACHE_KEY_DOMAIN: &[u8] = b"sparrow-processed-catalog-v3\0";
const FLAG_HAS_EPG: u32 = 1;
pub(super) const ABSENT_U32: u32 = u32::MAX;
pub(super) const UNMATCHED_PROG_BIT: u32 = 0x8000_0000;
const PARALLEL_CHANNEL_THRESHOLD: usize = 1_024;
const PARALLEL_BUILD_THRESHOLD: usize = 512;

/// Private derived storage key binding configuration, source checksums, format
/// version, and core build fingerprint (`SPARROW_CATALOG_BUILD`).
pub(crate) fn cache_key(
    configuration: &SourceConfiguration,
    m3u: &[u8; 32],
    epg: Option<&[u8; 32]>,
) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(CACHE_KEY_DOMAIN);
    hash.update(env!("SPARROW_CATALOG_BUILD").as_bytes());
    hash.update(configuration.fingerprint.as_bytes());
    hash.update(m3u);
    hash.update(&[u8::from(epg.is_some())]);
    if let Some(epg) = epg {
        hash.update(epg);
    }
    *hash.finalize().as_bytes()
}

#[derive(Clone, Copy)]
#[repr(usize)]
enum Section {
    // Tier 1: Hot (startup: groups_page, channels_page page 1)
    Groups = 0,
    GroupByNameSorted = 1,
    GroupStrings = 2,
    GroupSearchStrings = 3,
    ChannelsHot = 4,
    ChannelNames = 5,
    // Tier 2: Warm (guide_window page 1, by_id lookups)
    ChannelsById = 6,
    ChannelSchedules = 7,
    ScheduleEntries = 8,
    ScheduleOverlap = 9,
    MatchedProgrammesHot = 10,
    MatchedProgrammeTitles = 11,
    // Tier 3: Cold (tune, schedule descriptions, search, single-source refresh)
    PlaybackSpans = 12,
    PlaybackStrings = 13,
    TvgIdSpans = 14,
    TvgIdStrings = 15,
    MatchedProgrammeDescSpans = 16,
    MatchedProgrammeDescriptions = 17,
    ChannelSearchSpans = 18,
    ChannelSearchArena = 19,
    ProgrammeSearchSpans = 20,
    ProgrammeSearchArena = 21,
    GuideIdSpans = 22,
    GuideIdStrings = 23,
    GuideChannels = 24,
    GuideDisplaySpans = 25,
    GuideDisplayStrings = 26,
    MatchedProgGuideIds = 27,
    UnmatchedProgrammes = 28,
    UnmatchedProgrammeStrings = 29,
    GuideProgOrder = 30,
}

pub(super) const SECTION_COUNT: usize = 31;

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct SectionRange {
    pub(super) offset: U64,
    pub(super) byte_len: U64,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct FlatHeader {
    pub(super) magic: [u8; 12],
    pub(super) flags: U32,
    pub(super) generation: U64,
    pub(super) group_count: U32,
    pub(super) channel_count: U32,
    pub(super) matched_prog_count: U32,
    pub(super) schedule_entry_count: U32,
    pub(super) guide_id_count: U32,
    pub(super) guide_channel_count: U32,
    pub(super) guide_display_count: U32,
    pub(super) unmatched_prog_count: U32,
    pub(super) total_guide_prog_count: U32,
    pub(super) sections: [SectionRange; SECTION_COUNT],
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct GroupRecord {
    pub(super) name_off: U32,
    pub(super) name_len: U32,
    pub(super) norm_name_off: U32,
    pub(super) norm_name_len: U32,
    pub(super) channel_start: U32,
    pub(super) channel_end: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct ChannelHot {
    pub(super) id_digest: [u8; 32],
    pub(super) name_off: U32,
    pub(super) name_len: U32,
    pub(super) group_index: U32,
    pub(super) number: U32,
    pub(super) quality: u8,
    pub(super) pad: [u8; 3],
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct Span {
    pub(super) off: U32,
    pub(super) len: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct IndexRange {
    pub(super) start: U32,
    pub(super) end: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct ScheduleEntry {
    pub(super) channel_index: U32,
    pub(super) matched_prog_index: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct MatchedProgrammeHot {
    pub(super) starts_at_secs: I64,
    pub(super) ends_at_secs: I64,
    pub(super) starts_at_nanos: U32,
    pub(super) ends_at_nanos: U32,
    pub(super) title_off: U32,
    pub(super) title_len: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct ProgrammeSearchSpan {
    pub(super) primary_off: U32,
    pub(super) primary_len: U32,
    pub(super) secondary_off: U32,
    pub(super) secondary_len: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct GuideChannelRecord {
    pub(super) guide_id_index: U32,
    pub(super) display_start: U32,
    pub(super) display_end: U32,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub(super) struct UnmatchedProgrammeRecord {
    pub(super) guide_id_index: U32,
    pub(super) starts_at_secs: I64,
    pub(super) ends_at_secs: I64,
    pub(super) starts_at_nanos: U32,
    pub(super) ends_at_nanos: U32,
    pub(super) title_off: U32,
    pub(super) title_len: U32,
    pub(super) desc_off: U32,
    pub(super) desc_len: U32,
}

const _: () = {
    assert!(size_of::<SectionRange>() == 16);
    assert!(size_of::<FlatHeader>() == 60 + SECTION_COUNT * 16);
    assert!(size_of::<GroupRecord>() == 24);
    assert!(size_of::<ChannelHot>() == 52);
    assert!(size_of::<Span>() == 8);
    assert!(size_of::<IndexRange>() == 8);
    assert!(size_of::<ScheduleEntry>() == 8);
    assert!(size_of::<MatchedProgrammeHot>() == 32);
    assert!(size_of::<ProgrammeSearchSpan>() == 16);
    assert!(size_of::<GuideChannelRecord>() == 12);
    assert!(size_of::<UnmatchedProgrammeRecord>() == 44);
};

enum CatalogBacking {
    Owned(Vec<u8>),
    Mapped(CatalogCacheMapping),
}

impl CatalogBacking {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes.as_slice(),
            Self::Mapped(mapping) => mapping.as_bytes(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CatalogLayout {
    generation: CatalogGeneration,
    has_epg: bool,
    sections: [(usize, usize); SECTION_COUNT],
}

impl CatalogLayout {
    fn slice<'a>(&self, bytes: &'a [u8], section: Section) -> &'a [u8] {
        let (start, end) = self.sections[section as usize];
        &bytes[start..end]
    }
}

pub(super) struct FlatCatalog {
    backing: CatalogBacking,
    layout: CatalogLayout,
}

impl FlatCatalog {
    pub(super) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: &[ParsedChannel],
        guide: Option<&ParsedGuide>,
        generation: CatalogGeneration,
    ) -> Self {
        let guide_source = guide.map(ParsedGuideSource);
        build_flat_catalog(configuration, parsed, guide_source.as_ref(), generation)
    }

    pub(super) fn from_parsed_with_existing_guide(
        configuration: &SourceConfiguration,
        parsed: &[ParsedChannel],
        existing: &FlatCatalog,
        generation: CatalogGeneration,
    ) -> Self {
        let guide_view = existing.has_epg().then_some(FlatGuideView(existing));
        build_flat_catalog(configuration, parsed, guide_view.as_ref(), generation)
    }

    pub(super) fn with_updated_guide(
        &self,
        guide: &ParsedGuide,
        generation: CatalogGeneration,
    ) -> Self {
        let guide_source = ParsedGuideSource(guide);
        rebuild_with_guide(self, Some(&guide_source), generation)
    }

    pub(super) fn open_with_store(
        store: &dyn SnapshotStore,
        key: &[u8; 32],
        candidates: &[&SnapshotCandidate],
        expected_generation: CatalogGeneration,
    ) -> Option<Self> {
        let mut validated: Option<(CatalogLayout, usize)> = None;
        let mapping = store.open_mapped_catalog_cache(key, candidates, &mut |bytes| {
            validated =
                validate_flat_bytes(bytes, expected_generation).map(|layout| (layout, bytes.len()));
            validated.is_some()
        })?;
        let (layout, expected_len) = validated?;
        if mapping.as_bytes().len() != expected_len {
            return None;
        }
        Some(Self {
            backing: CatalogBacking::Mapped(mapping),
            layout,
        })
    }

    #[cfg(test)]
    pub(super) fn from_mapping(
        mapping: CatalogCacheMapping,
        expected_generation: CatalogGeneration,
    ) -> Option<Self> {
        let layout = validate_flat_bytes(mapping.as_bytes(), expected_generation)?;
        Some(Self {
            backing: CatalogBacking::Mapped(mapping),
            layout,
        })
    }

    pub(super) fn as_bytes(&self) -> &[u8] {
        self.backing.as_bytes()
    }

    pub(super) const fn is_mapped(&self) -> bool {
        matches!(self.backing, CatalogBacking::Mapped(_))
    }

    /// Promotes an `Owned` catalog to a `Mapped` catalog after verifying that
    /// the mapping has the exact expected byte length and identical header.
    pub(super) fn promote_to_mapping(&self, mapping: CatalogCacheMapping) -> Option<Self> {
        let expected = self.as_bytes();
        let mapped = mapping.as_bytes();
        if mapped.len() != expected.len() {
            return None;
        }
        let header_len = size_of::<FlatHeader>();
        if expected.len() < header_len || mapped[..header_len] != expected[..header_len] {
            return None;
        }
        Some(Self {
            backing: CatalogBacking::Mapped(mapping),
            layout: self.layout,
        })
    }

    pub(super) const fn generation(&self) -> CatalogGeneration {
        self.layout.generation
    }

    pub(super) const fn has_epg(&self) -> bool {
        self.layout.has_epg
    }

    fn section_bytes(&self, section: Section) -> &[u8] {
        self.layout.slice(self.backing.as_bytes(), section)
    }

    fn section_str_span(&self, section: Section, off: u32, len: u32) -> &str {
        let bytes = byte_span(self.section_bytes(section), off, len);
        std::str::from_utf8(bytes).expect("validated flat catalog string span is UTF-8")
    }

    fn section_slice<T: FromBytes + Immutable + Unaligned>(&self, section: Section) -> &[T] {
        <[T]>::ref_from_bytes(self.section_bytes(section))
            .expect("validated flat catalog record section matches stride")
    }

    pub(super) fn groups(&self) -> &[GroupRecord] {
        self.section_slice(Section::Groups)
    }

    pub(super) fn group_by_name_sorted(&self) -> &[U32] {
        self.section_slice(Section::GroupByNameSorted)
    }

    pub(super) fn group_name(&self, group_index: usize) -> &str {
        let group = &self.groups()[group_index];
        self.section_str_span(
            Section::GroupStrings,
            group.name_off.get(),
            group.name_len.get(),
        )
    }

    pub(super) fn group_search_name_bytes(&self, group_index: usize) -> &[u8] {
        let group = &self.groups()[group_index];
        byte_span(
            self.section_bytes(Section::GroupSearchStrings),
            group.norm_name_off.get(),
            group.norm_name_len.get(),
        )
    }

    pub(super) fn group_channel_range(&self, group_name: &str) -> Range<usize> {
        let sorted = self.group_by_name_sorted();
        let groups = self.groups();
        let strings = self.section_bytes(Section::GroupStrings);
        match sorted.binary_search_by(|entry| {
            let group = &groups[entry.get() as usize];
            byte_span(strings, group.name_off.get(), group.name_len.get())
                .cmp(group_name.as_bytes())
        }) {
            Ok(position) => {
                let group = &groups[sorted[position].get() as usize];
                group.channel_start.get() as usize..group.channel_end.get() as usize
            }
            Err(_) => 0..0,
        }
    }

    pub(super) fn channels_hot(&self) -> &[ChannelHot] {
        self.section_slice(Section::ChannelsHot)
    }

    pub(super) fn channels_by_id(&self) -> &[U32] {
        self.section_slice(Section::ChannelsById)
    }

    pub(super) fn channel_index_by_digest(&self, digest: &[u8; 32]) -> Option<usize> {
        let channels = self.channels_hot();
        let sorted = self.channels_by_id();
        sorted
            .binary_search_by(|entry| channels[entry.get() as usize].id_digest.cmp(digest))
            .ok()
            .map(|position| sorted[position].get() as usize)
    }

    pub(super) fn channel_name(&self, channel_index: usize) -> &str {
        let channel = &self.channels_hot()[channel_index];
        self.section_str_span(
            Section::ChannelNames,
            channel.name_off.get(),
            channel.name_len.get(),
        )
    }

    pub(super) fn channel_playback(&self, channel_index: usize) -> &str {
        let span = self.section_slice::<Span>(Section::PlaybackSpans)[channel_index];
        self.section_str_span(Section::PlaybackStrings, span.off.get(), span.len.get())
    }

    pub(super) fn channel_tvg_id(&self, channel_index: usize) -> &str {
        let span = self.section_slice::<Span>(Section::TvgIdSpans)[channel_index];
        self.section_str_span(Section::TvgIdStrings, span.off.get(), span.len.get())
    }

    pub(super) fn channel_schedule_range(&self, channel_index: usize) -> Range<usize> {
        let range = self.section_slice::<IndexRange>(Section::ChannelSchedules)[channel_index];
        range.start.get() as usize..range.end.get() as usize
    }

    pub(super) fn schedule_entries(&self) -> &[ScheduleEntry] {
        self.section_slice(Section::ScheduleEntries)
    }

    pub(super) fn schedule_overlap(&self) -> &[U32] {
        self.section_slice(Section::ScheduleOverlap)
    }

    pub(super) fn matched_programmes_hot(&self) -> &[MatchedProgrammeHot] {
        self.section_slice(Section::MatchedProgrammesHot)
    }

    pub(super) fn matched_programme_title(&self, matched_index: usize) -> &str {
        let programme = &self.matched_programmes_hot()[matched_index];
        self.section_str_span(
            Section::MatchedProgrammeTitles,
            programme.title_off.get(),
            programme.title_len.get(),
        )
    }

    /// Returns at most `max_bytes` UTF-8 bytes of a matched Programme's title
    /// together with whether the full title exceeded `max_bytes`, touching only
    /// the bounded prefix bytes in the mapped backing.
    pub(super) fn matched_programme_title_bounded(
        &self,
        matched_index: usize,
        max_bytes: usize,
    ) -> (&str, bool) {
        let programme = &self.matched_programmes_hot()[matched_index];
        let full_len = programme.title_len.get() as usize;
        let truncated = full_len > max_bytes;
        let prefix_len = full_len.min(max_bytes);
        let prefix_bytes = byte_span(
            self.section_bytes(Section::MatchedProgrammeTitles),
            programme.title_off.get(),
            prefix_len as u32,
        );
        let bounded = match std::str::from_utf8(prefix_bytes) {
            Ok(valid) => valid,
            Err(error) => {
                // Only trailing bytes of a split multi-byte UTF-8 scalar can
                // fail here because the full span was UTF-8 validated on open.
                debug_assert!(error.error_len().is_none());
                std::str::from_utf8(&prefix_bytes[..error.valid_up_to()])
                    .expect("validated flat catalog title prefix up to UTF-8 boundary")
            }
        };
        (bounded, truncated)
    }

    pub(super) fn matched_programme_description(&self, matched_index: usize) -> Option<&str> {
        let span = self.section_slice::<Span>(Section::MatchedProgrammeDescSpans)[matched_index];
        if span.off.get() == ABSENT_U32 {
            None
        } else {
            Some(self.section_str_span(
                Section::MatchedProgrammeDescriptions,
                span.off.get(),
                span.len.get(),
            ))
        }
    }

    pub(super) fn matched_programme_starts_at(&self, matched_index: usize) -> DateTime<Utc> {
        let programme = &self.matched_programmes_hot()[matched_index];
        DateTime::from_timestamp(
            programme.starts_at_secs.get(),
            programme.starts_at_nanos.get(),
        )
        .expect("validated flat catalog start timestamp is valid")
    }

    pub(super) fn matched_programme_ends_at(&self, matched_index: usize) -> DateTime<Utc> {
        let programme = &self.matched_programmes_hot()[matched_index];
        DateTime::from_timestamp(programme.ends_at_secs.get(), programme.ends_at_nanos.get())
            .expect("validated flat catalog end timestamp is valid")
    }

    pub(super) fn channel_search_spans(&self) -> &[Span] {
        self.section_slice(Section::ChannelSearchSpans)
    }

    pub(super) fn channel_search_arena(&self) -> &[u8] {
        self.section_bytes(Section::ChannelSearchArena)
    }

    fn channel_search_str(&self, channel_index: usize) -> &str {
        let span = self.channel_search_spans()[channel_index];
        self.section_str_span(Section::ChannelSearchArena, span.off.get(), span.len.get())
    }

    pub(super) fn programme_search_spans(&self) -> &[ProgrammeSearchSpan] {
        self.section_slice(Section::ProgrammeSearchSpans)
    }

    pub(super) fn programme_search_arena(&self) -> &[u8] {
        self.section_bytes(Section::ProgrammeSearchArena)
    }

    pub(super) fn first_possible_overlap(
        &self,
        schedule: &Range<usize>,
        instant: DateTime<Utc>,
    ) -> usize {
        self.first_possible_overlap_with_probe(schedule, instant, || {})
    }

    pub(super) fn first_possible_overlap_with_probe(
        &self,
        schedule: &Range<usize>,
        instant: DateTime<Utc>,
        mut on_probe: impl FnMut(),
    ) -> usize {
        let overlap = self.schedule_overlap();
        let programmes = self.matched_programmes_hot();
        let instant_secs = instant.timestamp();
        let instant_nanos = instant.timestamp_subsec_nanos();
        schedule.start
            + overlap[schedule.clone()].partition_point(|entry| {
                on_probe();
                let programme = &programmes[entry.get() as usize];
                (programme.ends_at_secs.get(), programme.ends_at_nanos.get())
                    <= (instant_secs, instant_nanos)
            })
    }

    fn guide_id(&self, guide_id_index: usize) -> &str {
        let span = self.section_slice::<Span>(Section::GuideIdSpans)[guide_id_index];
        self.section_str_span(Section::GuideIdStrings, span.off.get(), span.len.get())
    }
}

struct ParsedGuideSource<'a>(&'a ParsedGuide);

impl GuideSource for ParsedGuideSource<'_> {
    fn channel_count(&self) -> usize {
        self.0.channels.len()
    }

    fn channel_id(&self, index: usize) -> &str {
        &self.0.channels[index].id
    }

    fn channel_display_name_count(&self, index: usize) -> usize {
        self.0.channels[index].display_names.len()
    }

    fn channel_display_name(&self, channel_index: usize, name_index: usize) -> &str {
        &self.0.channels[channel_index].display_names[name_index]
    }

    fn programme_count(&self) -> usize {
        self.0.programmes.len()
    }

    fn programme_guide_channel_id(&self, index: usize) -> &str {
        &self.0.programmes[index].guide_channel_id
    }

    fn programme_title(&self, index: usize) -> &str {
        &self.0.programmes[index].title
    }

    fn programme_description(&self, index: usize) -> Option<&str> {
        self.0.programmes[index].description.as_deref()
    }

    fn programme_starts_at(&self, index: usize) -> DateTime<Utc> {
        self.0.programmes[index].starts_at
    }

    fn programme_ends_at(&self, index: usize) -> DateTime<Utc> {
        self.0.programmes[index].ends_at
    }
}

#[derive(Clone, Copy)]
enum GuideProgRef {
    Matched(usize),
    Unmatched(usize),
}

struct FlatGuideView<'a>(&'a FlatCatalog);

impl FlatGuideView<'_> {
    fn prog_ref(&self, index: usize) -> GuideProgRef {
        let raw = self.0.section_slice::<U32>(Section::GuideProgOrder)[index].get();
        if raw & UNMATCHED_PROG_BIT == 0 {
            GuideProgRef::Matched(raw as usize)
        } else {
            GuideProgRef::Unmatched((raw & !UNMATCHED_PROG_BIT) as usize)
        }
    }

    fn unmatched(&self, index: usize) -> &UnmatchedProgrammeRecord {
        &self
            .0
            .section_slice::<UnmatchedProgrammeRecord>(Section::UnmatchedProgrammes)[index]
    }
}

impl GuideSource for FlatGuideView<'_> {
    fn channel_count(&self) -> usize {
        self.0
            .section_slice::<GuideChannelRecord>(Section::GuideChannels)
            .len()
    }

    fn channel_id(&self, index: usize) -> &str {
        let record = &self
            .0
            .section_slice::<GuideChannelRecord>(Section::GuideChannels)[index];
        self.0.guide_id(record.guide_id_index.get() as usize)
    }

    fn channel_display_name_count(&self, index: usize) -> usize {
        let record = &self
            .0
            .section_slice::<GuideChannelRecord>(Section::GuideChannels)[index];
        (record.display_end.get() - record.display_start.get()) as usize
    }

    fn channel_display_name(&self, channel_index: usize, name_index: usize) -> &str {
        let record = &self
            .0
            .section_slice::<GuideChannelRecord>(Section::GuideChannels)[channel_index];
        let span = self.0.section_slice::<Span>(Section::GuideDisplaySpans)
            [record.display_start.get() as usize + name_index];
        self.0
            .section_str_span(Section::GuideDisplayStrings, span.off.get(), span.len.get())
    }

    fn programme_count(&self) -> usize {
        self.0.section_slice::<U32>(Section::GuideProgOrder).len()
    }

    fn programme_guide_channel_id(&self, index: usize) -> &str {
        match self.prog_ref(index) {
            GuideProgRef::Matched(idx) => {
                let guide_id_index =
                    self.0.section_slice::<U32>(Section::MatchedProgGuideIds)[idx].get() as usize;
                self.0.guide_id(guide_id_index)
            }
            GuideProgRef::Unmatched(idx) => self
                .0
                .guide_id(self.unmatched(idx).guide_id_index.get() as usize),
        }
    }

    fn programme_title(&self, index: usize) -> &str {
        match self.prog_ref(index) {
            GuideProgRef::Matched(idx) => self.0.matched_programme_title(idx),
            GuideProgRef::Unmatched(idx) => {
                let unmatched = self.unmatched(idx);
                self.0.section_str_span(
                    Section::UnmatchedProgrammeStrings,
                    unmatched.title_off.get(),
                    unmatched.title_len.get(),
                )
            }
        }
    }

    fn programme_description(&self, index: usize) -> Option<&str> {
        match self.prog_ref(index) {
            GuideProgRef::Matched(idx) => self.0.matched_programme_description(idx),
            GuideProgRef::Unmatched(idx) => {
                let unmatched = self.unmatched(idx);
                if unmatched.desc_off.get() == ABSENT_U32 {
                    None
                } else {
                    Some(self.0.section_str_span(
                        Section::UnmatchedProgrammeStrings,
                        unmatched.desc_off.get(),
                        unmatched.desc_len.get(),
                    ))
                }
            }
        }
    }

    fn programme_starts_at(&self, index: usize) -> DateTime<Utc> {
        match self.prog_ref(index) {
            GuideProgRef::Matched(idx) => self.0.matched_programme_starts_at(idx),
            GuideProgRef::Unmatched(idx) => {
                let unmatched = self.unmatched(idx);
                DateTime::from_timestamp(
                    unmatched.starts_at_secs.get(),
                    unmatched.starts_at_nanos.get(),
                )
                .expect("validated unmatched start timestamp is valid")
            }
        }
    }

    fn programme_ends_at(&self, index: usize) -> DateTime<Utc> {
        match self.prog_ref(index) {
            GuideProgRef::Matched(idx) => self.0.matched_programme_ends_at(idx),
            GuideProgRef::Unmatched(idx) => {
                let unmatched = self.unmatched(idx);
                DateTime::from_timestamp(
                    unmatched.ends_at_secs.get(),
                    unmatched.ends_at_nanos.get(),
                )
                .expect("validated unmatched end timestamp is valid")
            }
        }
    }
}

struct PendingChannel {
    source_index: usize,
    group_order: usize,
    name_order: String,
    id_digest: [u8; 32],
}

fn compute_channel_identities(parsed: &[ParsedChannel]) -> Vec<([u8; 32], String)> {
    let worker_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(8);
    if parsed.len() < PARALLEL_CHANNEL_THRESHOLD || worker_count <= 1 {
        return parsed
            .iter()
            .map(|channel| {
                identity::seed_and_normalized_name(&channel.tvg_id, &channel.name, &channel.group)
            })
            .collect();
    }
    let chunk_size = parsed.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        let handles: Vec<_> = parsed
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|channel| {
                            identity::seed_and_normalized_name(
                                &channel.tvg_id,
                                &channel.name,
                                &channel.group,
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut combined = Vec::with_capacity(parsed.len());
        for handle in handles {
            combined.extend(
                handle
                    .join()
                    .expect("channel identity worker did not panic"),
            );
        }
        combined
    })
}

fn compare_pending_channels(left: &PendingChannel, right: &PendingChannel) -> Ordering {
    left.group_order
        .cmp(&right.group_order)
        .then_with(|| left.source_index.cmp(&right.source_index))
}

struct BuiltEpgSections {
    channel_schedules: Vec<IndexRange>,
    schedule_entries: Vec<ScheduleEntry>,
    schedule_overlap: Vec<U32>,
    matched_programmes_hot: Vec<MatchedProgrammeHot>,
    matched_programme_titles: Vec<u8>,
    matched_programme_desc_spans: Vec<Span>,
    matched_programme_descriptions: Vec<u8>,
    programme_search_spans: Vec<ProgrammeSearchSpan>,
    programme_search_arena: Vec<u8>,
    guide_id_spans: Vec<Span>,
    guide_id_strings: Vec<u8>,
    guide_channels: Vec<GuideChannelRecord>,
    guide_display_spans: Vec<Span>,
    guide_display_strings: Vec<u8>,
    matched_prog_guide_ids: Vec<U32>,
    unmatched_programmes: Vec<UnmatchedProgrammeRecord>,
    unmatched_programme_strings: Vec<u8>,
    guide_prog_order: Vec<U32>,
}

fn build_epg_sections<G: GuideSource>(
    channels: &[ScheduleBuildChannel<'_>],
    channels_by_id: &[u32],
    guide: Option<&G>,
) -> BuiltEpgSections {
    let built = build_programmes(channels, channels_by_id, guide);
    let Some(guide) = guide else {
        return BuiltEpgSections {
            channel_schedules: built
                .channel_schedules
                .into_iter()
                .map(|(start, end)| IndexRange {
                    start: U32::new(start),
                    end: U32::new(end),
                })
                .collect(),
            schedule_entries: Vec::new(),
            schedule_overlap: Vec::new(),
            matched_programmes_hot: Vec::new(),
            matched_programme_titles: Vec::new(),
            matched_programme_desc_spans: Vec::new(),
            matched_programme_descriptions: Vec::new(),
            programme_search_spans: Vec::new(),
            programme_search_arena: Vec::new(),
            guide_id_spans: Vec::new(),
            guide_id_strings: Vec::new(),
            guide_channels: Vec::new(),
            guide_display_spans: Vec::new(),
            guide_display_strings: Vec::new(),
            matched_prog_guide_ids: Vec::new(),
            unmatched_programmes: Vec::new(),
            unmatched_programme_strings: Vec::new(),
            guide_prog_order: Vec::new(),
        };
    };

    // Intern XMLTV channel IDs in deterministic order: guide.channels then guide.programmes.
    let mut guide_id_map: HashMap<&str, u32> = HashMap::with_capacity(guide.channel_count());
    let mut guide_id_spans: Vec<Span> = Vec::with_capacity(guide.channel_count());
    let mut guide_id_strings: Vec<u8> = Vec::new();

    let mut guide_channels = Vec::with_capacity(guide.channel_count());
    let mut guide_display_spans = Vec::new();
    let mut guide_display_strings = Vec::new();
    for ch_idx in 0..guide.channel_count() {
        let guide_id_index = intern_guide_id(
            &mut guide_id_map,
            &mut guide_id_spans,
            &mut guide_id_strings,
            guide.channel_id(ch_idx),
        );
        let display_start =
            u32::try_from(guide_display_spans.len()).expect("guide display names fit in u32");
        for name_idx in 0..guide.channel_display_name_count(ch_idx) {
            let span = push_str_blob(
                &mut guide_display_strings,
                guide.channel_display_name(ch_idx, name_idx),
            );
            guide_display_spans.push(span);
        }
        let display_end =
            u32::try_from(guide_display_spans.len()).expect("guide display names fit in u32");
        guide_channels.push(GuideChannelRecord {
            guide_id_index: U32::new(guide_id_index),
            display_start: U32::new(display_start),
            display_end: U32::new(display_end),
        });
    }

    for prog_idx in 0..guide.programme_count() {
        let _ = intern_guide_id(
            &mut guide_id_map,
            &mut guide_id_spans,
            &mut guide_id_strings,
            guide.programme_guide_channel_id(prog_idx),
        );
    }

    let matched_count = built.matched_source_indices.len();
    let mut matched_programmes_hot = Vec::with_capacity(matched_count);
    let mut matched_programme_titles = Vec::new();
    let mut matched_programme_desc_spans = Vec::with_capacity(matched_count);
    let mut matched_programme_descriptions = Vec::new();
    let mut matched_prog_guide_ids = Vec::with_capacity(matched_count);
    let mut programme_search_spans = Vec::with_capacity(matched_count);
    let mut programme_search_arena = Vec::new();

    for &source_idx in &built.matched_source_indices {
        let source_idx = source_idx as usize;
        let starts_at = guide.programme_starts_at(source_idx);
        let ends_at = guide.programme_ends_at(source_idx);
        let title = guide.programme_title(source_idx);
        let description = guide.programme_description(source_idx);
        let title_span = push_str_blob(&mut matched_programme_titles, title);
        let desc_span = match description {
            Some(desc) => push_str_blob(&mut matched_programme_descriptions, desc),
            None => Span {
                off: U32::new(ABSENT_U32),
                len: U32::new(ABSENT_U32),
            },
        };
        let norm_title = normalize_search_text(title);
        let primary = push_str_blob(&mut programme_search_arena, &norm_title);
        let (secondary_off, secondary_len) = match description {
            Some(desc) => {
                let norm_desc = normalize_search_text(desc);
                let span = push_str_blob(&mut programme_search_arena, &norm_desc);
                (span.off, span.len)
            }
            None => (U32::new(ABSENT_U32), U32::new(ABSENT_U32)),
        };

        matched_programmes_hot.push(MatchedProgrammeHot {
            starts_at_secs: I64::new(starts_at.timestamp()),
            ends_at_secs: I64::new(ends_at.timestamp()),
            starts_at_nanos: U32::new(starts_at.timestamp_subsec_nanos()),
            ends_at_nanos: U32::new(ends_at.timestamp_subsec_nanos()),
            title_off: title_span.off,
            title_len: title_span.len,
        });
        matched_programme_desc_spans.push(desc_span);
        matched_prog_guide_ids.push(U32::new(
            guide_id_map[guide.programme_guide_channel_id(source_idx)],
        ));
        programme_search_spans.push(ProgrammeSearchSpan {
            primary_off: primary.off,
            primary_len: primary.len,
            secondary_off,
            secondary_len,
        });
    }

    let unmatched_count = built.unmatched_source_indices.len();
    let mut unmatched_programmes = Vec::with_capacity(unmatched_count);
    let mut unmatched_programme_strings = Vec::new();
    for &source_idx in &built.unmatched_source_indices {
        let source_idx = source_idx as usize;
        let starts_at = guide.programme_starts_at(source_idx);
        let ends_at = guide.programme_ends_at(source_idx);
        let title_span = push_str_blob(
            &mut unmatched_programme_strings,
            guide.programme_title(source_idx),
        );
        let desc_span = match guide.programme_description(source_idx) {
            Some(desc) => push_str_blob(&mut unmatched_programme_strings, desc),
            None => Span {
                off: U32::new(ABSENT_U32),
                len: U32::new(ABSENT_U32),
            },
        };
        unmatched_programmes.push(UnmatchedProgrammeRecord {
            guide_id_index: U32::new(guide_id_map[guide.programme_guide_channel_id(source_idx)]),
            starts_at_secs: I64::new(starts_at.timestamp()),
            ends_at_secs: I64::new(ends_at.timestamp()),
            starts_at_nanos: U32::new(starts_at.timestamp_subsec_nanos()),
            ends_at_nanos: U32::new(ends_at.timestamp_subsec_nanos()),
            title_off: title_span.off,
            title_len: title_span.len,
            desc_off: desc_span.off,
            desc_len: desc_span.len,
        });
    }

    BuiltEpgSections {
        channel_schedules: built
            .channel_schedules
            .into_iter()
            .map(|(start, end)| IndexRange {
                start: U32::new(start),
                end: U32::new(end),
            })
            .collect(),
        schedule_entries: built
            .schedule_entries
            .into_iter()
            .map(|entry| ScheduleEntry {
                channel_index: U32::new(entry.channel_index),
                matched_prog_index: U32::new(entry.matched_prog_index),
            })
            .collect(),
        schedule_overlap: built.schedule_overlap.into_iter().map(U32::new).collect(),
        matched_programmes_hot,
        matched_programme_titles,
        matched_programme_desc_spans,
        matched_programme_descriptions,
        programme_search_spans,
        programme_search_arena,
        guide_id_spans,
        guide_id_strings,
        guide_channels,
        guide_display_spans,
        guide_display_strings,
        matched_prog_guide_ids,
        unmatched_programmes,
        unmatched_programme_strings,
        guide_prog_order: built.guide_prog_order.into_iter().map(U32::new).collect(),
    }
}

fn build_flat_catalog<G: GuideSource + Sync>(
    configuration: &SourceConfiguration,
    parsed: &[ParsedChannel],
    guide: Option<&G>,
    generation: CatalogGeneration,
) -> FlatCatalog {
    let seeds_and_names = compute_channel_identities(parsed);
    let mut occurrences = HashMap::<[u8; 32], u32>::with_capacity(parsed.len());
    let mut group_first_seen = HashMap::<&str, usize>::new();
    let mut pending = Vec::with_capacity(parsed.len());

    for ((source_index, channel), (seed, name_order)) in
        parsed.iter().enumerate().zip(seeds_and_names)
    {
        let occurrence = occurrences.entry(seed).or_default();
        let id_digest = identity::channel_id_digest(&configuration.fingerprint, &seed, *occurrence);
        *occurrence = occurrence.saturating_add(1);
        let group_order = *group_first_seen
            .entry(channel.group.as_ref())
            .or_insert(source_index);
        pending.push(PendingChannel {
            source_index,
            group_order,
            name_order,
            id_digest,
        });
    }
    drop(occurrences);
    drop(group_first_seen);

    pending.sort_unstable_by(compare_pending_channels);

    let channel_count = pending.len();
    let mut channels_by_id: Vec<u32> = (0..channel_count as u32).collect();
    channels_by_id.sort_unstable_by(|&left, &right| {
        pending[left as usize]
            .id_digest
            .cmp(&pending[right as usize].id_digest)
    });

    // Groups in first-seen order (contiguous channel ranges).
    let mut groups = Vec::new();
    let mut group_strings = Vec::new();
    let mut group_search_strings = Vec::new();
    let mut channel_group_indices = vec![0_u32; channel_count];
    let mut group_start = 0;
    while group_start < channel_count {
        let group_name = parsed[pending[group_start].source_index].group.as_ref();
        let mut group_end = group_start + 1;
        while group_end < channel_count
            && parsed[pending[group_end].source_index].group.as_ref() == group_name
        {
            group_end += 1;
        }
        let group_index = u32::try_from(groups.len()).expect("group count fits in u32");
        for slot in &mut channel_group_indices[group_start..group_end] {
            *slot = group_index;
        }
        let name_span = push_str_blob(&mut group_strings, group_name);
        let norm_group = normalize_search_text(group_name);
        let norm_span = push_str_blob(&mut group_search_strings, &norm_group);
        groups.push(GroupRecord {
            name_off: name_span.off,
            name_len: name_span.len,
            norm_name_off: norm_span.off,
            norm_name_len: norm_span.len,
            channel_start: U32::new(group_start as u32),
            channel_end: U32::new(group_end as u32),
        });
        group_start = group_end;
    }

    let mut group_by_name_sorted: Vec<u32> = (0..groups.len() as u32).collect();
    let group_strings_str =
        std::str::from_utf8(&group_strings).expect("pushed group names are valid UTF-8");
    group_by_name_sorted.sort_unstable_by(|&left, &right| {
        let left_g = &groups[left as usize];
        let right_g = &groups[right as usize];
        str_span(
            group_strings_str,
            left_g.name_off.get(),
            left_g.name_len.get(),
        )
        .cmp(str_span(
            group_strings_str,
            right_g.name_off.get(),
            right_g.name_len.get(),
        ))
    });

    let build_placements = || {
        variants::placements(pending.iter().map(|channel| {
            let source = &parsed[channel.source_index];
            (&*source.name, &*source.group)
        }))
    };

    let build_epg = || {
        let schedule_channels: Vec<ScheduleBuildChannel<'_>> = pending
            .iter()
            .map(|ch| ScheduleBuildChannel {
                tvg_id: parsed[ch.source_index].tvg_id.as_ref(),
                name_order: ch.name_order.as_str(),
            })
            .collect();
        build_epg_sections(&schedule_channels, &channels_by_id, guide)
    };

    let (placements, epg_sections) = if channel_count >= PARALLEL_BUILD_THRESHOLD && guide.is_some()
    {
        std::thread::scope(|scope| {
            let placements_handle = scope.spawn(build_placements);
            let epg = build_epg();
            let placements = placements_handle
                .join()
                .expect("variant placement thread did not panic");
            (placements, epg)
        })
    } else {
        (build_placements(), build_epg())
    };

    // Reuse the normalized names already computed in `compute_channel_identities`.
    let mut channel_search_spans = Vec::with_capacity(channel_count);
    let mut channel_search_arena =
        Vec::with_capacity(pending.iter().map(|ch| ch.name_order.len()).sum::<usize>());
    for ch in &pending {
        channel_search_spans.push(push_str_blob(&mut channel_search_arena, &ch.name_order));
    }

    let mut channels_hot = Vec::with_capacity(channel_count);
    let mut channel_names = Vec::with_capacity(channel_count.saturating_mul(24));
    let mut playback_spans = Vec::with_capacity(channel_count);
    let mut playback_strings = Vec::with_capacity(channel_count.saturating_mul(64));
    let mut tvg_id_spans = Vec::with_capacity(channel_count);
    let mut tvg_id_strings = Vec::new();

    for ((idx, ch), placement) in pending.iter().enumerate().zip(placements) {
        let source = &parsed[ch.source_index];
        let name_span = push_str_blob(&mut channel_names, &source.name);
        let playback_span = push_str_blob(&mut playback_strings, &source.playback);
        let tvg_span = if source.tvg_id.is_empty() {
            Span {
                off: U32::new(0),
                len: U32::new(0),
            }
        } else {
            push_str_blob(&mut tvg_id_strings, &source.tvg_id)
        };
        channels_hot.push(ChannelHot {
            id_digest: ch.id_digest,
            name_off: name_span.off,
            name_len: name_span.len,
            group_index: U32::new(channel_group_indices[idx]),
            number: U32::new(placement.number),
            quality: encode_quality(placement.quality),
            pad: [0; 3],
        });
        playback_spans.push(playback_span);
        tvg_id_spans.push(tvg_span);
    }
    drop(pending);
    drop(channel_group_indices);

    let group_by_name_sorted: Vec<U32> = group_by_name_sorted.into_iter().map(U32::new).collect();
    let channels_by_id: Vec<U32> = channels_by_id.into_iter().map(U32::new).collect();

    assemble_flat_catalog(
        generation,
        guide.is_some(),
        FlatSectionsRef {
            groups: groups.as_bytes(),
            group_by_name_sorted: group_by_name_sorted.as_bytes(),
            group_strings: &group_strings,
            group_search_strings: &group_search_strings,
            channels_hot: channels_hot.as_bytes(),
            channel_names: &channel_names,
            channels_by_id: channels_by_id.as_bytes(),
            playback_spans: playback_spans.as_bytes(),
            playback_strings: &playback_strings,
            tvg_id_spans: tvg_id_spans.as_bytes(),
            tvg_id_strings: &tvg_id_strings,
            channel_search_spans: channel_search_spans.as_bytes(),
            channel_search_arena: &channel_search_arena,
            group_count: groups.len(),
            channel_count,
        },
        &epg_sections,
    )
}

fn rebuild_with_guide<G: GuideSource>(
    existing: &FlatCatalog,
    guide: Option<&G>,
    generation: CatalogGeneration,
) -> FlatCatalog {
    let channel_count = existing.channels_hot().len();
    // Normalized channel names already live in `ChannelSearchArena`, so EPG
    // refreshes borrow them zero-copy instead of allocating N heap Strings.
    let schedule_channels: Vec<ScheduleBuildChannel<'_>> = (0..channel_count)
        .map(|idx| ScheduleBuildChannel {
            tvg_id: existing.channel_tvg_id(idx),
            name_order: existing.channel_search_str(idx),
        })
        .collect();
    let channels_by_id: Vec<u32> = existing
        .channels_by_id()
        .iter()
        .map(|entry| entry.get())
        .collect();
    let epg_sections = build_epg_sections(&schedule_channels, &channels_by_id, guide);
    drop(schedule_channels);
    drop(channels_by_id);

    assemble_flat_catalog(
        generation,
        guide.is_some(),
        FlatSectionsRef {
            groups: existing.section_bytes(Section::Groups),
            group_by_name_sorted: existing.section_bytes(Section::GroupByNameSorted),
            group_strings: existing.section_bytes(Section::GroupStrings),
            group_search_strings: existing.section_bytes(Section::GroupSearchStrings),
            channels_hot: existing.section_bytes(Section::ChannelsHot),
            channel_names: existing.section_bytes(Section::ChannelNames),
            channels_by_id: existing.section_bytes(Section::ChannelsById),
            playback_spans: existing.section_bytes(Section::PlaybackSpans),
            playback_strings: existing.section_bytes(Section::PlaybackStrings),
            tvg_id_spans: existing.section_bytes(Section::TvgIdSpans),
            tvg_id_strings: existing.section_bytes(Section::TvgIdStrings),
            channel_search_spans: existing.section_bytes(Section::ChannelSearchSpans),
            channel_search_arena: existing.section_bytes(Section::ChannelSearchArena),
            group_count: existing.groups().len(),
            channel_count,
        },
        &epg_sections,
    )
}

struct FlatSectionsRef<'a> {
    groups: &'a [u8],
    group_by_name_sorted: &'a [u8],
    group_strings: &'a [u8],
    group_search_strings: &'a [u8],
    channels_hot: &'a [u8],
    channel_names: &'a [u8],
    channels_by_id: &'a [u8],
    playback_spans: &'a [u8],
    playback_strings: &'a [u8],
    tvg_id_spans: &'a [u8],
    tvg_id_strings: &'a [u8],
    channel_search_spans: &'a [u8],
    channel_search_arena: &'a [u8],
    group_count: usize,
    channel_count: usize,
}

fn assemble_flat_catalog(
    generation: CatalogGeneration,
    has_epg: bool,
    m3u: FlatSectionsRef<'_>,
    epg: &BuiltEpgSections,
) -> FlatCatalog {
    let section_slices: [&[u8]; SECTION_COUNT] = [
        m3u.groups,
        m3u.group_by_name_sorted,
        m3u.group_strings,
        m3u.group_search_strings,
        m3u.channels_hot,
        m3u.channel_names,
        m3u.channels_by_id,
        epg.channel_schedules.as_bytes(),
        epg.schedule_entries.as_bytes(),
        epg.schedule_overlap.as_bytes(),
        epg.matched_programmes_hot.as_bytes(),
        &epg.matched_programme_titles,
        m3u.playback_spans,
        m3u.playback_strings,
        m3u.tvg_id_spans,
        m3u.tvg_id_strings,
        epg.matched_programme_desc_spans.as_bytes(),
        &epg.matched_programme_descriptions,
        m3u.channel_search_spans,
        m3u.channel_search_arena,
        epg.programme_search_spans.as_bytes(),
        &epg.programme_search_arena,
        epg.guide_id_spans.as_bytes(),
        &epg.guide_id_strings,
        epg.guide_channels.as_bytes(),
        epg.guide_display_spans.as_bytes(),
        &epg.guide_display_strings,
        epg.matched_prog_guide_ids.as_bytes(),
        epg.unmatched_programmes.as_bytes(),
        &epg.unmatched_programme_strings,
        epg.guide_prog_order.as_bytes(),
    ];

    let total_bytes: usize = size_of::<FlatHeader>()
        + section_slices
            .iter()
            .map(|slice| slice.len())
            .sum::<usize>();
    let mut buffer = Vec::with_capacity(total_bytes);
    let mut section_ranges = [SectionRange {
        offset: U64::new(0),
        byte_len: U64::new(0),
    }; SECTION_COUNT];
    let mut layout_sections = [(0_usize, 0_usize); SECTION_COUNT];

    let mut cursor = size_of::<FlatHeader>();
    for (index, slice) in section_slices.iter().enumerate() {
        let end = cursor + slice.len();
        section_ranges[index] = SectionRange {
            offset: U64::new(cursor as u64),
            byte_len: U64::new(slice.len() as u64),
        };
        layout_sections[index] = (cursor, end);
        cursor = end;
    }

    let header = FlatHeader {
        magic: FLAT_MAGIC,
        flags: U32::new(if has_epg { FLAG_HAS_EPG } else { 0 }),
        generation: U64::new(generation.get()),
        group_count: U32::new(u32::try_from(m3u.group_count).expect("groups fit in u32")),
        channel_count: U32::new(u32::try_from(m3u.channel_count).expect("channels fit in u32")),
        matched_prog_count: U32::new(
            u32::try_from(epg.matched_programmes_hot.len()).expect("matched programmes fit in u32"),
        ),
        schedule_entry_count: U32::new(
            u32::try_from(epg.schedule_entries.len()).expect("schedule entries fit in u32"),
        ),
        guide_id_count: U32::new(
            u32::try_from(epg.guide_id_spans.len()).expect("guide IDs fit in u32"),
        ),
        guide_channel_count: U32::new(
            u32::try_from(epg.guide_channels.len()).expect("guide channels fit in u32"),
        ),
        guide_display_count: U32::new(
            u32::try_from(epg.guide_display_spans.len()).expect("guide display names fit in u32"),
        ),
        unmatched_prog_count: U32::new(
            u32::try_from(epg.unmatched_programmes.len()).expect("unmatched programmes fit in u32"),
        ),
        total_guide_prog_count: U32::new(
            u32::try_from(epg.guide_prog_order.len()).expect("total guide programmes fit in u32"),
        ),
        sections: section_ranges,
    };

    buffer.extend_from_slice(header.as_bytes());
    for slice in section_slices {
        buffer.extend_from_slice(slice);
    }
    debug_assert_eq!(buffer.len(), total_bytes);

    FlatCatalog {
        backing: CatalogBacking::Owned(buffer),
        layout: CatalogLayout {
            generation,
            has_epg,
            sections: layout_sections,
        },
    }
}

fn intern_guide_id<'a>(
    guide_id_map: &mut HashMap<&'a str, u32>,
    guide_id_spans: &mut Vec<Span>,
    guide_id_strings: &mut Vec<u8>,
    id: &'a str,
) -> u32 {
    if let Some(&idx) = guide_id_map.get(id) {
        return idx;
    }
    let idx = u32::try_from(guide_id_spans.len()).expect("guide channel IDs fit in u32");
    let span = push_str_blob(guide_id_strings, id);
    guide_id_spans.push(span);
    guide_id_map.insert(id, idx);
    idx
}

fn push_str_blob(blob: &mut Vec<u8>, value: &str) -> Span {
    let off = u32::try_from(blob.len()).expect("flat catalog string blob offset fits in u32");
    let len = u32::try_from(value.len()).expect("flat catalog string length fits in u32");
    blob.extend_from_slice(value.as_bytes());
    Span {
        off: U32::new(off),
        len: U32::new(len),
    }
}

fn encode_quality(value: Option<ChannelQuality>) -> u8 {
    match value {
        None => 0,
        Some(ChannelQuality::Sd) => 1,
        Some(ChannelQuality::Hd) => 2,
        Some(ChannelQuality::Fhd) => 3,
        Some(ChannelQuality::Uhd) => 4,
    }
}

pub(super) fn decode_quality(value: u8) -> Option<Option<ChannelQuality>> {
    match value {
        0 => Some(None),
        1 => Some(Some(ChannelQuality::Sd)),
        2 => Some(Some(ChannelQuality::Hd)),
        3 => Some(Some(ChannelQuality::Fhd)),
        4 => Some(Some(ChannelQuality::Uhd)),
        _ => None,
    }
}

fn str_span(blob: &str, off: u32, len: u32) -> &str {
    let start = off as usize;
    let end = start + len as usize;
    &blob[start..end]
}

fn byte_span(blob: &[u8], off: u32, len: u32) -> &[u8] {
    let start = off as usize;
    let end = start + len as usize;
    &blob[start..end]
}

fn checked_str_span(blob: &str, off: u32, len: u32) -> Option<&str> {
    let start = usize::try_from(off).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    blob.get(start..end)
}

/// Validates that `(off, len)` begins at `*cursor` within `blob` on valid UTF-8
/// boundaries and advances `*cursor` to `end`, preventing overlapping span
/// aliasing across records.
fn checked_contiguous_span<'a>(
    blob: &'a str,
    cursor: &mut usize,
    off: u32,
    len: u32,
) -> Option<&'a str> {
    if usize::try_from(off).ok()? != *cursor {
        return None;
    }
    let slice = checked_str_span(blob, off, len)?;
    *cursor += slice.len();
    Some(slice)
}

fn typed_section<'a, T: FromBytes + Immutable + Unaligned>(
    bytes: &'a [u8],
    layout: &CatalogLayout,
    section: Section,
    count: usize,
) -> Option<&'a [T]> {
    let expected_bytes = count.checked_mul(size_of::<T>())?;
    let raw = layout.slice(bytes, section);
    if raw.len() != expected_bytes {
        return None;
    }
    <[T]>::ref_from_bytes(raw).ok()
}

fn valid_browser_timestamp(secs: i64, nanos: u32) -> Option<DateTime<Utc>> {
    if nanos >= 1_000_000_000 {
        return None;
    }
    let dt = DateTime::from_timestamp(secs, nanos)?;
    (0..=9999).contains(&dt.year()).then_some(dt)
}

fn is_valid_playback_url(raw: &str) -> bool {
    Url::parse(raw)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

/// Performs complete O(N + M + P + file_bytes) structural validation of an
/// untrusted or reopened flat catalog buffer before any query runs against it.
///
/// Validation retains zero heap allocations; `Url::parse` holds one parsed URL
/// at a time across the non-overlapping `PlaybackStrings` partition (peak
/// transient scratch scales with the longest Playback Source), so cached
/// Playback Sources match `resolve_playback` and `m3u::parse`.
fn validate_flat_bytes(
    bytes: &[u8],
    expected_generation: CatalogGeneration,
) -> Option<CatalogLayout> {
    let (header, _) = FlatHeader::ref_from_prefix(bytes).ok()?;
    if header.magic != FLAT_MAGIC {
        return None;
    }
    let flags = header.flags.get();
    if flags & !FLAG_HAS_EPG != 0 {
        return None;
    }
    let has_epg = (flags & FLAG_HAS_EPG) != 0;
    let generation = CatalogGeneration::from_cursor(header.generation.get())?;
    if generation != expected_generation {
        return None;
    }

    let group_count = header.group_count.get() as usize;
    let channel_count = header.channel_count.get() as usize;
    let matched_prog_count = header.matched_prog_count.get() as usize;
    let schedule_entry_count = header.schedule_entry_count.get() as usize;
    let guide_id_count = header.guide_id_count.get() as usize;
    let guide_channel_count = header.guide_channel_count.get() as usize;
    let guide_display_count = header.guide_display_count.get() as usize;
    let unmatched_prog_count = header.unmatched_prog_count.get() as usize;
    let total_guide_prog_count = header.total_guide_prog_count.get() as usize;

    if header.matched_prog_count.get() >= UNMATCHED_PROG_BIT
        || header.unmatched_prog_count.get() >= UNMATCHED_PROG_BIT
    {
        return None;
    }
    if matched_prog_count.checked_add(unmatched_prog_count)? != total_guide_prog_count {
        return None;
    }
    if !has_epg
        && (matched_prog_count != 0
            || schedule_entry_count != 0
            || guide_id_count != 0
            || guide_channel_count != 0
            || guide_display_count != 0
            || unmatched_prog_count != 0
            || total_guide_prog_count != 0)
    {
        return None;
    }

    // Enforce exact contiguous non-overlapping section partitioning from header
    // end to EOF so no two sections can alias or exceed total file bytes.
    let mut section_cursor = size_of::<FlatHeader>() as u64;
    let total_len = bytes.len() as u64;
    let mut layout_sections = [(0_usize, 0_usize); SECTION_COUNT];
    for (index, range) in header.sections.iter().enumerate() {
        let offset = range.offset.get();
        let byte_len = range.byte_len.get();
        let end = offset.checked_add(byte_len)?;
        if offset != section_cursor || end > total_len {
            return None;
        }
        layout_sections[index] = (usize::try_from(offset).ok()?, usize::try_from(end).ok()?);
        section_cursor = end;
    }
    if section_cursor != total_len {
        return None;
    }

    let layout = CatalogLayout {
        generation,
        has_epg,
        sections: layout_sections,
    };

    if !has_epg {
        for section in [
            Section::MatchedProgrammeTitles,
            Section::MatchedProgrammeDescriptions,
            Section::ProgrammeSearchArena,
            Section::GuideIdStrings,
            Section::GuideDisplayStrings,
            Section::UnmatchedProgrammeStrings,
        ] {
            if !layout.slice(bytes, section).is_empty() {
                return None;
            }
        }
    }

    // Validate UTF-8 across every string/search blob.
    let group_strings = std::str::from_utf8(layout.slice(bytes, Section::GroupStrings)).ok()?;
    let group_search_strings =
        std::str::from_utf8(layout.slice(bytes, Section::GroupSearchStrings)).ok()?;
    let channel_names = std::str::from_utf8(layout.slice(bytes, Section::ChannelNames)).ok()?;
    let matched_programme_titles =
        std::str::from_utf8(layout.slice(bytes, Section::MatchedProgrammeTitles)).ok()?;
    let playback_strings =
        std::str::from_utf8(layout.slice(bytes, Section::PlaybackStrings)).ok()?;
    let tvg_id_strings = std::str::from_utf8(layout.slice(bytes, Section::TvgIdStrings)).ok()?;
    let matched_programme_descriptions =
        std::str::from_utf8(layout.slice(bytes, Section::MatchedProgrammeDescriptions)).ok()?;
    let channel_search_arena =
        std::str::from_utf8(layout.slice(bytes, Section::ChannelSearchArena)).ok()?;
    let programme_search_arena =
        std::str::from_utf8(layout.slice(bytes, Section::ProgrammeSearchArena)).ok()?;
    let guide_id_strings =
        std::str::from_utf8(layout.slice(bytes, Section::GuideIdStrings)).ok()?;
    let guide_display_strings =
        std::str::from_utf8(layout.slice(bytes, Section::GuideDisplayStrings)).ok()?;
    let unmatched_programme_strings =
        std::str::from_utf8(layout.slice(bytes, Section::UnmatchedProgrammeStrings)).ok()?;

    // Enforce exact checked `count * size_of::<T>()` lengths and cast every record table once.
    let groups = typed_section::<GroupRecord>(bytes, &layout, Section::Groups, group_count)?;
    let group_by_name_sorted =
        typed_section::<U32>(bytes, &layout, Section::GroupByNameSorted, group_count)?;
    let channels_hot =
        typed_section::<ChannelHot>(bytes, &layout, Section::ChannelsHot, channel_count)?;
    let channels_by_id =
        typed_section::<U32>(bytes, &layout, Section::ChannelsById, channel_count)?;
    let channel_schedules =
        typed_section::<IndexRange>(bytes, &layout, Section::ChannelSchedules, channel_count)?;
    let schedule_entries = typed_section::<ScheduleEntry>(
        bytes,
        &layout,
        Section::ScheduleEntries,
        schedule_entry_count,
    )?;
    let schedule_overlap = typed_section::<U32>(
        bytes,
        &layout,
        Section::ScheduleOverlap,
        schedule_entry_count,
    )?;
    let matched_programmes_hot = typed_section::<MatchedProgrammeHot>(
        bytes,
        &layout,
        Section::MatchedProgrammesHot,
        matched_prog_count,
    )?;
    let playback_spans =
        typed_section::<Span>(bytes, &layout, Section::PlaybackSpans, channel_count)?;
    let tvg_id_spans = typed_section::<Span>(bytes, &layout, Section::TvgIdSpans, channel_count)?;
    let matched_programme_desc_spans = typed_section::<Span>(
        bytes,
        &layout,
        Section::MatchedProgrammeDescSpans,
        matched_prog_count,
    )?;
    let channel_search_spans =
        typed_section::<Span>(bytes, &layout, Section::ChannelSearchSpans, channel_count)?;
    let programme_search_spans = typed_section::<ProgrammeSearchSpan>(
        bytes,
        &layout,
        Section::ProgrammeSearchSpans,
        matched_prog_count,
    )?;
    let guide_id_spans =
        typed_section::<Span>(bytes, &layout, Section::GuideIdSpans, guide_id_count)?;
    let guide_channels = typed_section::<GuideChannelRecord>(
        bytes,
        &layout,
        Section::GuideChannels,
        guide_channel_count,
    )?;
    let guide_display_spans = typed_section::<Span>(
        bytes,
        &layout,
        Section::GuideDisplaySpans,
        guide_display_count,
    )?;
    let matched_prog_guide_ids = typed_section::<U32>(
        bytes,
        &layout,
        Section::MatchedProgGuideIds,
        matched_prog_count,
    )?;
    let unmatched_programmes = typed_section::<UnmatchedProgrammeRecord>(
        bytes,
        &layout,
        Section::UnmatchedProgrammes,
        unmatched_prog_count,
    )?;
    let guide_prog_order = typed_section::<U32>(
        bytes,
        &layout,
        Section::GuideProgOrder,
        total_guide_prog_count,
    )?;

    // Validate groups contiguous partition + non-overlapping string spans.
    let mut channel_cursor = 0_usize;
    let mut group_str_cursor = 0_usize;
    let mut group_search_cursor = 0_usize;
    for group in groups {
        checked_contiguous_span(
            group_strings,
            &mut group_str_cursor,
            group.name_off.get(),
            group.name_len.get(),
        )?;
        checked_contiguous_span(
            group_search_strings,
            &mut group_search_cursor,
            group.norm_name_off.get(),
            group.norm_name_len.get(),
        )?;
        let start = group.channel_start.get() as usize;
        let end = group.channel_end.get() as usize;
        if start != channel_cursor || start > end || end > channel_count {
            return None;
        }
        channel_cursor = end;
    }
    if channel_cursor != channel_count
        || group_str_cursor != group_strings.len()
        || group_search_cursor != group_search_strings.len()
    {
        return None;
    }

    // Validate group_by_name_sorted strict ordering (proves uniqueness + bijection).
    let mut prev_group_name: Option<&str> = None;
    for entry in group_by_name_sorted {
        let idx = entry.get() as usize;
        let group = groups.get(idx)?;
        let name = checked_str_span(group_strings, group.name_off.get(), group.name_len.get())?;
        if let Some(prev) = prev_group_name
            && prev >= name
        {
            return None;
        }
        prev_group_name = Some(name);
    }

    // Validate channels and parallel channel tables with contiguous non-overlapping spans.
    let mut channel_name_cursor = 0_usize;
    let mut playback_cursor = 0_usize;
    let mut tvg_id_cursor = 0_usize;
    let mut channel_search_cursor = 0_usize;
    for index in 0..channel_count {
        let ch = &channels_hot[index];
        if ch.pad != [0; 3] || decode_quality(ch.quality).is_none() {
            return None;
        }
        let group = groups.get(ch.group_index.get() as usize)?;
        if index < group.channel_start.get() as usize || index >= group.channel_end.get() as usize {
            return None;
        }
        checked_contiguous_span(
            channel_names,
            &mut channel_name_cursor,
            ch.name_off.get(),
            ch.name_len.get(),
        )?;

        let playback_span = playback_spans[index];
        let playback = checked_contiguous_span(
            playback_strings,
            &mut playback_cursor,
            playback_span.off.get(),
            playback_span.len.get(),
        )?;
        if !is_valid_playback_url(playback) {
            return None;
        }

        let tvg_span = tvg_id_spans[index];
        if tvg_span.len.get() == 0 {
            if tvg_span.off.get() != 0 {
                return None;
            }
        } else {
            checked_contiguous_span(
                tvg_id_strings,
                &mut tvg_id_cursor,
                tvg_span.off.get(),
                tvg_span.len.get(),
            )?;
        }

        let search_span = channel_search_spans[index];
        checked_contiguous_span(
            channel_search_arena,
            &mut channel_search_cursor,
            search_span.off.get(),
            search_span.len.get(),
        )?;
    }
    if channel_name_cursor != channel_names.len()
        || playback_cursor != playback_strings.len()
        || tvg_id_cursor != tvg_id_strings.len()
        || channel_search_cursor != channel_search_arena.len()
    {
        return None;
    }

    // Validate channels_by_id strict digest ordering (O(N), proves uniqueness + bijection).
    let mut prev_digest: Option<&[u8; 32]> = None;
    for entry in channels_by_id {
        let idx = entry.get() as usize;
        let ch = channels_hot.get(idx)?;
        if let Some(prev) = prev_digest
            && prev >= &ch.id_digest
        {
            return None;
        }
        prev_digest = Some(&ch.id_digest);
    }

    // Validate matched programmes and parallel tables with contiguous non-overlapping spans.
    let mut prog_title_cursor = 0_usize;
    let mut prog_desc_cursor = 0_usize;
    let mut prog_search_cursor = 0_usize;
    for index in 0..matched_prog_count {
        let prog = &matched_programmes_hot[index];
        let starts_at =
            valid_browser_timestamp(prog.starts_at_secs.get(), prog.starts_at_nanos.get())?;
        let ends_at = valid_browser_timestamp(prog.ends_at_secs.get(), prog.ends_at_nanos.get())?;
        if ends_at <= starts_at {
            return None;
        }
        checked_contiguous_span(
            matched_programme_titles,
            &mut prog_title_cursor,
            prog.title_off.get(),
            prog.title_len.get(),
        )?;

        let desc_span = matched_programme_desc_spans[index];
        if !(desc_span.off.get() == ABSENT_U32 && desc_span.len.get() == ABSENT_U32) {
            checked_contiguous_span(
                matched_programme_descriptions,
                &mut prog_desc_cursor,
                desc_span.off.get(),
                desc_span.len.get(),
            )?;
        }

        let search = &programme_search_spans[index];
        checked_contiguous_span(
            programme_search_arena,
            &mut prog_search_cursor,
            search.primary_off.get(),
            search.primary_len.get(),
        )?;
        if !(search.secondary_off.get() == ABSENT_U32 && search.secondary_len.get() == ABSENT_U32) {
            checked_contiguous_span(
                programme_search_arena,
                &mut prog_search_cursor,
                search.secondary_off.get(),
                search.secondary_len.get(),
            )?;
        }

        if (matched_prog_guide_ids[index].get() as usize) >= guide_id_count {
            return None;
        }
    }
    if prog_title_cursor != matched_programme_titles.len()
        || prog_desc_cursor != matched_programme_descriptions.len()
        || prog_search_cursor != programme_search_arena.len()
    {
        return None;
    }

    // Validate schedule_entries and schedule_overlap in ChannelId order
    // (exact contiguous partition, non-decreasing starts_at, exact running-max ends_at).
    let mut schedule_cursor = 0_usize;
    for ch_entry in channels_by_id {
        let ch_idx = ch_entry.get() as usize;
        let sched = channel_schedules[ch_idx];
        let start = sched.start.get() as usize;
        let end = sched.end.get() as usize;
        if start != schedule_cursor || start > end || end > schedule_entry_count {
            return None;
        }
        let mut prev_start: Option<(i64, u32)> = None;
        let mut running_max_end: Option<(i64, u32)> = None;
        for entry_index in start..end {
            let entry = &schedule_entries[entry_index];
            if entry.channel_index.get() as usize != ch_idx {
                return None;
            }
            let prog_idx = entry.matched_prog_index.get() as usize;
            let prog = matched_programmes_hot.get(prog_idx)?;
            let prog_start = (prog.starts_at_secs.get(), prog.starts_at_nanos.get());
            let prog_end = (prog.ends_at_secs.get(), prog.ends_at_nanos.get());
            if let Some(prev) = prev_start
                && prog_start < prev
            {
                return None;
            }
            prev_start = Some(prog_start);
            running_max_end = Some(running_max_end.map_or(prog_end, |max| max.max(prog_end)));

            let overlap_idx = schedule_overlap[entry_index].get() as usize;
            let overlap_prog = matched_programmes_hot.get(overlap_idx)?;
            let overlap_end = (
                overlap_prog.ends_at_secs.get(),
                overlap_prog.ends_at_nanos.get(),
            );
            if Some(overlap_end) != running_max_end {
                return None;
            }
        }
        schedule_cursor = end;
    }
    if schedule_cursor != schedule_entry_count {
        return None;
    }

    // Validate cold EPG rematch metadata with contiguous non-overlapping spans.
    let mut guide_id_cursor = 0_usize;
    for span in guide_id_spans {
        checked_contiguous_span(
            guide_id_strings,
            &mut guide_id_cursor,
            span.off.get(),
            span.len.get(),
        )?;
    }
    if guide_id_cursor != guide_id_strings.len() {
        return None;
    }

    let mut display_cursor = 0_usize;
    for gc in guide_channels {
        if (gc.guide_id_index.get() as usize) >= guide_id_count {
            return None;
        }
        let start = gc.display_start.get() as usize;
        let end = gc.display_end.get() as usize;
        if start != display_cursor || start > end || end > guide_display_count {
            return None;
        }
        display_cursor = end;
    }
    if display_cursor != guide_display_count {
        return None;
    }

    let mut guide_display_str_cursor = 0_usize;
    for span in guide_display_spans {
        checked_contiguous_span(
            guide_display_strings,
            &mut guide_display_str_cursor,
            span.off.get(),
            span.len.get(),
        )?;
    }
    if guide_display_str_cursor != guide_display_strings.len() {
        return None;
    }

    let mut unmatched_str_cursor = 0_usize;
    for unmatched in unmatched_programmes {
        if (unmatched.guide_id_index.get() as usize) >= guide_id_count {
            return None;
        }
        let starts_at = valid_browser_timestamp(
            unmatched.starts_at_secs.get(),
            unmatched.starts_at_nanos.get(),
        )?;
        let ends_at =
            valid_browser_timestamp(unmatched.ends_at_secs.get(), unmatched.ends_at_nanos.get())?;
        if ends_at <= starts_at {
            return None;
        }
        checked_contiguous_span(
            unmatched_programme_strings,
            &mut unmatched_str_cursor,
            unmatched.title_off.get(),
            unmatched.title_len.get(),
        )?;
        if !(unmatched.desc_off.get() == ABSENT_U32 && unmatched.desc_len.get() == ABSENT_U32) {
            checked_contiguous_span(
                unmatched_programme_strings,
                &mut unmatched_str_cursor,
                unmatched.desc_off.get(),
                unmatched.desc_len.get(),
            )?;
        }
    }
    if unmatched_str_cursor != unmatched_programme_strings.len() {
        return None;
    }

    // Prove guide_prog_order is a bijection across matched + unmatched lanes in O(P).
    let mut next_matched = 0_u32;
    let mut next_unmatched = 0_u32;
    for entry in guide_prog_order {
        let raw = entry.get();
        if raw & UNMATCHED_PROG_BIT == 0 {
            if raw != next_matched {
                return None;
            }
            next_matched = next_matched.checked_add(1)?;
        } else {
            let idx = raw & !UNMATCHED_PROG_BIT;
            if idx != next_unmatched {
                return None;
            }
            next_unmatched = next_unmatched.checked_add(1)?;
        }
    }
    if next_matched as usize != matched_prog_count
        || next_unmatched as usize != unmatched_prog_count
    {
        return None;
    }

    Some(layout)
}
