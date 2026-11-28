//! Handheld Watch and Guide modes, turning the phone into fullscreen, and the
//! time chips that show the Guide at a later hour.

use chrono::{DateTime, FixedOffset, Local, Utc};
use gpui::{Context, Window};
use sparrow_tv_guide::{
    guide_screen::{GuideMode, GuideScreen, LaneFocus, ScreenFocus, ScreenKey, ToolbarFocus},
    time::next_full_local_hours,
};

use super::GuideSurface;
use crate::{
    FormFactor,
    guide_view::{PocketClass, PocketMode, PocketTool, Profile, SourceFocus},
};

/// The viewer's Pocket choices. The mode on screen is derived from them.
#[derive(Default)]
pub(super) struct PocketState {
    /// The viewer last chose the Watch screen.
    watch: bool,
    /// Turning the phone entered this fullscreen, so turning it back leaves it.
    turned: bool,
    /// The time chip's row of hours is open in place of the group pills.
    pub(super) hours_open: bool,
}

/// A sideways phone is at least this tall in logical pixels. A shorter
/// landscape window is the system's picture-in-picture window.
const TURNED_PHONE_MIN_HEIGHT: f32 = 320.;

/// A fullscreen change caused by turning the phone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Turn {
    Enter,
    Exit,
}

/// Only portrait handhelds have a Watch screen.
pub(super) fn watch_profile(form: FormFactor, profile: Profile) -> bool {
    form == FormFactor::Handheld && matches!(profile, Profile::Pocket(PocketClass::Portrait, _))
}

impl PocketState {
    /// Watch needs a portrait handheld and a selected Channel.
    pub(super) fn mode(&self, form: FormFactor, profile: Profile, selected: bool) -> PocketMode {
        if self.watch && selected && watch_profile(form, profile) {
            PocketMode::Watch
        } else {
            PocketMode::Guide
        }
    }

    /// Tuning from the Guide on a portrait handheld opens the Watch screen.
    pub(super) fn tuned(&mut self, form: FormFactor, profile: Profile) {
        if watch_profile(form, profile) {
            self.show_watch();
        }
    }

    pub(super) fn show_watch(&mut self) {
        self.watch = true;
        self.hours_open = false;
    }

    /// Back from Watch shows the Guide. Returns whether Watch was on screen.
    pub(super) fn leave_watch(&mut self, shown: PocketMode) -> bool {
        let leaving = shown == PocketMode::Watch;
        if leaving {
            self.watch = false;
        }
        leaving
    }

    /// The viewer asked for fullscreen, so turning the phone back keeps it.
    pub(super) fn chose_fullscreen(&mut self) {
        self.turned = false;
    }

    /// Turning a playing Watch screen sideways fills the screen; turning back
    /// returns only from a fullscreen that the turn entered. `height` is the
    /// new viewport's, in logical pixels.
    pub(super) fn turn(
        &mut self,
        form: FormFactor,
        (from, to): (Profile, Profile),
        height: f32,
        shown: PocketMode,
        playing: bool,
        fullscreen: bool,
    ) -> Option<Turn> {
        if form != FormFactor::Handheld {
            return None;
        }
        let class = |profile| match profile {
            Profile::Pocket(class, _) => Some(class),
            _ => None,
        };
        match (class(from)?, class(to)?) {
            (PocketClass::Portrait, PocketClass::ShortLandscape)
                if shown == PocketMode::Watch
                    && playing
                    && !fullscreen
                    && height >= TURNED_PHONE_MIN_HEIGHT =>
            {
                self.turned = true;
                Some(Turn::Enter)
            }
            (PocketClass::ShortLandscape, PocketClass::Portrait) if fullscreen && self.turned => {
                self.turned = false;
                Some(Turn::Exit)
            }
            _ => None,
        }
    }
}

/// Keeps the Pocket choices to a layout that shows their controls. Time chips
/// replace the board view switch, so Timeline returns to Now; without them the
/// chosen hour and its open row follow the clock again. Returns whether
/// anything changed.
pub(super) fn fit_time_chips(
    screen: &mut GuideScreen,
    pocket: &mut PocketState,
    time_chips: bool,
    now: DateTime<FixedOffset>,
) -> bool {
    if time_chips {
        return screen.leave_timeline(now.to_utc(), *now.offset());
    }
    let chosen = pocket.hours_open || screen.viewing_instant().is_some();
    pocket.hours_open = false;
    screen.set_viewing_instant(None, now.to_utc());
    chosen
}

/// The time chip's label: "Now", or the local hour being shown.
pub(crate) fn time_chip_label(viewing: Option<DateTime<Utc>>) -> String {
    viewing.map_or_else(|| "Now".to_owned(), crate::guide_view::clock_time)
}

