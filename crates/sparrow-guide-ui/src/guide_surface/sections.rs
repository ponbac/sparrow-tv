//! Each section is laid out in Guide coordinates. Its stable origin lets GPUI
//! replay the previous scene while independently painted decorations move.
use super::*;
use gpui::{Entity, StyleRefinement, WeakEntity};
use guide_view::{Layout, SlideFrame, arrival::Wave};

#[derive(Clone, Copy)]
enum Section {
    Band,
    Lane,
    Board,
}

struct SectionView {
    owner: WeakEntity<GuideSurface>,
    section: Section,
}

impl Render for SectionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let section = self
            .owner
            .update(cx, |owner, cx| {
                let frame = owner
                    .sections
                    .frame
                    .expect("Guide frame set before children render");
                let layout = frame.layout;
                let wave = frame.wave;
                let slide = frame.slide;
                let minute = frame.minute.frame(owner.minute_tick.as_ref());
                let marker = owner
                    .sections
                    .board
                    .key
                    .as_ref()
                    .and_then(|key| key.marker.as_ref());
                let section = match self.section {
                    Section::Band => guide_view::band::render(
                        layout,
                        guide_view::GuideBand {
                            minute,
                            preview: owner
                                .sections
                                .band
                                .key
                                .as_ref()
                                .and_then(|key| key.preview.as_ref()),
                            description: owner.description.as_ref(),
                            docked: frame.docked,
                            inline_search: frame.inline_search,
                            focus: frame.band_focus,
                            hover: match frame.hover {
                                ScreenFocus::Toolbar(toolbar) => Some(toolbar),
                                _ => None,
                            },
                        },
                        &owner.icons,
                        wave,
                    ),
                    Section::Lane => match owner.screen.mode() {
                        GuideMode::Now => guide_view::lane::render(
                            layout,
                            &owner.screen,
                            &mut owner.now_lane,
                            window,
                            (slide, wave),
                            frame.lane_hover(),
                        ),
                        GuideMode::Timeline => guide_view::timeline::rail(
                            layout,
                            &owner.screen,
                            &mut owner.timeline_rail,
                            slide,
                            frame.lane_hover(),
                        ),
                    },
                    Section::Board => match owner.screen.mode() {
                        GuideMode::Now => guide_view::board::render(
                            layout,
                            &owner.screen,
                            minute,
                            &owner.icons,
                            marker,
                            (slide, wave),
                            match frame.hover {
                                ScreenFocus::Board(focus) => Some(focus),
                                _ => None,
                            },
                        ),
                        GuideMode::Timeline => guide_view::timeline::render(
                            layout,
                            &owner.screen,
                            minute,
                            &owner.icons,
                            marker,
                            slide,
                            match frame.hover {
                                ScreenFocus::TimelineBoard(focus) => Some(focus),
                                _ => None,
                            },
                        ),
                    },
                };
                if matches!(self.section, Section::Board) {
                    owner.arm_badge_timer(cx);
                }
                section
            })
            .unwrap();
        div().relative().size_full().child(section)
    }
}

struct Cached<K> {
    name: &'static str,
    view: Entity<SectionView>,
    key: Option<K>,
    revision: usize,
}

impl<K: PartialEq> Cached<K> {
    fn new(section: Section, cx: &mut Context<GuideSurface>) -> Self {
        let owner = cx.weak_entity();
        Self {
            name: match section {
                Section::Band => "band",
                Section::Lane => "lane",
                Section::Board => "board",
            },
            view: cx.new(|_| SectionView { owner, section }),
            key: None,
            revision: 0,
        }
    }

    fn element(
        &mut self,
        key: K,
        moving: bool,
        viewport: gpui::Size<gpui::Pixels>,
    ) -> impl IntoElement {
        // A new element id discards the old scene during this very draw.
        // notify() during render would take effect on a later frame.
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.revision = self.revision.wrapping_add(1);
        }
        let layer = div().id((self.name, self.revision)).absolute().inset_0();
        if moving {
            layer.child(self.view.clone())
        } else {
            layer.child(
                self.view.clone().cached(
                    StyleRefinement::default()
                        .w(viewport.width)
                        .h(viewport.height),
                ),
            )
        }
    }
}

#[derive(PartialEq)]
struct BandInputs {
    toolbar: Option<ToolbarFocus>,
    hover: ScreenFocus,
    docked: bool,
    inline_search: bool,
    translucent: bool,
    preview: Option<guide_view::PreviewSelection>,
    description: Option<SharedString>,
    now: i64,
    icons: u64,
}

