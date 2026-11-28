use std::cell::RefCell;

use gpui::{
    App, ClipboardItem, Div, FontWeight, KeyDownEvent, SharedString, TextRun, Window, div, font,
    prelude::*, px, rgb, rgba,
};
use sparrow_guide_ui::{
    ClipboardTarget, DraftStatus, Edit, EditorKey, FieldEdit, FormFactor, GuideHost, HostEvent,
    SourceEvent, TextEditorHost,
};
use sparrow_installed_catalog::SavedSourceLocations;
use sparrow_tv_guide::{guide_screen::ScreenKey, now_focus::Direction, setup::SetupField};
use tokio::sync::mpsc;

pub(super) struct DesktopHost {
    events: mpsc::UnboundedSender<HostEvent>,
    draft: RefCell<Option<Draft>>,
    editor: RefCell<Option<Editor>>,
}

struct Draft {
    session: u64,
    m3u: String,
    epg: String,
}

impl Draft {
    fn field(&mut self, field: SetupField) -> &mut String {
        match field {
            SetupField::M3u => &mut self.m3u,
            SetupField::Epg => &mut self.epg,
        }
    }
}

const SAIRA: &str = "Sparrow Guide 92";
const ICE: u32 = 0x9fe0ff;
const TEXT: u32 = 0xeef3f8;
const SELECTION: u32 = 0x9fe0ff59;
const MASK: char = '•';

struct Editor {
    generation: u64,
    rect: [i32; 4],
    text: String,
    /// Byte offset of the caret, always on a character boundary.
    caret: usize,
    /// Ctrl+A selected the whole text; the next edit replaces it.
    all_selected: bool,
    source: Option<(u64, SetupField)>,
}

impl Editor {
    fn new(
        generation: u64,
        rect: [i32; 4],
        text: String,
        source: Option<(u64, SetupField)>,
    ) -> Self {
        Self {
            generation,
            rect,
            caret: text.len(),
            text,
            all_selected: false,
            source,
        }
    }

    fn insert(&mut self, inserted: &str) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
            self.caret = 0;
        }
        let clean: String = inserted.chars().filter(|ch| !ch.is_control()).collect();
        self.text.insert_str(self.caret, &clean);
        self.caret += clean.len();
    }

    /// Deletes the selection, or the character before or after the caret.
    fn delete(&mut self, forward: bool) {
        if std::mem::take(&mut self.all_selected) {
            self.text.clear();
            self.caret = 0;
            return;
        }
        let range = if forward {
            self.text[self.caret..]
                .chars()
                .next()
                .map(|ch| self.caret..self.caret + ch.len_utf8())
        } else {
            self.text[..self.caret]
                .chars()
                .next_back()
                .map(|ch| self.caret - ch.len_utf8()..self.caret)
        };
        if let Some(range) = range {
            self.caret = range.start;
            self.text.replace_range(range, "");
        }
    }

    fn move_caret(&mut self, key: &str) {
        let selected = std::mem::take(&mut self.all_selected);
        self.caret = match key {
            "home" => 0,
            "end" => self.text.len(),
            "left" if selected => 0,
            "right" if selected => self.text.len(),
            "left" => self.text[..self.caret]
                .chars()
                .next_back()
                .map_or(0, |ch| self.caret - ch.len_utf8()),
            _ => self.text[self.caret..]
                .chars()
                .next()
                .map_or(self.caret, |ch| self.caret + ch.len_utf8()),
        };
    }

    /// Text as shown: Source Configuration locations stay masked.
    fn shown(&self) -> (String, String) {
        let (before, after) = self.text.split_at(self.caret);
        if self.source.is_some() {
            let mask = |part: &str| MASK.to_string().repeat(part.chars().count());
            (mask(before), mask(after))
        } else {
            (before.to_owned(), after.to_owned())
        }
    }
}

impl DesktopHost {
    pub(super) fn new(events: mpsc::UnboundedSender<HostEvent>) -> Self {
        Self {
            events,
            draft: RefCell::new(None),
            editor: RefCell::new(None),
        }
    }

    fn send(&self, event: HostEvent) -> bool {
        self.events.send(event).is_ok()
    }

    fn source_event(&self, session: u64, update: SourceEvent) -> bool {
        self.send(HostEvent::Source { session, update })
    }

