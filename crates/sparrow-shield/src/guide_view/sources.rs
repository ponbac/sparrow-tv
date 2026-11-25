use chrono::{DateTime, Local, Utc};
use gpui::{Div, Pixels, Size, Styled, div, prelude::*, rgb, rgba};
use sparrow_core::{CatalogStatus, SafeFailure, SourceState};

use super::{
    CANVAS_WIDTH, CELL, CELL_ELAPSED, DIM, FAINT, ICE, INK, Layout, MUTE, ON_ICE, PANEL, SAIRA,
    SAIRA_NOTICE, SAIRA_TITLE, TEXT,
};

const WIDTH: f32 = CANVAS_WIDTH * 0.36;
const LEFT: f32 = CANVAS_WIDTH - WIDTH;
const PADDING: f32 = 30.;
const CONTENT_WIDTH: f32 = WIDTH - 2. * PADDING;
const CONTENT_Y: f32 = 116.;
const BUTTON_HEIGHT: f32 = 48.;
const BACKDROP_Y: f32 = 42.;
const AMBIENT_Y: f32 = BACKDROP_Y + BUTTON_HEIGHT + 6.;
const STATUS_Y: f32 = AMBIENT_Y + BUTTON_HEIGHT + 16.;
const CARD_Y: f32 = STATUS_Y + BUTTON_HEIGHT + 12.;
const CARD_HEIGHT: f32 = 112.;
const GUIDE_CARD_Y: f32 = CARD_Y + CARD_HEIGHT + 4.;
const DIAGNOSTICS_Y: f32 = GUIDE_CARD_Y + CARD_HEIGHT + 4.;
const SUMMARY_HEIGHT: f32 = 60.;
const DIAGNOSTICS_TEXT_Y: f32 = DIAGNOSTICS_Y + SUMMARY_HEIGHT + 12.;
const DIAGNOSTICS_LINE_HEIGHT: f32 = 27.;
const FIELD_HEIGHT: f32 = 56.;
const FIELD_LABEL_HEIGHT: f32 = 30.;
const FIELD_PADDING: f32 = 19.;
const FOCUSED_FIELD_BORDER: f32 = 3.;
const ERROR: u32 = 0xff8a75;
const CHEVRON_DOWN: &[u8] = include_bytes!("../../assets/icons/chevron-down.svg");
const CHEVRON_UP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m6 15 6-6 6 6"/></svg>"#;
const CLIPBOARD: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="8" y="2" width="8" height="4" rx="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/></svg>"#;
const CHECK: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m20 6-11 11-5-5"/></svg>"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in super::super) enum SourceFocus {
    Close,
    Backdrop,
    Refresh,
    Diagnostics,
    DiagnosticText,
    CopyDiagnostics,
    M3u,
    Epg,
    Save,
    Ambient,
}

#[derive(Clone, Copy)]
struct SourcePositions {
    diagnostics_height: f32,
    copy: f32,
    settings: f32,
    m3u: f32,
    epg: f32,
    save: f32,
}

impl SourcePositions {
    fn new(diagnostics: Option<&str>) -> Self {
        let diagnostics_height = diagnostics.map_or(0., |text| {
            text.lines().count() as f32 * DIAGNOSTICS_LINE_HEIGHT + 32.
        });
        let copy = DIAGNOSTICS_TEXT_Y + diagnostics_height + 12.;
        let settings = if diagnostics.is_some() {
            copy + BUTTON_HEIGHT + 28.
        } else {
            DIAGNOSTICS_Y + SUMMARY_HEIGHT + 28.
        };
        let m3u = settings + 138.;
        let epg = m3u + 100.;
        let save = epg + FIELD_HEIGHT + 16.;
        Self {
            diagnostics_height,
            copy,
            settings,
            m3u,
            epg,
            save,
        }
    }

    fn scroll(self, content_height: f32, focus: SourceFocus, editing: bool) -> f32 {
        let (top, height) = match focus {
            SourceFocus::Close | SourceFocus::Backdrop | SourceFocus::Refresh => return 0.,
            SourceFocus::Diagnostics => (DIAGNOSTICS_Y, SUMMARY_HEIGHT),
            SourceFocus::DiagnosticText => (DIAGNOSTICS_TEXT_Y, self.diagnostics_height),
            SourceFocus::CopyDiagnostics => (self.copy, BUTTON_HEIGHT),
            SourceFocus::M3u => (self.m3u, FIELD_HEIGHT),
            SourceFocus::Epg => (self.epg, FIELD_HEIGHT),
            SourceFocus::Ambient => (AMBIENT_Y, BUTTON_HEIGHT),
            SourceFocus::Save => (self.save, BUTTON_HEIGHT),
        };
        // Keep the label and editor at the top of the content so the TV
        // keyboard can cover the rest of the drawer without covering the text.
        if editing && matches!(focus, SourceFocus::M3u | SourceFocus::Epg) {
            return top - FIELD_LABEL_HEIGHT;
        }
        (top + height + 12. - content_height).clamp(0., top)
    }
}