#[derive(PartialEq)]
struct LaneInputs {
    mode: GuideMode,
    selected: LaneFocus,
    focus: Option<LaneFocus>,
    hover: Option<LaneFocus>,
    translucent: bool,
    catalog: u64,
}

#[derive(PartialEq)]
struct BoardInputs {
    marker: Option<guide_view::playing_marker::PlayingMarker>,
    mode: GuideMode,
    focus: ScreenFocus,
    hover: ScreenFocus,
    selected: LaneFocus,
    scroll: f32,
    translucent: bool,
    catalog: u64,
    page: Option<PageStatus>,
    notice: Option<sparrow_tv_guide::guide_screen::ScreenNotice>,
    now: i64,
    axis: i64,
    icons: u64,
}

#[derive(Clone, Copy)]
pub(super) struct GuideFrame {
    pub viewport: gpui::Size<gpui::Pixels>,
    pub layout: Layout,
    pub minute: guide_view::MinuteSample,
    pub wave: Wave,
    pub slide: SlideFrame,
    pub docked: bool,
    pub inline_search: bool,
    pub band_focus: ScreenFocus,
    /// The Guide control under the pointer.
    pub hover: ScreenFocus,
}

impl GuideFrame {
    fn lane_hover(&self) -> Option<LaneFocus> {
        match self.hover {
            ScreenFocus::GroupLane(lane) => Some(lane),
            _ => None,
        }
    }
}

pub(super) struct Sections {
    band: Cached<BandInputs>,
    lane: Cached<LaneInputs>,
    board: Cached<BoardInputs>,
    frame: Option<GuideFrame>,
}

impl Sections {
    pub(super) fn new(cx: &mut Context<GuideSurface>) -> Self {
        Self {
            band: Cached::new(Section::Band, cx),
            lane: Cached::new(Section::Lane, cx),
            board: Cached::new(Section::Board, cx),
            frame: None,
        }
    }
}

impl Sections {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn guide(
        &mut self,
        screen: &GuideScreen,
        preview: Option<&guide_view::PreviewSelection>,
        description: Option<&SharedString>,
        icons: &ChannelIcons,
        marker: Option<guide_view::playing_marker::PlayingMarker>,
        frame: GuideFrame,
        minute: guide_view::MinuteFrame,
    ) -> Div {
        self.frame = Some(frame);
        let GuideFrame {
            viewport,
            layout,
            wave,
            slide,
            docked,
            inline_search,
            ..
        } = frame;
        let ticking = minute.ticking();
        let translucent = layout.translucent;
        let arriving = wave != Wave::REST;
        let sliding = slide != SlideFrame::SETTLED;
        let focus = screen.focus();
        let toolbar = match frame.band_focus {
            ScreenFocus::Toolbar(t) => Some(t),
            _ => None,
        };
        let lane = match layout.shown_focus(focus) {
            ScreenFocus::GroupLane(l) => Some(l),
            _ => None,
        };
        let active = match layout.shown_focus(focus) {
            shown @ (ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_)) => shown,
            _ => ScreenFocus::None,
        };
        let hover = |region: fn(&ScreenFocus) -> bool| {
            Some(frame.hover)
                .filter(region)
                .unwrap_or(ScreenFocus::None)
        };
        let now = minute.now.timestamp();
        let band = self.band.element(
            BandInputs {
                toolbar,
                hover: hover(|focus| matches!(focus, ScreenFocus::Toolbar(_))),
                docked,
                inline_search,
                translucent,
                preview: preview.cloned(),
                description: description.cloned(),
                now,
                icons: icons.revision(),
            },
            arriving || ticking,
            viewport,
        );
        let lane = self.lane.element(
            LaneInputs {
                mode: screen.mode(),
                selected: screen.selected_lane(),
                focus: lane,
                hover: frame.lane_hover(),
                translucent,
                catalog: screen.content_revision(),
            },
            arriving || sliding,
            viewport,
        );
        let board = self.board.element(
            BoardInputs {
                marker,
                mode: screen.mode(),
                focus: active,
                hover: hover(|focus| {
                    matches!(focus, ScreenFocus::Board(_) | ScreenFocus::TimelineBoard(_))
                }),
                selected: screen.selected_lane(),
                scroll: screen.scroll_offset(),
                translucent,
                catalog: screen.content_revision(),
                page: screen.page_status(),
                notice: screen.notice(),
                now,
                axis: screen.timeline().map_or(0, |t| t.starts_at().timestamp()),
                icons: icons.revision(),
            },
            arriving || sliding || ticking,
            viewport,
        );
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family(guide_view::SAIRA)
            .text_color(layout.guide_text())
            .child(band)
            .child(lane)
            .child(board)
    }
}
