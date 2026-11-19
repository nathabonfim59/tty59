//! The hex view: one `(document, tab, group)` showing a [`super::data::HexData`]. It owns the
//! scroll position, the focus and the measurements it was last laid out with; the bytes and
//! the caret belong to the document.
//!
//! The rows are not a `uniform_list`: that positions rows with `f32` pixels, which cannot tell
//! one row from the next once a file is a few hundred thousand rows deep (a 1 GiB file is 67
//! million). The view keeps its own scroll position as a `u64` row plus a pixel offset inside
//! that row, builds only the rows the viewport reaches, and gives the shared scrollbar a
//! virtual content height small enough for `f32`.
//!
//! Render and prepaint never read files or change the document. The one write they make is the
//! measured layout into a shared cell; when what the viewport shows changed since the last
//! request, prepaint defers a call that asks the document for the pages (see `request_visible`).

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, FocusHandle, HighlightStyle, Hsla, KeyContext,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, SharedString, Size, StyledText, Subscription, UnderlineStyle, Window, canvas,
    div, point, px, size,
};
use gpui_component::scroll::ScrollbarHandle;
use gpui_component::{ActiveTheme as _, WindowExt as _};

use super::data::{COPY_LIMIT, Done, Failure, HexContent, HexData, Op};
use super::model::{self, Byte, EditRefusal, HexModel, ROW, Side, Values};
use crate::core::actions::{
    HEX_KEY_CONTEXT, HexCopy, HexMotion, HexMove, HexPaste, HexRedo, HexToggleSide, HexUndo,
};
use crate::core::config::Config;
use crate::ui::code_editor::DOCUMENT_KEY_CONTEXT;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::scrollbar::with_vertical_scrollbar;
use crate::ui::zoom::zoom_wheel;

/// What the editor's status bar shows for a hex view.
pub(crate) struct HexStatus {
    pub(super) offset: u64,
    /// Length of the selection in bytes.
    pub(super) selection: Option<u64>,
    /// The bytes at the caret (fewer at the end of the file).
    pub(super) values: Values,
    pub(super) len: u64,
}

// ---------------------------------------------------------------------------------------------
// Geometry: pure, so painting and hit-testing share it

/// Columns are laid out on the character grid of the monospace font, so one cell width places
/// every column and answers every click.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    /// Hex digits of the offset column: 8, or 16 for files over 4 GiB.
    offset_digits: u32,
}

/// Characters of margin before the offset column.
const MARGIN: u32 = 1;
/// Characters between the offset column and the hex bytes, and between those and the ASCII.
const COLUMN_GAP: u32 = 2;
/// Bytes after which the hex column leaves an extra space.
const GROUP: u32 = 8;
/// Characters of the hex column: sixteen `XX` with a space after all but the last, plus the
/// extra space in the middle.
const HEX_CHARS: u32 = 16 * 3 - 1 + 1;
const ROW_BYTES: u32 = 16;

impl Grid {
    fn new(len: u64) -> Self {
        Self {
            offset_digits: if len > u64::from(u32::MAX) + 1 { 16 } else { 8 },
        }
    }

    fn offset_start(&self) -> u32 {
        MARGIN
    }

    fn hex_start(&self) -> u32 {
        MARGIN + self.offset_digits + COLUMN_GAP
    }

    fn ascii_start(&self) -> u32 {
        self.hex_start() + HEX_CHARS + COLUMN_GAP
    }

    /// Where the horizontally scrolling part begins: the offset column stays pinned to its left.
    fn scroll_start(&self) -> u32 {
        self.hex_start()
    }

    /// Width of the scrolling part in characters: hex, gap, ASCII and a margin after it.
    fn scroll_chars(&self) -> u32 {
        (self.ascii_start() + ROW_BYTES + MARGIN).saturating_sub(self.scroll_start())
    }

    /// Where byte `i` of a row is drawn on `side`, in characters from `scroll_start`.
    fn cell(&self, i: u32, side: Side) -> Range<u32> {
        match side {
            Side::Hex => {
                let at = self.hex_start().saturating_sub(self.scroll_start())
                    + 3 * i
                    + u32::from(i >= GROUP);
                at..at + 2
            }
            Side::Ascii => {
                let at = self.ascii_start().saturating_sub(self.scroll_start()) + i;
                at..at + 1
            }
        }
    }

    /// The byte (0..16) and column under `x`, in characters from the row's left edge on screen.
    /// `scrolled` is how far the scrolling part is shifted left; the pinned part ignores it.
    fn hit(&self, x: f32, scrolled: f32) -> Option<(u32, Side)> {
        let x = if x < self.scroll_start() as f32 {
            x
        } else {
            x + scrolled
        };
        let hex = self.hex_start() as f32;
        let ascii = self.ascii_start() as f32;
        if x >= hex && x < hex + HEX_CHARS as f32 {
            let rel = (x - hex).floor() as u32;
            // The gap in the middle belongs to the byte before it.
            let i = if rel < 3 * GROUP {
                rel / 3
            } else {
                (rel - 1) / 3
            };
            return Some((i.min(ROW_BYTES - 1), Side::Hex));
        }
        if x >= ascii && x < ascii + ROW_BYTES as f32 {
            return Some(((x - ascii).floor() as u32, Side::Ascii));
        }
        None
    }
}

/// The font's cell size at the editor's zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Metrics {
    row_h: f32,
    cell_w: f32,
}

/// What a measurement depends on: when none of it changed the last one still holds.
#[derive(Clone, Debug, PartialEq)]
struct MeasureKey {
    zoom: f32,
    font_size: f32,
    rem: f32,
    family: SharedString,
}

impl MeasureKey {
    fn read(window: &Window, cx: &App) -> Self {
        Self {
            zoom: cx.global::<Config>().editor_zoom,
            font_size: f32::from(cx.theme().mono_font_size),
            rem: window.rem_size().as_f32(),
            family: cx.theme().mono_font_family.clone(),
        }
    }
}

impl Metrics {
    /// Returns the metrics and the font size they were measured at. `warned` makes a failed
    /// font lookup report once, not on every re-measure.
    fn measure(key: &MeasureKey, window: &Window, warned: &Cell<bool>) -> (Self, Pixels) {
        let font_size = px(key.font_size * key.zoom);
        let mut style = window.text_style();
        style.font_family = key.family.clone();
        style.font_size = font_size.into();
        let row_h = style.line_height_in_pixels(window.rem_size());
        let system = window.text_system();
        let font_id = system.resolve_font(&style.font());
        // A monospace face has one advance; a failed lookup falls back to the usual ratio so the
        // columns still line up with each other.
        let cell_w = system.em_advance(font_id, font_size).unwrap_or_else(|e| {
            if !warned.replace(true) {
                log::warn!("hex: no advance for the editor font, assuming 0.6 em: {e}");
            }
            font_size * 0.6
        });
        (
            Self {
                row_h: row_h.as_f32(),
                cell_w: cell_w.as_f32(),
            },
            font_size,
        )
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self::new(0)
    }
}

/// What the last prepaint measured; hit-testing and the scrollbar read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layout {
    /// The rows' viewport, in window coordinates.
    body: Bounds<Pixels>,
    metrics: Metrics,
    grid: Grid,
    /// Rows in the file.
    rows: u64,
}

impl Layout {
    /// Width of the pinned offset column and the gap after it.
    fn pinned_w(&self) -> f32 {
        self.grid.scroll_start() as f32 * self.metrics.cell_w
    }

    /// Width of the part of the viewport the hex and ASCII columns scroll in.
    fn span_w(&self) -> f32 {
        (self.body.size.width.as_f32() - self.pinned_w()).max(0.)
    }

    /// The furthest the columns can be scrolled: no gap after the last of them.
    fn max_x(&self) -> f32 {
        (self.grid.scroll_chars() as f32 * self.metrics.cell_w - self.span_w()).max(0.)
    }

    fn viewport_h(&self) -> f32 {
        self.body.size.height.as_f32()
    }

    /// Whole rows that fit the viewport, at least one.
    fn page(&self) -> u64 {
        page_rows(self.viewport_h(), self.metrics.row_h)
    }
}

fn page_rows(viewport_h: f32, row_h: f32) -> u64 {
    if row_h <= 0. {
        return 1;
    }
    ((viewport_h / row_h).floor() as u64).max(1)
}

/// The scroll position: the first row shown and how far into it the view is, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Scroll {
    top_row: u64,
    /// `0 <= sub < row height`.
    sub: f32,
    /// How far the hex and ASCII columns are shifted left, in pixels; the offset column stays.
    x: f32,
}

impl Scroll {
    fn set_x(&mut self, x: f32, max: f32) {
        self.x = x.clamp(0., max.max(0.));
    }

    /// The least horizontal scrolling that shows `cell` (pixels from the scrolling part's left
    /// edge) fully in a `span` wide viewport.
    fn reveal_x(&mut self, cell: Range<f32>, span: f32, max: f32) {
        if cell.start < self.x {
            self.set_x(cell.start, max);
        } else if cell.end > self.x + span {
            self.set_x(cell.end - span, max);
        }
    }

    fn max_top(total: u64, page: u64) -> u64 {
        total.saturating_sub(page)
    }

    fn set_top(&mut self, row: u64, total: u64, page: u64) {
        self.top_row = row.min(Self::max_top(total, page));
        self.sub = 0.;
    }

    /// Scrolls by `dy` pixels (positive is down) and carries whole rows into `top_row`.
    fn by_pixels(&mut self, dy: f32, row_h: f32, total: u64, page: u64) {
        if row_h <= 0. {
            return;
        }
        let max = Self::max_top(total, page) as f64;
        let at = self.top_row as f64 + (f64::from(self.sub) + f64::from(dy)) / f64::from(row_h);
        let at = at.clamp(0., max);
        let top = at.floor();
        self.top_row = top as u64;
        self.sub = ((at - top) * f64::from(row_h)) as f32;
    }

    /// The least scrolling that shows `row` fully.
    fn reveal(&mut self, row: u64, total: u64, page: u64) {
        if row < self.top_row || (row == self.top_row && self.sub > 0.) {
            self.set_top(row, total, page);
        } else if row >= self.top_row + page {
            self.set_top(row + 1 - page, total, page);
        }
    }
}

/// The rows the viewport reaches, a partly shown one at the bottom included.
fn visible_rows(scroll: Scroll, viewport_h: f32, row_h: f32, total: u64) -> Range<u64> {
    if row_h <= 0. || viewport_h <= 0. {
        return 0..0;
    }
    let shown = ((viewport_h + scroll.sub) / row_h).ceil() as u64;
    scroll.top_row.min(total)..scroll.top_row.saturating_add(shown).min(total)
}

// ---------------------------------------------------------------------------------------------
// Scrollbar

/// Height of the content the scrollbar is told about, at most. Real content can be 1.3e9 px,
/// where `f32` steps by 128; the bar only needs a proportion.
const VIRTUAL_MAX: f64 = 4_000_000.;

/// The scrollbar's end: the shared scroll position and the last layout.
#[derive(Clone)]
struct HexScrollHandle {
    scroll: Rc<Cell<Scroll>>,
    layout: Rc<Cell<Layout>>,
}

impl HexScrollHandle {
    /// (virtual content height, viewport height, last first row)
    fn geometry(&self) -> (f64, f64, u64) {
        let l = self.layout.get();
        let vh = f64::from(l.viewport_h());
        let real = l.rows as f64 * f64::from(l.metrics.row_h);
        (
            real.min(VIRTUAL_MAX).max(vh),
            vh,
            Scroll::max_top(l.rows, l.page()),
        )
    }
}

