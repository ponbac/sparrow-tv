use gpui::{Context, Window};
use sparrow_tv_guide::{
    PageUpdate,
    search::{SEARCH_DEBOUNCE, SearchRead},
};

use super::{GuideSurface, SearchDebounce, TextTarget};
use crate::{catalog, guide_view, search_flow::SearchEffect};

impl GuideSurface {
    pub(super) fn sync_search_preferences(&mut self, cx: &mut Context<Self>) {
        let effect = self.search.set_preferences(
            self.screen.exclusions().clone(),
            self.screen.variant_preferences().clone(),
        );
        self.search_effect(effect, None, cx);
    }

    pub(super) fn schedule_search(
        &mut self,
        timer: Option<SearchDebounce>,
        cx: &mut Context<Self>,
    ) {
        if let Some(timer) = timer {
            self.search_dwell = Some(cx.spawn(async move |view, cx| {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                let _ = view.update(cx, |view, cx| {
                    if let Some(read) = view.search.finish_debounce(timer) {
                        view.query_search(read, cx);
                    }
                });
            }));
            cx.notify();
        }
    }

    fn query_search(&mut self, read: SearchRead, cx: &mut Context<Self>) {
        let unavailable = read.unavailable();
        let Some(query) = catalog::search(read) else {
            let (outcome, _) = self.search.complete(unavailable);
            if outcome != PageUpdate::Superseded {
                cx.notify();
            }
            return;
        };
        cx.spawn(async move |view, cx| {
            let completion = query.await.unwrap_or(unavailable);
            let _ = view.update(cx, |view, cx| {
                let (outcome, effect) = view.search.complete(completion);
                if outcome == PageUpdate::ReconcileGeneration {
                    catalog::reconcile_status();
                }
                if outcome != PageUpdate::Superseded {
                    view.search_effect(effect, None, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn search_effect(
        &mut self,
        effect: SearchEffect,
        input: Option<(&mut Window, bool)>,
        cx: &mut Context<Self>,
    ) {
        match effect {
            SearchEffect::None => {}
            SearchEffect::Dismiss => self.retire_query_editor(),
            SearchEffect::Debounce(timer) => self.schedule_search(Some(timer), cx),
            SearchEffect::Read(read) => self.query_search(read, cx),
            SearchEffect::ReconcileGeneration => catalog::reconcile_status(),
            SearchEffect::Edit => {
                if let Some((window, _)) = input {
                    let rect = guide_view::search_editor_rect(
                        window.viewport_size(),
                        window.scale_factor(),
                        self.search.state(),
                        self.search.has_exclusions(),
                        self.playback.docked(),
                    );
                    if !self.open_query_editor(TextTarget::Search, rect) {
                        self.show_feedback(Some("Couldn’t open the keyboard."), cx);
                    }
                }
            }
            SearchEffect::GuideKey(key) => {
                if let Some((window, held)) = input {
                    self.press(key, held, window, cx);
                }
            }
            SearchEffect::Tune(tune) => self.search_tune(tune, cx),
        }
        cx.notify();
    }
}
