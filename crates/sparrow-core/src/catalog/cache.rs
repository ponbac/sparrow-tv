//! Private derived storage, never a transport format. Bump the key domain when
//! changing the storage protocol. A build fingerprint also invalidates derived
//! data whenever core semantics or locked dependencies change.
//!
//! Encoded in a compact, versioned little-endian binary format with interned
//! string tables so representative 64-75 MiB M3U + 24-31 MiB XMLTV catalogs
//! stay well under the 256 MiB cache ceiling and decode in milliseconds without
//! JSON parsing or intermediate maps.
use std::{collections::HashMap, ops::Range, sync::Arc};

use chrono::{DateTime, Utc};

use super::{
    CatalogChannel, ChannelCatalog,
    schedule::{CatalogProgramme, ScheduleOverlapIndex},
    search::{SearchDocument, SearchField, SearchIndex},
};
use crate::{
    domain::{CatalogGeneration, ChannelGroupView, ChannelId, ChannelQuality, SourceConfiguration},
    m3u::ParsedChannel,
    xmltv::{ParsedGuide, ParsedGuideChannel, ParsedProgramme},
};

const MAGIC: &[u8; 12] = b"SPARROWCAT\x02\x00";
const ABSENT_OFFSET: u32 = u32::MAX;

pub(crate) fn key(
    configuration: &SourceConfiguration,
    m3u: &[u8; 32],
    epg: Option<&[u8; 32]>,
) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"sparrow-processed-catalog-v2\0");
    hash.update(env!("SPARROW_CATALOG_BUILD").as_bytes());
    hash.update(configuration.fingerprint.as_bytes());
    hash.update(m3u);
    hash.update(&[u8::from(epg.is_some())]);
    if let Some(epg) = epg {
        hash.update(epg);
    }
    *hash.finalize().as_bytes()
}

impl ChannelCatalog {
    pub(crate) fn encode_cache(&self) -> Option<Vec<u8>> {
        let mut writer = BinaryWriter::with_capacity(self.estimated_encoded_bytes());
        writer.write_bytes(MAGIC);
        writer.write_u64(self.generation.get());

        // Intern Channel Groups once for both `source_channels` and `groups`.
        let mut group_table = StringTable::new();
        for channel in self.source_channels.iter() {
            group_table.intern(&channel.group);
        }
        for group in self.groups.iter() {
            group_table.intern(group.name());
        }
        group_table.write(&mut writer)?;

        // Source channels
        writer.write_len(self.source_channels.len())?;
        for channel in self.source_channels.iter() {
            writer.write_str(&channel.tvg_id)?;
            writer.write_str(&channel.name)?;
            writer.write_u32(group_table.index_of(&channel.group)?);
            writer.write_str(&channel.playback)?;
        }

        // Optional source guide with interned guide channel IDs.
        match self.source_guide.as_deref() {
            None => writer.write_u8(0),
            Some(guide) => {
                writer.write_u8(1);
                let mut guide_id_table = StringTable::new();
                for channel in &guide.channels {
                    guide_id_table.intern(&channel.id);
                }
                for programme in &guide.programmes {
                    guide_id_table.intern(&programme.guide_channel_id);
                }
                guide_id_table.write(&mut writer)?;

                writer.write_len(guide.channels.len())?;
                for channel in &guide.channels {
                    writer.write_u32(guide_id_table.index_of(&channel.id)?);
                    writer.write_len(channel.display_names.len())?;
                    for display_name in &channel.display_names {
                        writer.write_str(display_name)?;
                    }
                }

                writer.write_len(guide.programmes.len())?;
                for programme in &guide.programmes {
                    writer.write_u32(guide_id_table.index_of(&programme.guide_channel_id)?);
                    writer.write_str(&programme.title)?;
                    match programme.description.as_deref() {
                        None => writer.write_u8(0),
                        Some(description) => {
                            writer.write_u8(1);
                            writer.write_str(description)?;
                        }
                    }
                    writer.write_i64(programme.starts_at.timestamp());
                    writer.write_u32(programme.starts_at.timestamp_subsec_nanos());
                    writer.write_i64(programme.ends_at.timestamp());
                    writer.write_u32(programme.ends_at.timestamp_subsec_nanos());
                }
            }
        }

        // Channel Groups (ordered; `group_ranges` is rebuilt on decode from counts)
        writer.write_len(self.groups.len())?;
        for group in self.groups.iter() {
            writer.write_u32(group_table.index_of(group.name())?);
            writer.write_u32(group.channel_count());
        }

        // Catalog channels (`by_id` is rebuilt on decode)
        writer.write_len(self.channels.len())?;
        for channel in self.channels.iter() {
            writer.write_str(channel.id.as_str())?;
            writer.write_u32(u32::try_from(channel.source_index).ok()?);
            writer.write_u32(channel.number);
            writer.write_u8(encode_quality(channel.quality));
            writer.write_u32(channel.schedule_start);
            writer.write_u32(channel.schedule_end);
        }

        // Catalog programmes (8 bytes each)
        writer.write_len(self.programmes.len())?;
        for programme in self.programmes.iter() {
            writer.write_u32(programme.channel_index);
            writer.write_u32(programme.source_index);
        }

        // Schedule overlap index (4 bytes each)
        writer.write_len(self.schedule_overlap_index.prefix_max_end_sources.len())?;
        for &source_index in self.schedule_overlap_index.prefix_max_end_sources.iter() {
            writer.write_u32(source_index);
        }

        // Search indexes
        write_search_index(&mut writer, &self.channel_search)?;
        write_search_index(&mut writer, &self.programme_search)?;

        Some(writer.into_vec())
    }