impl ScrollbarHandle for HexScrollHandle {
    fn offset(&self) -> Point<Pixels> {
        let (content, vh, max_top) = self.geometry();
        let frac = if max_top == 0 {
            0.
        } else {
            self.scroll.get().top_row as f64 / max_top as f64
        };
        point(px(0.), px(-((frac * (content - vh)) as f32)))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let (content, vh, max_top) = self.geometry();
        let span = content - vh;
        if span <= 0. || max_top == 0 {
            return;
        }
        let travelled = -f64::from(offset.y.as_f32());
        // `f32` cannot say "exactly the end" at this height, so within a pixel of it is the end.
        let frac = if span - travelled <= 1. {
            1.
        } else {
            (travelled / span).clamp(0., 1.)
        };
        let row = (frac * max_top as f64).round() as u64;
        let l = self.layout.get();
        let mut scroll = self.scroll.get();
        scroll.set_top(row, l.rows, l.page());
        self.scroll.set(scroll);
    }

    fn content_size(&self) -> Size<Pixels> {
        let (content, ..) = self.geometry();
        size(px(0.), px(content as f32))
    }
}

// ---------------------------------------------------------------------------------------------
// The view

pub(crate) struct HexView {
    pub(crate) focus: FocusHandle,
    data: Entity<HexData>,
    slot: usize,
    scroll: Rc<Cell<Scroll>>,
    layout: Rc<Cell<Layout>>,
    /// The rows the pages were last asked for, so prepaint defers a request only on a change.
    /// The (generation, first, end) rows last asked for: a reload bumps the generation and
    /// drops every cached page, so the same rows must be asked for again.
    requested: Rc<Cell<Option<(u64, u64, u64)>>>,
    /// Where a mouse drag began, while the button is down.
    drag_from: Option<u64>,
    /// The one edit waiting for a page to land; see `run_op`.
    pending: Option<Op>,
    /// Set when the document changed while an edit was waiting: the next prepaint defers the
    /// retry, because a retry may need the window.
    retry_due: bool,
    /// The last measurement and what it was made for.
    measured: RefCell<Option<(MeasureKey, Metrics, Pixels)>>,
    font_warned: Cell<bool>,
    /// The column numbers above the rows; they never change.
    header: SharedString,
    _data_changed: Subscription,
    /// Rows built by render, for the tests that pin that only visible rows are.
    #[cfg(test)]
    row_builds: Rc<Cell<usize>>,
    /// The messages shown, for the tests (a notification's text is not readable back).
    #[cfg(test)]
    said: Rc<RefCell<Vec<String>>>,
}

impl HexView {
    pub(crate) fn new(data: Entity<HexData>, slot: usize, cx: &mut Context<Self>) -> Self {
        let _data_changed = cx.observe(&data, |this, _, cx| {
            this.retry_due |= this.pending.is_some();
            cx.notify();
        });
        Self {
            focus: cx.focus_handle(),
            data,
            slot,
            scroll: Rc::default(),
            layout: Rc::default(),
            requested: Rc::default(),
            drag_from: None,
            pending: None,
            retry_due: false,
            measured: RefCell::new(None),
            font_warned: Cell::new(false),
            header: header_text(),
            _data_changed,
            #[cfg(test)]
            row_builds: Rc::default(),
            #[cfg(test)]
            said: Rc::default(),
        }
    }

    /// `None` for a file that is too large to open.
    pub(crate) fn status(&self, cx: &App) -> Option<HexStatus> {
        let HexContent::Ready(m) = &self.data.read(cx).content else {
            return None;
        };
        let len = m.store.len();
        let at = m.caret.offset;
        let mut bytes = [0u8; 4];
        let mut filled = 0;
        for (slot, off) in bytes.iter_mut().zip(at..len) {
            let Some(value) = m.store.effective(off) else {
                break;
            };
            *slot = value;
            filled += 1;
        }
        Some(HexStatus {
            offset: at,
            selection: m.caret.selection().map(|r| r.end - r.start),
            values: model::inspect(&bytes[..filled]),
            len,
        })
    }

    /// The metrics for the current font and zoom, measured again only when those changed.
    fn metrics(&self, window: &Window, cx: &App) -> (Metrics, Pixels) {
        let key = MeasureKey::read(window, cx);
        let mut cached = self.measured.borrow_mut();
        if let Some((k, metrics, size)) = cached.as_ref()
            && *k == key
        {
            return (*metrics, *size);
        }
        let (metrics, size) = Metrics::measure(&key, window, &self.font_warned);
        *cached = Some((key, metrics, size));
        (metrics, size)
    }

    fn scroll_handle(&self) -> HexScrollHandle {
        HexScrollHandle {
            scroll: self.scroll.clone(),
            layout: self.layout.clone(),
        }
    }

    /// Asks the document for the pages under the rows on screen and keeps them cached. Runs
    /// from handlers and from the deferred call prepaint schedules, never from paint itself.
    fn request_visible(&mut self, window: &Window, cx: &mut Context<Self>) {
        let l = self.layout.get();
        let rows = visible_rows(self.scroll.get(), l.viewport_h(), l.metrics.row_h, l.rows);
        let generation = self.data.read(cx).generation();
        self.requested.set(Some((generation, rows.start, rows.end)));
        self.data.update(cx, |data, cx| {
            if let HexContent::Ready(m) = &mut data.content {
                m.store.touch_rows(rows.clone());
            }
            data.ensure_rows(rows, window, cx);
        });
    }

    /// Edits the document's caret: `f` gets it and the file length.
    fn with_caret(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut model::Caret, u64)) {
        self.data.update(cx, |data, cx| {
            if let HexContent::Ready(m) = &mut data.content {
                let len = m.store.len();
                f(&mut m.caret, len);
                m.store.pin_caret(m.caret.offset);
            }
            cx.notify();
        });
    }

    /// Scrolls `scroll` sideways the least that shows the caret's cell in the column it is in.
    fn reveal_caret_x(&self, scroll: &mut Scroll, cx: &App) {
        let HexContent::Ready(m) = &self.data.read(cx).content else {
            return;
        };
        // The remainder is under 16, so it always fits.
        let Ok(i) = u32::try_from(m.caret.offset % ROW) else {
            return;
        };
        let l = self.layout.get();
        let cell = Grid::new(m.store.len()).cell(i, m.caret.side);
        let cw = l.metrics.cell_w;
        scroll.reveal_x(
            cell.start as f32 * cw..cell.end as f32 * cw,
            l.span_w(),
            l.max_x(),
        );
    }

    /// After the caret moved by key or click: bring it into view and load what is there.
    fn caret_moved(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (offset, total) = match &self.data.read(cx).content {
            HexContent::Ready(m) => (m.caret.offset, m.store.len().div_ceil(ROW)),
            HexContent::TooLarge { .. } => return,
        };
        let l = self.layout.get();
        let mut scroll = self.scroll.get();
        scroll.reveal(offset / ROW, total, l.page());
        self.reveal_caret_x(&mut scroll, cx);
        self.scroll.set(scroll);
        self.request_visible(window, cx);
        cx.notify();
    }

    fn apply_motion(
        &mut self,
        motion: HexMotion,
        extend: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.pending = None;
        let page = self.layout.get().page();
        self.with_caret(cx, |caret, len| match motion {
            HexMotion::Left => caret.left(len, extend),
            HexMotion::Right => caret.right(len, extend),
            HexMotion::Up => caret.up(len, extend),
            HexMotion::Down => caret.down(len, extend),
            HexMotion::PageUp => caret.page_up(page, len, extend),
            HexMotion::PageDown => caret.page_down(page, len, extend),
            HexMotion::RowStart => caret.row_start(len, extend),
            HexMotion::RowEnd => caret.row_end(len, extend),
            HexMotion::DocStart => caret.doc_start(len, extend),
            HexMotion::DocEnd => caret.doc_end(len, extend),
        });
        self.caret_moved(window, cx);
    }

    /// Does one edit, copy, undo or redo. When a page is missing it asks for it and keeps the op
    /// as `pending`: it is tried once more when the document next changes (a page landing), and
    /// dropped if it is still short while no read is running, so a failed read never loops.
    fn run_op(&mut self, op: Op, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !retry {
            self.pending = None;
        }
        let pasting = match &op {
            Op::Paste(bytes) => bytes.len(),
            _ => 0,
        };
        match self.data.update(cx, |data, cx| data.run(&op, cx)) {
            Ok(Done::Edited) => self.caret_moved(window, cx),
            Ok(Done::Pasted(pasted)) => {
                if pasted.truncated {
                    let left = pasting.saturating_sub(pasted.written);
                    self.say(truncated_notice(pasted.written, left), window, cx);
                }
                self.caret_moved(window, cx);
            }
            Ok(Done::NothingToDo) => {}
            Ok(Done::Copied(text)) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Err(Failure::Edit(EditRefusal::NotLoaded)) => {
                let waiting = self.data.update(cx, |data, cx| {
                    // A retry that is still short must not ask again: a page whose read failed
                    // would be read, fail and be asked for in a loop.
                    if !retry {
                        let pages = data.pages_missing_for(&op);
                        data.ensure_pages(&pages, window, cx);
                    }
                    data.reading()
                });
                self.pending = waiting.then_some(op);
            }
            Err(failure) => {
                if let Some(text) = failure_notice(&failure) {
                    self.say(text, window, cx);
                }
            }
        }
    }

    /// Tells the user something short. The editor's status bar has no slot for a passing
    /// message, so it is a notification.
    fn say(&self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(test)]
        self.said.borrow_mut().push(text.clone());
        window.push_notification(text, cx);
    }

    /// The deferred retry of the edit that was waiting for a page.
    fn retry_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.retry_due = false;
        if let Some(op) = self.pending.take() {
            self.run_op(op, true, window, cx);
        }
    }

    /// Typing: a hex digit in the hex column, a printable character in the ASCII one. Chords with
    /// ctrl, alt or cmd belong to actions and pass through; so does anything that is not an edit.
    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let side = match &self.data.read(cx).content {
            HexContent::Ready(model) => model.caret.side,
            HexContent::TooLarge { .. } => return,
        };
        if matches!(k.key.as_str(), "backspace" | "delete") {
            // Bytes are overwritten, never inserted or removed: say so rather than do nothing.
            self.pending = None;
            self.say(t(L10nKey::HexEditOverwriteOnly).to_string(), window, cx);
            cx.stop_propagation();
            return;
        }
        let mut chars = k.key_char.iter().flat_map(|s| s.chars());
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        let op = match side {
            Side::Hex => match u8::try_from(ch) {
                Ok(digit) if digit.is_ascii_hexdigit() => Op::Hex(digit),
                _ => return,
            },
            // Only what can be typed is ours; Tab, Enter and the rest go on to their bindings.
            Side::Ascii if (' '..='~').contains(&ch) => Op::Ascii(ch),
            Side::Ascii => return,
        };
        cx.stop_propagation();
        self.run_op(op, false, window, cx);
    }

    /// The clipboard's text as bytes for the column the caret is in: hex digits in the hex
    /// column, the text's own UTF-8 bytes in the ASCII one.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let side = match &self.data.read(cx).content {
            HexContent::Ready(model) => model.caret.side,
            HexContent::TooLarge { .. } => return,
        };
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        let bytes = match (side, text) {
            (Side::Ascii, Some(text)) => text.into_bytes(),
            (Side::Hex, Some(text)) => model::parse_hex_pattern(&text).ok().unwrap_or_default(),
            (_, None) => Vec::new(),
        };
        if bytes.is_empty() {
            // No text in the ASCII column is just an empty paste; in the hex column the
            // clipboard held something that is not bytes.
            if side == Side::Hex {
                self.say(t(L10nKey::HexPasteNotHex).to_string(), window, cx);
            }
            return;
        }
        self.run_op(Op::Paste(bytes), false, window, cx);
    }

    fn toggle_side(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        self.data.update(cx, |data, cx| {
            if let HexContent::Ready(m) = &mut data.content {
                m.toggle_side();
            }
            cx.notify();
        });
        // The other column may be off screen; the rows stay where they are.
        let mut scroll = self.scroll.get();
        self.reveal_caret_x(&mut scroll, cx);
        self.scroll.set(scroll);
    }

    fn scroll_wheel(&mut self, ev: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        // The editor's zoom gesture is a wheel turn with the zoom modifier; it is not ours to
        // scroll with.
        if zoom_wheel(cx.global::<Config>().mouse_zoom_modifier, &ev.modifiers) {
            return;
        }
        let l = self.layout.get();
        let delta = ev.delta.pixel_delta(px(l.metrics.row_h));
        let (mut dx, mut dy) = (-delta.x.as_f32(), -delta.y.as_f32());
        // Shift turns a vertical wheel into a horizontal one.
        if ev.modifiers.shift {
            dx += dy;
            dy = 0.;
        }
        let mut scroll = self.scroll.get();
        if dx != 0. {
            scroll.set_x(scroll.x + dx, l.max_x());
        }
        if dy != 0. {
            scroll.by_pixels(dy, l.metrics.row_h, l.rows, l.page());
        }
        self.scroll.set(scroll);
        self.request_visible(window, cx);
        cx.notify();
    }

    /// Pulls the horizontal offset back after the viewport grew, the zoom shrank or the file
    /// got a narrower offset column, so no empty gap is left at the right.
    fn clamp_x(&mut self, cx: &mut Context<Self>) {
        let mut scroll = self.scroll.get();
        scroll.set_x(scroll.x, self.layout.get().max_x());
        self.scroll.set(scroll);
        cx.notify();
    }

    /// The byte and column under a window position, by the metrics the last paint used.
    fn hit(&self, position: Point<Pixels>, len: u64) -> Option<(u64, Side)> {
        let l = self.layout.get();
        let (cell_w, row_h) = (l.metrics.cell_w, l.metrics.row_h);
        if cell_w <= 0. || row_h <= 0. {
            return None;
        }
        let x = (position.x - l.body.origin.x).as_f32() / cell_w;
        let y = (position.y - l.body.origin.y).as_f32();
        let scroll = self.scroll.get();
        let row = scroll.top_row + ((y + scroll.sub) / row_h).floor().max(0.) as u64;
        let (i, side) = Grid::new(len).hit(x, scroll.x / cell_w)?;
        let offset = row.checked_mul(ROW)?.checked_add(u64::from(i))?;
        (offset < len).then_some((offset, side))
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pending = None;
        let len = self.data.read(cx).len().unwrap_or(0);
        let Some((offset, side)) = self.hit(ev.position, len) else {
            return;
        };
        let extend = ev.modifiers.shift;
        self.drag_from = Some(offset);
        self.with_caret(cx, |caret, _| {
            if extend {
                caret.anchor.get_or_insert(caret.offset);
            } else {
                caret.anchor = None;
            }
            caret.offset = offset;
            caret.side = side;
            caret.low_nibble = false;
        });
        self.caret_moved(window, cx);
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(from) = self.drag_from else {
            return;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        let len = self.data.read(cx).len().unwrap_or(0);
        let Some((offset, _)) = self.hit(ev.position, len) else {
            return;
        };
        self.with_caret(cx, |caret, _| {
            caret.anchor = (offset != from).then_some(from);
            caret.offset = offset;
            caret.low_nibble = false;
        });
        self.caret_moved(window, cx);
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag_from = None;
    }
}

