//! The header's file strip when there are more files than it has room for,
//! and the controls that share the header with it.
//!
//! **The strip keeps the files you use.** It draws as many cells as the
//! header has room for at their narrowest, which is a few in a docked column
//! and more in a filled window; past that the strip used to scroll sideways,
//! which put the file you were just in out of sight as often as not. Now it
//! holds the most recently fronted files — in their strip order, so nothing
//! jumps about as you switch between them — and a `+N` button at its end
//! lists everything, hidden files first, with a filter box.
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

/// How many of `total` files a strip with `room` points draws at
/// [`TAB_MIN_W`] each, with the `+N` the rest fold into. At least one, so the
/// file in front is never the one left out.
pub(crate) fn strip_fit(room: f32, total: usize, g: OverflowGlyphs) -> usize {
    let fit = |w: f32| ((w + TAB_GAP) / (TAB_MIN_W + TAB_GAP)).floor().max(1.) as usize;
    if total <= fit(room) {
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
        window: &Window,
        cx: &gpui::App,
    ) -> f32 {
        self.editor_header_w(chrome, window, cx) - self.editor_header_fixed_w(chrome, window, cx)
    }

    /// The width the header is drawn at in `chrome`.
    fn editor_header_w(&self, chrome: DocumentChrome, window: &Window, cx: &gpui::App) -> f32 {
        let docked = self.document_dock_px(window, cx);
        match chrome {
            DocumentChrome::Fill => self.document_body_px(window, cx),
            DocumentChrome::Dock => docked.map_or_else(
                || self.document_body_px(window, cx),
                crate::ui::document_column::document_content_px,
            ),
            // The title bar's trailing chrome is only in the way when no
            // detail panel is open and this column is the one at the
            // window's right edge, the same condition `render` reserves it
            // under.
            DocumentChrome::DockHoisted => {
                let trailing = match self.right_panel_open(cx) {
                    true => 0.,
                    false => crate::ui::tab_strip::trailing_chrome_w(window.is_fullscreen()),
                };
                docked.unwrap_or_else(|| self.document_body_px(window, cx)) - trailing
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
        window: &Window,
        cx: &gpui::App,
    ) -> usize {
        let total = self.tab_code().map_or(0, |c| c.files.len());
        strip_fit(
            self.editor_header_strip_room(chrome, window, cx),
            total,
            self.editor.overflow_glyphs.glyphs,
        )
    }

    /// The least width the hoisted header needs to keep its controls, the
    /// front file at [`TAB_MIN_W`] and the `+N` the others fold into.
    fn editor_header_need(&self, window: &Window, cx: &gpui::App) -> f32 {
        let total = self.tab_code().map_or(0, |c| c.files.len());
        let chrome = DocumentChrome::DockHoisted;
        let overflow = match total {
            0 | 1 => 0.,
            _ => overflow_w(digits(total - 1), self.editor.overflow_glyphs.glyphs),
        };
        self.editor_header_fixed_w(chrome, window, cx)
            + if total == 0 { 0. } else { TAB_MIN_W }
            + overflow
    }

    /// Whether a docked code editor draws its header inside its column
    /// rather than lifted into the title bar: the title bar's share of the
    /// column cannot hold the controls and the front file. Read-only; the
    /// latch it is judged against is [`Self::editor_header_latch_sync`]'s.
    pub(crate) fn editor_header_inline(&self, window: &Window, cx: &gpui::App) -> bool {
        if cfg!(target_os = "macos")
            || self.document_front() != Some(crate::ui::app::OverlayTop::Code)
            || self.document_dock_px(window, cx).is_none()
        {
            return false;
        }
        header_inline(
            self.editor_header_w(DocumentChrome::DockHoisted, window, cx),
            self.editor_header_need(window, cx),
            self.editor.header_inline,
        )
    }

    /// Measures the `+N` glyphs again if the font or size they are for has
    /// changed. Run by the observers of the theme and the config as well, which
    /// can change either without the app being notified.
    pub(crate) fn editor_overflow_refresh(&mut self, window: &Window, cx: &gpui::App) {
        if !self.editor.overflow_glyphs.is_for(cx) {
            self.editor.overflow_glyphs = OverflowMetrics::measure(window, cx);
        }
    }

    /// Records the answer [`Self::editor_header_inline`] gives now, so the
    /// next one is held [`HEADER_HYSTERESIS`] past its threshold. Runs when
    /// the window or the app changes, never while drawing.
    pub(crate) fn editor_header_latch_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_overflow_refresh(window, cx);
        let inline = self.editor_header_inline(window, cx);
        if self.editor.header_inline != inline {
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
                let f = self.buffer(id)?;
                let dir = if f.untitled.is_some() {
                    String::new()
                } else {
                    let local = f.host.id() == local_host;
                    let mut segs =
                        super::nav::path_segments(&f.path, if local { roots } else { &[] });
                    segs.pop();
                    segs.join("/")
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

    pub(super) fn editor_toggle_strip_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor.strip_picker.is_some() {
            self.editor_close_strip_picker(window, cx);
            return;
        }
        let n = self.tab_code().map_or(0, |c| c.files.len());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t_fmt(L10nKey::EditorStripSearch, &[("n", &n.to_string())]))
        });
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
            over_button: true,
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

    /// The `+N` button at the end of the strip, with its list when open.
    pub(super) fn render_strip_overflow(
        &self,
        hidden: &[BufferId],
        cap: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if hidden.is_empty() {
            return None;
        }
        let theme = cx.theme();
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
                d.child(
                    div()
                        .when(cfg!(test), |d| {
                            d.debug_selector(|| "editor-strip-more-dot".into())
                        })
                        .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                        .rounded_full()
                        .bg(theme.muted_foreground),
                )
            })
            .child(format!("+{}", hidden.len()))
            .child(
                Icon::new(IconName::ChevronDown)
                    .with_size(px(OVERFLOW_CHEVRON))
                    .text_color(theme.muted_foreground),
            )
            .tooltip(|window, cx| {
                gpui_component::tooltip::Tooltip::new(t(L10nKey::EditorStripAllFiles))
                    .build(window, cx)
            })
            .on_hover(cx.listener(|this, over: &bool, _, _| {
                if let Some(p) = this.editor.strip_picker.as_mut() {
                    p.over_button = *over;
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                this.editor_toggle_strip_picker(window, cx);
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
        let row_hover = gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().popover.hover);

        let heading = |label: SharedString| {
            div()
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
            if !querying && i == hidden {
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
                            .child(
                                div()
                                    .when(cfg!(test), |d| {
                                        d.debug_selector(move || {
                                            format!("editor-strip-row-dot-{i}")
                                        })
                                    })
                                    .size(px(crate::ui::tab_strip::ROW_STATUS_DOT))
                                    .rounded_full()
                                    .bg(muted),
                            ),
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
        let footer_button = |key: &'static str, label: L10nKey| {
            div()
                .id(key)
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
            .child(rule())
            .child(
                h_flex()
                    .h(px(28.))
                    .px(px(8.))
                    .gap(px(14.))
                    .items_center()
                    .text_size(rems(12. / 16.))
                    .child(
                        footer_button("editor-strip-close-saved", L10nKey::EditorStripCloseSaved)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor.strip_picker = None;
                                let ids = this.editor_strip_others(true);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    )
                    .child(
                        footer_button("editor-strip-close-others", L10nKey::EditorStripCloseOthers)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.editor.strip_picker = None;
                                let ids = this.editor_strip_others(false);
                                this.editor_close_buffers(ids, window, cx);
                                this.focus_editor(window, cx);
                            })),
                    ),
            );
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
    /// behind a `+N` that exists exactly when some file is left out, and
    /// every control clear of its neighbour.
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
            p.tabs.len() < total,
            "{label}: +N must be there exactly when files are left out ({} of {total} drawn)",
            p.tabs.len()
        );
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
        assert!(p.more.is_none(), "nothing is left out, so no +N");

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
                        p.more.is_some(),
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

    #[gpui::test]
    fn tab_and_picker_dirty_dots_are_5px(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        dock_config(&mut vcx, DocumentLayout::Dock);
        resize(&mut vcx, 1440., 900.);

        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(open_rs(&app, &mut vcx, &format!("/dots/dirty-file-{i}.rs")));
        }
        app.update_in(&mut vcx, |app, _, _| {
            for id in &ids {
                app.buffer_mut(*id).unwrap().dirty = true;
            }
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

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_toggle_strip_picker(window, cx);
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
        // more, so the same column has the room.
        app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_ratio(DOCUMENT_RATIO_MIN, cx)
        });
        vcx.run_until_parked();
        assert!(
            header_is_inline(&mut vcx),
            "the narrowest column, panel closed"
        );
        set_right_panel(&app, &mut vcx, true);
        assert!(
            docked_px(&app, &mut vcx).is_some(),
            "the window still seats a column beside the panel"
        );
        assert!(
            !header_is_inline(&mut vcx),
            "panel open frees the trailing room"
        );
        assert_header(&app, &mut vcx, "panel open");
        set_right_panel(&app, &mut vcx, false);
        assert!(header_is_inline(&mut vcx), "panel closed again");
        assert_header(&app, &mut vcx, "panel closed again");
    }
}
