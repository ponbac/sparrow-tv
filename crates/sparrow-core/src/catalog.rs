use std::{collections::HashMap, fmt, sync::Arc};

use url::Url;

use crate::{
    domain::{
        CatalogGeneration, ChannelDetails, ChannelId, ChannelSummary, CoreError, Page, PageLimit,
        SourceConfiguration,
    },
    identity,
    m3u::ParsedChannel,
};

pub(crate) struct ChannelCatalog {
    generation: CatalogGeneration,
    summaries: Arc<[ChannelSummary]>,
    records: Vec<ChannelRecord>,
    by_id: HashMap<ChannelId, usize>,
}

impl ChannelCatalog {
    pub(crate) fn from_parsed(
        configuration: &SourceConfiguration,
        parsed: Vec<ParsedChannel>,
        generation: CatalogGeneration,
    ) -> Self {
        let mut occurrences = HashMap::<[u8; 32], u32>::new();
        let mut summaries = Vec::with_capacity(parsed.len());
        let mut records = Vec::with_capacity(parsed.len());
        let mut by_id = HashMap::with_capacity(parsed.len());

        for channel in parsed {
            let seed = identity::seed(&channel.tvg_id, &channel.name, &channel.group);
            let occurrence = occurrences.entry(seed).or_default();
            let id = identity::channel_id(&configuration.fingerprint, &seed, *occurrence);
            *occurrence = occurrence.saturating_add(1);

            let name: Arc<str> = Arc::from(channel.name);
            let group: Arc<str> = Arc::from(channel.group);
            let summary = ChannelSummary::new(id.clone(), name.clone(), group.clone());
            let details = ChannelDetails::new(id.clone(), name, group);
            let record = ChannelRecord {
                details,
                _playback: SecretPlaybackLocation(channel.playback),
            };
            let index = records.len();

            debug_assert!(by_id.insert(id, index).is_none());
            summaries.push(summary);
            records.push(record);
        }

        Self {
            generation,
            summaries: Arc::from(summaries),
            records,
            by_id,
        }
    }

    pub(crate) fn first_page(&self, limit: PageLimit) -> Page<ChannelSummary> {
        Page::first(self.generation, Arc::clone(&self.summaries), limit)
    }

    pub(crate) fn channel(&self, id: &ChannelId) -> Result<ChannelDetails, CoreError> {
        self.by_id
            .get(id)
            .map(|index| self.records[*index].details.clone())
            .ok_or_else(|| CoreError::ChannelNotFound { id: id.clone() })
    }
}

struct ChannelRecord {
    details: ChannelDetails,
    _playback: SecretPlaybackLocation,
}

struct SecretPlaybackLocation(Url);

impl fmt::Debug for SecretPlaybackLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = &self.0;
        formatter.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::ChannelCatalog;

    assert_not_impl_any!(ChannelCatalog: Clone);
}