/// The words for a paste that ran past the end of the file.
fn truncated_notice(written: usize, left: usize) -> String {
    t_fmt(
        L10nKey::HexPasteTruncated,
        &[
            ("written", &written.to_string()),
            ("left", &left.to_string()),
        ],
    )
}

/// What the user is told when an op did nothing. A character that is not a digit, or not
/// printable, is not worth a notice per keystroke, and a missing page is waited for.
fn failure_notice(failure: &Failure) -> Option<String> {
    match failure {
        Failure::Edit(EditRefusal::NothingToEdit) => Some(t(L10nKey::HexEditEmpty).to_string()),
        Failure::Edit(
            EditRefusal::NotLoaded | EditRefusal::NotHexDigit | EditRefusal::NotPrintable,
        ) => None,
        Failure::CopyTooLarge => Some(t_fmt(
            L10nKey::HexCopyTooLarge,
            &[(
                "limit",
                &crate::ui::image_preview::view::human_size(COPY_LIMIT),
            )],
        )),
    }
}

// ---------------------------------------------------------------------------------------------
// Rows

/// `00 01 … 0F` laid out like a row's hex column.
fn header_text() -> SharedString {
    let mut header = String::with_capacity(HEX_CHARS as usize);
    for i in 0..ROW_BYTES {
        if i > 0 {
            header.push(' ');
        }
        if i == GROUP {
            header.push(' ');
        }
        header.push('0');
        header.extend(char::from_digit(i, 16).map(|c| c.to_ascii_uppercase()));
    }
    header.into()
}

/// One row as three texts, each with its highlights.
struct RowText {
    offset: String,
    hex: String,
    hex_marks: Vec<(Range<usize>, HighlightStyle)>,
    ascii: String,
    ascii_marks: Vec<(Range<usize>, HighlightStyle)>,
}

/// How the bytes of a row are marked.
struct Marks {
    selection: HighlightStyle,
    caret_here: HighlightStyle,
    caret_other: HighlightStyle,
    pending: HighlightStyle,
    /// The text colour of a byte that differs from the disk.
    edited: Hsla,
    /// The same colour as an underline on a highlighted byte, floored against the selection.
    edited_mark: Hsla,
}

fn row_text(m: &HexModel, row: u64, grid: Grid, marks: &Marks) -> RowText {
    let len = m.store.len();
    let selection = m.caret.selection();
    let mut out = RowText {
        offset: String::with_capacity(grid.offset_digits as usize),
        hex: String::with_capacity(HEX_CHARS as usize),
        hex_marks: Vec::new(),
        ascii: String::with_capacity(ROW_BYTES as usize),
        ascii_marks: Vec::new(),
    };
    model::write_offset(&mut out.offset, row * ROW, grid.offset_digits > 8);
    for i in 0..ROW_BYTES {
        let off = row * ROW + u64::from(i);
        if off >= len {
            break;
        }
        if i > 0 {
            out.hex.push(' ');
        }
        if i == GROUP {
            out.hex.push(' ');
        }
        let (hex_start, ascii_start) = (out.hex.len(), out.ascii.len());
        let byte = m.store.byte(off);
        let pending = match byte {
            Byte::Loaded { value, .. } => {
                let [hi, lo] = model::hex_byte(value);
                out.hex.push(char::from(hi));
                out.hex.push(char::from(lo));
                out.ascii.push(model::ascii_char(value));
                false
            }
            Byte::Pending => {
                out.hex.push_str("··");
                out.ascii.push('·');
                true
            }
        };
        let is_caret = off == m.caret.offset;
        let selected = selection.as_ref().is_some_and(|r| r.contains(&off));
        let edited = matches!(byte, Byte::Loaded { edited: true, .. });
        let style = |side: Side| {
            let base = if is_caret {
                Some(if m.caret.side == side {
                    marks.caret_here
                } else {
                    marks.caret_other
                })
            } else if selected {
                Some(marks.selection)
            } else if pending {
                Some(marks.pending)
            } else {
                None
            };
            if !edited {
                return base;
            }
            // The edit colour is text, and text in it has to clear the editor background but
            // not the selection's, which it misses in some themes. On a highlighted byte the
            // text stays the normal foreground and an underline (a mark, a lower floor)
            // carries the edit; the caret's own underline wins where it is already there.
            Some(match base {
                Some(b) if b.background_color.is_some() => HighlightStyle {
                    underline: b.underline.or(Some(UnderlineStyle {
                        thickness: px(2.),
                        color: Some(marks.edited_mark),
                        wavy: false,
                    })),
                    ..b
                },
                other => HighlightStyle {
                    color: Some(marks.edited),
                    ..other.unwrap_or_default()
                },
            })
        };
        let push = |list: &mut Vec<(Range<usize>, HighlightStyle)>,
                    start: usize,
                    end: usize,
                    style: Option<HighlightStyle>| {
            if let Some(style) = style {
                list.push((start..end, style));
            }
        };
        push(
            &mut out.hex_marks,
            hex_start,
            out.hex.len(),
            style(Side::Hex),
        );
        push(
            &mut out.ascii_marks,
            ascii_start,
            out.ascii.len(),
            style(Side::Ascii),
        );
        // A pending byte under a caret or selection keeps its dim text.
        if pending && (is_caret || selected) {
            for list in [&mut out.hex_marks, &mut out.ascii_marks] {
                if let Some((_, style)) = list.last_mut() {
                    style.color = marks.pending.color;
                }
            }
        }
    }
    out
}

