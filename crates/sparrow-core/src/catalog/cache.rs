//! Private derived storage, never a transport format. Bump the key domain when
//! changing the storage protocol. A build fingerprint also invalidates derived
//! data whenever core semantics or locked dependencies change.
use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error, ser::SerializeMap};

use super::ChannelCatalog;
use crate::domain::{CatalogGeneration, ChannelGroupView, ChannelId, SourceConfiguration};

pub(crate) fn key(
    configuration: &SourceConfiguration,
    m3u: &[u8; 32],
    epg: Option<&[u8; 32]>,
) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"sparrow-processed-catalog-v1\0");
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
        serde_json::to_vec(self).ok()
    }

    pub(crate) fn decode_cache(bytes: &[u8], generation: CatalogGeneration) -> Option<Self> {
        let catalog: Self = serde_json::from_slice(bytes).ok()?;
        (catalog.generation == generation && catalog.cache_indices_valid()).then_some(catalog)
    }

    pub(crate) fn cached_channels(&self) -> Arc<Vec<crate::m3u::ParsedChannel>> {
        Arc::clone(&self.source_channels)
    }

    pub(crate) fn cached_guide(&self) -> Option<Arc<crate::xmltv::ParsedGuide>> {
        self.source_guide.clone()
    }

    fn cache_indices_valid(&self) -> bool {
        let guide_len = self
            .source_guide
            .as_ref()
            .map_or(0, |guide| guide.programmes.len());
        let range_valid =
            |range: &std::ops::Range<usize>, len| range.start <= range.end && range.end <= len;
        self.channels
            .iter()
            .all(|c| c.source_index < self.source_channels.len())
            && self
                .programmes
                .iter()
                .all(|p| p.source_index < guide_len && self.by_id.contains_key(&p.channel_id))
            && self.by_id.len() == self.channels.len()
            && self
                .by_id
                .iter()
                .all(|(id, index)| self.channels.get(*index).is_some_and(|c| &c.id == id))
            && self
                .group_ranges
                .values()
                .all(|r| range_valid(r, self.channels.len()))
            && self
                .schedule_ranges
                .values()
                .all(|r| range_valid(r, self.programmes.len()))
            && self.schedule_overlap_index.prefix_max_end_sources.len() == self.programmes.len()
            && self
                .schedule_overlap_index
                .prefix_max_end_sources
                .iter()
                .all(|index| *index < guide_len)
            && self.channel_search.cache_valid(self.channels.len())
            && self.programme_search.cache_valid(self.programmes.len())
    }
}

pub(super) mod generation {
    use super::*;
    pub fn serialize<S: Serializer>(
        value: &CatalogGeneration,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.get().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<CatalogGeneration, D::Error> {
        CatalogGeneration::from_cursor(u64::deserialize(deserializer)?)
            .ok_or_else(|| D::Error::custom("invalid catalog generation"))
    }
}

pub(super) mod channel_id {
    use super::*;
    pub fn serialize<S: Serializer>(value: &ChannelId, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_str().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ChannelId, D::Error> {
        ChannelId::parse(String::deserialize(deserializer)?)
            .map_err(|_| D::Error::custom("invalid channel identifier"))
    }
}

pub(super) mod channel_map {
    use super::*;
    pub fn serialize<S: Serializer, T: Serialize>(
        value: &HashMap<ChannelId, T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(value.len()))?;
        for (id, item) in value {
            map.serialize_entry(id.as_str(), item)?;
        }
        map.end()
    }
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<HashMap<ChannelId, T>, D::Error> {
        HashMap::<String, T>::deserialize(deserializer)?
            .into_iter()
            .map(|(id, item)| {
                ChannelId::parse(id)
                    .map(|id| (id, item))
                    .map_err(|_| D::Error::custom("invalid channel identifier"))
            })
            .collect()
    }
}

pub(super) mod groups {
    use super::*;
    pub fn serialize<S: Serializer>(
        value: &[ChannelGroupView],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(|g| (g.name(), g.channel_count()))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<[ChannelGroupView]>, D::Error> {
        Ok(Vec::<(Arc<str>, u32)>::deserialize(deserializer)?
            .into_iter()
            .map(|(name, count)| ChannelGroupView::new(name, count))
            .collect())
    }
}