impl GuideSurface {
    /// Sheets, Search and Setup always open over the Guide.
    pub(super) fn pocket_mode(&self) -> PocketMode {
        let covered = self.setup.is_some()
            || self.source.is_some()
            || self.screen.group_chooser().is_some()
            || self.search.state().view() != sparrow_tv_guide::search::SearchView::Closed;
        self.pocket.mode(
            self.host.form_factor(),
            self.profile,
            self.playback.selected() && !covered,
        )
    }

    /// The hours the open time chip offers after Now.
    pub(super) fn chip_hours(&self) -> Vec<DateTime<Utc>> {
        next_full_local_hours(self.now.to_utc(), &Local)
    }

    /// Whether the lane's first control picks a time on this layout.
    pub(super) fn time_chips_shown(&self, window: &Window) -> bool {
        self.screen.mode() == GuideMode::Now && self.layout(window).time_chips()
    }

    /// The open time chip's row of hours is on screen, under no fullscreen picture.
    pub(super) fn hours_shown(&self, window: &Window) -> bool {
        self.pocket.hours_open && !self.playback.fullscreen() && self.time_chips_shown(window)
    }

    pub(super) fn toggle_hours(&mut self, cx: &mut Context<Self>) {
        self.pocket.hours_open = !self.pocket.hours_open;
        cx.notify();
    }

    /// Shows every row at `hour`, reading the eight-hour window first. `None`
    /// follows the clock again.
    pub(super) fn choose_hour(&mut self, hour: Option<DateTime<Utc>>, cx: &mut Context<Self>) {
        let now = self.now;
        if hour.is_some() {
            self.screen
                .enable_extended_window(now.to_utc(), *now.offset());
        }
        self.screen.set_viewing_instant(hour, now.to_utc());
        self.pocket.hours_open = false;
        self.sync_view(cx);
    }

    pub(super) fn pocket_tool(
        &mut self,
        tool: PocketTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = match tool {
            PocketTool::Search => ScreenFocus::Toolbar(ToolbarFocus::Search),
            PocketTool::Sources | PocketTool::Appearance => {
                ScreenFocus::Toolbar(ToolbarFocus::Sources)
            }
            PocketTool::Groups => ScreenFocus::GroupLane(LaneFocus::ChooseGroups),
        };
        self.clear_dock_focus();
        if self.screen.set_focus(focus) {
            self.press(ScreenKey::Activate, false, window, cx);
        }
        if tool == PocketTool::Appearance {
            self.focus_source(SourceFocus::Backdrop);
        }
    }

    /// Opens Search from the Watch masthead over the Guide.
    pub(super) fn search_from_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pocket.leave_watch(PocketMode::Watch);
        self.pocket_tool(PocketTool::Search, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::{px, size};

    use super::*;
    use crate::guide_view::{Layout, Pointer};

    const PORTRAIT: Profile = Profile::Pocket(PocketClass::Portrait, Pointer::Coarse);
    const LANDSCAPE: Profile = Profile::Pocket(PocketClass::ShortLandscape, Pointer::Coarse);
    const TABLET: Profile = Profile::Pocket(PocketClass::Tablet, Pointer::Coarse);
    const HANDHELD: FormFactor = FormFactor::Handheld;
    /// A sideways 412 by 915 phone.
    const PHONE_HEIGHT: f32 = 412.;

    #[test]
    fn tuning_on_a_portrait_phone_opens_watch_and_back_returns_to_the_guide() {
        let mut pocket = PocketState::default();
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, true), PocketMode::Guide);

