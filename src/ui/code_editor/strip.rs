//! The header's file strip when there are more files than it has room for,
//! and the controls that share the header with it.
//!
//! **The strip keeps the files you use.** It draws as many cells as the
//! header has room for at their narrowest, which is a few in a docked column
//! and more in a filled window; past that the strip used to scroll sideways,
//! which put the file you were just in out of sight as often as not. Now it
//! holds the most recently fronted files — in their strip order, so nothing
//! jumps about as you switch between them — and a `+N` button at its end
//! lists everything, hidden files first, with a filter box. With nothing
//! hidden the button is only its chevron, so the list is one click away from
//! the first file on.
//!
//! **Dock / Fill** is a two-cell switch rather than a menu entry, and a filled
//! editor leads with a pill naming the terminal it covers, which docks it back
//! beside that terminal.

use gpui::prelude::*;
use gpui::{
    Anchor, AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, anchored,
    deferred, div, px, rems,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use super::{BufferId, TabCode};
use crate::core::config::Config;
use crate::ui::app::Tty59App;
use crate::ui::document_column::DocumentChrome;
use crate::ui::i18n::{L10nKey, t, t_fmt};

pub(super) use crate::ui::document_column::{
    HEADER_CELL_H as CELL_H, HEADER_CELL_MAX_W as CELL_MAX_W, HEADER_CELL_RADIUS as CELL_RADIUS,
    HEADER_CELL_TEXT as CELL_TEXT,
};

const PICKER_W: f32 = 340.;
const PICKER_LIST_MAX_H: f32 = 340.;
const PICKER_ROW_H: f32 = 28.;

/// The narrowest a file cell is drawn. A cell shrinks to this and elides its
/// name; a file that would need less is not drawn at all.
pub(super) const TAB_MIN_W: f32 = 56.;
/// Between file cells.
pub(super) const TAB_GAP: f32 = 2.;
/// Between the header's children.
pub(super) const HEADER_GAP: f32 = 2.;
/// The close tile's lead.
pub(super) const CLOSE_LEAD: f32 = 4.;
/// The header's right padding: the glyph, not the tile, lands on the inset
/// the file name starts on at the other end of the bar.
pub(super) const HEADER_TRAIL: f32 = crate::ui::app::CONTENT_INSET
    - (crate::ui::tab_strip::RAIL_TILE - crate::ui::tab_strip::RAIL_TILE_GLYPH) / 2.;
/// How far past the point the hoisted header stops fitting its controls and
/// the front file it has to grow before it is lifted back into the title bar.
pub(crate) const HEADER_HYSTERESIS: f32 = 8.;

// The close tile ends `HEADER_TRAIL` inside the header; the rest of the group
// gap is what the mount reserves beyond the pill.
const _: () = assert!(crate::ui::tab_strip::CHROME_GROUP_GAP >= HEADER_TRAIL);

/// The `+N` button: its padding, the gap between its parts, the chevron's
/// size and its text.
const OVERFLOW_PAD_X: f32 = 8.;
const OVERFLOW_GAP: f32 = 5.;
const OVERFLOW_CHEVRON: f32 = 12.;
pub(super) const OVERFLOW_TEXT: f32 = 12. / 16.;

/// The width of `+` and of one tabular digit in the `+N` button's font.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OverflowGlyphs {
    plus: f32,
    digit: f32,
}

/// [`OverflowGlyphs`] and the font and size they were measured at. Only the
/// two widths: nothing about a file or a header is kept.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverflowMetrics {
    rem: f32,
    family: SharedString,
    glyphs: OverflowGlyphs,
}

impl OverflowMetrics {
    fn font(cx: &gpui::App) -> gpui::Font {
        gpui::Font {
            family: cx.theme().font_family.clone(),
            features: crate::ui::theme::tabular_figures(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: Default::default(),
        }
    }

    /// The rem a frame will be drawn at: the configured size, which `render`
    /// copies onto the window as its first act. Read from the config rather
    /// than the window so a change is measured at the size it is about to be
    /// drawn at, not the one the last frame left behind.
    fn rem(cx: &gpui::App) -> f32 {
        cx.global::<Config>().ui_font_size
    }

    pub(crate) fn measure(window: &Window, cx: &gpui::App) -> Self {
        let rem = Self::rem(cx);
        let (font, size) = (Self::font(cx), rem * OVERFLOW_TEXT);
        let width =
            |text| crate::ui::tab_strip::measure_text(window.text_system(), &font, size, text);
        Self {
            rem,
            family: font.family.clone(),
            glyphs: OverflowGlyphs {
                plus: width("+").ceil(),
                digit: width("0").ceil(),
            },
        }
    }

    pub(crate) fn is_for(&self, cx: &gpui::App) -> bool {
        self.rem == Self::rem(cx) && self.family == cx.theme().font_family
    }
}

/// The filter's placeholder. No count: the list opens for one file too.
fn strip_search_placeholder() -> gpui::SharedString {
    t(L10nKey::EditorStripSearch).into()
}

fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

/// What the `+N` button takes beside the strip for `hidden_digits` digits,
/// the gap before it included. The unsaved dot is always counted: a hidden
/// file turning dirty must not move the cells.
fn overflow_w(hidden_digits: usize, g: OverflowGlyphs) -> f32 {
    HEADER_GAP
        + 2. * OVERFLOW_PAD_X
        + crate::ui::tab_strip::ROW_STATUS_DOT
        + OVERFLOW_GAP
        + g.plus
        + hidden_digits as f32 * g.digit
        + OVERFLOW_GAP
        + OVERFLOW_CHEVRON
}

/// What the chevron alone takes beside the strip, the gap before it included:
/// the button with nothing hidden has no dot and no `+N`. It is there whenever
/// the editor has a file, so the list is always one click away.
fn chevron_w() -> f32 {
    HEADER_GAP + 2. * OVERFLOW_PAD_X + OVERFLOW_CHEVRON
}

/// How many of `total` files a strip with `room` points draws at
/// [`TAB_MIN_W`] each. When they all fit the room must also hold the bare
/// chevron; when they do not, the rest fold into a `+N` that takes
/// [`overflow_w`]. At least one, so the file in front is never the one left out.
pub(crate) fn strip_fit(room: f32, total: usize, g: OverflowGlyphs) -> usize {
    let fit = |w: f32| ((w + TAB_GAP) / (TAB_MIN_W + TAB_GAP)).floor().max(1.) as usize;
    if total <= fit(room - chevron_w()) {
        return total;
    }
    // `N` has at most as many digits as the files that could hide; settle on
    // the fewest it can have once the button is that wide.
    let mut width = digits(total - 1);
    loop {
        let kept = fit(room - overflow_w(width, g)).min(total - 1);
        let needed = digits(total - kept);
        if needed >= width {
            return kept;
        }
        width = needed;
    }
}

/// Whether the hoisted header gives way to the one inside the column. Inline
/// as soon as `avail` cannot hold `need`, and back only once it clears it by
/// [`HEADER_HYSTERESIS`].
pub(crate) fn header_inline(avail: f32, need: f32, was_inline: bool) -> bool {
    avail < need || (was_inline && avail < need + HEADER_HYSTERESIS)
}

impl TabCode {
    /// Every file in the strip, the one in front first and the rest by when
    /// they last were. Files that have never been in front follow in strip
    /// order.
    pub(crate) fn by_recency(&self) -> Vec<BufferId> {
        let front = self.active_id();
        let mut out: Vec<BufferId> = front.into_iter().collect();
        for id in self.recent.iter().chain(self.files.iter()) {
            if self.files.contains(id) && !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }

    /// Moves the file in front to the head of `recent`. Run every frame the
    /// editor draws, so whatever brought a file forward — a click, a link,
    /// back/forward, a restore — is counted without a hook of its own. The
    /// other group's files keep their places, so moving the focus between
    /// groups of a split does not wipe either one's order.
    pub(crate) fn note_front(&mut self) {
        let order = self.by_recency();
        let rest: Vec<BufferId> = self
            .recent
            .iter()
            .copied()
            .filter(|id| !order.contains(id) && self.shows(*id))
            .collect();
        self.recent = order;
        self.recent.extend(rest);
    }

    /// Which files the strip shows when it has room for `cap`: the `cap` most
    /// recent, as `(position in files, id)` in strip order; and the rest,
    /// most recent first.
    pub(crate) fn strip_split(&self, cap: usize) -> (Vec<(usize, BufferId)>, Vec<BufferId>) {
        let order = self.by_recency();
        let cap = cap.max(1);
        let keep = &order[..cap.min(order.len())];
        let shown = self
            .files
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, id)| keep.contains(id))
            .collect();
        let hidden = order.iter().skip(cap).copied().collect();
        (shown, hidden)
    }
}

/// The `+N` list while it is open.
pub(crate) struct StripPicker {
    input: Entity<InputState>,
    /// The highlighted row, an index into the rows the query leaves.
    hi: usize,
    /// Whether the pointer is on the `+N` button. A press there is the
    /// button's to answer — it closes the list on click — so the list does
    /// not also close itself on the press and have the click reopen it.
    over_button: bool,
    _sub: Subscription,
}

/// One row of the list.
struct PickerRow {
    id: BufferId,
    name: SharedString,
    dir: SharedString,
    dirty: bool,
    front: bool,
}

impl Tty59App {
    /// The room left of the header's first control for the strip and the
    /// `+N` button together: the header's width less everything that is not
    /// one of them. Every term is the constant the header draws with.
    fn editor_header_strip_room(
        &self,
        chrome: DocumentChrome,
        trail: f32,
        window: &Window,
        cx: &gpui::App,
    ) -> f32 {
        self.editor_header_w(chrome, trail, window, cx)
            - self.editor_header_fixed_w(chrome, window, cx)
    }

    /// What the hoisted header leaves free at its right end, in the title bar
    /// it is drawn into: the trailing chrome when no detail panel is open and
    /// this column is the one at the window's right edge, then the Agents
    /// pill (which stands at the end of the strip row, over this column, with
    /// either) and the air between chrome groups. The close tile already
    /// keeps `HEADER_TRAIL` of its own, which counts toward that air.
    ///
    /// `attention` is the count the frame's one scan found; the mount and
    /// every budget below take their `trail` from this, so they cannot
    /// disagree about what the pill takes.
    pub(crate) fn hoisted_header_trail(
        &self,
        attention: usize,
        window: &Window,
        cx: &gpui::App,
    ) -> f32 {
        let trailing = match self.right_panel_open(cx) {
            true => 0.,
            false => crate::ui::tab_strip::trailing_chrome_w(window.is_fullscreen()),
        };
        trailing
            + self.strip_metrics.pill_span(attention)
            + (crate::ui::tab_strip::CHROME_GROUP_GAP - HEADER_TRAIL)
    }

    /// The width the header is drawn at in `chrome`. `trail` is what
    /// [`Self::hoisted_header_trail`] reserves, read only by the hoisted
    /// header; a header inside its column has none.
    pub(super) fn editor_header_w(
        &self,
        chrome: DocumentChrome,
        trail: f32,
        window: &Window,
        cx: &gpui::App,
    ) -> f32 {
        let docked = self.document_dock_px(window, cx);
        match chrome {
            DocumentChrome::Fill => self.document_body_px(window, cx),
            DocumentChrome::Dock => docked.map_or_else(
                || self.document_body_px(window, cx),
                crate::ui::document_column::document_content_px,
            ),
            DocumentChrome::DockHoisted => {
                docked.unwrap_or_else(|| self.document_body_px(window, cx)) - trail
            }
        }
    }

    /// Where the header's first control starts. `TITLE_BAR_LEAD` is the room
    /// macOS's traffic lights need; only a header that starts at the left
    /// edge of the window has them to clear, and a docked column never does.
    pub(super) fn editor_header_lead(&self, chrome: DocumentChrome, cx: &gpui::App) -> f32 {
        if self.left_panel_open(cx) || chrome.is_dock() {
            crate::ui::app::CONTENT_INSET
        } else {
            crate::ui::app::TITLE_BAR_LEAD
        }
    }

    /// Everything in the header's row but the file strip and `+N`: the lead,
    /// the filled editor's terminal pill (at its widest), the new-file tile,
    /// the layout switch, the close tile, the trailing inset and the gaps
    /// between them.
    fn editor_header_fixed_w(
        &self,
        chrome: DocumentChrome,
        window: &Window,
        cx: &gpui::App,
    ) -> f32 {
        let back = chrome == DocumentChrome::Fill;
        // Strip, new-file tile, drag spacer, layout switch and close tile.
        let children = 5 + usize::from(back);
        self.editor_header_lead(chrome, cx)
            + HEADER_TRAIL
            + if back {
                crate::ui::document_column::terminal_button_max_w(window)
            } else {
                0.
            }
            + crate::ui::tab_strip::RAIL_TILE
            + crate::ui::document_column::layout_switch_w()
            + CLOSE_LEAD
            + crate::ui::tab_strip::RAIL_TILE
            + HEADER_GAP * (children - 1) as f32
    }

    /// How many files the strip draws: as many as fit at [`TAB_MIN_W`] beside
    /// the `+N` button the rest need.
    pub(super) fn editor_strip_cap(
        &self,
        chrome: DocumentChrome,
        trail: f32,
        window: &Window,
        cx: &gpui::App,
    ) -> usize {
        let total = self.tab_code().map_or(0, |c| c.files.len());
        strip_fit(
            self.editor_header_strip_room(chrome, trail, window, cx),
            total,
            self.editor.overflow_glyphs.glyphs,
        )
    }

    /// The least width the hoisted header needs to keep its controls, the
    /// front file at [`TAB_MIN_W`] and the `+N` the others fold into, or the
    /// bare chevron when there is only the one file.
    fn editor_header_need(&self, window: &Window, cx: &gpui::App) -> f32 {
        let total = self.tab_code().map_or(0, |c| c.files.len());
        let chrome = DocumentChrome::DockHoisted;
        let overflow = match total {
            0 => 0.,
            1 => chevron_w(),
            _ => overflow_w(digits(total - 1), self.editor.overflow_glyphs.glyphs),
        };
        self.editor_header_fixed_w(chrome, window, cx)
            + if total == 0 { 0. } else { TAB_MIN_W }
            + overflow
    }

    /// Whether a docked code editor or diff draws its header inside its
    /// column rather than lifted into the title bar: the title bar's share of
    /// the column cannot hold the controls and what the header must keep.
    /// Transient: the layout asked for is not touched. Read-only; the
    /// latch it is judged against is [`Self::editor_header_latch_sync`]'s.
    pub(crate) fn editor_header_inline(&self, trail: f32, window: &Window, cx: &gpui::App) -> bool {
        self.editor_header_hoistable(window, cx)
            && header_inline(
                self.editor_header_w(DocumentChrome::DockHoisted, trail, window, cx),
                self.document_header_need(window, cx),
                self.editor.header_inline,
            )
    }

    /// The narrowest the front document's column is drawn at: the plain floor,
    /// and for a diff what its header needs to keep its controls and fixed
    /// parts, with the column's rule. Read-only, from the overlay as it is;
    /// nothing here reads the column's own width, so it cannot loop on it.
    pub(crate) fn document_min_px(&self, cx: &gpui::App) -> f32 {
        match self.document_front() {
            Some(crate::ui::app::OverlayTop::Diff) => crate::ui::app::DOCUMENT_MIN_W
                .max(self.diff_header_need(cx) + crate::ui::document_column::DOCUMENT_RULE),
            _ => crate::ui::app::DOCUMENT_MIN_W,
        }
    }

    /// Whether a docked diff fills the workspace instead, for this frame: the
    /// terminal's floor leaves the column less than its header needs, and once
    /// filled it stays so until [`HEADER_HYSTERESIS`] more is there. Derived
    /// from the window and the latch [`Self::editor_header_latch_sync`] keeps;
    /// the layout the user saved is not touched.
    pub(crate) fn diff_fills(&self, window: &Window, cx: &gpui::App) -> bool {
        self.document_layout(cx) == crate::core::config::DocumentLayout::Dock
            && self.document_front() == Some(crate::ui::app::OverlayTop::Diff)
            && header_inline(
                self.document_body_px(window, cx) - crate::ui::app::TERMINAL_MIN_W,
                self.document_min_px(cx),
                self.editor.diff_fill,
            )
    }

