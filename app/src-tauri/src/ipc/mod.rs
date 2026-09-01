pub(crate) mod dto;
pub(crate) mod input;
pub(crate) mod subscriptions;

use tauri::{
    State,
    ipc::{Channel, Response},
};

use crate::runtime::{InstalledRuntime, InstalledRuntimeSlot};

use self::{
    dto::{
        AndroidPlaybackStatusDto, CapabilitiesDto, CatalogStatusDto, ChannelDetailsDto,
        ChannelGroupDto, ChannelSummaryDto, ClientErrorDto, CoreEventDto, GuideWindowChannelDto,
        PageDto, PlaybackDescriptorDto, ProgrammeDto, RefreshReportDto, SearchResultsDto,
    },
    input::{
        ChannelInput, GuideWindowInput, ListChannelsInput, ListGroupsInput, PlaybackActivityInput,
        PlaybackAndroidControlsInput, PlaybackAndroidIdentityInput, PlaybackAndroidStartInput,
        PlaybackAndroidViewportCommandInput, PlaybackMpvControlInput, PlaybackReadInput,
        PlaybackReopenInput, PlaybackRestartInput, PlaybackStartInput, PlaybackStopInput,
        PlaybackSuspendInput, ScheduleInput, SearchCancellationInput, SearchInput, SearchPageInput,
        SourceConfigurationInputDto,
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
pub(crate) async fn catalog_guide_window(
    slot: State<'_, InstalledRuntimeSlot>,
    input: GuideWindowInput,
) -> Result<PageDto<GuideWindowChannelDto>, ClientErrorDto> {
    let runtime = slot.wait().await;
    guide_window(runtime.as_ref(), input)
}

pub(crate) fn guide_window(
    state: &InstalledRuntime,
    input: GuideWindowInput,
) -> Result<PageDto<GuideWindowChannelDto>, ClientErrorDto> {
    let query = input.into_core()?;
    state
        .core()
        .guide_window(query)
        .map(|page| PageDto::guide_window(&page))
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

#[tauri::command]
pub(crate) async fn playback_start(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackStartInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    start_playback(runtime.as_ref(), input).await
}

pub(crate) async fn start_playback(
    state: &InstalledRuntime,
    input: PlaybackStartInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    let (channel_id, session_id) = input.into_playback()?;
    state
        .start_playback(session_id, channel_id)
        .await
        .map(PlaybackDescriptorDto::from)
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_read(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackReadInput,
) -> Result<Response, ClientErrorDto> {
    let runtime = slot.wait().await;
    read_playback(runtime.as_ref(), input).await
}

pub(crate) async fn read_playback(
    state: &InstalledRuntime,
    input: PlaybackReadInput,
) -> Result<Response, ClientErrorDto> {
    let (session_id, stream_handle) = input.into_playback()?;
    state
        .read_playback(session_id, stream_handle)
        .await
        .map(Response::new)
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_android_start(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackAndroidStartInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    start_android_playback(runtime.as_ref(), input).await
}

pub(crate) async fn start_android_playback(
    state: &InstalledRuntime,
    input: PlaybackAndroidStartInput,
) -> Result<(), ClientErrorDto> {
    let (identity, viewport, controls) = input.into_playback()?;
    state
        .start_android_playback(identity, viewport, controls)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_android_status(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackAndroidIdentityInput,
) -> Result<AndroidPlaybackStatusDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    android_playback_status(runtime.as_ref(), input).await
}

pub(crate) async fn android_playback_status(
    state: &InstalledRuntime,
    input: PlaybackAndroidIdentityInput,
) -> Result<AndroidPlaybackStatusDto, ClientErrorDto> {
    let identity = input.into_playback()?;
    state
        .android_playback_status(identity)
        .await
        .map(AndroidPlaybackStatusDto::from)
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_android_controls(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackAndroidControlsInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    set_android_playback_controls(runtime.as_ref(), input).await
}

pub(crate) async fn set_android_playback_controls(
    state: &InstalledRuntime,
    input: PlaybackAndroidControlsInput,
) -> Result<(), ClientErrorDto> {
    let (identity, controls) = input.into_playback()?;
    state
        .set_android_playback_controls(identity, controls)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_android_viewport(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackAndroidViewportCommandInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    set_android_playback_viewport(runtime.as_ref(), input).await
}

pub(crate) async fn set_android_playback_viewport(
    state: &InstalledRuntime,
    input: PlaybackAndroidViewportCommandInput,
) -> Result<(), ClientErrorDto> {
    let (identity, viewport) = input.into_playback()?;
    state
        .set_android_playback_viewport(identity, viewport)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_android_stop(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackAndroidIdentityInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    stop_android_playback(runtime.as_ref(), input).await
}

pub(crate) async fn stop_android_playback(
    state: &InstalledRuntime,
    input: PlaybackAndroidIdentityInput,
) -> Result<(), ClientErrorDto> {
    let identity = input.into_playback()?;
    state
        .stop_android_playback(identity)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_suspend(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackSuspendInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    suspend_playback(runtime.as_ref(), input).await
}

pub(crate) async fn suspend_playback(
    state: &InstalledRuntime,
    input: PlaybackSuspendInput,
) -> Result<(), ClientErrorDto> {
    state
        .suspend_playback(input.into_session_id()?)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_activity(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackActivityInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    set_playback_activity(runtime.as_ref(), input).await
}

pub(crate) async fn set_playback_activity(
    state: &InstalledRuntime,
    input: PlaybackActivityInput,
) -> Result<(), ClientErrorDto> {
    let (session_id, active) = input.into_playback()?;
    state
        .set_playback_activity(session_id, active)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_reopen(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackReopenInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    reopen_playback(runtime.as_ref(), input).await
}

pub(crate) async fn reopen_playback(
    state: &InstalledRuntime,
    input: PlaybackReopenInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    state
        .reopen_playback(input.into_session_id()?)
        .await
        .map(PlaybackDescriptorDto::from)
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_restart(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackRestartInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    let runtime = slot.wait().await;
    restart_playback(runtime.as_ref(), input).await
}

pub(crate) async fn restart_playback(
    state: &InstalledRuntime,
    input: PlaybackRestartInput,
) -> Result<PlaybackDescriptorDto, ClientErrorDto> {
    let (session_id, expected_stream_handle, intent) = input.into_playback()?;
    state
        .restart_playback(session_id, expected_stream_handle, intent)
        .await
        .map(PlaybackDescriptorDto::from)
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_stop(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackStopInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    stop_playback(runtime.as_ref(), input).await
}

pub(crate) async fn stop_playback(
    state: &InstalledRuntime,
    input: PlaybackStopInput,
) -> Result<(), ClientErrorDto> {
    let (session_id, stream_handle) = input.into_playback()?;
    state
        .stop_playback(session_id, stream_handle)
        .await
        .map_err(ClientErrorDto::from)
}

#[tauri::command]
pub(crate) async fn playback_mpv_control(
    slot: State<'_, InstalledRuntimeSlot>,
    input: PlaybackMpvControlInput,
) -> Result<(), ClientErrorDto> {
    let runtime = slot.wait().await;
    control_mpv(runtime.as_ref(), input).await
}

pub(crate) async fn control_mpv(
    state: &InstalledRuntime,
    input: PlaybackMpvControlInput,
) -> Result<(), ClientErrorDto> {
    let (session_id, control) = input.into_playback()?;
    state
        .control_mpv(session_id, control)
        .await
        .map_err(ClientErrorDto::from)
}

#[cfg(test)]
mod tests {
    use tauri::ipc::{InvokeResponseBody, IpcResponse as _};

    use super::*;

    #[test]
    fn playback_bytes_use_tauri_raw_responses_including_eof() {
        for expected in [vec![0_u8, 1, 2, 255], Vec::new()] {
            let body = Response::new(expected.clone())
                .body()
                .expect("raw IPC response is valid");
            assert!(matches!(body, InvokeResponseBody::Raw(bytes) if bytes == expected));
        }
    }
}
