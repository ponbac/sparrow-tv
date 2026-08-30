pub(crate) mod dto;
pub(crate) mod input;
pub(crate) mod subscriptions;

use tauri::{State, ipc::Channel};

use crate::runtime::InstalledRuntime;

use self::{
    dto::{
        CapabilitiesDto, CatalogStatusDto, ChannelDetailsDto, ChannelGroupDto, ChannelSummaryDto,
        ClientErrorDto, CoreEventDto, PageDto,
    },
    input::{ChannelInput, ListChannelsInput, ListGroupsInput, SourceConfigurationInputDto},
};

#[tauri::command]
pub(crate) fn installed_capabilities() -> CapabilitiesDto {
    CapabilitiesDto::installed_catalog()
}

#[tauri::command]
pub(crate) fn catalog_status(state: State<'_, InstalledRuntime>) -> CatalogStatusDto {
    CatalogStatusDto::from(state.core().status())
}

#[tauri::command]
pub(crate) fn catalog_list_groups(
    state: State<'_, InstalledRuntime>,
    input: ListGroupsInput,
) -> Result<PageDto<ChannelGroupDto>, ClientErrorDto> {
    list_groups(state.inner(), input)
}

pub(crate) fn list_groups(
    state: &InstalledRuntime,
    input: ListGroupsInput,
) -> Result<PageDto<ChannelGroupDto>, ClientErrorDto> {
    let request = input.into_core()?;
    state
        .core()
        .list_groups(request)
        .map(|page| PageDto::groups(&page))
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) fn catalog_list_channels(
    state: State<'_, InstalledRuntime>,
    input: ListChannelsInput,
) -> Result<PageDto<ChannelSummaryDto>, ClientErrorDto> {
    list_channels(state.inner(), input)
}

pub(crate) fn list_channels(
    state: &InstalledRuntime,
    input: ListChannelsInput,
) -> Result<PageDto<ChannelSummaryDto>, ClientErrorDto> {
    let query = input.into_core()?;
    state
        .core()
        .list_channels(query)
        .map(|page| PageDto::channels(&page))
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) fn catalog_channel(
    state: State<'_, InstalledRuntime>,
    input: ChannelInput,
) -> Result<ChannelDetailsDto, ClientErrorDto> {
    channel(state.inner(), input)
}

pub(crate) fn channel(
    state: &InstalledRuntime,
    input: ChannelInput,
) -> Result<ChannelDetailsDto, ClientErrorDto> {
    let id = input.into_core()?;
    state
        .core()
        .channel(&id)
        .map(|channel| ChannelDetailsDto::from(&channel))
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn source_configuration_replace(
    state: State<'_, InstalledRuntime>,
    input: SourceConfigurationInputDto,
) -> Result<CatalogStatusDto, ClientErrorDto> {
    state.replace_configuration(input).await
}

#[tauri::command]
pub(crate) fn catalog_subscribe(
    state: State<'_, InstalledRuntime>,
    events: Channel<CoreEventDto>,
) -> Result<String, ClientErrorDto> {
    state.subscribe(events)
}

#[tauri::command]
pub(crate) fn catalog_unsubscribe(state: State<'_, InstalledRuntime>, subscription_id: String) {
    state.unsubscribe(&subscription_id);
}
