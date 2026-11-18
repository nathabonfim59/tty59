//! The hex view: one `(document, tab, group)` showing a [`super::data::HexData`]. It owns the
//! scroll position, the focus and the measurements it was last laid out with; the bytes and
//! the caret belong to the document.

// Used once the editor opens hex tabs (WP2.1); removed then.
#![allow(dead_code)]

use gpui::prelude::*;
use gpui::{App, Context, Entity, FocusHandle, Window, div};

use super::data::{HexContent, HexData};
use super::model::{self, Values};

/// What the editor's status bar shows for a hex view.
pub(crate) struct HexStatus {
    pub(super) offset: u64,
    /// Length of the selection in bytes.
    pub(super) selection: Option<u64>,
    /// The bytes at the caret (fewer at the end of the file).
    pub(super) values: Values,
    pub(super) len: u64,
}

pub(crate) struct HexView {
    pub(crate) focus: FocusHandle,
    data: Entity<HexData>,
    slot: usize,
}

impl HexView {
    pub(crate) fn new(
        data: Entity<HexData>,
        slot: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            data,
            slot,
        }
    }

    /// `None` for a file that is too large to open.
    pub(crate) fn status(&self, cx: &App) -> Option<HexStatus> {
        let HexContent::Ready(m) = &self.data.read(cx).content else {
            return None;
        };
        let len = m.store.len();
        let at = m.caret.offset;
        let bytes: Vec<u8> = (at..at.saturating_add(4).min(len))
            .map_while(|off| m.store.effective(off))
            .collect();
        Some(HexStatus {
            offset: at,
            selection: m.caret.selection().map(|r| r.end - r.start),
            values: model::inspect(&bytes),
            len,
        })
    }
}

impl Render for HexView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