pub(in super::super) struct SourcePanel<'a> {
    pub(in super::super) backdrop: sparrow_installed_catalog::GuideBackdrop,
    pub(in super::super) ambient: bool,
    pub(in super::super) status: Option<&'a CatalogStatus>,
    pub(in super::super) focus: SourceFocus,
    pub(in super::super) editing: bool,
    pub(in super::super) has_m3u: bool,
    pub(in super::super) has_epg: bool,
    pub(in super::super) saving: bool,
    pub(in super::super) refreshing: bool,
    pub(in super::super) diagnostics: Option<&'a str>,
    pub(in super::super) copy_label: &'static str,
    pub(in super::super) copied: bool,
    /// Allowlisted action feedback. Draft locations belong to the Android editor.
    pub(in super::super) feedback: Option<&'static str>,
}

/// Physical x, y, width and height relative to the GPUI surface.
pub(in super::super) fn source_editor_rect(
    size: Size<Pixels>,
    pixel_scale: f32,
) -> Option<[i32; 4]> {
    let layout = Layout::new(size);
    let top = CONTENT_Y + FIELD_LABEL_HEIGHT;
    let height = FIELD_HEIGHT.min(layout.height - top);
    if height <= 0. {
        return None;
    }
    let scale = layout.scale * pixel_scale;
    let text_inset = FOCUSED_FIELD_BORDER + FIELD_PADDING;
    Some([
        ((LEFT + PADDING + text_inset) * scale).round() as i32,
        (top * scale).round() as i32,
        ((CONTENT_WIDTH - 2. * text_inset) * scale).round() as i32,
        (height * scale).round() as i32,
    ])
}

