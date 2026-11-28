//! Pointer and Desktop keyboard input. One root listener hit-tests the same
//! geometry the views draw, then takes the D-pad path: a click focuses its
//! target without revealing focus, then activates it like Enter.

use gpui::{
    Context, CursorStyle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, Window, px,
};
use sparrow_core::ChannelDirection;
use sparrow_tv_guide::{
    group_chooser::{GroupChooserFocus, GroupChooserIntent},
    guide_screen::{GuideMode, ScreenAction, ScreenFocus, ScreenKey, ToolbarFocus},
    now_focus::{Direction, NowFocus},
    search::SearchView,
    setup::SetupControl,
    timeline_focus::TimelineFocus,
};

use super::{GuideSurface, TextTarget};
use crate::{
    FormFactor,
    guide_view::{self, Layout, SourceFocus},
    search_flow::SearchFocus,
};

/// Logical pixels per wheel line.
const LINE: f32 = 44.;
/// Pointer travel, in logical pixels, that counts as moving over the picture.
const REVEAL_TRAVEL: f32 = 3.;

/// Something a pointer press can act on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum HitTarget {
    Screen(ScreenFocus),
    /// The docked picture and its caption.
    Dock,
    /// The fullscreen picture.
    Picture,
    /// The fullscreen band. Its controls take presses themselves.
    Band,
    Chooser(GroupChooserFocus),
    Source(SourceFocus),
    Setup(SetupControl),
    Search(SearchFocus),
    /// Outside an open sheet; a press closes the sheet.
    Scrim,
}

#[derive(Default)]
pub(super) struct PointerState {
    pub(super) hover: Option<HitTarget>,
    position: Option<Point<Pixels>>,
    /// Wheel travel, in layout units, not yet spent on whole rows or half hours.
    rows: f32,
    pan: f32,
}

/// Whether the window, rather than the Guide, went fullscreen with the picture.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum WindowFullscreen {
    #[default]
    Guide,
    Picture {
        ours: bool,
    },
}

impl GuideSurface {
    fn pointer_host(&self) -> bool {
        self.host.form_factor() != FormFactor::Television
    }

    pub(super) fn pointer_holds_band(&self) -> bool {
        self.pointer.hover == Some(HitTarget::Band)
    }

    pub(super) fn cursor(&self) -> CursorStyle {
        match self.pointer.hover {
            Some(
                HitTarget::Search(SearchFocus::Field)
                | HitTarget::Chooser(GroupChooserFocus::Filter)
                | HitTarget::Source(SourceFocus::M3u | SourceFocus::Epg)
                | HitTarget::Setup(SetupControl::Field),
            ) => CursorStyle::IBeam,
            None | Some(HitTarget::Picture | HitTarget::Band | HitTarget::Scrim) => {
                CursorStyle::Arrow
            }
            Some(_) => CursorStyle::PointingHand,
        }
    }

    /// The Guide control under a window position.
    fn hit_test(&mut self, window: &Window, position: Point<Pixels>) -> Option<HitTarget> {
        let layout = self.layout(window);
        let point = [
            f32::from(position.x) / layout.unit,
            f32::from(position.y) / layout.unit,
        ];
        if let Some(setup) = &self.setup {
            let panel = guide_view::SetupPanel {
                setup,
                draft: self.setup_draft(),
                editing: self.source_editor_open(),
                saving: self.setup_saving(),
                now: self.now,
            };
            return guide_view::setup_hit(layout, window, &panel, point).map(HitTarget::Setup);
        }
        self.activation?;
        if self.playback.fullscreen() {
            let band = self.playback.controls_visible()
                && point[1] >= guide_view::playback::band_top(layout);
            return Some(if band {
                HitTarget::Band
            } else {
                HitTarget::Picture
            });
        }
        let sheet_left = layout.width - layout.sheet_width();
        if self.sources_sheet_open() {
            return Some(
                self.source_sheet_target(layout, point)
                    .map_or(HitTarget::Scrim, HitTarget::Source),
            )
            .filter(|target| *target != HitTarget::Scrim || point[0] < sheet_left);
        }
        if self.screen.group_chooser().is_some() {
            return Some(
                guide_view::group_sheet_hit(layout, &self.screen, &self.group_viewport, point)
                    .map_or(HitTarget::Scrim, HitTarget::Chooser),
            )
            .filter(|target| *target != HitTarget::Scrim || point[0] < sheet_left);
        }
        if self.search.state().view() != SearchView::Closed {
            let panel = self.search_panel_inputs(None);
            return guide_view::search_hit(layout, &panel, &self.search_viewport, point)
                .map(HitTarget::Search);
        }
        let docked = self.playback.docked() || self.playback.fullscreen();
        if self.playback.docked() {
            let [x, y, w, h] = layout.m.dock;
            if (x..x + w).contains(&point[0]) && (y - 5. ..y + h + 44.).contains(&point[1]) {
                return Some(HitTarget::Dock);
            }
        }
        if let Some(toolbar) = guide_view::band::toolbar_hit(layout, window, docked, point) {
            return Some(HitTarget::Screen(ScreenFocus::Toolbar(toolbar)));
        }
        let lane = match self.screen.mode() {
            GuideMode::Now => {
                guide_view::lane::lane_hit(layout, &self.screen, &mut self.now_lane, window, point)
            }
            GuideMode::Timeline => {
                guide_view::timeline::rail_hit(layout, &self.screen, &self.timeline_rail, point)
            }
        };
        if let Some(lane) = lane {
            return Some(HitTarget::Screen(ScreenFocus::GroupLane(lane)));
        }
        let board = match self.screen.mode() {
            GuideMode::Now => self.now_hit(layout, point).map(ScreenFocus::Board),
            GuideMode::Timeline => guide_view::timeline::timeline_hit(layout, &self.screen, point)
                .map(ScreenFocus::TimelineBoard),
        };
        board.map(HitTarget::Screen)
    }