    fn finish_edit(&self, editor: Editor, committed: bool) {
        let Editor {
            generation,
            text,
            source,
            ..
        } = editor;
        if let Some((session, field)) = source {
            let mut draft = self.draft.borrow_mut();
            if let Some(draft) = draft.as_mut().filter(|draft| draft.session == session) {
                let value = draft.field(field);
                if committed {
                    *value = text.trim().to_owned();
                }
                self.source_event(
                    session,
                    SourceEvent::Field {
                        generation,
                        field,
                        edit: if committed {
                            FieldEdit::Committed
                        } else {
                            FieldEdit::Cancelled
                        },
                        has_text: !value.is_empty(),
                    },
                );
            }
        } else {
            self.send(HostEvent::Edited {
                generation,
                edit: Edit::Cancelled,
            });
        }
    }

    fn edit_key(
        &self,
        event: &KeyDownEvent,
        read_clipboard: impl FnOnce() -> Option<String>,
    ) -> EditorKey {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let mut slot = self.editor.borrow_mut();
        let Some(editor) = slot.as_mut() else {
            return EditorKey::Ignored;
        };
        let query = editor.source.is_none();

        match key {
            "escape" | "enter" if !event.is_held && !query => {
                if let Some(editor) = slot.take() {
                    drop(slot);
                    self.finish_edit(editor, key == "enter");
                }
                return EditorKey::Handled;
            }
            "escape" if !event.is_held => {
                let generation = editor.generation;
                slot.take();
                return EditorKey::QueryCancelled(generation);
            }
            "enter" | "up" | "down"
                if query
                    && !event.is_held
                    && !modifiers.control
                    && !modifiers.platform
                    && !modifiers.alt =>
            {
                let generation = editor.generation;
                let text = std::mem::take(&mut editor.text);
                slot.take();
                let key = match key {
                    "up" => ScreenKey::Arrow(Direction::Up),
                    "down" => ScreenKey::Arrow(Direction::Down),
                    _ => ScreenKey::Activate,
                };
                return EditorKey::QueryCommitted {
                    generation,
                    text,
                    key,
                };
            }
            "a" | "f" | "k"
                if modifiers.control
                    && (key == "a" || (query && !modifiers.platform && !modifiers.alt)) =>
            {
                editor.all_selected = !editor.text.is_empty();
                return EditorKey::Handled;
            }
            "left" | "right" | "home" | "end" => {
                editor.move_caret(key);
                return EditorKey::Handled;
            }
            "backspace" | "delete" => editor.delete(key == "delete"),
            "v" if modifiers.control => {
                let Some(text) = read_clipboard() else {
                    return EditorKey::Handled;
                };
                editor.insert(&text);
            }
            _ if modifiers.control || modifiers.platform || modifiers.alt => {
                return EditorKey::Handled;
            }
            _ => {
                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return EditorKey::Handled;
                };
                editor.insert(text);
            }
        }
        if query {
            self.send(HostEvent::Edited {
                generation: editor.generation,
                edit: Edit::Changed(editor.text.clone()),
            });
        }
        EditorKey::Handled
    }
}

impl GuideHost for DesktopHost {
    fn id(&self) -> u64 {
        1
    }

    fn form_factor(&self) -> FormFactor {
        FormFactor::Desktop
    }

    fn guide_painted(&self) {
        self.send(HostEvent::Presented);
    }

    fn finish(&self, window: &mut Window) {
        window.remove_window();
    }

    fn copy_diagnostics(&self, target: ClipboardTarget, text: &str, cx: &mut App) -> bool {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        match target {
            ClipboardTarget::Source { session, attempt } => self.source_event(
                session,
                SourceEvent::Copied {
                    attempt,
                    copied: true,
                },
            ),
            ClipboardTarget::Playback { attempt } => {
                self.send(HostEvent::PlaybackDiagnosticsCopied {
                    attempt,
                    copied: true,
                })
            }
        }
    }
}

impl TextEditorHost for DesktopHost {
    fn open_query(&self, generation: u64, rect: [i32; 4], initial: &str) -> bool {
        self.editor.replace(Some(Editor::new(
            generation,
            rect,
            initial.to_owned(),
            None,
        )));
        true
    }

    fn close_editor(&self, generation: u64) {
        self.editor
            .borrow_mut()
            .take_if(|editor| editor.generation == generation);
    }

