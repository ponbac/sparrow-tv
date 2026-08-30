pub(crate) mod dto;
pub(crate) mod input;
pub(crate) mod subscriptions;

use tauri::{State, ipc::Channel};

use crate::runtime::{InstalledRuntime, InstalledRuntimeSlot};

use self::{
    dto::{
        CapabilitiesDto, CatalogStatusDto, ChannelDetailsDto, ChannelGroupDto, ChannelSummaryDto,
        ClientErrorDto, CoreEventDto, PageDto, ProgrammeDto, RefreshReportDto, SearchResultsDto,
    },
    input::{
        ChannelInput, ListChannelsInput, ListGroupsInput, ScheduleInput, SearchCancellationInput,
        SearchInput, SearchPageInput, SourceConfigurationInputDto,
    },
};

#[tauri::command]
pub(crate) fn installed_capabilities() -> CapabilitiesDto {
    CapabilitiesDto::installed_catalog()
}

#[tauri::command]
pub(crate) async fn catalog_status(
    slot: State<'_, InstalledRuntimeSlot>,
) -> Result<CatalogStatusDto, ClientErrorDto> {
    Ok(CatalogStatusDto::from(slot.wait().await.status()))
}

#[tauri::command]
pub(crate) async fn catalog_list_groups(
    slot: State<'_, InstalledRuntimeSlot>,
    input: ListGroupsInput,
) -> Result<PageDto<ChannelGroupDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    list_groups(runtime.as_ref(), input)
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
pub(crate) async fn catalog_list_channels(
    slot: State<'_, InstalledRuntimeSlot>,
    input: ListChannelsInput,
) -> Result<PageDto<ChannelSummaryDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    list_channels(runtime.as_ref(), input)
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
pub(crate) async fn catalog_channel(
    slot: State<'_, InstalledRuntimeSlot>,
    input: ChannelInput,
) -> Result<ChannelDetailsDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    channel(runtime.as_ref(), input)
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
pub(crate) async fn catalog_schedule(
    slot: State<'_, InstalledRuntimeSlot>,
    input: ScheduleInput,
) -> Result<PageDto<ProgrammeDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    schedule(runtime.as_ref(), input)
}

pub(crate) fn schedule(
    state: &InstalledRuntime,
    input: ScheduleInput,
) -> Result<PageDto<ProgrammeDto>, ClientErrorDto> {
    let query = input.into_core()?;
    state
        .core()
        .schedule(query)
        .map(|page| PageDto::programmes(&page))
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn catalog_search(
    slot: State<'_, InstalledRuntimeSlot>,
    input: SearchInput,
) -> Result<SearchResultsDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    search(runtime.as_ref(), input).await
}

pub(crate) async fn search(
    state: &InstalledRuntime,
    input: SearchInput,
) -> Result<SearchResultsDto, ClientErrorDto> {
    let (request_id, request) = input.into_core()?;
    state
        .search(request_id, request)
        .await
        .map(|results| SearchResultsDto::from(&results))
}

#[tauri::command]
pub(crate) async fn catalog_search_channels(
    slot: State<'_, InstalledRuntimeSlot>,
    input: SearchPageInput,
) -> Result<PageDto<ChannelSummaryDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    search_channels(runtime.as_ref(), input).await
}

pub(crate) async fn search_channels(
    state: &InstalledRuntime,
    input: SearchPageInput,
) -> Result<PageDto<ChannelSummaryDto>, ClientErrorDto> {
    let (request_id, term, page) = input.into_core()?;
    state
        .search_channels(request_id, term, page)
        .await
        .map(|page| PageDto::channels(&page))
}

#[tauri::command]
pub(crate) async fn catalog_search_programmes(
    slot: State<'_, InstalledRuntimeSlot>,
    input: SearchPageInput,
) -> Result<PageDto<ProgrammeDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    search_programmes(runtime.as_ref(), input).await
}

pub(crate) async fn search_programmes(
    state: &InstalledRuntime,
    input: SearchPageInput,
) -> Result<PageDto<ProgrammeDto>, ClientErrorDto> {
    let (request_id, term, page) = input.into_core()?;
    state
        .search_programmes(request_id, term, page)
        .await
        .map(|page| PageDto::programmes(&page))
}

#[tauri::command]
pub(crate) async fn catalog_search_cancel(
    slot: State<'_, InstalledRuntimeSlot>,
    input: SearchCancellationInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    cancel_search(runtime.as_ref(), input)
}

pub(crate) fn cancel_search(
    state: &InstalledRuntime,
    input: SearchCancellationInput,
) -> Result<(), ClientErrorDto> {
    state.cancel_search(input.into_request_id()?);
    Ok(())
}

#[tauri::command]
pub(crate) async fn catalog_refresh(
    slot: State<'_, InstalledRuntimeSlot>,
) -> Result<RefreshReportDto, ClientErrorDto> {
    Ok(RefreshReportDto::from(slot.wait().await.refresh().await))
}

#[tauri::command]
pub(crate) async fn source_configuration_replace(
    slot: State<'_, InstalledRuntimeSlot>,
    input: SourceConfigurationInputDto,
) -> Result<CatalogStatusDto, ClientErrorDto> {
    slot.wait().await.replace_configuration(input).await
}

#[tauri::command]
pub(crate) async fn catalog_subscribe(
    slot: State<'_, InstalledRuntimeSlot>,
    events: Channel<CoreEventDto>,
) -> Result<String, ClientErrorDto> {
    slot.wait().await.subscribe(events)
}

#[tauri::command]
pub(crate) async fn catalog_unsubscribe(
    slot: State<'_, InstalledRuntimeSlot>,
    subscription_id: String,
) -> Result<(), ClientErrorDto> {
    slot.wait().await.unsubscribe(&subscription_id);
    Ok(())
}