    pub(crate) fn decode_cache(bytes: &[u8], generation: CatalogGeneration) -> Option<Self> {
        let mut reader = BinaryReader::new(bytes);
        if reader.read_fixed::<12>()? != MAGIC {
            return None;
        }
        let encoded_generation = CatalogGeneration::from_cursor(reader.read_u64()?)?;
        if encoded_generation != generation {
            return None;
        }

        let group_table = read_string_table(&mut reader)?;

        let source_channel_count = reader.read_len()?;
        let mut source_channels = Vec::with_capacity(source_channel_count);
        for _ in 0..source_channel_count {
            let tvg_id = reader.read_arc_str()?;
            let name = reader.read_arc_str()?;
            let group_index = reader.read_u32()? as usize;
            let group = Arc::clone(group_table.get(group_index)?);
            let playback_raw = reader.read_str()?;
            if !is_cached_http_playback(playback_raw) {
                return None;
            }
            source_channels.push(ParsedChannel {
                tvg_id,
                name,
                group,
                playback: Arc::from(playback_raw),
            });
        }

        let source_guide = match reader.read_u8()? {
            0 => None,
            1 => {
                let guide_id_table = read_string_table(&mut reader)?;
                let guide_channel_count = reader.read_len()?;
                let mut guide_channels = Vec::with_capacity(guide_channel_count);
                for _ in 0..guide_channel_count {
                    let id = Arc::clone(guide_id_table.get(reader.read_u32()? as usize)?);
                    let display_count = reader.read_len()?;
                    let mut display_names = Vec::with_capacity(display_count);
                    for _ in 0..display_count {
                        display_names.push(reader.read_arc_str()?);
                    }
                    guide_channels.push(ParsedGuideChannel { id, display_names });
                }

                let guide_programme_count = reader.read_len()?;
                let mut guide_programmes = Vec::with_capacity(guide_programme_count);
                for _ in 0..guide_programme_count {
                    let guide_channel_id =
                        Arc::clone(guide_id_table.get(reader.read_u32()? as usize)?);
                    let title = reader.read_arc_str()?;
                    let description = match reader.read_u8()? {
                        0 => None,
                        1 => Some(reader.read_arc_str()?),
                        _ => return None,
                    };
                    let starts_at =
                        DateTime::<Utc>::from_timestamp(reader.read_i64()?, reader.read_u32()?)?;
                    let ends_at =
                        DateTime::<Utc>::from_timestamp(reader.read_i64()?, reader.read_u32()?)?;
                    guide_programmes.push(ParsedProgramme {
                        guide_channel_id,
                        title,
                        description,
                        starts_at,
                        ends_at,
                    });
                }

                Some(Arc::new(ParsedGuide {
                    channels: guide_channels,
                    programmes: guide_programmes,
                }))
            }
            _ => return None,
        };

        let group_count = reader.read_len()?;
        let mut groups = Vec::with_capacity(group_count);
        let mut group_ranges = HashMap::<Arc<str>, Range<usize>>::with_capacity(group_count);
        let mut group_cursor = 0_usize;
        for _ in 0..group_count {
            let name = Arc::clone(group_table.get(reader.read_u32()? as usize)?);
            let channel_count = reader.read_u32()?;
            let group_end = group_cursor.checked_add(channel_count as usize)?;
            if group_ranges
                .insert(Arc::clone(&name), group_cursor..group_end)
                .is_some()
            {
                return None;
            }
            groups.push(ChannelGroupView::new(name, channel_count));
            group_cursor = group_end;
        }

        let channel_count = reader.read_len()?;
        if group_cursor != channel_count {
            return None;
        }
        let mut channels = Vec::with_capacity(channel_count);
        let mut by_id = HashMap::with_capacity(channel_count);
        for index in 0..channel_count {
            let id = ChannelId::parse_str(reader.read_str()?).ok()?;
            let source_index = reader.read_u32()? as usize;
            let number = reader.read_u32()?;
            let quality = decode_quality(reader.read_u8()?)?;
            let schedule_start = reader.read_u32()?;
            let schedule_end = reader.read_u32()?;
            if by_id.insert(id.clone(), index).is_some() {
                return None;
            }
            channels.push(CatalogChannel {
                id,
                source_index,
                number,
                quality,
                schedule_start,
                schedule_end,
            });
        }

        let programme_count = reader.read_len()?;
        let mut programmes = Vec::with_capacity(programme_count);
        for _ in 0..programme_count {
            let channel_index = reader.read_u32()?;
            let source_index = reader.read_u32()?;
            programmes.push(CatalogProgramme {
                channel_index,
                source_index,
            });
        }

        let overlap_count = reader.read_len()?;
        let mut prefix_max_end_sources = Vec::with_capacity(overlap_count);
        for _ in 0..overlap_count {
            prefix_max_end_sources.push(reader.read_u32()?);
        }

        let channel_search = read_search_index(&mut reader)?;
        let programme_search = read_search_index(&mut reader)?;

        if !reader.is_exhausted() {
            return None;
        }

        let catalog = Self {
            generation,
            source_channels: Arc::new(source_channels),
            source_guide,
            groups: Arc::from(groups),
            channels: channels.into_boxed_slice(),
            programmes: programmes.into_boxed_slice(),
            channel_search,
            programme_search,
            group_ranges,
            schedule_overlap_index: ScheduleOverlapIndex {
                prefix_max_end_sources: prefix_max_end_sources.into_boxed_slice(),
            },
            by_id,
        };
        catalog.cache_indices_valid().then_some(catalog)
    }

