//! Go to offset and find in a hex document. Both are the editor's one-line bar with a hex kind
//! (`BarKind::GoToOffset`, `BarKind::HexFind`); the search runs on the host's thread over a
//! snapshot of the bytes and the unsaved edits, and its answer is taken only while the bar that
//! asked for it is still up.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::prelude::*;
use gpui::{AnyElement, Window};
use gpui_component::button::ButtonVariants as _;
use gpui_component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex};

use super::data::HexContent;
use super::model::{self, OffsetError, PatternError};
use super::*;

/// What a find looks for: hex digits, or the UTF-8 bytes of the text typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui::code_editor) enum FindMode {
    #[default]
    Hex,
    Text,
}

/// The placeholder of a find bar, which says what the field takes.
pub(in crate::ui::code_editor) fn placeholder(mode: FindMode) -> SharedString {
    match mode {
        FindMode::Hex => t(L10nKey::HexFindHexPlaceholder),
        FindMode::Text => t(L10nKey::HexFindTextPlaceholder),
    }
    .into()
}

/// What a hex bar tells the user about the last thing asked of it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Note {
    /// A search of a large file is still reading.
    Searching,
    NotFound,
    /// An offset that is not one; the file's length is what out of range is measured against.
    Offset(OffsetError, u64),
    Pattern(PatternError),
    ReadFailed(String),
}

impl Note {
    fn text(&self) -> String {
        match self {
            Note::Searching => t(L10nKey::HexFindSearching).to_string(),
            Note::NotFound => t(L10nKey::HexFindNotFound).to_string(),
            Note::Offset(OffsetError::Empty, _) => t(L10nKey::HexOffsetEmpty).to_string(),
            Note::Offset(OffsetError::Invalid, _) => t(L10nKey::HexOffsetInvalid).to_string(),
            Note::Offset(OffsetError::OutOfRange, len) => {
                t_fmt(L10nKey::HexOffsetOutOfRange, &[("len", &len.to_string())])
            }
            Note::Pattern(PatternError::Empty) => t(L10nKey::HexPatternEmpty).to_string(),
            Note::Pattern(PatternError::OddDigits) => t(L10nKey::HexPatternOddDigits).to_string(),
            Note::Pattern(PatternError::InvalidDigit) => {
                t(L10nKey::HexPatternInvalidDigit).to_string()
            }
            Note::ReadFailed(e) => t_fmt(L10nKey::HexFindFailed, &[("e", e)]),
        }
    }

    /// Whether it is something to fix, rather than a state of the search.
    fn is_error(&self) -> bool {
        !matches!(self, Note::Searching | Note::NotFound)
    }
}

/// A search that is running. Its answer is wanted only while it is the one in the bar, and
/// letting go of it, for a newer query or because the bar went away, tells the thread to stop.
struct Search {
    seq: u64,
    cancel: Arc<AtomicBool>,
    /// Started from a key in the view, so the view gets the focus back with the match.
    refocus: bool,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a hex bar holds besides its field.
#[derive(Default)]
pub(in crate::ui::code_editor) struct BarState {
    note: Option<Note>,
    search: Option<Search>,
}

/// What the hex documents remember between bars.
#[derive(Default)]
pub(in crate::ui::code_editor) struct FindState {
    /// The mode of the last find, which the next bar opens in.
    mode: FindMode,
    /// The last query typed, which F3 searches for when the bar has gone.
    text: String,
    /// Numbers searches, so an answer can tell whether it is still wanted.
    seq: u64,
}

/// The caret and the length of a document that can be searched.
fn caret_and_len(data: &HexData) -> Option<(u64, u64)> {
    match &data.content {
        HexContent::Ready(m) => Some((m.caret.offset, m.store.len())),
        HexContent::TooLarge { .. } => None,
    }
}

/// The bytes a find looks for.
fn pattern_of(mode: FindMode, text: &str) -> Result<Vec<u8>, PatternError> {
    match mode {
        FindMode::Hex => model::parse_hex_pattern(text),
        FindMode::Text if text.is_empty() => Err(PatternError::Empty),
        FindMode::Text => Ok(text.as_bytes().to_vec()),
    }
}

/// Where a search starts: just after the caret going forward, just before it going back. Before
/// the first byte there is nothing, which `find` reads as the end, so it wraps to the last.
fn start_of(caret: u64, forward: bool) -> u64 {
    if forward {
        caret.saturating_add(1)
    } else {
        caret.checked_sub(1).unwrap_or(u64::MAX)
    }
}

impl Tty59App {
    /// The hex document in front that can be searched, while the panel is showing.
    pub(in crate::ui::code_editor) fn hex_ready_in_front(&self, cx: &App) -> Option<BufferId> {
        if !self.code_panel_visible() {
            return None;
        }
        let id = self.tab_code().and_then(TabCode::active_id)?;
        let doc = self.hex(id)?;
        doc.data.read(cx).len().map(|_| id)
    }

