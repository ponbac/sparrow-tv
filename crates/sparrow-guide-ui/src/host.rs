use std::{sync::Arc, time::Duration};

use gpui::{App, Div, KeyDownEvent, RenderImage, Window};
use sparrow_core::ChannelDirection;
use sparrow_installed_catalog::{InstalledCatalog, SavedSourceLocations};
use sparrow_installed_playback::tv::TvPlaybackRunner;
use sparrow_tv_guide::{now_focus::Direction, setup::SetupField};

use crate::FormFactor;

/// One guide window's native text editor and private Source Configuration draft.
/// Results come back as HostEvents after the request returns.
pub trait TextEditorHost {
    fn open_query(&self, generation: u64, rect: [i32; 4], initial: &str) -> bool;
    fn close_editor(&self, generation: u64);
    fn begin_draft(&self, session: u64, seed: Option<SavedSourceLocations>) -> bool;
    fn close_draft(&self, session: u64);
    fn edit_field(&self, session: u64, generation: u64, field: SetupField, rect: [i32; 4]) -> bool;
    fn clear_field(&self, session: u64, generation: u64, field: SetupField) -> bool;
    fn submit_draft(&self, session: u64, attempt: u64) -> bool;
    fn finish_save(&self, session: u64, attempt: u64, saved: bool);

    /// Drops the open editor as Cancelled, as a press outside it does.
    fn cancel_editor(&self) {}

    fn editor_key(&self, _event: &KeyDownEvent, _cx: &mut App) -> bool {
        false
    }

    fn editor_overlay(&self, _window: &Window) -> Option<Div> {
        None
    }
}

/// Window integration for the shared guide.
pub trait GuideHost: TextEditorHost {
    fn id(&self) -> u64;
    /// Selects the Guide's layout profile and input model.
    fn form_factor(&self) -> FormFactor;
    fn guide_painted(&self) {}
    fn finish(&self, window: &mut Window);
    fn copy_diagnostics(&self, target: ClipboardTarget, text: &str, cx: &mut App) -> bool;
}

/// Playback integration shared by the catalog worker and guide windows.
/// The runner supplies commands and snapshots. These hooks cover presentation
/// outside GPUI. Embedded playback can deliver LivePicture events instead.
pub trait PlaybackHost: Send + Sync {
    fn start(&self, catalog: &InstalledCatalog) -> Option<TvPlaybackRunner>;
    fn host_active(&self, _id: u64, _active: bool) {}
    fn host_destroyed(&self, _id: u64) {}
    fn set_picture_bounds(
        &self,
        _id: u64,
        _docked: Option<[i32; 4]>,
        _fullscreen: Option<[i32; 4]>,
    ) {
    }
    fn set_ambient_sampling(&self, _id: u64, _epoch: u64) {}
    fn capture_flight_still(&self, _id: u64, _request: u64, _size: [u32; 2]) -> bool {
        false
    }
    fn picture_reveal_duration(&self) -> Duration {
        Duration::ZERO
    }
}

pub enum HostEvent {
    Presented,
    Back,
    Key {
        key: HostKey,
        held: bool,
    },
    Edited {
        generation: u64,
        edit: Edit,
    },
    Source {
        session: u64,
        update: SourceEvent,
    },
    PlaybackDiagnosticsCopied {
        attempt: u64,
        copied: bool,
    },
    PictureBottom(i32),
    Ambient {
        epoch: u64,
        colors: [u32; 3],
    },
    FlightStill(u64, Option<image::RgbaImage>),
    /// Latest decoded picture for an embedded host. Clear it when the session
    /// changes or video is unavailable.
    LivePicture(Option<Arc<RenderImage>>),
}

#[derive(Clone, Copy)]
pub enum HostKey {
    Other,
    Arrow(Direction),
    Activate,
    Channel(ChannelDirection),
    TogglePause,
    Stop,
    Resume,
    Pause,
}

pub enum Edit {
    Changed(String),
    Committed(String),
    Cancelled,
    Rejected,
}

pub enum ClipboardTarget {
    Source { session: u64, attempt: u64 },
    Playback { attempt: u64 },
}

pub enum DraftStatus {
    Opened,
    OpenRejected,
    SaveRejected { attempt: u64 },
}

#[derive(Clone, Copy)]
pub enum FieldEdit {
    Committed,
    Cancelled,
    Rejected,
    Cleared,
}

pub enum SourceEvent {
    Copied {
        attempt: u64,
        copied: bool,
    },
    Draft(DraftStatus),
    Field {
        generation: u64,
        field: SetupField,
        edit: FieldEdit,
        has_text: bool,
    },
    Submit {
        attempt: u64,
        m3u: String,
        epg: Option<String>,
    },
}
