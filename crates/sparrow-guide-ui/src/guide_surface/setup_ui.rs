//! First-launch Setup on the Guide surface: catalog progress in, draft
//! operations out. Setup owns every remote key while it is shown.

use gpui::{Context, Div, Window};
use sparrow_core::CatalogStatus;
use sparrow_tv_guide::{
    guide_screen::ScreenKey,
    setup::{CatalogProgress, SetupAction, SetupKey, SetupMessage},
};

use super::{GuideSurface, source_flow::Presentation};
use crate::guide_view;

impl GuideSurface {
    /// Applies a catalog publication. The first one decides whether Setup
    /// greets this window.
    pub(super) fn setup_catalog(&mut self, status: &CatalogStatus, cx: &mut Context<Self>) {
        let progress = CatalogProgress::from_status(status);
        if let Some(setup) = self.setup.as_mut() {
            setup.catalog(progress);
        } else {
            self.setup = self.setup_gate.observe(progress);
        }
        self.ensure_setup_draft(cx);
    }

    /// A step with a field needs a Setup draft. It can be missing after a
    /// refused open, or after Change sources from a relaunch.
    fn ensure_setup_draft(&mut self, cx: &mut Context<Self>) {
        if self
            .setup
            .as_ref()
            .is_some_and(|setup| setup.field().is_some())
            && self.setup_draft_missing()
        {
            self.begin_source_draft(None, Presentation::Setup, cx);
        }
    }

    /// Found counts the unfiltered roster once it has loaded.
    pub(super) fn setup_roster(&mut self) {
        if let Some(setup) = self.setup.as_mut() {
            setup.roster(
                self.screen.generation(),
                self.screen.group_roster_status(),
                self.screen.group_roster(),
            );
        }
    }

    /// An edit, clear or save handoff is waiting for the Activity. Keys queued
    /// meanwhile would act on a draft that is about to change.
    pub(super) fn setup_settling(&self) -> bool {
        self.source_request_pending() || self.setup_saving()
    }

    pub(super) fn setup_key(
        &mut self,
        key: ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.setup_settling() {
            return;
        }
        let draft = self.setup_draft();
        let Some(setup) = self.setup.as_mut() else {
            return;
        };
        let key = match key {
            ScreenKey::Arrow(direction) => SetupKey::Arrow(direction),
            ScreenKey::Activate => SetupKey::Activate,
        };
        let action = setup.press(key, draft);
        self.setup_action(action, window, cx);
    }

    pub(super) fn setup_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.setup_draft();
        let Some(setup) = self.setup.as_mut() else {
            return;
        };
        let action = setup.back(draft);
        self.setup_action(action, window, cx);
    }

    fn setup_action(&mut self, action: SetupAction, window: &mut Window, cx: &mut Context<Self>) {
        let failure = match action {
            SetupAction::None => None,
            SetupAction::Edit(field) => {
                let filled = self.setup_draft().has(field);
                let area = self.setup_layout(window);
                let rect = guide_view::setup_editor_rect(area, window.scale_factor(), filled)
                    .map(|rect| area.to_window(rect, window.scale_factor()));
                (!self.edit_source_field(field, rect)).then_some(SetupMessage::KeyboardUnavailable)
            }
            SetupAction::Clear(field) => {
                (!self.clear_source_field(field)).then_some(SetupMessage::DraftUnavailable)
            }
            SetupAction::Save { include_epg } => {
                (!self.submit_source_draft(include_epg)).then_some(SetupMessage::SaveFailed)
            }
            SetupAction::Retry => self.setup_retry(cx),
            SetupAction::ChangeSources => {
                if let Some(setup) = self.setup.as_mut() {
                    setup.change_sources(Default::default());
                }
                self.begin_source_draft(None, Presentation::Setup, cx);
                None
            }
            SetupAction::OpenGuide => {
                self.setup = None;
                self.close_sources(cx);
                None
            }
            SetupAction::Exit => {
                self.host.finish(window);
                None
            }
        };
        if let (Some(message), Some(setup)) = (failure, self.setup.as_mut()) {
            setup.show_message(message);
        }
        self.ensure_setup_draft(cx);
        self.sync_view(cx);
    }

    fn setup_retry(&mut self, cx: &mut Context<Self>) -> Option<SetupMessage> {
        let Some(retry) = self.catalog.refresh_now() else {
            return Some(SetupMessage::RetryUnavailable);
        };
        if let Some(setup) = self.setup.as_mut() {
            setup.retry_started();
        }
        cx.spawn(async move |view, cx| {
            let report = retry.await.ok();
            let _ = view.update(cx, |view, cx| {
                let current = view.catalog.status();
                let progress = report
                    .as_ref()
                    .map(|report| report.status())
                    .or(current.as_deref())
                    .map_or(CatalogProgress::Loading, CatalogProgress::from_status);
                if let Some(setup) = view.setup.as_mut() {
                    setup.retry_finished(progress);
                }
                view.sync_view(cx);
            });
        })
        .detach();
        None
    }

    pub(super) fn render_setup(&self, window: &Window) -> Option<Div> {
        let setup = self.setup.as_ref()?;
        Some(guide_view::setup_screen(
            self.setup_layout(window),
            guide_view::SetupPanel {
                setup,
                draft: self.setup_draft(),
                editing: self.source_editor_open(),
                saving: self.setup_saving(),
                now: self.now,
            },
        ))
    }
}