    /// The view of `id` in the group that has the focus.
    fn hex_front_view(&self, id: BufferId, cx: &mut Context<Self>) -> Option<Entity<HexView>> {
        let slot = self.tab_code()?.focused_slot();
        let tab = self.tabs.get(self.active)?.tree_id.get();
        self.hex_view(id, tab, slot, cx)
    }

    fn hex_jump(
        &mut self,
        id: BufferId,
        offset: u64,
        len: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.hex_front_view(id, cx) {
            view.update(cx, |view, cx| view.jump_to(offset, len, window, cx));
        }
    }

    fn hex_bar_note(&mut self, note: Note, cx: &mut Context<Self>) {
        if let Some(bar) = self.editor.bar.as_mut() {
            bar.find.note = Some(note);
        }
        cx.notify();
    }

    /// The field changed: what the bar said about the last Enter was about something else.
    pub(in crate::ui::code_editor) fn hex_bar_edited(&mut self, cx: &mut Context<Self>) {
        if let Some(bar) = self.editor.bar.as_mut()
            && bar
                .find
                .note
                .as_ref()
                .is_some_and(|n| *n != Note::Searching)
        {
            bar.find.note = None;
            cx.notify();
        }
    }

    // ---- go to offset ----------------------------------------------------------------------

    /// Asks for an offset to put the caret on.
    pub(crate) fn hex_go_to_offset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.hex_ready_in_front(cx) else {
            return;
        };
        self.editor_open_bar(BarKind::GoToOffset { id }, String::new(), window, cx);
    }

    /// Enter in the go-to bar. A good offset closes the bar and moves the caret; a bad one
    /// stays up with the reason.
    pub(in crate::ui::code_editor) fn hex_go_to_offset_submit(
        &mut self,
        id: BufferId,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((caret, len)) = self.hex(id).and_then(|d| caret_and_len(d.data.read(cx))) else {
            return;
        };
        match model::parse_offset(text, caret, len) {
            Ok(offset) => {
                self.editor_close_bar(window, cx);
                self.hex_jump(id, offset, 0, window, cx);
            }
            Err(e) => self.hex_bar_note(Note::Offset(e, len), cx),
        }
    }

    // ---- find ------------------------------------------------------------------------------

