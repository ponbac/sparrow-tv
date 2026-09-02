use serde::Deserialize;
use sparrow_core::{ChannelGroupFilter, ChannelQuery, GuideWindowQuery};

use super::{PageLimitInput, page_request};
use crate::ipc::dto::ClientErrorDto;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GuideWindowInput {
    starts_at: String,
    ends_at: String,
    channel_limit: PageLimitInput,
    group: Option<String>,
    cursor: Option<String>,
}

impl GuideWindowInput {
    pub(crate) fn into_core(self) -> Result<GuideWindowQuery, ClientErrorDto> {
        let page = page_request(self.channel_limit, self.cursor)?;
        let channels = match self.group {
            Some(group) => ChannelQuery::in_group(
                ChannelGroupFilter::parse(group).map_err(ClientErrorDto::from)?,
                page,
            ),
            None => ChannelQuery::all(page),
        };
        GuideWindowQuery::parse(self.starts_at, self.ends_at, channels)
            .map_err(ClientErrorDto::from)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn refines_instants_duration_group_and_page() {
        for (starts_at, ends_at) in [
            ("2026-08-30T19:00:00+02:00", "2026-08-30T22:00:00+02:00"),
            (
                "2026-08-30T19:00:00.0000000001Z",
                "2026-08-30T19:00:00.0000000019Z",
            ),
        ] {
            let valid: GuideWindowInput = serde_json::from_value(json!({
                "startsAt": starts_at,
                "endsAt": ends_at,
                "channelLimit": 24,
                "group": "News"
            }))
            .expect("the guide input shape parses");
            assert!(valid.into_core().is_ok());
        }

        for (input, field, reason) in [
            (
                json!({
                    "startsAt": "not-an-instant",
                    "endsAt": "2026-08-30T22:00:00Z",
                    "channelLimit": 24
                }),
                "guide-starts-at",
                "invalid-format",
            ),
            (
                json!({
                    "startsAt": "x".repeat(GuideWindowQuery::MAX_INSTANT_BYTES + 1),
                    "endsAt": "2026-08-30T22:00:00Z",
                    "channelLimit": 24
                }),
                "guide-starts-at",
                "too-long",
            ),
            (
                json!({
                    "startsAt": "2026-08-30T19:00:00Z",
                    "endsAt": "2026-08-31T19:00:01Z",
                    "channelLimit": 24
                }),
                "guide-ends-at",
                "out-of-range",
            ),
        ] {
            let input: GuideWindowInput =
                serde_json::from_value(input).expect("the invalid guide shape still parses");
            assert!(matches!(
                input.into_core(),
                Err(ClientErrorDto::InvalidInput {
                    field: actual_field,
                    reason: actual_reason,
                }) if actual_field == field && actual_reason == reason
            ));
        }

        assert!(
            serde_json::from_value::<GuideWindowInput>(json!({
                "startsAt": "2026-08-30T19:00:00Z",
                "endsAt": "2026-08-30T22:00:00Z",
                "channelLimit": 24,
                "providerUrl": "https://provider.invalid/private"
            }))
            .is_err()
        );
    }
}