    fn begin_draft(&self, session: u64, seed: Option<SavedSourceLocations>) -> bool {
        let (m3u, epg) = seed
            .map(SavedSourceLocations::into_locations)
            .unwrap_or_default();
        self.draft.replace(Some(Draft {
            session,
            m3u,
            epg: epg.unwrap_or_default(),
        }));
        self.source_event(session, SourceEvent::Draft(DraftStatus::Opened))
    }

    fn close_draft(&self, session: u64) {
        self.draft
            .borrow_mut()
            .take_if(|draft| draft.session == session);
        self.editor
            .borrow_mut()
            .take_if(|editor| editor.source.is_some_and(|(owner, _)| owner == session));
    }

    fn edit_field(&self, session: u64, generation: u64, field: SetupField, rect: [i32; 4]) -> bool {
        let mut draft = self.draft.borrow_mut();
        let Some(draft) = draft.as_mut().filter(|draft| draft.session == session) else {
            return false;
        };
        self.editor.replace(Some(Editor::new(
            generation,
            rect,
            draft.field(field).clone(),
            Some((session, field)),
        )));
        true
    }

    fn clear_field(&self, session: u64, generation: u64, field: SetupField) -> bool {
        let mut draft = self.draft.borrow_mut();
        let Some(draft) = draft.as_mut().filter(|draft| draft.session == session) else {
            return false;
        };
        draft.field(field).clear();
        self.source_event(
            session,
            SourceEvent::Field {
                generation,
                field,
                edit: FieldEdit::Cleared,
                has_text: false,
            },
        )
    }

    fn submit_draft(&self, session: u64, attempt: u64) -> bool {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref().filter(|draft| draft.session == session) else {
            return false;
        };
        self.source_event(
            session,
            SourceEvent::Submit {
                attempt,
                m3u: draft.m3u.clone(),
                epg: (!draft.epg.is_empty()).then(|| draft.epg.clone()),
            },
        )
    }

    fn finish_save(&self, session: u64, _: u64, saved: bool) {
        if saved {
            self.close_draft(session);
        }
    }

    fn cancel_editor(&self) {
        let editor = self.editor.borrow_mut().take();
        if let Some(editor) = editor {
            self.finish_edit(editor, false);
        }
    }

    fn editor_key(&self, event: &KeyDownEvent, cx: &mut App) -> EditorKey {
        self.edit_key(event, || {
            cx.read_from_clipboard().and_then(|item| item.text())
        })
    }

