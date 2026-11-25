//! State and navigation for the Sparrow TV Programme Guide.

mod board_focus;
mod families;
pub mod group_chooser;
pub mod guide_screen;
pub mod now_focus;
mod pages;
pub mod preview;
pub mod search;
pub mod setup;
pub mod time;
pub mod timeline_focus;
mod viewport;

pub use board_focus::BoardView;

pub use families::{GuideFamily, VariantPreferences, normalize_preference_title};
pub use pages::{
    ChannelGroupPages, GroupPageRequest, GuidePageRequest, GuidePages, PageStatus, PageUpdate,
};
pub use time::GuideWindow;