impl Render for HexView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.read(cx);
        let HexContent::Ready(m) = &data.content else {
            // hx-doc draws the error card for a file that is too large.
            return div().into_any_element();
        };
        let (metrics, font_size) = self.metrics(window, cx);
        let len = m.store.len();
        let total = len.div_ceil(ROW);
        let grid = Grid::new(len);
        let theme = cx.theme();
        let (fg, muted, bg) = (theme.foreground, theme.muted_foreground, theme.background);
        let selection = HighlightStyle {
            background_color: Some(theme.selection),
            ..Default::default()
        };
        let marks = Marks {
            selection,
            // On top of a selection the caret must still show: the underline is what tells it
            // from the selected bytes around it.
            caret_here: HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(2.),
                    color: Some(theme.caret),
                    wavy: false,
                }),
                ..selection
            },
            caret_other: HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(theme.caret),
                    wavy: false,
                }),
                ..Default::default()
            },
            pending: HighlightStyle {
                color: Some(muted),
                ..Default::default()
            },
            // The same token the source-control panel uses for a modified file.
            edited: theme.warning,
            edited_mark: gpui::rgb(crate::ui::theme::semantics(cx).warning_mark).into(),
        };

        let measured = self.layout.get();
        let viewport_h = if measured.viewport_h() > 0. {
            measured.viewport_h()
        } else {
            // Before the first paint: more than the body can show, built once.
            window.viewport_size().height.as_f32()
        };
        let scroll = self.scroll.get();
        let rows = visible_rows(scroll, viewport_h, metrics.row_h, total);

        let at = |chars: u32| px(chars as f32 * metrics.cell_w);
        let column = |left: Pixels, text: StyledText, color| {
            div()
                .absolute()
                .left(left)
                .whitespace_nowrap()
                .text_color(color)
                .child(text)
        };
        // The hex and ASCII columns sit in a clip to the right of the pinned offset column and
        // are shifted left by the horizontal scroll.
        let shifted = |chars: u32| at(chars.saturating_sub(grid.scroll_start())) - px(scroll.x);
        let clip = || {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(at(grid.scroll_start()))
                .right_0()
                .overflow_hidden()
        };

        let mut body = div()
            .id(("hex-body", self.slot))
            .relative()
            .size_full()
            .overflow_hidden()
            .cursor_text()
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, window, cx| {
                this.scroll_wheel(ev, window, cx)
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.mouse_down(ev, window, cx)
                }),
            )
            .on_mouse_move(
                cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                    this.mouse_move(ev, window, cx)
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| this.mouse_up(ev, window, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| this.mouse_up(ev, window, cx)),
            );
        for row in rows.clone() {
            #[cfg(test)]
            self.row_builds.set(self.row_builds.get() + 1);
            let text = row_text(m, row, grid, &marks);
            let y = px((row - scroll.top_row) as f32 * metrics.row_h - scroll.sub);
            body = body.child(
                div()
                    .absolute()
                    .top(y)
                    .left_0()
                    .w_full()
                    .h(px(metrics.row_h))
                    .child(
                        column(at(grid.offset_start()), StyledText::new(text.offset), muted)
                            .debug_selector(|| "hex-offset-column".into()),
                    )
                    .child(
                        clip()
                            .child(
                                column(
                                    shifted(grid.hex_start()),
                                    StyledText::new(text.hex).with_highlights(text.hex_marks),
                                    fg,
                                )
                                .debug_selector(|| "hex-hex-column".into()),
                            )
                            .child(column(
                                shifted(grid.ascii_start()),
                                StyledText::new(text.ascii).with_highlights(text.ascii_marks),
                                fg,
                            )),
                    ),
            );
        }

        // The measuring canvas: records the viewport and, when the rows on screen are not the
        // ones last asked for (first frame, resize, a scrollbar drag), defers the request so no
        // document change happens during paint.
        let (layout, requested, view) = (self.layout.clone(), self.requested.clone(), cx.entity());
        let rows_total = total;
        let measure = canvas(
            move |bounds, window, cx| {
                let measured = Layout {
                    body: bounds,
                    metrics,
                    grid,
                    rows: rows_total,
                };
                layout.set(measured);
                if view.read(cx).scroll.get().x > measured.max_x() {
                    let view = view.clone();
                    window.defer(cx, move |_, cx| {
                        view.update(cx, |this, cx| this.clamp_x(cx));
                    });
                }
                if view.read(cx).retry_due {
                    let view = view.clone();
                    window.defer(cx, move |window, cx| {
                        view.update(cx, |this, cx| this.retry_pending(window, cx));
                    });
                }
                let (top, h) = (view.read(cx).scroll.get(), bounds.size.height.as_f32());
                let range = visible_rows(top, h, metrics.row_h, rows_total);
                let generation = view.read(cx).data.read(cx).generation();
                let key = Some((generation, range.start, range.end));
                if requested.get() != key {
                    requested.set(key);
                    let view = view.clone();
                    window.defer(cx, move |window, cx| {
                        view.update(cx, |this, cx| this.request_visible(window, cx));
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        // The header: the column numbers, sticky because it is outside the rows.
        let header = div()
            .relative()
            .w_full()
            // Clipped like the body: the absolutely placed column would otherwise paint past a
            // narrow panel's edge.
            .overflow_hidden()
            .h(px(metrics.row_h))
            .flex_none()
            .child(clip().child(column(
                shifted(grid.hex_start()),
                StyledText::new(self.header.clone()),
                muted,
            )));

        let mut context = KeyContext::default();
        context.add(DOCUMENT_KEY_CONTEXT);
        context.add(HEX_KEY_CONTEXT);
        div()
            .id(("hex-view", self.slot))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(font_size)
            .line_height(px(metrics.row_h))
            .key_context(context)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, a: &HexMove, window, cx| {
                this.apply_motion(a.motion, a.extend, window, cx)
            }))
            .on_action(cx.listener(|this, _: &HexToggleSide, _, cx| this.toggle_side(cx)))
            .on_action(
                cx.listener(|this, _: &HexUndo, window, cx| {
                    this.run_op(Op::Undo, false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &HexRedo, window, cx| {
                    this.run_op(Op::Redo, false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &HexCopy, window, cx| {
                    this.run_op(Op::Copy, false, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &HexPaste, window, cx| this.paste(window, cx)))
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.key_down(ev, window, cx)),
            )
            .child(header)
            .child(with_vertical_scrollbar(
                ("hex-scrollbar", self.slot),
                body.child(measure),
                &self.scroll_handle(),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext};

    use super::super::model::{PAGE, Source};
    use super::super::test_support::{Counting, paged};
    use super::*;
    use crate::core::config::MouseZoomModifier;

    const GIB: u64 = 1 << 30;

    struct Rig {
        view: Entity<HexView>,
        data: Entity<HexData>,
        host: Arc<Counting>,
        _dir: tempfile::TempDir,
    }

    /// Draws until the page reads the frames asked for have landed. They run on a real thread,
    /// so the scheduler may park and the loop polls, bounded at ten seconds.
    fn settle(vcx: &mut VisualTestContext, data: &Entity<HexData>) {
        vcx.executor().allow_parking();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut quiet = 0;
        while quiet < 3 && Instant::now() < deadline {
            vcx.run_until_parked();
            let _ = vcx.update(|window, cx| window.draw(cx));
            vcx.run_until_parked();
            if data.read_with(vcx, |d, _| d.reading()) {
                quiet = 0;
                std::thread::sleep(Duration::from_millis(2));
            } else {
                quiet += 1;
            }
        }
    }

    /// A view over a real file of `len` bytes starting with `head`, drawn and settled.
    fn rig<'a>(
        cx: &'a mut TestAppContext,
        len: u64,
        head: &[u8],
    ) -> (Rig, &'a mut VisualTestContext) {
        rig_in(cx, len, head, false)
    }

    /// Like `rig`, with the view under a `Root` so notifications have somewhere to go.
    fn rig_rooted<'a>(
        cx: &'a mut TestAppContext,
        len: u64,
        head: &[u8],
    ) -> (Rig, &'a mut VisualTestContext) {
        rig_in(cx, len, head, true)
    }

    fn rig_in<'a>(
        cx: &'a mut TestAppContext,
        len: u64,
        head: &[u8],
        rooted: bool,
    ) -> (Rig, &'a mut VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            cx.set_global(crate::ui::presets::builtins()[0].semantics());
        });
        let host = Counting::new();
        let (data, dir) = paged(len, head, &host);
        let data = cx.new(|_| data);
        let made: Rc<RefCell<Option<Entity<HexView>>>> = Rc::default();
        let (view, vcx) = if rooted {
            let (_, vcx) = cx.add_window_view({
                let (data, made) = (data.clone(), made.clone());
                move |window, cx| {
                    let view = cx.new(|cx| HexView::new(data, 0, cx));
                    *made.borrow_mut() = Some(view.clone());
                    gpui_component::Root::new(view, window, cx)
                }
            });
            (made.take().expect("the window made its view"), vcx)
        } else {
            cx.add_window_view({
                let data = data.clone();
                move |_, cx| HexView::new(data, 0, cx)
            })
        };
        settle(vcx, &data);
        let rig = Rig {
            view,
            data,
            host,
            _dir: dir,
        };
        (rig, vcx)
    }

    fn layout(rig: &Rig, vcx: &VisualTestContext) -> Layout {
        rig.view.read_with(vcx, |v, _| v.layout.get())
    }

    fn scroll(rig: &Rig, vcx: &VisualTestContext) -> Scroll {
        rig.view.read_with(vcx, |v, _| v.scroll.get())
    }

    fn status(rig: &Rig, vcx: &VisualTestContext) -> HexStatus {
        rig.view
            .read_with(vcx, |v, cx| v.status(cx))
            .expect("a ready document has a status")
    }

    fn motion(rig: &Rig, vcx: &mut VisualTestContext, m: HexMotion, extend: bool) {
        rig.view
            .update_in(vcx, |v, window, cx| v.apply_motion(m, extend, window, cx));
        settle(vcx, &rig.data);
    }

    fn set_zoom(vcx: &mut VisualTestContext, zoom: f32) {
        vcx.update(|_, cx| cx.update_global::<Config, _>(|c, _| c.0.editor_zoom = zoom));
    }

    fn redraw(rig: &Rig, vcx: &mut VisualTestContext) {
        rig.view.update(vcx, |_, cx| cx.notify());
        settle(vcx, &rig.data);
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    // --- rows ----------------------------------------------------------------------------

    /// A 1 GiB file is 67 million rows; a frame builds the few the window shows. Guards the
    /// render loop running over `visible_rows` rather than every row.
    #[gpui::test]
    fn only_the_rows_on_screen_are_built_for_a_one_gib_file(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, GIB, &[]);
        rig.view.update(vcx, |v, cx| {
            v.row_builds.set(0);
            cx.notify();
        });
        let _ = vcx.update(|window, cx| window.draw(cx));
        let built = rig.view.read_with(vcx, |v, _| v.row_builds.get());
        let l = layout(&rig, vcx);
        let shown = visible_rows(scroll(&rig, vcx), l.viewport_h(), l.metrics.row_h, l.rows);
        assert!(l.viewport_h() > 0., "the view was laid out");
        assert_eq!(l.rows, GIB / ROW);
        assert!(built > 0, "a frame was drawn");
        let per_frame = shown.count();
        assert!(
            per_frame > 0 && per_frame < 200,
            "{per_frame} rows on screen"
        );
        assert!(
            built >= per_frame && built % per_frame == 0 && built / per_frame <= 3,
            "{built} rows built over a few frames of {per_frame}"
        );
    }

    /// The first pending bytes draw as dim dots, loaded ones as hex and ASCII, with the extra
    /// gap after the eighth byte. Guards `row_text`'s two arms.
    #[test]
    fn rows_show_hex_and_ascii_and_dots_for_pending_bytes() {
        let marks = Marks {
            selection: HighlightStyle::default(),
            caret_here: HighlightStyle::default(),
            caret_other: HighlightStyle::default(),
            pending: HighlightStyle {
                color: Some(gpui::hsla(0.5, 0.5, 0.5, 1.)),
                ..Default::default()
            },
            edited: gpui::hsla(0.1, 0.9, 0.5, 1.),
            edited_mark: gpui::hsla(0.1, 0.9, 0.4, 1.),
        };
        let bytes: Vec<u8> = (0x41u8..0x51).collect();
        let loaded = HexModel::new(Source::Whole(Arc::from(bytes)));
        let row = row_text(&loaded, 0, Grid::new(16), &marks);
        assert_eq!(row.offset, "00000000");
        assert_eq!(row.hex, "41 42 43 44 45 46 47 48  49 4A 4B 4C 4D 4E 4F 50");
        assert_eq!(row.ascii, "ABCDEFGHIJKLMNOP");

        let unread = HexModel::new(Source::Paged { len: 40 });
        let row = row_text(&unread, 2, Grid::new(40), &marks);
        assert_eq!(row.offset, "00000020");
        assert_eq!(row.hex, "·· ·· ·· ·· ·· ·· ·· ··");
        assert_eq!(row.ascii, "········");
        assert!(
            row.hex_marks
                .iter()
                .all(|(_, s)| s.color == marks.pending.color),
            "pending bytes are muted"
        );
    }

    /// Past 4 GiB the offset column is 16 digits wide and the columns move over by that much.
    /// Guards `Grid::new`'s threshold and the shared column origins.
    #[test]
    fn the_offset_column_widens_past_four_gib() {
        let narrow = Grid::new(4 * GIB);
        let wide = Grid::new(4 * GIB + 1);
        assert_eq!(narrow.offset_digits, 8);
        assert_eq!(wide.offset_digits, 16);
        assert_eq!(wide.hex_start() - narrow.hex_start(), 8);
        assert_eq!(wide.ascii_start() - narrow.ascii_start(), 8);
    }

    // --- scroll position -----------------------------------------------------------------

    /// Wheel pixels pile up inside a row and carry whole rows into `top_row`, in either
    /// direction, and stop at both ends. Guards `Scroll::by_pixels`.
    #[test]
    fn wheel_deltas_accumulate_and_carry_whole_rows() {
        let mut s = Scroll::default();
        for _ in 0..3 {
            s.by_pixels(10., 20., 100, 10);
        }
        assert_eq!((s.top_row, s.sub), (1, 10.));
        s.by_pixels(-30., 20., 100, 10);
        assert_eq!((s.top_row, s.sub), (0, 0.));
        s.by_pixels(-500., 20., 100, 10);
        assert_eq!((s.top_row, s.sub), (0, 0.), "not above the first row");
        s.by_pixels(1e9, 20., 100, 10);
        assert_eq!((s.top_row, s.sub), (90, 0.), "the last page, whole rows");
    }

    /// Deep in a file a one-row scroll is still one row: `top_row` is `u64`, not pixels.
    /// Guards the scroll position being rows, not `f32` pixels.
    #[test]
    fn a_one_row_scroll_at_the_end_of_a_huge_file_moves_by_one_row() {
        let total = GIB / ROW;
        let mut s = Scroll::default();
        s.set_top(total - 50, total, 40);
        s.by_pixels(20., 20., total, 40);
        assert_eq!(s.top_row, total - 49);
        s.by_pixels(-20., 20., total, 40);
        assert_eq!(s.top_row, total - 50);
    }

    /// Keys scroll the least that shows the row. Guards `Scroll::reveal`.
    #[test]
    fn revealing_a_row_scrolls_the_least_that_shows_it() {
        let mut s = Scroll {
            top_row: 10,
            sub: 5.,
            ..Scroll::default()
        };
        s.reveal(12, 100, 8);
        assert_eq!((s.top_row, s.sub), (10, 5.), "inside: untouched");
        s.reveal(30, 100, 8);
        assert_eq!(s.top_row, 23, "below: the row is the last one shown");
        s.reveal(3, 100, 8);
        assert_eq!(s.top_row, 3, "above: the row is the first one shown");
        s.sub = 4.;
        s.reveal(3, 100, 8);
        assert_eq!(
            (s.top_row, s.sub),
            (3, 0.),
            "a half-hidden first row is shown whole"
        );
    }

    /// Rows reached include the part-shown one at the bottom and stop at the end of the file.
    #[test]
    fn the_visible_rows_include_the_partly_shown_one_and_stop_at_the_end() {
        let s = Scroll {
            top_row: 5,
            sub: 10.,
            ..Scroll::default()
        };
        assert_eq!(visible_rows(s, 100., 20., 1000), 5..11);
        assert_eq!(visible_rows(s, 100., 20., 8), 5..8);
        assert_eq!(visible_rows(s, 0., 20., 8), 0..0);
    }

    /// The scrollbar's virtual height stays inside what `f32` can step through, and dragging
    /// the thumb to either end or the middle lands on the matching row, in a 1 GiB file.
    /// Guards `HexScrollHandle`'s fraction mapping.
    #[test]
    fn the_scrollbar_maps_the_thumb_to_rows_in_a_one_gib_file() {
        let rows = GIB / ROW;
        let layout = Layout {
            body: Bounds::new(point(px(0.), px(0.)), size(px(600.), px(600.))),
            metrics: Metrics {
                row_h: 20.,
                cell_w: 8.,
            },
            rows,
            ..Layout::default()
        };
        let handle = HexScrollHandle {
            scroll: Rc::default(),
            layout: Rc::new(Cell::new(layout)),
        };
        let content = f64::from(handle.content_size().height.as_f32());
        assert!(content <= VIRTUAL_MAX, "{content}");
        let span = content - 600.;
        let max_top = rows - 30;

        handle.set_offset(point(px(0.), px(-(span as f32))));
        assert_eq!(handle.scroll.get().top_row, max_top, "the end");
        assert!((f64::from(handle.offset().y.as_f32()) + span).abs() < 2.);

        handle.set_offset(point(px(0.), px(-(span as f32) / 2.)));
        let mid = handle.scroll.get().top_row;
        assert!(
            mid.abs_diff(max_top / 2) < 100,
            "{mid} is about half of {max_top}"
        );

        handle.set_offset(point(px(0.), px(0.)));
        assert_eq!(handle.scroll.get().top_row, 0);
        handle.set_offset(point(px(0.), px(1e9)));
        assert_eq!(
            handle.scroll.get().top_row,
            0,
            "no further back than the first row"
        );
    }

    // --- loading -------------------------------------------------------------------------

    /// Going to the end of a 1 GiB file reads the last page and nothing between, and scrolls
    /// the caret into view. Guards `caret_moved` -> `request_visible` -> `ensure_rows` asking
    /// for the rows on screen and no others.
    #[gpui::test]
    fn going_to_the_end_of_a_one_gib_file_reads_only_the_last_page(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, GIB, &[]);
        assert!(
            rig.host.reads().is_empty(),
            "the first page came with the file"
        );
        motion(&rig, vcx, HexMotion::DocEnd, false);
        let last_page = (GIB - 1) / PAGE as u64 * PAGE as u64;
        assert_eq!(rig.host.reads(), vec![last_page]);
        let l = layout(&rig, vcx);
        assert_eq!(scroll(&rig, vcx).top_row, l.rows - l.page());
        assert_eq!(status(&rig, vcx).offset, GIB - 1);
        rig.data.read_with(vcx, |d, _| match &d.content {
            HexContent::Ready(m) => assert!(matches!(m.store.byte(GIB - 1), Byte::Loaded { .. })),
            HexContent::TooLarge { .. } => panic!("too large"),
        });
    }

    /// Dragging the scrollbar thumb to the end repaints and loads the last page, although the
    /// drag only writes the shared scroll cell. Guards the prepaint-deferred request.
    #[gpui::test]
    fn dragging_the_scrollbar_to_the_end_loads_the_last_page(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, GIB, &[]);
        let handle = rig.view.read_with(vcx, |v, _| v.scroll_handle());
        handle.set_offset(point(px(0.), px(-1e9)));
        redraw(&rig, vcx);
        let last_page = (GIB - 1) / PAGE as u64 * PAGE as u64;
        assert_eq!(rig.host.reads(), vec![last_page]);
        let l = layout(&rig, vcx);
        assert_eq!(scroll(&rig, vcx).top_row, l.rows - l.page());
    }

    /// A reload drops every cached page, and the rows still on screen are asked for again
    /// without any scroll. Guards the generation in the `requested` key (request_visible and
    /// the prepaint check): with only the rows in it the second read never happens.
    #[gpui::test]
    fn a_reload_loads_the_visible_rows_again_without_a_scroll(cx: &mut TestAppContext) {
        use super::super::io::{HexFile, HexSource};
        let (rig, vcx) = rig(cx, GIB, &[]);
        let handle = rig.view.read_with(vcx, |v, _| v.scroll_handle());
        handle.set_offset(point(px(0.), px(-1e9)));
        redraw(&rig, vcx);
        let last_page = (GIB - 1) / PAGE as u64 * PAGE as u64;
        assert_eq!(rig.host.reads(), vec![last_page]);

        rig.data.update(vcx, |d, cx| {
            let file = HexFile {
                path: d.path.clone(),
                source: HexSource::Paged {
                    len: GIB,
                    first_page: vec![0; PAGE],
                },
                mtime: None,
                len: GIB,
                opens_as_text: false,
            };
            d.apply_reload(file, cx);
        });
        settle(vcx, &rig.data);
        assert_eq!(rig.host.reads(), vec![last_page, last_page]);
    }

    // --- keys ----------------------------------------------------------------------------

    /// Each motion moves the caret as the model says. Guards `apply_motion`'s mapping of every
    /// `HexMotion` to its model move.
    #[gpui::test]
    fn the_motions_move_the_caret_as_the_model_says(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100_000, &[]);
        let page = layout(&rig, vcx).page();
        for (m, expect) in [
            (HexMotion::Right, 1),
            (HexMotion::Down, 17),
            (HexMotion::Left, 16),
            (HexMotion::Up, 0),
            (HexMotion::RowEnd, 15),
            (HexMotion::RowStart, 0),
            (HexMotion::PageDown, page * ROW),
            (HexMotion::PageUp, 0),
            (HexMotion::DocEnd, 99_999),
            (HexMotion::DocStart, 0),
        ] {
            motion(&rig, vcx, m, false);
            assert_eq!(status(&rig, vcx).offset, expect, "{m:?}");
        }
    }

    /// Shift extends a selection from where the caret was, and its length is in the status.
    /// Guards `extend` reaching the caret moves.
    #[gpui::test]
    fn shift_motions_select(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 1000, &[]);
        motion(&rig, vcx, HexMotion::Right, true);
        motion(&rig, vcx, HexMotion::Right, true);
        let s = status(&rig, vcx);
        assert_eq!((s.offset, s.selection), (2, Some(3)));
        motion(&rig, vcx, HexMotion::Left, false);
        assert_eq!(status(&rig, vcx).selection, None, "a plain move clears it");
    }

    /// Tab switches between the hex and ASCII columns. Guards `toggle_side`.
    #[gpui::test]
    fn tab_toggles_the_column(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[]);
        let side = |rig: &Rig, vcx: &VisualTestContext| {
            rig.data.read_with(vcx, |d, _| match &d.content {
                HexContent::Ready(m) => m.caret.side,
                HexContent::TooLarge { .. } => panic!("too large"),
            })
        };
        assert_eq!(side(&rig, vcx), Side::Hex);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        assert_eq!(side(&rig, vcx), Side::Ascii);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        assert_eq!(side(&rig, vcx), Side::Hex);
    }

    // --- editing -------------------------------------------------------------------------

    fn focus(rig: &Rig, vcx: &mut VisualTestContext) {
        rig.view
            .update_in(vcx, |v, window, cx| window.focus(&v.focus, cx));
    }

    /// A key-down as the window delivers it, straight to the view's handler.
    fn key(
        rig: &Rig,
        vcx: &mut VisualTestContext,
        key: &str,
        key_char: Option<&str>,
        modifiers: Modifiers,
    ) {
        let ev = KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers,
                key: key.into(),
                key_char: key_char.map(Into::into),
            },
            is_held: false,
            prefer_character_input: false,
        };
        rig.view
            .update_in(vcx, |v, window, cx| v.key_down(&ev, window, cx));
        settle(vcx, &rig.data);
    }

    /// Types each character as a plain key.
    fn type_text(rig: &Rig, vcx: &mut VisualTestContext, text: &str) {
        for ch in text.chars() {
            let s = ch.to_string();
            key(rig, vcx, &s, Some(&s), Modifiers::default());
        }
    }

    fn dispatch(rig: &Rig, vcx: &mut VisualTestContext, action: impl gpui::Action) {
        focus(rig, vcx);
        vcx.dispatch_action(action);
        settle(vcx, &rig.data);
    }

    /// The bytes as the user sees them, edits applied.
    fn bytes(rig: &Rig, vcx: &VisualTestContext, range: Range<u64>) -> Vec<Option<u8>> {
        rig.data.read_with(vcx, |d, _| match &d.content {
            HexContent::Ready(m) => range.map(|off| m.store.effective(off)).collect(),
            HexContent::TooLarge { .. } => panic!("too large"),
        })
    }

    fn dirty(rig: &Rig, vcx: &VisualTestContext) -> bool {
        rig.data.read_with(vcx, |d, _| d.dirty())
    }

    /// The messages the view has shown so far.
    fn said(rig: &Rig, vcx: &VisualTestContext) -> Vec<String> {
        rig.view.read_with(vcx, |v, _| v.said.borrow().clone())
    }

    fn put_on_clipboard(vcx: &mut VisualTestContext, text: &str) {
        vcx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    fn clipboard(vcx: &mut VisualTestContext) -> Option<String> {
        vcx.read_from_clipboard().and_then(|item| item.text())
    }

    fn select_first(rig: &Rig, vcx: &mut VisualTestContext, count: u64) {
        for _ in 1..count {
            motion(rig, vcx, HexMotion::Right, true);
        }
    }

    /// Typing `4` then `1` on the hex side writes 0x41 and moves on, through the window's own
    /// key dispatch. Guards `.on_key_down` on the view, `key_down`'s hex arm and `Op::Hex`
    /// reaching `type_hex`.
    #[gpui::test]
    fn typing_two_hex_digits_writes_a_byte_and_advances(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[0, 0]);
        focus(&rig, vcx);
        vcx.simulate_keystrokes("4 1");
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0x41), Some(0)]);
        assert_eq!(
            status(&rig, vcx).offset,
            1,
            "the caret moved to the next byte"
        );
        assert!(dirty(&rig, vcx));
    }

    /// Typing `A` on the ASCII side writes 0x41; the hex side ignores a character that is not
    /// a digit. Guards `key_down`'s ASCII arm and its digit filter.
    #[gpui::test]
    fn typing_a_character_on_the_ascii_side_writes_its_byte(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[]);
        key(&rig, vcx, "g", Some("g"), Modifiers::default());
        assert!(!dirty(&rig, vcx), "g is not a hex digit");
        assert!(said(&rig, vcx).is_empty(), "and not worth a notice");
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        key(&rig, vcx, "a", Some("A"), Modifiers::shift());
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0x41), Some(0)]);
        assert_eq!(status(&rig, vcx).offset, 1);
        key(&rig, vcx, "enter", Some("\n"), Modifiers::default());
        assert_eq!(
            bytes(&rig, vcx, 1..2),
            [Some(0)],
            "a control character is not typed"
        );
    }

    /// On the ASCII side Tab still switches the column and Enter writes nothing: only printable
    /// characters are typed, the rest go on to their bindings. Guards the printable filter in
    /// `key_down`.
    #[gpui::test]
    fn tab_and_enter_are_not_typed_on_the_ascii_side(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[]);
        vcx.update(|_, cx| {
            cx.bind_keys([gpui::KeyBinding::new(
                "tab",
                HexToggleSide,
                Some(HEX_KEY_CONTEXT),
            )]);
        });
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        focus(&rig, vcx);
        vcx.simulate_keystrokes("tab");
        let side = rig.data.read_with(vcx, |d, _| match &d.content {
            HexContent::Ready(m) => m.caret.side,
            HexContent::TooLarge { .. } => panic!("too large"),
        });
        assert_eq!(side, Side::Hex, "Tab switched the column");
        key(&rig, vcx, "enter", Some("\n"), Modifiers::default());
        key(&rig, vcx, "tab", Some("\t"), Modifiers::default());
        assert!(!dirty(&rig, vcx));
    }

    /// A parent that records the key-downs which bubble up to it.
    struct Parent {
        child: Entity<HexView>,
        seen: Rc<RefCell<Vec<String>>>,
    }

    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let seen = self.seen.clone();
            div()
                .size_full()
                .on_key_down(move |ev: &KeyDownEvent, _, _| {
                    seen.borrow_mut().push(ev.keystroke.key.to_string());
                })
                .child(self.child.clone())
        }
    }

    /// Keys that are not printable are not swallowed on the ASCII side: a parent's key-down
    /// still sees Enter and Escape, while a printable one stops at the view. Guards the
    /// printable filter in `key_down` (without it every character stops propagation).
    #[gpui::test]
    fn non_printable_keys_on_the_ascii_side_reach_the_parent(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            cx.set_global(crate::ui::presets::builtins()[0].semantics());
        });
        let host = Counting::new();
        let (data, _dir) = paged(100, &[], &host);
        let data = cx.new(|_| data);
        let seen: Rc<RefCell<Vec<String>>> = Rc::default();
        let made: Rc<RefCell<Option<Entity<HexView>>>> = Rc::default();
        let (_, vcx) = cx.add_window_view({
            let (data, seen, made) = (data.clone(), seen.clone(), made.clone());
            move |_, cx| {
                let child = cx.new(|cx| HexView::new(data, 0, cx));
                *made.borrow_mut() = Some(child.clone());
                Parent { child, seen }
            }
        });
        let view = made.take().expect("the window made its view");
        settle(vcx, &data);
        view.update(vcx, |v, cx| v.toggle_side(cx));
        view.update_in(vcx, |v, window, cx| window.focus(&v.focus, cx));
        let press = |vcx: &mut VisualTestContext, key: &str, ch: &str| {
            vcx.simulate_event(KeyDownEvent {
                keystroke: gpui::Keystroke {
                    modifiers: Modifiers::default(),
                    key: key.into(),
                    key_char: Some(ch.into()),
                },
                is_held: false,
                prefer_character_input: false,
            });
        };
        press(vcx, "enter", "\n");
        press(vcx, "escape", "\u{1b}");
        assert_eq!(*seen.borrow(), ["enter", "escape"], "the parent saw both");
        press(vcx, "a", "a");
        assert_eq!(
            seen.borrow().len(),
            2,
            "a typed character stops at the view"
        );
        assert!(
            data.read_with(vcx, |d, _| d.dirty()),
            "the typed character was written"
        );
        let first = data.read_with(vcx, |d, _| match &d.content {
            HexContent::Ready(m) => m.store.effective(0),
            HexContent::TooLarge { .. } => None,
        });
        assert_eq!(first, Some(b'a'), "and Enter and Escape wrote nothing");
    }

    /// Writing a byte back to what the disk holds makes the document clean again. Guards
    /// `HexData::run` notifying and the overlay drop, seen through `dirty`.
    #[gpui::test]
    fn a_byte_typed_back_to_its_original_is_clean(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[0x41]);
        type_text(&rig, vcx, "42");
        assert!(dirty(&rig, vcx));
        motion(&rig, vcx, HexMotion::Left, false);
        type_text(&rig, vcx, "41");
        assert_eq!(bytes(&rig, vcx, 0..1), [Some(0x41)]);
        assert!(!dirty(&rig, vcx));
    }

    /// Undo twice restores two separate edits and clears the dirty state; redo brings the
    /// first back. Goes through the bound actions. Guards the `HexUndo`/`HexRedo` handlers and
    /// `Op::Undo`/`Op::Redo`.
    #[gpui::test]
    fn undo_and_redo_run_through_their_actions(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[]);
        type_text(&rig, vcx, "41");
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        type_text(&rig, vcx, "42");
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0x41), Some(0x42)]);
        dispatch(&rig, vcx, HexUndo);
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0x41), Some(0)]);
        dispatch(&rig, vcx, HexUndo);
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0), Some(0)]);
        assert!(!dirty(&rig, vcx), "undone to the file as it was");
        dispatch(&rig, vcx, HexRedo);
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(0x41), Some(0)]);
        assert!(dirty(&rig, vcx));
        dispatch(&rig, vcx, HexUndo);
        dispatch(&rig, vcx, HexUndo);
        assert!(!dirty(&rig, vcx), "nothing left to undo is not an error");
    }

    /// A key on a page that has not been read is held, the page is requested, and the key lands
    /// when the page does, with one read and no second try. Guards `run_op`'s `NotLoaded` arm,
    /// `pending`, the observer's `retry_due` and the prepaint retry.
    #[gpui::test]
    fn typing_on_an_unread_page_lands_after_the_page_arrives(cx: &mut TestAppContext) {
        let page = PAGE as u64;
        let (rig, vcx) = rig(cx, page * 4, &[]);
        rig.data.update(vcx, |d, _| {
            if let HexContent::Ready(m) = &mut d.content {
                m.caret.offset = 2 * page;
            }
        });
        assert_eq!(bytes(&rig, vcx, 2 * page..2 * page + 1), [None]);
        type_text(&rig, vcx, "4");
        assert_eq!(bytes(&rig, vcx, 2 * page..2 * page + 1), [Some(0x40)]);
        let reads = rig.host.reads();
        assert_eq!(
            reads.iter().filter(|&&off| off == 2 * page).count(),
            1,
            "the page was read once, not again: {reads:?}"
        );
        assert!(rig.view.read_with(vcx, |v, _| v.pending.is_none()));
    }

    /// When the read fails the key is dropped and the page is not asked for in a loop; a later
    /// key asks again. Guards the `!retry` condition around `ensure_pages` and the `waiting`
    /// check that drops `pending`.
    #[gpui::test]
    fn a_failed_read_drops_the_key_without_looping(cx: &mut TestAppContext) {
        let page = PAGE as u64;
        let (rig, vcx) = rig(cx, page * 4, &[]);
        rig.host
            .fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        rig.data.update(vcx, |d, _| {
            if let HexContent::Ready(m) = &mut d.content {
                m.caret.offset = 2 * page;
            }
        });
        type_text(&rig, vcx, "4");
        let reads = rig.host.reads();
        assert_eq!(
            reads.iter().filter(|&&off| off == 2 * page).count(),
            1,
            "asked once: {reads:?}"
        );
        assert!(!dirty(&rig, vcx));
        assert!(rig.view.read_with(vcx, |v, _| v.pending.is_none()));
        rig.host
            .fail
            .store(false, std::sync::atomic::Ordering::SeqCst);
        type_text(&rig, vcx, "4");
        assert_eq!(bytes(&rig, vcx, 2 * page..2 * page + 1), [Some(0x40)]);
    }

    /// Backspace and Delete leave the bytes alone and say the editor only overwrites. Guards
    /// the `backspace`/`delete` arm of `key_down`.
    #[gpui::test]
    fn backspace_and_delete_change_nothing_and_say_why(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[7, 8]);
        let words = t(L10nKey::HexEditOverwriteOnly);
        for (i, name) in ["backspace", "delete"].into_iter().enumerate() {
            key(&rig, vcx, name, None, Modifiers::default());
            assert_eq!(said(&rig, vcx).len(), i + 1, "{name} says so");
        }
        assert!(said(&rig, vcx).iter().all(|s| s == words));
        assert_eq!(bytes(&rig, vcx, 0..2), [Some(7), Some(8)]);
        assert!(!dirty(&rig, vcx));
        assert_eq!(status(&rig, vcx).len, 100);
    }

    /// Chords with ctrl, alt or cmd are not typed, even with a character attached. Guards the
    /// modifier check at the top of `key_down`.
    #[gpui::test]
    fn chords_with_ctrl_alt_or_cmd_are_not_typed(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[]);
        for modifiers in [
            Modifiers::control(),
            Modifiers::alt(),
            Modifiers::command(),
            Modifiers::function(),
        ] {
            key(&rig, vcx, "a", Some("a"), modifiers);
            key(&rig, vcx, "backspace", None, modifiers);
        }
        assert!(!dirty(&rig, vcx));
        assert!(said(&rig, vcx).is_empty(), "not even the overwrite message");
    }

    /// Copy on the hex side writes `de ad be ef`; on the ASCII side the same selection as text
    /// with dots; with no selection, the caret's byte. Guards `HexData::copy_text` and the
    /// `HexCopy` handler.
    #[gpui::test]
    fn copy_formats_both_sides_and_the_caret_byte(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, &[0xde, 0xad, 0xbe, 0xef]);
        dispatch(&rig, vcx, HexCopy);
        assert_eq!(
            clipboard(vcx).as_deref(),
            Some("de"),
            "no selection copies the caret byte"
        );
        select_first(&rig, vcx, 4);
        dispatch(&rig, vcx, HexCopy);
        assert_eq!(clipboard(vcx).as_deref(), Some("de ad be ef"));
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        dispatch(&rig, vcx, HexCopy);
        assert_eq!(
            clipboard(vcx).as_deref(),
            Some("...."),
            "non-printables are dots"
        );
    }

    /// Printable bytes copy as themselves from the ASCII side. Guards the `Side::Ascii` arm of
    /// `copy_text`.
    #[gpui::test]
    fn copy_from_the_ascii_side_keeps_printable_bytes(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100, b"Hi\x00!");
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        select_first(&rig, vcx, 4);
        dispatch(&rig, vcx, HexCopy);
        assert_eq!(clipboard(vcx).as_deref(), Some("Hi.!"));
    }

    /// Pasting hex overwrites from the caret as one undo step. Guards the hex arm of `paste`
    /// and `Op::Paste` reaching `HexModel::paste`.
    #[gpui::test]
    fn pasting_hex_overwrites_from_the_caret(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[1, 2, 3, 4]);
        motion(&rig, vcx, HexMotion::Right, false);
        put_on_clipboard(vcx, "de:AD 0xbe");
        dispatch(&rig, vcx, HexPaste);
        assert_eq!(
            bytes(&rig, vcx, 0..5),
            [Some(1), Some(0xde), Some(0xad), Some(0xbe), Some(0)]
        );
        assert!(said(&rig, vcx).is_empty(), "nothing was cut");
        dispatch(&rig, vcx, HexUndo);
        assert_eq!(bytes(&rig, vcx, 0..4), [Some(1), Some(2), Some(3), Some(4)]);
        assert!(!dirty(&rig, vcx));
    }

    /// Pasting text on the ASCII side writes its bytes. Guards the ASCII arm of `paste`.
    #[gpui::test]
    fn pasting_text_on_the_ascii_side_writes_its_bytes(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[]);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        put_on_clipboard(vcx, "Hi!");
        dispatch(&rig, vcx, HexPaste);
        assert_eq!(
            bytes(&rig, vcx, 0..4),
            [Some(0x48), Some(0x69), Some(0x21), Some(0)]
        );
    }

    /// A paste that runs past the end writes what fits and says how much was left out. Guards
    /// the `truncated` branch of `run_op` and `truncated_notice`.
    #[gpui::test]
    fn pasting_past_the_end_truncates_and_says_so(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 4, &[9, 9, 9, 9]);
        motion(&rig, vcx, HexMotion::Right, false);
        motion(&rig, vcx, HexMotion::Right, false);
        put_on_clipboard(vcx, "01 02 03 04 05");
        dispatch(&rig, vcx, HexPaste);
        assert_eq!(bytes(&rig, vcx, 0..4), [Some(9), Some(9), Some(1), Some(2)]);
        assert_eq!(status(&rig, vcx).len, 4, "the file did not grow");
        assert_eq!(
            said(&rig, vcx),
            [truncated_notice(2, 3)],
            "2 written, 3 left out"
        );
    }

    /// The truncation message carries both counts. Guards `truncated_notice`.
    #[test]
    fn the_truncation_notice_names_what_was_written_and_what_was_left_out() {
        let text = truncated_notice(2, 5);
        assert!(text.contains('2') && text.contains('5'), "{text}");
        assert_ne!(truncated_notice(2, 5), truncated_notice(2, 6));
    }

    /// Text that is not hex pastes nothing and says so, for non-hex digits and for an odd
    /// number of them. Guards the parse in `paste` and its message.
    #[gpui::test]
    fn invalid_hex_pastes_nothing(cx: &mut TestAppContext) {
        let (rig, vcx) = rig_rooted(cx, 100, &[1, 2]);
        for text in ["zz", "abc", ""] {
            put_on_clipboard(vcx, text);
            let before = said(&rig, vcx).len();
            dispatch(&rig, vcx, HexPaste);
            let now = said(&rig, vcx);
            assert_eq!(now.len(), before + 1, "{text:?} is refused out loud");
            assert_eq!(
                now.last().map(String::as_str),
                Some(t(L10nKey::HexPasteNotHex))
            );
            assert!(!dirty(&rig, vcx), "{text:?} wrote nothing");
        }
    }

    /// A refusal speaks only when it helps: an empty file does, a stray character does not.
    /// Guards `failure_notice`'s arms.
    #[test]
    fn only_some_refusals_get_a_notice() {
        assert!(failure_notice(&Failure::Edit(EditRefusal::NothingToEdit)).is_some());
        assert!(failure_notice(&Failure::CopyTooLarge).is_some());
        for silent in [
            EditRefusal::NotLoaded,
            EditRefusal::NotHexDigit,
            EditRefusal::NotPrintable,
        ] {
            assert!(
                failure_notice(&Failure::Edit(silent)).is_none(),
                "{silent:?}"
            );
        }
    }

    /// An edited byte takes the edit colour in both columns, on top of the caret's mark; its
    /// neighbours do not. Guards the `edited` branch of `row_text`'s style.
    #[test]
    fn an_edited_byte_is_drawn_in_the_edit_colour() {
        let edited = gpui::hsla(0.1, 0.9, 0.5, 1.);
        let edited_mark = gpui::hsla(0.1, 0.9, 0.4, 1.);
        let marks = Marks {
            selection: HighlightStyle::default(),
            caret_here: HighlightStyle::default(),
            caret_other: HighlightStyle::default(),
            pending: HighlightStyle::default(),
            edited,
            edited_mark,
        };
        let mut model = HexModel::new(Source::Whole(Arc::from(vec![0x41u8, 0x42, 0x43])));
        model.caret.offset = 2;
        model.caret.low_nibble = false;
        assert!(model.type_hex(b'5').is_ok());
        let row = row_text(&model, 0, Grid::new(3), &marks);
        // The byte typed into is the one the caret stayed on.
        assert_eq!(row.hex, "41 42 53");
        let colour = |marks: &[(Range<usize>, HighlightStyle)], start: usize| {
            marks
                .iter()
                .find(|(r, _)| r.start == start)
                .and_then(|(_, s)| s.color)
        };
        assert_eq!(colour(&row.hex_marks, 6), Some(edited));
        assert_eq!(colour(&row.ascii_marks, 2), Some(edited));
        assert_eq!(colour(&row.hex_marks, 3), None);
        assert_eq!(colour(&row.ascii_marks, 0), None);
    }

    /// An edited byte under a selection keeps the normal text colour and gets an underline in
    /// the edit colour instead, since that colour is not legible on the selection in every
    /// theme. Guards the highlighted-byte arm of the `edited` branch of `row_text`.
    #[test]
    fn an_edited_selected_byte_is_underlined_not_recoloured() {
        let edited = gpui::hsla(0.1, 0.9, 0.5, 1.);
        let edited_mark = gpui::hsla(0.1, 0.9, 0.4, 1.);
        let marks = Marks {
            selection: HighlightStyle {
                background_color: Some(gpui::hsla(0., 0., 0.8, 1.)),
                ..Default::default()
            },
            caret_here: HighlightStyle::default(),
            caret_other: HighlightStyle::default(),
            pending: HighlightStyle::default(),
            edited,
            edited_mark,
        };
        let mut model = HexModel::new(Source::Whole(Arc::from(vec![0x41u8, 0x42, 0x43])));
        assert!(model.type_hex(b'5').is_ok());
        // Caret to the end with the anchor on the edited byte: bytes 0..=2 are selected.
        model.caret.anchor = Some(0);
        model.caret.offset = 2;
        let row = row_text(&model, 0, Grid::new(3), &marks);
        let style = row
            .hex_marks
            .iter()
            .find(|(r, _)| r.start == 0)
            .map(|(_, s)| *s)
            .expect("the selected edited byte is marked");
        assert_eq!(style.color, None, "text stays the normal foreground");
        assert_eq!(style.underline.and_then(|u| u.color), Some(edited_mark));
        let neighbour = row.hex_marks.iter().find(|(r, _)| r.start == 3);
        assert!(
            neighbour.is_some_and(|(_, s)| s.underline.is_none()),
            "an unedited selected byte has no underline"
        );
    }

    // --- mouse ---------------------------------------------------------------------------

    /// Clicks in the hex and the ASCII column land on the byte under the pointer, at 1x and at
    /// 1.5x zoom, where the cell is wider and the rows taller. The positions come from the
    /// column origins the grid documents (11 characters in for hex, 61 for ASCII), not from
    /// `Grid`. Guards hit-testing using the metrics the painter used.
    #[gpui::test]
    fn clicks_land_on_the_byte_under_the_pointer_at_both_zooms(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        let mut cell_widths = Vec::new();
        for zoom in [1.0, 1.5] {
            set_zoom(vcx, zoom);
            redraw(&rig, vcx);
            let l = layout(&rig, vcx);
            let (ox, oy) = (l.body.origin.x.as_f32(), l.body.origin.y.as_f32());
            let (cw, rh) = (l.metrics.cell_w, l.metrics.row_h);
            cell_widths.push(cw);

            // Byte 9 of row 2 in hex: characters 11 + 3*9 + 1 (the middle gap) = 39..41.
            vcx.simulate_click(at(ox + 40. * cw, oy + 2.5 * rh), Modifiers::default());
            let s = status(&rig, vcx);
            assert_eq!(s.offset, 2 * 16 + 9, "hex at {zoom}x");
            let side = rig.data.read_with(vcx, |d, _| match &d.content {
                HexContent::Ready(m) => m.caret.side,
                HexContent::TooLarge { .. } => panic!("too large"),
            });
            assert_eq!(side, Side::Hex);

            // Byte 5 of row 3 in ASCII: character 61 + 5.
            vcx.simulate_click(at(ox + 66.5 * cw, oy + 3.5 * rh), Modifiers::default());
            assert_eq!(status(&rig, vcx).offset, 3 * 16 + 5, "ascii at {zoom}x");
            let side = rig.data.read_with(vcx, |d, _| match &d.content {
                HexContent::Ready(m) => m.caret.side,
                HexContent::TooLarge { .. } => panic!("too large"),
            });
            assert_eq!(side, Side::Ascii);
        }
        assert!(
            (cell_widths[1] / cell_widths[0] - 1.5).abs() < 0.05,
            "the cell scaled with the zoom: {cell_widths:?}"
        );
    }

    /// Shift-click extends from the caret, and a drag selects between where it began and where
    /// it is. Guards `mouse_down`'s anchor handling and `mouse_move`.
    #[gpui::test]
    fn shift_click_extends_and_a_drag_selects(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        let l = layout(&rig, vcx);
        let (ox, oy) = (l.body.origin.x.as_f32(), l.body.origin.y.as_f32());
        let (cw, rh) = (l.metrics.cell_w, l.metrics.row_h);
        let hex = |row: f32, i: f32| at(ox + (11. + 3. * i + 1.) * cw, oy + (row + 0.5) * rh);

        vcx.simulate_click(hex(0., 2.), Modifiers::default());
        vcx.simulate_click(
            hex(0., 5.),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        let s = status(&rig, vcx);
        assert_eq!((s.offset, s.selection), (5, Some(4)));

        vcx.simulate_mouse_down(hex(1., 3.), MouseButton::Left, Modifiers::default());
        assert_eq!(
            status(&rig, vcx).selection,
            None,
            "a plain press selects nothing"
        );
        vcx.simulate_mouse_move(hex(2., 4.), MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_up(hex(2., 4.), MouseButton::Left, Modifiers::default());
        let s = status(&rig, vcx);
        assert_eq!((s.offset, s.selection), (36, Some(18)), "bytes 19..=36");
        vcx.simulate_mouse_move(hex(3., 0.), MouseButton::Left, Modifiers::default());
        assert_eq!(
            status(&rig, vcx).offset,
            36,
            "moving after the release drags nothing"
        );
    }

    /// The wheel scrolls by the pixels it brings, and with the editor's zoom modifier held it
    /// leaves the view alone so the editor can zoom. Guards `scroll_wheel` and its early
    /// return.
    #[gpui::test]
    fn the_wheel_scrolls_and_the_zoom_wheel_does_not(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100_000, &[]);
        vcx.update(|_, cx| {
            cx.update_global::<Config, _>(|c, _| c.0.mouse_zoom_modifier = MouseZoomModifier::Ctrl)
        });
        let l = layout(&rig, vcx);
        let inside = at(
            l.body.origin.x.as_f32() + 40.,
            l.body.origin.y.as_f32() + 40.,
        );
        let wheel = |modifiers| gpui::ScrollWheelEvent {
            position: inside,
            delta: ScrollDelta::Pixels(point(px(0.), px(-(l.metrics.row_h * 3.5)))),
            modifiers,
            touch_phase: TouchPhase::Moved,
        };
        vcx.simulate_event(wheel(Modifiers {
            control: true,
            ..Modifiers::default()
        }));
        assert_eq!(scroll(&rig, vcx), Scroll::default(), "ctrl+wheel is a zoom");
        vcx.simulate_event(wheel(Modifiers::default()));
        let s = scroll(&rig, vcx);
        assert_eq!(s.top_row, 3);
        assert!((s.sub - l.metrics.row_h * 0.5).abs() < 0.01, "{}", s.sub);
    }

    // --- horizontal scroll ---------------------------------------------------------------

    /// A window too narrow for the 67 characters of hex and ASCII beside the offset column.
    fn narrow(rig: &Rig, vcx: &mut VisualTestContext) {
        vcx.simulate_resize(size(px(400.), px(300.)));
        redraw(rig, vcx);
        assert!(layout(rig, vcx).max_x() > 0., "the columns do not fit");
    }

    fn set_x(rig: &Rig, vcx: &mut VisualTestContext, x: f32) {
        rig.view.update(vcx, |v, _| {
            let mut s = v.scroll.get();
            s.x = x;
            v.scroll.set(s);
        });
        redraw(rig, vcx);
    }

    fn caret_side(rig: &Rig, vcx: &VisualTestContext) -> Side {
        rig.data.read_with(vcx, |d, _| match &d.content {
            HexContent::Ready(m) => m.caret.side,
            HexContent::TooLarge { .. } => panic!("too large"),
        })
    }

    /// Asserts the cell at `chars` (start..end, from the scrolling part's edge) is fully shown.
    fn assert_shown(rig: &Rig, vcx: &VisualTestContext, chars: Range<f32>, what: &str) {
        let l = layout(rig, vcx);
        let x = scroll(rig, vcx).x;
        let (start, end) = (chars.start * l.metrics.cell_w, chars.end * l.metrics.cell_w);
        assert!(
            x <= start + 0.01 && end <= x + l.span_w() + 0.01,
            "{what}: cell {start}..{end} outside the view {x}..{}",
            x + l.span_w()
        );
    }

    /// Moving right across the hex column keeps each byte's cell fully shown, scrolling the
    /// least (the last byte ends flush with the right edge), and Home goes back to the left.
    /// Guards `caret_moved` calling `reveal_caret_x` and `Scroll::reveal_x`'s two branches.
    #[gpui::test]
    fn the_caret_moving_right_scrolls_its_hex_cell_into_view(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        let l = layout(&rig, vcx);
        let cw = l.metrics.cell_w;
        assert_eq!(scroll(&rig, vcx).x, 0.);
        for i in 1..16u32 {
            motion(&rig, vcx, HexMotion::Right, false);
            let rel = (3 * i + u32::from(i >= 8)) as f32;
            assert_shown(&rig, vcx, rel..rel + 2., &format!("hex byte {i}"));
        }
        let x = scroll(&rig, vcx).x;
        assert!(x > 0., "scrolled right");
        assert!(
            (x - (48. * cw - l.span_w()).min(l.max_x())).abs() < 0.01,
            "the least: byte 15 ends at the right edge, x = {x}"
        );
        motion(&rig, vcx, HexMotion::RowStart, false);
        assert_eq!(scroll(&rig, vcx).x, 0., "Home shows the first byte again");
    }

    /// Tab to the ASCII column scrolls to the ASCII cell of the caret byte, and Tab back to
    /// the hex cell. Guards `toggle_side` calling `reveal_caret_x` and the cell for each side.
    #[gpui::test]
    fn tab_scrolls_to_the_cell_of_the_side_it_switches_to(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        redraw(&rig, vcx);
        assert_eq!(caret_side(&rig, vcx), Side::Ascii);
        // ASCII byte 0: character 50 from the scrolling part's edge.
        assert_shown(&rig, vcx, 50.0..51.0, "ascii byte 0");
        assert!(scroll(&rig, vcx).x > 0.);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        redraw(&rig, vcx);
        assert_eq!(scroll(&rig, vcx).x, 0., "back to the hex cell");
    }

    /// Typing advances the caret and reveals the next cell: fifteen characters typed on the
    /// ASCII side leave the caret on the last byte of the row. Guards the edit path reaching
    /// `caret_moved` (`run_op`), for the ASCII cell at the far end of the row.
    #[gpui::test]
    fn typing_on_the_ascii_side_keeps_the_caret_cell_shown(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        rig.view.update(vcx, |v, cx| v.toggle_side(cx));
        for _ in 0..15 {
            type_text(&rig, vcx, "a");
        }
        assert_eq!(status(&rig, vcx).offset, 15);
        assert_shown(&rig, vcx, 65.0..66.0, "ascii byte 15");
    }

    /// A click while scrolled lands on the byte under the pointer in both columns at both
    /// zooms, and a click on the pinned offset column selects nothing. Positions come from
    /// the documented column origins (11 characters pinned, 50 more to the ASCII). Guards
    /// `Grid::hit`'s shift for the scrolling part and its pinned-part exception.
    #[gpui::test]
    fn clicks_land_on_the_right_byte_while_scrolled(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        for zoom in [1.0, 1.5] {
            set_zoom(vcx, zoom);
            narrow(&rig, vcx);
            let l = layout(&rig, vcx);
            let (ox, oy) = (l.body.origin.x.as_f32(), l.body.origin.y.as_f32());
            let (cw, rh) = (l.metrics.cell_w, l.metrics.row_h);
            let (pinned, y) = (11. * cw, oy + 1.5 * rh);

            // Hex byte 12 of row 1: cell 37..39 from the edge, 38 is its middle.
            let x = 20. * cw;
            set_x(&rig, vcx, x);
            vcx.simulate_click(at(ox + pinned + 38. * cw - x, y), Modifiers::default());
            assert_eq!(status(&rig, vcx).offset, 16 + 12, "hex at {zoom}x");
            assert_eq!(caret_side(&rig, vcx), Side::Hex);
            assert_eq!(scroll(&rig, vcx).x, x, "a shown cell does not scroll");

            // The offset column is pinned: its pixels are not shifted into the hex column.
            vcx.simulate_click(at(ox + 5. * cw, y), Modifiers::default());
            assert_eq!(
                status(&rig, vcx).offset,
                16 + 12,
                "offset column at {zoom}x"
            );

            // ASCII byte 5 of row 1: cell 55..56.
            let x = layout(&rig, vcx).max_x();
            set_x(&rig, vcx, x);
            vcx.simulate_click(at(ox + pinned + 55.5 * cw - x, y), Modifiers::default());
            assert_eq!(status(&rig, vcx).offset, 16 + 5, "ascii at {zoom}x");
            assert_eq!(caret_side(&rig, vcx), Side::Ascii);
        }
    }

    /// A drag selects across bytes while scrolled. Guards `mouse_move` using the shifted hit.
    #[gpui::test]
    fn a_drag_selects_while_scrolled(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        let l = layout(&rig, vcx);
        let (ox, oy) = (l.body.origin.x.as_f32(), l.body.origin.y.as_f32());
        let (cw, rh) = (l.metrics.cell_w, l.metrics.row_h);
        let x = 20. * cw;
        set_x(&rig, vcx, x);
        let hex = |i: f32| at(ox + 11. * cw + (3. * i + 1.) * cw - x, oy + 0.5 * rh);
        vcx.simulate_mouse_down(hex(10.), MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_move(hex(12.), MouseButton::Left, Modifiers::default());
        vcx.simulate_mouse_up(hex(12.), MouseButton::Left, Modifiers::default());
        let s = status(&rig, vcx);
        assert_eq!((s.offset, s.selection), (12, Some(3)));
    }

    /// The offset column's position does not depend on the horizontal offset, the hex column
    /// moves by exactly it. Guards the offset column staying outside the shifted clip.
    #[gpui::test]
    fn the_offset_column_stays_put_while_the_hex_column_moves(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        let at_x = |rig: &Rig, vcx: &mut VisualTestContext| {
            let offset = vcx
                .debug_bounds("hex-offset-column")
                .expect("offset column");
            let hex = vcx.debug_bounds("hex-hex-column").expect("hex column");
            (
                offset.origin.x.as_f32(),
                hex.origin.x.as_f32(),
                scroll(rig, vcx).x,
            )
        };
        let (off0, hex0, x0) = at_x(&rig, vcx);
        assert_eq!(x0, 0.);
        set_x(&rig, vcx, 37.);
        let (off1, hex1, x1) = at_x(&rig, vcx);
        assert_eq!(x1, 37.);
        assert_eq!(off0, off1, "the offset column did not move");
        assert!((hex0 - hex1 - 37.).abs() < 0.01, "{hex0} -> {hex1}");
    }

    /// A horizontal wheel delta (and shift+wheel) scrolls sideways, a vertical one only rows,
    /// and the end is clamped. Guards `scroll_wheel`'s dx/dy split and `Scroll::set_x`.
    #[gpui::test]
    fn horizontal_wheel_scrolls_sideways_and_vertical_does_not(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 100_000, &[]);
        narrow(&rig, vcx);
        vcx.update(|_, cx| {
            cx.update_global::<Config, _>(|c, _| c.0.mouse_zoom_modifier = MouseZoomModifier::Ctrl)
        });
        let l = layout(&rig, vcx);
        let inside = at(
            l.body.origin.x.as_f32() + 40.,
            l.body.origin.y.as_f32() + 40.,
        );
        let wheel = |dx: f32, dy: f32, modifiers| gpui::ScrollWheelEvent {
            position: inside,
            delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
            modifiers,
            touch_phase: TouchPhase::Moved,
        };
        vcx.simulate_event(wheel(-30., 0., Modifiers::default()));
        assert_eq!(scroll(&rig, vcx).x, 30.);
        assert_eq!(scroll(&rig, vcx).top_row, 0, "no rows moved");
        vcx.simulate_event(wheel(0., -l.metrics.row_h * 3., Modifiers::default()));
        let s = scroll(&rig, vcx);
        assert_eq!((s.x, s.top_row), (30., 3), "vertical moves rows only");
        vcx.simulate_event(wheel(0., -10., Modifiers::shift()));
        let s = scroll(&rig, vcx);
        assert_eq!((s.x, s.top_row), (40., 3), "shift+wheel moves sideways");
        vcx.simulate_event(wheel(1e6, 0., Modifiers::default()));
        assert_eq!(scroll(&rig, vcx).x, 0., "not before the first column");
        vcx.simulate_event(wheel(-1e6, 0., Modifiers::default()));
        assert_eq!(scroll(&rig, vcx).x, l.max_x(), "not past the last column");
    }

    /// Widening the viewport pulls the offset back so no gap is left at the right. Guards the
    /// prepaint check and `clamp_x`.
    #[gpui::test]
    fn widening_the_view_clamps_the_offset(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 4096, &[]);
        narrow(&rig, vcx);
        let x = layout(&rig, vcx).max_x();
        set_x(&rig, vcx, x);
        assert_eq!(scroll(&rig, vcx).x, x);
        vcx.simulate_resize(size(px(500.), px(300.)));
        redraw(&rig, vcx);
        let l = layout(&rig, vcx);
        assert!(l.max_x() < x, "the wider view needs less scrolling");
        assert!((scroll(&rig, vcx).x - l.max_x()).abs() < 0.01);
        vcx.simulate_resize(size(px(1600.), px(300.)));
        redraw(&rig, vcx);
        assert_eq!(scroll(&rig, vcx).x, 0.);
    }

    /// Cells and clicks agree: every byte's cell is where `hit` finds that byte. Guards
    /// `Grid::cell` against drifting from `Grid::hit`.
    #[test]
    fn a_cells_middle_hits_its_own_byte() {
        let grid = Grid::new(4096);
        let start = grid.scroll_start() as f32;
        for i in 0..ROW_BYTES {
            for side in [Side::Hex, Side::Ascii] {
                let cell = grid.cell(i, side);
                let mid = start + (cell.start + cell.end) as f32 / 2.;
                assert_eq!(grid.hit(mid - 1., 1.), Some((i, side)));
                assert_eq!(grid.hit(mid, 0.), Some((i, side)));
            }
        }
    }

    // --- status --------------------------------------------------------------------------

    /// The status carries the caret's offset, the selection length, the file length and the
    /// bytes read as numbers, fewer at the end of the file. Guards `status`.
    #[gpui::test]
    fn the_status_reads_the_bytes_at_the_caret(cx: &mut TestAppContext) {
        let (rig, vcx) = rig(cx, 5, &[1, 2, 3, 4, 5]);
        let s = status(&rig, vcx);
        assert_eq!((s.offset, s.len, s.selection), (0, 5, None));
        assert_eq!(s.values.u8, Some(1));
        assert_eq!(s.values.u16le, Some(0x0201));
        assert_eq!(s.values.u32be, Some(0x0102_0304));
        for _ in 0..3 {
            motion(&rig, vcx, HexMotion::Right, false);
        }
        let s = status(&rig, vcx);
        assert_eq!(s.offset, 3);
        assert_eq!(s.values.u16be, Some(0x0405));
        assert_eq!(s.values.u32le, None, "two bytes are left");
        motion(&rig, vcx, HexMotion::Left, true);
        assert_eq!(status(&rig, vcx).selection, Some(2));
    }

    /// A file too large to open draws nothing, builds no rows and has no status.
    #[gpui::test]
    fn a_too_large_document_draws_nothing_and_has_no_status(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            cx.set_global(crate::ui::presets::builtins()[0].semantics());
        });
        let host = Counting::new();
        let data = cx.new(|_| {
            HexData::from_loaded(
                host.clone(),
                super::super::io::HexLoaded::TooLarge {
                    path: "/x/big.bin".into(),
                    len: 9,
                    limit: 5,
                },
            )
        });
        let (view, vcx) = cx.add_window_view({
            let data = data.clone();
            move |_, cx| HexView::new(data, 0, cx)
        });
        settle(vcx, &data);
        assert!(view.read_with(vcx, |v, cx| v.status(cx)).is_none());
        assert_eq!(view.read_with(vcx, |v, _| v.row_builds.get()), 0);
        assert!(host.reads().is_empty());
    }
}