    fn editor_overlay(&self, window: &Window) -> Option<Div> {
        let editor = self.editor.borrow();
        let editor = editor.as_ref()?;
        let [x, y, width, height] = editor.rect.map(|v| v as f32 / window.scale_factor());
        // The rect is the text area of a field the Guide draws around it.
        let size = (height * 0.36).clamp(12., 30.);
        let (before, after) = editor.shown();
        let measure = |text: &str| {
            let text = SharedString::from(text.to_owned());
            let mut run_font = font(SAIRA);
            run_font.weight = FontWeight(400.);
            let line = window.text_system().shape_line(
                text.clone(),
                px(size),
                &[TextRun {
                    font: run_font,
                    len: text.len(),
                    ..TextRun::default()
                }],
                None,
            );
            f32::from(line.width())
        };
        // Keep the caret inside the field by scrolling the text left.
        let shift = (measure(&before) + 2. - width).max(0.);
        let text = |part: String| div().flex_none().whitespace_nowrap().child(part);
        let caret = div().flex_none().w(px(2.)).h(px(size * 1.3)).bg(rgb(ICE));
        let content = div()
            .flex()
            .items_center()
            .ml(px(-shift))
            .when(editor.all_selected, |line| {
                line.child(
                    text(format!("{before}{after}"))
                        .bg(rgba(SELECTION))
                        .rounded(px(2.)),
                )
            })
            .when(!editor.all_selected, |line| {
                line.child(text(before)).child(caret).child(text(after))
            });
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(width))
                .h(px(height))
                .flex()
                .items_center()
                .overflow_hidden()
                .font_family(SAIRA)
                .text_size(px(size))
                .text_color(rgb(TEXT))
                .child(content),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Keystroke, Modifiers};

    use super::*;

    fn key(key: &str, control: bool) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                key: key.into(),
                key_char: (key.len() == 1).then(|| key.into()),
                modifiers: Modifiers {
                    control,
                    ..Modifiers::default()
                },
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn query_arrows_and_enter_commit_the_text_and_return_the_navigation_key() {
        let (send, _) = mpsc::unbounded_channel();
        let host = DesktopHost::new(send);
        for (name, expected) in [
            ("down", ScreenKey::Arrow(Direction::Down)),
            ("up", ScreenKey::Arrow(Direction::Up)),
            ("enter", ScreenKey::Activate),
        ] {
            assert!(host.open_query(1, [0; 4], "query"));
            let result = host.edit_key(&key(name, false), || None);
            assert!(
                matches!(result, EditorKey::QueryCommitted { generation: 1, text, key } if text == "query" && key == expected)
            );
            assert!(host.editor.borrow().is_none());
        }
    }

    #[test]
    fn escape_dismisses_the_query_layer_but_only_cancels_a_source_field() {
        let (send, mut events) = mpsc::unbounded_channel();
        let host = DesktopHost::new(send);
        let escape = key("escape", false);
        assert!(host.open_query(1, [0; 4], "query"));
        assert!(matches!(
            host.edit_key(&escape, || None),
            EditorKey::QueryCancelled(1)
        ));
        assert!(host.editor.borrow().is_none());
        assert!(events.try_recv().is_err());

        assert!(host.begin_draft(2, None));
        assert!(matches!(
            events.try_recv(),
            Ok(HostEvent::Source {
                update: SourceEvent::Draft(DraftStatus::Opened),
                ..
            })
        ));
        assert!(host.edit_field(2, 3, SetupField::M3u, [0; 4]));
        assert!(matches!(
            host.edit_key(&escape, || None),
            EditorKey::Handled
        ));
        assert!(matches!(
            events.try_recv(),
            Ok(HostEvent::Source {
                session: 2,
                update: SourceEvent::Field {
                    generation: 3,
                    field: SetupField::M3u,
                    edit: FieldEdit::Cancelled,
                    ..
                },
            })
        ));
        assert!(host.editor.borrow().is_none());
        assert!(host.draft.borrow().is_some());
    }

    #[test]
    fn search_shortcuts_select_query_text_for_replacement() {
        let (send, mut events) = mpsc::unbounded_channel();
        let host = DesktopHost::new(send);
        for shortcut in ["f", "k"] {
            assert!(host.open_query(1, [0; 4], "old"));
            assert!(matches!(
                host.edit_key(&key(shortcut, true), || None),
                EditorKey::Handled
            ));
            assert!(events.try_recv().is_err());
            assert!(matches!(
                host.edit_key(&key("x", false), || None),
                EditorKey::Handled
            ));
            assert!(
                matches!(events.try_recv(), Ok(HostEvent::Edited { edit: Edit::Changed(text), .. }) if text == "x")
            );
        }
    }

    #[test]
    fn caret_edits_insert_and_delete_around_the_caret() {
        let mut editor = Editor::new(1, [0; 4], "naïve".into(), None);
        assert_eq!(editor.caret, editor.text.len());
        editor.move_caret("left");
        editor.move_caret("left");
        editor.insert("X");
        assert_eq!(editor.text, "naïXve");
        editor.delete(false);
        editor.delete(false);
        assert_eq!(editor.text, "nave");
        editor.move_caret("home");
        editor.delete(true);
        assert_eq!(editor.text, "ave");
        editor.move_caret("end");
        editor.delete(true);
        assert_eq!(editor.text, "ave");
        editor.insert("ny\u{7}");
        assert_eq!(editor.text, "aveny");
    }

    #[test]
    fn select_all_is_replaced_by_the_next_edit() {
        let mut editor = Editor::new(1, [0; 4], "old".into(), None);
        editor.all_selected = true;
        editor.insert("new");
        assert_eq!((editor.text.as_str(), editor.caret), ("new", 3));

        editor.all_selected = true;
        editor.move_caret("left");
        assert_eq!(editor.caret, 0);
        editor.all_selected = true;
        editor.delete(false);
        assert!(editor.text.is_empty());
    }

    #[test]
    fn source_locations_are_masked_on_both_sides_of_the_caret() {
        let mut editor = Editor::new(1, [0; 4], "http://a".into(), Some((2, SetupField::M3u)));
        editor.move_caret("left");
        let (before, after) = editor.shown();
        assert_eq!(before, "•".repeat(7));
        assert_eq!(after, "•");
        assert!(!before.contains('h'));
    }
}