pub(in super::super) fn sources_sheet(size: Size<Pixels>, panel: SourcePanel<'_>) -> Div {
    let layout = Layout::new(size);
    let content_height = layout.height - CONTENT_Y;
    let positions = SourcePositions::new(panel.diagnostics);
    let scroll = positions.scroll(content_height, panel.focus, panel.editing);
    let header_subtitle = panel
        .feedback
        .unwrap_or("Source locations stay on this device. Only their status is shown here.");
    let configured = panel
        .status
        .map(|status| status.configuration().is_configured());
    let epg_configured = panel.status.map(|status| status.configuration().has_epg());
    let section = layout
        .rect(0., -scroll, CONTENT_WIDTH, positions.save + BUTTON_HEIGHT)
        .child(
            layout
                .text(SAIRA_NOTICE, 28., 34., 700.)
                .child("Appearance"),
        )
        .child(
            action_frame(
                layout,
                layout.rect(0., BACKDROP_Y, CONTENT_WIDTH, BUTTON_HEIGHT),
                panel.focus == SourceFocus::Backdrop,
            )
            .justify_between()
            .child(layout.text(SAIRA, 22., 30., 600.).child("Backdrop"))
            .child(
                layout
                    .text(SAIRA, 21., 30., 500.)
                    .child(match panel.backdrop {
                        sparrow_installed_catalog::GuideBackdrop::Flat => "‹  Flat  ›",
                        sparrow_installed_catalog::GuideBackdrop::NordicGradient => {
                            "‹  Nordic gradient  ›"
                        }
                        sparrow_installed_catalog::GuideBackdrop::ChannelTint => {
                            "‹  Channel tint  ›"
                        }
                        sparrow_installed_catalog::GuideBackdrop::AuroraDrift => {
                            "‹  Aurora drift  ›"
                        }
                    }),
            ),
        )
        .child(
            action_frame(
                layout,
                layout.rect(0., AMBIENT_Y, CONTENT_WIDTH, BUTTON_HEIGHT),
                panel.focus == SourceFocus::Ambient,
            )
            .justify_between()
            .child(layout.text(SAIRA, 22., 30., 600.).child("Ambient light"))
            .child(layout.text(SAIRA, 21., 30., 500.).child(if panel.ambient {
                "‹  On  ›"
            } else {
                "‹  Off  ›"
            })),
        )
        .child(
            layout
                .rect(0., STATUS_Y, CONTENT_WIDTH, BUTTON_HEIGHT)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    layout
                        .text(SAIRA_NOTICE, 28., 34., 700.)
                        .child("Source status"),
                )
                .child(
                    action(
                        layout,
                        div(),
                        if panel.refreshing {
                            "Refresh in progress"
                        } else {
                            "Refresh sources"
                        },
                        panel.focus == SourceFocus::Refresh,
                    )
                    .when(configured != Some(true), |button| button.opacity(0.55)),
                ),
        )
        .child(status_card(
            layout,
            CARD_Y,
            "Channel source",
            SourceDetails::new(
                panel.status.map(CatalogStatus::m3u),
                configured,
                "Add an M3U source to load channels.",
            ),
        ))
        .child(status_card(
            layout,
            GUIDE_CARD_Y,
            "Programme guide",
            SourceDetails::new(
                panel.status.and_then(CatalogStatus::epg),
                epg_configured,
                "An XMLTV source adds programme listings.",
            ),
        ))
        .child(source_diagnostics(layout, &panel, positions))
        .child(
            layout
                .rect(0., positions.settings, CONTENT_WIDTH, 1.)
                .bg(rgba(FAINT)),
        )
        .child(
            layout
                .rect(0., positions.settings + 24., CONTENT_WIDTH, 48.)
                .flex()
                .items_center()
                .child(layout.text(SAIRA_NOTICE, 28., 34., 700.).child(
                    if configured == Some(true) {
                        "Replace sources"
                    } else {
                        "Add your sources"
                    },
                )),
        )
        .child(
            layout
                .rect(0., positions.settings + 72., CONTENT_WIDTH, 29.)
                .child(
                    layout
                        .text(SAIRA, 20., 28., 400.)
                        .text_color(rgba(DIM))
                        .child("Use an M3U source and, if you have one, an XMLTV guide."),
                ),
        )
        .child(source_field(
            layout,
            positions.m3u,
            "Channel source (required)",
            "https://…/channels.m3u",
            panel.has_m3u,
            panel.focus == SourceFocus::M3u,
            panel.editing && panel.focus == SourceFocus::M3u,
        ))
        .child(source_field(
            layout,
            positions.epg,
            "Guide source (optional)",
            "https://…/guide.xml",
            panel.has_epg,
            panel.focus == SourceFocus::Epg,
            panel.editing && panel.focus == SourceFocus::Epg,
        ))
        .child(
            layout
                .rect(0., positions.save, CONTENT_WIDTH, BUTTON_HEIGHT)
                .flex()
                .child(action(
                    layout,
                    div(),
                    if panel.saving {
                        "Checking and saving"
                    } else if configured == Some(true) {
                        "Replace sources"
                    } else {
                        "Save sources"
                    },
                    panel.focus == SourceFocus::Save,
                )),
        );
    let sheet = layout
        .rect(LEFT, 0., WIDTH, layout.height)
        .bg(rgb(PANEL))
        .text_color(rgb(TEXT))
        .overflow_hidden()
        .child(
            layout
                .rect(PADDING, 30., WIDTH - 130., 48.)
                .child(layout.text(SAIRA_TITLE, 40., 42., 700.).child("Sources")),
        )
        .child(
            layout.rect(PADDING, 81., CONTENT_WIDTH, 30.).child(
                layout
                    .text(SAIRA, 18., 25., 400.)
                    .text_color(if panel.feedback.is_some() {
                        rgb(ICE)
                    } else {
                        rgba(DIM)
                    })
                    .truncate()
                    .child(header_subtitle),
            ),
        )
        .child(
            action(
                layout,
                layout.rect(WIDTH - 78., 30., 48., 48.),
                "×",
                panel.focus == SourceFocus::Close,
            )
            .px(layout.px(0.)),
        )
        .child(
            layout
                .rect(PADDING, CONTENT_Y, CONTENT_WIDTH, content_height)
                .overflow_hidden()
                .child(section),
        );
    div()
        .absolute()
        .inset_0()
        .bg(rgba(
            if matches!(panel.focus, SourceFocus::Backdrop | SourceFocus::Ambient) {
                0x05091040
            } else {
                0x050910b3
            },
        ))
        .child(sheet)
}

