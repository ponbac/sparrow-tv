use serde::Deserialize;
use sparrow_core::{
    ChannelGroupFilter, ChannelId, ChannelQuery, PageCursor, PageLimit, PageRequest,
    SourceConfiguration,
};

use crate::config_store::StoredSourceConfiguration;

use super::dto::ClientErrorDto;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListGroupsInput {
    limit: u16,
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
    limit: u16,
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

fn page_request(limit: u16, cursor: Option<String>) -> Result<PageRequest, ClientErrorDto> {
    let limit = PageLimit::new(limit).map_err(ClientErrorDto::from)?;
    let cursor = cursor
        .map(PageCursor::parse)
        .transpose()
        .map_err(ClientErrorDto::from)?;
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