    fn now_hit(&self, layout: Layout, point: [f32; 2]) -> Option<NowFocus> {
        if point[1] < layout.m.board.now.top {
            return None;
        }
        let scroll = self.screen.scroll_offset();
        self.screen.visible_families(0).find_map(|index| {
            let card = guide_view::board::card_bounds(layout, index, scroll);
            let [x, y, w, h] = card;
            if !(x..x + w).contains(&point[0]) || !(y..y + h).contains(&point[1]) {
                return None;
            }
            let family = &self.screen.families()[index];
            Some(guide_view::board::card_chip(family, card, point).map_or(
                NowFocus::Channel(index),
                |chip| NowFocus::Variant {
                    family: index,
                    chip,
                },
            ))
        })
    }

    pub(super) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moved = self.pointer.position.is_none_or(|last| {
            let delta = event.position - last;
            f32::from(delta.x).abs() + f32::from(delta.y).abs() > REVEAL_TRAVEL
        });
        if moved {
            self.pointer.position = Some(event.position);
            if self.playback.fullscreen() {
                self.reveal_player_controls(cx);
            }
        }
        let hover = self.hit_test(window, event.position);
        if hover != self.pointer.hover {
            self.pointer.hover = hover;
            cx.notify();
        }
    }

    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        self.cancel_arrival(cx);
        let target = self.hit_test(window, event.position);
        let editing = match self.active_editor.map(|(_, target)| target) {
            Some(TextTarget::Search) => Some(HitTarget::Search(SearchFocus::Field)),
            Some(TextTarget::GroupFilter) => Some(HitTarget::Chooser(GroupChooserFocus::Filter)),
            None if self.source_editor_open() => target.filter(|target| {
                matches!(
                    target,
                    HitTarget::Setup(SetupControl::Field)
                        | HitTarget::Source(SourceFocus::M3u | SourceFocus::Epg)
                )
            }),
            None => None,
        };
        if editing.is_some() && editing == target {
            return;
        }
        // A press anywhere else drops the open editor without committing it.
        self.retire_query_editor();
        if self.source_editor_open() {
            self.host.cancel_editor();
        }
        let Some(target) = target else {
            cx.notify();
            return;
        };
        let double = event.click_count >= 2;
        match target {
            HitTarget::Setup(control) => {
                let draft = self.setup_draft();
                if self
                    .setup
                    .as_mut()
                    .is_some_and(|setup| setup.set_focus(control, draft))
                {
                    self.press(ScreenKey::Activate, false, window, cx);
                }
            }
            HitTarget::Source(focus) => {
                self.focus_source(focus);
                self.press(ScreenKey::Activate, false, window, cx);
            }
            HitTarget::Chooser(focus) => {
                self.group_viewport.follow_focus();
                let action = self
                    .screen
                    .group_chooser_intent(GroupChooserIntent::Activate(focus));
                self.chooser_action(action, window, cx);
            }
            HitTarget::Search(focus) => {
                self.search_viewport.follow_focus();
                let effect = self.search.activate(focus);
                self.search_effect(effect, Some((window, false)), cx);
            }
            HitTarget::Scrim => {
                self.close_top_layer(window, cx);
            }
            HitTarget::Dock => {
                self.playback_set_fullscreen(true);
            }
            HitTarget::Picture if double => {
                self.playback_set_fullscreen(false);
            }
            HitTarget::Picture | HitTarget::Band => self.reveal_player_controls(cx),
            HitTarget::Screen(focus) => {
                self.clear_dock_focus();
                if self.screen.set_focus(focus) {
                    self.press(ScreenKey::Activate, false, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn chooser_action(&mut self, action: ScreenAction, window: &Window, cx: &mut Context<Self>) {
        let feedback = match action {
            ScreenAction::GroupVisibilityChanged(change) => self.save_group_visibility(change, cx),
            ScreenAction::EditGroupFilter => {
                (!self.open_group_filter(window)).then_some("Couldn’t open the keyboard.")
            }
            ScreenAction::None | ScreenAction::Unavailable => None,
        };
        if self.screen.group_chooser().is_none() {
            self.retire_query_editor();
        }
        self.show_feedback(feedback, cx);
        self.sync_view(cx);
    }

    pub(super) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = self.layout(window);
        let delta = event.delta.pixel_delta(px(LINE));
        let (mut dx, mut dy) = (
            -f32::from(delta.x) / layout.unit,
            -f32::from(delta.y) / layout.unit,
        );
        if event.modifiers.shift && dx == 0. {
            (dx, dy) = (dy, 0.);
        }
        let target = self.hit_test(window, event.position);
        let point_y = f32::from(event.position.y) / layout.unit;
        let mut moved = false;
        match target {
            Some(HitTarget::Picture | HitTarget::Band) => {
                // Only wheel notches change the volume, so trackpad noise cannot.
                if let ScrollDelta::Lines(lines) = event.delta
                    && lines.y != 0.
                {
                    self.set_volume_step(0.05 * lines.y.signum());
                }
                self.reveal_player_controls(cx);
            }
            Some(HitTarget::Chooser(_)) => {
                if let Some(rows) = self.wheel_rows(dy, 64.) {
                    self.group_viewport.scroll(rows);
                    moved = true;
                }
            }
            Some(HitTarget::Search(_)) => {
                let view = self.search.state().view();
                let pitch = if view == SearchView::Desk { 82. } else { 58. };
                if let Some(rows) = self.wheel_rows(dy, pitch) {
                    self.search_viewport.scroll(view, rows);
                    moved = true;
                }
            }
            Some(HitTarget::Screen(ScreenFocus::GroupLane(_))) => match self.screen.mode() {
                GuideMode::Now => {
                    let travel = if dx != 0. { dx } else { dy };
                    guide_view::lane::scroll_lane(
                        layout,
                        &self.screen,
                        &mut self.now_lane,
                        window,
                        travel,
                    );
                    moved = true;
                }
                GuideMode::Timeline => {
                    if let Some(rows) = self.wheel_rows(dy, 48.) {
                        self.timeline_rail.scroll(layout, &self.screen, rows);
                        moved = true;
                    }
                }
            },
            Some(HitTarget::Screen(ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_)))
            | None
                if self.guide_shown()
                    && point_y >= layout.m.board.board(self.screen.mode()).top =>
            {
                moved = self.screen.scroll_by(dy);
                if self.screen.mode() == GuideMode::Timeline && dx != 0. {
                    self.pointer.pan += dx;
                    let half_hour = guide_view::timeline::track_width(layout) / 4.;
                    let steps = (self.pointer.pan / half_hour).trunc();
                    if steps != 0. {
                        self.pointer.pan -= steps * half_hour;
                        moved |= self.screen.pan_timeline(steps as i64);
                    }
                }
            }
            _ => {}
        }
        if moved {
            self.pointer.hover = self.hit_test(window, event.position);
            self.sync_view(cx);
        }
    }

    /// The Guide is on screen with no sheet, search or Setup over it.
    fn guide_shown(&self) -> bool {
        self.setup.is_none()
            && !self.playback.fullscreen()
            && !self.sources_sheet_open()
            && self.screen.group_chooser().is_none()
            && self.search.state().view() == SearchView::Closed
    }

    /// Spends wheel travel on whole rows of `pitch` units.
    fn wheel_rows(&mut self, travel: f32, pitch: f32) -> Option<isize> {
        self.pointer.rows += travel;
        let rows = (self.pointer.rows / pitch).trunc();
        self.pointer.rows -= rows * pitch;
        (rows != 0.).then_some(rows as isize)
    }

    /// Desktop keyboard shortcuts. Returns whether the key was taken.
    pub(super) fn shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.pointer_host() || self.activation.is_none() || self.setup.is_some() {
            return false;
        }
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        let control_key = matches!(key, "f" | "up" | "down");
        if modifiers.alt || modifiers.platform || (modifiers.control && !control_key) {
            return false;
        }
        let sheet = self.sources_sheet_open()
            || self.screen.group_chooser().is_some()
            || self.search.state().view() != SearchView::Closed;
        match key {
            "escape" if !event.is_held => {
                self.close_top_layer(window, cx);
            }
            "/" | "f" if key == "/" || modifiers.control => {
                self.playback_set_fullscreen(false);
                if !sheet
                    && self
                        .screen
                        .set_focus(ScreenFocus::Toolbar(ToolbarFocus::Search))
                {
                    self.press(ScreenKey::Activate, false, window, cx);
                }
            }
            "f" if !event.is_held && !sheet => {
                let fullscreen = !self.playback.fullscreen();
                self.playback_set_fullscreen(fullscreen);
            }
            "g" if !event.is_held => {
                if !self.playback_set_fullscreen(false) && sheet {
                    self.close_top_layer(window, cx);
                }
            }
            "space" if !event.is_held && !sheet => self.toggle_pause(),
            "m" if !event.is_held && !sheet => self.toggle_mute(),
            "-" | "=" | "+" if !sheet => {
                self.set_volume_step(if key == "-" { -0.05 } else { 0.05 });
                self.reveal_player_controls(cx);
            }
            "pageup" | "pagedown" if !sheet && !self.playback.fullscreen() => {
                let metrics = self.screen.board_metrics();
                let geometry = metrics.board(self.screen.mode());
                let rows = (metrics.surface_height - geometry.top) / geometry.pitch;
                let direction = if key == "pageup" {
                    Direction::Up
                } else {
                    Direction::Down
                };
                for _ in 0..(rows.floor() as usize).max(1) {
                    self.press(ScreenKey::Arrow(direction), false, window, cx);
                }
            }
            "home" | "end" if !sheet && !self.playback.fullscreen() => {
                let last = self.screen.families().len().checked_sub(1);
                let family = if key == "home" { Some(0) } else { last };
                let focus = family.map(|family| match self.screen.mode() {
                    GuideMode::Now => ScreenFocus::Board(NowFocus::Channel(family)),
                    GuideMode::Timeline => {
                        ScreenFocus::TimelineBoard(TimelineFocus::Channel(family))
                    }
                });
                if let Some(focus) = focus
                    && self.screen.set_focus(focus)
                {
                    self.screen
                        .scroll_by(if key == "home" { f32::MIN } else { f32::MAX });
                    self.sync_view(cx);
                }
            }
            "tab" if !sheet && !self.playback.fullscreen() => self.next_region(modifiers.shift, cx),
            "up" | "down" if modifiers.control && !self.playback.fullscreen() => {
                let direction = if key == "up" {
                    ChannelDirection::Previous
                } else {
                    ChannelDirection::Next
                };
                self.player_zap(direction, cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Tab moves between the toolbar, the lane and the board.
    fn next_region(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let lane = ScreenFocus::GroupLane(self.screen.selected_lane());
        let first = self.screen.visible_families(0).start;
        let board = match self.screen.mode() {
            GuideMode::Now => ScreenFocus::Board(NowFocus::Channel(first)),
            GuideMode::Timeline => ScreenFocus::TimelineBoard(TimelineFocus::Channel(first)),
        };
        let toolbar = ScreenFocus::Toolbar(ToolbarFocus::Search);
        let regions = [toolbar, lane, board];
        let current = match self.screen.focus() {
            ScreenFocus::Toolbar(_) => 0,
            ScreenFocus::GroupLane(_) => 1,
            ScreenFocus::None | ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_) => 2,
        };
        for step in 1..regions.len() {
            let offset = if backwards {
                regions.len() - step
            } else {
                step
            };
            if self
                .screen
                .set_focus(regions[(current + offset) % regions.len()])
            {
                break;
            }
        }
        self.sync_view(cx);
    }

    /// Closes the topmost menu, sheet, search or fullscreen picture. Returns
    /// whether there was one; Back with none open leaves the Guide.
    pub(super) fn close_top_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.source.is_some() {
            self.close_sources(cx);
        } else if self.search.state().view() != SearchView::Closed {
            let effect = self.search.back();
            self.search_effect(effect, Some((window, false)), cx);
        } else if self.screen.group_chooser().is_some() {
            self.screen.group_chooser_intent(GroupChooserIntent::Close);
            self.retire_query_editor();
            self.sync_view(cx);
        } else {
            return self.playback_back(cx);
        }
        true
    }

    /// Keeps the window's fullscreen in step with the picture's on Desktop.
    /// A compositor that leaves fullscreen on its own keeps the picture fullscreen.
    pub(super) fn sync_window_fullscreen(&mut self, window: &mut Window) {
        if !self.pointer_host() {
            return;
        }
        let picture = self.playback.fullscreen();
        match (self.window_fullscreen, picture) {
            (WindowFullscreen::Guide, true) => {
                let ours = !window.is_fullscreen();
                if ours {
                    window.toggle_fullscreen();
                }
                self.window_fullscreen = WindowFullscreen::Picture { ours };
            }
            (WindowFullscreen::Picture { ours }, false) => {
                if ours && window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                self.window_fullscreen = WindowFullscreen::Guide;
            }
            _ => {}
        }
    }
}