    fn estimated_encoded_bytes(&self) -> usize {
        let guide_bytes = self
            .source_guide
            .as_deref()
            .map_or(0, |guide| guide.programmes.len().saturating_mul(64));
        1024 + self.source_channels.len().saturating_mul(160)
            + self.channels.len().saturating_mul(88)
            + self.programmes.len().saturating_mul(12)
            + self.channel_search.arena.len()
            + self.channel_search.documents.len().saturating_mul(16)
            + self.programme_search.arena.len()
            + self.programme_search.documents.len().saturating_mul(16)
            + guide_bytes
    }

    pub(crate) fn cached_channels(&self) -> Arc<Vec<ParsedChannel>> {
        Arc::clone(&self.source_channels)
    }

    pub(crate) fn cached_guide(&self) -> Option<Arc<ParsedGuide>> {
        self.source_guide.clone()
    }

    fn cache_indices_valid(&self) -> bool {
        let guide_len = self
            .source_guide
            .as_ref()
            .map_or(0, |guide| guide.programmes.len());
        let range_valid = |range: &Range<usize>, len| range.start <= range.end && range.end <= len;
        self.channels.iter().all(|c| {
            c.source_index < self.source_channels.len()
                && c.schedule_start <= c.schedule_end
                && (c.schedule_end as usize) <= self.programmes.len()
        }) && self.programmes.iter().all(|p| {
            (p.source_index as usize) < guide_len
                && (p.channel_index as usize) < self.channels.len()
        }) && self.by_id.len() == self.channels.len()
            && self
                .group_ranges
                .values()
                .all(|r| range_valid(r, self.channels.len()))
            && self.schedule_overlap_index.prefix_max_end_sources.len() == self.programmes.len()
            && self
                .schedule_overlap_index
                .prefix_max_end_sources
                .iter()
                .all(|&index| (index as usize) < guide_len)
            && self.channel_search.cache_valid(self.channels.len())
            && self.programme_search.cache_valid(self.programmes.len())
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

fn decode_quality(value: u8) -> Option<Option<ChannelQuality>> {
    match value {
        0 => Some(None),
        1 => Some(Some(ChannelQuality::Sd)),
        2 => Some(Some(ChannelQuality::Hd)),
        3 => Some(Some(ChannelQuality::Fhd)),
        4 => Some(Some(ChannelQuality::Uhd)),
        _ => None,
    }
}

fn write_search_index(writer: &mut BinaryWriter, index: &SearchIndex) -> Option<()> {
    writer.write_str(&index.arena)?;
    writer.write_len(index.documents.len())?;
    for doc in index.documents.iter() {
        writer.write_u32(u32::try_from(doc.primary.start).ok()?);
        writer.write_u32(u32::try_from(doc.primary.end).ok()?);
        match doc.secondary() {
            Some(secondary) => {
                let start = u32::try_from(secondary.start).ok()?;
                let end = u32::try_from(secondary.end).ok()?;
                if start == ABSENT_OFFSET || end == ABSENT_OFFSET {
                    return None;
                }
                writer.write_u32(start);
                writer.write_u32(end);
            }
            None => {
                writer.write_u32(ABSENT_OFFSET);
                writer.write_u32(ABSENT_OFFSET);
            }
        }
    }
    Some(())
}

fn read_search_index(reader: &mut BinaryReader<'_>) -> Option<SearchIndex> {
    let arena = reader.read_str()?.to_owned();
    let count = reader.read_len()?;
    let mut documents = Vec::with_capacity(count);
    for _ in 0..count {
        let primary_start = reader.read_u32()? as usize;
        let primary_end = reader.read_u32()? as usize;
        let secondary_start = reader.read_u32()?;
        let secondary_end = reader.read_u32()?;
        let secondary = if secondary_start == ABSENT_OFFSET && secondary_end == ABSENT_OFFSET {
            None
        } else {
            Some(SearchField {
                start: secondary_start as usize,
                end: secondary_end as usize,
            })
        };
        documents.push(SearchDocument::new(
            SearchField {
                start: primary_start,
                end: primary_end,
            },
            secondary,
        ));
    }
    Some(SearchIndex {
        arena,
        documents: documents.into_boxed_slice(),
    })
}

struct StringTable<'a> {
    indices: HashMap<&'a str, u32>,
    values: Vec<&'a str>,
}

impl<'a> StringTable<'a> {
    fn new() -> Self {
        Self {
            indices: HashMap::new(),
            values: Vec::new(),
        }
    }