    /// How many tiers of a filled diff's secondary parts are on a second
    /// line, and in how many rows and summary lines: the diff is in front and
    /// filling the workspace (the layout asked for, or `fills`, the fallback),
    /// and the plan it gets in the body it is drawn in. Read-only; the latch it
    /// is held against is [`Self::editor_header_latch_sync`]'s.
    fn diff_wrap_level(
        &self,
        fills: bool,
        window: &Window,
        cx: &gpui::App,
    ) -> crate::ui::diff_overlay::WrapState {
        let filled = fills || self.document_layout(cx) == crate::core::config::DocumentLayout::Fill;
        if !filled || self.document_front() != Some(crate::ui::app::OverlayTop::Diff) {
            return Default::default();
        }
        let Some(overlay) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.diff_overlay.as_ref())
        else {
            return Default::default();
        };
        self.diff_fill_need(overlay, window, cx)
            .plan(self.document_body_px(window, cx), self.editor.diff_wrap)
            .state
    }

    /// Whether the header is a code editor's or a diff's in a docked column
    /// off macOS, the headers whose place in the title bar can be given up.
    fn editor_header_hoistable(&self, window: &Window, cx: &gpui::App) -> bool {
        !cfg!(target_os = "macos")
            && self.document_front().is_some()
            && self.document_dock_px(window, cx).is_some()
    }

    /// The least width the front document's hoisted header needs: the code
    /// editor's controls and front file, or the diff's controls and fixed
    /// parts.
    fn document_header_need(&self, window: &Window, cx: &gpui::App) -> f32 {
        match self.document_front() {
            Some(crate::ui::app::OverlayTop::Diff) => self.diff_header_need(cx),
            _ => self.editor_header_need(window, cx),
        }
    }

    /// Measures the `+N` glyphs again if the font or size they are for has
    /// changed. Run by the observers of the theme and the config as well, which
    /// can change either without the app being notified.
    pub(crate) fn editor_overflow_refresh(&mut self, window: &Window, cx: &gpui::App) {
        if !self.editor.overflow_glyphs.is_for(cx) {
            self.editor.overflow_glyphs = OverflowMetrics::measure(window, cx);
        }
        if !self.editor.diff_header.is_for(&self.font_family, cx) {
            self.editor.diff_header =
                crate::ui::diff_overlay::DiffHeaderMetrics::measure(window, cx, &self.font_family);
        }
    }

    /// Records the answer [`Self::editor_header_inline`] gives now, so the
    /// next one is held [`HEADER_HYSTERESIS`] past its threshold. Runs when
    /// the window or the app changes, never while drawing.
    pub(crate) fn editor_header_latch_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_overflow_refresh(window, cx);
        // The diff's fill latch first, so the column the header latch is
        // judged against is the one every reader sees this frame.
        let fills = self.diff_fills(window, cx);
        let refilled = self.editor.diff_fill != fills;
        self.editor.diff_fill = fills;
        // Then which tiers of a filled diff's secondary parts are on its second
        // line, judged against the same body the header is drawn in, each
        // level held the same margin.
        let wrap = self.diff_wrap_level(fills, window, cx);
        let rewrapped = self.editor.diff_wrap != wrap;
        self.editor.diff_wrap = wrap;
        // The pill's count is scanned only for a header that can be lifted;
        // every other notification leaves here without touching the panes.
        let inline = self.editor_header_hoistable(window, cx) && {
            let trail = self.hoisted_header_trail(self.attention_total(), window, cx);
            self.editor_header_inline(trail, window, cx)
        };
        if refilled || rewrapped || self.editor.header_inline != inline {
            self.editor.header_inline = inline;
            cx.notify();
        }
    }

    fn editor_strip_rows(&self, cx: &gpui::App) -> Vec<PickerRow> {
        let Some(code) = self.tab_code() else {
            return Vec::new();
        };
        let front = code.active_id();
        let roots = code.roots.as_slice();
        let local_host = self.spawn_host(cx);
        code.by_recency()
            .into_iter()
            .filter_map(|id| {
                let Some(f) = self.buffer(id) else {
                    // A picture: its file's name and folder, never unsaved.
                    let image = self.image(id)?;
                    let local = image.host.id() == local_host;
                    let dir =
                        super::nav::path_crumbs(&image.path, if local { roots } else { &[] }).0;
                    return Some(PickerRow {
                        id,
                        name: image.name.clone(),
                        dir: dir.into(),
                        dirty: false,
                        front: Some(id) == front,
                    });
                };
                let dir = if f.untitled.is_some() {
                    String::new()
                } else {
                    let local = f.host.id() == local_host;
                    super::nav::path_crumbs(&f.path, if local { roots } else { &[] }).0
                };
                Some(PickerRow {
                    id,
                    name: f.label(),
                    dir: dir.into(),
                    dirty: f.dirty,
                    front: Some(id) == front,
                })
            })
            .collect()
    }

    /// The rows the query leaves, in the order Up/Down walks them.
    fn editor_strip_matches(&self, cap: usize, cx: &gpui::App) -> (Vec<PickerRow>, usize) {
        let q = self
            .editor
            .strip_picker
            .as_ref()
            .map(|p| p.input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let rows = self.editor_strip_rows(cx);
        if !q.is_empty() {
            let hits: Vec<PickerRow> = rows
                .into_iter()
                .filter(|r| r.name.to_lowercase().contains(&q) || r.dir.to_lowercase().contains(&q))
                .collect();
            return (hits, 0);
        }
        // Hidden files first — they are what the list is for — then the ones
        // the strip already shows. Rows are in recency order, and the strip
        // shows the first `cap` of it.
        let cap = cap.max(1).min(rows.len());
        let mut rows = rows;
        let shown: Vec<PickerRow> = rows.drain(..cap).collect();
        let hidden = rows.len();
        rows.extend(shown);
        (rows, hidden)
    }

    /// Opens the list, or closes it when it is open. `from_button` is whether
    /// the pointer is on the button: opened from the keyboard it is not, so a
    /// click outside closes the list straight away.
    pub(super) fn editor_toggle_strip_picker(
        &mut self,
        from_button: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.strip_picker.is_some() {
            self.editor_close_strip_picker(window, cx);
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(strip_search_placeholder()));
        input.update(cx, |state, cx| state.focus(window, cx));
        let _sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                if let Some(p) = this.editor.strip_picker.as_mut() {
                    p.hi = 0;
                }
                cx.notify();
            }
        });
        self.editor.strip_picker = Some(StripPicker {
            input,
            hi: 0,
            over_button: from_button,
            _sub,
        });
        cx.notify();
    }

    pub(super) fn editor_close_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.strip_picker.take().is_some() {
            self.focus_editor(window, cx);
            cx.notify();
        }
    }

    fn editor_strip_pick(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.strip_picker = None;
        if let Some(pos) = self
            .tab_code()
            .and_then(|c| c.files.iter().position(|f| *f == id))
        {
            self.editor_activate(pos, window, cx);
        }
        cx.notify();
    }

    fn on_strip_picker_key(
        &mut self,
        ev: &gpui::KeyDownEvent,
        cap: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (rows, _) = self.editor_strip_matches(cap, cx);
        let Some(p) = self.editor.strip_picker.as_mut() else {
            return;
        };
        let n = rows.len();
        match ev.keystroke.key.as_str() {
            "escape" => self.editor_close_strip_picker(window, cx),
            "up" if n > 0 => {
                p.hi = p.hi.min(n - 1).saturating_sub(1);
                cx.notify();
            }
            "down" if n > 0 => {
                p.hi = (p.hi + 1).min(n - 1);
                cx.notify();
            }
            "enter" => {
                if let Some(row) = rows.get(p.hi.min(n.saturating_sub(1))) {
                    let id = row.id;
                    self.editor_strip_pick(id, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The button at the end of the strip, with its list when open: `+N ▾`
    /// when files are hidden, the bare chevron otherwise. Drawn whenever the
    /// editor has a file.
    pub(super) fn render_strip_overflow(
        &self,
        hidden: &[BufferId],
        cap: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.tab_code().is_none_or(|c| c.files.is_empty()) {
            return None;
        }
        let theme = cx.theme();
        let unsaved = gpui::rgb(crate::ui::theme::semantics(cx).warning.ink);
        let open = self.editor.strip_picker.is_some();
        let hidden_dirty = hidden
            .iter()
            .any(|id| self.buffer(*id).is_some_and(|f| f.dirty));
        let button = h_flex()
            .id("editor-strip-more")
            .when(cfg!(test), |d| {
                d.debug_selector(|| "editor-strip-more".into())
            })
            .occlude()
            .flex_none()
            .h(px(CELL_H))
            .px(px(OVERFLOW_PAD_X))
            .gap(px(OVERFLOW_GAP))
            .items_center()
            .rounded(px(CELL_RADIUS))
            .text_size(rems(OVERFLOW_TEXT))
            .font_features(crate::ui::theme::tabular_figures())
            .text_color(theme.muted_foreground)
            .when(open, |d| d.bg(theme.sidebar_accent))
            .hover(|s| s.bg(theme.sidebar_accent).text_color(theme.foreground))
            .when(hidden_dirty, |d| {
                let dot = div()
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "editor-strip-more-dot".into())
                    })
                    .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                    .rounded_full()
                    .bg(unsaved);
                #[cfg(test)]
                let dot = {
                    let mut dot = dot;
                    crate::ui::paint_probe::note("editor-dirty-dot", &mut dot);
                    dot
                };
                d.child(dot)
            })
            .when(!hidden.is_empty(), |d| {
                d.child(
                    div()
                        .when(cfg!(test), |d| {
                            d.debug_selector(|| "editor-strip-more-count".into())
                        })
                        .child(format!("+{}", hidden.len())),
                )
            })
            .child(
                Icon::new(IconName::ChevronDown)
                    .with_size(px(OVERFLOW_CHEVRON))
                    .text_color(theme.muted_foreground),
            )
            .tooltip(|window, cx| {
                // Resolved on hover, not every frame.
                crate::ui::tab_strip::chord_tooltip(
                    t(L10nKey::EditorStripAllFiles),
                    "EditorSearchOpenFiles",
                    cx,
                )(window, cx)
                .into()
            })
            .on_hover(cx.listener(|this, over: &bool, _, _| {
                if let Some(p) = this.editor.strip_picker.as_mut() {
                    p.over_button = *over;
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                this.editor_toggle_strip_picker(true, window, cx);
            }));
        Some(
            div()
                .relative()
                .flex_none()
                .child(button)
                .children(self.render_strip_picker(cap, cx))
                .into_any_element(),
        )
    }

    fn render_strip_picker(&self, cap: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.editor.strip_picker.as_ref()?;
        let (rows, hidden) = self.editor_strip_matches(cap, cx);
        let querying = !picker.input.read(cx).value().trim().is_empty();
        let hi = picker.hi.min(rows.len().saturating_sub(1));
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let unsaved = gpui::rgb(crate::ui::theme::semantics(cx).warning.ink);
        let row_hover = gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().popover.hover);

        let heading = |label: SharedString| {
            div()
                .when(cfg!(test), |d| {
                    d.debug_selector(|| "editor-strip-heading".into())
                })
                .h(px(24.))
                .pt(px(4.))
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(rems(11. / 16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        let empty = rows.is_empty();
        let mut list = v_flex()
            .id("editor-strip-list")
            .max_h(px(PICKER_LIST_MAX_H))
            .overflow_y_scroll()
            .gap(px(1.));
        for (i, row) in rows.into_iter().enumerate() {
            if !querying && i == 0 && hidden > 0 {
                list = list.child(heading(
                    t_fmt(L10nKey::EditorStripHidden, &[("n", &hidden.to_string())]).into(),
                ));
            }
            if !querying && hidden > 0 && i == hidden {
                list = list.child(heading(t(L10nKey::EditorStripInBar).into()));
            }
            let id = row.id;
            let group: SharedString = format!("editor-strip-row-{i}").into();
            let slot = div()
                .id(("editor-strip-row-close", i))
                .relative()
                .flex_none()
                .size(px(18.))
                .rounded(px(4.))
                .hover(|s| s.bg(theme.muted))
                .when(row.dirty, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |s| s.invisible())
                            .child({
                                let dot = div()
                                    .when(cfg!(test), |d| {
                                        d.debug_selector(move || {
                                            format!("editor-strip-row-dot-{i}")
                                        })
                                    })
                                    .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                                    .rounded_full()
                                    .bg(unsaved);
                                #[cfg(test)]
                                let dot = {
                                    let mut dot = dot;
                                    crate::ui::paint_probe::note("editor-dirty-dot", &mut dot);
                                    dot
                                };
                                dot
                            }),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(row.dirty, |d| d.invisible())
                        .group_hover(group.clone(), |s| s.visible())
                        .child(Icon::new(IconName::Close).xsmall().text_color(muted)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.editor_close_buffer(id, window, cx);
                }));
            list = list.child(
                h_flex()
                    .id(("editor-strip-row", i))
                    .group(group)
                    .flex_none()
                    .h(px(PICKER_ROW_H))
                    .pl(px(8.))
                    .pr(px(4.))
                    .gap(px(8.))
                    .items_center()
                    .rounded(px(6.))
                    .when(i == hi, |d| d.bg(row_hover))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.editor.strip_picker.as_mut()
                            && p.hi != i
                        {
                            p.hi = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.editor_strip_pick(id, window, cx);
                    }))
                    .child(
                        div()
                            .min_w(px(40.))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_color(fg)
                            .when(row.front, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(row.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .text_size(rems(11.5 / 16.))
                            .text_color(muted)
                            .child(row.dir),
                    )
                    .child(slot),
            );
        }
        if empty {
            list = list.child(
                div()
                    .h(px(PICKER_ROW_H))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .text_color(muted)
                    .child(t(L10nKey::EditorStripNoMatch)),
            );
        }

        let rule = || div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(theme.border);
        // The footer acts on every file, whatever the query, so it is
        // decided from the editor's files, not the filtered rows.
        let show_saved = !self.editor_strip_others(true).is_empty();
        let show_others = !self.editor_strip_others(false).is_empty();
        let footer_button = |key: &'static str, label: L10nKey| {
            div()
                .id(key)
                .when(cfg!(test), |d| d.debug_selector(|| key.into()))
                .text_color(muted)
                .hover(|s| s.text_color(fg))
                .child(t(label))
        };
        let card = v_flex()
            .occlude()
            .w(px(PICKER_W))
            .p(px(5.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow_lg()
            .text_size(rems(CELL_TEXT))
            .on_key_down(
                cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                    this.on_strip_picker_key(ev, cap, window, cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this
                    .editor
                    .strip_picker
                    .as_ref()
                    .is_some_and(|p| p.over_button)
                {
                    return;
                }
                this.editor_close_strip_picker(window, cx);
            }))
            .child(
                h_flex()
                    .h(px(30.))
                    .px(px(4.))
                    .gap(px(3.))
                    .items_center()
                    .child(Icon::new(IconName::Search).xsmall().text_color(muted))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&picker.input)
                                .appearance(false)
                                .small()
                                .text_size(rems(CELL_TEXT)),
                        ),
                    ),
            )
            .child(rule())
            .child(list)
            .when(show_saved || show_others, |card| {
                card.child(rule()).child(
                    h_flex()
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(14.))
                        .items_center()
                        .text_size(rems(12. / 16.))
                        .when(show_saved, |row| {
                            row.child(
                                footer_button(
                                    "editor-strip-close-saved",
                                    L10nKey::EditorStripCloseSaved,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.editor.strip_picker = None;
                                        let ids = this.editor_strip_others(true);
                                        this.editor_close_buffers(ids, window, cx);
                                        this.focus_editor(window, cx);
                                    },
                                )),
                            )
                        })
                        .when(show_others, |row| {
                            row.child(
                                footer_button(
                                    "editor-strip-close-others",
                                    L10nKey::EditorStripCloseOthers,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.editor.strip_picker = None;
                                        let ids = this.editor_strip_others(false);
                                        this.editor_close_buffers(ids, window, cx);
                                        this.focus_editor(window, cx);
                                    },
                                )),
                            )
                        }),
                )
            });
        Some(
            div()
                .absolute()
                .right_0()
                .top(px(CELL_H + 4.))
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// Every file in the strip but the one in front — only the saved ones
    /// when `saved_only`.
    fn editor_strip_others(&self, saved_only: bool) -> Vec<BufferId> {
        let Some(code) = self.tab_code() else {
            return Vec::new();
        };
        let front = code.active_id();
        code.files
            .iter()
            .copied()
            .filter(|id| Some(*id) != front)
            .filter(|id| !saved_only || self.buffer(*id).is_some_and(|f| !f.dirty))
            .collect()
    }

    /// Whether the open-files picker is showing.
    pub(crate) fn editor_strip_picker_open(&self) -> bool {
        self.editor.strip_picker.is_some()
    }

    /// Whether the open-files picker is showing with the keyboard in its
    /// filter. What a test asks after a chord or a palette command.
    #[cfg(test)]
    pub(crate) fn editor_strip_picker_filter_focused(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        use gpui::Focusable as _;
        self.editor
            .strip_picker
            .as_ref()
            .is_some_and(|p| p.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// The keyboard and palette way into the open-files picker. With the
    /// picker showing, `toggle` closes it and otherwise it stays, its filter
    /// focused; with it closed it opens. `false` when the tab's editor holds
    /// no file, so the caller can let the chord go on.
    pub(crate) fn editor_search_open_files(
        &mut self,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(picker) = self.editor.strip_picker.as_ref() {
            if toggle {
                self.editor_close_strip_picker(window, cx);
            } else {
                picker.input.update(cx, |state, cx| state.focus(window, cx));
            }
            return true;
        }
        if self.tab_code().is_none_or(|c| c.files.is_empty()) {
            return false;
        }
        self.editor_toggle_strip_picker(false, window, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<BufferId> {
        (1..=n).map(gpui::EntityId::from).collect()
    }

    fn code(files: &[BufferId], active: usize) -> TabCode {
        let mut code = TabCode::new();
        code.files = files.to_vec();
        code.active = active;
        code
    }

    #[test]
    fn the_strip_keeps_the_files_last_in_front_in_their_strip_order() {
        let ids = ids(5);
        let mut code = code(&ids, 0);
        for front in [3, 1, 4] {
            code.active = front;
            code.note_front();
        }
        let (shown, hidden) = code.strip_split(3);
        // 4, 1 and 3 were in front last; they keep their strip positions.
        assert_eq!(shown, vec![(1, ids[1]), (3, ids[3]), (4, ids[4])]);
        // The rest by recency: 0 was in front before any of them, 2 never.
        assert_eq!(hidden, vec![ids[0], ids[2]]);
    }

    #[test]
    fn the_file_in_front_always_has_a_place() {
        let ids = ids(4);
        let mut code = code(&ids, 0);
        code.note_front();
        // Brought forward from outside the strip's view (a link, the list)
        // without a frame drawn in between.
        code.active = 3;
        let (shown, _) = code.strip_split(1);
        assert_eq!(shown, vec![(3, ids[3])]);
    }

    #[test]
    fn a_split_keeps_the_other_groups_order() {
        let ids = ids(4);
        let mut code = code(&ids[..2], 1);
        code.split = Some(super::super::split::OtherGroup {
            files: ids[2..].to_vec(),
            active: 0,
            focus_left: true,
        });
        code.note_front();
        code.swap_focus();
        code.note_front();
        code.swap_focus();
        code.note_front();
        assert_eq!(code.recent, vec![ids[1], ids[0], ids[2], ids[3]]);
    }

    const GLYPHS: OverflowGlyphs = OverflowGlyphs {
        plus: 7.,
        digit: 7.,
    };

    /// The room `n` cells take at [`TAB_MIN_W`], gaps between.
    fn cells_w(n: usize) -> f32 {
        n as f32 * TAB_MIN_W + (n - 1) as f32 * TAB_GAP
    }

    #[test]
    fn the_chevron_is_the_plus_n_button_without_its_dot_and_text() {
        for digits in 1..=3 {
            assert_eq!(
                overflow_w(digits, GLYPHS),
                chevron_w()
                    + crate::ui::tab_strip::ROW_STATUS_DOT
                    + 2. * OVERFLOW_GAP
                    + GLYPHS.plus
                    + digits as f32 * GLYPHS.digit,
                "{digits} digits: the +N button's width is unchanged"
            );
        }
        assert!(chevron_w() < overflow_w(1, GLYPHS));
    }

    #[test]
    fn all_fit_only_with_the_chevron_beside_them() {
        let boundary = cells_w(3) + chevron_w();
        assert_eq!(strip_fit(boundary, 3, GLYPHS), 3);
        assert!(
            strip_fit(boundary - 0.5, 3, GLYPHS) < 3,
            "half a point under the chevron's room, one file folds"
        );
        // The old rule held three in `cells_w(3)` alone: the boundary moved by
        // exactly the chevron.
        assert!(strip_fit(cells_w(3), 3, GLYPHS) < 3);
        assert_eq!(boundary - cells_w(3), chevron_w());
    }

    #[test]
    fn a_single_file_is_always_drawn() {
        for room in [0., 10., 56., cells_w(1) + chevron_w(), 1000.] {
            assert_eq!(strip_fit(room, 1, GLYPHS), 1, "room {room}");
        }
        assert_eq!(strip_fit(500., 0, GLYPHS), 0);
    }

    #[test]
    fn folding_still_reserves_the_plus_n_button() {
        // Room for five cells and the +N for the other five (one digit):
        let room = cells_w(5) + overflow_w(1, GLYPHS);
        assert_eq!(strip_fit(room, 10, GLYPHS), 5);
        assert_eq!(strip_fit(room - 0.5, 10, GLYPHS), 4);
        // Ten digits' worth of files: the digit count settles.
        let room = cells_w(3) + overflow_w(2, GLYPHS);
        assert_eq!(strip_fit(room, 100, GLYPHS), 3);
    }

    #[test]
    fn closed_files_leave_the_order() {
        let ids = ids(3);
        let mut code = code(&ids, 2);
        code.note_front();
        code.files.remove(1);
        code.active = 1;
        code.note_front();
        assert_eq!(code.recent, vec![ids[2], ids[0]]);
    }
}

#[cfg(test)]
mod gpui_tests {
    use super::super::nav::gpui_tests::open_rs;
    use super::*;
    use crate::core::config::{
        Config, DOCUMENT_RATIO_MAX, DOCUMENT_RATIO_MIN, DocumentLayout, TabBarPosition,
    };
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::editor_session::{self, SplitEditor, TabEditor};
    use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px, size};

    const TAB_SELECTORS: [&str; 12] = [
        "editor-file-tab-0",
        "editor-file-tab-1",
        "editor-file-tab-2",
        "editor-file-tab-3",
        "editor-file-tab-4",
        "editor-file-tab-5",
        "editor-file-tab-6",
        "editor-file-tab-7",
        "editor-file-tab-8",
        "editor-file-tab-9",
        "editor-file-tab-10",
        "editor-file-tab-11",
    ];
    const TAB_DOT_SELECTORS: [&str; 6] = [
        "editor-file-tab-dot-0",
        "editor-file-tab-dot-1",
        "editor-file-tab-dot-2",
        "editor-file-tab-dot-3",
        "editor-file-tab-dot-4",
        "editor-file-tab-dot-5",
    ];
    const NAMES: [&str; 10] = [
        "/repo/src/changelog-history-notes.md",
        "/repo/src/alpha-component.ts",
        "/repo/src/отчёт-по-продажам-за-квартал.md",
        "/repo/src/gamma-component.ts",
        "/repo/src/架构设计说明文档与接口约定.md",
        "/repo/src/order-reconciliation-pipeline-handler.ts",
        "/repo/src/epsilon-component.ts",
        "/repo/src/zeta-component.ts",
        "/repo/src/eta-component.ts",
        "/repo/src/crlf-notes-final.txt",
    ];

    struct Painted {
        header: Bounds<Pixels>,
        strip: Bounds<Pixels>,
        tabs: Vec<(usize, Bounds<Pixels>)>,
        more: Option<Bounds<Pixels>>,
        /// The `+N` text inside the button.
        count: Option<Bounds<Pixels>>,
        new_file: Bounds<Pixels>,
        switch: Bounds<Pixels>,
    }

    fn painted(vcx: &mut VisualTestContext, total: usize) -> Painted {
        let tabs = TAB_SELECTORS
            .iter()
            .take(total)
            .enumerate()
            .filter_map(|(pos, selector)| vcx.debug_bounds(selector).map(|b| (pos, b)))
            .collect();
        Painted {
            header: vcx
                .debug_bounds("editor-header")
                .expect("editor header should be painted"),
            strip: vcx
                .debug_bounds("editor-file-tabs")
                .expect("file tab strip should be painted"),
            tabs,
            more: vcx.debug_bounds("editor-strip-more"),
            count: vcx.debug_bounds("editor-strip-more-count"),
            new_file: vcx
                .debug_bounds("editor-new-file")
                .expect("new file button should be painted"),
            switch: vcx
                .debug_bounds("document-layout-switch")
                .expect("layout switch should be painted"),
        }
    }

    /// What the header must look like in whatever box it is in: the file in
    /// front among the cells, no cell under 56px or past the strip, the rest
    /// behind a button that is there whenever there is a file and counts
    /// them (`+N`) exactly when some file is left out, and every control
    /// clear of its neighbour.
    fn assert_header(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, label: &str) -> Painted {
        let (total, active) = app.read_with(vcx, |app, _| {
            let code = app.tab_code().expect("the tab has editor state");
            (code.files.len(), code.active)
        });
        let p = painted(vcx, total);
        let eps = px(0.5);
        let drawn: Vec<usize> = p.tabs.iter().map(|(pos, _)| *pos).collect();
        assert!(
            drawn.contains(&active),
            "{label}: the front file {active} of {total} must be drawn, drawn {drawn:?}"
        );
        for (pos, tab) in &p.tabs {
            assert!(
                tab.size.width >= px(56.) - eps,
                "{label}: tab {pos} is {:?} wide, under 56px",
                tab.size.width
            );
            assert!(
                tab.left() >= p.strip.left() - eps && tab.right() <= p.strip.right() + eps,
                "{label}: tab {pos} {tab:?} is clipped by the strip {:?}",
                p.strip
            );
        }
        for pair in p.tabs.windows(2) {
            assert!(
                pair[1].1.left() >= pair[0].1.right() - eps,
                "{label}: tabs {} and {} overlap",
                pair[0].0,
                pair[1].0
            );
        }
        assert_eq!(
            p.more.is_some(),
            total >= 1,
            "{label}: the chevron must be there exactly when there is a file ({total})"
        );
        assert_eq!(
            p.count.is_some(),
            p.tabs.len() < total,
            "{label}: +N must be there exactly when files are left out ({} of {total} drawn)",
            p.tabs.len()
        );
        if let (Some(more), Some(count)) = (p.more, p.count) {
            assert!(
                count.left() >= more.left() - eps && count.right() <= more.right() + eps,
                "{label}: +N {count:?} leaves its button {more:?}"
            );
        }
        let strip_end = p.more.map_or(p.strip.right(), |m| {
            assert!(
                m.left() >= p.strip.right() - eps,
                "{label}: +N {m:?} overlaps the strip {:?}",
                p.strip
            );
            m.right()
        });
        assert!(
            p.new_file.left() >= strip_end - eps,
            "{label}: the new-file tile {:?} overlaps what precedes it",
            p.new_file
        );
        assert!(
            p.switch.left() >= p.new_file.right() - eps,
            "{label}: the layout switch {:?} overlaps the new-file tile {:?}",
            p.switch,
            p.new_file
        );
        assert!(
            p.switch.right() <= p.header.right() + eps,
            "{label}: the layout switch {:?} leaves the header {:?}",
            p.switch,
            p.header
        );
        p
    }

    fn dock_config(vcx: &mut VisualTestContext, layout: DocumentLayout) {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.tab_bar_position = TabBarPosition::Left;
            cfg.document_layout = layout;
            cx.set_global(cfg);
        });
    }

    fn set_right_panel(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, open: bool) {
        if app.read_with(vcx, |app, cx| app.right_panel_open(cx)) != open {
            app.update_in(vcx, |app, _, cx| app.toggle_right_panel(cx));
        }
        vcx.run_until_parked();
    }

    fn resize(vcx: &mut VisualTestContext, width: f32, height: f32) {
        vcx.simulate_resize(size(px(width), px(height)));
        vcx.run_until_parked();
    }

    fn docked_px(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<f32> {
        vcx.update(|window, cx| app.read(cx).document_dock_px(window, cx))
    }

    /// Whether the header is in the column, below the title bar, rather than
    /// lifted into it.
    fn header_is_inline(vcx: &mut VisualTestContext) -> bool {
        let header = vcx
            .debug_bounds("editor-header")
            .expect("editor header should be painted");
        header.top() >= px(crate::ui::app::TITLE_BAR_HEIGHT) - px(1.)
    }

    fn set_zoom(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, zoom: f32) {
        app.update_in(vcx, |app, _, cx| app.set_editor_zoom(zoom, cx));
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| cx.global::<Config>().editor_zoom),
            zoom,
            "the zoom reached the config"
        );
    }

    /// The column widths at which the hoisted header first falls into the
    /// column when narrowing, and lifts again when widening, found from far
    /// above so no earlier answer is held. Both headers are checked on the way.
    fn hoist_boundary(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> (f32, f32) {
        resize(vcx, 2400., 800.);
        assert!(!header_is_inline(vcx), "a wide window lifts the header");
        let mut width = 2400.;
        while !header_is_inline(vcx) {
            width -= 5.;
            assert!(width > 900., "the header never moved into the column");
            resize(vcx, width, 800.);
        }
        // Back to the last lifted coarse step, then one point at a time.
        width += 5.;
        resize(vcx, 2400., 800.);
        resize(vcx, width, 800.);
        assert!(!header_is_inline(vcx));
        while !header_is_inline(vcx) {
            width -= 1.;
            resize(vcx, width, 800.);
        }
        assert_header(app, vcx, "first inline");
        let first_inline = docked_px(app, vcx).expect("still docked");
        while header_is_inline(vcx) {
            width += 1.;
            assert!(width < 2400., "the header never went back up");
            resize(vcx, width, 800.);
        }
        assert_header(app, vcx, "lifted again");
        (first_inline, docked_px(app, vcx).expect("docked"))
    }

    fn open_names(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, names: &[&str]) {
        for name in names {
            open_rs(app, vcx, name);
        }
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn the_front_file_is_drawn_and_no_cell_clips_or_shrinks_below_56px(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/a.rs", "/repo/b.rs"]);
        let p = assert_header(&app, &mut vcx, "two files in a wide fill");
        assert_eq!(p.tabs.len(), 2, "files that fit are all drawn");
        assert!(p.count.is_none(), "nothing is left out, so no count");
        assert!(p.more.is_some(), "the chevron is there all the same");

        dock_config(&mut vcx, DocumentLayout::Dock);
        open_names(&app, &mut vcx, &NAMES);

        // (width, height, layout, right panel, docked)
        let scenes = [
            (1440., 900., DocumentLayout::Dock, false, true),
            (1000., 700., DocumentLayout::Dock, false, true),
            (1440., 900., DocumentLayout::Dock, true, true),
            (900., 700., DocumentLayout::Fill, false, false),
            (1440., 900., DocumentLayout::Fill, true, false),
        ];
        // The editor's zoom scales the text below the header, never the
        // header: what is drawn at 100 % is what is drawn at every zoom.
        let mut at_full: Vec<(usize, Pixels, Pixels, Pixels)> = Vec::new();
        for zoom in [1.0, 0.5, 3.0] {
            set_zoom(&app, &mut vcx, zoom);
            let mut seen = Vec::new();
            for (width, height, layout, panel, docked) in scenes {
                app.update_in(&mut vcx, |app, _, cx| app.set_document_layout(layout, cx));
                set_right_panel(&app, &mut vcx, panel);
                resize(&mut vcx, width, height);
                assert_eq!(
                    docked_px(&app, &mut vcx).is_some(),
                    docked,
                    "{width}x{height} {layout:?} panel {panel} zoom {zoom}: the column is {}docked",
                    if docked { "" } else { "not " }
                );
                // Fronted from the far end of the strip, then from the middle,
                // then from a file recency has never seen.
                for pos in [9, 0, 5, 3, 9] {
                    app.update_in(&mut vcx, |app, window, cx| {
                        app.editor_activate(pos, window, cx);
                    });
                    vcx.run_until_parked();
                    let label = format!(
                        "{width}x{height} {layout:?} panel {panel} zoom {zoom} front {pos}"
                    );
                    let p = assert_header(&app, &mut vcx, &label);
                    assert!(
                        p.count.is_some(),
                        "{label}: ten files cannot all fit, +N expected"
                    );
                    seen.push((
                        p.tabs.len(),
                        p.header.size.width,
                        p.strip.size.width,
                        p.more.map_or(px(0.), |m| m.size.width),
                    ));
                }
            }
            if zoom == 1.0 {
                at_full = seen;
            } else {
                assert_eq!(
                    seen, at_full,
                    "zoom {zoom}: cell count and header, strip and +N widths differ from 100 %"
                );
            }
        }
    }

    fn click_at(vcx: &mut VisualTestContext, at: gpui::Point<Pixels>) {
        vcx.simulate_mouse_move(at, None, gpui::Modifiers::none());
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.simulate_click(at, gpui::Modifiers::none());
        vcx.run_until_parked();
    }

    fn picker_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        app.read_with(&*vcx, |app, _| app.editor.strip_picker.is_some())
    }

    #[gpui::test]
    fn one_file_has_a_chevron_that_opens_the_list_without_headings(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/only.rs"]);

        let p = assert_header(&app, &mut vcx, "one file in a wide fill");
        let more = p.more.expect("the chevron is painted for a single file");
        assert!(p.count.is_none(), "nothing hidden, so no +N text");
        assert!(
            more.left() >= p.header.left() && more.right() <= p.header.right(),
            "the chevron {more:?} is inside the header {:?}",
            p.header
        );
        assert!(
            more.left() >= p.strip.right() && more.right() <= p.new_file.left(),
            "the chevron {more:?} sits between the strip {:?} and the new-file tile {:?}",
            p.strip,
            p.new_file
        );
        // Bare chevron: its box is the width `chevron_w` reserves, less the
        // gap before it.
        assert_eq!(
            more.size.width,
            px(chevron_w() - HEADER_GAP),
            "the bare chevron is as wide as the strip reserved"
        );

        assert!(!picker_open(&app, &mut vcx));
        click_at(&mut vcx, more.center());
        assert!(picker_open(&app, &mut vcx), "a click on the chevron opens");
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.editor_strip_rows(cx).len()),
            1,
            "the one file is the one row"
        );
        assert!(
            vcx.debug_bounds("editor-strip-heading").is_none(),
            "nothing hidden, so no section headings over the only list"
        );
        assert!(
            vcx.debug_bounds("editor-strip-more-count").is_none(),
            "the open list does not add a count"
        );

        click_at(&mut vcx, more.center());
        assert!(!picker_open(&app, &mut vcx), "a second click closes");
    }

    /// Open the picker from the keyboard and let it paint.
    fn open_picker(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        app.update_in(vcx, |app, window, cx| {
            app.editor_toggle_strip_picker(false, window, cx);
        });
        vcx.run_until_parked();
        assert!(picker_open(app, vcx), "the picker is open");
    }

    fn footer_painted(vcx: &mut VisualTestContext) -> (bool, bool) {
        (
            vcx.debug_bounds("editor-strip-close-saved").is_some(),
            vcx.debug_bounds("editor-strip-close-others").is_some(),
        )
    }

    /// Covers the `show_saved`/`show_others` false branches of the footer
    /// (no rule, no row) and the count-free placeholder.
    #[gpui::test]
    fn one_file_has_no_footer_and_a_count_free_placeholder(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/only.rs"]);
        open_picker(&app, &mut vcx);
        assert_eq!(
            footer_painted(&mut vcx),
            (false, false),
            "nothing to close, so no footer button"
        );
        let placeholder = strip_search_placeholder();
        assert_eq!(placeholder.as_ref(), t(L10nKey::EditorStripSearch));
        assert!(
            !placeholder.chars().any(|c| c.is_ascii_digit()),
            "the placeholder carries no count: {placeholder}"
        );
    }

    /// Covers the `when(show_saved)` and `when(show_others)` true branches.
    #[gpui::test]
    fn two_saved_files_offer_both_footer_actions(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/a.rs", "/repo/b.rs"]);
        open_picker(&app, &mut vcx);
        assert_eq!(footer_painted(&mut vcx), (true, true));
    }

    /// Covers `show_saved` false with `show_others` true: the one other
    /// file is unsaved, so only "Close others" has work.
    #[gpui::test]
    fn an_unsaved_other_file_hides_close_saved_only(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/a.rs", "/repo/b.rs"]);
        app.update_in(&mut vcx, |app, _, cx| {
            let other = app.editor_strip_others(false);
            assert_eq!(other.len(), 1, "one file besides the front one");
            app.buffer_mut(other[0]).unwrap().dirty = true;
            cx.notify();
        });
        open_picker(&app, &mut vcx);
        assert_eq!(footer_painted(&mut vcx), (false, true));
    }

    #[gpui::test]
    fn hidden_files_keep_their_headings_and_count(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1000., 700.);
        open_names(&app, &mut vcx, &NAMES);
        let p = assert_header(&app, &mut vcx, "ten files");
        assert!(p.count.is_some(), "ten files fold into a +N");
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_toggle_strip_picker(true, window, cx);
        });
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("editor-strip-heading").is_some(),
            "with files hidden the list is sectioned"
        );
    }

    #[gpui::test]
    fn a_keyboard_opened_list_closes_on_an_outside_press(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Fill);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &["/repo/only.rs"]);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_toggle_strip_picker(false, window, cx);
        });
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| {
            app.editor
                .strip_picker
                .as_ref()
                .is_some_and(|p| !p.over_button)
        }));
        let header = vcx.debug_bounds("editor-header").expect("header");
        let away = gpui::point(px(10.), header.bottom() + px(300.));
        vcx.simulate_mouse_move(away, None, gpui::Modifiers::none());
        vcx.simulate_mouse_down(away, gpui::MouseButton::Left, gpui::Modifiers::none());
        vcx.run_until_parked();
        assert!(
            !picker_open(&app, &mut vcx),
            "a press outside closes a list the pointer never entered the button for"
        );
    }

    #[gpui::test]
    fn the_hoisted_header_with_one_file_never_clips_the_chevron(cx: &mut TestAppContext) {
        // The header is never hoisted on macOS.
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        open_names(&app, &mut vcx, &["/repo/only.rs"]);
        // `hoist_boundary` runs `assert_header` on both sides of the point
        // the header leaves the title bar, so the chevron is checked at the
        // narrowest the hoisted header gets.
        let (first_inline, lifted) = hoist_boundary(&app, &mut vcx);
        assert!(
            lifted > first_inline,
            "the hysteresis keeps the lifted header from flapping ({first_inline} vs {lifted})"
        );
        let p = painted(&mut vcx, 1);
        let more = p.more.expect("the chevron is painted");
        assert!(
            more.right() <= p.header.right() && more.left() >= p.strip.right(),
            "the chevron {more:?} is whole inside the lifted header {:?}",
            p.header
        );
    }

    #[gpui::test]
    fn a_restored_split_keeps_each_groups_front_file_drawn_with_stale_recency(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::ui::code_editor::tests::test_real_dir(dir.path());
        let mut paths = Vec::new();
        for i in 0..10 {
            let p = root.join(format!("long-module-filename-entry-{i:02}.rs"));
            std::fs::write(&p, "fn main() {}\n").unwrap();
            paths.push(p);
        }

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);

        let state = TabEditor {
            files: paths[..5].to_vec(),
            active: 4,
            visible: true,
            split: Some(SplitEditor {
                files: paths[5..].to_vec(),
                active: 3,
                focused: true,
            }),
        };
        let tab = app.read_with(&vcx, |app, _| app.tabs[1].tree_id.get());
        vcx.update(|_, cx| editor_session::put(cx, tab, state));
        app.update_in(&mut vcx, |app, window, cx| {
            app.active = 1;
            app.editor.restored.remove(&tab);
            app.editor_restore_active(window, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !app.read_with(&vcx, |app, _| app.tab_code().is_some_and(TabCode::is_split)) {
            assert!(
                std::time::Instant::now() < deadline,
                "restored split files never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            vcx.run_until_parked();
        }
        vcx.run_until_parked();
        assert_header(&app, &mut vcx, "restored split");

        // Recency that says nothing about the file in front.
        app.update_in(&mut vcx, |app, _, cx| {
            let code = app.tab_code_mut().expect("the tab has editor state");
            code.recent = vec![code.files[0], code.files[1], code.files[2]];
            code.active = 4;
            cx.notify();
        });
        vcx.run_until_parked();
        assert_header(&app, &mut vcx, "stale recency, front last in strip");

        // The other group takes the focus; its front file is the one that
        // has to be there.
        let left_id = app.read_with(&vcx, |app, _| {
            app.tab_code()
                .and_then(TabCode::other_active_id)
                .expect("a split has a second group")
        });
        let left_input = app.read_with(&vcx, |app, _| {
            app.buffer(left_id)
                .expect("the other front is open")
                .input
                .clone()
        });
        app.update_in(&mut vcx, |app, window, cx| {
            let code = app.tab_code_mut().expect("the tab has editor state");
            code.recent = vec![code.files[0], code.files[1]];
            left_input.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        });
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| app.tab_code().and_then(TabCode::active_id)),
            Some(left_id),
            "focus moved to the other group"
        );
        assert_header(&app, &mut vcx, "other group focused");
    }

    /// Editor F3 across every theme (verification gate 7): the built-ins and
    /// the whole generated grid, light, dark and weak, with no exclusions.
    /// Each theme is installed through `apply_theme` and the editor drawn with
    /// a dirty active tab, a dirty inactive tab, dirty files behind `+N` and
    /// the picker open, so every unsaved-dot site is built in the frame. The
    /// dots' paint, read from the probe, must be the cached warning ink (so a
    /// grey dot fails), and that ink must clear the 3:1 COMPONENT floor on
    /// every fill the renderer puts behind a visible dot: the column surface
    /// (inactive tab, `+N` at rest), `sidebar_accent` over it (active tab,
    /// `+N` open or hovered), and the picker's popover and highlighted row.
    /// A dot under the pointer is hidden for the close button, so hover fills
    /// of tabs and picker rows are not pairs.
    #[gpui::test]
    fn dirty_dots_are_the_warning_ink_and_clear_3_to_1_in_every_theme(cx: &mut TestAppContext) {
        const COMPONENT_FLOOR: f32 = 3.0;
        crate::core::config::pin_test_config_dir();
        let _probe = crate::ui::paint_probe::scope();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);
        let ids: Vec<_> = (0..6)
            .map(|i| open_rs(&app, &mut vcx, &format!("/sweep/dirty-file-{i}.rs")))
            .collect();
        app.update_in(&mut vcx, |app, window, cx| {
            for id in &ids {
                app.buffer_mut(*id).unwrap().dirty = true;
            }
            app.editor_toggle_strip_picker(true, window, cx);
        });
        vcx.run_until_parked();

        let mut themes = crate::ui::presets::builtins();
        themes.extend(crate::ui::presets::generated_theme_grid());
        let total = themes.len();
        let theme_ids: Vec<String> = themes.iter().map(|t| t.id.clone()).collect();
        vcx.update(|_, cx| cx.set_global(crate::ui::presets::Themes(themes)));

        let mut misses = Vec::new();
        for id in &theme_ids {
            vcx.update(|window, cx| {
                let mut cfg = cx.global::<Config>().clone();
                cfg.theme_follow_system = false;
                cfg.theme_preset = id.clone();
                cx.set_global(cfg);
                crate::ui::theme::apply_theme(Some(window), cx);
            });
            app.update_in(&mut vcx, |_, _, cx| cx.notify());
            vcx.run_until_parked();
            crate::ui::paint_probe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });

            let active_pos = app.read_with(&vcx, |app, _| app.tab_code().unwrap().active);
            let inactive_drawn = TAB_DOT_SELECTORS
                .iter()
                .enumerate()
                .any(|(pos, sel)| pos != active_pos && vcx.debug_bounds(sel).is_some());
            for (site, drawn) in [
                (
                    "active tab dot",
                    vcx.debug_bounds(TAB_DOT_SELECTORS[active_pos]).is_some(),
                ),
                ("inactive tab dot", inactive_drawn),
                (
                    "+N dot",
                    vcx.debug_bounds("editor-strip-more-dot").is_some(),
                ),
                (
                    "picker row dot",
                    vcx.debug_bounds("editor-strip-row-dot-0").is_some(),
                ),
            ] {
                assert!(drawn, "{id}: the {site} was not drawn");
            }

            let (warning, fills) = vcx.update(|_, cx| {
                let hex = crate::ui::paint_probe::hsla24;
                let column = crate::ui::theme::workspace_surface_color(cx);
                let popover = column.blend(cx.theme().popover);
                let row_hi: gpui::Hsla =
                    gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().popover.hover).into();
                (
                    crate::ui::theme::semantics(cx).warning.ink,
                    [
                        ("column surface", hex(column)),
                        (
                            "sidebar_accent",
                            hex(column.blend(cx.theme().sidebar_accent)),
                        ),
                        ("picker popover", hex(popover)),
                        ("picker highlighted row", hex(popover.blend(row_hi))),
                    ],
                )
            });
            let dots = crate::ui::paint_probe::all("editor-dirty-dot");
            if dots.len() < 4 {
                misses.push(format!("{id}: {} dirty dots built, want >= 4", dots.len()));
            }
            for dot in &dots {
                if dot.fill != Some(warning) || dot.fill_alpha != Some(1.) {
                    misses.push(format!(
                        "{id}: dot paint {:?} alpha {:?}, want the warning ink {warning:06x}",
                        dot.fill, dot.fill_alpha
                    ));
                }
            }
            for (name, fill) in fills {
                let ratio = crate::ui::presets::contrast(warning, fill);
                if ratio < COMPONENT_FLOOR {
                    misses.push(format!(
                        "{id}: dot {warning:06x} on {name} {fill:06x} = {ratio:.2}:1"
                    ));
                }
            }
        }
        assert!(
            misses.is_empty(),
            "{} misses over {total} themes, first 12:\n{}",
            misses.len(),
            misses
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// The unsaved mark's paint, as the builders put it: every dirty dot
    /// drawn since the last clear is filled with the theme's cached warning
    /// ink (editor F3), none in the grey it used to be.
    fn assert_dirty_dots_warning(vcx: &mut VisualTestContext, at: &str) {
        let (warning, muted) = vcx.update(|_, cx| {
            (
                crate::ui::theme::semantics(cx).warning.ink,
                crate::ui::paint_probe::hsla24(cx.theme().muted_foreground),
            )
        });
        let dots = crate::ui::paint_probe::all("editor-dirty-dot");
        assert!(!dots.is_empty(), "{at}: no unsaved dot was built");
        for dot in &dots {
            assert_eq!(
                dot.fill,
                Some(warning),
                "{at}: the warning ink, not {muted:06x}"
            );
            assert_eq!(dot.fill_alpha, Some(1.), "{at}: an opaque mark");
        }
        crate::ui::paint_probe::clear();
    }

    #[gpui::test]
    fn tab_and_picker_dirty_dots_are_5px(cx: &mut TestAppContext) {
        let _probe = crate::ui::paint_probe::scope();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);

        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(open_rs(&app, &mut vcx, &format!("/dots/dirty-file-{i}.rs")));
        }
        // The buffers are marked dirty the way the editor does it, and the
        // window is told to draw them: without the notification a frame is
        // only drawn if something else asks for one, which a long run of other
        // tests in the process can make late.
        app.update_in(&mut vcx, |app, window, cx| {
            for id in &ids {
                app.buffer_mut(*id).unwrap().dirty = true;
            }
            cx.notify();
            window.refresh();
        });
        let active_pos = app.read_with(&vcx, |app, _| app.tab_code().unwrap().active);
        vcx.run_until_parked();

        let tab_dot = vcx
            .debug_bounds(TAB_DOT_SELECTORS[active_pos])
            .expect("dirty tab dot should be painted");
        assert_eq!(
            tab_dot.size,
            size(px(5.), px(5.)),
            "tab dirty dot must be 5px"
        );

        let more_dot = vcx
            .debug_bounds("editor-strip-more-dot")
            .expect("+N dirty dot should be painted");
        assert_eq!(
            more_dot.size,
            size(px(5.), px(5.)),
            "+N dirty dot must be 5px"
        );
        assert_dirty_dots_warning(&mut vcx, "strip and +N");

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_toggle_strip_picker(true, window, cx);
        });
        vcx.run_until_parked();

        let picker_dot = vcx
            .debug_bounds("editor-strip-row-dot-0")
            .expect("picker dirty row dot should be painted");
        assert_eq!(
            picker_dot.size,
            size(px(5.), px(5.)),
            "picker row dirty dot must be 5px"
        );
        assert_dirty_dots_warning(&mut vcx, "picker");
    }

    /// A clean file has no dot and a dirty one has one, and on either the
    /// slot still turns into the close button under the pointer: clicking
    /// it closes the clean file and asks about the dirty one.
    #[gpui::test]
    fn the_dirty_dot_marks_only_unsaved_files_and_yields_to_close_on_hover(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);
        let ids: Vec<_> = (0..3)
            .map(|i| open_rs(&app, &mut vcx, &format!("/dots/hover-file-{i}.rs")))
            .collect();
        app.update_in(&mut vcx, |app, _, cx| {
            app.buffer_mut(ids[1]).unwrap().dirty = true;
            cx.notify();
        });
        vcx.run_until_parked();
        let pos_of = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext, id: BufferId| {
            app.read_with(&*vcx, |app, _| {
                app.tab_code()
                    .and_then(|code| code.files.iter().position(|f| *f == id))
                    .expect("the file is in the strip")
            })
        };
        let (clean, dirty) = (
            pos_of(&app, &mut vcx, ids[0]),
            pos_of(&app, &mut vcx, ids[1]),
        );
        assert!(
            vcx.debug_bounds(TAB_DOT_SELECTORS[clean]).is_none(),
            "clean: no dot"
        );
        assert!(
            vcx.debug_bounds(TAB_DOT_SELECTORS[dirty]).is_some(),
            "dirty: a dot"
        );

        // The slot is the tab's last 16px before its 6px end padding.
        let slot_centre = |vcx: &mut VisualTestContext, pos: usize| {
            let tab = vcx
                .debug_bounds(TAB_SELECTORS[pos])
                .expect("the tab is drawn");
            gpui::point(tab.right() - px(6. + 8.), tab.center().y)
        };
        let hover_click = |vcx: &mut VisualTestContext, at: gpui::Point<Pixels>| {
            vcx.simulate_mouse_move(at, None, gpui::Modifiers::none());
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            vcx.simulate_click(at, gpui::Modifiers::none());
            vcx.run_until_parked();
        };

        let at = slot_centre(&mut vcx, dirty);
        hover_click(&mut vcx, at);
        assert!(
            vcx.has_pending_prompt(),
            "the dirty tab's dot gave way to close, which asks first"
        );
        vcx.simulate_prompt_answer(t(L10nKey::Cancel));
        vcx.run_until_parked();

        let at = slot_centre(&mut vcx, clean);
        hover_click(&mut vcx, at);
        assert!(
            app.read_with(&vcx, |app, _| app
                .tab_code()
                .is_some_and(|code| !code.files.contains(&ids[0]))),
            "the clean tab's close button closed it"
        );
    }

    fn hoisted_setup(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(1. / 3., cx));
        resize(&mut vcx, 2400., 800.);
        open_names(&app, &mut vcx, &NAMES[..3]);
        (app, vcx, streams)
    }

    #[gpui::test]
    fn the_hoisted_header_moves_into_the_column_when_it_cannot_fit_and_returns_with_a_margin(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = hoisted_setup(cx);
        assert!(
            !header_is_inline(&mut vcx),
            "a wide window lifts the header"
        );
        assert_header(&app, &mut vcx, "wide, lifted");

        // The files were opened at the default size. Growing the interface
        // font afterwards widens `+N`, so the boundary swept below only holds
        // if the glyph metrics were measured again for the new size.
        app.update_in(&mut vcx, |app, _, cx| {
            let delta = 20. - app.ui_font_size(cx);
            app.change_ui_font_size(delta, cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            vcx.update(|window, _| window.rem_size()),
            px(20.),
            "the font change reached the window"
        );
        assert_header(&app, &mut vcx, "wide, lifted, larger font");

        // Narrowing in coarse steps finds roughly where it stops fitting.
        let mut width = 2400.;
        while !header_is_inline(&mut vcx) {
            width -= 5.;
            assert!(width > 900., "the header never moved into the column");
            resize(&mut vcx, width, 800.);
        }
        // Again from far above, so no earlier answer is being held, one point
        // at a time from just above that.
        resize(&mut vcx, 2400., 800.);
        let mut down = width + 5.;
        resize(&mut vcx, down, 800.);
        assert!(!header_is_inline(&mut vcx));
        let mut last_lifted = docked_px(&app, &mut vcx).expect("docked");
        assert_header(&app, &mut vcx, "narrowest lifted");
        loop {
            down -= 1.;
            resize(&mut vcx, down, 800.);
            let column = docked_px(&app, &mut vcx).expect("still docked");
            if header_is_inline(&mut vcx) {
                assert_header(&app, &mut vcx, "first inline");
                let first_inline = column;

                // Widening again: held in the column through the margin.
                let mut up = down;
                loop {
                    up += 1.;
                    assert!(up < down + 200., "the header never went back up");
                    resize(&mut vcx, up, 800.);
                    if !header_is_inline(&mut vcx) {
                        break;
                    }
                }
                assert_header(&app, &mut vcx, "back up");
                let back_up = docked_px(&app, &mut vcx).expect("docked");
                assert!(
                    back_up - first_inline > 8.,
                    "lifted again {back_up} after dropping at {first_inline}: no margin"
                );
                assert!(
                    back_up - last_lifted < 9.,
                    "lifted again {back_up}, more than 8px past {last_lifted}"
                );
                // And a point narrower does not flip it back.
                resize(&mut vcx, up - 1., 800.);
                assert!(!header_is_inline(&mut vcx), "flickers inside the margin");
                break;
            }
            assert_header(&app, &mut vcx, "lifted");
            last_lifted = column;
        }

        // The zoom scales the text under the header, not the header, so the
        // column width where it falls into the column, and where it is lifted
        // again, are the same at every zoom.
        set_zoom(&app, &mut vcx, 1.0);
        let at_full = hoist_boundary(&app, &mut vcx);
        assert!(
            at_full.1 - at_full.0 > 8.,
            "{at_full:?}: lifted again with no margin past the drop"
        );
        for zoom in [0.5, 3.0] {
            set_zoom(&app, &mut vcx, zoom);
            assert_eq!(
                hoist_boundary(&app, &mut vcx),
                at_full,
                "zoom {zoom}: the hoisted header's fallback boundary moved"
            );
        }
    }

    #[gpui::test]
    fn dragging_the_column_and_opening_the_panel_move_the_header_with_them(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = hoisted_setup(cx);
        resize(&mut vcx, 1900., 800.);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(DOCUMENT_RATIO_MAX, cx)
        });
        vcx.run_until_parked();
        assert!(!header_is_inline(&mut vcx), "the widest column lifts it");
        assert_header(&app, &mut vcx, "widest column");

        let mut ratio = DOCUMENT_RATIO_MAX;
        while !header_is_inline(&mut vcx) {
            ratio -= 0.01;
            assert!(
                ratio >= DOCUMENT_RATIO_MIN - 0.005,
                "narrowing the column never moved the header"
            );
            app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(ratio, cx));
            vcx.run_until_parked();
        }
        assert_header(&app, &mut vcx, "column narrowed");

        // With the detail panel open nothing sits at the window edge any
        // more, so a column too narrow for the trailing chrome has the room
        // again — with the Agents pill, which stands over it either way, still
        // reserved.
        let mut ratio = DOCUMENT_RATIO_MIN;
        loop {
            app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(ratio, cx));
            set_right_panel(&app, &mut vcx, false);
            let closed = header_is_inline(&mut vcx);
            set_right_panel(&app, &mut vcx, true);
            if closed && docked_px(&app, &mut vcx).is_some() && !header_is_inline(&mut vcx) {
                break;
            }
            ratio += 0.01;
            assert!(
                ratio <= DOCUMENT_RATIO_MAX,
                "no column width keeps the header lifted with the panel open and not closed"
            );
        }
        assert_header(&app, &mut vcx, "panel open");
        set_right_panel(&app, &mut vcx, false);
        assert!(header_is_inline(&mut vcx), "panel closed again");
        assert_header(&app, &mut vcx, "panel closed again");
    }

    /// While the header is lifted into the title bar the Agents pill is drawn
    /// in that same bar, at the right end of the strip row, so no control of
    /// the header may stand under it and the close tile keeps its 8px of air
    /// from it. A header that has fallen into the column is below the bar and
    /// has nothing to clear. Returns whether the header was lifted.
    fn assert_clear_of_pill(vcx: &mut VisualTestContext, what: &str) -> bool {
        let pill = vcx
            .debug_bounds("agent-mode-pill")
            .unwrap_or_else(|| panic!("{what}: the Agents pill is not drawn"));
        let header = vcx
            .debug_bounds("editor-header")
            .unwrap_or_else(|| panic!("{what}: the editor header is not drawn"));
        if header_is_inline(vcx) {
            assert!(
                header.top() + px(1.) >= pill.bottom(),
                "{what}: the in-column header {header:?} rises into the pill {pill:?}"
            );
            return false;
        }
        let switch = vcx
            .debug_bounds("document-layout-switch")
            .unwrap_or_else(|| panic!("{what}: the layout switch is not drawn"));
        let eps = px(0.5);
        assert!(
            switch.right() <= pill.left(),
            "{what}: the layout switch {switch:?} is drawn over the Agents pill {pill:?}"
        );
        let close = vcx
            .debug_bounds("editor-panel-close")
            .unwrap_or_else(|| panic!("{what}: the close button is not drawn"));
        assert!(
            close.right() <= header.right(),
            "{what}: the close button {close:?} is outside its header {header:?}"
        );
        let close_right = close.right();
        let gap = pill.left() - close_right;
        assert!(
            gap >= px(8.) - eps,
            "{what}: {gap:?} between the close tile ({close_right:?}) and the Agents pill \
             {pill:?}; chrome groups keep 8px"
        );
        // The panel's own 1px rule is the only spacing beyond the 8px.
        assert!(
            gap <= px(9.) + eps,
            "{what}: {gap:?} between the close tile and the Agents pill: more than the 8px \
             group gap and the panel's rule"
        );
        true
    }

    /// The header and the Agents pill share the title bar: whatever the
    /// count, the language, the interface font, the panel and the column's
    /// share of the window, the header stands clear of the pill or has gone
    /// into the column.
    #[gpui::test]
    fn the_hoisted_header_stands_clear_of_the_agents_pill(cx: &mut TestAppContext) {
        use crate::core::config::{UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX};
        use crate::ui::tab_strip::overflow_gpui_tests::{
            LOCALES, agents_wait, set_ui_font_size, use_locale,
        };
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 12);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &NAMES);

        // (width, height, column ratio, panel open)
        let scenes = [
            (1440., 900., 0.5, false),
            (1440., 900., 1. / 3., false),
            (1440., 900., 0.5, true),
            (1000., 700., 0.5, false),
            (1000., 700., 0.5, true),
            (900., 700., 1. / 3., false),
        ];
        let mut lifted_at_rest = false;
        for count in [0usize, 1, 12] {
            if count > 0 {
                agents_wait(&app, &mut vcx, &mut streams, count);
            }
            for (locale, agents) in LOCALES {
                use_locale(&app, &mut vcx, locale, agents);
                for font in [UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX] {
                    set_ui_font_size(&app, &mut vcx, font);
                    for (index, (width, height, ratio, panel)) in scenes.into_iter().enumerate() {
                        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(ratio, cx));
                        set_right_panel(&app, &mut vcx, panel);
                        resize(&mut vcx, width, height);
                        if docked_px(&app, &mut vcx).is_none() {
                            continue;
                        }
                        for pos in [9, 0] {
                            app.update_in(&mut vcx, |app, window, cx| {
                                app.editor_activate(pos, window, cx);
                            });
                            vcx.run_until_parked();
                            let what = format!(
                                "{locale} {font}px {count} agents, {width}x{height} ratio \
                                 {ratio:.2} panel {panel}, front {pos}"
                            );
                            assert_header(&app, &mut vcx, &what);
                            let lifted = assert_clear_of_pill(&mut vcx, &what);
                            if index == 0
                                && count == 0
                                && locale == "en"
                                && font == UI_FONT_SIZE_DEFAULT
                            {
                                lifted_at_rest |= lifted;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            lifted_at_rest,
            "the half-width column at 1440px, panel closed, keeps its header in the title \
             bar, so the pill has something to clear"
        );
        crate::ui::i18n::set_locale("en");
        set_ui_font_size(&app, &mut vcx, UI_FONT_SIZE_DEFAULT);
    }

    /// Narrowing the window walks the header through the lifted layout, the
    /// fallback and back: at every width, panel open or closed, before and
    /// after the pill grows two digits, the header is clear of the pill or in
    /// the column.
    #[gpui::test]
    fn narrowing_the_window_never_puts_the_header_under_the_agents_pill(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::agents_wait;
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 12);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 800.);
        open_names(&app, &mut vcx, &NAMES[..3]);

        let (mut lifted, mut inline) = (0, 0);
        for count in [0usize, 12] {
            if count > 0 {
                agents_wait(&app, &mut vcx, &mut streams, count);
            }
            for panel in [false, true] {
                set_right_panel(&app, &mut vcx, panel);
                let mut width = 2400.;
                while width >= 900. {
                    resize(&mut vcx, width, 800.);
                    if docked_px(&app, &mut vcx).is_some() {
                        let what = format!("{width}px, panel {panel}, {count} agents");
                        assert_header(&app, &mut vcx, &what);
                        match assert_clear_of_pill(&mut vcx, &what) {
                            true => lifted += 1,
                            false => inline += 1,
                        }
                    }
                    width -= 23.;
                }
            }
        }
        assert!(
            lifted > 0 && inline > 0,
            "the sweep has to cover both layouts: {lifted} lifted, {inline} in the column"
        );
    }

    /// A real object id, forty hex digits, which the header draws as its first
    /// eight.
    const COMMIT_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A subject the header is opened on: a pull request's number, a commit
    /// (each with the label the UI always gives it: the commit's summary or the
    /// pull request's title), a commit with no label at all as the bare-source
    /// control, and refs that are long and Unicode, which the header has always
    /// accepted whole. The first three are short enough to be drawn whole in
    /// every state.
    enum DiffRef {
        Patch(&'static str),
        Commit(&'static str),
        BareCommit(&'static str),
    }

    const DIFF_REFS: [DiffRef; 5] = [
        DiffRef::Patch("974"),
        DiffRef::Commit(COMMIT_SHA),
        DiffRef::BareCommit(COMMIT_SHA),
        DiffRef::Patch(
            "origin/feature/a-very-long-branch-name-that-keeps-going-well-past-any-header",
        ),
        DiffRef::Patch("功能/分支-идентификатор-очень-длинное-название-ветки-для-проверки-ширины"),
    ];

    /// How many of [`DIFF_REFS`], from the first, are drawn whole.
    const WHOLE_REFS: usize = 3;

    /// What the commit, or the pull request, is about, as the UI always sets
    /// it (`scm/detail.rs` and `github/detail.rs`): the same words for every
    /// labelled subject, so the point a header gives way at is comparable.
    fn real_label() -> Option<crate::terminal::git_diff::CommitLabel> {
        Some(crate::terminal::git_diff::CommitLabel {
            subject:
                "feat(checkout): add tax rate and regional VAT handling for saved payment methods"
                    .into(),
            author: "Dev Example".into(),
            at: 1_700_000_000,
        })
    }

    impl DiffRef {
        /// What the header draws for it.
        fn drawn(&self) -> String {
            match self {
                DiffRef::Patch(n) => format!("#{n}"),
                DiffRef::Commit(rev) | DiffRef::BareCommit(rev) => rev[..8].to_string(),
            }
        }

        /// Whether a label stands beside it. The bare control has none, and
        /// has one child fewer (the spacer a labelled header does without), so
        /// its thresholds are its own.
        fn labelled(&self) -> bool {
            !matches!(self, DiffRef::BareCommit(_))
        }

        fn source(&self) -> crate::terminal::git_diff::DiffSource {
            use crate::terminal::git_diff::DiffSource;
            match self {
                DiffRef::Patch(n) => DiffSource::Patch {
                    id: format!("nathabonfim59/tty59#{n}"),
                    label: real_label(),
                },
                DiffRef::Commit(rev) => DiffSource::Commit {
                    rev: (*rev).to_string(),
                    label: real_label(),
                },
                DiffRef::BareCommit(rev) => DiffSource::Commit {
                    rev: (*rev).to_string(),
                    label: None,
                },
            }
        }
    }

    /// Opens a diff on `subject`, over one file whose counts make the header's
    /// stats seven digits wide.
    fn open_diff(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        subject: &DiffRef,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        use crate::terminal::git_diff::{DiffSnapshot, FileDiff, FileStatus};
        let snapshot = std::sync::Arc::new(DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source: subject.source(),
            branch: "move-tab-keybind".into(),
            files: vec![FileDiff {
                path: "a.rs".into(),
                old_path: None,
                status: FileStatus::Modified,
                added: 1_234_567,
                removed: 7_654_321,
                binary: false,
                truncated: None,
                hunks: Vec::new(),
            }],
            ..Default::default()
        });
        let held = std::sync::Arc::clone(&snapshot);
        app.update_in(vcx, |app, window, cx| {
            app.open_loaded_diff_for_test(snapshot, false, window, cx);
        });
        vcx.run_until_parked();
        assert_holds(app, vcx, &held, "freshly opened");
        held
    }

    /// The overlay is still ready on exactly the snapshot the test installed:
    /// nothing, a probe included, has replaced it with another or with
    /// "not a repository" since.
    fn assert_holds(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        held: &std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
        what: &str,
    ) {
        assert!(
            app.read_with(vcx, |app, _| app.diff_overlay_holds_for_test(held)),
            "{what}: the overlay no longer holds the snapshot the test opened"
        );
    }

    /// The terminal is at least its floor wide: as painted (`pane_area` is
    /// written by the terminal body's own canvas), and, as a control, as the
    /// layout models it.
    fn assert_terminal_floor(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        column: f32,
        what: &str,
    ) {
        let painted = app
            .read_with(vcx, |app, _| app.pane_area.get())
            .unwrap_or_else(|| panic!("{what}: the terminal body is not painted"));
        assert!(
            f32::from(painted.size.width) >= crate::ui::app::TERMINAL_MIN_W - 0.5,
            "{what}: the terminal is painted {:?} wide, under its floor",
            painted.size.width
        );
        let body = vcx.update(|window, cx| app.read(cx).document_body_px(window, cx));
        assert!(
            body - column >= crate::ui::app::TERMINAL_MIN_W - 0.01,
            "{what}: the layout leaves the terminal {}px, under its floor",
            body - column
        );
    }

    /// The diff header's parts as painted, and the column that holds them.
    struct DiffHeader {
        pill: Bounds<Pixels>,
        /// The header's own row, whatever holds it.
        header: Bounds<Pixels>,
        /// The docked column that holds the header; none while the diff fills
        /// the workspace.
        column: Option<Bounds<Pixels>>,
        /// The ref's text; a branch subject has none of its own to find.
        rev: Option<Bounds<Pixels>>,
        view: Bounds<Pixels>,
        layout: Bounds<Pixels>,
        close: Bounds<Pixels>,
    }

    fn diff_header(vcx: &mut VisualTestContext, what: &str) -> DiffHeader {
        let rev = vcx.debug_bounds("diff-overlay-rev");
        let column = vcx.debug_bounds("document-column");
        let mut get = |selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        DiffHeader {
            pill: get("agent-mode-pill"),
            header: get("diff-overlay-header"),
            column,
            rev,
            view: get("diff-overlay-view"),
            layout: get("document-layout-switch"),
            close: get("diff-overlay-close"),
        }
    }

    /// Whether the diff header has gone into its column, below the title bar.
    fn diff_inline(vcx: &mut VisualTestContext) -> bool {
        let layout = vcx
            .debug_bounds("document-layout-switch")
            .expect("the diff header's layout switch is drawn");
        layout.top() >= px(crate::ui::app::TITLE_BAR_HEIGHT) - px(1.)
    }

    /// `text` shaped as the header draws its subject: the editor font, medium
    /// weight, the mono size and the features the header inherits (the
    /// configured list, or with none the terminal's ligatures-off set), by the
    /// real text system.
    fn shaped_rev(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &str) -> f32 {
        let (family, configured) = app.read_with(vcx, |app, _| {
            (app.font_family.clone(), app.font_features.clone())
        });
        let features = match configured.filter(|f| !f.0.is_empty()) {
            Some(configured) => configured,
            None => gpui::FontFeatures(std::sync::Arc::new(vec![
                ("calt".to_string(), 0),
                ("liga".to_string(), 0),
                ("clig".to_string(), 0),
            ])),
        };
        vcx.update(|window, _| {
            let font = gpui::Font {
                family: family.into(),
                features,
                fallbacks: None,
                weight: gpui::FontWeight::MEDIUM,
                style: gpui::FontStyle::Normal,
            };
            let size = window.rem_size().as_f32() * crate::ui::right_panel::TEXT_MONO;
            crate::ui::tab_strip::measure_text(window.text_system(), &font, size, text)
        })
    }

    /// `text` shaped in the interface font at `size_rems` of the interface font
    /// size, as the filled header draws its summary (`tabular`: the figures'
    /// numerals) and the terminal button's name (the header's feature set), by
    /// the real text system.
    fn shaped_ui(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        text: &str,
        size_rems: f32,
        tabular: bool,
    ) -> f32 {
        use gpui_component::ActiveTheme as _;
        let configured = app.read_with(vcx, |app, _| app.font_features.clone());
        let features = match (tabular, configured.filter(|f| !f.0.is_empty())) {
            (true, _) => crate::ui::diff_overlay::tabular(),
            (false, Some(configured)) => configured,
            (false, None) => gpui::FontFeatures(std::sync::Arc::new(vec![
                ("calt".to_string(), 0),
                ("liga".to_string(), 0),
                ("clig".to_string(), 0),
            ])),
        };
        vcx.update(|window, cx| {
            let font = gpui::Font {
                family: cx.theme().font_family.clone(),
                features,
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            };
            let size = window.rem_size().as_f32() * size_rems;
            crate::ui::tab_strip::measure_text(window.text_system(), &font, size, text)
        })
    }

    /// What a header paints whole: its three controls, and the widths of the
    /// figures and words it has (the Staged chip, the two counts, the
    /// Refreshing word), each as one painted run. Read in a window with room,
    /// it is what every narrower state must paint too.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Whole {
        controls: (Pixels, Pixels, Pixels),
        chip: Option<Pixels>,
        added: Option<Pixels>,
        removed: Option<Pixels>,
        refreshing: Option<Pixels>,
    }

    fn diff_whole(vcx: &mut VisualTestContext, what: &str) -> Whole {
        let h = diff_header(vcx, what);
        let mut width = |selector: &'static str| vcx.debug_bounds(selector).map(|b| b.size.width);
        Whole {
            controls: (h.view.size.width, h.layout.size.width, h.close.size.width),
            chip: width("ui-chip"),
            added: width("diff-overlay-added"),
            removed: width("diff-overlay-removed"),
            refreshing: width("diff-overlay-refreshing"),
        }
    }

    /// The painted widths of what the header holds, for the message of an
    /// assertion about one of them: the header row, the terminal button and the
    /// summary (both only a filled diff draws), the subject, the figures and
    /// the controls.
    fn diff_widths(vcx: &mut VisualTestContext) -> String {
        let mut out = Vec::new();
        for (name, selector) in [
            ("header", "diff-overlay-header"),
            ("terminal button", "document-back-to-terminal"),
            ("subject", "diff-overlay-rev"),
            ("summary", "diff-overlay-summary"),
            ("chip", "ui-chip"),
            ("added", "diff-overlay-added"),
            ("removed", "diff-overlay-removed"),
            ("refreshing", "diff-overlay-refreshing"),
            ("view", "diff-overlay-view"),
            ("layout", "document-layout-switch"),
            ("close", "diff-overlay-close"),
        ] {
            if let Some(b) = vcx.debug_bounds(selector) {
                out.push(format!("{name} {:?}", b.size.width));
            }
        }
        out.join(", ")
    }

    /// Whatever the subject, the controls are all there, whole, in order and
    /// inside the column that holds them (the window, while the diff fills it);
    /// the subject ends before them; lifted into the bar the header keeps 8px
    /// from the Agents pill, and in its column it is below the bar; filling the
    /// workspace it is the overlay's own. Returns whether it is lifted.
    fn assert_diff_header(
        vcx: &mut VisualTestContext,
        whole: &Whole,
        window_w: f32,
        what: &str,
    ) -> bool {
        let h = diff_header(vcx, what);
        assert_eq!(
            diff_whole(vcx, what),
            *whole,
            "{what}: a control, figure or word of the header is squeezed or gone"
        );
        if let Some(rev) = h.rev {
            assert!(
                rev.right() <= h.view.left(),
                "{what}: the subject {rev:?} runs into the view switch {:?}",
                h.view
            );
        }
        assert!(
            h.view.right() <= h.layout.left() && h.layout.right() <= h.close.left(),
            "{what}: the controls overlap: {:?} {:?} {:?}",
            h.view,
            h.layout,
            h.close
        );
        // The header's row sits inside what holds it, and every part of it
        // inside the row: a part that does not fit shows as one drawn past it.
        let (left, right) = h
            .column
            .map_or((px(0.), px(window_w)), |c| (c.left(), c.right()));
        assert!(
            h.header.left() >= left - px(0.5) && h.header.right() <= right + px(0.5),
            "{what}: the header {:?} is outside the {left:?}..{right:?} it is drawn in",
            h.header
        );
        for (name, part) in h.rev.map(|rev| ("subject", rev)).into_iter().chain([
            ("view switch", h.view),
            ("layout switch", h.layout),
            ("close", h.close),
        ]) {
            assert!(
                part.left() >= h.header.left() - px(0.5)
                    && part.right() <= h.header.right() + px(0.5),
                "{what}: the {name} {part:?} is outside its header row {:?}",
                h.header
            );
        }
        // The chip and the figures follow the subject, in this order, and end
        // before the switches, each inside the header row.
        let mut after = h.rev.map(|rev| rev.right());
        for (name, selector) in [
            ("Staged chip", "ui-chip"),
            ("back button", "diff-overlay-unfocus"),
            ("summary", "diff-overlay-summary"),
            ("added figure", "diff-overlay-added"),
            ("removed figure", "diff-overlay-removed"),
            ("Refreshing word", "diff-overlay-refreshing"),
        ] {
            let Some(field) = vcx.debug_bounds(selector) else {
                continue;
            };
            assert!(
                field.left() >= h.header.left() - px(0.5)
                    && field.right() <= h.header.right() + px(0.5),
                "{what}: the {name} {field:?} is outside its header row {:?}",
                h.header
            );
            if let Some(prev) = after {
                assert!(
                    field.left() >= prev - px(0.5),
                    "{what}: the {name} {field:?} starts before what precedes it ends, {prev:?}"
                );
            }
            after = Some(field.right());
        }
        if let Some(prev) = after {
            assert!(
                prev <= h.view.left() + px(0.5),
                "{what}: the figures end at {prev:?}, past the view switch {:?}",
                h.view
            );
        }
        // Filling the workspace the diff's own header stands over the bar: no
        // column, and nothing of the Agents pill's to keep clear of.
        if h.column.is_none() {
            return false;
        }
        match diff_inline(vcx) {
            true => {
                assert!(
                    h.layout.top() + px(1.) >= h.pill.bottom(),
                    "{what}: the in-column header rises into the Agents pill {:?}",
                    h.pill
                );
                false
            }
            false => {
                assert!(
                    h.close.right() + px(8.) <= h.pill.left() + px(0.5),
                    "{what}: the close button {:?} is within 8px of the Agents pill {:?}",
                    h.close,
                    h.pill
                );
                true
            }
        }
    }

    /// The Diff header gives its place in the title bar to its column when
    /// its controls cannot fit beside the Agents pill, and never lets a long,
    /// Unicode or arbitrary ref push a control away: the ref is the part that
    /// shortens. A short ref is left exactly as it was.
    #[gpui::test]
    fn a_diff_header_gives_way_to_its_column_never_to_its_controls(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, agents_wait, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 12);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 900.);

        // Each subject shaped in full, by the text system, as the bound a
        // drawn subject can never pass and what a shortened one is measured
        // against. Only the short refs are required to be whole at 2400px: a
        // long one may already have given way there beside seven-digit stats.
        let mut whole = None;
        let mut short_wide = Vec::new();
        let mut shaped = Vec::new();
        for (index, subject) in DIFF_REFS.iter().enumerate() {
            let held = open_diff(&app, &mut vcx, subject);
            let h = diff_header(&mut vcx, "2400px");
            assert_holds(&app, &mut vcx, &held, "2400px");
            whole.get_or_insert(diff_whole(&mut vcx, "2400px"));
            let drawn = subject.drawn();
            let text = shaped_rev(&app, &mut vcx, &drawn);
            if index < WHOLE_REFS {
                let rev = h.rev.expect("a revision's subject is drawn");
                let widths = diff_widths(&mut vcx);
                assert!(
                    (f32::from(rev.size.width) - text).abs() <= 2.,
                    "{drawn}: {:?} painted at 2400px, {text}px shaped; painted widths: {widths}",
                    rev.size.width
                );
                short_wide.push(rev.size.width);
            }
            shaped.push(text);
        }
        let whole = whole.expect("a header was drawn");
        // How tall one line of the filled header is, with room: the reference
        // for the one state in the matrix that is on two.
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_layout(DocumentLayout::Fill, cx)
        });
        resize(&mut vcx, 2400., 900.);
        let one_line = diff_header(&mut vcx, "filled at 2400px").header.size.height;
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_layout(DocumentLayout::Dock, cx)
        });
        resize(&mut vcx, 2400., 900.);

        // (width, height, panel open, docked beside the terminal). At 2400px
        // the column is wide enough for the header to be lifted into the title
        // bar; at 1440px it docks but the header is in the column; at 1000px
        // the seven-digit header leaves the terminal less than its floor, so
        // the diff fills.
        let scenes = [
            (2400., 900., false, true),
            (1440., 900., false, true),
            (1440., 900., true, true),
            (1000., 700., false, false),
            (1000., 700., true, false),
        ];
        let (mut lifted_seen, mut inline_seen, mut shortened_seen) = (false, false, false);
        let (mut checked, mut filled) = (0, Vec::new());
        for count in [0usize, 12] {
            if count > 0 {
                agents_wait(&app, &mut vcx, &mut streams, count);
            }
            for (locale, agents) in LOCALES {
                use_locale(&app, &mut vcx, locale, agents);
                for (index, subject) in DIFF_REFS.iter().enumerate() {
                    let held = open_diff(&app, &mut vcx, subject);
                    let drawn = subject.drawn();
                    for (width, height, panel, docks) in scenes {
                        set_right_panel(&app, &mut vcx, panel);
                        resize(&mut vcx, width, height);
                        let what = format!(
                            "{drawn} {locale} {width}x{height} panel {panel}, {count} agents"
                        );
                        let column = docked_px(&app, &mut vcx);
                        assert_eq!(
                            column.is_some(),
                            docks,
                            "{what}: docked {column:?}, expected {}",
                            if docks {
                                "docked"
                            } else {
                                "filling the workspace"
                            }
                        );
                        match column {
                            Some(column) => {
                                assert_terminal_floor(&app, &mut vcx, column, &what);
                            }
                            None => filled.push(what.clone()),
                        }
                        checked += 1;
                        assert_holds(&app, &mut vcx, &held, &what);
                        // The one known state that is on two lines is held to the
                        // two-line contract; every other is the single row it was.
                        let lifted =
                            if wraps_into_two_lines(locale, subject.labelled(), width, panel) {
                                assert_filled_lines(&mut vcx, &whole, width, one_line, 2, &what);
                                false
                            } else {
                                assert_diff_header(&mut vcx, &whole, width, &what)
                            };
                        let rev = diff_header(&mut vcx, &what)
                            .rev
                            .expect("a revision's subject is drawn")
                            .size
                            .width;
                        // The wide scene is the one that has to be lifted: the
                        // others are in the column or filling, by their widths.
                        assert!(
                            lifted || width != 2400.,
                            "{what}: the header is not lifted into the title bar at 2400px"
                        );
                        match lifted {
                            true => lifted_seen = true,
                            false => inline_seen = true,
                        }
                        if index < WHOLE_REFS {
                            let widths = diff_widths(&mut vcx);
                            assert_eq!(
                                rev, short_wide[index],
                                "{what}: a short ref is drawn whole, exactly as it was; painted \
                                 widths: {widths}"
                            );
                        } else if f32::from(rev) + 8. < shaped[index] {
                            shortened_seen = true;
                        }
                        assert!(
                            f32::from(rev) <= shaped[index] + 2.,
                            "{what}: the subject {rev:?} is wider than its text, {}px",
                            shaped[index]
                        );
                        assert!(
                            app.read_with(&vcx, |app, cx| app.document_layout(cx))
                                == DocumentLayout::Dock,
                            "{what}: the saved layout is still the column"
                        );
                    }
                }
            }
        }
        // Every scene is checked, whichever layout it gave; those that filled
        // are listed.
        assert_eq!(
            checked,
            2 * LOCALES.len() * DIFF_REFS.len() * scenes.len(),
            "scenes checked; filling the workspace: {filled:?}"
        );
        assert!(
            lifted_seen && inline_seen && shortened_seen,
            "the matrix has to cover a lifted header ({lifted_seen}), one in its column \
             or filling ({inline_seen}) and a ref that gave way ({shortened_seen}); filling the \
             workspace in {filled:?}"
        );
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// Where the Diff header falls into its column when the window narrows and
    /// lifts again when it widens, as column widths, through both histories:
    /// the header holds the title bar until its column is too narrow (`enter`),
    /// and once in the column returns only after `HEADER_HYSTERESIS` more
    /// (`leave`). Returns `(last lifted, enter, leave, narrowest lifted window)`.
    fn diff_boundary(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        held: &std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
    ) -> (f32, f32, f32, f32) {
        assert_holds(app, vcx, held, "before the boundary walk");
        resize(vcx, 2400., 800.);
        assert!(!diff_inline(vcx), "a wide window lifts the diff header");
        let mut width = 2400.;
        while !diff_inline(vcx) {
            width -= 5.;
            assert!(width > 900., "the diff header never moved into the column");
            resize(vcx, width, 800.);
        }
        // From far above again, so no earlier answer is held, one point at a
        // time from just above where it dropped.
        width += 5.;
        resize(vcx, 2400., 800.);
        resize(vcx, width, 800.);
        assert!(!diff_inline(vcx));
        let mut last_lifted = docked_px(app, vcx).expect("docked");
        let enter = loop {
            width -= 1.;
            resize(vcx, width, 800.);
            let column = docked_px(app, vcx).expect("still docked");
            if diff_inline(vcx) {
                break column;
            }
            last_lifted = column;
        };

        // History one: in the column, widening holds it there through the
        // margin and lifts it only past it.
        let (mut up, mut held_inline) = (width, 0);
        let leave = loop {
            up += 1.;
            assert!(up < width + 200., "the diff header never went back up");
            resize(vcx, up, 800.);
            if !diff_inline(vcx) {
                break docked_px(app, vcx).expect("docked");
            }
            held_inline += 1;
        };
        assert!(held_inline > 0, "lifted again the moment it dropped");

        // History two: lifted again, one point narrower is still lifted, and
        // it drops only below where it dropped the first time.
        resize(vcx, up - 1., 800.);
        assert!(
            !diff_inline(vcx),
            "one point under the return, inside the margin, it dropped again"
        );
        let mut down = up - 1.;
        let dropped_at = loop {
            down -= 1.;
            assert!(down > width - 200., "the lifted header never dropped");
            resize(vcx, down, 800.);
            if diff_inline(vcx) {
                break docked_px(app, vcx).expect("docked");
            }
        };
        assert_eq!(
            dropped_at, enter,
            "lifted, it must drop where it dropped the first time, not inside the margin"
        );
        assert_holds(app, vcx, held, "after the boundary walk");
        (last_lifted, enter, leave, width + 1.)
    }

    /// The Diff header's fallback has the editor's margin through both
    /// histories, depends on the controls and the fixed labels and not on the
    /// ref's text, and leaves the saved layout alone, in each language.
    #[gpui::test]
    fn the_diff_header_fallback_is_exact_with_the_editors_margin(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 800.);
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            let mut enters = Vec::new();
            for (index, subject) in DIFF_REFS.iter().enumerate() {
                let n = subject.drawn();
                let held = open_diff(&app, &mut vcx, subject);
                resize(&mut vcx, 2400., 800.);
                let natural = diff_header(&mut vcx, "2400px").rev.map(|r| r.size.width);
                let (last_lifted, enter, leave, narrowest) = diff_boundary(&app, &mut vcx, &held);
                // At the narrowest window that still lifts the header it has
                // exactly what it needs, and a short ref is drawn whole.
                if index < WHOLE_REFS {
                    resize(&mut vcx, 2400., 800.);
                    resize(&mut vcx, narrowest, 800.);
                    let widths = diff_widths(&mut vcx);
                    let rev = diff_header(&mut vcx, &n).rev.map(|r| r.size.width);
                    assert_eq!(
                        rev, natural,
                        "{locale} {n}: at the narrowest lifted window a short ref is drawn \
                         whole; painted widths: {widths}"
                    );
                }
                assert!(
                    leave - enter > HEADER_HYSTERESIS,
                    "{locale} {n}: lifted again {leave} after dropping at {enter}: no margin"
                );
                assert!(
                    leave - last_lifted < HEADER_HYSTERESIS + 1.,
                    "{locale} {n}: lifted again {leave}, more than the margin past {last_lifted}"
                );
                assert!(
                    app.read_with(&vcx, |app, cx| app.document_layout(cx)) == DocumentLayout::Dock,
                    "{locale} {n}: the saved layout is still the column"
                );
                if subject.labelled() {
                    enters.push(enter);
                }
            }
            assert!(
                enters.windows(2).all(|w| w[0] == w[1]),
                "{locale}: the fallback moved with the ref's text: {enters:?}"
            );
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// Where a docked diff gives its column up for the whole workspace when the
    /// window narrows and takes it back when it widens, as window widths: the
    /// last that held the column, the first that filled, and the one it came
    /// back at, through both histories. The diff fills as soon as the room
    /// beside the terminal's floor is under its header's need (`first fill`),
    /// and once filled docks again only [`HEADER_HYSTERESIS`] past that.
    fn fill_boundary(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        held: &std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
    ) -> (f32, f32, f32) {
        assert_holds(app, vcx, held, "before the fill walk");
        resize(vcx, 2400., 800.);
        assert!(
            docked_px(app, vcx).is_some(),
            "a wide window docks the diff"
        );
        let mut width = 2400.;
        while docked_px(app, vcx).is_some() {
            width -= 5.;
            assert!(width > 700., "the diff never filled the workspace");
            resize(vcx, width, 800.);
        }
        // From far above again, so no earlier answer is held.
        width += 5.;
        resize(vcx, 2400., 800.);
        resize(vcx, width, 800.);
        assert!(docked_px(app, vcx).is_some());
        let mut last_docked = width;
        let first_fill = loop {
            width -= 1.;
            resize(vcx, width, 800.);
            if docked_px(app, vcx).is_none() {
                break width;
            }
            last_docked = width;
        };

        // History one: filled, widening holds it filled through the margin and
        // docks it only past it.
        let (mut up, mut held_filled) = (first_fill, 0);
        let back = loop {
            up += 1.;
            assert!(up < first_fill + 200., "the diff never docked again");
            resize(vcx, up, 800.);
            if docked_px(app, vcx).is_some() {
                break up;
            }
            held_filled += 1;
        };
        assert!(held_filled > 0, "docked again the moment it filled");

        // History two: docked again, one point narrower is still docked, and
        // it fills only below where it filled the first time.
        resize(vcx, back - 1., 800.);
        assert!(
            docked_px(app, vcx).is_some(),
            "one point under the return, inside the margin, it filled again"
        );
        let mut down = back - 1.;
        let filled_at = loop {
            down -= 1.;
            assert!(down > first_fill - 200., "the docked diff never filled");
            resize(vcx, down, 800.);
            if docked_px(app, vcx).is_none() {
                break down;
            }
        };
        assert_eq!(
            filled_at, first_fill,
            "docked, it must fill where it filled the first time, not inside the margin"
        );
        assert_holds(app, vcx, held, "after the fill walk");
        (last_docked, first_fill, back)
    }

    /// A diff's column is as narrow as its header's controls need and the
    /// terminal's floor allow; narrower, the diff fills the workspace and
    /// docks again only past the editor's margin. At the last docked width the
    /// terminal has exactly its floor and every field of the header is whole;
    /// filled, the header and a short ref are still whole; the point does not
    /// depend on the ref's text, in either language; and the saved layout is
    /// the column throughout.
    #[gpui::test]
    fn the_diff_column_fills_the_workspace_below_its_headers_need_with_the_margin(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 800.);
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            let mut edges = Vec::new();
            for (index, subject) in DIFF_REFS.iter().enumerate() {
                let held = open_diff(&app, &mut vcx, subject);
                let drawn = subject.drawn();
                resize(&mut vcx, 2400., 800.);
                let wide = diff_header(&mut vcx, "2400px");
                let whole = diff_whole(&mut vcx, "2400px");
                let natural = wide.rev.expect("a revision's subject is drawn").size.width;

                let (last_docked, first_fill, back) = fill_boundary(&app, &mut vcx, &held);
                let what = format!("{locale} {drawn}");
                assert!(
                    back - last_docked >= HEADER_HYSTERESIS - 0.01
                        && back - last_docked <= HEADER_HYSTERESIS + 1.,
                    "{what}: docked again at {back} after the last docked {last_docked}: \
                     not the margin of {HEADER_HYSTERESIS}"
                );
                assert!(
                    back - first_fill > HEADER_HYSTERESIS,
                    "{what}: docked again at {back} after filling at {first_fill}: no margin"
                );

                // The last docked width, reached from a wide window (from the
                // filled one it would sit inside the margin and stay filled): the
                // terminal at its floor, the header whole.
                resize(&mut vcx, 2400., 800.);
                resize(&mut vcx, last_docked, 800.);
                let column = docked_px(&app, &mut vcx).expect("docked at its last width");
                assert_terminal_floor(&app, &mut vcx, column, &format!("{what} last docked"));
                assert_diff_header(
                    &mut vcx,
                    &whole,
                    last_docked,
                    &format!("{what} last docked"),
                );
                // Exactly its need is what the column has here, and a short ref
                // is drawn whole.
                if index < WHOLE_REFS {
                    let widths = diff_widths(&mut vcx);
                    let rev = diff_header(&mut vcx, &what)
                        .rev
                        .expect("a revision's subject is drawn")
                        .size
                        .width;
                    assert_eq!(
                        rev, natural,
                        "{what} last docked: a short ref is drawn whole; painted widths: {widths}"
                    );
                }
                // Filled: the header and a short ref are whole.
                for filled in [first_fill, first_fill - 40.] {
                    resize(&mut vcx, filled, 800.);
                    assert!(
                        docked_px(&app, &mut vcx).is_none(),
                        "{what}: {filled}px fills the workspace"
                    );
                    let at = format!("{what} filled at {filled}");
                    assert_diff_header(&mut vcx, &whole, filled, &at);
                    if index < WHOLE_REFS {
                        let rev = diff_header(&mut vcx, &at)
                            .rev
                            .expect("a revision's subject is drawn")
                            .size
                            .width;
                        assert_eq!(rev, natural, "{at}: a short ref is drawn whole");
                    }
                    assert!(
                        app.read_with(&vcx, |app, cx| app.document_layout(cx))
                            == DocumentLayout::Dock,
                        "{at}: the saved layout is still the column"
                    );
                }
                if subject.labelled() {
                    edges.push(first_fill);
                }
            }
            assert!(
                edges.windows(2).all(|w| w[0] == w[1]),
                "{locale}: the point the diff fills at moved with the ref's text: {edges:?}"
            );
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// The tab the filled diff's terminal button names, titled so that the name
    /// is whole beside the room at 1440px and 2400px and cut at 1000px. Seeded
    /// state: the field a rename leaves behind is set directly; no rename
    /// interaction is under test. Seventeen characters: the test text backend
    /// advances 0.6 of the font size per character, so 127.5px at the name's
    /// 12.5px, under the 168px cap; real fonts are part of the native captures.
    const TAB_TITLE: &str = "release-candidate";

    /// The states at which the filled header's measured minima do not fit one
    /// line: Russian, 1000px wide, the panel open. The panel's tab-count slot
    /// (B3) leaves that state a 519px body in the test backend (it was 547px),
    /// under the labelled refs' 555px minima as before and now under the
    /// plain refs' too, which the header measured at 1000px as two lines when
    /// asked. The locale is the code the tests switch to, exactly.
    fn wraps_into_two_lines(locale: &str, _labelled: bool, width: f32, panel: bool) -> bool {
        locale == "ru-RU" && width == 1000. && panel
    }

    /// Where the filled header puts its secondary text, from painted bounds:
    /// on one line the summary between the ref and the first count (or the
    /// switches) and the terminal button's name inside that button; on two,
    /// whichever of them left the first line is on the second, clear of the
    /// first line, the name before the summary, and whichever stayed is where
    /// it was on one line.
    fn assert_secondary_bounds(vcx: &mut VisualTestContext, lines: usize, what: &str) {
        let get = |vcx: &mut VisualTestContext, selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        let (header, button) = (
            get(vcx, "diff-overlay-header"),
            get(vcx, "document-back-to-terminal"),
        );
        let (name, summary, rev) = (
            get(vcx, "document-terminal-name"),
            get(vcx, "diff-overlay-summary"),
            get(vcx, "diff-overlay-rev"),
        );
        let after = vcx
            .debug_bounds("diff-overlay-added")
            .or_else(|| vcx.debug_bounds("diff-overlay-view"))
            .unwrap_or_else(|| panic!("{what}: neither a count nor the switches are drawn"));
        let close = get(vcx, "diff-overlay-close");
        let eps = px(0.5);
        assert_eq!(
            header_lines(vcx, what),
            lines,
            "{what}: the header is not on {lines} line(s)"
        );
        assert!(
            button.left() >= header.left() - eps && button.right() <= header.right() + eps,
            "{what}: the terminal button {button:?} is outside the header row {header:?}"
        );
        assert!(
            summary.left() >= header.left() - eps && summary.right() <= header.right() + eps,
            "{what}: the summary {summary:?} is outside the header row {header:?}"
        );
        let (name_below, summary_below) = (
            name.center().y > close.bottom(),
            summary.center().y > close.bottom(),
        );
        assert!(
            lines == 2 || (!name_below && !summary_below),
            "{what}: one line, and the name or the summary is below the close button"
        );
        if name_below {
            assert!(
                !overlaps(name, button) && !overlaps(name, rev) && !overlaps(name, close),
                "{what}: the name {name:?} is painted over the first line"
            );
        } else {
            assert!(
                name.left() >= button.left() - eps && name.right() <= button.right() + eps,
                "{what}: the name {name:?} is outside its button {button:?}"
            );
        }
        if summary_below {
            for (above, other) in [("terminal button", button), ("ref", rev), ("close", close)] {
                assert!(
                    !overlaps(summary, other),
                    "{what}: the summary {summary:?} is painted over the {above} {other:?}"
                );
            }
            assert!(
                !name_below || name.right() <= summary.left() + eps,
                "{what}: the summary {summary:?} is before the name {name:?} on the second line"
            );
        } else {
            assert!(
                summary.left() >= rev.right() - eps && summary.right() <= after.left() + eps,
                "{what}: the summary {summary:?} is not between the ref {rev:?} and {after:?}"
            );
        }
        // The summary never leaves before the name does.
        assert!(
            !(summary_below && !name_below),
            "{what}: the summary left the first line before the terminal's name"
        );
    }

    /// A prefix of `text` of three characters and the ellipsis, shaped and
    /// rounded up the way a drawn run is: what a
    /// cut summary or name has to keep visible at least. A readable design
    /// minimum for these strings, not a promise of three of any glyphs.
    fn kept_prefix(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        text: &str,
        size_rems: f32,
        tabular: bool,
    ) -> f32 {
        let prefix: String = text.chars().take(3).chain(['\u{2026}']).collect();
        shaped_ui(app, vcx, &prefix, size_rems, tabular).ceil()
    }

    /// In a filled diff the file-count summary and the terminal button's name
    /// are the secondary text: with the panel open at 1000px they both give way
    /// so that a short ref is drawn at least at its natural width and the
    /// counts and the controls stay whole, each keeping the first characters of
    /// its own text and an ellipsis. With room, at 2400px, neither is cut.
    /// Where those minima and the fixed parts do not fit one line (the labelled
    /// refs in Russian, panel open: 555px of 547px) the metadata takes a second
    /// line and nothing is cut; the other states stay on one.
    #[gpui::test]
    fn a_filled_diff_lets_its_secondary_names_yield_before_a_short_ref(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            // The summary as the header says it for the fixture's one file.
            let summary_text =
                crate::ui::i18n::t_plural(crate::ui::i18n::L10nKey::DiffChangedFiles, 1, &[]);
            let summary_kept = kept_prefix(
                &app,
                &mut vcx,
                &summary_text,
                crate::ui::right_panel::META,
                true,
            );
            let name_kept = kept_prefix(
                &app,
                &mut vcx,
                TAB_TITLE,
                crate::ui::document_column::HEADER_CELL_TEXT,
                false,
            );
            for subject in DIFF_REFS.iter().take(WHOLE_REFS) {
                let held = open_diff(&app, &mut vcx, subject);
                let drawn = subject.drawn();

                // With room: filled at 2400px nothing yields; the ref, the
                // summary and the name are at their natural width, which the
                // text system confirms for the two strings.
                app.update_in(&mut vcx, |app, _, cx| {
                    app.set_document_layout(DocumentLayout::Fill, cx)
                });
                set_right_panel(&app, &mut vcx, false);
                resize(&mut vcx, 2400., 900.);
                assert!(
                    docked_px(&app, &mut vcx).is_none(),
                    "{drawn}: filled at 2400px"
                );
                let widest = |vcx: &mut VisualTestContext, selector: &'static str| {
                    vcx.debug_bounds(selector)
                        .unwrap_or_else(|| panic!("{drawn} {locale}: {selector} is not drawn"))
                        .size
                        .width
                };
                let rev_natural = widest(&mut vcx, "diff-overlay-rev");
                let summary_natural = widest(&mut vcx, "diff-overlay-summary");
                let name_natural = widest(&mut vcx, "document-terminal-name");
                let summary_shaped = shaped_ui(
                    &app,
                    &mut vcx,
                    &summary_text,
                    crate::ui::right_panel::META,
                    true,
                );
                let name_shaped = shaped_ui(
                    &app,
                    &mut vcx,
                    TAB_TITLE,
                    crate::ui::document_column::HEADER_CELL_TEXT,
                    false,
                );
                assert!(
                    (f32::from(summary_natural) - summary_shaped).abs() <= 2.
                        && (f32::from(name_natural) - name_shaped).abs() <= 2.,
                    "{drawn} {locale}: at 2400px the summary is {summary_natural:?} ({summary_shaped}px \
                     shaped) and the name {name_natural:?} ({name_shaped}px shaped)"
                );
                assert!(
                    f32::from(name_natural) < crate::ui::document_column::HEADER_CELL_MAX_W,
                    "{drawn} {locale}: the title is whole under its cap at 2400px, or it is not a \
                     title that cuts only for want of room"
                );
                let whole = diff_whole(&mut vcx, "filled at 2400px");
                let one_line = diff_header(&mut vcx, "filled at 2400px").header.size.height;
                assert_diff_header(
                    &mut vcx,
                    &whole,
                    2400.,
                    &format!("{drawn} {locale} filled at 2400px"),
                );
                assert_secondary_bounds(&mut vcx, 1, &format!("{drawn} {locale} filled at 2400px"));

                // Filled at 1440px, the panel open and closed, with the same long
                // title: there is room, so the ref, the summary and the name are
                // each whole, at their 2400px widths.
                for panel in [true, false] {
                    set_right_panel(&app, &mut vcx, panel);
                    resize(&mut vcx, 1440., 900.);
                    let what = format!("{drawn} {locale} filled at 1440px panel {panel}");
                    assert!(docked_px(&app, &mut vcx).is_none(), "{what}: filled");
                    assert_holds(&app, &mut vcx, &held, &what);
                    assert_diff_header(&mut vcx, &whole, 1440., &what);
                    assert_secondary_bounds(&mut vcx, 1, &what);
                    let widths = diff_widths(&mut vcx);
                    for (name, selector, natural) in [
                        ("ref", "diff-overlay-rev", rev_natural),
                        ("summary", "diff-overlay-summary", summary_natural),
                        ("terminal name", "document-terminal-name", name_natural),
                    ] {
                        let painted = widest(&mut vcx, selector);
                        assert!(
                            (f32::from(painted) - f32::from(natural)).abs() <= 0.5,
                            "{what}: the {name} {painted:?} is not whole ({natural:?}); \
                             painted widths: {widths}"
                        );
                    }
                }
                app.update_in(&mut vcx, |app, _, cx| {
                    app.set_document_layout(DocumentLayout::Dock, cx)
                });

                // The panel open, and closed, at 1000px: the diff fills.
                for panel in [true, false] {
                    set_right_panel(&app, &mut vcx, panel);
                    resize(&mut vcx, 1000., 700.);
                    let what = format!("{drawn} {locale} 1000px panel {panel}");
                    assert!(
                        docked_px(&app, &mut vcx).is_none(),
                        "{what}: the diff fills the workspace"
                    );
                    assert_holds(&app, &mut vcx, &held, &what);
                    // Measured minima that do not fit one line take a second:
                    // the labelled refs in Russian, with the panel open, need
                    // 555px of a 547px body. Every other state here is one line.
                    let lines = if wraps_into_two_lines(locale, subject.labelled(), 1000., panel) {
                        2
                    } else {
                        1
                    };
                    assert_filled_lines(&mut vcx, &whole, 1000., one_line, lines, &what);
                    assert_secondary_bounds(&mut vcx, lines, &what);
                    let widths = diff_widths(&mut vcx);
                    let rev = widest(&mut vcx, "diff-overlay-rev");
                    assert!(
                        rev >= rev_natural,
                        "{what}: the ref {rev:?} is narrower than its natural {rev_natural:?}; \
                         painted widths: {widths}"
                    );
                    for (name, selector, natural, kept) in [
                        (
                            "summary",
                            "diff-overlay-summary",
                            summary_natural,
                            summary_kept,
                        ),
                        (
                            "terminal name",
                            "document-terminal-name",
                            name_natural,
                            name_kept,
                        ),
                    ] {
                        let painted = widest(&mut vcx, selector);
                        assert!(
                            painted <= natural + px(0.5),
                            "{what}: the {name} {painted:?} is wider than its natural {natural:?}"
                        );
                        assert!(
                            f32::from(painted) + 0.01 >= kept,
                            "{what}: the {name} {painted:?} keeps less than its first characters \
                             and an ellipsis ({kept}px); painted widths: {widths}"
                        );
                        if lines == 2 && on_second_line(&mut vcx, selector, &what) {
                            // What a second line takes it takes whole.
                            assert!(
                                painted + px(0.5) >= natural,
                                "{what}: on the second line the {name} {painted:?} is whole, not \
                                 cut from {natural:?}; painted widths: {widths}"
                            );
                        } else if panel && lines == 1 {
                            assert!(
                                painted + px(0.5) < natural,
                                "{what}: with the panel open the {name} {painted:?} is cut from \
                                 {natural:?}; painted widths: {widths}"
                            );
                        }
                    }
                    assert!(
                        app.read_with(&vcx, |app, cx| app.document_layout(cx))
                            == DocumentLayout::Dock,
                        "{what}: the saved layout is still the column"
                    );
                }
            }
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// What the filled header's first line always holds, left to right: the
    /// actions (the terminal button, the focused file's way back, the two
    /// switches, the close tile), what the diff is of, and its two totals.
    const FIRST_ROW: [(&str, &str); 8] = [
        ("terminal button", "document-back-to-terminal"),
        ("ref", "diff-overlay-rev"),
        ("back button", "diff-overlay-unfocus"),
        ("added figure", "diff-overlay-added"),
        ("removed figure", "diff-overlay-removed"),
        ("view switch", "diff-overlay-view"),
        ("layout switch", "document-layout-switch"),
        ("close", "diff-overlay-close"),
    ];

    /// What may leave the first line, in the order it leaves: the Staged chip
    /// with the Refreshing word, then the terminal's name, then the summary.
    /// Where they are drawn on the second line they keep this order.
    const TIERS: [&[(&str, &str)]; 3] = [
        &[
            ("Staged chip", "ui-chip"),
            ("Refreshing word", "diff-overlay-refreshing"),
        ],
        &[("terminal name", "document-terminal-name")],
        &[("summary", "diff-overlay-summary")],
    ];

    /// Which tiers are drawn on the second line, from painted bounds: `None`
    /// for a tier the header does not have, and the tier's own fields never
    /// split across the two lines.
    fn tiers_lower(vcx: &mut VisualTestContext, what: &str) -> [Option<bool>; 3] {
        let close = vcx
            .debug_bounds("diff-overlay-close")
            .unwrap_or_else(|| panic!("{what}: the close button is not drawn"));
        let mut out = [None; 3];
        for (tier, fields) in TIERS.iter().enumerate() {
            let rows: Vec<(&str, bool)> = fields
                .iter()
                .filter_map(|(name, selector)| {
                    vcx.debug_bounds(selector)
                        .map(|field| (*name, field.center().y > close.bottom()))
                })
                .collect();
            if let Some((_, first)) = rows.first() {
                assert!(
                    rows.iter().all(|(_, lower)| lower == first),
                    "{what}: the fields of one tier are on different lines: {rows:?}"
                );
                out[tier] = Some(*first);
            }
        }
        out
    }

    /// The chip with the Refreshing word leaves the first line before the
    /// terminal's name, and the name before the summary: a tier on the second
    /// line has every tier before it there too.
    fn assert_tier_order(tiers: [Option<bool>; 3], what: &str) {
        let mut still_first: Option<usize> = None;
        for (tier, lower) in tiers.iter().enumerate() {
            match lower {
                Some(false) => still_first = still_first.or(Some(tier)),
                Some(true) => assert!(
                    still_first.is_none(),
                    "{what}: tier {tier} is on the second line while tier {still_first:?} is \
                     still on the first: {tiers:?}"
                ),
                None => {}
            }
        }
    }

    /// Whether the painted field is below the first line, which the close
    /// tile is on whatever the header does.
    fn on_second_line(vcx: &mut VisualTestContext, selector: &'static str, what: &str) -> bool {
        let close = vcx
            .debug_bounds("diff-overlay-close")
            .unwrap_or_else(|| panic!("{what}: the close button is not drawn"));
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
            .center()
            .y
            > close.bottom()
    }

    /// How many lines the filled header is drawn on, from painted bounds: two
    /// when any of what may leave the first line is centred below the close
    /// tile, which is on the first line whatever the header does.
    fn header_lines(vcx: &mut VisualTestContext, what: &str) -> usize {
        let close = vcx
            .debug_bounds("diff-overlay-close")
            .unwrap_or_else(|| panic!("{what}: the close button is not drawn"));
        let lower = TIERS
            .iter()
            .flat_map(|fields| fields.iter())
            .filter_map(|(_, selector)| vcx.debug_bounds(selector))
            .any(|field| field.center().y > close.bottom());
        1 + usize::from(lower)
    }

    /// Whether two painted boxes share area, ignoring a touching edge.
    fn overlaps(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
        let eps = px(0.5);
        a.left() < b.right() - eps
            && b.left() < a.right() - eps
            && a.top() < b.bottom() - eps
            && b.top() < a.bottom() - eps
    }

    /// The header on one line or on two, whichever it is asked to be: every
    /// control, count and word whole and inside the header's own bounds on
    /// both axes, nothing painted over anything else. On one line that is the
    /// header's whole contract and its height is the one it has with room;
    /// on two, the actions, the focused file's way back and the totals stay on
    /// the first, and what leaves it leaves in order (the chip with the
    /// Refreshing word, then the terminal's name, then the summary) to a
    /// second line below the close button, left to right, and the header is
    /// taller than one line.
    fn assert_filled_lines(
        vcx: &mut VisualTestContext,
        whole: &Whole,
        window_w: f32,
        one_line: Pixels,
        lines: usize,
        what: &str,
    ) {
        let widths = diff_widths(vcx);
        assert_eq!(
            header_lines(vcx, what),
            lines,
            "{what}: the filled header is not on {lines} line(s); painted widths: {widths}"
        );
        let h = diff_header(vcx, what);
        let eps = px(0.5);
        if lines == 1 {
            assert!(
                (f32::from(h.header.size.height) - f32::from(one_line)).abs() <= 0.5,
                "{what}: one line is {one_line:?} tall, the header is {:?}",
                h.header.size.height
            );
            assert_diff_header(vcx, whole, window_w, what);
            return;
        }
        assert_eq!(
            diff_whole(vcx, what),
            *whole,
            "{what}: a control, figure or word of the header is squeezed or gone"
        );
        assert!(
            h.header.size.height > one_line + px(1.),
            "{what}: two lines are taller than one ({one_line:?}), the header is {:?}",
            h.header.size.height
        );
        assert!(
            h.header.left() >= px(0.) - eps && h.header.right() <= px(window_w) + eps,
            "{what}: the header {:?} is outside the window",
            h.header
        );
        let tiers = tiers_lower(vcx, what);
        assert_tier_order(tiers, what);
        let mut first = Vec::new();
        for (name, selector) in FIRST_ROW {
            if let Some(field) = vcx.debug_bounds(selector) {
                first.push((name, field));
            }
        }
        // What stayed on the first line of what may leave it, and what left.
        let (mut kept, mut second) = (Vec::new(), Vec::new());
        for (tier, fields) in TIERS.iter().enumerate() {
            for (name, selector) in fields.iter() {
                if let Some(field) = vcx.debug_bounds(selector) {
                    if tiers[tier] == Some(true) {
                        second.push((*name, field));
                    } else {
                        kept.push((*name, field));
                    }
                }
            }
        }
        assert!(
            !second.is_empty(),
            "{what}: two lines, and nothing on the second; painted widths: {widths}"
        );
        for (name, field) in first.iter().chain(kept.iter()).chain(second.iter()) {
            assert!(
                field.left() >= h.header.left() - eps
                    && field.right() <= h.header.right() + eps
                    && field.top() >= h.header.top() - eps
                    && field.bottom() <= h.header.bottom() + eps,
                "{what}: the {name} {field:?} is outside the header {:?}; painted widths: {widths}",
                h.header
            );
        }
        // The first line, in order, on the first line: the actions and the
        // totals never leave it.
        let mut end: Option<Pixels> = None;
        for (name, field) in &first {
            assert!(
                field.center().y <= h.close.bottom(),
                "{what}: the {name} {field:?} is below the first line"
            );
            if let Some(end) = end {
                assert!(
                    field.left() >= end - eps,
                    "{what}: the {name} {field:?} starts before what precedes it ends, {end:?}"
                );
            }
            end = Some(field.right());
        }
        // What stayed beside them is clear of them; the terminal's name is
        // inside its own button.
        for (name, field) in &kept {
            assert!(
                field.center().y <= h.close.bottom(),
                "{what}: the {name} {field:?} is below the first line"
            );
            for (above, other) in &first {
                assert!(
                    (*name == "terminal name" && *above == "terminal button")
                        || !overlaps(*field, *other),
                    "{what}: the {name} {field:?} is painted over the {above} {other:?}"
                );
            }
        }
        // What left is on one line of its own, below the first, in the order
        // the tiers leave in, clear of everything on the first.
        let first_bottom = first
            .iter()
            .chain(kept.iter())
            .map(|(_, field)| f32::from(field.bottom()))
            .fold(f32::MIN, f32::max);
        for (name, field) in &second {
            assert!(
                f32::from(field.top()) >= first_bottom - 0.01,
                "{what}: the {name} {field:?} starts above the first line's bottom \
                 {first_bottom}: the lines are not separate bands"
            );
        }
        let band = second[0].1.center().y;
        let mut end: Option<Pixels> = None;
        for (name, field) in &second {
            assert!(
                (f32::from(field.center().y) - f32::from(band)).abs() <= 1.,
                "{what}: the {name} {field:?} is not on the second line's band {band:?}"
            );
            if let Some(end) = end {
                assert!(
                    field.left() >= end - eps,
                    "{what}: the {name} {field:?} starts before what precedes it ends, {end:?}"
                );
            }
            end = Some(field.right());
            for (above, other) in first.iter().chain(kept.iter()) {
                assert!(
                    !overlaps(*field, *other),
                    "{what}: the {name} {field:?} is painted over the {above} {other:?}"
                );
            }
        }
    }

    /// Where a filled diff's header goes onto a second line when the window
    /// narrows and back onto one when it widens, as window widths: the last
    /// that kept one line, the first that needed two, and the one it came back
    /// at. Walked through both histories, with the held answer checked at
    /// every width in the margin.
    fn wrap_boundary(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        held: &std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
        what: &str,
    ) -> (f32, f32, f32) {
        assert_holds(app, vcx, held, "before the wrap walk");
        resize(vcx, 2400., 900.);
        assert!(docked_px(app, vcx).is_none(), "{what}: the diff is filled");
        assert_eq!(header_lines(vcx, what), 1, "{what}: one line with room");
        let mut width = 2400.;
        while header_lines(vcx, what) == 1 {
            width -= 5.;
            assert!(
                width >= 720.,
                "{what}: the header never went onto a second line, down to the window minimum"
            );
            resize(vcx, width, 900.);
        }
        // From far above again, so no earlier answer is held.
        width += 5.;
        resize(vcx, 2400., 900.);
        resize(vcx, width, 900.);
        assert_eq!(header_lines(vcx, what), 1);
        let mut last_one = width;
        let first_two = loop {
            width -= 1.;
            resize(vcx, width, 900.);
            if header_lines(vcx, what) == 2 {
                break width;
            }
            last_one = width;
        };

        // History one: on two lines, widening holds them through the margin
        // and goes back to one only past it.
        let (mut up, mut held_two) = (first_two, 0);
        let back = loop {
            up += 1.;
            assert!(
                up < first_two + 200.,
                "{what}: the header never went back onto one line"
            );
            resize(vcx, up, 900.);
            if header_lines(vcx, what) == 1 {
                break up;
            }
            held_two += 1;
        };
        assert!(held_two > 0, "{what}: back to one line the moment it left");

        // History two: on one line again, one point narrower is still one
        // line, and it wraps only where it wrapped the first time.
        resize(vcx, back - 1., 900.);
        assert_eq!(
            header_lines(vcx, what),
            1,
            "{what}: one point under the return, inside the margin, it wrapped again"
        );
        let mut down = back - 1.;
        let wrapped_at = loop {
            down -= 1.;
            assert!(down > first_two - 200., "{what}: never wrapped again");
            resize(vcx, down, 900.);
            if header_lines(vcx, what) == 2 {
                break down;
            }
        };
        assert_eq!(
            wrapped_at, first_two,
            "{what}: from one line it must wrap where it wrapped the first time, not inside \
             the margin"
        );
        assert_holds(app, vcx, held, "after the wrap walk");
        (last_one, first_two, back)
    }

    /// The Staged diff that is reading, in a filled header, gives its metadata
    /// a second line only when its fixed parts and the least the names may
    /// keep do not fit one: at 1000px with the panel open, in either language,
    /// every field is whole on two lines; at 1440px, panel open or closed, the
    /// same states are on one line, as tall as with room; a docked header is
    /// never on two lines; the saved layout is untouched.
    #[gpui::test]
    fn a_filled_staged_diff_puts_its_metadata_on_a_second_line_only_when_it_must(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        resize(&mut vcx, 2400., 900.);
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            for loading in [true, false] {
                let held = open_staged_diff(&app, &mut vcx, loading);
                let state = format!("{locale} Staged, loading {loading}");

                // Docked, the header never takes a second line, whatever the
                // width, and the metadata a column drops stays dropped.
                set_right_panel(&app, &mut vcx, false);
                let mut width = 2400.;
                let mut docked = 0;
                while width >= 900. {
                    resize(&mut vcx, width, 900.);
                    if docked_px(&app, &mut vcx).is_some() {
                        docked += 1;
                        assert_eq!(
                            header_lines(&mut vcx, &state),
                            1,
                            "{state}: docked at {width}px the header is on two lines"
                        );
                    }
                    width -= 23.;
                }
                assert!(docked > 0, "{state}: the sweep never docked");

                // Filled with room: the reference for what whole is, and for
                // how tall a line is.
                app.update_in(&mut vcx, |app, _, cx| {
                    app.set_document_layout(DocumentLayout::Fill, cx)
                });
                resize(&mut vcx, 2400., 900.);
                assert!(docked_px(&app, &mut vcx).is_none(), "{state}: filled");
                let whole = diff_whole(&mut vcx, &state);
                assert!(
                    whole.chip.is_some()
                        && whole.added.is_some()
                        && whole.removed.is_some()
                        && whole.refreshing.is_some() == loading,
                    "{state}: the Staged header has its chip and both counts, and its \
                     Refreshing word only while reading: {whole:?}"
                );
                let one_line = diff_header(&mut vcx, &state).header.size.height;
                assert_filled_lines(&mut vcx, &whole, 2400., one_line, 1, &state);
                // The secondary text, whole at the wide reference: both nodes
                // must be there to be compared with.
                let (name_wide, summary_wide) = {
                    let wide = |vcx: &mut VisualTestContext, selector: &'static str| {
                        vcx.debug_bounds(selector)
                            .unwrap_or_else(|| panic!("{state}: {selector} is not drawn at 2400px"))
                            .size
                            .width
                    };
                    (
                        wide(&mut vcx, "document-terminal-name"),
                        wide(&mut vcx, "diff-overlay-summary"),
                    )
                };

                // 1440px, saved Fill, the panel open and closed: one line.
                for panel in [true, false] {
                    set_right_panel(&app, &mut vcx, panel);
                    resize(&mut vcx, 1440., 900.);
                    let what = format!("{state} filled at 1440px panel {panel}");
                    assert_holds(&app, &mut vcx, &held, &what);
                    assert_filled_lines(&mut vcx, &whole, 1440., one_line, 1, &what);
                }

                // 1000px, the saved layout the column and the diff filling by
                // fallback: reading, with the panel open, it is two lines;
                // either way every field is whole.
                app.update_in(&mut vcx, |app, _, cx| {
                    app.set_document_layout(DocumentLayout::Dock, cx)
                });
                for panel in [true, false] {
                    set_right_panel(&app, &mut vcx, panel);
                    resize(&mut vcx, 1440., 900.);
                    resize(&mut vcx, 1000., 700.);
                    let what = format!("{state} 1000px panel {panel}");
                    assert!(
                        docked_px(&app, &mut vcx).is_none(),
                        "{what}: the diff fills the workspace"
                    );
                    assert_holds(&app, &mut vcx, &held, &what);
                    let lines = if loading && panel {
                        2
                    } else {
                        header_lines(&mut vcx, &what)
                    };
                    assert_filled_lines(&mut vcx, &whole, 1000., one_line, lines, &what);
                    if lines == 2 {
                        // The chip, with the Refreshing word, is the first to
                        // leave the first line; the totals never do; and what
                        // else is on the second line is whole, at its wide
                        // width.
                        let tiers = tiers_lower(&mut vcx, &what);
                        assert_eq!(
                            tiers[0],
                            Some(true),
                            "{what}: the Staged chip is the first to leave the first line:                              {tiers:?}"
                        );
                        for (selector, wide) in [
                            ("diff-overlay-added", None),
                            ("diff-overlay-removed", None),
                            ("document-terminal-name", Some(name_wide)),
                            ("diff-overlay-summary", Some(summary_wide)),
                        ] {
                            let lower = on_second_line(&mut vcx, selector, &what);
                            match wide {
                                None => assert!(
                                    !lower,
                                    "{what}: {selector} left the first line: the totals stay"
                                ),
                                Some(wide) if lower => {
                                    let painted = vcx
                                        .debug_bounds(selector)
                                        .unwrap_or_else(|| panic!("{what}: {selector} gone"))
                                        .size
                                        .width;
                                    assert!(
                                        (f32::from(painted) - f32::from(wide)).abs() <= 0.5,
                                        "{what}: on the second line {selector} {painted:?} is \
                                         whole: {wide:?}"
                                    );
                                }
                                Some(_) => {}
                            }
                        }
                    }
                    assert!(
                        app.read_with(&vcx, |app, cx| app.document_layout(cx))
                            == DocumentLayout::Dock,
                        "{what}: the saved layout is still the column"
                    );
                }
                set_right_panel(&app, &mut vcx, false);
            }
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// The second line has the editor's margin through both histories, and
    /// the point it is taken at is the header's own need, in either language:
    /// on one line until what it must keep does not fit, on two until the
    /// margin past that is there, every field whole and in place at both
    /// edges and at every width between them.
    #[gpui::test]
    fn the_second_line_has_the_editors_margin_through_both_histories(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        resize(&mut vcx, 2400., 900.);
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            let held = open_staged_diff(&app, &mut vcx, true);
            app.update_in(&mut vcx, |app, _, cx| {
                app.set_document_layout(DocumentLayout::Fill, cx)
            });
            let what = format!("{locale} Staged, reading");
            resize(&mut vcx, 2400., 900.);
            let whole = diff_whole(&mut vcx, &what);
            let one_line = diff_header(&mut vcx, &what).header.size.height;

            let (last_one, first_two, back) = wrap_boundary(&app, &mut vcx, &held, &what);
            assert!(
                back - last_one >= HEADER_HYSTERESIS - 0.01
                    && back - last_one <= HEADER_HYSTERESIS + 1.,
                "{what}: one line again at {back} after the last one-line width {last_one}: not \
                 the margin of {HEADER_HYSTERESIS}"
            );
            assert!(
                back - first_two > HEADER_HYSTERESIS,
                "{what}: one line again at {back} after wrapping at {first_two}: no margin"
            );

            // From a wide window, the last width that has one line has it
            // whole, and the first that wraps has two lines whole.
            resize(&mut vcx, 2400., 900.);
            resize(&mut vcx, last_one, 900.);
            assert_filled_lines(
                &mut vcx,
                &whole,
                last_one,
                one_line,
                1,
                &format!("{what} last one line"),
            );
            resize(&mut vcx, first_two, 900.);
            assert_filled_lines(
                &mut vcx,
                &whole,
                first_two,
                one_line,
                2,
                &format!("{what} first two lines"),
            );

            // Widening from two lines: held through the margin, whole at each
            // width; one line only at the margin.
            let mut at = first_two;
            while at < back {
                resize(&mut vcx, at, 900.);
                assert_filled_lines(
                    &mut vcx,
                    &whole,
                    at,
                    one_line,
                    2,
                    &format!("{what} widening, {at}px"),
                );
                at += 1.;
            }
            resize(&mut vcx, back, 900.);
            assert_filled_lines(
                &mut vcx,
                &whole,
                back,
                one_line,
                1,
                &format!("{what} back on one line"),
            );
            // Narrowing from one line: held through the margin down to where
            // it first wrapped.
            let mut at = back;
            while at > first_two {
                resize(&mut vcx, at, 900.);
                assert_eq!(
                    header_lines(&mut vcx, &what),
                    1,
                    "{what}: narrowing, {at}px is inside the margin and keeps one line"
                );
                at -= 1.;
            }
            resize(&mut vcx, first_two, 900.);
            assert_eq!(
                header_lines(&mut vcx, &what),
                2,
                "{what}: wraps at {first_two}"
            );
            assert!(
                app.read_with(&vcx, |app, cx| app.document_layout(cx)) == DocumentLayout::Fill,
                "{what}: the layout the test asked for is untouched"
            );
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// A commit as the UI opens it from a file row (`scm/detail.rs`): the
    /// commit's own label, and the overlay focused on that file; or, as the
    /// bare-ref control, the same with no label. The counts are
    /// small: a focused header draws no stats.
    fn open_focused_commit(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        subject: &DiffRef,
        path: &str,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        use crate::terminal::git_diff::{DiffSnapshot, FileDiff, FileStatus};
        let snapshot = std::sync::Arc::new(DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source: subject.source(),
            branch: "move-tab-keybind".into(),
            files: vec![FileDiff {
                path: path.into(),
                old_path: None,
                status: FileStatus::Modified,
                added: 12,
                removed: 3,
                binary: false,
                truncated: None,
                hunks: Vec::new(),
            }],
            ..Default::default()
        });
        let held = std::sync::Arc::clone(&snapshot);
        app.update_in(vcx, |app, window, cx| {
            app.open_loaded_diff_for_test(snapshot, false, window, cx);
            let overlay = app.tabs[0].diff_overlay.as_mut().expect("the diff is open");
            overlay.focus = Some(path.to_string());
            cx.notify();
        });
        vcx.run_until_parked();
        assert_holds(app, vcx, &held, "a focused commit, freshly opened");
        assert!(
            vcx.debug_bounds("diff-overlay-unfocus").is_some(),
            "a focused commit draws the focused file's back button"
        );
        held
    }

    /// The focused file's back button is inside the header, on its first
    /// line, after the ref and before the view switch.
    fn assert_back_button(vcx: &mut VisualTestContext, what: &str) {
        let h = diff_header(vcx, what);
        let back = vcx
            .debug_bounds("diff-overlay-unfocus")
            .unwrap_or_else(|| panic!("{what}: the back button is not drawn"));
        let eps = px(0.5);
        assert!(
            back.left() >= h.header.left() - eps
                && back.right() <= h.header.right() + eps
                && back.top() >= h.header.top() - eps
                && back.bottom() <= h.header.bottom() + eps,
            "{what}: the back button {back:?} is outside the header {:?}",
            h.header
        );
        // The way back is an action: it is on the first line, after the ref
        // and before the view switch, on one line or two.
        assert!(
            back.center().y <= h.close.bottom(),
            "{what}: the back button {back:?} is below the first line"
        );
        if let Some(rev) = h.rev {
            assert!(
                back.left() >= rev.right() - eps,
                "{what}: the back button {back:?} starts inside the ref {rev:?}"
            );
        }
        assert!(
            back.right() <= h.view.left() + eps,
            "{what}: the back button {back:?} runs into the view switch {:?}",
            h.view
        );
    }

    /// A commit opened on a file, as its detail's file rows open it: the label
    /// and the focused file's back button are both on the header, and the ref
    /// is still whole at the narrowest window that lifts the header, at the last
    /// docked width and filled, for a short path and a long one. The points the
    /// header falls into its column and fills the workspace at are the same for
    /// both paths: they have identical parts, and a path's text moves nothing.
    #[gpui::test]
    fn a_focused_commit_keeps_its_ref_whole_and_its_back_button_in_place(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{LOCALES, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        const PATHS: [&str; 2] = [
            "a.rs",
            "src/ui/components/very/deep/directory/tree/with/a-file-name-that-keeps-going.rs",
        ];
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 800.);
        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            for subject in [DiffRef::Commit(COMMIT_SHA), DiffRef::BareCommit(COMMIT_SHA)] {
                let (mut enters, mut fills) = (Vec::new(), Vec::new());
                for path in PATHS {
                    let held = open_focused_commit(&app, &mut vcx, &subject, path);
                    let kind = if subject.labelled() {
                        "commit"
                    } else {
                        "bare ref"
                    };
                    let what = format!("{locale} focused {kind} on {path}");
                    resize(&mut vcx, 2400., 800.);
                    let natural = diff_header(&mut vcx, &what)
                        .rev
                        .expect("a revision's subject is drawn")
                        .size
                        .width;
                    // An already squeezed ref at 2400px must not become the
                    // reference: it is checked against the text system's own width.
                    let shaped = shaped_rev(&app, &mut vcx, &subject.drawn());
                    assert!(
                        (f32::from(natural) - shaped).abs() <= 2.,
                        "{what}: the ref is {natural:?} at 2400px, {shaped}px shaped"
                    );
                    let whole = diff_whole(&mut vcx, &what);
                    assert_diff_header(&mut vcx, &whole, 2400., &what);
                    assert_back_button(&mut vcx, &what);

                    // Lifted into the title bar: the narrowest window that keeps it
                    // there has the ref whole and the back button in place.
                    let (last_lifted, enter, leave, narrowest) =
                        diff_boundary(&app, &mut vcx, &held);
                    assert!(
                        leave - enter > HEADER_HYSTERESIS
                            && leave - last_lifted < HEADER_HYSTERESIS + 1.,
                        "{what}: lifted again at {leave} after {enter} (last lifted {last_lifted}): \
                     not the margin of {HEADER_HYSTERESIS}"
                    );
                    resize(&mut vcx, 2400., 800.);
                    resize(&mut vcx, narrowest, 800.);
                    let widths = diff_widths(&mut vcx);
                    let rev = diff_header(&mut vcx, &what).rev.map(|r| r.size.width);
                    assert_eq!(
                        rev,
                        Some(natural),
                        "{what}: at the narrowest lifted window the ref is whole; painted widths: \
                     {widths}"
                    );
                    assert_diff_header(&mut vcx, &whole, narrowest, &what);
                    assert_back_button(&mut vcx, &what);

                    // The column, and the point it fills the workspace at.
                    let (last_docked, first_fill, back) = fill_boundary(&app, &mut vcx, &held);
                    assert!(
                        back - last_docked >= HEADER_HYSTERESIS - 0.01
                            && back - last_docked <= HEADER_HYSTERESIS + 1.,
                        "{what}: docked again at {back} after {last_docked}: not the margin"
                    );
                    resize(&mut vcx, 2400., 800.);
                    resize(&mut vcx, last_docked, 800.);
                    let widths = diff_widths(&mut vcx);
                    let rev = diff_header(&mut vcx, &what).rev.map(|r| r.size.width);
                    assert_eq!(
                        rev,
                        Some(natural),
                        "{what}: at the last docked width the ref is whole; painted widths: {widths}"
                    );
                    assert_diff_header(&mut vcx, &whole, last_docked, &what);
                    assert_back_button(&mut vcx, &what);

                    // Filled by the fallback, the header is whatever it needs to be
                    // to keep every part whole, on one line or two.
                    app.update_in(&mut vcx, |app, _, cx| {
                        app.set_document_layout(DocumentLayout::Fill, cx)
                    });
                    resize(&mut vcx, 2400., 800.);
                    let one_line = diff_header(&mut vcx, &what).header.size.height;
                    app.update_in(&mut vcx, |app, _, cx| {
                        app.set_document_layout(DocumentLayout::Dock, cx)
                    });
                    resize(&mut vcx, 2400., 800.);
                    for (filled, panel) in [
                        (first_fill, false),
                        (first_fill - 40., false),
                        (1000., true),
                    ] {
                        set_right_panel(&app, &mut vcx, panel);
                        resize(&mut vcx, 2400., 800.);
                        resize(&mut vcx, filled, 800.);
                        let at = format!("{what} filled at {filled}px panel {panel}");
                        assert!(
                            docked_px(&app, &mut vcx).is_none(),
                            "{at}: the diff fills the workspace"
                        );
                        assert_holds(&app, &mut vcx, &held, &at);
                        let widths = diff_widths(&mut vcx);
                        let rev = diff_header(&mut vcx, &at).rev.map(|r| r.size.width);
                        assert_eq!(
                            rev,
                            Some(natural),
                            "{at}: a short ref is whole; painted widths: {widths}"
                        );
                        let lines = header_lines(&mut vcx, &at);
                        assert_filled_lines(&mut vcx, &whole, filled, one_line, lines, &at);
                        assert_back_button(&mut vcx, &at);
                        assert!(
                            app.read_with(&vcx, |app, cx| app.document_layout(cx))
                                == DocumentLayout::Dock,
                            "{at}: the saved layout is still the column"
                        );
                    }
                    set_right_panel(&app, &mut vcx, false);
                    enters.push(enter);
                    fills.push(first_fill);
                }
                assert!(
                    enters[0] == enters[1] && fills[0] == fills[1],
                    "{locale} {}: a path's text moved a threshold: lifted {enters:?}, filled {fills:?}",
                    subject.drawn()
                );
            }
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// A Staged diff: the branch with its chip beside it and, while `loading`,
    /// the Refreshing word, over a file whose counts are seven digits.
    fn open_staged_diff(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        loading: bool,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        open_staged_counts(app, vcx, loading, 1_234_567, 7_654_321)
    }

    /// [`open_staged_diff`] with the counts the one file holds: the seven-digit
    /// ones are a stress case; a native capture's are `50_000` and `50_000`.
    fn open_staged_counts(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        loading: bool,
        added: u32,
        removed: u32,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        use crate::terminal::git_diff::{DiffSnapshot, DiffSource, FileDiff, FileStatus};
        let snapshot = std::sync::Arc::new(DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source: DiffSource::Staged,
            branch: "move-tab-keybind".into(),
            files: vec![FileDiff {
                path: "a.rs".into(),
                old_path: None,
                status: FileStatus::Modified,
                added,
                removed,
                binary: false,
                truncated: None,
                hunks: Vec::new(),
            }],
            ..Default::default()
        });
        let held = std::sync::Arc::clone(&snapshot);
        app.update_in(vcx, |app, window, cx| {
            app.open_loaded_diff_for_test(snapshot, loading, window, cx);
        });
        vcx.run_until_parked();
        assert_holds(app, vcx, &held, "a Staged diff, freshly opened");
        held
    }

    /// The Staged header at the narrowest window that still lifts it: whole,
    /// clear of the pill and inside its column. Returns the column width it
    /// drops at.
    fn staged_edge(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        held: &std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
        what: &str,
    ) -> f32 {
        let (_, enter, _, narrowest) = diff_boundary(app, vcx, held);
        // The boundary walk ends in the column, inside the margin; from the
        // wide window again the header holds the bar down to its narrowest.
        resize(vcx, 2400., 800.);
        // What the header paints with room, under the settings as they are
        // now: the chip, the Refreshing word and both counts must all be
        // there, and every narrower state paints each of them at that width.
        let whole = diff_whole(vcx, what);
        assert!(
            whole.chip.is_some()
                && whole.refreshing.is_some()
                && whole.added.is_some()
                && whole.removed.is_some(),
            "{what}: a Staged, reading header has its chip, its Refreshing word and both counts: \
             {whole:?}"
        );
        resize(vcx, narrowest, 800.);
        assert!(!diff_inline(vcx), "{what}: lifted at its narrowest window");
        assert_holds(app, vcx, held, what);
        assert_diff_header(vcx, &whole, narrowest, what);
        enter
    }

    fn set_config(vcx: &mut VisualTestContext, change: impl FnOnce(&mut Config)) {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            change(&mut cfg);
            cx.set_global(cfg);
        });
        vcx.run_until_parked();
    }

    /// The Staged chip and the Refreshing word are measured, so the header's
    /// budget must follow what they are drawn at: at the narrowest window that
    /// lifts the header, after each live change of the interface font size,
    /// the language, the editor font and the ligature setting, nothing is
    /// pushed onto the pill or out of the column, and the size and the
    /// language move the point where the header gives way.
    #[gpui::test]
    fn the_staged_diff_header_budget_follows_live_changes_to_its_words(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 2400., 800.);
        let held = open_staged_diff(&app, &mut vcx, true);

        let rest = staged_edge(&app, &mut vcx, &held, "at rest");
        set_ui_font_size(&app, &mut vcx, 24.);
        let larger = staged_edge(&app, &mut vcx, &held, "a larger interface font");
        assert!(
            larger > rest,
            "the header gives way at {larger} with a larger font, {rest} at rest"
        );
        set_ui_font_size(&app, &mut vcx, 16.);

        use_locale(&app, &mut vcx, "ru-RU", "Агенты");
        let russian = staged_edge(&app, &mut vcx, &held, "Russian");
        assert_ne!(
            russian, rest,
            "the Staged and Refreshing words did not move the point it gives way"
        );
        use_locale(&app, &mut vcx, "en", "Agents");

        let family = vcx.update(|_, cx| cx.global::<Config>().font_family.clone());
        set_config(&mut vcx, |cfg| cfg.font_family = "DejaVu Sans Mono".into());
        staged_edge(&app, &mut vcx, &held, "another editor font");
        set_config(&mut vcx, |cfg| {
            cfg.font_features = Some(crate::core::config::FontFeatures(std::sync::Arc::new(
                vec![("calt".to_string(), 1), ("liga".to_string(), 1)],
            )))
        });
        staged_edge(&app, &mut vcx, &held, "ligatures on");
        set_config(&mut vcx, |cfg| {
            cfg.font_family = family;
            cfg.font_features = None;
        });
        let back = staged_edge(&app, &mut vcx, &held, "back at rest");
        assert_eq!(back, rest, "the same settings give the same point");
    }

    /// Everything the native Staged, reading header draws, as selectors: each
    /// must be drawn, at the wide reference and at every target. A missing node
    /// is a failure, never something to skip.
    const NATIVE_NODES: [(&str, &str); 11] = [
        ("subject icon", "diff-overlay-icon"),
        ("Staged chip", "ui-chip"),
        ("Refreshing word", "diff-overlay-refreshing"),
        ("terminal button", "document-back-to-terminal"),
        ("terminal name", "document-terminal-name"),
        ("summary", "diff-overlay-summary"),
        ("added figure", "diff-overlay-added"),
        ("removed figure", "diff-overlay-removed"),
        ("view switch", "diff-overlay-view"),
        ("layout switch", "document-layout-switch"),
        ("close", "diff-overlay-close"),
    ];

    /// What the filled header's parts measure with room, as painted, and what
    /// each keeps at the least: the numbers a failure reports. Read from
    /// painted bounds and the text system, never from the header's own budget,
    /// so a test that reports them cannot agree with the header by sharing its
    /// arithmetic.
    struct Wide {
        lead: f32,
        trail: f32,
        gap: f32,
        /// What never leaves the first line, as painted: the terminal button
        /// without its name, the subject's icon, the totals, the two switches
        /// and the close tile.
        fixed: Vec<(&'static str, f32)>,
        /// What may leave it, whole and at the least it keeps: the chip, the
        /// Refreshing word, the terminal's name and the summary.
        movable: Vec<(&'static str, f32, f32)>,
        /// The margins the two switches are drawn with, before each of them,
        /// on top of the row's gap.
        margins: f32,
        /// The gap between the terminal's badge and its name, which goes with
        /// the name.
        name_gap: f32,
        /// One line of the summary, as painted: its height with room.
        summary_line_h: f32,
        /// Whether the diff has totals to draw: the two counts are parts of
        /// the header only then, and are required exactly then.
        totals: bool,
        /// The least the subject keeps. A branch name gives way entirely, so
        /// it has no floor of its own but is still a child of the row, and
        /// takes a gap.
        subject_floor: f32,
    }

    impl Wide {
        /// The parts this fixture's header draws, every one of them required.
        fn nodes(&self) -> Vec<(&'static str, &'static str)> {
            NATIVE_NODES
                .iter()
                .copied()
                .filter(|(name, _)| self.totals || !name.ends_with("figure"))
                .collect()
        }

        /// The order the parts read in the overflow rows for this fixture.
        fn order(&self) -> Vec<(&'static str, &'static str)> {
            OVERFLOW_ORDER
                .iter()
                .copied()
                .filter(|(name, _)| self.totals || !name.ends_with("figure"))
                .collect()
        }

        /// The first line's core: the fixed parts, the subject at its floor,
        /// the lead and trail, and a gap between each of the row's children
        /// (the fixed parts and the subject).
        fn core(&self) -> f32 {
            self.lead
                + self.trail
                + self.margins
                + self.fixed.iter().map(|(_, w)| w).sum::<f32>()
                + self.subject_floor
                + self.gap * self.fixed.len() as f32
        }

        /// The tier each of the movable parts is in: the chip and the
        /// Refreshing word, the terminal's name, the summary.
        const TIER_OF: [usize; 4] = [0, 0, 1, 2];

        /// The first line with the first `level` tiers on a second, from the
        /// painted widths: what never leaves, the terminal group (with its
        /// name's gap and the name while the name is there), the tiers that
        /// are still there at the least they keep, a gap between each child.
        /// The switches keep their margins, except at the last level where
        /// they may yield to nothing.
        fn first_line_at(&self, level: usize) -> f32 {
            let mut width = self.lead
                + self.trail
                + if level >= 3 { 0. } else { self.margins }
                + self.fixed.iter().map(|(_, w)| w).sum::<f32>()
                + self.subject_floor;
            let mut children = self.fixed.len() + 1;
            if level < 2 {
                width += self.name_gap;
            }
            for (part, (label, _, kept)) in self.movable.iter().enumerate() {
                if Self::TIER_OF[part] >= level {
                    width += kept;
                    if *label != "terminal name" {
                        children += 1;
                    }
                }
            }
            width + self.gap * (children as f32 - 1.)
        }

        /// The second line with the first `level` tiers on it, or nothing.
        fn second_line_at(&self, level: usize) -> f32 {
            let moved: Vec<f32> = self
                .movable
                .iter()
                .enumerate()
                .filter(|(part, _)| Self::TIER_OF[*part] < level)
                .map(|(_, (_, _, kept))| *kept)
                .collect();
            if moved.is_empty() {
                return 0.;
            }
            self.lead
                + self.trail
                + moved.iter().sum::<f32>()
                + self.gap * (moved.len() as f32 - 1.)
        }

        /// The least body any ladder level fits both lines in, from the
        /// painted widths: the fewest pixels at which the header needs no
        /// more rows than the ladder's two.
        fn ladder_threshold(&self) -> f32 {
            (0..=3)
                .map(|level| self.first_line_at(level).max(self.second_line_at(level)))
                .fold(f32::MAX, f32::min)
        }

        /// The second line with every tier on it, each at the least it keeps.
        fn second_line(&self) -> f32 {
            self.lead
                + self.trail
                + self.movable.iter().map(|(_, _, kept)| kept).sum::<f32>()
                + self.gap * (self.movable.len() as f32 - 1.).max(0.)
        }

        /// Everything on one line, each at the least it keeps: the core and
        /// the movable parts, of which only the chip, the Refreshing word and
        /// the summary are children of their own (the name is in the button).
        fn full(&self) -> f32 {
            self.core()
                + self.movable.iter().map(|(_, _, kept)| kept).sum::<f32>()
                + self.gap * (self.movable.len() as f32 - 1.).max(0.)
        }

        fn report(&self, body: f32) -> String {
            format!(
                "body {body}px; first-line core {}px (lead {}, trail {}, switch margins {}, gap {} \
                 between each of {} children, subject floor {}, fixed {:?}); second line with every tier on it \
                 {}px; everything on one line at the least each keeps {}px; movable (whole, \
                 kept) {:?}",
                self.core(),
                self.lead,
                self.trail,
                self.margins,
                self.gap,
                self.fixed.len() + 1,
                self.subject_floor,
                self.fixed,
                self.second_line(),
                self.full(),
                self.movable
            )
        }
    }

    /// The filled header with room (the window wide, the diff filling it),
    /// measured as painted. Every native node must be drawn.
    fn measure_wide(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        what: &str,
        totals: bool,
    ) -> Wide {
        for (name, selector) in NATIVE_NODES {
            let wanted = totals || !name.ends_with("figure");
            assert_eq!(
                vcx.debug_bounds(selector).is_some(),
                wanted,
                "{what}: the {name} ({selector}) must {}be drawn: the fixture has {}totals",
                if wanted { "" } else { "not " },
                if totals { "" } else { "no " }
            );
        }
        let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        let width = |vcx: &mut VisualTestContext, selector: &'static str| {
            f32::from(bounds(vcx, selector).size.width)
        };
        let header = bounds(vcx, "diff-overlay-header");
        let group = bounds(vcx, "document-terminal-group");
        let badge = bounds(vcx, "document-terminal-badge");
        let name_box = bounds(vcx, "document-terminal-name");
        let close = bounds(vcx, "diff-overlay-close");
        let name = f32::from(name_box.size.width);
        // The whole terminal group (button, divider and its margins) without
        // the name and the gap between the badge and the name, which go
        // together when the name leaves.
        let name_gap = f32::from(name_box.left() - badge.right());
        let gap = 0.5 * vcx.update(|window, _| window.rem_size().as_f32());
        // Each switch is drawn with a margin before it, on top of the row's
        // gap: between the two it is the painted space less the gap.
        let (view, layout) = (
            bounds(vcx, "diff-overlay-view"),
            bounds(vcx, "document-layout-switch"),
        );
        let switch_lead = f32::from(layout.left() - view.right()) - gap;
        let mut fixed = vec![
            (
                "terminal group without its name and badge gap",
                f32::from(group.size.width) - name - name_gap,
            ),
            ("subject icon", width(vcx, "diff-overlay-icon")),
        ];
        for (label, selector) in [
            ("added figure", "diff-overlay-added"),
            ("removed figure", "diff-overlay-removed"),
            ("view switch", "diff-overlay-view"),
            ("layout switch", "document-layout-switch"),
            ("close", "diff-overlay-close"),
        ] {
            if totals || !label.ends_with("figure") {
                fixed.push((label, width(vcx, selector)));
            }
        }
        let name_kept = kept_prefix(
            app,
            vcx,
            TAB_TITLE,
            crate::ui::document_column::HEADER_CELL_TEXT,
            false,
        );
        let summary_text =
            crate::ui::i18n::t_plural(crate::ui::i18n::L10nKey::DiffChangedFiles, 1, &[]);
        let summary_kept = kept_prefix(app, vcx, &summary_text, crate::ui::right_panel::META, true);
        let movable = vec![
            ("Staged chip", width(vcx, "ui-chip"), width(vcx, "ui-chip")),
            (
                "Refreshing word",
                width(vcx, "diff-overlay-refreshing"),
                width(vcx, "diff-overlay-refreshing"),
            ),
            ("terminal name", name, name_kept),
            ("summary", width(vcx, "diff-overlay-summary"), summary_kept),
        ];
        Wide {
            lead: f32::from(group.left() - header.left()),
            trail: f32::from(header.right() - close.right()),
            gap,
            margins: 2. * switch_lead,
            name_gap,
            summary_line_h: f32::from(bounds(vcx, "diff-overlay-summary").size.height),
            totals,
            fixed,
            movable,
            subject_floor: 0.,
        }
    }

    /// The window put into one scene: the saved layout, the panel, and the
    /// size, reached from a wide window so that no earlier answer is held.
    fn put_in_scene(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        saved: DocumentLayout,
        panel: bool,
        width: f32,
        height: f32,
    ) {
        app.update_in(vcx, |app, _, cx| app.set_document_layout(saved, cx));
        set_right_panel(app, vcx, panel);
        resize(vcx, 2400., 900.);
        resize(vcx, width, height);
    }

    /// The margin before each of the two switches, as painted: the space
    /// between what is before it and the switch, less the row's gap.
    fn switch_leads(vcx: &mut VisualTestContext, wide: &Wide, what: &str) -> (f32, f32) {
        let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        let removed = bounds(vcx, "diff-overlay-removed");
        let view = bounds(vcx, "diff-overlay-view");
        let layout = bounds(vcx, "document-layout-switch");
        let close = bounds(vcx, "diff-overlay-close");
        // What comes before the view switch: the Refreshing word when it is on
        // the first line, after the totals; the totals otherwise.
        let before = match vcx.debug_bounds("diff-overlay-refreshing") {
            Some(word) if word.center().y <= close.bottom() => word,
            _ => removed,
        };
        (
            f32::from(view.left() - before.right()) - wide.gap,
            f32::from(layout.left() - view.right()) - wide.gap,
        )
    }

    /// The filled header at the largest interface font, in the window as it
    /// is, against the body it is actually drawn in: the shell's painted
    /// bounds. The first-line core and the second line's minima must each fit
    /// that body (the numbers are the failure), the header and every native
    /// node must lie inside it, and the lines are the contract of
    /// [`assert_filled_lines`]. The body the layout models is a labelled
    /// control. With `oracle`, the number of lines is also what the painted
    /// minima say: two when everything on one line at the least each keeps is
    /// wider than the body, one otherwise.
    #[allow(clippy::too_many_arguments)]
    fn assert_fits_its_body(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        wide: &Wide,
        whole: &Whole,
        one_line: Pixels,
        window_w: f32,
        oracle: bool,
        what: &str,
    ) {
        let modeled = vcx.update(|window, cx| app.read(cx).document_body_px(window, cx));
        let body = vcx
            .debug_bounds("diff-overlay-body")
            .unwrap_or_else(|| panic!("{what}: the overlay's body is not drawn"));
        let body_w = f32::from(body.size.width);
        let report = format!(
            "{}; modeled body (a control) {modeled}px; painted widths: {}",
            wide.report(body_w),
            diff_widths(vcx)
        );
        // Where no ladder level fits both lines the header is in the overflow
        // rows, which carry their own contract ([`assert_overflow_rows`]): the
        // first-line core and the second line's minima below are the ladder's
        // precondition, and a body under them (the 402px a panel with a
        // tab-count slot leaves at 1000px) is the overflow's case, not a
        // reason for a taller ladder.
        if wide.ladder_threshold() - 3. > body_w {
            assert_overflow_rows(app, vcx, wide, whole, one_line, &summary_text(1, 0), what);
            return;
        }
        // The switches' margins yield to what the first line lacks, so what
        // has to fit is the core with them at nothing.
        let zero_lead_core = wide.core() - wide.margins;
        assert!(
            zero_lead_core <= body_w + 0.01,
            "{what}: the first-line core with the switches' margins at nothing, \
             {zero_lead_core}px, is wider than the painted body, {body_w}px; {report}"
        );
        assert!(
            wide.second_line() <= body_w + 0.01,
            "{what}: the second line's minima, {}px, are wider than the painted body, \
             {body_w}px; {report}",
            wide.second_line()
        );
        let eps = px(0.5);
        let inside = |field: Bounds<Pixels>| {
            field.left() >= body.left() - eps
                && field.right() <= body.right() + eps
                && field.top() >= body.top() - eps
                && field.bottom() <= body.bottom() + eps
        };
        let h = diff_header(vcx, what);
        assert!(
            inside(h.header),
            "{what}: the header {:?} is outside the painted body {body:?}; {report}",
            h.header
        );
        for (name, selector) in NATIVE_NODES {
            let field = vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: the {name} ({selector}) is not drawn"));
            assert!(
                inside(field),
                "{what}: the {name} {field:?} is outside the painted body {body:?}; {report}"
            );
        }
        let lines = header_lines(vcx, what);
        // The margins are whole (8 each) wherever the last ladder level is not
        // what is drawn or the first line fits with them, and otherwise give
        // up what the first line is short of, half each.
        let (lead_view, lead_layout) = switch_leads(vcx, wide, what);
        let all_moved = lines == 2 && tiers_moved(vcx, what) == 3;
        let short = (wide.core() - body_w).max(0.);
        let combined = if all_moved {
            (wide.margins - short).clamp(0., wide.margins)
        } else {
            wide.margins
        };
        // Whole margins are whole pixels and are checked exactly. A margin that
        // has yielded is not a whole device pixel, and the layout snaps each
        // edge to the device grid: it is painted as planned to within one
        // device pixel (0.5px at 2x, 1px at 1x), and that is the whole allowance.
        let device_px = 1. / vcx.update(|window, _| window.scale_factor());
        let allowed = if all_moved && short > 0. {
            device_px + 0.01
        } else {
            0.01
        };
        assert!(
            (lead_view - combined / 2.).abs() <= allowed
                && (lead_layout - combined / 2.).abs() <= allowed,
            "{what}: the switches' margins are {lead_view}px and {lead_layout}px, not {}px each \
             to within {allowed}px (the first line is {short}px short of the body, all tiers \
             moved: {all_moved}); {report}",
            combined / 2.
        );
        // The owner's case, the Russian 442px body: the core is 1.5px over it,
        // so the two margins have 14.5px between them.
        if (body_w - 442.).abs() <= 0.5 && all_moved {
            assert!(
                (lead_view + lead_layout - 14.5).abs() <= device_px + 0.01,
                "{what}: at the 442px body the margins add up to {}px, not 14.5px to within one \
                 device pixel, {device_px}px; {report}",
                lead_view + lead_layout
            );
        }
        if oracle {
            let expected = if body_w < wide.full() { 2 } else { 1 };
            assert_eq!(
                lines,
                expected,
                "{what}: the header is on {lines} line(s); the painted minima ({}px on one \
                 line, against a {body_w}px body) say {expected}. The native capture's two \
                 rows at 1440px are a native-text fact, proven by the capture; the test \
                 backend's glyphs are 0.6 em; {report}",
                wide.full()
            );
        }
        assert!(
            (modeled - body_w).abs() <= 1.,
            "{what}: the modeled body {modeled}px is not the painted body {body_w}px (a \
             control of the layout's arithmetic); {report}"
        );
        assert_filled_lines(vcx, whole, window_w, one_line, lines, what);
    }

    /// The scenes of the native capture at the largest interface font, in the
    /// order the narrowest comes first: 1000px with the panel open (saved
    /// layout the column, so the diff fills by fallback), then 1440px with the
    /// panel open and closed (saved layout Fill).
    const NATIVE_SCENES: [(DocumentLayout, f32, f32, bool); 3] = [
        (DocumentLayout::Dock, 1000., 700., true),
        (DocumentLayout::Fill, 1440., 900., true),
        (DocumentLayout::Fill, 1440., 900., false),
    ];

    /// A UI24 window in Russian with the native capture's diff open: Staged,
    /// reading, over one file of 50000 added and 50000 removed.
    fn native_ui24_diff(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        std::sync::Arc<crate::terminal::git_diff::DiffSnapshot>,
        Vec<crate::daemon::transport::Stream>,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        set_ui_font_size(&app, &mut vcx, 24.);
        use_locale(&app, &mut vcx, "ru-RU", "Агенты");
        resize(&mut vcx, 2400., 900.);
        let held = open_staged_counts(&app, &mut vcx, true, 50_000, 50_000);
        (app, vcx, held, streams)
    }

    /// The Staged, reading header at the largest interface font in Russian,
    /// with the native capture's totals (50000 and 50000): at 1000px with the
    /// panel open (a body that natively measures about 437px) and at 1440px
    /// with it open and closed, every action, total, chip, word and name is
    /// inside the body the shell paints. All the measurements are printed
    /// before the first assertion, the narrowest scene first, so a failure at
    /// 1440px cannot hide the numbers of 1000px. The seven-digit totals are
    /// covered, at the default font, by the tests above.
    #[gpui::test]
    fn at_the_largest_font_a_filled_staged_header_stays_inside_its_body(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        let state = "UI24 ru-RU Staged reading, native 50000/50000";
        put_in_scene(&app, &mut vcx, DocumentLayout::Fill, false, 2400., 900.);
        assert!(docked_px(&app, &mut vcx).is_none(), "{state}: filled");
        let whole = diff_whole(&mut vcx, state);
        let one_line = diff_header(&mut vcx, state).header.size.height;
        let wide = measure_wide(&app, &mut vcx, state, true);
        // With room the margins are whole: 8px each, as they always were.
        let (lead_view, lead_layout) = switch_leads(&mut vcx, &wide, state);
        assert!(
            (lead_view - 8.).abs() <= 0.01 && (lead_layout - 8.).abs() <= 0.01,
            "{state}: with room the switches' margins are {lead_view}px and {lead_layout}px, not 8"
        );

        // The numbers first, for every scene, before anything is asserted.
        for (saved, width, height, panel) in NATIVE_SCENES {
            put_in_scene(&app, &mut vcx, saved, panel, width, height);
            let body = vcx
                .debug_bounds("diff-overlay-body")
                .map(|b| f32::from(b.size.width));
            let lines = header_lines(&mut vcx, state);
            eprintln!(
                "{state} at {width}px panel {panel}: {lines} line(s), painted body {body:?}; {}; \
                 painted widths: {}",
                wide.report(body.unwrap_or(f32::NAN)),
                diff_widths(&mut vcx)
            );
        }

        for (saved, width, height, panel) in NATIVE_SCENES {
            put_in_scene(&app, &mut vcx, saved, panel, width, height);
            let what = format!("{state} at {width}px panel {panel}");
            assert!(
                docked_px(&app, &mut vcx).is_none(),
                "{what}: the diff fills the workspace"
            );
            assert_holds(&app, &mut vcx, &held, &what);
            assert_fits_its_body(&app, &mut vcx, &wide, &whole, one_line, width, true, &what);
        }
        set_right_panel(&app, &mut vcx, false);
        set_ui_font_size(&app, &mut vcx, 16.);
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// Narrowing the window at the largest interface font, from 2400px down to
    /// 1000px (the narrowest scene of the native capture) with the panel open
    /// and closed, the filled header gives up its secondary text in order: the
    /// chip with the Refreshing word first, then the terminal's name, then the
    /// summary, the actions and the totals never; once a tier has left it
    /// stays away, and every width is drawn whole and inside the painted body.
    /// The bands and the order are read from painted bounds.
    #[gpui::test]
    fn at_the_largest_font_the_filled_header_gives_up_its_secondary_text_in_order(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        let mut saw_two = false;
        for panel in [false, true] {
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, panel, 2400., 900.);
            let state = format!("UI24 ru-RU Staged reading, panel {panel}");
            assert_holds(&app, &mut vcx, &held, &state);
            let whole = diff_whole(&mut vcx, &state);
            let one_line = diff_header(&mut vcx, &state).header.size.height;
            let wide = measure_wide(&app, &mut vcx, &state, true);
            let summary = summary_text(1, 0);
            let threshold = wide.ladder_threshold();
            let mut width = 2400.;
            let mut gone = 0;
            while width >= 1000. {
                resize(&mut vcx, width, 900.);
                let what = format!("{state} at {width}px");
                // Below the least body any ladder level fits both lines in (the
                // painted one, with the switches' margins at nothing) the header
                // is in the overflow rows, and every bound of those is checked
                // strictly; at and above it, it is the ladder, as it always was.
                if painted_body(&mut vcx, &what) < threshold {
                    assert_overflow_rows(&app, &mut vcx, &wide, &whole, one_line, &summary, &what);
                    width -= 5.;
                    continue;
                }
                assert_fits_its_body(&app, &mut vcx, &wide, &whole, one_line, width, false, &what);
                let tiers = tiers_lower(&mut vcx, &what);
                assert_tier_order(tiers, &what);
                let now = tiers.iter().filter(|tier| **tier == Some(true)).count();
                assert!(
                    now >= gone,
                    "{what}: narrowing, a tier came back to the first line: {tiers:?}"
                );
                gone = now;
                saw_two |= header_lines(&mut vcx, &what) == 2;
                width -= 5.;
            }
        }
        assert!(
            saw_two,
            "the walk never saw the header on two lines, so it showed nothing about the order"
        );
        set_ui_font_size(&app, &mut vcx, 16.);
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// How many tiers of the filled header's secondary parts are on its second
    /// line, from painted bounds.
    fn tiers_moved(vcx: &mut VisualTestContext, what: &str) -> usize {
        tiers_lower(vcx, what)
            .iter()
            .filter(|tier| **tier == Some(true))
            .count()
    }

    /// Each level the filled header goes through at the largest interface
    /// font has the editor's margin, in both histories, in a real render:
    /// narrowing one point at a time from 2400px to 1000px a tier is added to
    /// the second line at some width, and widening back from 1000px it is
    /// taken off again only `HEADER_HYSTERESIS` past the last width that kept
    /// it, for every level, with the panel closed and open. Levels only ever
    /// add tiers while narrowing and take them off while widening.
    #[gpui::test]
    fn at_the_largest_font_each_level_has_the_editors_margin_through_both_histories(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        for panel in [false, true] {
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, panel, 2400., 900.);
            let state = format!("UI24 ru-RU Staged reading, panel {panel}");
            assert_holds(&app, &mut vcx, &held, &state);
            let mut down: [Option<f32>; 3] = [None; 3];
            let (mut width, mut prev) = (2400., 0);
            while width >= 1000. {
                resize(&mut vcx, width, 900.);
                let now = tiers_moved(&mut vcx, &state);
                assert!(
                    now >= prev,
                    "{state}: narrowing, at {width}px a tier came back to the first line"
                );
                down[prev..now].iter_mut().for_each(|at| *at = Some(width));
                prev = now;
                width -= 1.;
            }
            let mut up: [Option<f32>; 3] = [None; 3];
            let mut width = 1000.;
            while width <= 2400. {
                resize(&mut vcx, width, 900.);
                let now = tiers_moved(&mut vcx, &state);
                assert!(
                    now <= prev,
                    "{state}: widening, at {width}px a tier left the first line"
                );
                up[now..prev].iter_mut().for_each(|at| *at = Some(width));
                prev = now;
                width += 1.;
            }
            assert!(
                down[0].is_some(),
                "{state}: no tier left the first line between 2400px and 1000px, so no margin \
                 was observed"
            );
            for (level, (down, up)) in down.iter().zip(up.iter()).enumerate() {
                let what = format!("{state}, level {}", level + 1);
                match (down, up) {
                    (None, None) => {}
                    (Some(down), Some(up)) => {
                        // The last width that kept the tier on the first line
                        // is a point wider than the first that moved it.
                        let kept = down + 1.;
                        assert!(
                            *up - kept >= HEADER_HYSTERESIS - 0.01
                                && *up - kept <= HEADER_HYSTERESIS + 1.,
                            "{what}: moved at {down}px (last kept at {kept}px), back at {up}px: \
                             not the margin of {HEADER_HYSTERESIS}"
                        );
                    }
                    other => panic!("{what}: moved and returned unevenly: {other:?}"),
                }
            }
        }
        set_ui_font_size(&app, &mut vcx, 16.);
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// The native minimum-body capture: for each language and interface size
    /// the body, in logical pixels, the filled header has at the 720px window
    /// minimum with the sidebar and the right panel open (the same at 1x and
    /// 2x). Seeded from the capture, not computed. Re-seeded on the tree with
    /// the right panel's tab-count slot (B3): the panel's floor grew with the
    /// interface size and the language, so five of the eight bodies fell (en
    /// 16, ja 16 and zh 16 did not). The capture is
    /// `.pr-assets/A4/b3/native/run/a4-b3-native-evidence.tgz`, the probe's
    /// `body` at the 720px window of each language and size.
    const NATIVE_MINIMA: [(&str, f32, f32); 8] = [
        ("en", 16., 300.),
        ("ja-JP", 16., 300.),
        ("ru-RU", 16., 235.),
        ("zh-CN", 16., 300.),
        ("en", 24., 225.),
        ("ja-JP", 24., 226.),
        ("ru-RU", 24., 117.),
        ("zh-CN", 24., 263.),
    ];

    /// The order the parts read in once the header has more rows than the
    /// ladder gives it: what the diff is of, the totals, the two switches, the
    /// chip, the Refreshing word, the terminal's name, the summary.
    const OVERFLOW_ORDER: [(&str, &str); 9] = [
        ("subject icon", "diff-overlay-icon"),
        ("added figure", "diff-overlay-added"),
        ("removed figure", "diff-overlay-removed"),
        ("view switch", "diff-overlay-view"),
        ("layout switch", "document-layout-switch"),
        ("Staged chip", "ui-chip"),
        ("Refreshing word", "diff-overlay-refreshing"),
        ("terminal name", "document-terminal-name"),
        ("summary", "diff-overlay-summary"),
    ];

    /// The summary's words, as the overflow rows draw them: one element each.
    const SUMMARY_WORDS: [&str; 16] = [
        "diff-overlay-summary-word-0",
        "diff-overlay-summary-word-1",
        "diff-overlay-summary-word-2",
        "diff-overlay-summary-word-3",
        "diff-overlay-summary-word-4",
        "diff-overlay-summary-word-5",
        "diff-overlay-summary-word-6",
        "diff-overlay-summary-word-7",
        "diff-overlay-summary-word-8",
        "diff-overlay-summary-word-9",
        "diff-overlay-summary-word-10",
        "diff-overlay-summary-word-11",
        "diff-overlay-summary-word-12",
        "diff-overlay-summary-word-13",
        "diff-overlay-summary-word-14",
        "diff-overlay-summary-word-15",
    ];

    /// The height of a row after the first, which the filled header's second
    /// line has always been (`SECOND_LINE_H`).
    const EXTRA_ROW_H: f32 = 36.;

    /// The language and the interface size for a native context.
    fn set_native_context(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        locale: &str,
        size: f32,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        set_ui_font_size(app, vcx, size);
        crate::ui::i18n::set_locale(locale);
        let agents = crate::ui::i18n::t(crate::ui::i18n::L10nKey::VisionAgents).to_string();
        use_locale(app, vcx, locale, &agents);
    }

    /// The window width at which the painted body is `body`, the sidebar and
    /// the panel open: the panels' drawn widths at the 720px minimum, which
    /// is where both sit on their floors. A fixture: the test backend's panel
    /// floors differ from the native ones, so the window is not always 720.
    fn window_for_body(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, body: f32) -> f32 {
        resize(vcx, 720., 520.);
        let (sidebar, panel) = vcx.update(|window, cx| {
            let app = app.read(cx);
            (app.sidebar_px(window, cx), app.right_panel_px(window, cx))
        });
        body + sidebar + panel
    }

    /// The painted body's width.
    fn painted_body(vcx: &mut VisualTestContext, what: &str) -> f32 {
        f32::from(
            vcx.debug_bounds("diff-overlay-body")
                .unwrap_or_else(|| panic!("{what}: the overlay's body is not drawn"))
                .size
                .width,
        )
    }

    /// The distinct lines the header's parts are on, top to bottom, from
    /// painted bounds: centres within 4px are one line.
    fn header_bands(vcx: &mut VisualTestContext) -> Vec<f32> {
        let mut centres: Vec<f32> = NATIVE_NODES
            .iter()
            .filter_map(|(_, selector)| vcx.debug_bounds(selector))
            .map(|field| f32::from(field.center().y))
            .collect();
        centres.sort_by(|a, b| a.total_cmp(b));
        let mut bands: Vec<f32> = Vec::new();
        for y in centres {
            if !bands.last().is_some_and(|last| y - *last <= 4.) {
                bands.push(y);
            }
        }
        bands
    }

    /// Every part of the header the fixture has is drawn, at least a pixel
    /// wide and tall, and inside the body the shell paints, header included.
    fn assert_inside_body(vcx: &mut VisualTestContext, wide: &Wide, what: &str) {
        let body = vcx
            .debug_bounds("diff-overlay-body")
            .unwrap_or_else(|| panic!("{what}: the overlay's body is not drawn"));
        let widths = diff_widths(vcx);
        let eps = px(0.5);
        let inside = |field: Bounds<Pixels>| {
            field.left() >= body.left() - eps
                && field.right() <= body.right() + eps
                && field.top() >= body.top() - eps
                && field.bottom() <= body.bottom() + eps
        };
        let header = diff_header(vcx, what).header;
        assert!(
            inside(header),
            "{what}: the header {header:?} is outside the painted body {body:?}; {widths}"
        );
        for (name, selector) in wide.nodes() {
            let field = vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: the {name} ({selector}) is not drawn"));
            assert!(
                field.size.width >= px(1.) && field.size.height >= px(1.),
                "{what}: the {name} {field:?} is hidden"
            );
            assert!(
                inside(field),
                "{what}: the {name} {field:?} is outside the painted body {body:?}; {widths}"
            );
        }
    }

    /// No row is empty: the header is as tall as its first row and one extra
    /// row for each other line its parts are on.
    fn assert_rows_tight(vcx: &mut VisualTestContext, one_line: Pixels, what: &str) -> usize {
        let rows = header_bands(vcx).len();
        let height = f32::from(diff_header(vcx, what).header.size.height);
        let expected = f32::from(one_line) + (rows as f32 - 1.) * EXTRA_ROW_H;
        assert!(
            (height - expected).abs() <= 0.5,
            "{what}: {rows} line(s) of parts and a header {height}px tall, not {expected}px: \
             a row is empty or missing"
        );
        rows
    }

    /// The summary as the header says it for a diff of `files` files and
    /// `untracked` untracked ones.
    fn summary_text(files: usize, untracked: usize) -> String {
        let mut text =
            crate::ui::i18n::t_plural(crate::ui::i18n::L10nKey::DiffChangedFiles, files, &[]);
        if untracked > 0 {
            text.push_str(&crate::ui::i18n::t_plural(
                crate::ui::i18n::L10nKey::DiffUntrackedCount,
                untracked,
                &[],
            ));
        }
        text
    }

    /// How many lines `text` takes in `width` pixels broken only between
    /// words, each word shaped by the real text system as the summary draws
    /// it and rounded up the way a drawn run is (every word is an element of
    /// its own, so each rounds up on its own, not the paragraph as a whole),
    /// the gap between words the raw width of a space. A word wider than `width` on its own is a capped word (ellipsis and
    /// a tooltip) on a line of its own: the words before it end their line, the
    /// words after it begin a new one.
    fn words_in(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        text: &str,
        width: f32,
    ) -> usize {
        let meta = crate::ui::right_panel::META;
        let space = shaped_ui(app, vcx, " ", meta, true);
        let (mut lines, mut used) = (0, 0.);
        for word in text.split_whitespace() {
            let w = shaped_ui(app, vcx, word, meta, true).ceil();
            if w > width + 0.01 {
                lines += 1;
                used = 0.;
            } else if used == 0. {
                lines += 1;
                used = w;
            } else if used + space + w <= width + 0.01 {
                used += space + w;
            } else {
                lines += 1;
                used = w;
            }
        }
        lines.max(1)
    }

    /// The header with more rows than the ladder gives it: row 1 is the
    /// terminal's badge group and the close tile at the trailing end and
    /// nothing else; every other part is whole, in order, on the rows below,
    /// nothing hidden or past the body. The terminal's name keeps its
    /// single-line ellipsis and tooltip, never under its minimum. The summary
    /// is prose on a row of its own: every word is visible and broken only
    /// between words, with no ellipsis unless one word is wider than the body
    /// by itself; its row is as tall as its lines need, which the header's own
    /// height has room for.
    fn assert_overflow_rows(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        wide: &Wide,
        whole: &Whole,
        one_line: Pixels,
        summary: &str,
        what: &str,
    ) {
        assert_inside_body(vcx, wide, what);
        let report = format!(
            "{}; painted widths: {}",
            wide.report(painted_body(vcx, what)),
            diff_widths(vcx)
        );
        // Every control, figure and word is whole, but for two: the Refreshing
        // word, where no row can hold it whole (it is wider than a row's usable
        // width): it takes that width and is cut, the way a summary word wider
        // than its row is; and the Staged chip, which gives up exactly what it
        // lacks of its padding, at most 3px, its word never trimmed. Nothing
        // else gets an exception.
        let (mut painted, mut natural) = (diff_whole(vcx, what), *whole);
        let usable = (painted_body(vcx, what) - wide.lead - wide.trail).floor();
        if let (Some(drawn), Some(whole_w)) = (painted.refreshing, whole.refreshing) {
            if f32::from(whole_w) > usable + 0.01 {
                assert!(
                    (f32::from(drawn) - usable).abs() <= 1.,
                    "{what}: the Refreshing word, {whole_w:?} whole, is {drawn:?} where no row \
                     holds it: it takes the {usable}px a row has; {report}"
                );
                painted.refreshing = None;
                natural.refreshing = None;
            }
        }
        if let (Some(drawn), Some(whole_w)) = (painted.chip, whole.chip) {
            let whole_w = f32::from(whole_w);
            if whole_w > usable + 0.01 {
                let wanted = whole_w - (whole_w - usable).clamp(0., 3.);
                assert!(
                    (f32::from(drawn) - wanted).abs() <= 0.5,
                    "{what}: the Staged chip, {whole_w}px whole, is {drawn:?} in a {usable}px \
                     row: it gives up exactly the deficit of its padding, at most 3px, so \
                     {wanted}px; {report}"
                );
                painted.chip = None;
                natural.chip = None;
            }
        }
        assert_eq!(
            painted, natural,
            "{what}: a control, figure or word is squeezed or gone; {report}"
        );
        let bands = header_bands(vcx);
        let rows = bands.len();
        assert!(
            rows >= 3,
            "{what}: {rows} row(s), where the ladder cannot fit; {report}"
        );
        let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        let body = bounds(vcx, "diff-overlay-body");
        let nodes = wide.nodes();
        let order_items = wide.order();
        let mut first: Vec<&str> = nodes
            .iter()
            .filter(|(_, selector)| bounds(vcx, selector).center().y <= px(bands[0] + 4.))
            .map(|(name, _)| *name)
            .collect();
        first.sort_unstable();
        assert_eq!(
            first,
            ["close", "terminal button"],
            "{what}: the first row is the terminal button and the close tile only; {report}"
        );
        let (group, close) = (
            bounds(vcx, "document-terminal-group"),
            bounds(vcx, "diff-overlay-close"),
        );
        assert!(
            (f32::from(group.left() - body.left()) - wide.lead).abs() <= 0.5
                && (f32::from(body.right() - close.right()) - wide.trail).abs() <= 0.5,
            "{what}: the badge group {group:?} and the close tile {close:?} are not at the \
             leading and trailing ends of the body {body:?}; {report}"
        );
        // The rest, read row by row and left to right, in order.
        let mut seq: Vec<(usize, f32, &str)> = order_items
            .iter()
            .map(|(name, selector)| {
                let field = bounds(vcx, selector);
                let y = f32::from(field.center().y);
                let band = bands
                    .iter()
                    .position(|band| (band - y).abs() <= 4.)
                    .unwrap_or_else(|| panic!("{what}: the {name} is on no line"));
                (band, f32::from(field.left()), *name)
            })
            .collect();
        seq.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let order: Vec<&str> = seq.iter().map(|(_, _, name)| *name).collect();
        let wanted: Vec<&str> = order_items.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            order, wanted,
            "{what}: the parts are not in order; {report}"
        );
        assert!(
            seq.iter().all(|(band, _, _)| *band >= 1),
            "{what}: a part is on the first row; {report}"
        );
        // The terminal's name: one line, under its cap, over its minimum.
        let name = bounds(vcx, "document-terminal-name");
        let name_kept = wide
            .movable
            .iter()
            .find(|(label, _, _)| *label == "terminal name")
            .map(|(_, _, kept)| *kept)
            .unwrap_or_else(|| panic!("{what}: no name was measured with room"));
        assert!(
            f32::from(name.size.width) >= name_kept - 0.01
                && f32::from(name.size.width)
                    <= crate::ui::document_column::HEADER_CELL_MAX_W + 0.5
                && f32::from(name.size.height) <= EXTRA_ROW_H,
            "{what}: the name {name:?} is not one line of {name_kept}px to its cap; {report}"
        );
        // The summary: on a row of its own, as tall as its lines.
        let summary_box = bounds(vcx, "diff-overlay-summary");
        let summary_band = bands
            .iter()
            .position(|band| (band - f32::from(summary_box.center().y)).abs() <= 4.)
            .unwrap_or_else(|| panic!("{what}: the summary is on no line"));
        assert!(
            seq.iter()
                .all(|(band, _, label)| *label == "summary" || *band != summary_band),
            "{what}: the summary shares its row with another part; {report}"
        );
        let lines = words_in(app, vcx, summary, f32::from(summary_box.size.width));
        let line_h = wide.summary_line_h;
        let header_h = f32::from(diff_header(vcx, what).header.size.height);
        let summary_row_h = header_h - f32::from(one_line) - (rows as f32 - 2.) * EXTRA_ROW_H;
        let word_boxes: Vec<String> = SUMMARY_WORDS
            .iter()
            .enumerate()
            .filter_map(|(i, selector)| {
                vcx.debug_bounds(selector)
                    .map(|word| format!("word {i}: {word:?}"))
            })
            .collect();
        assert!(
            (f32::from(summary_box.size.height) - lines as f32 * line_h).abs() <= 1.,
            "{what}: the summary {summary_box:?} is {}px tall; {lines} whole-word line(s) of \
             {line_h}px need {}px, so a word is cut or the text is not wrapped; the words are \
             {}; the header is {header_h}px over a {one_line:?} first row and {} other rows of \
             {EXTRA_ROW_H}px, leaving {summary_row_h}px for the summary's row; {report}",
            summary_box.size.height,
            lines as f32 * line_h,
            word_boxes.join("; "),
            rows - 2
        );
        // The header is as tall as its first row, a standard extra row for
        // each other row, and the summary's row: which holds the summary and,
        // for a single line, is the standard row.
        let header = f32::from(diff_header(vcx, what).header.size.height);
        let summary_row = header - f32::from(one_line) - (rows as f32 - 2.) * EXTRA_ROW_H;
        let summary_h = f32::from(summary_box.size.height);
        assert!(
            summary_row >= summary_h - 0.5
                && summary_row <= summary_h.max(EXTRA_ROW_H) + EXTRA_ROW_H
                && (lines > 1 || (summary_row - EXTRA_ROW_H).abs() <= 0.5),
            "{what}: {rows} rows and a {header}px header leave {summary_row}px for the summary's \
             row, which holds a {summary_h}px summary of {lines} line(s); {report}"
        );
        // Nothing is painted over anything else.
        let all: Vec<(&str, Bounds<Pixels>)> = nodes
            .iter()
            .map(|(name, selector)| (*name, bounds(vcx, selector)))
            .collect();
        for (i, (a_name, a)) in all.iter().enumerate() {
            for (b_name, b) in &all[i + 1..] {
                let name_in_button = [*a_name, *b_name].contains(&"terminal name")
                    && [*a_name, *b_name].contains(&"terminal button");
                assert!(
                    name_in_button || !overlaps(*a, *b),
                    "{what}: the {a_name} {a:?} is painted over the {b_name} {b:?}; {report}"
                );
            }
        }
    }

    /// The header in the window as it is, against the body it is painted in.
    /// Every part is inside that body and no row is empty whatever the width.
    /// Where the painted minima say no ladder level fits both lines (the
    /// least any level needs, the last with its switches' margins at nothing,
    /// exceeds the body by more than 3px, a band for the measurements' own
    /// rounding) the rows are the overflow rows; where they say one does (by
    /// more than 3px) the lines are the contract of [`assert_filled_lines`].
    #[allow(clippy::too_many_arguments)]
    fn assert_contract_at(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        wide: &Wide,
        whole: &Whole,
        one_line: Pixels,
        window_w: f32,
        summary: &str,
        what: &str,
    ) {
        let body = painted_body(vcx, what);
        let threshold = wide.ladder_threshold();
        if threshold - 3. > body {
            assert_overflow_rows(app, vcx, wide, whole, one_line, summary, what);
        } else if threshold + 3. <= body {
            assert_inside_body(vcx, wide, what);
            let lines = header_lines(vcx, what);
            assert_filled_lines(vcx, whole, window_w, one_line, lines, what);
        } else {
            assert_inside_body(vcx, wide, what);
            assert_rows_tight(vcx, one_line, what);
        }
    }

    /// At the 720px window minimum, in every native context (each language at
    /// each interface size) the filled Staged, reading header with the native
    /// capture's 50000 and 50000 keeps every part and every control inside the
    /// painted body: as the test backend lays the window out, and with the
    /// window solved so that the painted body is exactly the native minimum
    /// body (the 157px Russian one at the largest size among them).
    #[gpui::test]
    fn at_the_native_minimum_bodies_every_part_and_control_is_inside_the_body(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        for (locale, size, native) in NATIVE_MINIMA {
            set_native_context(&app, &mut vcx, locale, size);
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
            let state = format!("{locale} UI{size} Staged reading, native 50000/50000");
            assert_holds(&app, &mut vcx, &held, &state);
            let whole = diff_whole(&mut vcx, &state);
            let one_line = diff_header(&mut vcx, &state).header.size.height;
            let wide = measure_wide(&app, &mut vcx, &state, true);
            let summary = summary_text(1, 0);

            // The window as it is at its minimum, the test backend's panels.
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 720., 520.);
            let what = format!("{state}, the 720px window as laid out");
            assert_holds(&app, &mut vcx, &held, &what);
            assert_contract_at(
                &app, &mut vcx, &wide, &whole, one_line, 720., &summary, &what,
            );

            // The window that paints the native minimum body.
            let width = window_for_body(&app, &mut vcx, native);
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, width, 520.);
            let what = format!("{state}, the native {native}px body in a {width}px window");
            assert_holds(&app, &mut vcx, &held, &what);
            let body = painted_body(&mut vcx, &what);
            assert!(
                (body - native).abs() <= 0.5,
                "{what}: the fixture's window paints a {body}px body, not the native {native}px"
            );
            assert_contract_at(
                &app, &mut vcx, &wide, &whole, one_line, width, &summary, &what,
            );
        }
        set_native_context(&app, &mut vcx, "en", 16.);
    }

    /// Where no row can hold the Refreshing word whole (the body the panel's
    /// tab-count slot leaves at the 720px window, in Russian at the largest
    /// size: 122px in the test backend, 117px natively) the word, and only
    /// it, takes the width of the usable row and is cut there, the way a
    /// summary word wider than its row is: nothing of it is painted past the
    /// row's start or end, and hovering it shows the whole word. Where a row
    /// holds it (the 157px body) it is unchanged: whole, with no tooltip.
    /// The cut is the measured elision (End), which is what carries the whole
    /// word to assistive technology; the tooltip and the elision are checked here,
    /// the accessible name natively through AT-SPI.
    #[gpui::test]
    fn the_refreshing_word_takes_the_row_it_has_and_shows_its_whole_text_where_none_holds_it(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        set_native_context(&app, &mut vcx, "ru-RU", 24.);
        put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
        let state = "ru-RU UI24 Staged reading, native 50000/50000".to_string();
        assert_holds(&app, &mut vcx, &held, &state);
        let whole = diff_whole(&mut vcx, &state);
        let natural = f32::from(
            whole
                .refreshing
                .expect("a reading header has its Refreshing word"),
        );
        let one_line = diff_header(&mut vcx, &state).header.size.height;
        let wide = measure_wide(&app, &mut vcx, &state, true);
        let summary = summary_text(1, 0);
        let text = crate::ui::i18n::t(crate::ui::i18n::L10nKey::Refreshing).to_string();
        let eps = px(0.5);
        let lead = crate::ui::app::CONTENT_INSET;
        for target in [122., 117.] {
            let width = solve_window(&app, &mut vcx, target, 520., &state);
            let what = format!("{state}, a {target}px body in a {width}px window");
            assert_holds(&app, &mut vcx, &held, &what);
            let body = vcx
                .debug_bounds("diff-overlay-body")
                .unwrap_or_else(|| panic!("{what}: the body is not drawn"));
            let close = vcx
                .debug_bounds("diff-overlay-close")
                .unwrap_or_else(|| panic!("{what}: the close tile is not drawn"));
            let trail = f32::from(body.right() - close.right());
            let usable = (f32::from(body.size.width) - lead - trail).floor();
            assert!(
                natural > usable,
                "{what}: the premise is that no row holds the {natural}px word in {usable}px"
            );
            let word = vcx
                .debug_bounds("diff-overlay-refreshing")
                .unwrap_or_else(|| panic!("{what}: the Refreshing word is not drawn"));
            assert!(
                (f32::from(word.size.width) - usable).abs() <= 1.
                    && word.left() >= body.left() + px(lead) - eps
                    && word.right() <= body.right() - px(trail) + eps,
                "{what}: the word {word:?} should be the {usable}px of the row, inside the \
                 {lead}px lead and the {trail}px trail of the body {body:?}; {}",
                diff_widths(&mut vcx)
            );
            // The cut is the UI's measured elision, End mode, and it is that label
            // that carries the whole word to assistive technology (as its value:
            // a real AT-SPI read showed a `Label` named only by `aria_label`
            // comes out empty), so it must be the thing drawn.
            crate::ui::text_elision::observe::clear();
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let shown = crate::ui::text_elision::observe::label(&gpui::ElementId::from(
                "diff-overlay-refreshing-text",
            ))
            .unwrap_or_else(|| panic!("{what}: the cut word is not an elided label"));
            assert!(
                shown.painted
                    && shown.text.ends_with('\u{2026}')
                    && shown.text.chars().next() == text.chars().next()
                    && shown.text.chars().count() < text.chars().count(),
                "{what}: the cut word {text:?} is not painted as its head and an ellipsis: {shown:?}"
            );
            let card = hover_tooltip(&mut vcx, "diff-overlay-refreshing")
                .unwrap_or_else(|| panic!("{what}: the cut word has no tooltip with its text"));
            let lines = tooltip_lines(&mut vcx, &text, f32::from(card.size.width));
            let drawn = (f32::from(card.size.height) / tooltip_line_h(&mut vcx)).round() as usize;
            assert!(
                lines == 1 && drawn == 1 && f32::from(card.size.width) + 1. >= natural,
                "{what}: the tooltip {card:?} does not show the whole word {text:?} on one line \
                 ({lines} shaped, {drawn} drawn)"
            );
            assert_contract_at(
                &app, &mut vcx, &wide, &whole, one_line, width, &summary, &what,
            );
        }
        // A body with room holds it: whole, unchanged, nothing to hover.
        let width = solve_window(&app, &mut vcx, 157., 520., &state);
        let what = format!("{state}, the 157px body in a {width}px window");
        assert_holds(&app, &mut vcx, &held, &what);
        let word = vcx
            .debug_bounds("diff-overlay-refreshing")
            .unwrap_or_else(|| panic!("{what}: the Refreshing word is not drawn"));
        assert!(
            (f32::from(word.size.width) - natural).abs() <= 0.5,
            "{what}: the word {word:?} is not its whole {natural}px where a row holds it"
        );
        assert!(
            hover_tooltip(&mut vcx, "diff-overlay-refreshing").is_none(),
            "{what}: a whole word has no tooltip"
        );
        crate::ui::text_elision::observe::clear();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        assert!(
            crate::ui::text_elision::observe::label(&gpui::ElementId::from(
                "diff-overlay-refreshing-text"
            ))
            .is_none(),
            "{what}: a whole word is plain text, not an elided label"
        );
        set_native_context(&app, &mut vcx, "en", 16.);
    }

    /// The Staged chip is a 90px word between two 5px paddings, 100px. Where
    /// the usable row (the body less the lead and the trail) is narrower, it
    /// gives up exactly the deficit, and at most 3px of it (the paddings go from
    /// 5px to 3.5px each), so its 90px word is never trimmed; where the row
    /// holds it, it is the 100px it always was. The padding is not an
    /// observable on its own, so the chip's painted width is: the whole word
    /// plus both paddings, none less than 3.5px.
    #[gpui::test]
    fn the_staged_chip_gives_up_only_its_padding_deficit_and_keeps_its_whole_word(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        set_native_context(&app, &mut vcx, "ru-RU", 24.);
        put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
        let state = "ru-RU UI24 Staged reading, native 50000/50000".to_string();
        assert_holds(&app, &mut vcx, &held, &state);
        let natural = f32::from(
            diff_whole(&mut vcx, &state)
                .chip
                .expect("a Staged header has its chip"),
        );
        let word = natural - 2. * crate::ui::right_panel::CHIP_PAD_X;
        let eps = px(0.5);
        let lead = crate::ui::app::CONTENT_INSET;
        for target in [157., 122., 117.] {
            let width = solve_window(&app, &mut vcx, target, 520., &state);
            let what = format!("{state}, a {target}px body in a {width}px window");
            assert_holds(&app, &mut vcx, &held, &what);
            let body = vcx
                .debug_bounds("diff-overlay-body")
                .unwrap_or_else(|| panic!("{what}: the body is not drawn"));
            let close = vcx
                .debug_bounds("diff-overlay-close")
                .unwrap_or_else(|| panic!("{what}: the close tile is not drawn"));
            let trail = f32::from(body.right() - close.right());
            let usable = (f32::from(body.size.width) - lead - trail).floor();
            let chip = vcx
                .debug_bounds("ui-chip")
                .unwrap_or_else(|| panic!("{what}: the chip is not drawn"));
            let wanted = natural - (natural - usable).clamp(0., 3.);
            assert!(
                (f32::from(chip.size.width) - wanted).abs() <= 0.5
                    && f32::from(chip.size.width) >= word + 2. * 3.5 - 0.5
                    && chip.left() >= body.left() + px(lead) - eps,
                "{what}: the chip {chip:?} should be {wanted}px (the {natural}px chip less the \
                 deficit against the {usable}px row, 3px at most, around its whole {word}px \
                 word); {}",
                diff_widths(&mut vcx)
            );
            if natural <= usable {
                assert!(
                    f32::from(chip.size.width) == natural
                        || (f32::from(chip.size.width) - natural).abs() <= 0.5,
                    "{what}: the chip {chip:?} changed where the row holds it whole"
                );
            }
        }
        set_native_context(&app, &mut vcx, "en", 16.);
    }

    /// The one ordered state of the filled header, from painted bounds: 0 for
    /// one row; 1 to 3 for the ladder's two rows with that many tiers on the
    /// second; then one more than the rows for each overflow row (4 for three
    /// rows). Narrowing only raises it and widening only lowers it.
    fn header_level(vcx: &mut VisualTestContext, what: &str) -> usize {
        match header_bands(vcx).len() {
            1 => 0,
            2 => tiers_moved(vcx, what),
            rows => rows + 1,
        }
    }

    /// The summary with room, on one line, is the ladder's single text element,
    /// as wide as the text system shapes the whole run (rounded up as a run is)
    /// with the interface font at the summary's size and the metrics' tabular
    /// features: what the lexicon's words and digits are measured in is what
    /// is drawn. The overflow rows' per-word elements are checked by
    /// [`words_in`]. The test backend's glyphs all advance the
    /// same, so this cannot tell tabular figures from proportional ones; a
    /// native capture does.
    fn assert_summary_features(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        summary: &str,
        what: &str,
    ) {
        let painted = f32::from(
            vcx.debug_bounds("diff-overlay-summary")
                .unwrap_or_else(|| panic!("{what}: the summary is not drawn"))
                .size
                .width,
        );
        // With room the summary is the ladder's: one text element, a single
        // run rounded up as a whole, whatever the overflow rows do with words.
        let shaped = shaped_ui(app, vcx, summary, crate::ui::right_panel::META, true).ceil();
        assert!(
            (painted - shaped).abs() <= 1.5,
            "{what}: the summary is painted {painted}px wide with room and shaped {shaped}px as \
             one run with the tabular features it is measured in: {summary:?}"
        );
    }

    /// How many lines the summary is drawn on, from its painted height over
    /// the painted height of one line.
    fn summary_lines(vcx: &mut VisualTestContext, wide: &Wide, what: &str) -> usize {
        let height = vcx
            .debug_bounds("diff-overlay-summary")
            .unwrap_or_else(|| panic!("{what}: the summary is not drawn"))
            .size
            .height;
        (f32::from(height) / wide.summary_line_h).round() as usize
    }

    /// Each value a component reached while narrowing is lost again while
    /// widening only `HEADER_HYSTERESIS` past the last width that held it,
    /// measured in the painted body (the panels' widths change with the window,
    /// so window widths are not comparable): the body at the last width that
    /// kept the value (`kept`) and at the first width that lost it (`up`).
    fn assert_margins(kept: &[Option<f32>], up: &[Option<f32>], first: usize, what: &str) {
        for (value, (kept, up)) in kept.iter().zip(up.iter()).enumerate().skip(first) {
            let what = format!("{what}, value {value}");
            match (kept, up) {
                (None, None) => {}
                (Some(kept), Some(up)) => {
                    assert!(
                        *up - kept >= HEADER_HYSTERESIS - 0.01
                            && *up - kept <= HEADER_HYSTERESIS + 1.,
                        "{what}: the last body that kept it was {kept}px, it was lost at a {up}px \
                         body: not the margin of {HEADER_HYSTERESIS}"
                    );
                }
                other => panic!("{what}: reached and lost unevenly: {other:?}"),
            }
        }
    }

    /// The other history for each value of a component: from just past the
    /// width it was lost at, narrowing holds it until the width it was first
    /// reached at, and not before; every part stays inside the body.
    fn assert_history_two(
        vcx: &mut VisualTestContext,
        wide: &Wide,
        down: &[Option<f32>],
        up: &[Option<f32>],
        observe: impl Fn(&mut VisualTestContext, &str) -> usize,
        what: &str,
    ) {
        for (value, (down, up)) in down.iter().zip(up.iter()).enumerate().skip(1) {
            let (Some(down), Some(up)) = (down, up) else {
                continue;
            };
            resize(vcx, 2400., 900.);
            resize(vcx, *up, 700.);
            let what = format!("{what}, value {value}, history two");
            assert!(
                observe(vcx, &what) < value,
                "{what}: reached at the width it was lost, {up}px"
            );
            let mut width = up - 1.;
            let regained = loop {
                resize(vcx, width, 700.);
                assert_inside_body(vcx, wide, &what);
                if observe(vcx, &what) >= value {
                    break width;
                }
                width -= 1.;
                assert!(width > down - 3., "{what}: never regained, below {down}px");
            };
            assert_eq!(
                regained, *down,
                "{what}: from inside the margin it must be regained where it was first reached"
            );
        }
    }

    /// Narrowing from 2400px to the native 157px body at the largest size in
    /// Russian, one point at a time, the header only ever grows: its state
    /// (rows and ladder tiers), the lines of its summary and its height each
    /// only rise; widening back each only falls, and each value either
    /// component reached is lost again only 8px past the last width that held
    /// it, and from there narrowing again does not regain it before the first
    /// width it was reached at, in a real render. Entering the overflow rows
    /// is at the least body some ladder level fits both lines in (the
    /// measured one, with the switches' margins at nothing), and leaving them
    /// is 8px past it, not past a restored margin. Every part is inside the
    /// body at every width.
    #[gpui::test]
    fn at_the_native_ui24_minimum_the_header_only_grows_while_narrowing_and_keeps_the_margin(
        cx: &mut TestAppContext,
    ) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, held, _streams) = native_ui24_diff(cx);
        put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
        let state = "UI24 ru-RU Staged reading, native 50000/50000";
        let whole = diff_whole(&mut vcx, state);
        let one_line = diff_header(&mut vcx, state).header.size.height;
        let wide = measure_wide(&app, &mut vcx, state, true);
        let summary = summary_text(1, 0);
        let narrowest = window_for_body(&app, &mut vcx, 157.);
        put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);

        let mut down: [Option<f32>; 16] = [None; 16];
        let mut down_lines: [Option<f32>; 16] = [None; 16];
        // The painted body at the last width that kept each value.
        let mut kept: [Option<f32>; 16] = [None; 16];
        let mut kept_lines: [Option<f32>; 16] = [None; 16];
        let (mut width, mut prev, mut prev_lines, mut prev_height) = (2400., 0, 0, 0.);
        let mut prev_body = painted_body(&mut vcx, state);
        while width >= narrowest {
            resize(&mut vcx, width, 700.);
            let what = format!("{state} narrowing at {width}px");
            assert_holds(&app, &mut vcx, &held, &what);
            assert_contract_at(
                &app, &mut vcx, &wide, &whole, one_line, width, &summary, &what,
            );
            let level = header_level(&mut vcx, &what);
            let lines = summary_lines(&mut vcx, &wide, &what);
            let height = f32::from(diff_header(&mut vcx, &what).header.size.height);
            let body = painted_body(&mut vcx, &what);
            assert!(
                level >= prev && lines >= prev_lines && height >= prev_height - 0.5,
                "{what}: state {level} after {prev}, summary lines {lines} after {prev_lines}, \
                 height {height} after {prev_height}: something shrank while narrowing"
            );
            down[prev + 1..=level]
                .iter_mut()
                .for_each(|at| *at = Some(width));
            down_lines[prev_lines + 1..=lines]
                .iter_mut()
                .for_each(|at| *at = Some(width));
            kept[prev + 1..=level]
                .iter_mut()
                .for_each(|at| *at = Some(prev_body));
            kept_lines[prev_lines + 1..=lines]
                .iter_mut()
                .for_each(|at| *at = Some(prev_body));
            (prev, prev_lines, prev_height, prev_body) = (level, lines, height, body);
            width -= 1.;
        }
        assert!(
            prev >= 4,
            "narrowing to the native 157px body the header never went past the ladder: state {prev}"
        );
        assert!(
            prev_lines >= 2,
            "narrowing to the native 157px body the summary never took a second line"
        );
        let mut up: [Option<f32>; 16] = [None; 16];
        let mut up_lines: [Option<f32>; 16] = [None; 16];
        let mut up_body: [Option<f32>; 16] = [None; 16];
        let mut up_body_lines: [Option<f32>; 16] = [None; 16];
        let mut width = narrowest;
        while width <= 2400. {
            resize(&mut vcx, width, 700.);
            let what = format!("{state} widening at {width}px");
            assert_inside_body(&mut vcx, &wide, &what);
            let level = header_level(&mut vcx, &what);
            let lines = summary_lines(&mut vcx, &wide, &what);
            let height = f32::from(diff_header(&mut vcx, &what).header.size.height);
            let body = painted_body(&mut vcx, &what);
            assert!(
                level <= prev && lines <= prev_lines && height <= prev_height + 0.5,
                "{what}: state {level} after {prev}, summary lines {lines} after {prev_lines}, \
                 height {height} after {prev_height}: something grew while widening"
            );
            up[level + 1..=prev]
                .iter_mut()
                .for_each(|at| *at = Some(width));
            up_lines[lines + 1..=prev_lines]
                .iter_mut()
                .for_each(|at| *at = Some(width));
            up_body[level + 1..=prev]
                .iter_mut()
                .for_each(|at| *at = Some(body));
            up_body_lines[lines + 1..=prev_lines]
                .iter_mut()
                .for_each(|at| *at = Some(body));
            (prev, prev_lines, prev_height) = (level, lines, height);
            width += 1.;
        }
        assert_margins(&kept, &up_body, 1, &format!("{state}, state"));
        assert_margins(
            &kept_lines,
            &up_body_lines,
            2,
            &format!("{state}, summary lines"),
        );

        // The overflow rows begin where the ladder stops fitting and end 8px
        // past it: against the painted body at the last width without them.
        let threshold = wide.ladder_threshold();
        let (Some(kept_body), Some(left_body)) = (kept[4], up_body[4]) else {
            panic!("{state}: the overflow rows were never entered and left");
        };
        assert!(
            (kept_body - threshold).abs() <= 3.,
            "{state}: the last painted body without the overflow rows is {kept_body}px; the least \
             a ladder level fits is {threshold}px: {}",
            wide.report(kept_body)
        );
        assert!(
            (left_body - threshold - HEADER_HYSTERESIS).abs() <= 4.,
            "{state}: the overflow rows ended at a {left_body}px body; that is {threshold}px plus \
             the margin of {HEADER_HYSTERESIS}, not a restored switch margin plus it ({}px)",
            threshold + wide.margins + HEADER_HYSTERESIS
        );

        assert_history_two(
            &mut vcx,
            &wide,
            &down,
            &up,
            |vcx, what| header_level(vcx, what),
            &format!("{state}, state"),
        );
        assert_history_two(
            &mut vcx,
            &wide,
            &down_lines,
            &up_lines,
            |vcx, what| summary_lines(vcx, &wide, what),
            &format!("{state}, summary lines"),
        );
        set_native_context(&app, &mut vcx, "en", 16.);
    }

    /// A Staged, reading diff over `files` files and `untracked` untracked
    /// ones, with the native capture's totals (50000 and 50000) all in the
    /// first file.
    fn open_staged_files(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        files: usize,
        untracked: usize,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        use crate::terminal::git_diff::{DiffSnapshot, DiffSource, FileDiff, FileStatus};
        let snapshot = std::sync::Arc::new(DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source: DiffSource::Staged,
            branch: "move-tab-keybind".into(),
            files: (0..files)
                .map(|n| FileDiff {
                    path: format!("a{n}.rs"),
                    old_path: None,
                    status: FileStatus::Modified,
                    added: if n == 0 { 50_000 } else { 0 },
                    removed: if n == 0 { 50_000 } else { 0 },
                    binary: false,
                    truncated: None,
                    hunks: Vec::new(),
                })
                .collect(),
            untracked: (0..untracked).map(|n| format!("u{n}.txt")).collect(),
            ..Default::default()
        });
        let held = std::sync::Arc::clone(&snapshot);
        app.update_in(vcx, |app, window, cx| {
            app.open_loaded_diff_for_test(snapshot, true, window, cx);
        });
        vcx.run_until_parked();
        assert_holds(
            app,
            vcx,
            &held,
            "a Staged diff over several files, freshly opened",
        );
        held
    }

    /// The summary follows its counts, in a real render at the native minimum
    /// window of the largest size (height 520) in all four languages: for
    /// the counts below, which exercise these plural forms of the changed
    /// files, in Russian zero, one (1, 21), few (2), many (5, 12) and in the
    /// other languages zero, one and other, and of the untracked ones, in
    /// Russian one, few and many and in the others one and other (the zero
    /// forms of the untracked count and Russian's `other` are not reachable
    /// from a diff). With no changed files there are no totals, which the
    /// fixture says. Each diff is a new one opened in the same window:
    /// nothing is keyed by the text, so a stale measurement would show. The
    /// summary's words are all visible on a row of its own, broken only
    /// between words, with no ellipsis but for a word wider than the body, and
    /// every part is inside the painted body, the header tall enough for the
    /// summary's lines.
    #[gpui::test]
    fn the_summary_follows_its_counts_at_the_native_ui24_minimum_bodies(cx: &mut TestAppContext) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _held, _streams) = native_ui24_diff(cx);
        for (locale, size, native) in NATIVE_MINIMA.iter().filter(|(_, size, _)| *size == 24.) {
            set_native_context(&app, &mut vcx, locale, *size);
            for (files, untracked) in [
                (1, 0),
                (2, 0),
                (5, 0),
                (12, 0),
                (21, 0),
                (0, 3),
                (0, 1),
                (1, 1),
                (2, 2),
                (5, 5),
                (25, 7),
            ] {
                let held = open_staged_files(&app, &mut vcx, files, untracked);
                let state =
                    format!("{locale} UI{size}, {files} file(s), {untracked} untracked, native");
                put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
                assert_holds(&app, &mut vcx, &held, &state);
                let whole = diff_whole(&mut vcx, &state);
                let one_line = diff_header(&mut vcx, &state).header.size.height;
                let wide = measure_wide(&app, &mut vcx, &state, files > 0);
                let summary = summary_text(files, untracked);
                assert_summary_features(&app, &mut vcx, &summary, &state);
                let width = window_for_body(&app, &mut vcx, *native);
                put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, width, 520.);
                let what = format!("{state} {native}px body in a {width}px window");
                assert_holds(&app, &mut vcx, &held, &what);
                assert_contract_at(
                    &app, &mut vcx, &wide, &whole, one_line, width, &summary, &what,
                );
            }

            // One untracked file and then 8888, in the same window at the same
            // painted body, the language and the font unchanged: a measurement
            // that kept the first count would show.
            let (few, many) = ((1, 1), (1, 8888));
            let state = format!("{locale} UI{size}, untracked 1 then 8888, native");
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 2400., 900.);
            let mut measured = Vec::new();
            for (files, untracked) in [few, many] {
                let held = open_staged_files(&app, &mut vcx, files, untracked);
                assert_holds(&app, &mut vcx, &held, &state);
                let summary = summary_text(files, untracked);
                assert_summary_features(&app, &mut vcx, &summary, &state);
                measured.push((
                    diff_whole(&mut vcx, &state),
                    diff_header(&mut vcx, &state).header.size.height,
                    measure_wide(&app, &mut vcx, &state, true),
                    summary,
                ));
            }
            let width = window_for_body(&app, &mut vcx, *native);
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, width, 520.);
            let mut bodies = Vec::new();
            for ((files, untracked), (whole, one_line, wide, summary)) in
                [few, many].into_iter().zip(&measured)
            {
                let held = open_staged_files(&app, &mut vcx, files, untracked);
                let what = format!("{state}, {untracked} untracked at {native}px");
                assert_holds(&app, &mut vcx, &held, &what);
                assert_contract_at(
                    &app, &mut vcx, wide, whole, *one_line, width, summary, &what,
                );
                // Each word is an element of its own, and every word's box is
                // at least the independently shaped word, rounded up: so a
                // count priced under its digits cannot pass because both
                // summaries still wrap onto the same number of lines. A word
                // wider than the row is the one capped word, left out.
                let row = f32::from(
                    vcx.debug_bounds("diff-overlay-summary")
                        .unwrap_or_else(|| panic!("{what}: the summary is not drawn"))
                        .size
                        .width,
                );
                for (i, token) in summary.split_whitespace().enumerate() {
                    let shaped =
                        shaped_ui(&app, &mut vcx, token, crate::ui::right_panel::META, true).ceil();
                    if shaped > row {
                        continue;
                    }
                    let word = vcx
                        .debug_bounds(SUMMARY_WORDS[i])
                        .unwrap_or_else(|| panic!("{what}: word {i} {token:?} is not drawn"));
                    assert!(
                        f32::from(word.size.width) >= shaped - 0.01,
                        "{what}: word {i} {token:?} is a {:?} box, under its shaped {shaped}px",
                        word.size.width
                    );
                }
                bodies.push(painted_body(&mut vcx, &what));
            }
            assert_eq!(
                bodies[0], bodies[1],
                "{state}: the two counts were not drawn in the same painted body"
            );
        }
        set_native_context(&app, &mut vcx, "en", 16.);
    }

    /// `text` shaped as the focused file's path is drawn: the editor font, normal
    /// weight, the mono meta size and the features the header inherits, by the
    /// real text system.
    fn shaped_path(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, text: &str) -> f32 {
        let (family, configured) = app.read_with(vcx, |app, _| {
            (app.font_family.clone(), app.font_features.clone())
        });
        let features = match configured.filter(|f| !f.0.is_empty()) {
            Some(configured) => configured,
            None => gpui::FontFeatures(std::sync::Arc::new(vec![
                ("calt".to_string(), 0),
                ("liga".to_string(), 0),
                ("clig".to_string(), 0),
            ])),
        };
        vcx.update(|window, _| {
            let font = gpui::Font {
                family: family.into(),
                features,
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            };
            let size = window.rem_size().as_f32() * crate::ui::right_panel::META_MONO;
            crate::ui::tab_strip::measure_text(window.text_system(), &font, size, text)
        })
    }

    /// The least the commit's label, its byline and the focused path keep, each
    /// shaped by the real text system in the face it is drawn in: three digits
    /// and an ellipsis, and for the path a head, an ellipsis and a tail.
    #[derive(Clone, Copy)]
    struct CommitMinima {
        label: f32,
        byline: f32,
        path: f32,
    }

    /// Where a commit's header is drawn at the largest interface size, as the
    /// scenes of the native captures: the diff filling the workspace at 1440px
    /// with the panel open, the saved column falling back at 1000px (a body of
    /// 437px), and the native 157px minimum body.
    #[derive(Clone, Copy)]
    enum CommitScene {
        Wide1440,
        Body437,
        Body157,
        /// The test backend's 720px window with the panel's tab-count slot
        /// (B3): a 122px body, whose usable row (less the lead and the trail)
        /// is 102px, under the ref's icon, gap and short SHA (119px).
        Body122,
        /// The native 720px minimum with that slot at the largest size in
        /// Russian: a 117px body, a 97px usable row.
        Body117,
        /// The native 1000px window with the diff's saved Dock falling back
        /// (sidebar 180, panel 423): a 397px body.
        Body397,
        /// The native 1000px window in the other state, the Fill latch on
        /// (sidebar 217, panel 423): a 360px body. A different state from the
        /// saved-Dock fallback above, never a replacement for it.
        Body360,
    }

    /// The commit's parts as painted: every one the header draws is whole where
    /// it has to be and inside the painted body, none painted over another. The
    /// short ref is drawn whole wherever its own room (the icon, a gap and the
    /// ref) fits the body, which it does in every scene here, whatever the
    /// path or the label: `natural` is its width with room. The focused file's
    /// back control keeps its chevron part; the commit's label, in the overflow
    /// rows (where it has a selector, and `require_label` says it must), is at
    /// least a readable minimum wide, so it can be seen and hovered.
    #[allow(clippy::too_many_arguments)]
    fn assert_commit_fits(
        vcx: &mut VisualTestContext,
        focused: bool,
        labelled: bool,
        natural: Pixels,
        minima: CommitMinima,
        require: bool,
        what: &str,
    ) {
        let body = vcx
            .debug_bounds("diff-overlay-body")
            .unwrap_or_else(|| panic!("{what}: the overlay's body is not drawn"));
        let widths = diff_widths(vcx);
        let mut nodes: Vec<(&str, &str)> = vec![
            ("subject icon", "diff-overlay-icon"),
            ("ref", "diff-overlay-rev"),
            ("terminal group", "document-terminal-group"),
            ("terminal name", "document-terminal-name"),
            ("view switch", "diff-overlay-view"),
            ("layout switch", "document-layout-switch"),
            ("close", "diff-overlay-close"),
        ];
        if focused {
            nodes.push(("back button", "diff-overlay-unfocus"));
        }
        let eps = px(0.5);
        let mut found: Vec<(&str, Bounds<Pixels>)> = Vec::new();
        for (name, selector) in &nodes {
            let field = vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: the {name} ({selector}) is not drawn"));
            assert!(
                field.size.width >= px(1.) && field.size.height >= px(1.),
                "{what}: the {name} {field:?} is hidden; {widths}"
            );
            assert!(
                field.left() >= body.left() - eps
                    && field.right() <= body.right() + eps
                    && field.top() >= body.top() - eps
                    && field.bottom() <= body.bottom() + eps,
                "{what}: the {name} {field:?} is outside the painted body {body:?}; {widths}"
            );
            found.push((*name, field));
        }
        let rev = found.iter().find(|(name, _)| *name == "ref").unwrap().1;
        assert!(
            rev.size.width + eps >= natural,
            "{what}: the ref is {:?} wide, not its {natural:?} with room; {widths}",
            rev.size.width
        );
        // The ref is its icon, a gap and its short SHA as one piece where the
        // usable row (the body less the lead and the trail) holds all three:
        // the icon and the SHA on one line, the gap between them. Where it
        // does not, the icon is alone on a row before and the SHA, whole, is
        // the first thing on the next, at the row's own start and inside its
        // end: nothing is cut, and nothing is dropped.
        let icon = found
            .iter()
            .find(|(name, _)| *name == "subject icon")
            .unwrap()
            .1;
        let close = found.iter().find(|(name, _)| *name == "close").unwrap().1;
        let lead = crate::ui::app::CONTENT_INSET;
        let trail = f32::from(body.right() - close.right());
        let usable = f32::from(body.size.width) - lead - trail;
        let gap = 0.5 * vcx.update(|window, _| window.rem_size().as_f32());
        let atom = f32::from(icon.size.width) + gap + f32::from(natural);
        if atom <= usable + 0.01 {
            assert!(
                (f32::from(icon.center().y) - f32::from(rev.center().y)).abs() <= 4.
                    && (f32::from(rev.left() - icon.right()) - gap).abs() <= 0.5,
                "{what}: the usable row is {usable}px, holding the {atom}px icon, gap and SHA, \
                 but the icon {icon:?} and the ref {rev:?} are not one piece on one line; \
                 {widths}"
            );
        } else {
            assert!(
                icon.bottom() <= rev.top() + eps
                    && (f32::from(icon.left() - body.left()) - lead).abs() <= 0.5
                    && (f32::from(rev.left() - body.left()) - lead).abs() <= 0.5
                    && rev.right() <= body.right() - px(trail) + eps,
                "{what}: the usable row is {usable}px, under the {atom}px icon, gap and SHA: \
                 the icon {icon:?} should be alone on the row before and the whole SHA {rev:?} \
                 first on the next, inside the {lead}px lead and the {trail}px trail of the \
                 body {body:?}; {widths}"
            );
        }
        if focused {
            let back = found
                .iter()
                .find(|(name, _)| *name == "back button")
                .unwrap()
                .1;
            let chevron_part = 4. + 0.875 * 24.;
            assert!(
                f32::from(back.size.width) >= chevron_part,
                "{what}: the back button {back:?} is under its {chevron_part}px chevron part; \
                 {widths}"
            );
        }
        // The overflow rows' pieces, each at no less than its readable
        // minimum and inside the body: where they exist (they have selectors
        // in the overflow rows only), and where `require` says they must.
        // The way back stays in the usable row, whatever its path: its chevron
        // part is whole, and the path gives way inside it.
        if focused {
            let back = found
                .iter()
                .find(|(name, _)| *name == "back button")
                .unwrap()
                .1;
            assert!(
                back.left() >= body.left() + px(lead) - eps
                    && back.right() <= body.right() - px(trail) + eps,
                "{what}: the back button {back:?} is outside the usable row of {usable}px, the \
                 body {body:?} less its {lead}px lead and {trail}px trail; {widths}"
            );
        }
        for (name, selector, least, wanted) in [
            (
                "commit label",
                "diff-overlay-label-subject",
                minima.label,
                labelled,
            ),
            (
                "commit byline",
                "diff-overlay-label-byline",
                minima.byline,
                labelled,
            ),
            ("path tail", "diff-overlay-path", minima.path, focused),
        ] {
            match vcx.debug_bounds(selector) {
                Some(piece) => {
                    // The path's readable minimum is kept wherever the way back
                    // holds it in the usable row; where it cannot, the path
                    // gives up only what is lacking, down to what the row
                    // leaves after the back button's chevron part.
                    let least = if name == "path tail" {
                        let back = found
                            .iter()
                            .find(|(owner, _)| *owner == "back button")
                            .unwrap_or_else(|| panic!("{what}: a path without its back button"))
                            .1;
                        let chrome = f32::from(back.size.width - piece.size.width);
                        if chrome + least <= usable + 0.01 {
                            least
                        } else {
                            (usable - chrome).max(0.)
                        }
                    } else {
                        least
                    };
                    assert!(
                        f32::from(piece.size.width) >= least - 0.01
                            && piece.size.height >= px(1.)
                            && piece.left() >= body.left() - eps
                            && piece.right() <= body.right() + eps
                            && piece.top() >= body.top() - eps
                            && piece.bottom() <= body.bottom() + eps,
                        "{what}: the {name} {piece:?} is under its readable minimum, {least}px, \
                         hidden, or outside the body {body:?}; {widths}"
                    );
                    // The path tail is inside its own back button.
                    if name == "path tail" {
                        let back = found
                            .iter()
                            .find(|(owner, _)| *owner == "back button")
                            .unwrap_or_else(|| panic!("{what}: a path without its back button"))
                            .1;
                        assert!(
                            piece.left() >= back.left() - eps
                                && piece.right() <= back.right() + eps
                                && piece.top() >= back.top() - eps
                                && piece.bottom() <= back.bottom() + eps,
                            "{what}: the path tail {piece:?} is outside its back button \
                             {back:?}; {widths}"
                        );
                    }
                    found.push((name, piece));
                }
                None => assert!(
                    !(require && wanted),
                    "{what}: the {name} is not drawn where the overflow rows must show it; \
                     {widths}"
                ),
            }
        }
        for (i, (a_name, a)) in found.iter().enumerate() {
            for (b_name, b) in &found[i + 1..] {
                let name_in_button = [*a_name, *b_name].contains(&"terminal name")
                    && [*a_name, *b_name].contains(&"terminal group");
                let path_in_back = [*a_name, *b_name].contains(&"path tail")
                    && [*a_name, *b_name].contains(&"back button");
                assert!(
                    name_in_button || path_in_back || !overlaps(*a, *b),
                    "{what}: the {a_name} {a:?} is painted over the {b_name} {b:?}; {widths}"
                );
            }
        }
    }

    /// The window width at which the diff filling the workspace, the panels
    /// open, paints a body of `target` pixels, found by reading the painted
    /// body and correcting the width by what it is off by: the panels' widths
    /// change with the window (the test backend's are not the native ones), so
    /// no constant offset does it. The window is left at that width.
    fn solve_window(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        target: f32,
        height: f32,
        what: &str,
    ) -> f32 {
        let mut width = target + 560.;
        for _ in 0..16 {
            put_in_scene(app, vcx, DocumentLayout::Fill, true, width, height);
            let painted = painted_body(vcx, what);
            if (painted - target).abs() <= 0.5 {
                return width;
            }
            width += target - painted;
        }
        panic!("{what}: no window paints a {target}px body");
    }

    /// One commit header case at the largest interface size in Russian: the
    /// commit (with its label, or bare), plain or opened on `path`, drawn in
    /// each of `scenes`, the short ref whole at its natural width in every one
    /// and every part of the header inside the painted body.
    #[allow(clippy::too_many_arguments)]
    fn check_commit_case(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        subject: &DiffRef,
        focused: bool,
        path: &str,
        scenes: &[CommitScene],
        minima: CommitMinima,
        checks: Checks,
    ) {
        let kind = if subject.labelled() {
            "commit"
        } else {
            "bare ref"
        };
        let held = if focused {
            open_focused_commit(app, vcx, subject, path)
        } else {
            open_diff(app, vcx, subject)
        };
        let state = format!(
            "ru-RU UI24 {kind}{}",
            if focused {
                format!(" focused on {path}")
            } else {
                String::new()
            }
        );
        put_in_scene(app, vcx, DocumentLayout::Fill, false, 2400., 900.);
        assert_holds(app, vcx, &held, &state);
        let natural = diff_header(vcx, &state)
            .rev
            .expect("a revision's subject is drawn")
            .size
            .width;
        let shaped = shaped_rev(app, vcx, &subject.drawn());
        assert!(
            (f32::from(natural) - shaped).abs() <= 2.,
            "{state}: the ref is {natural:?} with room, {shaped}px shaped"
        );
        for scene in scenes {
            // The native bodies are solved for, and asserted, in the window that
            // paints them.
            let (width, painted, label_needed) = match scene {
                CommitScene::Wide1440 => {
                    put_in_scene(app, vcx, DocumentLayout::Fill, true, 1440., 900.);
                    (1440., painted_body(vcx, &state), false)
                }
                CommitScene::Body437 => {
                    let width = solve_window(app, vcx, 437., 700., &state);
                    (width, painted_body(vcx, &state), false)
                }
                CommitScene::Body157 => {
                    let width = solve_window(app, vcx, 157., 520., &state);
                    (width, painted_body(vcx, &state), true)
                }
                CommitScene::Body122 => {
                    let width = solve_window(app, vcx, 122., 520., &state);
                    (width, painted_body(vcx, &state), true)
                }
                CommitScene::Body117 => {
                    let width = solve_window(app, vcx, 117., 520., &state);
                    (width, painted_body(vcx, &state), true)
                }
                CommitScene::Body397 => {
                    let width = solve_window(app, vcx, 397., 900., &state);
                    (width, painted_body(vcx, &state), false)
                }
                CommitScene::Body360 => {
                    let width = solve_window(app, vcx, 360., 900., &state);
                    (width, painted_body(vcx, &state), false)
                }
            };
            let what = format!("{state} at {width}px, body {painted}px");
            assert_holds(app, vcx, &held, &what);
            if checks == Checks::Fits {
                assert_commit_fits(
                    vcx,
                    focused,
                    subject.labelled(),
                    natural,
                    minima,
                    label_needed,
                    &what,
                );
            }
            if checks == Checks::WayBack && focused {
                assert_way_back_row(vcx, minima.path, &what);
                // A path short enough to fit its share whole is not cut at all.
                if path.chars().count() > 12
                    && matches!(scene, CommitScene::Body122 | CommitScene::Body117)
                {
                    assert_back_path_shows_both_ends_and_its_whole_text_on_hover(vcx, path, &what);
                }
            }
        }
    }

    /// Which part of a commit header a case asserts: every part inside the
    /// body with the ref's icon and SHA placed as the usable row allows, or the
    /// focused file's way back and its path alone.
    #[derive(Clone, Copy, PartialEq)]
    enum Checks {
        Fits,
        WayBack,
    }

    /// The focused file's way back in the usable row (the body less the lead
    /// and the trail): the whole control inside it, the path inside the
    /// control, and the path at its readable minimum wherever the control holds
    /// it there, or, where it cannot, at no less than what the row leaves
    /// after the control's chevron part: the path gives up what is lacking and
    /// no more.
    fn assert_way_back_row(vcx: &mut VisualTestContext, least_path: f32, what: &str) {
        let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{what}: {selector} is not drawn"))
        };
        let body = bounds(vcx, "diff-overlay-body");
        let close = bounds(vcx, "diff-overlay-close");
        let back = bounds(vcx, "diff-overlay-unfocus");
        let lead = crate::ui::app::CONTENT_INSET;
        let trail = f32::from(body.right() - close.right());
        let usable = f32::from(body.size.width) - lead - trail;
        let eps = px(0.5);
        // The path has a selector in the overflow rows only; in the ladder it
        // is the header's own, and only the control has to be in its row.
        let Some(path) = vcx.debug_bounds("diff-overlay-path") else {
            assert!(
                back.left() >= body.left() + px(lead) - eps
                    && back.right() <= body.right() - px(trail) + eps,
                "{what}: the back button {back:?} is outside the {usable}px usable row of the \
                 body {body:?}; {}",
                diff_widths(vcx)
            );
            return;
        };
        let chrome = f32::from(back.size.width - path.size.width);
        let floor = if chrome + least_path <= usable + 0.01 {
            least_path
        } else {
            (usable - chrome).max(0.)
        };
        let widths = diff_widths(vcx);
        assert!(
            back.left() >= body.left() + px(lead) - eps
                && back.right() <= body.right() - px(trail) + eps
                && path.left() >= back.left() - eps
                && path.right() <= back.right() + eps,
            "{what}: the back button {back:?} and its path {path:?} are not inside the {usable}px \
             usable row of the body {body:?} ({lead}px lead, {trail}px trail); {widths}"
        );
        assert!(
            f32::from(path.size.width) >= floor - 0.5,
            "{what}: the path {path:?} is under the {floor}px the row leaves it ({chrome}px of \
             chevron part, {least_path}px minimum, {usable}px row); {widths}"
        );
    }

    /// Where the way back gives its path less than the readable minimum (the
    /// row cannot hold it), the path is still the one measured elision of the
    /// UI, Middle, with the head of the path and its tail both painted and an
    /// ellipsis between, and hovering it shows the whole path on the lines the
    /// text system gives it. The accessible label of the control is checked by
    /// code review and, natively, through AT-SPI: not observable here.
    fn assert_back_path_shows_both_ends_and_its_whole_text_on_hover(
        vcx: &mut VisualTestContext,
        path: &str,
        what: &str,
    ) {
        crate::ui::text_elision::observe::clear();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        let id = gpui::ElementId::from("diff-overlay-path-text");
        let label = crate::ui::text_elision::observe::label(&id)
            .unwrap_or_else(|| panic!("{what}: the way back's path was not drawn"));
        let (first, last) = (path.chars().next().unwrap(), path.chars().last().unwrap());
        assert!(
            label.painted
                && label.text.contains('\u{2026}')
                && label.text.chars().next() == Some(first)
                && label.text.chars().last() == Some(last)
                && label.text.chars().count() < path.chars().count(),
            "{what}: the path {path:?} is not painted with its head, an ellipsis and its tail: \
             {label:?}"
        );
        let card = hover_tooltip(vcx, "diff-overlay-path")
            .unwrap_or_else(|| panic!("{what}: the cut path has no tooltip with its whole text"));
        let lines = tooltip_lines(vcx, path, f32::from(card.size.width));
        let drawn = (f32::from(card.size.height) / tooltip_line_h(vcx)).round() as usize;
        assert_eq!(
            drawn, lines,
            "{what}: the tooltip {card:?} is {drawn} line(s), and the whole path {path:?} takes \
             {lines} in its width"
        );
    }

    /// A commit's header, with its label and bare, plain and opened on a file
    /// (a short path, a long one and the native capture's), at the largest
    /// interface size in Russian, in the three scenes of the native captures:
    /// the short ref is whole at its natural width in every one, every control
    /// and identity is inside the painted body and none is painted over
    /// another, and in the 157px body the commit's label is still there. The
    /// first case is the finding itself: the labelled commit opened on
    /// `src/native-diff-lines.txt` in the native 437px body, whose SHA used to
    /// vanish.
    #[gpui::test]
    fn a_commit_header_keeps_its_ref_whole_and_every_part_inside_at_the_largest_font(
        cx: &mut TestAppContext,
    ) {
        check_commit_scenes(
            cx,
            &[
                CommitScene::Wide1440,
                CommitScene::Body437,
                CommitScene::Body157,
                CommitScene::Body397,
                CommitScene::Body360,
            ],
            true,
            Checks::Fits,
        );
    }

    /// The focused file's way back keeps its path whole at its readable
    /// minimum wherever the usable row holds both (the wide bodies, and the
    /// two 1000px states: 397px with the saved Dock falling back, 360px with
    /// the Fill latch on): the same behavior as before, unchanged.
    #[gpui::test]
    fn the_way_back_keeps_its_path_floor_where_the_row_holds_it(cx: &mut TestAppContext) {
        check_commit_scenes(
            cx,
            &[
                CommitScene::Wide1440,
                CommitScene::Body437,
                CommitScene::Body157,
                CommitScene::Body397,
                CommitScene::Body360,
            ],
            false,
            Checks::WayBack,
        );
    }

    /// At the floors (122px and the native 117px: usable rows of 102px and
    /// 97px) the way back's chevron part and a path floor of 70px need 109px,
    /// more than the row. The path gives up only what is lacking, down to the
    /// 58px the native row leaves, through the UI's one Middle elision: its
    /// head and its tail painted with an ellipsis between, the whole path on
    /// hover. The control is inside the usable row. (The accessible label is
    /// not observable here.)
    #[gpui::test]
    fn the_way_back_gives_its_path_only_the_width_the_row_lacks_at_the_floors(
        cx: &mut TestAppContext,
    ) {
        check_commit_scenes(
            cx,
            &[CommitScene::Body122, CommitScene::Body117],
            false,
            Checks::WayBack,
        );
    }

    /// The same commit headers where the usable row (the body less the lead
    /// and the trail) is under the ref's icon, gap and short SHA: the 122px
    /// body of the test backend's 720px window and the native 117px one (the
    /// panel's tab-count slot, B3). The icon is alone on a row before and the
    /// whole SHA is first on the next, nothing is outside the body or the row,
    /// and a focused file's way back gives its path only the width it lacks,
    /// keeping both ends of the path and its whole text on hover.
    #[gpui::test]
    fn a_commit_header_puts_the_icon_on_the_row_before_a_whole_sha_at_the_floors(
        cx: &mut TestAppContext,
    ) {
        check_commit_scenes(
            cx,
            &[CommitScene::Body122, CommitScene::Body117],
            false,
            Checks::Fits,
        );
    }

    /// Each of `scenes` for the commit headers of the native captures; with
    /// `finding_first` the labelled commit on the native path leads, so its red
    /// is the first one reported.
    fn check_commit_scenes(
        cx: &mut TestAppContext,
        scenes: &[CommitScene],
        finding_first: bool,
        checks: Checks,
    ) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        const PATHS: [&str; 3] = [
            "a.rs",
            "src/ui/components/very/deep/directory/tree/with/a-file-name-that-keeps-going.rs",
            "src/native-diff-lines.txt",
        ];
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        set_native_context(&app, &mut vcx, "ru-RU", 24.);
        resize(&mut vcx, 2400., 900.);
        let minima = CommitMinima {
            label: kept_prefix(&app, &mut vcx, "feat", crate::ui::right_panel::TEXT, false),
            byline: kept_prefix(&app, &mut vcx, "000", crate::ui::right_panel::META, true),
            path: shaped_path(&app, &mut vcx, "000\u{2026}000").ceil(),
        };
        // The finding first, so its red is the first one reported.
        if finding_first {
            check_commit_case(
                &app,
                &mut vcx,
                &DiffRef::Commit(COMMIT_SHA),
                true,
                PATHS[2],
                &[CommitScene::Body437],
                minima,
                checks,
            );
        }
        for subject in [DiffRef::Commit(COMMIT_SHA), DiffRef::BareCommit(COMMIT_SHA)] {
            for focused in [false, true] {
                for path in PATHS {
                    if !focused && path != PATHS[0] {
                        continue;
                    }
                    check_commit_case(
                        &app, &mut vcx, &subject, focused, path, scenes, minima, checks,
                    );
                }
            }
        }
        set_ui_font_size(&app, &mut vcx, 16.);
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// A commit with `label_subject` as its label, opened on `path`, as the UI
    /// opens it from a file row.
    fn open_labelled_commit(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        label_subject: &str,
        path: &str,
    ) -> std::sync::Arc<crate::terminal::git_diff::DiffSnapshot> {
        use crate::terminal::git_diff::{
            CommitLabel, DiffSnapshot, DiffSource, FileDiff, FileStatus,
        };
        let snapshot = std::sync::Arc::new(DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source: DiffSource::Commit {
                rev: COMMIT_SHA.to_string(),
                label: Some(CommitLabel {
                    subject: label_subject.to_string(),
                    author: "Dev Example".into(),
                    at: 1_700_000_000,
                }),
            },
            branch: "move-tab-keybind".into(),
            files: vec![FileDiff {
                path: path.into(),
                old_path: None,
                status: FileStatus::Modified,
                added: 12,
                removed: 3,
                binary: false,
                truncated: None,
                hunks: Vec::new(),
            }],
            ..Default::default()
        });
        let held = std::sync::Arc::clone(&snapshot);
        app.update_in(vcx, |app, window, cx| {
            app.open_loaded_diff_for_test(snapshot, false, window, cx);
            let overlay = app.tabs[0].diff_overlay.as_mut().expect("the diff is open");
            overlay.focus = Some(path.to_string());
            cx.notify();
        });
        vcx.run_until_parked();
        assert_holds(app, vcx, &held, "a labelled commit, freshly opened");
        held
    }

    /// The real tooltip of the actual header element `selector`: the mouse
    /// moved onto it, the clock advanced past the show delay, a frame drawn.
    /// Returns where the tooltip put its content, or `None` if it is not there.
    fn hover_tooltip(
        vcx: &mut VisualTestContext,
        selector: &'static str,
    ) -> Option<Bounds<Pixels>> {
        // Off every element first, so no tooltip of an earlier hover is up.
        vcx.simulate_mouse_move(
            gpui::point(px(2.), px(2.)),
            None,
            gpui::Modifiers::default(),
        );
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        let target = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn, so it cannot be hovered"));
        vcx.simulate_mouse_move(target.center(), None, gpui::Modifiers::default());
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.debug_bounds("diff-overlay-tooltip-text")
    }

    /// One line of the tooltip's text: the default line height at the card's
    /// text size (0.875 of the interface size), rounded as a drawn line is.
    fn tooltip_line_h(vcx: &mut VisualTestContext) -> f32 {
        vcx.update(|window, _| {
            let mut style = window.text_style();
            style.font_size = px(0.875 * window.rem_size().as_f32()).into();
            style.line_height_in_pixels(window.rem_size()).as_f32()
        })
    }

    /// How many lines `text` takes in `width` pixels as the real text system
    /// lays it out: the whole text shaped in the card's own font (the interface
    /// family, normal weight, 0.875 of the interface size) with every byte in
    /// one run and the width as its wrap width, which also breaks a word wider
    /// than the width, as the drawn text does.
    fn tooltip_lines(vcx: &mut VisualTestContext, text: &str, width: f32) -> usize {
        use gpui_component::ActiveTheme as _;
        vcx.update(|window, cx| {
            let font = gpui::Font {
                family: cx.theme().font_family.clone(),
                features: Default::default(),
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            };
            let run = gpui::TextRun {
                len: text.len(),
                font,
                color: gpui::Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let size = px(0.875 * window.rem_size().as_f32());
            let lines = window
                .text_system()
                .shape_text(
                    gpui::SharedString::from(text.to_string()),
                    size,
                    &[run],
                    Some(px(width)),
                    None,
                )
                .expect("the text shapes");
            lines
                .iter()
                .map(|line| line.wrap_boundaries.len() + 1)
                .sum()
        })
    }

    /// At the 720px window minimum, in Russian at the largest size, the
    /// overflow header's real tooltips (the commit label, the SHA and the
    /// focused path) hovered on the actual header with the clock advanced: the
    /// whole visible card, its content with its padding, border and outer
    /// margin, is inside the window, and the content is the whole text: as
    /// tall as the lines an independent wrap of every word takes, which for a
    /// text wider than the window is more than one. Two labels, the native
    /// capture's 78 characters and a long Cyrillic sentence. The words
    /// themselves are for the native capture to read.
    #[gpui::test]
    fn the_overflow_label_ref_and_path_tooltips_wrap_inside_the_window(cx: &mut TestAppContext) {
        use crate::ui::tab_strip::overflow_gpui_tests::{set_ui_font_size, use_locale};
        if cfg!(target_os = "macos") {
            return;
        }
        const NATIVE: &str =
            "feat(checkout): add tax rate and regional VAT handling for saved payment methods";
        const CYRILLIC: &str = "Исправление: добавлена обработка налоговой ставки и региональных \
            вычетов для сохранённых способов оплаты при оформлении заказа в корзине покупателя";
        const PATH: &str =
            "src/ui/components/very/deep/directory/tree/with/a-file-name-that-keeps-going.rs";
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        set_right_panel(&app, &mut vcx, false);
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(0.5, cx);
            app.tabs[0].name = Some(TAB_TITLE.to_string());
            cx.notify();
        });
        set_native_context(&app, &mut vcx, "ru-RU", 24.);
        resize(&mut vcx, 2400., 900.);
        let rem = vcx.update(|window, _| window.rem_size().as_f32());
        // What surrounds the tooltip's content, per side, in the component's
        // own card (`tooltip.rs`): the outer margin `m_3` (0.75 rem), the
        // padding `px_2p5` (0.625 rem) and the 1px border.
        let chrome = 0.75 * rem + 0.625 * rem + 1.;
        // Above and below: the outer margin, the padding `py_1` (0.25 rem) and
        // the border.
        let chrome_v = 0.75 * rem + 0.25 * rem + 1.;
        let line_h = tooltip_line_h(&mut vcx);
        for label in [NATIVE, CYRILLIC] {
            let held = open_labelled_commit(&app, &mut vcx, label, PATH);
            put_in_scene(&app, &mut vcx, DocumentLayout::Fill, true, 720., 520.);
            assert_holds(&app, &mut vcx, &held, label);
            let (window_w, window_h) = (720., 520.);
            for (name, selector, text) in [
                ("commit label", "diff-overlay-label-subject", label),
                ("ref", "diff-overlay-rev", COMMIT_SHA),
                ("path", "diff-overlay-unfocus", PATH),
            ] {
                let what = format!("ru-RU UI24 at 720px, the {name} tooltip of {label:?}");
                let content = hover_tooltip(&mut vcx, selector).unwrap_or_else(|| {
                    panic!("{what}: no tooltip content was drawn after the hover")
                });
                assert!(
                    f32::from(content.left()) - chrome >= -0.5
                        && f32::from(content.right()) + chrome <= window_w + 0.5,
                    "{what}: the whole card, content {content:?} and {chrome}px of padding, \
                     border and margin each side, is not inside the {window_w}px window"
                );
                assert!(
                    f32::from(content.top()) - chrome_v >= -0.5
                        && f32::from(content.bottom()) + chrome_v <= window_h + 0.5,
                    "{what}: the whole card, content {content:?} and {chrome_v}px of padding, \
                     border and margin above and below, is not inside the {window_h}px window"
                );
                // The visible card, content with its padding and border (the
                // outer margin is not part of it), is at most the hover maximum.
                let width = f32::from(content.size.width);
                let card = width + 2. * (0.625 * rem + 1.);
                assert!(
                    card <= 420. + 0.5,
                    "{what}: the visible card is {card}px wide, over the 420px hover maximum"
                );
                let lines = tooltip_lines(&mut vcx, text, width);
                assert!(
                    (f32::from(content.size.height) - lines as f32 * line_h).abs() <= 1.,
                    "{what}: the content {content:?} is not the {lines} lines of {line_h}px the \
                     whole text takes in {width}px"
                );
            }
        }
        set_ui_font_size(&app, &mut vcx, 16.);
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// The keycap labels a title-bar chord can carry in these tests, each with
    /// the selector the keycap paints its bounds under.
    const CAP_SELECTORS: [(&str, &str); 5] = [
        ("Ctrl", "dialog-keycap-Ctrl"),
        ("Shift", "dialog-keycap-Shift"),
        ("Alt", "dialog-keycap-Alt"),
        ("P", "dialog-keycap-P"),
        ("K", "dialog-keycap-K"),
    ];

    fn cap_label(token: &str) -> &'static str {
        CAP_SELECTORS
            .iter()
            .find(|(label, _)| *label == token)
            .map(|(label, _)| *label)
            .unwrap_or_else(|| panic!("no selector for the {token} key"))
    }

    /// One frame of the window at `width`: the title-bar search box, and every
    /// keycap the frame painted, left to right, by label.
    struct SearchFrame {
        search: Bounds<Pixels>,
        caps: Vec<(&'static str, Bounds<Pixels>)>,
    }

    impl SearchFrame {
        fn width(&self) -> f32 {
            f32::from(self.search.size.width)
        }
    }

    fn search_frame(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        width: f32,
    ) -> SearchFrame {
        resize(vcx, width, 900.);
        app.update_in(vcx, |_, _, cx| cx.notify());
        vcx.run_until_parked();
        // A fresh frame, so no bound from an earlier one is read back.
        vcx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        let search = vcx
            .debug_bounds("titlebar-search")
            .expect("the title-bar search box is drawn with the tabs in the rail");
        let mut caps: Vec<(&'static str, Bounds<Pixels>)> = CAP_SELECTORS
            .iter()
            .filter_map(|(label, selector)| vcx.debug_bounds(selector).map(|b| (*label, b)))
            .collect();
        caps.sort_by(|a, b| a.1.left().partial_cmp(&b.1.left()).unwrap());
        SearchFrame { search, caps }
    }

    /// The chord as painted: one cap per key in the binding's order, each
    /// 18px tall, 3px apart and inside the search box.
    fn assert_chord(frame: &SearchFrame, tokens: &[&str], what: &str) {
        let found: Vec<&str> = frame.caps.iter().map(|(label, _)| *label).collect();
        let expected: Vec<&str> = tokens.iter().map(|t| cap_label(t)).collect();
        assert_eq!(
            found, expected,
            "{what}: the caps painted in the search box"
        );
        let eps = 0.01;
        for (label, cap) in &frame.caps {
            assert!(
                (f32::from(cap.size.height) - 18.).abs() < eps,
                "{what}: the {label} cap is {:?} tall, not 18px",
                cap.size.height
            );
            assert!(
                cap.left() >= frame.search.left()
                    && cap.right() <= frame.search.right()
                    && cap.top() >= frame.search.top()
                    && cap.bottom() <= frame.search.bottom(),
                "{what}: the {label} cap {cap:?} is outside the search box {:?}",
                frame.search
            );
        }
        for pair in frame.caps.windows(2) {
            let gap = f32::from(pair[1].1.left() - pair[0].1.right());
            assert!(
                (gap - 3.).abs() < eps,
                "{what}: {gap}px between the {} and {} caps, not 3px",
                pair[0].0,
                pair[1].0
            );
        }
    }

    /// The chord is one keycap per key, on the search box, and only while the
    /// box is 240px or wider. Swept across a window that carries a column and
    /// the panel so the box crosses the threshold, then pinned either side of
    /// it to the pixel.
    #[gpui::test]
    fn the_title_bar_search_draws_its_chord_as_keycaps_from_240px(cx: &mut TestAppContext) {
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        app.update_in(&mut vcx, |app, _, cx| app.set_document_ratio(0.5, cx));
        resize(&mut vcx, 1440., 900.);
        open_names(&app, &mut vcx, &NAMES[..1]);
        set_right_panel(&app, &mut vcx, true);
        let spec = vcx
            .update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx))
            .expect("Search Everywhere has a binding");
        let tokens = crate::ui::keymap::key_tokens(&spec);
        assert_eq!(tokens, ["Ctrl", "Shift", "P"], "the resolved default chord");
        let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();

        let mut first_wide = None;
        let (mut narrow_seen, mut wide_seen) = (false, false);
        let mut width = 700.;
        while width <= 1800. {
            let frame = search_frame(&app, &mut vcx, width);
            match frame.width() >= 240. {
                true => {
                    wide_seen = true;
                    first_wide.get_or_insert(width);
                    assert_chord(
                        &frame,
                        &tokens,
                        &format!("{width}px, a {}px box", frame.width()),
                    );
                }
                false => {
                    narrow_seen = true;
                    assert_chord(
                        &frame,
                        &[],
                        &format!("{width}px, a {}px box", frame.width()),
                    );
                }
            }
            width += 12.;
        }
        assert!(
            narrow_seen && wide_seen,
            "the sweep must cross the threshold: narrow {narrow_seen}, wide {wide_seen}"
        );

        // To the pixel: the narrowest window whose box is 240px, and the one
        // before it.
        let mut edge = first_wide.expect("a wide box was seen");
        while search_frame(&app, &mut vcx, edge - 1.).width() >= 240. {
            edge -= 1.;
        }
        let at = search_frame(&app, &mut vcx, edge);
        let below = search_frame(&app, &mut vcx, edge - 1.);
        assert!(
            at.width() >= 240. && below.width() < 240.,
            "{edge}px must be where the box reaches 240px: {} -> {}",
            below.width(),
            at.width()
        );
        assert_chord(&at, &tokens, &format!("at {}px", at.width()));
        assert_chord(&below, &[], &format!("at {}px", below.width()));
    }

    /// The chord is the binding the user has, not the default's, and an
    /// unbound search draws no caps at all. The action's binding is read, never
    /// written.
    #[gpui::test]
    fn the_title_bar_search_chord_follows_the_users_binding(cx: &mut TestAppContext) {
        use crate::core::config::KeybindingOverride;
        if cfg!(target_os = "macos") {
            return;
        }
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        let frame = search_frame(&app, &mut vcx, 1440.);
        assert!(frame.width() >= 240., "the box is wide: {}", frame.width());
        assert_chord(&frame, &["Ctrl", "Shift", "P"], "the default binding");

        for (binding, tokens) in [
            (vec!["alt-k".to_string()], vec!["Alt", "K"]),
            (Vec::new(), Vec::new()),
        ] {
            vcx.update(|_, cx| {
                let mut cfg = cx.global::<Config>().clone();
                cfg.keybindings.insert(
                    "TogglePalette".to_string(),
                    KeybindingOverride::Exact(binding.clone()),
                );
                cx.set_global(cfg);
            });
            let frame = search_frame(&app, &mut vcx, 1440.);
            assert!(
                frame.width() >= 240.,
                "{binding:?}: the box is still wide: {}",
                frame.width()
            );
            assert_chord(&frame, &tokens, &format!("{binding:?}"));
            let resolved =
                vcx.update(|_, cx| crate::ui::keymap::effective_key("TogglePalette", cx));
            assert_eq!(
                resolved,
                binding.first().cloned(),
                "the binding the chord is read from is the user's, unchanged"
            );
        }
    }
}
