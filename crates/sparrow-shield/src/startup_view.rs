use gpui::{
    AnyWindowHandle, App, Context, FocusHandle, IntoElement, KeyDownEvent, Render, Task, Window,
    WindowOptions, div, prelude::*, px, rgb,
};

use crate::{DIAGNOSTICS, catalog};

#[derive(Clone, Copy, Debug)]
pub(super) enum Panel {
    Catalog,
    Graphics,
}

struct StartupView {
    update: catalog::Snapshot,
    panel: Panel,
    focus: FocusHandle,
    _subscription: Task<()>,
}

impl StartupView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut updates = catalog::subscribe();
        let update = *updates.borrow_and_update();
        let subscription = cx.spawn(async move |view, cx| {
            while updates.changed().await.is_ok() {
                let update = *updates.borrow_and_update();
                if view
                    .update(cx, |view, cx| {
                        view.update = update;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            update,
            panel: Panel::Catalog,
            focus,
            _subscription: subscription,
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.panel = match event.keystroke.key.as_str() {
            "left" | "up" => Panel::Catalog,
            "right" | "down" => Panel::Graphics,
            _ => return,
        };
        DIAGNOSTICS.lock().unwrap().focus = self.panel;
        cx.notify();
    }
}

impl Render for StartupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detail = match self.panel {
            Panel::Catalog => self.update.phase.label(),
            Panel::Graphics => crate::renderer_status_label(),
        };
        div()
            .id("sparrow-startup")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .gap(px(16.))
            .bg(rgb(0x0a1220))
            .text_color(rgb(0xe3eefc))
            .child(div().text_size(px(42.)).child("Sparrow"))
            .child(div().text_size(px(24.)).child(detail))
    }
}

pub(super) fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    match cx.open_window(WindowOptions::default(), |window, cx| {
        let mut diagnostics = DIAGNOSTICS.lock().unwrap();
        diagnostics.software = window.gpu_specs().map(|specs| specs.is_software_emulated);
        diagnostics.focus = Panel::Catalog;
        drop(diagnostics);
        cx.new(|cx| StartupView::new(window, cx))
    }) {
        Ok(window) => Some(window.into()),
        Err(_) => {
            DIAGNOSTICS.lock().unwrap().graphics_failed = true;
            log::error!("Sparrow Shield could not open its window");
            None
        }
    }
}