fn source_diagnostics(layout: Layout, panel: &SourcePanel<'_>, positions: SourcePositions) -> Div {
    let focused = panel.focus == SourceFocus::Diagnostics;
    let ink = rgb(if focused { ON_ICE } else { TEXT });
    div()
        .absolute()
        .inset_0()
        .child(
            layout
                .rect(0., DIAGNOSTICS_Y, CONTENT_WIDTH, SUMMARY_HEIGHT)
                .rounded(layout.px(8.))
                .bg(rgb(if focused { ICE } else { CELL }))
                .text_color(ink)
                .flex()
                .items_center()
                .justify_between()
                .px(layout.px(21.))
                .when(panel.status.is_none(), |summary| summary.opacity(0.55))
                .child(
                    layout
                        .text(SAIRA, 20., 28., 600.)
                        .child("Diagnostics you can copy"),
                )
                .child(layout.icon(
                    if panel.diagnostics.is_some() {
                        CHEVRON_UP
                    } else {
                        CHEVRON_DOWN
                    },
                    18.,
                    ink,
                )),
        )
        .when_some(panel.diagnostics, |section, text| {
            let text_focused = panel.focus == SourceFocus::DiagnosticText;
            let body = layout
                .rect(
                    0.,
                    DIAGNOSTICS_TEXT_Y,
                    CONTENT_WIDTH,
                    positions.diagnostics_height,
                )
                .rounded(layout.px(8.))
                .bg(rgb(INK))
                .border(layout.px(if text_focused { 4. } else { 0. }))
                .border_color(rgb(ICE))
                .py(layout.px(if text_focused { 12. } else { 16. }))
                .px(layout.px(if text_focused { 17. } else { 21. }))
                .overflow_hidden()
                .children(text.lines().map(|line| {
                    layout
                        .text(SAIRA, 18., DIAGNOSTICS_LINE_HEIGHT, 400.)
                        .text_color(rgba(DIM))
                        .truncate()
                        .child(line.to_owned())
                }));
            section.child(body).child(
                layout
                    .rect(0., positions.copy, CONTENT_WIDTH, BUTTON_HEIGHT)
                    .flex()
                    .child(
                        action_frame(layout, div(), panel.focus == SourceFocus::CopyDiagnostics)
                            .gap(layout.px(12.))
                            .child(layout.icon(
                                if panel.copied { CHECK } else { CLIPBOARD },
                                24.,
                                rgb(if panel.focus == SourceFocus::CopyDiagnostics {
                                    ON_ICE
                                } else {
                                    TEXT
                                }),
                            ))
                            .child(layout.text(SAIRA, 18., 24., 600.).child(panel.copy_label)),
                    ),
            )
        })
}

fn action(layout: Layout, bounds: Div, label: &'static str, focused: bool) -> Div {
    action_frame(layout, bounds, focused).child(layout.text(SAIRA, 18., 24., 600.).child(label))
}

fn action_frame(layout: Layout, bounds: Div, focused: bool) -> Div {
    bounds
        .h(layout.px(BUTTON_HEIGHT))
        .px(layout.px(22.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(if focused { ICE } else { CELL_ELAPSED }))
        .text_color(rgb(if focused { ON_ICE } else { TEXT }))
}

fn source_field(
    layout: Layout,
    top: f32,
    label: &'static str,
    placeholder: &'static str,
    filled: bool,
    focused: bool,
    editing: bool,
) -> Div {
    layout
        .rect(
            0.,
            top - FIELD_LABEL_HEIGHT,
            CONTENT_WIDTH,
            FIELD_HEIGHT + FIELD_LABEL_HEIGHT,
        )
        .child(layout.text(SAIRA, 18., 24., 600.).child(label))
        .child(
            layout
                .rect(0., FIELD_LABEL_HEIGHT, CONTENT_WIDTH, FIELD_HEIGHT)
                .rounded_full()
                .bg(rgb(INK))
                .border(layout.px(if focused { FOCUSED_FIELD_BORDER } else { 2. }))
                .border_color(if focused { rgb(ICE) } else { rgba(FAINT) })
                .flex()
                .items_center()
                .px(layout.px(FIELD_PADDING))
                .when(!editing, |field| {
                    field.child(
                        layout
                            .text(SAIRA, 21., 28., 400.)
                            .text_color(if filled { rgb(TEXT) } else { rgba(MUTE) })
                            .child(if filled {
                                "Location entered"
                            } else {
                                placeholder
                            }),
                    )
                }),
        )
}

fn status_card(layout: Layout, top: f32, title: &'static str, details: SourceDetails<'_>) -> Div {
    let SourceDetails {
        label,
        dot,
        detail,
        checked,
        next,
        failure,
    } = details;
    let telemetry = match (checked, next) {
        (Some((label, checked)), Some(next)) => Some(format!(
            "{label} {} · Next attempt {}",
            local_time(checked),
            local_time(next)
        )),
        (Some((label, checked)), None) => Some(format!("{label} {}", local_time(checked))),
        (None, Some(next)) => Some(format!("Next attempt {}", local_time(next))),
        (None, None) => None,
    };
    layout
        .rect(0., top, CONTENT_WIDTH, CARD_HEIGHT)
        .rounded(layout.px(8.))
        .bg(rgb(CELL))
        .px(layout.px(21.))
        .py(layout.px(12.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(layout.text(SAIRA_NOTICE, 24., 30., 700.).child(title))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(layout.px(9.))
                        .child(div().size(layout.px(8.)).rounded_full().bg(rgba(dot)))
                        .child(layout.text(SAIRA, 18., 25., 600.).child(label)),
                ),
        )
        .when_some(detail, |card, detail| {
            card.child(
                layout
                    .text(SAIRA, 20., 26., 400.)
                    .text_color(rgba(DIM))
                    .child(detail),
            )
        })
        .when_some(telemetry, |card, telemetry| {
            card.child(
                layout
                    .text(SAIRA, 19., 25., 400.)
                    .text_color(rgba(DIM))
                    .child(telemetry),
            )
        })
        .when_some(failure, |card, failure| {
            card.child(
                layout
                    .text(SAIRA, 19., 25., 600.)
                    .text_color(rgb(ERROR))
                    .truncate()
                    .child(failure_detail(failure)),
            )
        })
}

fn local_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%b %-d, %H:%M").to_string()
}