        pocket.tuned(HANDHELD, PORTRAIT);
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, true), PocketMode::Watch);
        // Nothing playing always shows the Guide, and other classes have no Watch screen.
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, false), PocketMode::Guide);
        assert_eq!(pocket.mode(HANDHELD, LANDSCAPE, true), PocketMode::Guide);
        assert_eq!(pocket.mode(HANDHELD, TABLET, true), PocketMode::Guide);

        assert!(pocket.leave_watch(pocket.mode(HANDHELD, PORTRAIT, true)));
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, true), PocketMode::Guide);
        // Back at the Guide root falls through to the host, which enters PiP.
        assert!(!pocket.leave_watch(pocket.mode(HANDHELD, PORTRAIT, true)));

        pocket.show_watch();
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, true), PocketMode::Watch);
    }

    #[test]
    fn desktop_and_tablet_tunes_stay_in_the_guide() {
        let mut pocket = PocketState::default();
        let small_window = Profile::Pocket(PocketClass::Portrait, Pointer::Fine);
        pocket.tuned(FormFactor::Desktop, small_window);
        pocket.tuned(HANDHELD, TABLET);
        assert_eq!(pocket.mode(HANDHELD, PORTRAIT, true), PocketMode::Guide);
        pocket.show_watch();
        assert_eq!(
            pocket.mode(FormFactor::Desktop, small_window, true),
            PocketMode::Guide
        );
    }

    #[test]
    fn turning_a_playing_watch_screen_enters_and_leaves_fullscreen() {
        let mut pocket = PocketState::default();
        let watch = PocketMode::Watch;
        assert_eq!(
            pocket.turn(
                HANDHELD,
                (PORTRAIT, LANDSCAPE),
                PHONE_HEIGHT,
                watch,
                true,
                false
            ),
            Some(Turn::Enter)
        );
        assert_eq!(
            pocket.turn(
                HANDHELD,
                (LANDSCAPE, PORTRAIT),
                915.,
                PocketMode::Guide,
                true,
                true
            ),
            Some(Turn::Exit)
        );
        // Without the turn, turning back keeps an explicit fullscreen.
        assert_eq!(
            pocket.turn(
                HANDHELD,
                (LANDSCAPE, PORTRAIT),
                915.,
                PocketMode::Guide,
                true,
                true
            ),
            None
        );
    }

    #[test]
    fn turning_only_relays_out_in_the_guide_while_paused_or_after_a_chosen_fullscreen() {
        let mut pocket = PocketState::default();
        let turn = |pocket: &mut PocketState, shown, playing, fullscreen| {
            pocket.turn(
                HANDHELD,
                (PORTRAIT, LANDSCAPE),
                PHONE_HEIGHT,
                shown,
                playing,
                fullscreen,
            )
        };
        assert_eq!(turn(&mut pocket, PocketMode::Guide, true, false), None);
        assert_eq!(turn(&mut pocket, PocketMode::Watch, false, false), None);
        assert_eq!(turn(&mut pocket, PocketMode::Watch, true, true), None);
        assert_eq!(
            pocket.turn(
                FormFactor::Desktop,
                (PORTRAIT, LANDSCAPE),
                PHONE_HEIGHT,
                PocketMode::Watch,
                true,
                false
            ),
            None
        );

        assert_eq!(
            turn(&mut pocket, PocketMode::Watch, true, false),
            Some(Turn::Enter)
        );
        pocket.chose_fullscreen();
        assert_eq!(
            pocket.turn(
                HANDHELD,
                (LANDSCAPE, PORTRAIT),
                915.,
                PocketMode::Guide,
                true,
                true
            ),
            None
        );
    }

    #[test]
    fn a_playing_watch_screen_shrinking_into_picture_in_picture_stays_out_of_fullscreen() {
        let mut pocket = PocketState::default();
        let pip = 135.;
        assert_eq!(
            pocket.turn(
                HANDHELD,
                (PORTRAIT, LANDSCAPE),
                pip,
                PocketMode::Watch,
                true,
                false
            ),
            None
        );
    }

    #[test]
    fn layouts_without_their_control_drop_the_time_chip_choices() {
        let now = Local::now().fixed_offset();
        let mut screen = GuideScreen::new(now.to_utc(), *now.offset(), Default::default());
        let mut pocket = PocketState::default();
        let tablet = |width: f32, height: f32| {
            let profile = Profile::select(HANDHELD, (width, height), None);
            Layout::new(profile, size(px(width), px(height))).time_chips()
        };
        let (landscape, portrait) = (tablet(1280., 800.), tablet(800., 1280.));

        // A landscape tablet shows Timeline; turned to portrait, it shows Now.
        assert!(screen.set_focus(ScreenFocus::GroupLane(LaneFocus::Phase)));
        screen.press(ScreenKey::Activate, now.to_utc(), *now.offset());
        assert_eq!(screen.mode(), GuideMode::Timeline);
        assert!(!fit_time_chips(&mut screen, &mut pocket, landscape, now));
        assert_eq!(screen.mode(), GuideMode::Timeline);
        assert!(fit_time_chips(&mut screen, &mut pocket, portrait, now));
        assert_eq!(screen.mode(), GuideMode::Now);

        // The hour chosen in portrait, and its open row, end in landscape.
        let later = now.to_utc() + chrono::TimeDelta::hours(2);
        screen.set_viewing_instant(Some(later), now.to_utc());
        pocket.hours_open = true;
        assert!(!fit_time_chips(&mut screen, &mut pocket, portrait, now));
        assert_eq!(screen.viewing_instant(), Some(later));
        assert!(fit_time_chips(&mut screen, &mut pocket, landscape, now));
        assert_eq!(screen.viewing_instant(), None);
        assert!(!pocket.hours_open);
    }

    #[test]
    fn the_time_chip_names_the_chosen_hour() {
        assert_eq!(time_chip_label(None), "Now");
        let hour = Local::now().to_utc();
        assert_eq!(
            time_chip_label(Some(hour)),
            hour.with_timezone(&Local).format("%H:%M").to_string()
        );
    }
}
