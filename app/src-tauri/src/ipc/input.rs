use serde::Deserialize;
use sparrow_core::{
    ChannelGroupFilter, ChannelId, ChannelQuery, PageCursor, PageLimit, PageRequest, ScheduleQuery,
    SearchRequest, SearchTerm, SourceConfiguration,
};

use crate::config_store::StoredSourceConfiguration;

use super::dto::ClientErrorDto;

const MAX_CURSOR_BYTES: usize = 1024;
const SEARCH_REQUEST_ID_PREFIX: &str = "srch1_";
const SEARCH_REQUEST_ID_NONCE_HEX_BYTES: usize = 32;
const MAX_SEARCH_REQUEST_ID_BYTES: usize = 64;

#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct SearchRequestId(String);

impl SearchRequestId {
    fn parse(value: String) -> Result<Self, ClientErrorDto> {
        let Some(suffix) = value.strip_prefix(SEARCH_REQUEST_ID_PREFIX) else {
            return Err(ClientErrorDto::service_unavailable());
        };
        let Some((nonce, sequence)) = suffix.split_once('_') else {
            return Err(ClientErrorDto::service_unavailable());
        };
        if value.len() > MAX_SEARCH_REQUEST_ID_BYTES
            || nonce.len() != SEARCH_REQUEST_ID_NONCE_HEX_BYTES
            || sequence.is_empty()
            || !nonce.bytes().all(is_lower_hex)
            || !sequence.bytes().all(is_lower_hex)
        {
            return Err(ClientErrorDto::service_unavailable());
        }
        Ok(Self(value))
    }
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListGroupsInput {
    limit: PageLimitInput,
    cursor: Option<String>,
}

impl ListGroupsInput {
    pub(crate) fn into_core(self) -> Result<PageRequest, ClientErrorDto> {
        page_request(self.limit, self.cursor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListChannelsInput {
    limit: PageLimitInput,
    group: Option<String>,
    cursor: Option<String>,
}

impl ListChannelsInput {
    pub(crate) fn into_core(self) -> Result<ChannelQuery, ClientErrorDto> {
        let page = page_request(self.limit, self.cursor)?;
        match self.group {
            Some(group) => ChannelGroupFilter::parse(group)
                .map(|group| ChannelQuery::in_group(group, page))
                .map_err(ClientErrorDto::from),
            None => Ok(ChannelQuery::all(page)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelInput {
    id: String,
}

impl ChannelInput {
    pub(crate) fn into_core(self) -> Result<ChannelId, ClientErrorDto> {
        ChannelId::parse(self.id).map_err(ClientErrorDto::from)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScheduleInput {
    id: String,
    limit: PageLimitInput,
    cursor: Option<String>,
}

impl ScheduleInput {
    pub(crate) fn into_core(self) -> Result<ScheduleQuery, ClientErrorDto> {
        let channel_id = ChannelId::parse(self.id).map_err(ClientErrorDto::from)?;
        let page = page_request(self.limit, self.cursor)?;
        Ok(ScheduleQuery::new(channel_id, page))
    }
}

/// Raw search text exists only while refining an installed query. This type
/// intentionally implements neither `Debug` nor `Serialize`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchInput {
    request_id: String,
    term: String,
    channel_limit: PageLimitInput,
    channel_cursor: Option<String>,
    programme_limit: PageLimitInput,
    programme_cursor: Option<String>,
}

impl SearchInput {
    pub(crate) fn into_core(self) -> Result<(SearchRequestId, SearchRequest), ClientErrorDto> {
        let request_id = SearchRequestId::parse(self.request_id)?;
        let term = SearchTerm::parse(self.term).map_err(ClientErrorDto::from)?;
        let channels = page_request(self.channel_limit, self.channel_cursor)?;
        let programmes = page_request(self.programme_limit, self.programme_cursor)?;
        Ok((request_id, SearchRequest::new(term, channels, programmes)))
    }
}

/// Raw search text exists only while refining an installed lane query. This
/// type intentionally implements neither `Debug` nor `Serialize`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchPageInput {
    request_id: String,
    term: String,
    limit: PageLimitInput,
    cursor: Option<String>,
}

impl SearchPageInput {
    pub(crate) fn into_core(
        self,
    ) -> Result<(SearchRequestId, SearchTerm, PageRequest), ClientErrorDto> {
        let request_id = SearchRequestId::parse(self.request_id)?;
        let term = SearchTerm::parse(self.term).map_err(ClientErrorDto::from)?;
        let page = page_request(self.limit, self.cursor)?;
        Ok((request_id, term, page))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchCancellationInput {
    request_id: String,
}

impl SearchCancellationInput {
    pub(crate) fn into_request_id(self) -> Result<SearchRequestId, ClientErrorDto> {
        SearchRequestId::parse(self.request_id)
    }
}

/// Raw source locations exist only while processing the installed settings command.
/// This type intentionally implements neither `Debug` nor `Serialize`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceConfigurationInputDto {
    m3u_location: String,
    epg_location: Option<String>,
}

impl SourceConfigurationInputDto {
    pub(crate) fn validate(
        self,
    ) -> Result<(StoredSourceConfiguration, SourceConfiguration), ClientErrorDto> {
        let stored = StoredSourceConfiguration::normalized(self.m3u_location, self.epg_location);
        let configuration =
            sparrow_core::SparrowCore::parse_source_configuration(stored.source_input())
                .map_err(ClientErrorDto::from)?;
        Ok((stored, configuration))
    }
}

/// Keeps malformed JSON numbers inside the typed command boundary instead of
/// letting Serde turn them into an opaque Tauri transport rejection.
#[derive(Deserialize)]
#[serde(transparent)]
struct PageLimitInput(serde_json::Value);

impl PageLimitInput {
    fn into_core(self) -> Result<PageLimit, ClientErrorDto> {
        let value = self
            .0
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .ok_or(ClientErrorDto::InvalidInput {
                field: "page-limit",
                reason: "invalid-format",
            })?;
        PageLimit::new(value).map_err(ClientErrorDto::from)
    }
}

fn page_request(
    limit: PageLimitInput,
    cursor: Option<String>,
) -> Result<PageRequest, ClientErrorDto> {
    let limit = limit.into_core()?;
    let cursor = cursor
        .map(|cursor| {
            if cursor.len() > MAX_CURSOR_BYTES {
                return Err(ClientErrorDto::InvalidInput {
                    field: "page-cursor",
                    reason: "too-long",
                });
            }
            PageCursor::parse(cursor).map_err(ClientErrorDto::from)
        })
        .transpose()?;
    Ok(PageRequest::new(cursor, limit))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sparrow_core::CoreError;

    use super::*;

    #[test]
    fn strict_inputs_reject_unknown_fields_and_refine_values_in_core() {
        assert!(
            serde_json::from_value::<ListGroupsInput>(json!({
                "limit": 10,
                "cursor": null,
                "providerUrl": "https://provider.invalid/private"
            }))
            .is_err()
        );
        let input: ListGroupsInput = serde_json::from_value(json!({ "limit": 0, "cursor": null }))
            .expect("input shape parses");
        assert!(matches!(
            input.into_core(),
            Err(ClientErrorDto::InvalidInput {
                field: "page-limit",
                reason: "out-of-range"
            })
        ));

        assert!(
            serde_json::from_value::<SearchInput>(json!({
                "term": "news",
                "channelLimit": 10,
                "channelCursor": null,
                "programmeLimit": 10,
                "programmeCursor": null,
                "providerUrl": "https://provider.invalid/private"
            }))
            .is_err()
        );
    }

    #[test]
    fn schedule_and_search_inputs_share_core_refinement() {
        let invalid_schedule: ScheduleInput = serde_json::from_value(json!({
            "id": "not-a-channel-id",
            "limit": 10
        }))
        .expect("schedule shape parses");
        assert!(matches!(
            invalid_schedule.into_core(),
            Err(ClientErrorDto::InvalidInput {
                field: "channel-id",
                reason: "invalid-format"
            })
        ));

        let invalid_search: SearchInput = serde_json::from_value(json!({
            "requestId": "srch1_0123456789abcdef0123456789abcdef_1",
            "term": "   ",
            "channelLimit": 5,
            "programmeLimit": 5
        }))
        .expect("search shape parses");
        assert!(matches!(
            invalid_search.into_core(),
            Err(ClientErrorDto::InvalidInput {
                field: "search-term",
                reason: "required"
            })
        ));

        let invalid_lane: SearchPageInput = serde_json::from_value(json!({
            "requestId": "srch1_0123456789abcdef0123456789abcdef_2",
            "term": "news",
            "limit": 101
        }))
        .expect("lane shape parses");
        assert!(matches!(
            invalid_lane.into_core(),
            Err(ClientErrorDto::InvalidInput {
                field: "page-limit",
                reason: "out-of-range"
            })
        ));

        let oversized_cursor: SearchPageInput = serde_json::from_value(json!({
            "requestId": "srch1_0123456789abcdef0123456789abcdef_3",
            "term": "news",
            "limit": 10,
            "cursor": "x".repeat(MAX_CURSOR_BYTES + 1)
        }))
        .expect("lane shape parses");
        assert!(matches!(
            oversized_cursor.into_core(),
            Err(ClientErrorDto::InvalidInput {
                field: "page-cursor",
                reason: "too-long"
            })
        ));

        let invalid_request_id: SearchPageInput = serde_json::from_value(json!({
            "requestId": "search-secret-or-unbounded-value",
            "term": "news",
            "limit": 10
        }))
        .expect("lane shape parses");
        assert!(matches!(
            invalid_request_id.into_core(),
            Err(ClientErrorDto::ServiceUnavailable)
        ));
    }

    #[test]
    fn malformed_numeric_page_limits_return_typed_invalid_input() {
        for value in [
            json!(-1),
            json!(1.5),
            json!(65_536),
            json!("10"),
            json!(null),
        ] {
            let input: ListGroupsInput = serde_json::from_value(json!({
                "limit": value
            }))
            .expect("the command shape retains malformed limits for typed refinement");
            assert!(matches!(
                input.into_core(),
                Err(ClientErrorDto::InvalidInput {
                    field: "page-limit",
                    reason: "invalid-format"
                })
            ));
        }
    }

    #[test]
    fn source_input_is_private_normalized_and_validated_once() {
        let input: SourceConfigurationInputDto = serde_json::from_value(json!({
            "m3uLocation": "  https://example.invalid/list.m3u  ",
            "epgLocation": "   "
        }))
        .expect("input shape parses");
        let (stored, configuration) = input.validate().expect("locations validate");
        assert!(
            sparrow_core::SparrowCore::parse_source_configuration(stored.source_input()).is_ok()
        );
        assert!(!format!("{configuration:?}").contains("example.invalid"));

        let invalid: SourceConfigurationInputDto = serde_json::from_value(json!({
            "m3uLocation": "file:///private/list.m3u",
            "epgLocation": null
        }))
        .expect("input shape parses");
        let error = invalid
            .validate()
            .err()
            .expect("unsupported location is rejected");
        assert!(matches!(
            error,
            ClientErrorDto::InvalidInput {
                field: "m3u",
                reason: "unsupported-location"
            }
        ));
    }

    #[test]
    fn core_error_mapping_does_not_serialize_private_channel_input() {
        let private_canary = "https://provider.invalid/channel";
        let error = ChannelId::parse(private_canary).expect_err("URL is not a Channel ID");
        let serialized =
            serde_json::to_string(&ClientErrorDto::from(error)).expect("client error serializes");
        assert!(!serialized.contains(private_canary));
        assert!(!matches!(
            CoreError::NotConfigured,
            CoreError::InvalidInput { .. }
        ));
    }
}