fn failure_detail(failure: &SafeFailure) -> String {
    match failure {
        SafeFailure::SourceAccess { reason, .. } => reason.to_string(),
        SafeFailure::SourceRead { reason, .. } => reason.to_string(),
        SafeFailure::Snapshot { reason, .. } => reason.to_string(),
        failure => failure.to_string(),
    }
}

struct SourceDetails<'a> {
    label: &'static str,
    dot: u32,
    detail: Option<&'static str>,
    checked: Option<(&'static str, DateTime<Utc>)>,
    next: Option<DateTime<Utc>>,
    failure: Option<&'a SafeFailure>,
}

impl<'a> SourceDetails<'a> {
    fn new(state: Option<&'a SourceState>, configured: Option<bool>, setup: &'static str) -> Self {
        if configured == Some(false) {
            return Self::label("Not set up", FAINT, Some(setup));
        }
        match state {
            None => Self::label("Checking", DIM, None),
            Some(SourceState::Fresh { validated_at }) => Self {
                checked: Some(("Checked", *validated_at)),
                ..Self::label("Fresh", 0x8ed9a1ff, None)
            },
            Some(SourceState::Stale {
                validated_at,
                next_attempt_at,
            }) => Self {
                checked: Some(("Last checked", *validated_at)),
                next: *next_attempt_at,
                ..Self::label("Stale, showing the saved copy", 0xffc76aff, None)
            },
            Some(SourceState::Refreshing {
                validated_at,
                started_at,
            }) => Self {
                checked: Some(("Started", *started_at)),
                ..if validated_at.is_some() {
                    Self::label("Refreshing, showing the saved copy", DIM, None)
                } else {
                    Self::label(
                        "Refreshing",
                        DIM,
                        Some("Sparrow is loading this source for the first time."),
                    )
                }
            },
            Some(SourceState::Deferred {
                validated_at,
                deferred_at,
            }) => Self {
                checked: Some(("Waiting since", *deferred_at)),
                ..if validated_at.is_some() {
                    Self::label("Waiting, showing the saved copy", DIM, None)
                } else {
                    Self::label(
                        "Waiting",
                        DIM,
                        Some("Refresh starts when Sparrow is free to run it."),
                    )
                }
            },
            Some(SourceState::Failed {
                validated_at,
                next_attempt_at,
                failure,
            }) => Self {
                checked: validated_at.map(|at| ("Last checked", at)),
                next: Some(*next_attempt_at),
                failure: Some(failure),
                ..if validated_at.is_some() {
                    Self::label("Failed, showing the saved copy", ERROR << 8 | 0xff, None)
                } else {
                    Self::label("Failed", ERROR << 8 | 0xff, None)
                }
            },
            Some(SourceState::Unavailable { failure }) => Self {
                failure: failure.as_ref(),
                ..Self::label(
                    "Unavailable",
                    ERROR << 8 | 0xff,
                    Some("This source has not loaded yet."),
                )
            },
        }
    }

    fn label(label: &'static str, dot: u32, detail: Option<&'static str>) -> Self {
        Self {
            label,
            dot,
            detail,
            checked: None,
            next: None,
            failure: None,
        }
    }
}