    /// The find chord: the bar, in the mode it was last used in. With the bar already up the
    /// field takes the focus again.
    pub(crate) fn hex_find_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.hex_ready_in_front(cx) else {
            return;
        };
        if let Some(bar) = &self.editor.bar
            && matches!(bar.kind, BarKind::HexFind { id: shown, .. } if shown == id)
        {
            bar.input.update(cx, |st, cx| st.focus(window, cx));
            return;
        }
        let mode = self.editor.hex.find.mode;
        let text = self.editor.hex.find.text.clone();
        self.editor_open_bar(BarKind::HexFind { id, mode }, text, window, cx);
    }

    /// F3 and Shift+F3 in the view: the next or previous match of what the bar says, or of
    /// the last query when the bar has gone, which is then put back. With nothing searched yet
    /// it is the bar that is asked for.
    pub(crate) fn hex_find_step(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.hex_ready_in_front(cx) else {
            return;
        };
        let shown = match &self.editor.bar {
            Some(bar) => match bar.kind {
                BarKind::HexFind { id: shown, mode } if shown == id => {
                    Some((mode, bar.input.read(cx).value().to_string()))
                }
                _ => None,
            },
            None => None,
        };
        let (mode, text) = match shown {
            Some(query) => query,
            None if self.editor.hex.find.text.is_empty() => {
                self.hex_find_open(window, cx);
                return;
            }
            None => {
                let mode = self.editor.hex.find.mode;
                let text = self.editor.hex.find.text.clone();
                self.editor_open_bar(BarKind::HexFind { id, mode }, text.clone(), window, cx);
                (mode, text)
            }
        };
        self.hex_find(id, mode, &text, forward, true, window, cx);
    }

    /// Enter in the find bar.
    pub(in crate::ui::code_editor) fn hex_find_submit(
        &mut self,
        id: BufferId,
        mode: FindMode,
        text: &str,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hex_find(id, mode, text, forward, false, window, cx);
    }

    fn hex_find(
        &mut self,
        id: BufferId,
        mode: FindMode,
        text: &str,
        forward: bool,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pattern = match pattern_of(mode, text) {
            Ok(pattern) => pattern,
            Err(e) => {
                self.hex_bar_note(Note::Pattern(e), cx);
                return;
            }
        };
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (host, data) = (doc.host.clone(), doc.data.clone());
        let Some(snapshot) = data.read(cx).search_snapshot() else {
            return;
        };
        self.editor.hex.find.mode = mode;
        self.editor.hex.find.text = text.to_string();
        self.editor.hex.find.seq += 1;
        let seq = self.editor.hex.find.seq;
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(bar) = self.editor.bar.as_mut() else {
            return;
        };
        // Taking the place of the search before sets its flag, which is what stops it.
        bar.find.search = Some(Search {
            seq,
            cancel: cancel.clone(),
            refocus,
        });
        bar.find.note = snapshot.large.then_some(Note::Searching);
        cx.notify();

        let from = start_of(snapshot.caret, forward);
        let generation = snapshot.generation;
        let len = u64::try_from(pattern.len()).unwrap_or(u64::MAX);
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| snapshot.find(h, &pattern, from, forward, &cancel),
            move |app, found, window, cx| {
                app.hex_found(id, seq, generation, len, found, window, cx)
            },
        );
    }

    /// A search came back. It is used only if it is still the one the bar is waiting for, and
    /// the file is still the one it read.
    fn hex_found(
        &mut self,
        id: BufferId,
        seq: u64,
        generation: u64,
        len: u64,
        found: std::io::Result<Option<u64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bar) = self.editor.bar.as_mut() else {
            return;
        };
        if !matches!(bar.kind, BarKind::HexFind { id: shown, .. } if shown == id) {
            return;
        }
        let Some(search) = bar.find.search.take_if(|s| s.seq == seq) else {
            return;
        };
        bar.find.note = None;
        let current = self.hex(id).map(|d| d.data.read(cx).generation());
        if current != Some(generation) {
            cx.notify();
            return;
        }
        match found {
            Ok(Some(offset)) => {
                self.hex_jump(id, offset, len, window, cx);
                if search.refocus {
                    self.focus_editor(window, cx);
                }
            }
            Ok(None) => self.hex_bar_note(Note::NotFound, cx),
            Err(e) => self.hex_bar_note(Note::ReadFailed(e.to_string()), cx),
        }
        cx.notify();
    }

    /// Switches what the find bar looks for; the field keeps what was typed.
    fn hex_find_mode(&mut self, mode: FindMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.editor.bar.as_mut() else {
            return;
        };
        let BarKind::HexFind { id, .. } = bar.kind else {
            return;
        };
        bar.kind = BarKind::HexFind { id, mode };
        bar.find.note = None;
        let input = bar.input.clone();
        self.editor.hex.find.mode = mode;
        input.update(cx, |st, cx| {
            st.set_placeholder(placeholder(mode), window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }

    /// What follows the field in a hex bar: the mode of a find, and what the bar has to say.
    pub(in crate::ui::code_editor) fn hex_bar_tail(
        &self,
        bar: &EditorBar,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut tail = Vec::new();
        if let BarKind::HexFind { mode, .. } = bar.kind {
            let muted = cx.theme().muted_foreground;
            let toggle = |id: &'static str, label: L10nKey, this: FindMode| {
                let on = mode == this;
                Button::new(id)
                    .label(t(label))
                    .ghost()
                    .xsmall()
                    .selected(on)
                    .when(!on, |b| b.text_color(muted))
                    .on_click(
                        cx.listener(move |app, _, window, cx| app.hex_find_mode(this, window, cx)),
                    )
            };
            tail.push(
                crate::ui::code_editor::status::probe(
                    h_flex()
                        .flex_none()
                        .gap(px(2.))
                        .child(toggle(
                            "hex-find-mode-hex",
                            L10nKey::HexFindModeHex,
                            FindMode::Hex,
                        ))
                        .child(toggle(
                            "hex-find-mode-text",
                            L10nKey::HexFindModeText,
                            FindMode::Text,
                        )),
                    "hex-find-mode",
                )
                .into_any_element(),
            );
        }
        if let Some(note) = &bar.find.note {
            let color = if note.is_error() {
                cx.theme().danger
            } else {
                cx.theme().muted_foreground
            };
            tail.push(
                crate::ui::code_editor::status::probe(
                    div()
                        .flex_none()
                        .text_size(gpui::rems(crate::ui::right_panel::META))
                        .text_color(color)
                        .child(note.text()),
                    "editor-bar-note",
                )
                .into_any_element(),
            );
        }
        tail
    }
}

#[cfg(test)]
mod tests;
