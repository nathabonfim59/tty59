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

// Used once the editor opens hex tabs (WP2.1); removed then.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, HighlightStyle, KeyContext, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString,
    Size, StyledText, Subscription, UnderlineStyle, Window, canvas, div, point, px, size,
};
use gpui_component::ActiveTheme as _;
use gpui_component::scroll::ScrollbarHandle;

use super::data::{HexContent, HexData};
use super::model::{self, Byte, HexModel, ROW, Side, Values};
use crate::core::actions::{HEX_KEY_CONTEXT, HexMotion, HexMove, HexToggleSide};
use crate::core::config::Config;
use crate::ui::code_editor::DOCUMENT_KEY_CONTEXT;
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

    fn total(&self) -> u32 {
        self.ascii_start() + ROW_BYTES + MARGIN
    }

    /// The byte (0..16) and column under `x`, in characters from the row's left edge.
    fn hit(&self, x: f32) -> Option<(u32, Side)> {
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

/// What the last prepaint measured; hit-testing and the scrollbar read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Layout {
    /// The rows' viewport, in window coordinates.
    body: Bounds<Pixels>,
    metrics: Metrics,
    /// Rows in the file.
    rows: u64,
}

impl Layout {
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
}

impl Scroll {
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
    requested: Rc<Cell<Option<(u64, u64)>>>,
    /// Where a mouse drag began, while the button is down.
    drag_from: Option<u64>,
    /// The last measurement and what it was made for.
    measured: RefCell<Option<(MeasureKey, Metrics, Pixels)>>,
    font_warned: Cell<bool>,
    /// The column numbers above the rows; they never change.
    header: SharedString,
    _data_changed: Subscription,
    /// Rows built by render, for the tests that pin that only visible rows are.
    #[cfg(test)]
    row_builds: Rc<Cell<usize>>,
}

impl HexView {
    pub(crate) fn new(data: Entity<HexData>, slot: usize, cx: &mut Context<Self>) -> Self {
        let _data_changed = cx.observe(&data, |_, _, cx| cx.notify());
        Self {
            focus: cx.focus_handle(),
            data,
            slot,
            scroll: Rc::default(),
            layout: Rc::default(),
            requested: Rc::default(),
            drag_from: None,
            measured: RefCell::new(None),
            font_warned: Cell::new(false),
            header: header_text(),
            _data_changed,
            #[cfg(test)]
            row_builds: Rc::default(),
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
        self.requested.set(Some((rows.start, rows.end)));
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

    /// After the caret moved by key or click: bring it into view and load what is there.
    fn caret_moved(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (offset, total) = match &self.data.read(cx).content {
            HexContent::Ready(m) => (m.caret.offset, m.store.len().div_ceil(ROW)),
            HexContent::TooLarge { .. } => return,
        };
        let l = self.layout.get();
        let mut scroll = self.scroll.get();
        scroll.reveal(offset / ROW, total, l.page());
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

    fn toggle_side(&mut self, cx: &mut Context<Self>) {
        self.data.update(cx, |data, cx| {
            if let HexContent::Ready(m) = &mut data.content {
                m.toggle_side();
            }
            cx.notify();
        });
    }

    fn scroll_wheel(&mut self, ev: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        // The editor's zoom gesture is a wheel turn with the zoom modifier; it is not ours to
        // scroll with.
        if zoom_wheel(cx.global::<Config>().mouse_zoom_modifier, &ev.modifiers) {
            return;
        }
        let l = self.layout.get();
        let dy = -ev.delta.pixel_delta(px(l.metrics.row_h)).y.as_f32();
        let mut scroll = self.scroll.get();
        scroll.by_pixels(dy, l.metrics.row_h, l.rows, l.page());
        self.scroll.set(scroll);
        self.request_visible(window, cx);
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
        let (i, side) = Grid::new(len).hit(x)?;
        let offset = row.checked_mul(ROW)?.checked_add(u64::from(i))?;
        (offset < len).then_some((offset, side))
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
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
        let style = |side: Side| {
            if is_caret {
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
            }
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
        let column = |chars: u32, text: StyledText, color| {
            div()
                .absolute()
                .left(at(chars))
                .whitespace_nowrap()
                .text_color(color)
                .child(text)
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
                    .child(column(
                        grid.offset_start(),
                        StyledText::new(text.offset),
                        muted,
                    ))
                    .child(column(
                        grid.hex_start(),
                        StyledText::new(text.hex).with_highlights(text.hex_marks),
                        fg,
                    ))
                    .child(column(
                        grid.ascii_start(),
                        StyledText::new(text.ascii).with_highlights(text.ascii_marks),
                        fg,
                    )),
            );
        }

        // The measuring canvas: records the viewport and, when the rows on screen are not the
        // ones last asked for (first frame, resize, a scrollbar drag), defers the request so no
        // document change happens during paint.
        let (layout, requested, view) = (self.layout.clone(), self.requested.clone(), cx.entity());
        let rows_total = total;
        let measure = canvas(
            move |bounds, window, cx| {
                layout.set(Layout {
                    body: bounds,
                    metrics,
                    rows: rows_total,
                });
                let (top, h) = (view.read(cx).scroll.get(), bounds.size.height.as_f32());
                let range = visible_rows(top, h, metrics.row_h, rows_total);
                if requested.get() != Some((range.start, range.end)) {
                    requested.set(Some((range.start, range.end)));
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
            .h(px(metrics.row_h))
            .flex_none()
            .child(column(
                grid.hex_start(),
                StyledText::new(self.header.clone()),
                muted,
            ));

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
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
        });
        let host = Counting::new();
        let (data, dir) = paged(len, head, &host);
        let data = cx.new(|_| data);
        let (view, vcx) = cx.add_window_view({
            let data = data.clone();
            move |_, cx| HexView::new(data, 0, cx)
        });
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