    fn intern(&mut self, value: &'a str) {
        if !self.indices.contains_key(value)
            && let Ok(index) = u32::try_from(self.values.len())
        {
            self.indices.insert(value, index);
            self.values.push(value);
        }
    }

    fn index_of(&self, value: &str) -> Option<u32> {
        self.indices.get(value).copied()
    }

    fn write(&self, writer: &mut BinaryWriter) -> Option<()> {
        writer.write_len(self.values.len())?;
        for value in &self.values {
            writer.write_str(value)?;
        }
        Some(())
    }
}

fn read_string_table(reader: &mut BinaryReader<'_>) -> Option<Vec<Arc<str>>> {
    let count = reader.read_len()?;
    let mut table = Vec::with_capacity(count);
    for _ in 0..count {
        table.push(reader.read_arc_str()?);
    }
    Some(table)
}

struct BinaryWriter {
    buffer: Vec<u8>,
}

impl BinaryWriter {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.buffer
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    fn write_u8(&mut self, value: u8) {
        self.buffer.push(value);
    }

    fn write_u32(&mut self, value: u32) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn write_i64(&mut self, value: i64) {
        self.buffer.extend_from_slice(&value.to_le_bytes());
    }

    fn write_len(&mut self, len: usize) -> Option<()> {
        self.write_u32(u32::try_from(len).ok()?);
        Some(())
    }

    fn write_str(&mut self, value: &str) -> Option<()> {
        self.write_len(value.len())?;
        self.buffer.extend_from_slice(value.as_bytes());
        Some(())
    }
}

fn is_cached_http_playback(value: &str) -> bool {
    let Some(rest) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.is_empty() && !rest.starts_with('/') && !rest.bytes().any(|b| b.is_ascii_control())
}

struct BinaryReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    empty: Arc<str>,
}

impl<'a> BinaryReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            empty: Arc::from(""),
        }
    }

    fn is_exhausted(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_slice(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(len)?;
        let slice = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(slice)
    }

    fn read_fixed<const N: usize>(&mut self) -> Option<&'a [u8; N]> {
        self.read_slice(N)?.try_into().ok()
    }

    fn read_u8(&mut self) -> Option<u8> {
        Some(self.read_fixed::<1>()?[0])
    }

    fn read_u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(*self.read_fixed::<4>()?))
    }

    fn read_u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(*self.read_fixed::<8>()?))
    }

    fn read_i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(*self.read_fixed::<8>()?))
    }

    fn read_len(&mut self) -> Option<usize> {
        let len = self.read_u32()? as usize;
        // Every element consumes at least 1 byte in the payload; reject bogus
        // counts up front before allocating `Vec::with_capacity`.
        if len > self.remaining() {
            return None;
        }
        Some(len)
    }

    fn read_str(&mut self) -> Option<&'a str> {
        let len = self.read_len()?;
        std::str::from_utf8(self.read_slice(len)?).ok()
    }

    fn read_arc_str(&mut self) -> Option<Arc<str>> {
        let value = self.read_str()?;
        if value.is_empty() {
            Some(Arc::clone(&self.empty))
        } else {
            Some(Arc::from(value))
        }
    }
}
