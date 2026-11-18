use gpui::{
    Animation, AnimationExt as _, AnyElement, Axis, Bounds, Context, Div, ExternalPaths,
    FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString,
    Stateful, Window, canvas, deferred, div, ease_out_quint, linear_color_stop, linear_gradient,
    prelude::*, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent};
use gpui_component::kbd::Kbd;
use gpui_component::menu::{ContextMenu, ContextMenuExt as _, PopupMenuItem};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use std::path::{Path, PathBuf};

use crate::core::config::Config;
use crate::core::group_key::{
    AutoKey, GroupId, GroupKey, PinnedGroup, WorkspaceGroups, auto_key, auto_names, pinned_names,
    place,
};
use crate::terminal::git_status::GitStatusCache;
use crate::ui::agent_vision::{PaneKey, PaneSummary, State};
use crate::ui::app::{TITLE_BAR_BORDER, Tab, TitleStripRow as _, Tty59App};
use crate::ui::hints::{badge_positions_from_order, tab_badge_label};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::reorder::{self, Reorder, Surface};
use crate::ui::right_panel::RESIZE_HANDLE_WIDTH;
use crate::ui::tab_strip::{DragTab, REORDER_SLIDE_MS, abbreviate_home};
use crate::ui::text_elision::{ElidedLabel, Mode};

pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 180.;

const GRAB_HANDLE_W: f32 = 48.;

const ROW_GAP: f32 = 1.;

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
struct RenderedSidebarAgentRow {
    key: PaneKey,
    title: String,
}

#[cfg(test)]
std::thread_local! {
    static RENDERED_SIDEBAR_AGENT_ROWS: std::cell::RefCell<Vec<RenderedSidebarAgentRow>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn take_rendered_sidebar_agent_rows() -> Vec<RenderedSidebarAgentRow> {
    RENDERED_SIDEBAR_AGENT_ROWS.with(|rows| std::mem::take(&mut *rows.borrow_mut()))
}

// The "nothing matches" sentences handed to the rail's elements, exactly as
// handed: the text a user would read, not the selector it was found by.
#[cfg(test)]
std::thread_local! {
    static RENDERED_SIDEBAR_NOTHING_MATCHES: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn take_rendered_sidebar_nothing_matches() -> Vec<String> {
    RENDERED_SIDEBAR_NOTHING_MATCHES.with(|lines| std::mem::take(&mut *lines.borrow_mut()))
}

/// What the rail's search field is called: agents' search only while the
/// Agents list is both enabled and the one showing.
fn sidebar_search_placeholder(agents_enabled: bool, list: SidebarList) -> L10nKey {
    match (agents_enabled, list) {
        (true, SidebarList::Agents) => L10nKey::VisionSearch,
        _ => L10nKey::SearchTabs,
    }
}

/// The strokes the keymap has for New Agent Tab, first binding only, in the
/// order they are typed; empty when the action is unbound. Read from the
/// keymap's per-action index rather than by resolving the whole table.
fn new_agent_chord(cx: &gpui::App) -> Vec<gpui::Keystroke> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    keymap
        .bindings_for_action(&crate::core::actions::NewAgentTab)
        .next()
        .map(|binding| {
            binding
                .keystrokes()
                .iter()
                .map(|stroke| stroke.inner().clone())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidebarList {
    #[default]
    Terminal,
    Agents,
}

#[derive(Clone, Debug)]
struct AgentRow {
    tab: usize,
    summary: PaneSummary,
}

impl AgentRow {
    fn marker(&self) -> Option<String> {
        self.summary.pane_marker()
    }

    fn status(&self) -> Option<crate::core::cli_agent::AgentStatus> {
        use crate::core::cli_agent::AgentStatus;
        match self.summary.state {
            State::Idle => Some(AgentStatus::Idle),
            State::Working => Some(AgentStatus::Working),
            State::Waiting => Some(AgentStatus::Waiting),
            State::Done => Some(AgentStatus::Done),
            State::Exited => None,
        }
    }
}

/// Follow tab and split order, matching machine and pane together. Shells
/// have no summary and therefore cannot produce a row.
fn build_agent_rows(
    workspace: crate::core::session::WorkspaceId,
    layout: &[(usize, tty59_core::core::machine::TabId, Vec<PaneKey>)],
    summaries: &[PaneSummary],
) -> Vec<AgentRow> {
    layout
        .iter()
        .flat_map(|(tab, id, panes)| {
            panes.iter().filter_map(move |key| {
                let summary = summaries.iter().find(|summary| {
                    summary.location.workspace == workspace
                        && summary.location.tab == *id
                        && summary.location.key == *key
                })?;
                Some(AgentRow {
                    tab: *tab,
                    summary: summary.clone(),
                })
            })
        })
        .collect()
}

/// Shared row chrome for tab rows and agent-pane rows.
fn sidebar_row_chrome(
    id: impl Into<gpui::ElementId>,
    active: bool,
    sf: crate::ui::presets::Surface,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .cursor_pointer()
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap(px(row_metrics::GAP))
        .px_2()
        .rounded(crate::ui::rounding::ROW_RADIUS)
        .when(active, |s| {
            s.bg(gpui::rgb(sf.selected))
                .text_color(gpui::rgb(sf.text_selected))
        })
        .when(!active, |s| {
            s.text_color(gpui::rgb(sf.text_resting))
                .hover(|s| s.bg(gpui::rgb(sf.hover)))
        })
}

/// What marks a kept group, and what the header button that keeps one reads.
///
/// A pin, drawn: it names what was done to the group, where the `◆` it
/// replaced only said "different". With no line between kept and derived
/// groups it is also the only thing that tells the two apart, so every kept
/// group carries it.
const PIN_MARK: &str = "icons/pin.svg";

/// The mark's size, and the glyph of the header's pin and "+" actions: one
/// 12px size, so the status mark and the buttons that replace it read as a set.
const PIN_MARK_SIZE: f32 = 12.;

/// A read-only record of which header action's press handler ran, in order,
/// for the tests that must tell one press from two: pinning an already pinned
/// group is a no-op, so a final state cannot show a handler firing twice.
#[cfg(test)]
mod header_action_probe {
    use std::cell::RefCell;

    thread_local! {
        static CALLS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record(action: &'static str) {
        CALLS.with(|c| c.borrow_mut().push(action));
    }

    pub(super) fn take() -> Vec<&'static str> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
}

/// A single-line tab row: one line of `text_sm` and a little air.
const ROW_HEIGHT: f32 = 30.;

/// A row with a branch line under its title.
const ROW_HEIGHT_TWO_LINE: f32 = 42.;

/// The air between two groups — what makes a group a group without a box.
const GROUP_GAP: f32 = 16.;

/// A group heading's line.
const HEADER_HEIGHT: f32 = 22.;

/// The group heading and the row's branch line: 11.5px at the default size.
const META_REM: f32 = 11.5 / 16.;

/// The rail's side inset, for its header tiles, controls and rows alike.
const RAIL_INSET: f32 = 12.;

/// The workspace chip and the search field under the rail's header.
const HEAD_CONTROL_HEIGHT: f32 = 28.;
const HEAD_CONTROL_RADIUS: f32 = 7.;

/// What sits between a group's branch and its diff counts: the branch's tail,
/// led by the space that would otherwise be a flex gap.
const META_SEP_TRIMMED: &str = "  ·";

/// The fewest characters of a header's branch worth drawing. Below this the
/// branch names nothing (`fea…`), so it is dropped and only the counts stay.
const MIN_HEADER_BRANCH: usize = 6;

/// Tabular numerals, so a column of diff counts lines up digit for digit.
fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The sizes the row's fixed-width children are drawn with, shared by the
/// painters that need them.
mod row_metrics {
    /// The avatar handed to `tab_avatar_plain`.
    pub(super) const AVATAR: f32 = 16.;
    /// `gap_2p5` between the row's children.
    pub(super) const GAP: f32 = 10.;
    /// The trailing action slot, laid over the row's right end instead of
    /// beside the title. The hover × and the Alt badge are both drawn in it,
    /// from these numbers and `tab_strip::MIN_TARGET` (the slot's width), so
    /// the two cannot drift apart.
    pub(super) const TRAIL_INSET: f32 = 6.;
    /// The fade from the row into the slot's backing, to the slot's left.
    pub(super) const TRAIL_FADE: f32 = 10.;
    /// The badge's own box, centred in the slot.
    pub(super) const BADGE: f32 = 20.;
}

/// The row's trailing action slot: absolute at the row's right inset and its
/// full height (a two-line row's diff count sits under the button's band), a
/// `TRAIL_FADE`px fade into `rest`, then `content` on `rest`.
///
/// Taking no room from the line is the point: what is in the slot (the hover
/// × or the Alt badge) covers the last few glyphs of the row, it never makes
/// the title give them up, so holding the shortcut chord moves nothing.
/// `hovered` is the colour the row turns while the pointer is over it, for a
/// slot that is on screen at rest (the badge) and must keep matching the row.
fn trailing_slot(
    rest: gpui::Hsla,
    hovered: Option<(SharedString, gpui::Hsla)>,
    content: impl IntoElement,
) -> Div {
    let fade = |color: gpui::Hsla| {
        let mut from = color;
        from.a = 0.;
        linear_gradient(
            90.,
            linear_color_stop(from, 0.),
            linear_color_stop(color, 1.),
        )
    };
    let mut ramp = div().w(px(row_metrics::TRAIL_FADE)).h_full().bg(fade(rest));
    let mut body = div().h_full().flex().items_center().bg(rest).child(content);
    if let Some((group, color)) = hovered {
        ramp = ramp.group_hover(group.clone(), move |s| s.bg(fade(color)));
        body = body.group_hover(group, move |s| s.bg(color));
    }
    h_flex()
        .absolute()
        .top_0()
        .bottom_0()
        .right(px(row_metrics::TRAIL_INSET))
        .child(ramp)
        .child(body)
}

/// A status mark kept in the line, and so in the row's layout, but invisible
/// while the shortcut badge stands over its place.
fn hold_hidden(mark: gpui::AnyElement, hidden: bool) -> Div {
    div()
        .flex_shrink_0()
        .when(hidden, |d| d.opacity(0.))
        .child(mark)
}

/// The narrowest a group's heading is allowed to get: three or four capitals
/// and an ellipsis, which is still a name and not a stub.
const HEADER_NAME_FLOOR: f32 = 40.;

/// The narrowest a row's title is allowed to get before the working directory
/// beside it stops taking room, and the narrowest that path may be drawn at:
/// below this it is an ellipsis and a slash, which names no directory.
const ROW_TITLE_FLOOR: f32 = 48.;
const ROW_CWD_FLOOR: f32 = 24.;

/// The branch a whole group shares, lifted off its rows and onto its header.
struct SharedGit {
    status: crate::terminal::git_status::GitStatus,
    /// Where a click on the counts opens the diff overlay, if the setting
    /// allows one.
    click: Option<(crate::ui::host_ops::HostId, PathBuf)>,
    /// Every row the group counts, drawn or folded away.
    rows: Vec<usize>,
}

#[derive(Clone)]
pub(crate) struct DragGroup;

impl Render for DragGroup {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The whole values behind a sidebar row's labels, collected so the hover card
/// can be rendered from cloneable data (an `AnyElement` cannot be cloned, but
/// the tooltip closure has to rebuild its content on every hover). The labels
/// elide against whatever width layout gives them, so the card cannot compare
/// "shown" with "full" and is offered for every row.
#[derive(Clone)]
struct SidebarInfo {
    /// The full title. `None` for a placeholder (`Shell 3`), which nothing
    /// can expand.
    title: Option<SharedString>,
    /// The full branch plus diff counts.
    branch: Option<(SharedString, u32, u32)>,
    /// The full working directory.
    cwd: Option<SharedString>,
    /// Remote host, when the avatar only shows a dot for it.
    host: Option<SharedString>,
}

impl Tty59App {
    /// Whether the tab rail is on screen — the same three conditions `render`
    /// assembles the layout from, in one place the panel opposite can ask.
    pub(crate) fn sidebar_open(&self, cx: &gpui::App) -> bool {
        cx.global::<Config>().tab_bar_position == crate::core::config::TabBarPosition::Left
            && !self.tabs.is_empty()
            && !self.sidebar_collapsed
    }

    /// What the right panel has reserved, from the sidebar's point of view.
    pub(crate) fn right_panel_floor(&self, window: &Window, cx: &gpui::App) -> f32 {
        if self.right_panel_open(cx) {
            self.right_panel_min_px(window, cx)
        } else {
            0.
        }
    }

    pub(crate) fn sidebar_max_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        crate::ui::app::side_panel_max(
            window.viewport_size().width.as_f32(),
            MIN_SIDEBAR_WIDTH,
            self.right_panel_floor(window, cx) + self.document_floor(cx),
        )
    }

    /// How wide the sidebar is drawn, given the live cell and the cap the rest
    /// of the window leaves it. Read here rather than clamped at each caller so
    /// the document column's budget and the sidebar itself can never disagree
    /// about how much width is already spoken for.
    pub(crate) fn sidebar_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        self.sidebar_width
            .get()
            .clamp(MIN_SIDEBAR_WIDTH, self.sidebar_max_px(window, cx))
    }

    pub(crate) fn sidebar_agents_mode(&self, cx: &gpui::App) -> bool {
        self.sidebar_open(cx)
            && cx.global::<Config>().sidebar_agents_toggle
            && self.sidebar_list == SidebarList::Agents
    }

    fn sidebar_agent_rows(&self, summaries: &[PaneSummary], cx: &gpui::App) -> Vec<AgentRow> {
        let layout: Vec<_> = self
            .visual_tab_order(cx)
            .into_iter()
            .map(|i| {
                let tab = &self.tabs[i];
                let panes = tab
                    .pane
                    .leaves()
                    .iter()
                    .filter_map(|slot| {
                        let view = slot.terminal()?.read(cx);
                        Some(PaneKey {
                            host: view.host_id(),
                            pane: view.pane_id,
                        })
                    })
                    .collect();
                (i, tab.tree_id.get(), panes)
            })
            .collect();
        build_agent_rows(self.workspace, &layout, summaries)
    }

    fn select_sidebar_list(
        &mut self,
        mode: SidebarList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_list = mode;
        self.sidebar_scroll.set_offset(gpui::point(px(0.), px(0.)));
        self.sidebar_reveal.set(None);
        self.sidebar_agent_reveal.set(None);
        self.sync_sidebar_search_placeholder(window, cx);
        cx.notify();
    }

    /// The rail's search field names what it searches: agents while the
    /// Agents list is the one showing, tabs otherwise. It is written from the
    /// events that change that — the list switch, a language change and a
    /// config reload (the toggle can be turned off from the file) — because
    /// setting a placeholder notifies the field, which a render must not do.
    pub(crate) fn sync_sidebar_search_placeholder(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = sidebar_search_placeholder(
            cx.global::<Config>().sidebar_agents_toggle,
            self.sidebar_list,
        );
        self.sidebar_search
            .update(cx, |state, cx| state.set_placeholder(t(key), window, cx));
    }

    fn sidebar_list_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let control = h_flex().flex_shrink_0().p_3().gap_1().children(
            [
                (SidebarList::Terminal, L10nKey::SidebarListTerminal),
                (SidebarList::Agents, L10nKey::SidebarListAgents),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (mode, key))| {
                let selected = self.sidebar_list == mode;
                Button::new(("sidebar-list-mode", i))
                    .label(t(key))
                    .custom(crate::ui::tab_strip::chrome_tile_variant_for(selected, cx))
                    .selected(selected)
                    .small()
                    .flex_1()
                    .min_w_0()
                    .h(px(HEAD_CONTROL_HEIGHT))
                    .rounded(px(HEAD_CONTROL_RADIUS))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_sidebar_list(mode, window, cx)
                    }))
            }),
        );
        #[cfg(test)]
        let control = control.debug_selector(|| "sidebar-list-toggle".into());
        control
    }

    fn sidebar_agent_row(
        &self,
        row: &AgentRow,
        slot: usize,
        badge: Option<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let sf = cx.global::<crate::ui::presets::Surfaces>().rail;
        let key = row.summary.location.key;
        let tab = &self.tabs[row.tab];
        let selected = row.tab == self.active
            && tab
                .pane
                .focused_leaf(window, cx)
                .or_else(|| tab.focus_target())
                .and_then(|leaf| leaf.terminal().cloned())
                .is_some_and(|view| {
                    let view = view.read(cx);
                    view.host_id() == key.host && view.pane_id == key.pane
                });
        let status = row.status();
        let title = row.summary.title().to_string();
        let exited = row.summary.state == State::Exited;
        let marker = row.marker();
        #[cfg(test)]
        RENDERED_SIDEBAR_AGENT_ROWS.with(|rows| {
            rows.borrow_mut().push(RenderedSidebarAgentRow {
                key,
                title: title.clone(),
            });
        });
        let rail_fill: gpui::Hsla = gpui::rgb(sf.base).into();
        let group = SharedString::from(format!("sidebar-agent-row-{slot}"));
        let row = sidebar_row_chrome(("sidebar-agent-row", slot), selected, sf)
            // Named so the trailing badge can follow the row's hover fill.
            .group(group.clone())
            .child(
                canvas(
                    {
                        let reveal = self.sidebar_agent_reveal.clone();
                        let scroll = self.sidebar_scroll.clone();
                        move |bounds, window, _cx| {
                            if reveal.get() == Some(key) {
                                reveal.set(None);
                                let view = scroll.bounds();
                                let shift = reveal_shift(
                                    (bounds.top(), bounds.bottom()),
                                    px(0.),
                                    (view.top(), view.bottom()),
                                );
                                if shift != px(0.) {
                                    let mut offset = scroll.offset();
                                    offset.y += shift;
                                    scroll.set_offset(offset);
                                    window.refresh();
                                }
                            }
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .role(gpui::Role::Tab)
            .aria_selected(selected)
            .aria_label(SharedString::from(match &marker {
                Some(marker) => format!("{title}, {marker}"),
                None => title.clone(),
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.activate_sidebar_agent(key, window, cx);
            }))
            .child(self.tab_avatar_plain(
                ("sidebar-agent-avatar", slot),
                row.summary.agent,
                status,
                self.tab_ssh_dot(tab, cx),
                row_metrics::AVATAR,
                cx,
            ))
            .child({
                let title = div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(selected, |d| d.font_weight(FontWeight::MEDIUM))
                    .when(exited, |d| d.text_color(cx.theme().muted_foreground))
                    .child(title);
                #[cfg(test)]
                let title = title.debug_selector(move || format!("sidebar-agent-title-{slot}"));
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(title)
                    .when_some(marker, |d, marker| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(marker),
                        )
                    })
            })
            // In the line, and so in the title's budget, whether or not the
            // chord is held; only invisible while the badge stands over it.
            .children(
                self.row_status_dot(status, 0, rail_fill, cx)
                    .map(|mark| hold_hidden(mark, badge.is_some())),
            )
            .when_some(badge, |d, pos| {
                let badge = div()
                    .size(px(row_metrics::BADGE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .child(tab_badge_label(pos));
                #[cfg(test)]
                let badge = badge.debug_selector(move || format!("sidebar-agent-badge-{slot}"));
                // The row's own fills: what it rests on, and what it turns
                // under the pointer.
                let (rest, hovered): (gpui::Hsla, gpui::Hsla) = if selected {
                    (gpui::rgb(sf.selected).into(), gpui::rgb(sf.selected).into())
                } else {
                    (rail_fill, gpui::rgb(sf.hover).into())
                };
                d.child(trailing_slot(
                    rest,
                    Some((group, hovered)),
                    div()
                        .size(px(crate::ui::tab_strip::MIN_TARGET))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(badge),
                ))
            });
        #[cfg(test)]
        let row = row.debug_selector(move || format!("sidebar-agent-row-{slot}"));
        row
    }

    fn sidebar_agent_shortcut_rows(&self, cx: &gpui::App) -> Vec<AgentRow> {
        let group = cx
            .global::<Config>()
            .tab_group_shortcuts
            .then(|| self.active_group_tab_order(cx));
        self.sidebar_agent_rows(&self.agent_pane_summaries(cx), cx)
            .into_iter()
            .filter(|row| group.as_ref().is_none_or(|tabs| tabs.contains(&row.tab)))
            .collect()
    }

    pub(crate) fn cycle_sidebar_agent(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.sidebar_agent_rows(&self.agent_pane_summaries(cx), cx);
        if rows.is_empty() {
            return;
        }
        let key = self
            .tabs
            .get(self.active)
            .and_then(|tab| {
                tab.pane
                    .focused_leaf(window, cx)
                    .or_else(|| tab.focus_target())
            })
            .and_then(|slot| slot.terminal().cloned())
            .map(|leaf| {
                let view = leaf.read(cx);
                PaneKey {
                    host: view.host_id(),
                    pane: view.pane_id,
                }
            });
        let current = rows
            .iter()
            .position(|row| Some(row.summary.location.key) == key);
        let next = match (current, forward) {
            (Some(i), true) => (i + 1) % rows.len(),
            (Some(i), false) => (i + rows.len() - 1) % rows.len(),
            (None, true) => 0,
            (None, false) => rows.len() - 1,
        };
        self.activate_sidebar_agent(rows[next].summary.location.key, window, cx);
    }

    pub(in crate::ui) fn activate_sidebar_agent(
        &mut self,
        key: PaneKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.tabs.iter().enumerate().find_map(|(i, tab)| {
            tab.pane.leaves().iter().find_map(|slot| {
                let leaf = slot.terminal()?;
                let view = leaf.read(cx);
                (view.host_id() == key.host && view.pane_id == key.pane).then(|| (i, leaf.clone()))
            })
        });
        if let Some((i, leaf)) = target {
            self.activate(i, window, cx);
            self.maximize_pane(leaf, window, cx);
            self.sidebar_agent_reveal.set(Some(key));
        }
    }

    pub(crate) fn tab_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active;
        let sf = cx.global::<crate::ui::presets::Surfaces>().rail;
        let width = self.sidebar_px(window, cx);
        // As typed, for the "nothing matches" line; matching uses the lowered copy.
        let query_text = self.sidebar_search.read(cx).value().trim().to_string();
        let query = query_text.to_lowercase();
        let agents_mode = self.sidebar_agents_mode(cx);
        let agent_rows = if agents_mode {
            self.sidebar_agent_rows(&self.agent_pane_summaries(cx), cx)
        } else {
            Vec::new()
        };
        let agent_badges: Vec<PaneKey> =
            if agents_mode && self.mod_hint_badges && !self.mod_hint_group {
                self.sidebar_agent_shortcut_rows(cx)
                    .into_iter()
                    .take(crate::ui::hints::MAX_TAB_BADGES)
                    .map(|row| row.summary.location.key)
                    .collect()
            } else {
                Vec::new()
            };
        // Blanked here, written again from paint: a row filtered out by the
        // search — or hidden with its collapsed group — must leave no rectangle
        // behind for a pane to be dropped between.
        *self.sidebar_slots.borrow_mut() = vec![Bounds::default(); self.tabs.len()];
        // Read before it is blanked: the rectangles a tab held over the
        // sidebar is measured against are the ones drawn last frame, the
        // same way a pane dropped here is. Blanked and written again below
        // so a group that folds or filters away stops accepting drops.
        let over_group = self.sidebar_regroup_target(window);
        // Taken rather than read, for the same reason: last frame's divider is
        // what a drag is measured against, and the one drawn below replaces it.
        let divider_last = self.sidebar_divider.take();
        let lifting_row = crate::ui::reorder::dragged_sidebar_tab(&self.reorder).is_some();
        // Offered every frame the pointer is over a group, and cleared with
        // the rest of the drag's pending state on the frames it is not — so
        // letting go anywhere else drops on nothing.
        if let Some(target) = over_group {
            crate::ui::reorder::set_regroup(&self.reorder, target);
        }
        self.sidebar_group_slots.borrow_mut().clear();
        // Every group drops itself when its rows filter out, so a query that
        // matches nothing left the sidebar showing only its own search box.
        let mut any_rows = false;

        let mut list = v_flex()
            .id("tab-sidebar-list")
            // A folder dragged in from Finder is pinned as a group of its own.
            .drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().drag_border.opacity(0.06)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.pin_dropped_folders(paths.paths(), cx)
            }))
            .track_scroll(&self.sidebar_scroll)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .px_3()
            .pb(px(20.))
            .gap(px(GROUP_GAP));

        let keys: Rc<Vec<Option<GroupKey>>> = Rc::new(self.sidebar_group_keys(cx));
        let groups = &self.sidebar_groups;
        let sections = sidebar_sections(&keys, groups);
        let badge_positions = self.sidebar_badge_positions(&sections, cx);
        // A search outranks a fold. Typing something that matches a row inside
        // a folded group has to show that row — a box that says nothing
        // matches while the match sits behind a chevron is just lying.
        let folds_apply = query.is_empty();

        // Badges and activation share one mapping. A global hold numbers the
        // sidebar's visual order; a group hold numbers only the active section,
        // starting at one, and leaves rows outside it without a badge.

        // The row shows an elided title and a branch; the filter used to read
        // only the elided title, so typing the branch you can see, or the part
        // of the path the row dropped, matched nothing. The label is built
        // here only when there is a query to match it against — the rows
        // themselves elide against measured width and no longer need it.
        let visible_by_section: Vec<Vec<usize>> = sections
            .iter()
            .map(|s| {
                s.tabs
                    .iter()
                    .copied()
                    .filter(|&i| {
                        (!agents_mode || agent_rows.iter().any(|row| row.tab == i))
                            && (query.is_empty()
                                || (agents_mode
                                    && agent_rows.iter().any(|row| {
                                        row.tab == i
                                            && row
                                                .summary
                                                .agent_name()
                                                .to_lowercase()
                                                .contains(&query)
                                    }))
                                || self
                                    .tab_label(&self.tabs[i], i, Some(window), cx)
                                    .to_lowercase()
                                    .contains(&query)
                                || self.tabs[i]
                                    .leaf_title(Some(window), cx)
                                    .to_lowercase()
                                    .contains(&query)
                                || self.tabs[i]
                                    .git_status(Some(window), cx)
                                    .is_some_and(|g| g.branch.to_lowercase().contains(&query)))
                    })
                    .collect()
            })
            .collect();

        let pointer = window.mouse_position();
        let rem = window.rem_size().as_f32();
        let header_size = rem * META_REM;
        // On the rail the diff counts are metadata like the branch beside
        // them, so they take its caption ink; the hover card, which is where
        // someone goes to read them, keeps the resting green and red.
        let rail_fill: gpui::Hsla = gpui::rgb(sf.base).into();
        let count_ink = cx.theme().muted_foreground;
        let added_ink = crate::ui::presets::resting_ink(
            cx.theme().success,
            cx.theme().muted_foreground,
            rail_fill,
        );
        let removed_ink = crate::ui::presets::resting_ink(
            cx.theme().danger,
            cx.theme().muted_foreground,
            rail_fill,
        );
        // A pinned group is drawn with no tabs in it — it is kept, and the
        // row it grows says how to fill it — except while a search is
        // narrowing the list, where a group with nothing matching is noise.
        let rendered = |ix: &usize| {
            !visible_by_section[*ix].is_empty()
                || (query.is_empty() && sections[*ix].pinned().is_some())
        };
        // Pinned headers reorder among themselves, by hand; that order is the
        // workspace's and is written back whole when a drag lands.
        let pinned_slots: Vec<usize> = (0..sections.len())
            .filter(|&ix| sections[ix].pinned().is_some())
            .filter(rendered)
            .collect();
        let pinned_rects: Rc<RefCell<Vec<Bounds<Pixels>>>> =
            Rc::new(RefCell::new(vec![Bounds::default(); pinned_slots.len()]));
        let pinned_preview = reorder::preview(
            &self.reorder,
            &Surface::PinnedGroups,
            pinned_slots.len(),
            pointer,
        );
        let pinned_display: Vec<usize> = match &pinned_preview {
            Some(p) => {
                let all: Vec<GroupId> = groups.pinned.iter().map(|g| g.id).collect();
                let shown: Vec<GroupId> = pinned_slots
                    .iter()
                    .filter_map(|&ix| sections[ix].pinned())
                    .collect();
                if let Some(order) = reordered_pinned(&all, &shown, p.from, p.target) {
                    reorder::set_pending(&self.reorder, &Surface::PinnedGroups, order);
                }
                p.order.clone()
            }
            None => (0..pinned_slots.len()).collect(),
        };
        // Every auto group draws a header, and a header is what there is to
        // grab, so these are the slots the auto-group surface runs over.
        // Ungrouped is excluded: it is where the keyless tabs fall, and it
        // always sits last.
        let keyed_slots: Vec<usize> = (0..sections.len())
            .filter(|&ix| {
                sections[ix]
                    .key
                    .as_ref()
                    .is_some_and(|k| k.auto().is_some())
            })
            .filter(rendered)
            .collect();
        let keyed_groups = keyed_slots.len();
        let group_slots: Rc<RefCell<Vec<Bounds<Pixels>>>> =
            Rc::new(RefCell::new(vec![Bounds::default(); keyed_groups]));
        let group_preview = reorder::preview(
            &self.reorder,
            &Surface::SidebarGroups,
            keyed_groups,
            pointer,
        );
        let keyed_keys: Vec<AutoKey> = keyed_slots
            .iter()
            .filter_map(|&ix| sections[ix].key.as_ref()?.auto().cloned())
            .collect();
        let slot_display: Vec<usize> = match &group_preview {
            Some(p) => {
                if let (Some(from), Some(to)) = (keyed_keys.get(p.from), keyed_keys.get(p.target))
                    && let Some(order) = regrouped_order(&keys, from, to)
                {
                    reorder::set_pending(&self.reorder, &Surface::SidebarGroups, order);
                }
                p.order.clone()
            }
            None => (0..keyed_groups).collect(),
        };
        let mut blocks: Vec<(Option<HeaderSlot>, usize)> = pinned_display
            .into_iter()
            .map(|slot| (Some(HeaderSlot::Pinned(slot)), pinned_slots[slot]))
            .collect();
        let first_unpinned = blocks.len();
        blocks.extend(
            slot_display
                .into_iter()
                .map(|slot| (Some(HeaderSlot::Auto(slot)), keyed_slots[slot])),
        );
        blocks.extend(
            (0..sections.len())
                .filter(|&ix| sections[ix].key.is_none())
                .filter(rendered)
                .map(|ix| (None, ix)),
        );
        // The divider: kept groups above it, derived ones below. Drawn
        // whenever something is kept, and while an auto header is in the air
        // even when nothing is yet — carrying it above the divider is how it
        // gets pinned, so there has to be a divider to carry it above.
        let lifted_auto = reorder::dragged_auto_group(&self.reorder);
        let show_divider = !groups.pinned.is_empty() || lifted_auto.is_some();
        let pin_hover = lifted_auto.is_some()
            && divider_last.is_some_and(|d| pointer.y < d.origin.y + d.size.height);
        if pin_hover {
            reorder::set_pin(&self.reorder);
        }
        let divider_lit = pin_hover || over_group == Some(reorder::Regroup::ToAuto);
        let divider_zone = groups.pinned.is_empty();
        let mut divider_drawn = false;
        // Whether the tab in the air is kept in a pinned group — then
        // everything below the divider is somewhere it can go.
        let lifted_is_kept = reorder::dragged_sidebar_tab(&self.reorder).is_some_and(|id| {
            self.tabs
                .iter()
                .find(|t| t.tree_id.get() == id)
                .and_then(|t| t.group.get())
                .is_some_and(|g| groups.contains(g))
        });
        let workspace_is_local = !crate::core::session::WorkspaceStore::all(cx)
            .get(self.workspace)
            .is_some_and(|w| w.is_remote());

        // A reveal whose row is not drawn this frame (folded away, or the tab
        // gone) is dropped, so it cannot fire when the row turns up later.
        let reveal = self.sidebar_reveal.get();
        let mut reveal_drawn = false;
        for (n, (group_slot, group_ix)) in blocks.into_iter().enumerate() {
            if n == first_unpinned && show_divider {
                list = list.child(self.sidebar_divider(divider_lit, divider_zone, cx));
                divider_drawn = true;
            }
            let section = &sections[group_ix];
            let group_key = section.key.clone();
            // Only a group that draws a header can be folded — there is
            // nothing to click otherwise, and the one headerless section (the
            // list below the pinned groups, when nothing else is there) must
            // never answer to Ungrouped's fold.
            let folded =
                section.name.is_some() && folds_apply && groups.is_folded(group_key.as_ref());
            let mut rows: Vec<ContextMenu<Stateful<Div>>> = Vec::new();
            // The header keeps counting every row the group has; folding only
            // stops them being drawn. Nothing downstream then registers a
            // rectangle for them, which is what keeps a pane from being
            // dropped into a group that is shut.
            //
            // No exception for the active tab. A fold that leaves one row
            // hanging under a shut chevron, with the header counting rows
            // that are not there, reads as a list that failed to load. The
            // cost is that ⌘T inside a folded group — `spawn_group` seeds
            // the new tab with the group it came from — puts the new tab
            // behind the chevron: the pane area shows the fresh shell and the
            // header count goes up, but the row waits for the group to open.
            let row_count = if agents_mode {
                agent_rows
                    .iter()
                    .filter(|row| visible_by_section[group_ix].contains(&row.tab))
                    .count()
            } else {
                visible_by_section[group_ix].len()
            };
            let visible: Vec<usize> = match folded {
                true => Vec::new(),
                false => visible_by_section[group_ix].clone(),
            };
            let visible_tabs: Vec<usize> = visible.clone();
            let row_slots: Rc<RefCell<Vec<Bounds<Pixels>>>> =
                Rc::new(RefCell::new(vec![Bounds::default(); visible.len()]));
            let row_preview = (!agents_mode)
                .then(|| {
                    reorder::preview(
                        &self.reorder,
                        &Surface::SidebarRows(group_key.clone()),
                        visible.len(),
                        pointer,
                    )
                })
                .flatten();
            // A group whose rows all sit on the same branch with the same
            // diff says so once, on its header, instead of once per row.
            // Four copies of `pr-818 +94 −26` under one heading describe the
            // repo, not the tabs, and being the only coloured text in the
            // column they were also the loudest thing in it. Read off every
            // row the group counts rather than the ones it draws, so a folded
            // group still names its branch.
            //
            // A lone row is no exception. Its branch describes the same repo
            // the heading above it names, and leaving it down there gave a
            // one-tab group a shape no other group in the column has: a
            // bare heading over a two-line row. It lifts like any other.
            let shared_git: Option<SharedGit> = section.name.as_ref().and_then(|_| {
                let rows = &visible_by_section[group_ix];
                if rows.is_empty() {
                    return None;
                }
                // Only rows that *have* a status get a vote. A tab that was
                // just opened has none until its shell reports a directory
                // and the poll comes back; counting it as a disagreement
                // pulled the branch off the header and grew a branch line
                // under every sibling for the half second it took, then
                // folded them all back — the column jumped twice for every
                // ⌘T. Unknown is not different; it is not yet known.
                let mut known = rows
                    .iter()
                    .filter_map(|&i| Some((i, self.tabs[i].git_status(Some(window), cx)?)));
                let (first, status) = known.next()?;
                let same = known.all(|(_, other)| other == status);
                same.then(|| SharedGit {
                    status,
                    click: git_click(&self.tabs[first], window, cx),
                    rows: rows.clone(),
                })
            });
            let group_badges =
                badge_positions.is_some() && self.mod_hint_group && section.name.is_some();
            let header_badge_pos = group_badges
                .then(|| {
                    let first = *section.tabs.first()?;
                    badge_positions.as_ref()?.get(first).copied().flatten()
                })
                .flatten();
            for (slot, i) in visible.into_iter().enumerate() {
                if agents_mode {
                    for (agent_slot, agent_row) in agent_rows
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.tab == i)
                    {
                        let group_badge = if self.mod_hint_group
                            && section.name.is_none()
                            && agent_rows
                                .iter()
                                .find(|row| section.tabs.contains(&row.tab))
                                .is_some_and(|first| {
                                    first.summary.location.key == agent_row.summary.location.key
                                }) {
                            section
                                .tabs
                                .first()
                                .and_then(|i| badge_positions.as_ref()?.get(*i).copied().flatten())
                        } else {
                            None
                        };
                        let badge = group_badge.or_else(|| {
                            agent_badges
                                .iter()
                                .position(|key| *key == agent_row.summary.location.key)
                        });
                        let row = self.sidebar_agent_row(agent_row, agent_slot, badge, window, cx);
                        let menu_app = cx.entity().downgrade();
                        rows.push(row.context_menu(move |menu, window, cx| {
                            Tty59App::tab_context_menu(menu, i, true, &menu_app, window, cx)
                        }));
                    }
                    continue;
                }
                let badge_pos = if group_badges {
                    None
                } else {
                    badge_positions.as_ref().and_then(|positions| positions[i])
                };
                let show_badge = badge_pos.is_some();
                let tab = &self.tabs[i];
                reveal_drawn |= reveal == Some(tab.tree_id.get());
                let is_active = i == active;
                let ssh_dot = self.tab_ssh_dot(tab, cx);
                let asleep = tab.is_asleep();
                let agent = tab.agent(cx).or_else(|| tab.asleep_agent());
                let agent_status = tab.agent_status(cx);
                let agent_unread = tab.agent_unread_count(cx);
                let git_cwd = git_click(tab, window, cx);
                let zoomed = self.tab_is_zoomed(i);
                // Status rides at the row's trailing end. It stays in the
                // line, and so in the title's budget, whether or not the
                // modifier is held: the badge stands over it (the slot's
                // backing hides it) and the mark itself is made invisible
                // while that is so, never taken out of the layout.
                let status_dot = self
                    .row_status_dot(agent_status, agent_unread, rail_fill, cx)
                    .map(|mark| hold_hidden(mark, show_badge));
                let meta_size = META_REM * rem;
                // Title: the *full* label. The label elides it against the
                // width layout actually gives it, so a wide sidebar shows the
                // whole thing and a narrow one keeps whichever end identifies
                // it — the tail for a path, both edges for anything else. A
                // fixed segment cap (`short_title`) would elide even when the
                // row has room, so only the width may decide here.
                //
                // `full_title` is the unelided string the card can spell out;
                // `None` means the row is showing a placeholder that no card
                // can improve on.
                //
                // The ladder the strip and the switcher climb, read here for
                // the name and not for the shortening. A renamed tab is elided
                // like anything else, and the card spells it back out.
                //
                // An ended pane's own title keeps the words its exit wrote
                // whole and elides only the name in front of them.
                let (title_text, title_tail, full_title) =
                    match self.full_tab_label(tab, Some(window), cx) {
                        Some(label) => {
                            let tail = label.ended_tail;
                            (label.text.clone(), tail, Some(label.text))
                        }
                        // Nothing to expand: the row is naming an unnamed shell,
                        // not hiding a title behind an ellipsis.
                        None => {
                            let placeholder = SharedString::from(t_fmt(
                                L10nKey::TabUnnamedShell,
                                &[("n", &((i + 1).to_string()))],
                            ));
                            (placeholder, None, None)
                        }
                    };
                let git = match shared_git.is_some() {
                    true => None,
                    false => tab.git_status(Some(window), cx),
                };
                let branch_full = git
                    .as_ref()
                    .map(|g| (SharedString::from(g.branch.clone()), g.added, g.removed));
                let git_line = git.map(|g| {
                    let mut line = h_flex()
                        .id(("sidebar-git", i))
                        .w_full()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(meta_size))
                        .text_color(cx.theme().muted_foreground)
                        .font_features(tabular());
                    // Branch: cut from the end, the way any other label is. The
                    // counts are intrinsic siblings that never shrink, so the
                    // branch is cut to whatever they leave.
                    line = line.child(div().flex_1().min_w_0().child(ElidedLabel::new(
                        ("sidebar-row-branch", i),
                        g.branch.clone(),
                        Mode::End,
                    )));
                    if g.added > 0 || g.removed > 0 {
                        let mut counts = h_flex()
                            .id(("sidebar-diff", i))
                            .flex_shrink_0()
                            .items_center()
                            .gap_1p5()
                            .when_some(git_cwd, |counts, (host, cwd)| {
                                // A click target inside a click target: the row
                                // highlights as a whole, which says nothing
                                // about the counts being their own button. The
                                // underline the SFTP breadcrumb uses for
                                // clickable text says where this one starts.
                                counts
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            // Swallowing the press also swallows the
                                            // row's click, the only thing that
                                            // activates a tab — so this row has to
                                            // activate itself, or the overlay lands
                                            // in whichever tab was already on
                                            // screen, carrying this row's repo (#706).
                                            this.activate(i, window, cx);
                                            this.toggle_diff_overlay(host, cwd.clone(), window, cx);
                                        }),
                                    )
                            });
                        if g.added > 0 {
                            counts = counts
                                .child(div().text_color(count_ink).child(format!("+{}", g.added)));
                        }
                        if g.removed > 0 {
                            counts = counts.child(
                                div().text_color(count_ink).child(format!("−{}", g.removed)),
                            );
                        }
                        line = line.child(counts);
                    }
                    line
                });
                // Outside a repo there is no branch line, and the working
                // directory rides on the title's own line rather than growing
                // a second one under it: a group of plain shells was a column
                // of two-line rows describing paths that mostly agree, which
                // is twice the height for a line of small grey text nobody
                // was reading. A row keeps its second line only for a branch.
                let cwd_full: Option<SharedString> = match git_line.is_none()
                    && shared_git.is_none()
                {
                    false => None,
                    true => tab
                        .pane
                        .focused_or_first(window, cx)
                        .and_then(|leaf| {
                            let leaf = leaf.read(cx);
                            Some((leaf.effective_cwd()?, leaf.display_home(cx)))
                        })
                        .map(|(cwd, home)| {
                            let text = cwd.display().to_string();
                            SharedString::from(abbreviate_home(&text, home.as_deref()).into_owned())
                        })
                        // The title already carries the whole path; a second
                        // copy adds noise, not information.
                        .filter(|full| full.as_ref() != title_text.as_ref()),
                };
                let rename_input = self
                    .renaming
                    .as_ref()
                    .filter(|r| r.tab == tab.tree_id.get())
                    .map(|r| r.input.clone());

                let info = self.sidebar_info(
                    tab,
                    window,
                    cx,
                    SidebarInfo {
                        title: full_title,
                        branch: branch_full,
                        cwd: cwd_full.clone(),
                        host: None,
                    },
                );
                // Colors are captured by value so the tooltip builder (which
                // borrows no app state) can style the card on its own.
                let muted = cx.theme().muted_foreground;
                let success = added_ink;
                let danger = removed_ink;

                // A row that grows a branch line under its title stands at
                // `ROW_HEIGHT_TWO_LINE`; a one-line row at `ROW_HEIGHT`.
                let two_line = git_line.is_some();
                let row_h = match two_line {
                    true => ROW_HEIGHT_TWO_LINE,
                    false => ROW_HEIGHT,
                };
                let title_slot = div()
                    .flex_1()
                    .min_w(px(ROW_TITLE_FLOOR))
                    .text_sm()
                    .when(is_active, |d| d.font_weight(FontWeight::MEDIUM))
                    .child({
                        let elided = ElidedLabel::new(
                            ("sidebar-row-title", i),
                            title_text.clone(),
                            Mode::Label,
                        );
                        match title_tail {
                            Some(tail) => elided.tail_included(tail),
                            None => elided,
                        }
                    });
                #[cfg(test)]
                let title_slot = title_slot.debug_selector(|| format!("sidebar-row-title-{i}"));
                let label_region = match rename_input {
                    Some(input) => div()
                        .id(("sidebar-rename", i))
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        // The row switches tabs on the *release* now, so
                        // holding the press back is no longer enough: a click
                        // landing in the field would reach the row behind it
                        // and switch away from the name being typed, taking
                        // the focus with it.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        // No inset of its own: the label it replaces starts
                        // flush, and the field's 12px padding jumped the name
                        // sideways the moment rename began.
                        .child(Input::new(&input).appearance(false).px_0())
                        .into_any_element(),
                    None => v_flex()
                        .id(("sidebar-label", i))
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .when_some(info, |col, info| {
                            col.tooltip(move |window, cx| {
                                // `Tooltip::element` rebuilds its content on
                                // every hover, so the captured info is cloned
                                // per call instead of being moved out.
                                let info = info.clone();
                                gpui_component::tooltip::Tooltip::element(move |_window, _cx| {
                                    v_flex()
                                        .gap_1()
                                        // The card is the one place that
                                        // promised the whole string, so a long
                                        // path wraps here rather than being
                                        // truncated a second time.
                                        .when_some(info.title.clone(), |c, title| {
                                            c.child(
                                                div()
                                                    .max_w(px(420.))
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(title),
                                            )
                                        })
                                        .when_some(
                                            info.branch.clone(),
                                            |c, (branch, added, removed)| {
                                                let mut line = h_flex()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(
                                                        gpui::svg()
                                                            .path("icons/git-branch.svg")
                                                            .flex_shrink_0()
                                                            .size(px(11.))
                                                            .text_color(muted),
                                                    )
                                                    .child(div().child(branch));
                                                if added > 0 {
                                                    line = line.child(
                                                        div()
                                                            .text_color(success)
                                                            .child(format!("+{added}")),
                                                    );
                                                }
                                                if removed > 0 {
                                                    line = line.child(
                                                        div()
                                                            .text_color(danger)
                                                            .child(format!("−{removed}")),
                                                    );
                                                }
                                                c.child(line)
                                            },
                                        )
                                        .when_some(info.cwd.clone(), |c, cwd| {
                                            c.child(
                                                div()
                                                    .max_w(px(420.))
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(cwd),
                                            )
                                        })
                                        .when_some(info.host.clone(), |c, host| {
                                            c.child(
                                                h_flex()
                                                    .items_center()
                                                    .gap_1p5()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(
                                                        gpui::svg()
                                                            .path("icons/machine-remote.svg")
                                                            .flex_shrink_0()
                                                            .size(px(11.))
                                                            .text_color(muted),
                                                    )
                                                    .child(div().truncate().child(host)),
                                            )
                                        })
                                })
                                .build(window, cx)
                            })
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .child(title_slot)
                                // The path takes what it needs up to half the
                                // line and the title keeps the rest, the same
                                // split a group header makes with its branch.
                                // Flex would otherwise hand the overflow to
                                // the two in proportion to what each asked
                                // for, which cuts the longer string hardest.
                                .when_some(cwd_full, |line, cwd| {
                                    let cwd_slot = div()
                                        .flex_shrink(1.)
                                        .min_w(px(ROW_CWD_FLOOR))
                                        .max_w(relative(0.5))
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(ElidedLabel::new(
                                            ("sidebar-row-cwd", i),
                                            cwd,
                                            Mode::Path,
                                        ));
                                    #[cfg(test)]
                                    let cwd_slot =
                                        cwd_slot.debug_selector(|| format!("sidebar-row-cwd-{i}"));
                                    line.child(cwd_slot)
                                }),
                        )
                        .children(git_line)
                        .into_any_element(),
                };

                let row = sidebar_row_chrome(("tab-row", i), is_active, sf)
                    // What a screen reader announces and presses: the full
                    // title, not the elided one drawn.
                    .role(gpui::Role::Tab)
                    .aria_label(SharedString::from(title_text.to_string()))
                    .aria_selected(is_active)
                    .group(SharedString::from(format!("tab-row-{i}")))
                    .cursor_pointer()
                    .on_drag(DragTab, {
                        let state = self.reorder.clone();
                        let slots = row_slots.clone();
                        let group_key = group_key.clone();
                        let id = tab.tree_id.get();
                        move |_drag, grab, _window, cx| {
                            cx.stop_propagation();
                            *state.borrow_mut() = Some(
                                Reorder::new(
                                    Surface::SidebarRows(group_key.clone()),
                                    slot,
                                    slots.borrow().clone(),
                                    Axis::Vertical,
                                    px(ROW_GAP),
                                    grab,
                                )
                                .of_tab(id),
                            );
                            cx.new(|_| DragTab)
                        }
                    })
                    .h(px(row_h))
                    // Faded as well as marked, the way the strip's chip is: the
                    // tabs holding nothing should be the quiet ones in the column.
                    .when(asleep, |s| s.opacity(0.6))
                    .when(row_preview.as_ref().is_some_and(|p| p.from == slot), |s| {
                        s.opacity(0.75)
                    })
                    .child(
                        canvas(
                            {
                                let slots = row_slots.clone();
                                // The row by tab as well as by slot: reordering
                                // reads the slots of one group, a pane dropped
                                // on the sidebar reads every row there is.
                                let by_tab = self.sidebar_slots.clone();
                                let reveal = self.sidebar_reveal.clone();
                                let scroll = self.sidebar_scroll.clone();
                                let id = tab.tree_id.get();
                                // The header sits directly above a group's
                                // first row, one gap away.
                                let lead = match slot == 0 && section.name.is_some() {
                                    true => px(HEADER_HEIGHT + ROW_GAP),
                                    false => px(0.),
                                };
                                move |bounds, window, _cx| {
                                    if let Some(s) = slots.borrow_mut().get_mut(slot) {
                                        *s = bounds;
                                    }
                                    if let Some(s) = by_tab.borrow_mut().get_mut(i) {
                                        *s = bounds;
                                    }
                                    if reveal.get() == Some(id) {
                                        reveal.set(None);
                                        let view = scroll.bounds();
                                        let shift = reveal_shift(
                                            (bounds.top(), bounds.bottom()),
                                            lead,
                                            (view.top(), view.bottom()),
                                        );
                                        if shift != px(0.) {
                                            let mut offset = scroll.offset();
                                            offset.y += shift;
                                            scroll.set_offset(offset);
                                            window.refresh();
                                        }
                                    }
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    // Switched on the release, not the press: a press that turns
                    // into a drag is the tab being picked up, and a tab on its
                    // way into another tab's layout must not put itself on
                    // screen on the way there.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, ev, window, cx| {
                        cx.stop_propagation();
                        this.click_tab(i, ev, window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(i, window, cx);
                        }),
                    )
                    .child(self.tab_avatar_plain(
                        ("sidebar-avatar", i),
                        agent,
                        agent_status,
                        ssh_dot,
                        row_metrics::AVATAR,
                        cx,
                    ))
                    // Leading, like the chip's: the trailing end of a row is
                    // the badge's, and the close button fades in over it.
                    .when(zoomed, |row| {
                        row.child(self.zoom_mark(("sidebar-zoom", i), cx))
                    })
                    .when(asleep, |row| {
                        row.child(self.sleep_mark(("sidebar-asleep", i), cx))
                    })
                    .child(label_region)
                    .children(status_dot)
                    // While the modifier is held the badge takes the trailing
                    // slot the hover × uses, over the row, taking nothing
                    // from the title. It rests on the row's own fill and
                    // turns with it under the pointer.
                    .when_some(badge_pos, |row, badge_pos| {
                        let badge = div()
                            .size(px(row_metrics::BADGE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_active {
                                gpui::rgb(sf.text_selected).into()
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(tab_badge_label(badge_pos));
                        #[cfg(test)]
                        let badge = badge.debug_selector(move || format!("sidebar-row-badge-{i}"));
                        let slot = div()
                            .size(px(crate::ui::tab_strip::MIN_TARGET))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(badge);
                        #[cfg(test)]
                        let slot = slot.debug_selector(move || format!("sidebar-row-hit-{i}"));
                        let (rest, hovered): (gpui::Hsla, gpui::Hsla) = if is_active {
                            (gpui::rgb(sf.selected).into(), gpui::rgb(sf.selected).into())
                        } else {
                            (rail_fill, gpui::rgb(sf.hover).into())
                        };
                        let trailing = trailing_slot(
                            rest,
                            Some((SharedString::from(format!("tab-row-{i}")), hovered)),
                            slot,
                        );
                        #[cfg(test)]
                        let trailing =
                            trailing.debug_selector(move || format!("sidebar-row-trailing-{i}"));
                        row.child(trailing)
                    })
                    .when(!show_badge, |row| {
                        let backing: gpui::Hsla = if is_active {
                            gpui::rgb(sf.selected).into()
                        } else {
                            gpui::rgb(sf.hover).into()
                        };
                        let close = crate::ui::tab_strip::hit_target(
                            Button::new(("sidebar-close", i))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall(),
                        )
                        .tooltip(t(L10nKey::TabContextCloseTab))
                        // Held here, because the row behind it
                        // switches tabs on the release too:
                        // without this the same click closes
                        // tab `i` and then activates whichever
                        // tab slid into its place.
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx);
                            },
                        ));
                        #[cfg(test)]
                        let close = close.debug_selector(move || format!("sidebar-row-hit-{i}"));
                        // The row's full height, not the button's (see
                        // `trailing_slot`): on a two-line row the diff count
                        // sits under the button's band, and a band-high
                        // backing left the bottom of "+2" showing beneath
                        // the ×.
                        let trailing = trailing_slot(backing, None, close)
                            .opacity(0.)
                            .group_hover(SharedString::from(format!("tab-row-{i}")), |s| {
                                s.opacity(1.)
                            });
                        #[cfg(test)]
                        let trailing =
                            trailing.debug_selector(move || format!("sidebar-row-trailing-{i}"));
                        row.child(trailing)
                    });

                let menu_app = cx.entity().downgrade();
                rows.push(row.context_menu(move |menu, window, cx| {
                    Tty59App::tab_context_menu(menu, i, true, &menu_app, window, cx)
                }));
            }

            // An empty pinned group is still drawn: it is kept until deleted.
            if row_count == 0 && section.pinned().is_none() {
                continue;
            }

            let row_display: Vec<usize> = match &row_preview {
                Some(p) => {
                    if let Some(order) =
                        reordered_rows(&keys, groups, &group_key, &visible_tabs, p.from, p.target)
                    {
                        reorder::set_pending(
                            &self.reorder,
                            &Surface::SidebarRows(group_key.clone()),
                            order,
                        );
                    }
                    p.order.clone()
                }
                None => (0..rows.len()).collect(),
            };
            let mut rows: Vec<Option<ContextMenu<Stateful<Div>>>> =
                rows.into_iter().map(Some).collect();
            let rows: Vec<AnyElement> = row_display
                .into_iter()
                .map(|slot| match &row_preview {
                    Some(p) if p.from == slot => deferred(
                        rows[slot]
                            .take()
                            .expect("each slot emitted once")
                            .relative()
                            .top(p.held),
                    )
                    .into_any_element(),
                    Some(p) => {
                        let offset = p.offsets[slot].as_f32();
                        rows[slot]
                            .take()
                            .expect("each slot emitted once")
                            .with_animation(
                                (
                                    SharedString::from(format!("row-slide-{}", p.generation)),
                                    slot,
                                ),
                                Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                                    .with_easing(ease_out_quint()),
                                move |el, delta| el.top(px(offset * (1. - delta))),
                            )
                            .into_any_element()
                    }
                    None => rows[slot]
                        .take()
                        .expect("each slot emitted once")
                        .into_any_element(),
                })
                .collect();
            // A pinned group carries a mark. It is the only thing separating
            // it on sight from a derived one — they behave differently (a
            // `cd` moves a tab out of a repo group and never out of this
            // one), and a group pinned on a real repo would otherwise print a
            // header identical to that repo's.
            let pinned_id = section.pinned();
            let pinned = pinned_id.is_some();
            let pinned_folder: Option<SharedString> = pinned_id
                .and_then(|id| groups.get(id))
                .and_then(|g| g.folder.clone())
                .map(SharedString::from);
            let auto_key_here = group_key.as_ref().and_then(GroupKey::auto).cloned();
            let renaming_group = self
                .group_rename
                .as_ref()
                .filter(|r| Some(r.group) == pinned_id)
                .map(|r| r.input.clone());
            let header = section.name.clone().map(|name| {
                // The header packs a heading and the branch its whole group
                // shares onto one 11px line, and the branch is the unbounded
                // half of it: beside `fix/rpc-proxy-and-error-classification`,
                // `DELTA-NEUTRAL-BOT` came out as `DEL…`. Flex splits an
                // overflow between the two in proportion to how much room each
                // asked for, which is backwards here — the name is what the
                // group *is*, the branch only what it happens to be sitting
                // on. So the branch group may take up to half the line and
                // the name keeps the rest down to a floor; each is a label
                // that elides to the width layout hands it.
                let count_label = row_count.to_string();
                let hover_group = SharedString::from(format!("sidebar-group-{group_ix}"));
                let has_shared_git = shared_git.is_some();
                let is_renaming = renaming_group.is_some();
                let group_is_active = section.tabs.contains(&active);
                let bar = h_flex()
                    .id(("sidebar-group", group_ix))
                    .group(hover_group.clone())
                    .relative()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    // The air above a heading is the list's `GROUP_GAP`;
                    // the heading itself is one short caption line.
                    .h(px(HEADER_HEIGHT))
                    .flex_shrink_0()
                    .text_size(px(header_size))
                    .text_color(cx.theme().muted_foreground)
                    .font_features(tabular())
                    .hover(|s| s.text_color(cx.theme().foreground))
                    .on_click(cx.listener({
                        let key = group_key.clone();
                        move |this, _, _window, cx| this.toggle_sidebar_group(key.as_ref(), cx)
                    }))
                    .when_some(group_slot, |header, slot| {
                        let (surface, slot, slots) = match slot {
                            HeaderSlot::Pinned(s) => {
                                (Surface::PinnedGroups, s, pinned_rects.clone())
                            }
                            HeaderSlot::Auto(s) => (Surface::SidebarGroups, s, group_slots.clone()),
                        };
                        let auto = auto_key_here.clone();
                        crate::ui::reorder::cursor_grab(header).on_drag(DragGroup, {
                            let state = self.reorder.clone();
                            move |_drag, grab, _window, cx| {
                                cx.stop_propagation();
                                let reorder = Reorder::new(
                                    surface.clone(),
                                    slot,
                                    slots.borrow().clone(),
                                    Axis::Vertical,
                                    px(GROUP_GAP),
                                    grab,
                                );
                                // An auto header says which group it is, so
                                // letting it go above the divider can pin it.
                                *state.borrow_mut() = Some(match auto.clone() {
                                    Some(key) => reorder.of_auto(key),
                                    None => reorder,
                                });
                                cx.new(|_| DragGroup)
                            }
                        })
                    })
                    // The heading names the group; the chevron only says
                    // something when there is something behind it. An open
                    // group shows its rows, which is its own answer.
                    .when(folded, |header| {
                        header.child(
                            div()
                                .flex_shrink_0()
                                .child(Icon::new(IconName::ChevronRight).xsmall()),
                        )
                    })
                    // Every kept group carries the mark; it is what sets them
                    // apart from the derived groups below. It is status only,
                    // on a folder group and a label group alike: a glyph that
                    // reads as status is no place for an act with no undo, and
                    // a stray click on it used to scatter a folder's tabs back
                    // into auto groups. Letting go is Unpin (a folder group) or
                    // Delete Group, in the header's menu; a click here folds
                    // the group like the rest of the header.
                    //
                    // Backported from l0ng-ai/tty7 8693c732.
                    .when(pinned, |header| {
                        let tip = if pinned_folder.is_some() {
                            L10nKey::SidebarPinnedFolderMark
                        } else {
                            L10nKey::SidebarPinnedGroupMark
                        };
                        let mark = div()
                            .id(("sidebar-group-pin-mark", group_ix))
                            .flex_shrink_0()
                            .child(Icon::empty().path(PIN_MARK).size(px(PIN_MARK_SIZE)))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(t(tip)).build(window, cx)
                            });
                        #[cfg(test)]
                        let mark = mark
                            .debug_selector(move || format!("sidebar-group-pin-mark-{group_ix}"));
                        header.child(mark)
                    })
                    .child(match renaming_group {
                        Some(input) => div()
                            .id(("sidebar-group-rename", group_ix))
                            .flex_1()
                            .min_w_0()
                            // The header folds the group on click, so a click
                            // landing in the field would shut the very group
                            // whose name is being typed — and take the focus
                            // with it.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(|_, _, cx| cx.stop_propagation())
                            // No inset of its own: the label it replaces starts
                            // flush, and the field's 12px padding jumped the name
                            // sideways the moment rename began.
                            .child(Input::new(&input).appearance(false).px_0())
                            .into_any_element(),
                        None => {
                            // One shared string for the label and the tooltip.
                            let name_text = SharedString::from(name);
                            let name_slot = div()
                                .id(("sidebar-group-name", group_ix))
                                .flex_shrink(1.)
                                .min_w(px(HEADER_NAME_FLOOR))
                                // A caption, not a title: the rows under it are
                                // what the column is for, so the heading keeps
                                // the metadata's grey and only a medium weight
                                // to stand apart from the branch beside it.
                                .font_weight(FontWeight::MEDIUM)
                                .child(ElidedLabel::new(
                                    ("sidebar-group-name-label", group_ix),
                                    name_text.clone(),
                                    Mode::Label,
                                ))
                                // A folder group may have been renamed to
                                // anything; the folder it keeps is what the name
                                // stands for. Any other heading spells itself
                                // out whole.
                                .tooltip({
                                    let full = pinned_folder.clone().unwrap_or(name_text);
                                    move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(full.clone())
                                            .build(window, cx)
                                    }
                                });
                            #[cfg(test)]
                            let name_slot = name_slot
                                .debug_selector(|| format!("sidebar-group-name-{group_ix}"));
                            name_slot.into_any_element()
                        }
                    })
                    .when_some(shared_git, |bar, shared| {
                        let SharedGit {
                            status,
                            click,
                            rows,
                        } = shared;
                        let has_counts = status.added > 0 || status.removed > 0;
                        // Cut from the end, like a row's. The separator rides
                        // with the branch as its tail, so the two appear and
                        // disappear together: a branch that cannot keep
                        // `MIN_HEADER_BRANCH` readable characters is dropped,
                        // and no dot is left pointing at nothing. It stays the
                        // only shrinkable item in this right-pinned group, so
                        // the counts keep their place either way.
                        let mut branch = ElidedLabel::new(
                            ("sidebar-group-branch", group_ix),
                            status.branch.clone(),
                            Mode::End,
                        )
                        .min_visible(MIN_HEADER_BRANCH);
                        if has_counts {
                            branch = branch.tail(META_SEP_TRIMMED);
                        }
                        // The line may give up room, down to its counts: the
                        // branch inside it is the shrinkable part and the counts
                        // never shrink. Held rigid, a header that was exactly
                        // full (the narrowest rail, the heading on its floor,
                        // the line on its half) pushed the counts past the
                        // padding as soon as anything in front of it grew by a
                        // pixel, such as a 12px pin mark.
                        let line = h_flex()
                            .id(("sidebar-group-git", group_ix))
                            .flex_shrink(1.)
                            .min_w_0()
                            .max_w(relative(0.5))
                            .items_center()
                            .gap_1p5()
                            .child(div().flex_shrink(1.).min_w_0().child(branch));
                        #[cfg(test)]
                        let line = line.debug_selector(|| format!("sidebar-group-git-{group_ix}"));
                        let mut line = line;
                        if has_counts {
                            let counts = h_flex()
                                .id(("sidebar-group-diff", group_ix))
                                .flex_shrink_0()
                                .items_center()
                                .gap_1p5()
                                .when_some(click, |counts, (host, cwd)| {
                                    counts
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, _: &MouseDownEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    // The overlay opens over the
                                                    // active tab; make sure that
                                                    // is one of this group's,
                                                    // the same way a row's counts
                                                    // activate their row first.
                                                    if !rows.contains(&this.active)
                                                        && let Some(&first) = rows.first()
                                                    {
                                                        this.activate(first, window, cx);
                                                    }
                                                    this.toggle_diff_overlay(
                                                        host,
                                                        cwd.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ),
                                        )
                                });
                            #[cfg(test)]
                            let counts =
                                counts.debug_selector(|| format!("sidebar-group-diff-{group_ix}"));
                            let mut counts = counts;
                            if status.added > 0 {
                                counts = counts.child(
                                    div()
                                        .text_color(count_ink)
                                        .child(format!("+{}", status.added)),
                                );
                            }
                            if status.removed > 0 {
                                counts = counts.child(
                                    div()
                                        .text_color(count_ink)
                                        .child(format!("−{}", status.removed)),
                                );
                            }
                            line = line.child(counts);
                        }
                        bar.child(div().flex_1()).child(line)
                    })
                    // The count is redundant while the rows are on screen; it
                    // is what a shut group has instead of them.
                    .when(folded, |bar| {
                        bar.child(div().flex_shrink_0().child(count_label))
                    })
                    .when(
                        header_badge_pos.is_some() && !has_shared_git && !is_renaming,
                        |bar| bar.child(div().flex_1()),
                    )
                    .when_some(header_badge_pos, |bar, badge_pos| {
                        bar.child(
                            div()
                                .id(("sidebar-group-badge", group_ix))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if group_is_active {
                                    gpui::rgb(sf.text_selected).into()
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(tab_badge_label(badge_pos)),
                        )
                    })
                    .when(header_badge_pos.is_none(), |bar| {
                        bar.children(self.header_actions(
                            group_ix,
                            group_key.clone(),
                            hover_group,
                            rail_fill,
                            cx,
                        ))
                    });
                #[cfg(test)]
                let bar = bar.debug_selector(|| format!("sidebar-group-{group_ix}"));
                // Renaming is offered on a menu rather than a double click:
                // the first click of a double would fold the group, so the
                // name would be edited on a box that just shut. An auto group
                // offers no rename — it is named after its repo or host, and a
                // rename there could only lie about where its tabs are; pin it
                // first, and it is the user's to name.
                //
                // Attached last and erased to `AnyElement`, because the menu
                // wrapper changes the element's type and the arms have to
                // agree.
                let app = cx.entity().downgrade();
                match (pinned_id, auto_key_here.clone()) {
                    (Some(id), _) => {
                        let has_folder = pinned_folder.is_some();
                        bar.context_menu(move |menu, window, cx| {
                            pinned_header_menu(
                                menu,
                                id,
                                has_folder,
                                workspace_is_local,
                                &app,
                                window,
                                cx,
                            )
                        })
                        .into_any_element()
                    }
                    (None, Some(key)) => bar
                        .context_menu(move |menu, _window, _cx| {
                            auto_header_menu(menu, key.clone(), &app)
                        })
                        .into_any_element(),
                    (None, None) => bar.into_any_element(),
                }
            });

            // A tab in the air makes the difference between the two kinds of
            // group visible: the ones that can take it stay lit, the ones
            // that cannot fade back. Until a drag is under way they look
            // alike, and this is where a user finds out which is which
            // without being told.
            let takes_drops = pinned || (lifted_is_kept && n >= first_unpinned);
            let (preview, slot) = match group_slot {
                Some(HeaderSlot::Pinned(s)) => (pinned_preview.as_ref(), Some(s)),
                Some(HeaderSlot::Auto(s)) => (group_preview.as_ref(), Some(s)),
                None => (None, None),
            };
            // An empty pinned group says how to fill it, in the place its
            // rows would be: a tab can be dragged onto it, or opened here.
            let empty_row = (row_count == 0 && !folded)
                .then_some(pinned_id)
                .flatten()
                .map(|id| {
                    h_flex()
                        .id(("sidebar-group-empty", group_ix))
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .items_center()
                        .gap_2()
                        .pl_2()
                        .rounded(crate::ui::rounding::CARD_RADIUS)
                        .cursor_pointer()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| {
                            s.bg(gpui::rgb(sf.hover))
                                .text_color(cx.theme().sidebar_foreground)
                        })
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .size(px(row_metrics::AVATAR))
                                .child(Icon::empty().path("icons/plus.svg").xsmall()),
                        )
                        .child(t(L10nKey::SidebarGroupNewTab))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_tab_in_group(GroupKey::Pinned(id), window, cx)
                        }))
                });
            let block = v_flex()
                .w_full()
                .gap(px(ROW_GAP))
                .when(preview.is_some_and(|p| Some(p.from) == slot), |b| {
                    b.opacity(0.75)
                })
                .when(lifting_row && !takes_drops, |b| b.opacity(0.4))
                .when(
                    pinned_id.is_some_and(|id| over_group == Some(reorder::Regroup::Into(id))),
                    |b| b.rounded_md().bg(cx.theme().drag_border.opacity(0.15)),
                )
                .children(header)
                .children(rows)
                .children(empty_row)
                .child(
                    canvas(
                        {
                            let slots = match group_slot {
                                Some(HeaderSlot::Pinned(_)) => pinned_rects.clone(),
                                _ => group_slots.clone(),
                            };
                            let landing = self.sidebar_group_slots.clone();
                            // Only a pinned group is recorded, so a drag
                            // looking for somewhere to land finds nothing over
                            // an auto group or over Ungrouped.
                            move |bounds, _window, _cx| {
                                if let Some(slot) = slot
                                    && let Some(s) = slots.borrow_mut().get_mut(slot)
                                {
                                    *s = bounds;
                                }
                                if let Some(id) = pinned_id {
                                    landing.borrow_mut().push((id, bounds));
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );

            any_rows = true;
            list = list.child(match (preview, slot) {
                (Some(p), Some(slot)) if p.from == slot => {
                    deferred(block.relative().top(p.held)).into_any_element()
                }
                (Some(p), Some(slot)) => {
                    let offset = p.offsets[slot].as_f32();
                    block
                        .with_animation(
                            (
                                SharedString::from(format!("group-slide-{}", p.generation)),
                                slot,
                            ),
                            Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                                .with_easing(ease_out_quint()),
                            move |el, delta| el.top(px(offset * (1. - delta))),
                        )
                        .into_any_element()
                }
                _ => block.into_any_element(),
            });
        }
        if !reveal_drawn {
            self.sidebar_reveal.set(None);
        }
        if show_divider && !divider_drawn {
            list = list.child(self.sidebar_divider(divider_lit, divider_zone, cx));
        }

        // An empty list says so where its first row would be, so it goes in
        // before the filler below: appended after it, the filler's `flex_1`
        // pushed the message to the bottom edge of the rail.
        let muted = cx.theme().muted_foreground;
        let empty_state = if agents_mode && agent_rows.is_empty() && query.is_empty() {
            let chord = new_agent_chord(cx);
            let cta = div().pt_1().child(
                Button::new("sidebar-new-agent")
                    .ghost()
                    .small()
                    .accessible_label(t(L10nKey::SidebarNewAgent))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(t(L10nKey::SidebarNewAgent))
                            .children((!chord.is_empty()).then(|| {
                                let caps =
                                    h_flex().gap_1().children(chord.into_iter().map(Kbd::new));
                                #[cfg(test)]
                                let caps = caps.debug_selector(|| "sidebar-new-agent-chord".into());
                                caps
                            })),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.new_agent_tab(window, cx))),
            );
            #[cfg(test)]
            let cta = cta.debug_selector(|| "sidebar-new-agent-cta".into());
            let state = v_flex()
                .items_start()
                .px_2()
                .py_2()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t(L10nKey::SidebarNoAgents)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::SidebarAgentsEmptyHint)),
                )
                .child(cta);
            #[cfg(test)]
            let state = state.debug_selector(|| "sidebar-empty-state".into());
            Some(state)
        } else if !any_rows && !query.is_empty() {
            let line = t_fmt(L10nKey::SidebarNothingMatches, &[("query", &query_text)]);
            #[cfg(test)]
            RENDERED_SIDEBAR_NOTHING_MATCHES.with(|lines| lines.borrow_mut().push(line.clone()));
            let state = v_flex()
                .px_2()
                .py_2()
                .child(div().text_sm().text_color(muted).child(line));
            #[cfg(test)]
            let state = state.debug_selector(|| "sidebar-nothing-matches".into());
            Some(state)
        } else {
            None
        };
        list = list.children(empty_state);

        // The room under the last row. Right-clicking a row opens that tab's
        // menu, so the list's own — new tab, new group — lives here rather
        // than on the list, where it would open over every row's as well.
        // It keeps a row's height when the list scrolls, so there is always
        // somewhere to click.
        list = list.child(
            div()
                .id("tab-sidebar-empty")
                .flex_1()
                .min_h(px(ROW_HEIGHT))
                .context_menu({
                    let app = cx.entity().downgrade();
                    move |menu, _window, _cx| {
                        let new_tab = app.clone();
                        let new_group = app.clone();
                        menu.min_w(px(200.))
                            .item(PopupMenuItem::new(t(L10nKey::AppMenuNewTab)).on_click(
                                move |_, window, cx| {
                                    let _ = new_tab.update(cx, |this, cx| this.new_tab(window, cx));
                                },
                            ))
                            .item(PopupMenuItem::new(t(L10nKey::SidebarNewGroup)).on_click(
                                move |_, window, cx| {
                                    let _ = new_group
                                        .update(cx, |this, cx| this.new_empty_group(window, cx));
                                },
                            ))
                    }
                }),
        );

        // On screen at rest unless the Appearance switch says otherwise; then
        // these two tiles wait for the pointer to reach the rail. The mark
        // stays either way — it names the window rather than doing anything.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let chrome_shown = crate::ui::app::titlebar_chrome_shown(
            auto_hide_chrome,
            self.sidebar_chrome_hover.get(),
        );
        let controls = h_flex()
            .flex_shrink_0()
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .justify_end()
            .gap(px(2.))
            .pr(px(RAIL_INSET))
            .when_some(crate::ui::app::window_mark(), |row, mark| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .pl(px(crate::ui::app::CONTENT_INSET))
                        .child(mark),
                )
                .child(div().flex_1().min_w(px(GRAB_HANDLE_W)))
            })
            .child(
                crate::ui::app::resting_chrome(div().occlude().flex_shrink_0(), chrome_shown)
                    // These are the same controls as the collapsed title bar's
                    // pair. Keep their hit targets and glyphs unchanged when
                    // the sidebar opens, rather than switching to row-sized tiles.
                    .child(self.new_tab_button("sidebar-add", cx)),
            )
            .child(
                crate::ui::app::resting_chrome(div().occlude().flex_shrink_0(), chrome_shown)
                    .child(
                        crate::ui::tab_strip::chrome_tile(
                            Button::new("sidebar-collapse")
                                .icon(Icon::empty().path("icons/panel-left.svg")),
                            false,
                            cx,
                        )
                        .rounded_lg()
                        .accessible_label(t(L10nKey::TabTooltipHideSidebar))
                        .tooltip_element(crate::ui::tab_strip::chord_tooltip(
                            t(L10nKey::TabTooltipHideSidebar),
                            "ToggleLeftPanel",
                            cx,
                        ))
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                    ),
            );
        // The tile inside asks for `w_full`, and a percentage is only a width
        // while some box above it has a real one. This row used to have none of
        // its own and borrowed the column's by cross-axis stretch, which did not
        // always hold; `w_full` here swapped that for a second percentage, and a
        // row whose width is `Percent` is no longer `auto`, so it lost stretch
        // as well — on the passes that size the column from its content there
        // was still nothing to resolve against and the tile fell back to hugging
        // the workspace name. Hand the row real pixels: the rail is
        // `w(px(width))` and layout is border-box, so its content is one pixel
        // narrower than that because of the right border.
        let search = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap(px(7.))
            .h(px(HEAD_CONTROL_HEIGHT))
            .pl_2()
            .pr_1()
            .rounded(px(HEAD_CONTROL_RADIUS))
            .bg(cx.theme().muted)
            .child(
                Icon::new(IconName::Search)
                    .size(px(12.))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.sidebar_search)
                        .appearance(false)
                        .cleanable(true)
                        .pl_0(),
                ),
            );
        #[cfg(test)]
        let search = search.debug_selector(|| "sidebar-search".into());
        let workspace_head = v_flex()
            .w(px(width - 1.))
            .flex_shrink_0()
            .gap(px(10.))
            .px(px(RAIL_INSET))
            .pt(px(4.))
            .pb(px(16.))
            .child(self.workspace_head(cx))
            .child(search);

        let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        // Read while there is still a `cx` to read it from: the drag handler
        // below only ever sees a `Window`, and the cap it clamps against has to
        // be the same one the layout applies or the sidebar springs back from
        // wherever it was dropped.
        let others_floor = self.right_panel_floor(window, cx) + self.document_floor(cx);
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let width_cell = self.sidebar_width.clone();
                let dragging = self.sidebar_dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let raw = (ev.position.x - b.origin.x).as_f32();
                            let max = crate::ui::app::side_panel_max(
                                window.viewport_size().width.as_f32(),
                                MIN_SIDEBAR_WIDTH,
                                others_floor,
                            );
                            width_cell.set(raw.clamp(MIN_SIDEBAR_WIDTH, max));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window, cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            let w = width_cell.get();
                            let cfg = cx.global_mut::<Config>();
                            if cfg.sidebar_width != w {
                                cfg.sidebar_width = w;
                                cfg.save();
                            }
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full();

        let handle_active = self.sidebar_dragging.get();
        let handle = div()
            .group("sidebar-resize")
            .occlude()
            .absolute()
            .top_0()
            .right(px(-(RESIZE_HANDLE_WIDTH / 2.)))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .when(handle_active, |d| d.bg(cx.theme().drag_border))
                    .group_hover("sidebar-resize", |s| s.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.sidebar_dragging.clone();
                move |_ev, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            });

        div()
            .relative()
            .flex_shrink_0()
            .w(px(width))
            .h_full()
            .bg(crate::ui::theme::rail_surface_color(cx))
            .border_r(crate::ui::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .child(backing)
            .child(
                // Real pixels, not `size_full`: the rail's own width is a
                // definite `px`, but a percentage off it is still a percentage,
                // and on the passes that size this column from its content it
                // resolves against nothing. Everything below asks for `w_full`
                // — the tab rows, their group blocks, the scroll area — so one
                // unresolved link here collapsed the whole chain and every row
                // fell back to hugging the longest tab name. Border-box takes
                // the rail's 1px right border off the content width.
                v_flex()
                    .w(px(width - 1.))
                    .h_full()
                    .child(crate::ui::app::title_bar_drag(
                        controls.id("sidebar-titlebar-drag"),
                        "sidebar-titlebar-drag",
                        window,
                        cx,
                    ))
                    .child(workspace_head)
                    .child(crate::ui::scrollbar::with_vertical_scrollbar(
                        "tab-sidebar-scrollbar",
                        list,
                        &self.sidebar_scroll,
                    ))
                    .when(cx.global::<Config>().sidebar_agents_toggle, |rail| {
                        rail.child(self.sidebar_list_toggle(cx))
                    }),
            )
            .child(handle)
            .when(auto_hide_chrome, |rail| {
                rail.child(crate::ui::app::hover_sheet(
                    "sidebar-chrome-hover",
                    &self.sidebar_chrome_hover,
                ))
            })
    }

    /// The boundary between the kept groups and the derived ones, recording
    /// where it lies so a drag next frame can tell which side it is on.
    ///
    /// Not drawn at rest — the mark on each kept header already says which
    /// side a group is on, and a hairline across the list was one more rule
    /// for the eye to cross. It keeps its place in the layout all the same, so
    /// nothing jumps when a drag starts, and shows as a line only while it is
    /// the target: when letting go would pin a header or hand a tab back.
    ///
    /// With nothing pinned yet it only appears while an auto header is being
    /// carried, and then as a drop zone rather than a hairline: a one-pixel
    /// line at the very top of the list is not something anyone could aim
    /// above. `lit` while letting go would pin the header, or hand a kept tab
    /// back to auto grouping.
    fn sidebar_divider(&self, lit: bool, zone: bool, cx: &Context<Self>) -> AnyElement {
        let bounds = self.sidebar_divider.clone();
        let ink = match (lit, zone) {
            (true, _) => cx.theme().drag_border,
            (false, true) => cx.theme().sidebar_border,
            (false, false) => gpui::transparent_black(),
        };
        let body = match zone {
            true => h_flex()
                .h(px(ROW_HEIGHT))
                .w_full()
                .items_center()
                .justify_center()
                .rounded(crate::ui::rounding::CARD_RADIUS)
                .border_1()
                .border_color(ink)
                .when(lit, |d| d.bg(cx.theme().drag_border.opacity(0.15)))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::SidebarDropToPin))
                .into_any_element(),
            false => div()
                .w_full()
                .h(px(if lit { 2. } else { 1. }))
                .bg(ink)
                .into_any_element(),
        };
        v_flex()
            .id("sidebar-divider")
            .relative()
            .w_full()
            .px_2()
            .map(|d| match zone {
                true => d.pt(px(4.)).pb(px(2.)),
                // The rule takes up the gap between two groups instead of
                // adding one of its own: as a child of the list it would
                // otherwise sit between two `GROUP_GAP`s, and kept groups
                // stood more than twice as far from the rest as groups stand
                // from each other.
                false => d.h(px(GROUP_GAP)).my(px(-GROUP_GAP)).justify_center(),
            })
            .child(
                canvas(move |b, _window, _cx| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(body)
            .into_any_element()
    }

    /// The buttons a header shows on hover, laid over its right end: pin for
    /// an auto group, and "+" to open a tab in the group. `None` for
    /// Ungrouped, which has neither.
    ///
    /// Laid over the branch rather than beside it, the way a row's close
    /// button covers its badge, so a header does not give up width it would
    /// only use while the pointer is on it. Acted on at the press, as a row's
    /// diff counts are: the header folds on click and picks itself up on a
    /// drag, and swallowing the press is what keeps both from happening too.
    fn header_actions(
        &self,
        group_ix: usize,
        key: Option<GroupKey>,
        hover: SharedString,
        // The rail's own fill, so the buttons cover what they sit over
        // without a patch of another grey around them.
        backing: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let key = key?;
        let mut fade = backing;
        fade.a = 0.;
        let muted = cx.theme().muted_foreground;
        // Each target is `MIN_TARGET` wide and the header's own height tall;
        // `hit_target` sets the 24 square and only the height is narrowed back,
        // so a target never reaches into the line above or below the header.
        // The glyph is a child at its own size, not `.icon()`: the toolkit sets
        // an icon's size from the button's, and the child keeps both the 12px
        // and the button's centring. Out of the Tab order on purpose: they are
        // revealed by the pointer alone, and a stop that appears nowhere is a
        // stop a keyboard user cannot see. The keyboard path is a later change.
        let button = |id: &'static str, path: &'static str, tip: L10nKey| {
            let glyph = div()
                .flex_none()
                .child(Icon::empty().path(path).size(px(PIN_MARK_SIZE)));
            #[cfg(test)]
            let glyph = glyph.debug_selector(move || format!("{id}-glyph-{group_ix}"));
            let target = crate::ui::tab_strip::hit_target(
                Button::new((id, group_ix))
                    .ghost()
                    .xsmall()
                    .text_color(muted)
                    .child(glyph)
                    .tab_stop(false)
                    .tooltip(t(tip)),
            )
            .h(px(HEADER_HEIGHT));
            #[cfg(test)]
            let target = target.debug_selector(move || format!("{id}-{group_ix}"));
            target
        };
        let mut buttons = h_flex().gap_0p5().bg(backing);
        if let Some(auto) = key.auto().cloned() {
            buttons = buttons.child(
                // The mark the group will carry once kept.
                button("sidebar-group-pin", PIN_MARK, L10nKey::SidebarPinGroup).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        #[cfg(test)]
                        header_action_probe::record("pin");
                        this.pin_auto_group(auto.clone(), cx);
                    }),
                ),
            );
        }
        if !matches!(key, GroupKey::Auto(AutoKey::SshHost(_))) {
            buttons = buttons.child(
                button(
                    "sidebar-group-add",
                    "icons/plus.svg",
                    L10nKey::SidebarGroupNewTab,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        #[cfg(test)]
                        header_action_probe::record("add");
                        this.new_tab_in_group(key.clone(), window, cx);
                    }),
                ),
            );
        }
        Some(
            h_flex()
                .absolute()
                .right(px(4.))
                // The strip is the header's height, so it needs no offset.
                .top_0()
                .opacity(0.)
                .group_hover(hover, |s| s.opacity(1.))
                .child(div().w(px(10.)).h(px(HEADER_HEIGHT)).bg(linear_gradient(
                    90.,
                    linear_color_stop(fade, 0.),
                    linear_color_stop(backing, 1.),
                )))
                .child(buttons)
                .into_any_element(),
        )
    }

    /// What the sidebar row's card spells out: the full title, the full branch
    /// and diff counts, the working directory, and the remote host the avatar
    /// only dots. `None` for a row with nothing to say. The host is included
    /// even for a row that cut nothing, because the title strips the
    /// `user@host:` prefix the avatar cannot spell out.
    fn sidebar_info(
        &self,
        tab: &crate::ui::app::Tab,
        window: &mut Window,
        cx: &gpui::App,
        mut info: SidebarInfo,
    ) -> Option<SidebarInfo> {
        // The host is read off the same leaf the title and cwd came from; a
        // split tab whose panes sit on different machines would otherwise
        // name whichever one happens to be first.
        if let Some(target) = tab.pane.focused_or_first(window, cx).and_then(|leaf| {
            leaf.read(cx)
                .remote_context()
                .map(|r| SharedString::from(r.target.clone()))
        }) {
            info.host = Some(target);
        }
        (info.title.is_some() || info.branch.is_some() || info.cwd.is_some() || info.host.is_some())
            .then_some(info)
    }

    /// Where a tab being dragged in the sidebar would land if let go now,
    /// when that is somewhere other than a new place in its own group.
    ///
    /// Over a pinned group it did not come from, into that group; anywhere
    /// below the divider, back to auto grouping — but only for a tab that is
    /// in a pinned group, since one that is not already is auto grouped and
    /// a drag that would change nothing offers nothing, falling back to plain
    /// reordering.
    ///
    /// No auto group is a target. Its membership is decided by its tabs'
    /// cwds, so "put this tab in tty59" is a request the sidebar has no honest
    /// way to honour; the only thing a drop below the divider can mean is
    /// "stop keeping this tab by hand".
    fn sidebar_regroup_target(&self, window: &Window) -> Option<reorder::Regroup> {
        let dragged = crate::ui::reorder::dragged_sidebar_tab(&self.reorder)?;
        let here = self
            .tabs
            .iter()
            .find(|t| t.tree_id.get() == dragged)
            .and_then(|t| t.group.get())
            .filter(|g| self.sidebar_groups.contains(*g));
        let pointer = window.mouse_position();
        if let Some((id, _)) = self
            .sidebar_group_slots
            .borrow()
            .iter()
            .find(|(_, bounds)| bounds.contains(&pointer))
        {
            return (Some(*id) != here).then_some(reorder::Regroup::Into(*id));
        }
        let below = self
            .sidebar_divider
            .get()
            .is_some_and(|d| pointer.y >= d.origin.y);
        (below && here.is_some()).then_some(reorder::Regroup::ToAuto)
    }

    /// The pinned groups there are, in sidebar order, with the name each
    /// header reads — for menus that offer them.
    pub(crate) fn pinned_group_names(&self) -> Vec<(GroupId, String)> {
        let names = pinned_names(&self.sidebar_groups.pinned);
        self.sidebar_groups
            .pinned
            .iter()
            .map(|g| g.id)
            .zip(names)
            .collect()
    }

    /// Change this workspace's groups, and send the change to the machine so
    /// every other window onto the workspace draws it too.
    pub(crate) fn edit_groups(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut WorkspaceGroups),
    ) {
        edit(&mut self.sidebar_groups);
        // Up as one `WorkspaceSetGroups`, queued ahead of any `TabSetGroup`
        // the same edit made — which the save below sends.
        crate::ui::tree_sync::push_groups(cx, self.workspace, self.sidebar_groups.clone());
        self.save_session(cx);
        cx.notify();
    }

    /// Take the workspace's groups as another window, or the machine, has them.
    ///
    /// Every tab's [`EntryWatch`](crate::core::group_key::EntryWatch) starts
    /// over from where it is. A folder that arrives from elsewhere is not one
    /// this window's tabs walked into — and in particular, a window that drew
    /// a frame before its copy of the groups landed saw no folders at all, so
    /// without this every tab sitting in a pinned folder at launch would read
    /// the groups landing as an entry, and a tab dragged out of its folder
    /// group would be pulled back in on every start. The window that pinned
    /// the folder gathers the tabs itself and says so tab by tab.
    pub(crate) fn adopt_sidebar_groups(&mut self, groups: WorkspaceGroups, cx: &mut Context<Self>) {
        if self.sidebar_groups == groups {
            return;
        }
        self.sidebar_groups = groups;
        for tab in &self.tabs {
            tab.folder_watch
                .set(crate::core::group_key::EntryWatch::baseline());
        }
        cx.notify();
    }

    /// Put the dragged tab where it was dropped.
    ///
    /// By id rather than index: a drag is several frames long, and a tab
    /// closing anywhere else in that time would shift every index after it.
    pub(crate) fn regroup_tab(
        &mut self,
        tab: tty59_core::core::machine::TabId,
        target: reorder::Regroup,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|t| t.tree_id.get() == tab) else {
            return;
        };
        let group = match target {
            reorder::Regroup::Into(id) => Some(id),
            reorder::Regroup::ToAuto => None,
        };
        self.set_tab_group(index, group, cx);
    }

    /// Put tab `index` in pinned group `group`, or hand it back to auto
    /// grouping when `group` is `None`.
    pub(crate) fn set_tab_group(
        &mut self,
        index: usize,
        group: Option<GroupId>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        tab.group.set(group);
        // Carries the move to the daemon as a `TabSetGroup`, so another
        // window on the same workspace sees it too.
        self.save_session(cx);
        cx.notify();
    }

    /// Put tab `index` in group `key`. An auto group's members are decided by
    /// their cwds, so a tab goes into one by pinning the group, as its
    /// header's pin does — the tab joins in the same edit.
    pub(crate) fn move_tab_to(&mut self, index: usize, key: GroupKey, cx: &mut Context<Self>) {
        match key {
            GroupKey::Pinned(id) => self.set_tab_group(index, Some(id), cx),
            GroupKey::Auto(auto) => self.pin_auto_group_with(auto, Some(index), cx),
        }
    }

    /// A name no pinned group is using yet, for a group about to be made.
    ///
    /// The placeholder only has to be unique — the rename box opens on it
    /// selected, so the first keystroke replaces it. Unique still matters:
    /// two headers reading the same name are two groups nobody can tell
    /// apart.
    fn fresh_group_name(&self) -> String {
        let taken: Vec<String> = self
            .pinned_group_names()
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        let base = t(L10nKey::SidebarNewGroupName).to_string();
        (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => format!("{base} {n}"),
            })
            .find(|candidate| !taken.contains(candidate))
            .expect("an unbounded range always reaches an untaken name")
    }

    /// Make a new label group, put tab `index` in it, and open its header for
    /// renaming.
    ///
    /// No dialog: the tab is in the group before a character is typed, so
    /// what the name is being given to is on screen while it is chosen.
    pub(crate) fn new_tab_group(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group = PinnedGroup::label(self.fresh_group_name());
        let id = group.id;
        if let Some(tab) = self.tabs.get(index) {
            tab.group.set(Some(id));
        }
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        self.start_group_rename(id, window, cx);
    }

    /// Make a new, empty label group and open its header for renaming — the
    /// palette's "New Group". It stays on screen empty until a tab is dragged
    /// into it or opened from its header.
    pub(crate) fn new_empty_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let group = PinnedGroup::label(self.fresh_group_name());
        let id = group.id;
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        self.start_group_rename(id, window, cx);
    }

    /// Keep the auto group `key`: pin it, with the tabs in it now.
    ///
    /// A repo group becomes a folder group on the repo's home, so tabs that
    /// walk into the repo later join it too, and it reads the repo's name
    /// until renamed. A host group has no folder on this workspace's machine
    /// to keep, so it becomes a label group named after the host.
    ///
    /// The fold comes along: a group that was shut stays shut, rather than
    /// springing open in its new place.
    pub(crate) fn pin_auto_group(&mut self, key: AutoKey, cx: &mut Context<Self>) {
        self.pin_auto_group_with(key, None, cx);
    }

    /// [`pin_auto_group`](Self::pin_auto_group), taking tab `also` into the
    /// new group in the same edit.
    fn pin_auto_group_with(&mut self, key: AutoKey, also: Option<usize>, cx: &mut Context<Self>) {
        let mut group = match &key {
            AutoKey::Repo(root) => PinnedGroup::folder(root),
            AutoKey::SshHost(host) => PinnedGroup::label(host.clone()),
        };
        group.collapsed = self.sidebar_groups.auto_collapsed.contains(&key);
        // A repo whose folder is already kept — a tab dragged out of that
        // group while still in the repo is drawn under the repo again — joins
        // the kept group rather than making a second: two groups keeping one
        // folder split its tabs between them by nothing but list order.
        let kept = group.folder.as_deref().and_then(|folder| {
            self.sidebar_groups
                .pinned
                .iter()
                .find(|g| g.folder.as_deref() == Some(folder))
                .map(|g| g.id)
        });
        let id = kept.unwrap_or(group.id);
        let wanted = Some(GroupKey::Auto(key.clone()));
        for (i, (tab, place)) in self
            .tabs
            .iter()
            .zip(self.sidebar_group_keys(cx))
            .enumerate()
        {
            if place == wanted || also == Some(i) {
                tab.group.set(Some(id));
            }
        }
        self.edit_groups(cx, |groups| {
            groups.auto_collapsed.retain(|k| *k != key);
            if kept.is_none() {
                groups.pinned.push(group);
            }
        });
    }

    /// Pin `folder` as a group of its own — a folder dropped from Finder, or
    /// "Pin as Group" in the file tree. Pinning one that is already pinned
    /// does nothing: two groups keeping one folder would split its tabs
    /// between them by nothing but list order.
    ///
    /// Tabs already sitting in the folder are gathered in on the next frame,
    /// the way a tab walking in would be: the folder is new, so each of them
    /// has just "entered" it as far as [`EntryWatch`](crate::core::group_key::EntryWatch) can tell.
    pub(crate) fn pin_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) -> GroupId {
        let spelled = folder.to_string_lossy();
        if let Some(kept) = self
            .sidebar_groups
            .pinned
            .iter()
            .find(|g| g.folder.as_deref() == Some(&*spelled))
        {
            return kept.id;
        }
        let group = PinnedGroup::folder(&folder);
        let id = group.id;
        self.edit_groups(cx, |groups| groups.pinned.push(group));
        id
    }

    /// The palette's "Open Folder as Group…": pick a folder on this computer,
    /// pin it, and open a tab in it — "open" is the promise, and an empty group
    /// with nothing in it would not keep it.
    pub(crate) fn open_folder_as_group(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let id = this.pin_folder(path, cx);
                    this.new_tab_in_group(GroupKey::Pinned(id), window, cx);
                });
            }
        })
        .detach();
    }

    /// Folders dropped on the sidebar from Finder or Explorer, pinned as
    /// groups. Anything that is not a directory is let fall: a file names no
    /// group, and guessing its parent would pin something nobody pointed at.
    ///
    /// Only on a workspace on this computer. A dropped path is this
    /// machine's, and a folder group keeps a directory on the workspace's
    /// host — on a remote one the same spelling names something else, or
    /// nothing at all.
    pub(crate) fn pin_dropped_folders(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if crate::core::session::WorkspaceStore::all(cx)
            .get(self.workspace)
            .is_some_and(|w| w.is_remote())
        {
            return;
        }
        for path in paths.iter().filter(|p| p.is_dir()) {
            self.pin_folder(path.clone(), cx);
        }
    }

    /// Delete pinned group `id`. Its tabs are not closed — they go back to
    /// auto grouping, which is where a tab nobody filed by hand belongs.
    ///
    /// Unpinning a folder group is the same act: the group stops being kept,
    /// and its tabs fall back to the auto groups their cwds resolve to.
    pub(crate) fn delete_group(&mut self, id: GroupId, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if tab.group.get() == Some(id) {
                tab.group.set(None);
            }
        }
        if self.group_rename.as_ref().is_some_and(|r| r.group == id) {
            self.group_rename = None;
        }
        self.edit_groups(cx, |groups| groups.pinned.retain(|g| g.id != id));
    }

    /// Point pinned group `id` at `folder`, or make it a label group when
    /// `folder` is `None`.
    ///
    /// A folder group nobody renamed reads its folder's name, and would read
    /// nothing once the folder is gone — so clearing the folder keeps the
    /// name it was showing.
    pub(crate) fn set_group_folder(
        &mut self,
        id: GroupId,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let shown = self
            .pinned_group_names()
            .into_iter()
            .find(|(g, _)| *g == id)
            .map(|(_, n)| n);
        self.edit_groups(cx, |groups| {
            let Some(group) = groups.get_mut(id) else {
                return;
            };
            if folder.is_none() && group.given_name().is_none() {
                group.name = shown;
            }
            group.folder = folder.map(|f| f.to_string_lossy().into_owned());
        });
    }

    /// Ask for a folder for pinned group `id` with the system picker. Only
    /// for a workspace on this machine: the picker browses this computer, and
    /// a path picked here means nothing to a remote one.
    pub(crate) fn pick_group_folder(&mut self, id: GroupId, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.set_group_folder(id, Some(path), cx));
            }
        })
        .detach();
    }

    /// The directory the active tab is working in, on the workspace's host —
    /// what "Use Current Tab's Folder" pins a group to.
    pub(crate) fn active_tab_folder(&self, window: &Window, cx: &gpui::App) -> Option<PathBuf> {
        let leaf = self
            .tabs
            .get(self.active)?
            .pane
            .focused_or_first(window, cx)?;
        leaf.read(cx).effective_host_cwd()
    }

    /// Open a tab in the group drawn under `key` — a header's "+" and its
    /// menu's "New Tab".
    ///
    /// A folder group opens in its folder, a repo group in its repo; a label
    /// group has no directory of its own and opens where ⌘T would. The tab
    /// joins a pinned group outright rather than waiting for its cwd to walk
    /// in: a label group has nothing to walk into, and "new tab here" should
    /// not depend on a probe.
    pub(crate) fn new_tab_in_group(
        &mut self,
        key: GroupKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.tabs.len();
        let folder = match &key {
            GroupKey::Pinned(id) => self
                .sidebar_groups
                .get(*id)
                .and_then(|g| g.folder_path().map(Path::to_path_buf)),
            GroupKey::Auto(AutoKey::Repo(root)) => Some(root.clone()),
            GroupKey::Auto(AutoKey::SshHost(_)) => None,
        };
        match folder {
            Some(folder) => self.new_tab_at(folder, window, cx),
            None => self.new_tab(window, cx),
        }
        if self.tabs.len() == before {
            return;
        }
        if let GroupKey::Pinned(id) = key
            && let Some(tab) = self.tabs.get(self.active)
        {
            tab.group.set(Some(id));
            self.save_session(cx);
        }
    }

    /// Put the pinned groups in `order` — indices into the list as it stands.
    pub(crate) fn apply_pinned_order(&mut self, order: &[usize], cx: &mut Context<Self>) {
        let pinned = &self.sidebar_groups.pinned;
        if order.len() != pinned.len() || order.iter().enumerate().all(|(i, &o)| i == o) {
            return;
        }
        let reordered: Vec<PinnedGroup> = order
            .iter()
            .filter_map(|&i| pinned.get(i).cloned())
            .collect();
        if reordered.len() != pinned.len() {
            return;
        }
        self.edit_groups(cx, |groups| groups.pinned = reordered);
    }

    /// Open the header of pinned group `id` for renaming.
    pub(crate) fn start_group_rename(
        &mut self,
        id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .pinned_group_names()
            .into_iter()
            .find(|(g, _)| *g == id)
            .map(|(_, n)| n)
        else {
            return;
        };
        let input = Self::rename_box(current, window, cx);
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.commit_group_rename(window, cx)
                }
                _ => {}
            },
        )];
        self.group_rename = Some(crate::ui::app::GroupRename {
            group: id,
            input,
            _subs: subs,
        });
        cx.notify();
    }

    /// Write the typed name onto the group being renamed.
    pub(crate) fn commit_group_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.group_rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_string();
        self.edit_groups(cx, |groups| {
            let Some(group) = groups.get_mut(rename.group) else {
                return;
            };
            match (value.is_empty(), group.folder.is_some()) {
                // A folder group handed a blank name goes back to reading
                // its folder's — which is how a rename is undone.
                (true, true) => group.name = None,
                // A label group has nothing to fall back on: a blank name
                // would draw an unlabelled header, so it keeps the one it had.
                (true, false) => {}
                (false, _) => group.name = Some(value),
            }
        });
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Fold the group drawn under `key` (`None`: Ungrouped), or unfold it if
    /// it is already shut. Stored with the workspace: a group folded away is a
    /// statement about work you are done with for now, and every window onto
    /// the workspace — and the next launch — should still show it shut.
    pub(crate) fn toggle_sidebar_group(&mut self, key: Option<&GroupKey>, cx: &mut Context<Self>) {
        self.edit_groups(cx, |groups| groups.toggle_folded(key));
    }

    /// Rule 2: a tab whose cwd walks into a pinned folder joins that group.
    ///
    /// Run once per frame, before anything asks where a tab is drawn. Every
    /// tab's [`EntryWatch`](crate::core::group_key::EntryWatch) is fed whether or not the tab is in a pinned group
    /// — that is what lets a tab dragged out of a folder group while it is
    /// still inside the folder stay out: the watch already knows it is
    /// inside, so staying there is no entry. Only a tab not already kept in a
    /// pinned group acts on an entry; a kept tab never leaves on its own.
    ///
    /// A tab is only looked at once its repo probe has answered. Before that
    /// the question "is it inside" has half an answer — the cwd without the
    /// repo home a worktree is matched by — and recording that half as where
    /// the tab *was* would read the other half landing a frame later as an
    /// entry, pulling every restored worktree tab into its group at launch.
    ///
    /// SSH panes are never looked at: their cwd is on another machine, and a
    /// folder pinned in this workspace names a directory on this one.
    ///
    /// Also where a tab's auto-group hint is refreshed from the live answer
    /// and, when that answer moved, sent up with the tab — so the next launch
    /// draws the tab where it last was instead of in Ungrouped.
    pub(crate) fn settle_sidebar_groups(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for tab in &self.tabs {
            let before = tab.auto_group.borrow().clone();
            changed |= self.tab_auto_group(tab, cx) != before;
        }
        let Some(cache) = cx.try_global::<GitStatusCache>() else {
            if changed {
                self.save_session(cx);
            }
            return;
        };
        let mut joined = changed;
        for tab in &self.tabs {
            let Some(leaf) = tab.pane.first_leaf() else {
                continue;
            };
            let Some(view) = leaf.terminal() else {
                continue;
            };
            let view = view.read(cx);
            if view.remote_context().is_some() {
                continue;
            }
            let Some(cwd) = view.git_status_cwd() else {
                continue;
            };
            let Some(home) = cache.known_repo_for(view.host_id(), cwd) else {
                continue;
            };
            let inside = self.sidebar_groups.folder_for(Some(cwd), home.as_deref());
            let mut watch = tab.folder_watch.get();
            let entered = watch.observe(inside);
            tab.folder_watch.set(watch);
            let kept = tab
                .group
                .get()
                .is_some_and(|g| self.sidebar_groups.contains(g));
            if let Some(id) = entered
                && !kept
            {
                tab.group.set(Some(id));
                joined = true;
            }
        }
        if joined {
            self.save_session(cx);
        }
    }

    /// The auto group tab `tab` resolves to (rule 3): the host for an SSH
    /// pane, otherwise the repo its cwd is in, otherwise none (Ungrouped).
    ///
    /// Remembered on the tab, and the memory is what answers while there is
    /// nothing to go on — a probe in flight, or a pane (a WSL one, say) whose
    /// paths no probe reaches — so a tab does not bounce through Ungrouped
    /// between one answer and the next.
    fn tab_auto_group(&self, tab: &Tab, cx: &gpui::App) -> Option<AutoKey> {
        let resolved = tab.pane.first_leaf().and_then(|leaf| {
            let view = leaf.terminal()?.read(cx);
            let host = ssh_host(view.remote_context().as_ref());
            let known = view.git_status_cwd().and_then(|cwd| {
                cx.try_global::<GitStatusCache>()?
                    .known_repo_for(view.host_id(), cwd)
            });
            auto_key(host.as_deref(), known)
        });
        if let Some(key) = resolved {
            *tab.auto_group.borrow_mut() = key;
        }
        tab.auto_group.borrow().clone()
    }

    /// Where each tab is drawn, in `self.tabs` order: its pinned group, its
    /// auto group, or `None` for Ungrouped (or for the flat list below the
    /// pinned groups, with auto grouping off).
    pub(crate) fn sidebar_group_keys(&self, cx: &gpui::App) -> Vec<Option<GroupKey>> {
        let auto_grouping = cx.global::<Config>().sidebar_auto_grouping;
        self.tabs
            .iter()
            .map(|tab| {
                // Resolved even with auto grouping off, so the remembered
                // answer is warm the moment it is turned back on.
                let auto = self.tab_auto_group(tab, cx);
                place(tab.group.get(), &self.sidebar_groups, auto_grouping, auto)
            })
            .collect()
    }

    pub(crate) fn visual_tab_order(&self, cx: &gpui::App) -> Vec<usize> {
        if cx.global::<Config>().tab_bar_position != crate::core::config::TabBarPosition::Left {
            return (0..self.tabs.len()).collect();
        }
        let keys = self.sidebar_group_keys(cx);
        sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .flat_map(|s| s.tabs)
            .collect()
    }

    /// The tabs drawn in the same run as `index`, in `self.tabs` order: its
    /// sidebar section on a left tab bar, every tab on a top one. Empty when
    /// no run holds `index`.
    pub(crate) fn tab_run(&self, index: usize, cx: &gpui::App) -> Vec<usize> {
        if cx.global::<Config>().tab_bar_position != crate::core::config::TabBarPosition::Left {
            return (0..self.tabs.len()).collect();
        }
        let keys = self.sidebar_group_keys(cx);
        sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .map(|s| s.tabs)
            .find(|tabs| tabs.contains(&index))
            .unwrap_or_default()
    }

    pub(crate) fn activate_visual(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_agents_mode(cx) {
            if let Some(row) = self.sidebar_agent_shortcut_rows(cx).get(n) {
                self.activate_sidebar_agent(row.summary.location.key, window, cx);
            }
            return;
        }
        let order = if cx.global::<Config>().tab_group_shortcuts {
            self.active_group_tab_order(cx)
        } else {
            self.visual_tab_order(cx)
        };
        if let Some(&i) = order.get(n) {
            self.activate(i, window, cx);
        }
    }

    /// Each non-empty group's tabs in visual order.
    pub(crate) fn group_tab_orders(&self, cx: &gpui::App) -> Vec<Vec<usize>> {
        let keys = self.sidebar_group_keys(cx);
        sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .map(|s| s.tabs)
            .filter(|tabs| !tabs.is_empty())
            .collect()
    }

    /// The active tab's group in display order. `None` is a real key here:
    /// ungrouped tabs form a section too, and number within it just like a
    /// pinned or automatic group. This deliberately does not use `tab_run`,
    /// which means "every tab" on a top strip; group scoping is independent of
    /// where the tabs are drawn.
    pub(crate) fn active_group_tab_order(&self, cx: &gpui::App) -> Vec<usize> {
        let keys = self.sidebar_group_keys(cx);
        let Some(active_group) = keys.get(self.active) else {
            return Vec::new();
        };
        (0..keys.len())
            .filter(|&i| keys.get(i) == Some(active_group))
            .collect()
    }

    /// Badge positions derived from the sidebar's already-resolved sections, so
    /// rendering a held modifier does not resolve `sidebar_group_keys` or
    /// rebuild `sidebar_sections` a second time per frame.
    fn sidebar_badge_positions(
        &self,
        sections: &[Section],
        cx: &gpui::App,
    ) -> Option<Vec<Option<usize>>> {
        let group_shortcuts = cx.global::<Config>().tab_group_shortcuts;
        if !self.mod_hint_badges || (self.mod_hint_group && !group_shortcuts) {
            return None;
        }
        let positions = if self.mod_hint_group {
            let agents = self
                .sidebar_agents_mode(cx)
                .then(|| self.sidebar_agent_rows(&self.agent_pane_summaries(cx), cx));
            badge_positions_from_order(
                self.tabs.len(),
                sections
                    .iter()
                    .filter(|s| {
                        agents
                            .as_ref()
                            .is_none_or(|rows| rows.iter().any(|row| s.tabs.contains(&row.tab)))
                    })
                    .filter_map(|s| s.tabs.first().copied()),
            )
        } else if group_shortcuts {
            let active_tabs = sections
                .iter()
                .find(|s| s.tabs.contains(&self.active))
                .map(|s| s.tabs.as_slice())
                .unwrap_or(&[]);
            badge_positions_from_order(self.tabs.len(), active_tabs.iter().copied())
        } else {
            badge_positions_from_order(
                self.tabs.len(),
                sections.iter().flat_map(|s| s.tabs.iter().copied()),
            )
        };
        Some(positions)
    }

    pub(crate) fn activate_group_visual(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cfg = cx.global::<Config>();
        if !cfg.tab_group_shortcuts {
            return;
        }
        let activation = cfg.tab_group_activation;
        let agents = self
            .sidebar_agents_mode(cx)
            .then(|| self.sidebar_agent_rows(&self.agent_pane_summaries(cx), cx));
        let keys = self.sidebar_group_keys(cx);
        let Some(section) = sidebar_sections(&keys, &self.sidebar_groups)
            .into_iter()
            .filter_map(|mut section| {
                if let Some(rows) = &agents {
                    section
                        .tabs
                        .retain(|i| rows.iter().any(|row| row.tab == *i));
                }
                (!section.tabs.is_empty()).then_some(section)
            })
            .nth(n)
        else {
            return;
        };
        let target = match activation {
            crate::core::config::TabGroupActivation::First => section.tabs.first().copied(),
            crate::core::config::TabGroupActivation::Recent => self
                .tabs_by_mru()
                .into_iter()
                .find(|&i| {
                    section.tabs.contains(&i)
                        && (i == self.active
                            || self.tabs.get(i).is_some_and(|t| t.last_used.get() > 0))
                })
                .or_else(|| section.tabs.first().copied()),
        };
        if let Some(i) = target {
            if self.sidebar_groups.is_folded(section.key.as_ref()) {
                self.toggle_sidebar_group(section.key.as_ref(), cx);
            }
            if let Some(rows) = agents {
                let remembered = self.tabs[i]
                    .focus_target()
                    .and_then(|slot| slot.terminal().cloned())
                    .map(|leaf| {
                        let view = leaf.read(cx);
                        PaneKey {
                            host: view.host_id(),
                            pane: view.pane_id,
                        }
                    });
                if let Some(row) = rows
                    .iter()
                    .find(|row| row.tab == i && Some(row.summary.location.key) == remembered)
                    .or_else(|| rows.iter().find(|row| row.tab == i))
                {
                    self.activate_sidebar_agent(row.summary.location.key, window, cx);
                }
            } else {
                self.activate(i, window, cx);
            }
        }
    }

    /// Where a tab about to be spawned in `cwd` goes.
    ///
    /// A tab spawned from one kept in a pinned group joins it, before the cwd
    /// is consulted at all — the cwd says nothing about a group the user
    /// stated by hand. Every caller here spawns from the active tab (the ones
    /// that start from a named tab activate it first), so that is the one to
    /// inherit from. Without this, ⌘T inside a folded group would draw
    /// nothing but the header's count going up by one.
    ///
    /// Otherwise a cwd inside a pinned folder joins that folder's group, and
    /// the auto group is seeded from the repo cache when its probe for that
    /// directory has already landed. A tab's auto group otherwise starts
    /// empty and only fills in once its shell has reported a cwd, which parks
    /// every new tab in Ungrouped at the bottom of the sidebar until then; a
    /// tab spawned from one already in a repo inherits a warm cache, so this
    /// lands it in its group on the first frame.
    pub(crate) fn spawn_group(&self, cwd: Option<&Path>, cx: &gpui::App) -> SpawnPlace {
        let kept = self
            .tabs
            .get(self.active)
            .and_then(|t| t.group.get())
            .filter(|g| self.sidebar_groups.contains(*g));
        let known = cwd.and_then(|cwd| {
            let host = self
                .window_workspace(cx)
                .as_ref()
                .map_or(crate::ui::host_ops::HostId::LOCAL, |ws| ws.target.host_id());
            cx.try_global::<GitStatusCache>()?.known_repo_for(host, cwd)
        });
        let entered = cwd.and_then(|cwd| {
            let home = known.clone().flatten();
            self.sidebar_groups.folder_for(Some(cwd), home.as_deref())
        });
        SpawnPlace {
            group: kept.or(entered),
            auto: auto_key(None, known),
        }
    }
}

/// A header that can be picked up, and which of the two orders it belongs to.
#[derive(Clone, Copy)]
enum HeaderSlot {
    /// Slot `n` among the pinned headers, above the divider.
    Pinned(usize),
    /// Slot `n` among the auto headers, below it.
    Auto(usize),
}

/// One item of a group header's menu, acting on the app when it is chosen.
fn menu_item(
    label: L10nKey,
    app: &gpui::WeakEntity<Tty59App>,
    act: impl Fn(&mut Tty59App, &mut Window, &mut Context<Tty59App>) + 'static,
) -> PopupMenuItem {
    let app = app.clone();
    PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
        let _ = app.update(cx, |this, cx| act(this, window, cx));
    })
}

/// A pinned group's header menu. Everything about a kept group is the
/// user's to change: its name, the folder it keeps, and whether it is kept at
/// all. Deleting closes nothing — its tabs go back to auto grouping.
///
/// "Set Folder…" asks the system picker, which browses this computer, so it
/// is only offered for a workspace on it; "Use Current Tab's Folder" works
/// anywhere, since the tab's cwd is on the workspace's own host.
fn pinned_header_menu(
    menu: gpui_component::menu::PopupMenu,
    id: GroupId,
    has_folder: bool,
    local: bool,
    app: &gpui::WeakEntity<Tty59App>,
    window: &Window,
    cx: &gpui::App,
) -> gpui_component::menu::PopupMenu {
    let current = app
        .upgrade()
        .and_then(|a| a.read(cx).active_tab_folder(window, cx));
    let mut menu = menu.min_w(px(200.)).item(menu_item(
        L10nKey::SidebarRenameGroup,
        app,
        move |this, w, cx| this.start_group_rename(id, w, cx),
    ));
    if local {
        menu = menu.item(menu_item(
            L10nKey::SidebarSetGroupFolder,
            app,
            move |this, _, cx| this.pick_group_folder(id, cx),
        ));
    }
    menu = menu.item(
        menu_item(
            L10nKey::SidebarUseCurrentTabFolder,
            app,
            move |this, w, cx| {
                if let Some(folder) = this.active_tab_folder(w, cx) {
                    this.set_group_folder(id, Some(folder), cx);
                }
            },
        )
        .disabled(current.is_none()),
    );
    if has_folder {
        menu = menu.item(menu_item(
            L10nKey::SidebarClearGroupFolder,
            app,
            move |this, _, cx| this.set_group_folder(id, None, cx),
        ));
    }
    menu = menu
        .separator()
        .item(menu_item(
            L10nKey::SidebarGroupNewTab,
            app,
            move |this, w, cx| this.new_tab_in_group(GroupKey::Pinned(id), w, cx),
        ))
        .separator();
    if has_folder {
        menu = menu.item(menu_item(
            L10nKey::SidebarUnpinGroup,
            app,
            move |this, _, cx| this.delete_group(id, cx),
        ));
    }
    menu.item(menu_item(
        L10nKey::SidebarDeleteGroup,
        app,
        move |this, _, cx| this.delete_group(id, cx),
    ))
}

/// An auto group's header menu: pin it, or open a tab in it. Nothing else —
/// an auto group is derived, so there is nothing about it to edit until it
/// is pinned, and pinning is always an explicit act.
fn auto_header_menu(
    menu: gpui_component::menu::PopupMenu,
    key: AutoKey,
    app: &gpui::WeakEntity<Tty59App>,
) -> gpui_component::menu::PopupMenu {
    let pin = key.clone();
    let mut menu = menu.min_w(px(200.)).item(menu_item(
        L10nKey::SidebarPinGroup,
        app,
        move |this, _, cx| this.pin_auto_group(pin.clone(), cx),
    ));
    // A host group has no directory to open a tab in, and no way to open a
    // shell on the host that would not guess at how the others got there.
    if let AutoKey::Repo(_) = &key {
        menu = menu.item(menu_item(
            L10nKey::SidebarGroupNewTab,
            app,
            move |this, w, cx| this.new_tab_in_group(GroupKey::Auto(key.clone()), w, cx),
        ));
    }
    menu
}

/// Where a tab about to be spawned goes, worked out before it exists — see
/// [`Tty59App::spawn_group`].
pub(crate) struct SpawnPlace {
    group: Option<GroupId>,
    auto: Option<Option<AutoKey>>,
}

impl SpawnPlace {
    /// Seed the auto group with SSH host `host`, for a tab dialling a machine
    /// whose pane has not reported its remote context yet.
    pub(crate) fn on_host(mut self, host: Option<String>) -> Self {
        if let Some(auto) = auto_key(host.as_deref(), None) {
            self.auto = Some(auto);
        }
        self
    }

    /// Put `tab` where this says.
    pub(crate) fn seat(&self, tab: &Tab) {
        if let Some(id) = self.group {
            tab.group.set(Some(id));
        }
        if let Some(auto) = &self.auto {
            *tab.auto_group.borrow_mut() = auto.clone();
        }
    }
}

/// The host an SSH pane is on, when it is one.
///
/// A native SSH pane and a shell that has ssh'd onward from a local prompt
/// are grouped alike, by the target they name: in neither case does a `Host`
/// of ours reach the far side, so neither has a repo to probe — the first
/// reports a remote cwd that names nothing here, the second only the local
/// directory the `ssh` was typed in, which says nothing about where its shell
/// now is. The host is the one thing about either that is certain. A WSL pane
/// is not an SSH one: its distro is on this machine, and it keeps the group it
/// had.
fn ssh_host(remote: Option<&crate::daemon::protocol::RemoteContext>) -> Option<String> {
    use crate::daemon::protocol::RemoteKind;
    remote
        .filter(|r| matches!(r.kind, RemoteKind::Ssh | RemoteKind::NativeSsh))
        .map(|r| r.target.clone())
}

#[derive(Debug, PartialEq)]
struct Section {
    key: Option<GroupKey>,
    name: Option<String>,
    tabs: Vec<usize>,
}

impl Section {
    fn pinned(&self) -> Option<GroupId> {
        self.key.as_ref().and_then(GroupKey::pinned)
    }
}

/// How far to move the sidebar's scroll offset so a newly active row shows.
/// A row with any part in view stays put — clicking a row must not move the
/// list under the pointer; one out of view comes in at the nearest edge.
/// `lead` is what sits on top of the row and belongs with it — its group's
/// header, for the first row — so a row coming in from above brings the
/// name of the group it is in, not just itself.
fn reveal_shift(row: (Pixels, Pixels), lead: Pixels, view: (Pixels, Pixels)) -> Pixels {
    let ((top, bottom), (view_top, view_bottom)) = (row, view);
    if bottom <= view_top {
        view_top - (top - lead)
    } else if top >= view_bottom {
        view_bottom - bottom
    } else {
        px(0.)
    }
}

/// The sidebar's sections, top to bottom: every pinned group in the user's
/// order — an empty one too, since a kept group stays until it is deleted —
/// then the auto groups in the order their first tab appears, then
/// Ungrouped.
///
/// Ungrouped only draws a header beside other groups below the divider. With
/// no auto group to set it apart from — grouping off, or nothing resolved
/// yet — it is just the list, and a header over the whole of it would be a
/// label on nothing. The same goes for a sidebar with no groups at all, which
/// comes out as one headerless section holding every tab.
fn sidebar_sections(keys: &[Option<GroupKey>], groups: &WorkspaceGroups) -> Vec<Section> {
    let members = |key: &GroupKey| -> Vec<usize> {
        (0..keys.len())
            .filter(|&i| keys[i].as_ref() == Some(key))
            .collect()
    };
    let mut auto_order: Vec<&AutoKey> = Vec::new();
    for k in keys.iter().flatten().filter_map(GroupKey::auto) {
        if !auto_order.contains(&k) {
            auto_order.push(k);
        }
    }
    let rest: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].is_none()).collect();
    if groups.pinned.is_empty() && auto_order.is_empty() {
        return vec![Section {
            key: None,
            name: None,
            tabs: rest,
        }];
    }
    let pinned = pinned_names(&groups.pinned);
    let auto = auto_names(&auto_order);
    let mut sections: Vec<Section> = groups
        .pinned
        .iter()
        .zip(pinned)
        .map(|(g, name)| {
            let key = GroupKey::Pinned(g.id);
            Section {
                tabs: members(&key),
                key: Some(key),
                name: Some(name),
            }
        })
        .collect();
    sections.extend(auto_order.iter().zip(auto).map(|(k, name)| {
        let key = GroupKey::Auto((*k).clone());
        Section {
            tabs: members(&key),
            key: Some(key),
            name: Some(name),
        }
    }));
    if !rest.is_empty() {
        sections.push(Section {
            key: None,
            name: (!auto_order.is_empty()).then(|| t(L10nKey::SidebarUngroupedGroup).to_string()),
            tabs: rest,
        });
    }
    sections
}

/// A row of the tab menu's "Move to Group".
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MoveTarget {
    pub key: GroupKey,
    pub name: String,
    pub checked: bool,
}

/// Every group the sidebar draws, in its order — pinned and auto alike —
/// with the one tab `index` is in checked.
pub(crate) fn move_targets(
    keys: &[Option<GroupKey>],
    groups: &WorkspaceGroups,
    index: usize,
) -> Vec<MoveTarget> {
    let here = keys.get(index).cloned().flatten();
    sidebar_sections(keys, groups)
        .into_iter()
        .filter_map(|s| {
            let key = s.key?;
            Some(MoveTarget {
                checked: here.as_ref() == Some(&key),
                name: s.name.expect("a section with a key has a header"),
                key,
            })
        })
        .collect()
}

fn reordered_rows(
    keys: &[Option<GroupKey>],
    groups: &WorkspaceGroups,
    group: &Option<GroupKey>,
    visible: &[usize],
    from: usize,
    to: usize,
) -> Option<Vec<usize>> {
    let (&moved, &anchor) = (visible.get(from)?, visible.get(to)?);
    if moved == anchor {
        return None;
    }
    let mut members: Vec<usize> = (0..keys.len()).filter(|&i| keys[i] == *group).collect();
    members.retain(|&i| i != moved);
    let at = members.iter().position(|&i| i == anchor)? + usize::from(to > from);
    members.insert(at, moved);

    let mut out: Vec<usize> = Vec::with_capacity(keys.len());
    for g in sidebar_sections(keys, groups).iter().map(|s| &s.key) {
        if g == group {
            out.extend_from_slice(&members);
        } else {
            out.extend((0..keys.len()).filter(|&i| keys[i] == *g));
        }
    }
    Some(out)
}

/// The tab order that moves auto group `from` to where auto group `to` is.
///
/// Auto groups are ordered by where their first tab sits, so moving one is
/// moving its tabs. Pinned groups keep an order of their own and take no part:
/// their tabs follow the auto groups' here, where they draw no differently.
fn regrouped_order(keys: &[Option<GroupKey>], from: &AutoKey, to: &AutoKey) -> Option<Vec<usize>> {
    if from == to {
        return None;
    }
    let mut order: Vec<&AutoKey> = Vec::new();
    for k in keys.iter().flatten().filter_map(GroupKey::auto) {
        if !order.contains(&k) {
            order.push(k);
        }
    }
    let fi = order.iter().position(|g| *g == from)?;
    let ti = order.iter().position(|g| *g == to)?;
    let moved = order.remove(fi);
    order.insert(ti, moved);

    let auto_of = |i: usize| keys[i].as_ref().and_then(GroupKey::auto);
    let mut out: Vec<usize> = Vec::with_capacity(keys.len());
    for g in &order {
        out.extend((0..keys.len()).filter(|&i| auto_of(i) == Some(*g)));
    }
    out.extend((0..keys.len()).filter(|&i| auto_of(i).is_none()));
    Some(out)
}

/// The order of all pinned groups after the header in visible slot `from` is
/// dropped on slot `to`, as indices into `all`. `shown` is the pinned groups
/// on screen, in order — a search can hide some, and those keep their places.
fn reordered_pinned(
    all: &[GroupId],
    shown: &[GroupId],
    from: usize,
    to: usize,
) -> Option<Vec<usize>> {
    let (&moved, &anchor) = (shown.get(from)?, shown.get(to)?);
    if moved == anchor {
        return None;
    }
    let mut order: Vec<GroupId> = all.to_vec();
    order.retain(|g| *g != moved);
    let at = order.iter().position(|g| *g == anchor)? + usize::from(to > from);
    order.insert(at, moved);
    order
        .iter()
        .map(|g| all.iter().position(|a| a == g))
        .collect()
}

/// Where a click on a tab's diff counts opens the overlay: the focused pane's
/// repo.
fn git_click(
    tab: &Tab,
    window: &Window,
    cx: &gpui::App,
) -> Option<(crate::ui::host_ops::HostId, PathBuf)> {
    tab.pane.focused_or_first(window, cx).and_then(|leaf| {
        let view = leaf.read(cx);
        let cwd = view.git_status_cwd()?.to_path_buf();
        Some((view.host_id(), cwd))
    })
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    /// Bounds a row registered for itself while it was on screen. A folded
    /// row leaves the default rectangle behind, and that is what stops a pane
    /// being dropped into a group that is shut.
    fn drawn(app: &Tty59App, i: usize) -> bool {
        app.sidebar_slots.borrow()[i].size.height > px(0.)
    }

    /// Put tab `i` in a directory and tell the cache that directory is in the
    /// repo `root`, whose home is `home` — the same as `root` unless the
    /// checkout is a linked worktree. Both halves are needed: the sidebar
    /// reads the tab's cwd and looks it up in the cache, and either one
    /// missing makes it return "no decision", which would leave every group
    /// below untouched and every assertion about moving vacuous.
    fn plant(app: &Tty59App, i: usize, cwd: &str, root: &str, home: &str, cx: &mut gpui::App) {
        plant_on(app, i, cwd, (root, home), ("main", (0, 0)), cx);
    }

    /// [`plant`] with the branch the cache answers and its diff counts.
    fn plant_on(
        app: &Tty59App,
        i: usize,
        cwd: &str,
        (root, home): (&str, &str),
        (branch, counts): (&str, (u32, u32)),
        cx: &mut gpui::App,
    ) {
        use crate::terminal::git_status::{GitStatusCache, RepoSnapshot};
        use crate::ui::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: PathBuf::from(root),
                    home: PathBuf::from(home),
                    branch: branch.into(),
                    counts: Some(counts),
                }),
            );
        });
    }

    fn plant_repo(app: &Tty59App, i: usize, cwd: &str, root: &str, cx: &mut gpui::App) {
        plant(app, i, cwd, root, root, cx);
    }

    /// Put tab `i` somewhere the cache knows is in no repo at all.
    fn plant_plain(app: &Tty59App, i: usize, cwd: &str, cx: &mut gpui::App) {
        use crate::terminal::git_status::GitStatusCache;
        use crate::ui::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(HostId::LOCAL, &cwd, None);
        });
    }

    fn repo(s: &str) -> Option<GroupKey> {
        Some(GroupKey::Auto(AutoKey::Repo(PathBuf::from(s))))
    }

    /// Pin a label group called `name`, answering its id.
    fn label(app: &mut Tty59App, name: &str, cx: &mut Context<Tty59App>) -> GroupId {
        let group = PinnedGroup::label(name);
        let id = group.id;
        app.edit_groups(cx, |groups| groups.pinned.push(group));
        id
    }

    /// Pin `folder`, answering the group's id.
    fn folder(app: &mut Tty59App, folder: &str, cx: &mut Context<Tty59App>) -> GroupId {
        app.pin_folder(PathBuf::from(folder), cx);
        app.sidebar_groups
            .pinned
            .iter()
            .find(|g| g.folder.as_deref() == Some(folder))
            .expect("just pinned")
            .id
    }

    #[gpui::test]
    fn group_numbers_stay_in_the_active_group_on_a_top_strip(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 5);
        app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            let other = label(app, "other", cx);
            for i in [0, 2, 4] {
                app.set_tab_group(i, Some(work), cx);
            }
            for i in [1, 3] {
                app.set_tab_group(i, Some(other), cx);
            }
        });
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.tab_group_shortcuts = true;
            cfg.tab_bar_position = crate::core::config::TabBarPosition::Top;
            cx.set_global(cfg);
            crate::ui::keymap::rebind(cx);
        });

        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.active_group_tab_order(cx), [0, 2, 4]);
            assert_eq!(app.group_tab_orders(cx), vec![vec![0, 2, 4], vec![1, 3]]);
            app.mod_hint_badges = true;
            app.mod_hint_group = false;
            assert_eq!(
                app.tab_badge_positions(cx).unwrap(),
                [Some(0), None, Some(1), None, Some(2)],
                "the tab hold numbers only the active group, starting at one"
            );
            app.mod_hint_group = true;
            assert_eq!(
                app.tab_badge_positions(cx).unwrap(),
                [Some(0), Some(1), None, None, None],
                "the group hold numbers the first tab of each group"
            );
            app.mod_hint_badges = false;
        });

        let tab_chord = if cfg!(target_os = "macos") {
            "secondary-2"
        } else {
            "alt-2"
        };
        vcx.simulate_keystrokes(tab_chord);
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| app.active),
            2,
            "the ordinary tab chord scopes to the active group while enabled"
        );

        let group_chord = if cfg!(target_os = "macos") {
            "secondary-shift-2"
        } else {
            "alt-shift-2"
        };
        vcx.simulate_keystrokes(group_chord);
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| app.active),
            1,
            "the group chord selects the second group, defaulting to its first tab when unused"
        );

        app.update_in(&mut vcx, |app, window, cx| {
            app.activate_visual(1, window, cx);
            assert_eq!(app.active, 3, "second tab in the second group");
            app.activate_visual(2, window, cx);
            assert_eq!(app.active, 3, "there is no third tab in the second group");

            // Switching back to group 1 picks its most-recently focused tab (tab 2).
            app.activate_group_visual(0, window, cx);
            assert_eq!(app.active, 2);

            // Switching to group 2 picks its most-recently focused tab (tab 3).
            app.activate_group_visual(1, window, cx);
            assert_eq!(app.active, 3);
        });

        vcx.update(|_, cx| {
            cx.global_mut::<Config>().tab_group_activation =
                crate::core::config::TabGroupActivation::First;
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.activate_group_visual(0, window, cx);
            assert_eq!(
                app.active, 0,
                "First mode selects the first tab of the group rather than the recent one"
            );
        });

        vcx.update(|_, cx| cx.global_mut::<Config>().tab_group_shortcuts = false);
        app.update_in(&mut vcx, |app, window, cx| {
            app.activate_group_visual(1, window, cx);
            assert_eq!(
                app.active, 0,
                "a dispatched group action is inert while off"
            );
            app.activate_visual(1, window, cx);
            assert_eq!(
                app.active, 1,
                "ordinary tab chords return to global order when off"
            );
        });
    }

    #[gpui::test]
    fn group_numbers_follow_auto_groups_and_ungrouped_tabs(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 5);
        vcx.update(|_, cx| cx.global_mut::<Config>().tab_group_shortcuts = true);
        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/alpha/one", "/w/alpha", cx);
            plant_repo(app, 1, "/w/beta/one", "/w/beta", cx);
            plant_repo(app, 2, "/w/alpha/two", "/w/alpha", cx);
            plant_plain(app, 3, "/plain/three", cx);
            plant_plain(app, 4, "/plain/four", cx);
        });

        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.active_group_tab_order(cx), [0, 2]);
            assert_eq!(
                app.group_tab_orders(cx),
                vec![vec![0, 2], vec![1], vec![3, 4]]
            );
            app.activate_visual(1, window, cx);
            assert_eq!(app.active, 2);

            // Select group 3 (Ungrouped) and then its second tab.
            app.activate_group_visual(2, window, cx);
            assert_eq!(app.active, 3);
            assert_eq!(
                app.active_group_tab_order(cx),
                [3, 4],
                "ungrouped is a section with its own numbering too"
            );
            app.activate_visual(1, window, cx);
            assert_eq!(app.active, 4);
        });
    }

    /// Rule 1. A tab kept in a pinned group is not the probe's to move:
    /// without this the cwd would drag a hand-placed tab back into its repo
    /// on the very next frame, and no amount of clicking would keep it where
    /// it was put.
    ///
    /// Tab 1 is the control. It is kept nowhere, so the same probe that must
    /// leave tab 0 alone has to file tab 1 — otherwise this test would pass
    /// just as well with the probe switched off entirely.
    #[gpui::test]
    fn a_probe_files_an_auto_tab_and_never_a_pinned_one(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            for i in 0..2 {
                plant_repo(app, i, "/w/probed/sub", "/w/probed", cx);
            }
            cx.notify();
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys[0],
                Some(GroupKey::Pinned(work)),
                "the pinned group survived a probe that had a real answer"
            );
            assert_eq!(
                keys[1],
                repo("/w/probed"),
                "and that same probe did file the tab that was kept nowhere"
            );
        });
    }

    /// Dropping a tab below the divider is the way out of a pinned group: the
    /// cwd decides again.
    #[gpui::test]
    fn a_tab_handed_back_to_auto_follows_its_cwd_again(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let (work, dragged) = app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            (work, app.tabs[0].tree_id.get())
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.sidebar_group_keys(cx)[0], Some(GroupKey::Pinned(work)));
        });

        app.update(&mut vcx, |app, cx| {
            app.regroup_tab(dragged, reorder::Regroup::ToAuto, cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                repo("/w/probed"),
                "dropped below the divider, so the probe takes the tab back"
            );
        });
    }

    /// A drag is several frames long, so the tab it is carrying is named by
    /// id. Were it an index, any tab closing before the drop — in another
    /// window on the same workspace, or by a shell exiting — would shift it,
    /// and the drop would land on whichever tab slid into that slot.
    #[gpui::test]
    fn a_drop_finds_its_tab_after_the_indexes_shift(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        let dragged = app.update(&mut vcx, |app, _| app.tabs[2].tree_id.get());
        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.close_tab(0, window, cx);
            app.regroup_tab(dragged, reorder::Regroup::Into(work), cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let moved = app
                .tabs
                .iter()
                .find(|t| t.tree_id.get() == dragged)
                .expect("the dragged tab is still open");
            assert_eq!(
                moved.group.get(),
                Some(work),
                "the tab that was picked up is the tab that moved"
            );
            assert!(
                app.tabs
                    .iter()
                    .filter(|t| t.tree_id.get() != dragged)
                    .all(|t| t.group.get().is_none()),
                "and no bystander was regrouped in its place"
            );
        });
    }

    /// ⌘T inside a pinned group has to land in it. Otherwise the new tab
    /// goes wherever its cwd says, and if the group it was opened from is
    /// folded, the only thing that happens on screen is the header's count
    /// going up by one — the symptom #804 fixed for repo groups.
    #[gpui::test]
    fn a_tab_spawned_inside_a_pinned_group_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let work = app.update(&mut vcx, |app, cx| {
            // A probe that would send the tab somewhere else if it were
            // consulted, so this cannot pass by there being no answer.
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            app.active = 0;
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(Some(&PathBuf::from("/w/probed/sub")), cx);
            assert_eq!(
                place.group,
                Some(work),
                "the pinned group is inherited ahead of anything the cwd says"
            );
        });

        app.update(&mut vcx, |app, cx| app.set_tab_group(0, None, cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(Some(&PathBuf::from("/w/probed/sub")), cx);
            assert_eq!(place.group, None, "with nothing kept, nothing inherited");
            assert_eq!(
                place.auto,
                Some(Some(AutoKey::Repo(PathBuf::from("/w/probed")))),
                "and the cwd seeds the auto group from the warm cache"
            );
        });
    }

    /// ⌘T from an SSH tab dials the same host, and the new tab is filed
    /// under that host before its pane has reported where it is — not in
    /// Ungrouped, which is where the local shell it used to open landed.
    #[gpui::test]
    fn a_tab_dialling_a_host_is_filed_under_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let place = app.spawn_group(None, cx).on_host(Some("hermes_ali".into()));
            assert_eq!(
                place.auto,
                Some(Some(AutoKey::SshHost("hermes_ali".into())))
            );
            let place = app.spawn_group(None, cx).on_host(None);
            assert_eq!(place.auto, None, "no host, no opinion");
        });
    }

    /// A tab opened in a pinned folder is filed there before its shell has
    /// said a word — from the cwd it was opened in.
    #[gpui::test]
    fn a_tab_spawned_in_a_pinned_folder_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let tty59 = folder(app, "/w/tty59", cx);
            let place = app.spawn_group(Some(&PathBuf::from("/w/tty59/src")), cx);
            assert_eq!(place.group, Some(tty59));
            let place = app.spawn_group(Some(&PathBuf::from("/w/else")), cx);
            assert_eq!(place.group, None);
        });
    }

    /// A rename lands on the group itself: its tabs point at it by id, so
    /// nothing about them has to change.
    #[gpui::test]
    fn renaming_a_group_keeps_its_tabs_in_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            app.set_tab_group(1, Some(work), cx);
            app.start_group_rename(work, window, cx);
            app.group_rename
                .as_ref()
                .expect("the box is up")
                .input
                .update(cx, |state, cx| state.set_value("urgent", window, cx));
            app.commit_group_rename(window, cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.pinned_group_names(), vec![(work, "urgent".to_string())]);
            assert!(app.tabs.iter().all(|t| t.group.get() == Some(work)));
        });
    }

    /// Clearing the box and dismissing it must not leave a label group with
    /// no name to draw; a folder group, which has its folder's name to fall
    /// back on, goes back to reading it.
    #[gpui::test]
    fn a_blank_rename_keeps_a_label_and_resets_a_folder(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            let tty59 = folder(app, "/w/tty59", cx);
            app.edit_groups(cx, |groups| {
                groups.get_mut(tty59).expect("pinned").name = Some("mine".into())
            });
            for id in [work, tty59] {
                app.start_group_rename(id, window, cx);
                app.group_rename
                    .as_ref()
                    .expect("the box is up")
                    .input
                    .update(cx, |state, cx| state.set_value("   ", window, cx));
                app.commit_group_rename(window, cx);
            }
            assert_eq!(
                app.pinned_group_names(),
                vec![(work, "work".to_string()), (tty59, "tty59".to_string())],
                "the label kept its name; the folder went back to its own"
            );
        });
    }

    /// The placeholder only has to be unique — the box opens selected, so
    /// the first keystroke replaces it. But two headers with one name are two
    /// groups nobody can tell apart.
    #[gpui::test]
    fn a_second_new_group_does_not_land_on_the_first(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            app.new_tab_group(0, window, cx);
            app.commit_group_rename(window, cx);
            app.new_tab_group(1, window, cx);
            app.commit_group_rename(window, cx);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let names = app.pinned_group_names();
            assert_eq!(names.len(), 2, "two groups, not one shared by both tabs");
            assert_ne!(names[0].1, names[1].1);
            assert_ne!(app.tabs[0].group.get(), app.tabs[1].group.get());
        });
    }

    /// "Auto grouping off" hides the groups the sidebar works out, and
    /// Ungrouped's header with them — but a pinned group is the user's, and
    /// still stands.
    #[gpui::test]
    fn auto_grouping_off_hides_auto_groups_and_keeps_pinned_ones(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            app.set_tab_group(0, Some(work), cx);
            plant_repo(app, 1, "/w/alpha", "/w/alpha", cx);
            plant_plain(app, 2, "/tmp", cx);
            let mut cfg = cx.global::<Config>().clone();
            cfg.sidebar_auto_grouping = false;
            cx.set_global(cfg);
            cx.notify();
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(keys, vec![Some(GroupKey::Pinned(work)), None, None]);
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(
                sections.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                vec![Some("work".into()), None],
                "the pinned header, then a flat list with no header over it"
            );
        });

        app.update(&mut vcx, |_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.sidebar_auto_grouping = true;
            cx.set_global(cfg);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys,
                vec![Some(GroupKey::Pinned(work)), repo("/w/alpha"), None]
            );
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(
                sections.last().and_then(|s| s.name.clone()),
                Some("Ungrouped".into()),
                "beside an auto group, the rest is Ungrouped again"
            );
        });
    }

    /// Rule 2: a tab walking into a pinned folder joins it.
    #[gpui::test]
    fn a_tab_walking_into_a_pinned_folder_joins_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let probed = app.update(&mut vcx, |app, cx| {
            plant_plain(app, 0, "/w/elsewhere", cx);
            plant_plain(app, 1, "/w/elsewhere", cx);
            let probed = folder(app, "/w/probed", cx);
            cx.notify();
            probed
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), Some(probed), "walked in: joined");
            assert_eq!(app.tabs[1].group.get(), None, "stayed out: did not");
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                Some(GroupKey::Pinned(probed)),
                "and the pinned folder beats the repo root it sits in"
            );
        });
    }

    /// Nested folders: the deepest one a tab is inside wins, so pinning a
    /// monorepo and a package in it files the package's tabs under the
    /// package.
    #[gpui::test]
    fn nested_pinned_folders_file_a_tab_under_the_deepest(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let (mono, pkg) = app.update(&mut vcx, |app, cx| {
            let mono = folder(app, "/w/mono", cx);
            let pkg = folder(app, "/w/mono/pkg", cx);
            plant_repo(app, 0, "/w/mono/pkg/src", "/w/mono", cx);
            plant_repo(app, 1, "/w/mono/docs", "/w/mono", cx);
            cx.notify();
            (mono, pkg)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(pkg));
            assert_eq!(app.tabs[1].group.get(), Some(mono));
        });
    }

    /// A linked worktree lives outside its main checkout, but its repo home
    /// is that checkout — so pinning the repo keeps its worktrees too.
    #[gpui::test]
    fn a_worktree_joins_the_folder_its_repo_home_is_pinned_as(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let tty59 = app.update(&mut vcx, |app, cx| {
            let tty59 = folder(app, "/w/tty59", cx);
            plant(
                app,
                0,
                "/tmp/wt/feature/src",
                "/tmp/wt/feature",
                "/w/tty59",
                cx,
            );
            cx.notify();
            tty59
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(tty59));
        });
    }

    /// The edge the whole rule turns on. A tab dragged out of a folder group
    /// while it is still in the folder has been told where to go; it must not
    /// be pulled straight back, and only walking out and in again rejoins it.
    #[gpui::test]
    fn a_tab_dragged_out_is_not_pulled_back_until_it_re_enters(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let probed = app.update(&mut vcx, |app, cx| {
            let probed = folder(app, "/w/probed", cx);
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
            probed
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.tabs[0].group.get(),
                Some(probed),
                "joined on the way in"
            );
            app.set_tab_group(0, None, cx);
        });
        for _ in 0..3 {
            app.update(&mut vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), None, "still inside: stays out");
            plant_plain(app, 0, "/w/elsewhere", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs[0].group.get(), None, "left: nothing to join");
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), Some(probed), "re-entered: joined");
        });
    }

    /// A tab restored from the machine tree was filed by whoever had it
    /// last. Sitting in a folder at launch is not walking into it.
    #[gpui::test]
    fn a_restored_tab_is_not_pulled_in_by_where_it_already_is(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            app.tabs[0]
                .folder_watch
                .set(crate::core::group_key::EntryWatch::baseline());
            folder(app, "/w/probed", cx);
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), None);
        });
    }

    /// Deleting a group closes nothing: its tabs go back to auto grouping.
    #[gpui::test]
    fn deleting_a_group_returns_its_tabs_to_auto_grouping(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let work = app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            plant_repo(app, 0, "/w/alpha", "/w/alpha", cx);
            app.set_tab_group(0, Some(work), cx);
            app.set_tab_group(1, Some(work), cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| app.delete_group(work, cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(app.tabs.len(), 2, "no tab was closed");
            assert!(app.sidebar_groups.pinned.is_empty());
            assert!(app.tabs.iter().all(|t| t.group.get().is_none()));
            assert_eq!(app.sidebar_group_keys(cx)[0], repo("/w/alpha"));
        });
    }

    /// Pinning an auto group keeps the tabs in it now, keeps its fold, and —
    /// for a repo — keeps its folder, so tabs walking in later join too.
    #[gpui::test]
    fn pinning_an_auto_group_keeps_its_tabs_and_its_folder(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/alpha", "/w/alpha", cx);
            plant_repo(app, 1, "/w/alpha/src", "/w/alpha", cx);
            plant_repo(app, 2, "/w/beta", "/w/beta", cx);
            app.toggle_sidebar_group(repo("/w/alpha").as_ref(), cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            app.pin_auto_group(AutoKey::Repo(PathBuf::from("/w/alpha")), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let group = app.sidebar_groups.pinned.first().expect("pinned");
            assert_eq!(group.folder.as_deref(), Some("/w/alpha"));
            assert!(group.collapsed, "the fold came along");
            assert!(app.sidebar_groups.auto_collapsed.is_empty());
            assert_eq!(app.tabs[0].group.get(), Some(group.id));
            assert_eq!(app.tabs[1].group.get(), Some(group.id));
            assert_eq!(app.tabs[2].group.get(), None, "beta stays auto");
        });
    }

    /// Moving a tab into an auto group pins that group, in one edit: the
    /// group's own tabs and the moved one end up in a single folder group.
    #[gpui::test]
    fn moving_a_tab_into_an_auto_group_pins_it_with_the_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let r = AutoKey::Repo(PathBuf::from("/w/r"));
        app.update(&mut vcx, |app, cx| {
            for tab in &app.tabs[..2] {
                *tab.auto_group.borrow_mut() = Some(r.clone());
            }
            *app.tabs[2].auto_group.borrow_mut() = Some(AutoKey::Repo(PathBuf::from("/w/s")));
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            app.move_tab_to(2, GroupKey::Auto(r.clone()), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let [group] = app.sidebar_groups.pinned.as_slice() else {
                panic!("exactly one pinned group: {:?}", app.sidebar_groups.pinned);
            };
            assert_eq!(group.folder.as_deref(), Some("/w/r"));
            for (i, tab) in app.tabs[..3].iter().enumerate() {
                assert_eq!(tab.group.get(), Some(group.id), "tab {i}");
            }
            let keys = app.sidebar_group_keys(cx);
            assert!(
                !keys.contains(&Some(GroupKey::Auto(r.clone()))),
                "r is no longer an auto group"
            );
        });
    }

    /// A tab dragged out of a folder group while still in the repo is drawn
    /// under the repo's auto group, beside the folder group keeping the same
    /// directory. Moving a tab into that auto group joins the kept group
    /// rather than pinning the folder a second time.
    #[gpui::test]
    fn moving_into_an_auto_group_whose_folder_is_kept_joins_the_kept_group(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let r = AutoKey::Repo(PathBuf::from("/w/r"));
        let kept = PinnedGroup::folder(Path::new("/w/r"));
        let id = kept.id;
        app.update(&mut vcx, |app, cx| {
            app.sidebar_groups.pinned.push(kept);
            app.tabs[0].group.set(Some(id));
            for tab in &app.tabs[..2] {
                *tab.auto_group.borrow_mut() = Some(r.clone());
            }
            *app.tabs[2].auto_group.borrow_mut() = Some(AutoKey::Repo(PathBuf::from("/w/s")));
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            assert!(
                app.sidebar_group_keys(cx)
                    .contains(&Some(GroupKey::Auto(r.clone()))),
                "tab 1 is drawn under the repo's auto group"
            );
            app.move_tab_to(2, GroupKey::Auto(r.clone()), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let [group] = app.sidebar_groups.pinned.as_slice() else {
                panic!("still one pinned group: {:?}", app.sidebar_groups.pinned);
            };
            assert_eq!(group.id, id);
            for (i, tab) in app.tabs[..3].iter().enumerate() {
                assert_eq!(tab.group.get(), Some(id), "tab {i}");
            }
        });
    }

    /// A pinned group with no tabs is still drawn — it is kept until it is
    /// deleted — while an auto group with none simply is not there.
    #[gpui::test]
    fn an_empty_pinned_group_still_has_a_section(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let work = label(app, "work", cx);
            let keys = app.sidebar_group_keys(cx);
            let sections = sidebar_sections(&keys, &app.sidebar_groups);
            assert_eq!(sections[0].key, Some(GroupKey::Pinned(work)));
            assert!(sections[0].tabs.is_empty());
        });
    }

    #[gpui::test]
    fn folding_a_group_takes_its_rows_off_the_sidebar(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        let alpha = AutoKey::Repo(PathBuf::from("/w/alpha"));
        let beta = AutoKey::Repo(PathBuf::from("/w/beta"));

        app.update(&mut vcx, |app, cx| {
            for (i, root) in [(0, &alpha), (1, &alpha), (2, &beta)] {
                *app.tabs[i].auto_group.borrow_mut() = Some(root.clone());
            }
            app.active = 2;
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!(
                (0..3).all(|i| drawn(app, i)),
                "every row is on screen before anything is folded"
            );
        });

        let alpha_key = GroupKey::Auto(alpha.clone());
        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha_key), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!(
                !drawn(app, 0) && !drawn(app, 1),
                "the folded group's rows left no rectangle behind"
            );
            assert!(drawn(app, 2), "the group next to it is untouched");
            assert_eq!(
                app.sidebar_groups.auto_collapsed,
                vec![alpha.clone()],
                "the fold is kept with the workspace, where the next launch reads it"
            );
        });

        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha_key), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!((0..3).all(|i| drawn(app, i)), "unfolding brings them back");
            assert!(
                app.sidebar_groups.auto_collapsed.is_empty(),
                "and takes the entry back out rather than piling up"
            );
        });
    }

    /// A middle click closes the row under it, wherever on the row it lands,
    /// and leaves the others and the active tab alone.
    #[gpui::test]
    fn a_middle_click_closes_the_row_under_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        vcx.run_until_parked();

        let (row, kept) = app.update(&mut vcx, |app, _| {
            assert!(drawn(app, 1), "the row is on screen to be clicked");
            let kept = [app.tabs[0].tree_id.get(), app.tabs[2].tree_id.get()];
            (app.sidebar_slots.borrow()[1], kept)
        });
        vcx.simulate_mouse_down(row.center(), MouseButton::Middle, gpui::Modifiers::none());
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let left: Vec<_> = app.tabs.iter().map(|t| t.tree_id.get()).collect();
            assert_eq!(left, kept, "only the middle-clicked tab is gone");
            assert_eq!(app.active, 0, "the active tab stays active");
        });
    }

    #[gpui::test]
    fn a_search_outranks_a_fold(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        let alpha = AutoKey::Repo(PathBuf::from("/w/alpha"));

        app.update(&mut vcx, |app, cx| {
            for tab in &app.tabs[..2] {
                *tab.auto_group.borrow_mut() = Some(alpha.clone());
            }
            app.toggle_sidebar_group(Some(&GroupKey::Auto(alpha.clone())), cx);
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert!(!drawn(app, 1), "folded, so the row is not drawn");
        });

        // Whatever the row is actually showing — the label is derived from the
        // test process's cwd, and this has to be a query that matches it.
        app.update_in(&mut vcx, |app, window, cx| {
            let label = app.tab_label(&app.tabs[1], 1, Some(window), cx).to_string();
            app.sidebar_search.update(cx, |state, cx| {
                state.set_value(&label, window, cx);
            });
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!(
                drawn(app, 1),
                "a row a query matches has to show, fold or no fold"
            );
        });
    }

    /// A fold hides every row the group has, the active one included. The
    /// alternative — leaving the active row on screen under a shut chevron,
    /// with the header counting rows that are not drawn — looks like a list
    /// that failed to load, which is what folding a group you are working in
    /// used to produce.
    #[gpui::test]
    fn a_fold_hides_the_active_row_too(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        let alpha = GroupKey::Auto(AutoKey::Repo(PathBuf::from("/w/alpha")));

        app.update(&mut vcx, |app, cx| {
            for tab in &app.tabs[..2] {
                *tab.auto_group.borrow_mut() = alpha.auto().cloned();
            }
            app.active = 0;
            app.toggle_sidebar_group(Some(&alpha), cx);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!(!drawn(app, 0), "the active row folds away with the rest");
            assert!(!drawn(app, 1), "and so does everything else in the group");
        });

        app.update(&mut vcx, |app, cx| {
            app.toggle_sidebar_group(Some(&alpha), cx)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert!((0..2).all(|i| drawn(app, i)), "unfolding brings both back");
        });
    }
    /// The divider stands between kept and derived groups, so it is there
    /// exactly when something is kept — and it is recorded, because a tab
    /// dropped below it leaves its pinned group.
    #[gpui::test]
    fn the_divider_is_drawn_once_something_is_pinned(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        app.update(&mut vcx, |_, cx| cx.notify());
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert!(app.sidebar_divider.get().is_none(), "nothing kept, no line");
        });

        app.update(&mut vcx, |app, cx| {
            label(app, "work", cx);
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, _| {
            assert!(app.sidebar_divider.get().is_some(), "kept, so a line");
        });
    }

    /// An empty pinned group stays on screen, and takes drops like any other:
    /// its block is recorded where a dragged tab looks for somewhere to land.
    #[gpui::test]
    fn an_empty_pinned_group_is_drawn_and_takes_drops(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let work = app.update(&mut vcx, |app, cx| label(app, "work", cx));
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            let landing = app.sidebar_group_slots.borrow();
            let block = landing
                .iter()
                .find(|(id, _)| *id == work)
                .expect("the empty group was drawn");
            assert!(block.1.size.height > px(0.));
        });
    }

    /// Pinned headers are ordered by hand; a drag lands as a new order for
    /// the list, not for the tabs.
    #[gpui::test]
    fn a_pinned_drag_reorders_the_groups_not_the_tabs(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update(&mut vcx, |app, cx| {
            let (a, b, c) = (
                label(app, "a", cx),
                label(app, "b", cx),
                label(app, "c", cx),
            );
            app.set_tab_group(0, Some(a), cx);
            app.set_tab_group(1, Some(c), cx);
            let tabs: Vec<_> = app.tabs.iter().map(|t| t.tree_id.get()).collect();
            app.apply_pinned_order(&[2, 0, 1], cx);
            assert_eq!(
                app.pinned_group_names()
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>(),
                vec![c, a, b]
            );
            assert_eq!(
                app.tabs.iter().map(|t| t.tree_id.get()).collect::<Vec<_>>(),
                tabs,
                "the tabs stayed where they were"
            );
            // An order that is not a permutation of the list changes nothing.
            app.apply_pinned_order(&[0, 1], cx);
            assert_eq!(app.sidebar_groups.pinned.len(), 3);
        });
    }

    /// "New Tab" on a pinned group opens a tab in it — joining outright,
    /// without waiting for a cwd to walk in, since a label group has nothing
    /// to walk into.
    #[gpui::test]
    fn a_label_group_can_be_filled_from_its_header(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let before = app.update(&mut vcx, |app, _| app.tabs.len());
        let work = app.update_in(&mut vcx, |app, window, cx| {
            let work = label(app, "work", cx);
            app.new_tab_in_group(GroupKey::Pinned(work), window, cx);
            work
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            // A harness without a daemon may refuse to spawn; when it does
            // open one, it has to be in the group.
            if app.tabs.len() > before {
                assert_eq!(app.tabs[app.active].group.get(), Some(work));
            }
        });
    }

    /// Clearing a folder keeps the name the header was showing: a folder
    /// group nobody renamed reads its folder's name, and would read nothing
    /// once the folder is gone.
    #[gpui::test]
    fn clearing_a_folder_keeps_the_name_it_was_showing(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            let tty59 = folder(app, "/w/tty59", cx);
            app.set_group_folder(tty59, None, cx);
            let group = app.sidebar_groups.get(tty59).expect("still pinned");
            assert_eq!(group.folder, None);
            assert_eq!(group.given_name(), Some("tty59"));
            app.set_group_folder(tty59, Some(PathBuf::from("/w/else")), cx);
            let group = app.sidebar_groups.get(tty59).expect("still pinned");
            assert_eq!(group.folder.as_deref(), Some("/w/else"));
            assert_eq!(group.given_name(), Some("tty59"), "a set name stays");
        });
    }
    /// A folder dropped from Finder is pinned; a file is let fall, and a
    /// folder already pinned is not pinned twice.
    #[gpui::test]
    fn a_dropped_folder_is_pinned_and_a_dropped_file_is_not(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "x").expect("write");

        app.update(&mut vcx, |app, cx| {
            let paths = [dir.path().to_path_buf(), file.clone()];
            app.pin_dropped_folders(&paths, cx);
            app.pin_dropped_folders(&paths, cx);
            let folders: Vec<_> = app
                .sidebar_groups
                .pinned
                .iter()
                .map(|g| g.folder.clone())
                .collect();
            assert_eq!(
                folders,
                vec![Some(dir.path().to_string_lossy().into_owned())]
            );
        });
    }

    /// The palette's "New Group" makes an empty pinned group and opens its
    /// name for typing.
    #[gpui::test]
    fn the_palette_makes_an_empty_group_ready_to_name(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| {
            app.new_empty_group(window, cx);
            let id = app.sidebar_groups.pinned[0].id;
            assert_eq!(app.group_rename.as_ref().map(|r| r.group), Some(id));
            assert!(app.tabs.iter().all(|t| t.group.get().is_none()));
            let kinds: Vec<_> = app
                .search_actions(window, cx)
                .into_iter()
                .map(|c| c.kind)
                .collect();
            assert!(kinds.contains(&crate::ui::search::CommandKind::NewGroup));
            assert!(
                kinds.contains(&crate::ui::search::CommandKind::OpenFolderAsGroup),
                "a workspace on this computer can pick a folder"
            );
        });
    }

    /// Groups that arrive from another window, or from the machine at launch,
    /// are not folders this window's tabs walked into. A tab sitting in one —
    /// here, one that was dragged out of it — stays where it is.
    #[gpui::test]
    fn groups_arriving_from_elsewhere_pull_no_tab_in(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update(&mut vcx, |app, cx| {
            plant_repo(app, 0, "/w/probed/sub", "/w/probed", cx);
            cx.notify();
        });
        vcx.run_until_parked();
        app.update(&mut vcx, |app, cx| {
            let mut groups = WorkspaceGroups::default();
            groups
                .pinned
                .push(PinnedGroup::folder(Path::new("/w/probed")));
            app.adopt_sidebar_groups(groups, cx);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, _| {
            assert_eq!(app.tabs[0].group.get(), None);
        });
    }

    /// A tab restored from the tree is drawn in the auto group it last had
    /// before any probe has answered — the launch no longer parks every tab
    /// in Ungrouped — and the live answer, once it lands, wins and becomes
    /// the new hint. A hint never outranks a pinned group.
    #[gpui::test]
    fn a_restored_tab_is_drawn_from_its_hint_until_a_probe_answers(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update(&mut vcx, |app, cx| {
            for tab in &mut app.tabs[..2] {
                let pane = std::mem::replace(&mut tab.pane, crate::ui::pane::Pane::Empty);
                let tree = tty59_core::core::machine::Tab {
                    id: tab.tree_id.get(),
                    name: None,
                    group: None,
                    last_auto: Some(AutoKey::Repo(PathBuf::from("/w/alpha"))),
                    root: tty59_core::core::machine::PaneNode::Leaf { pane: 1 },
                    hibernated: false,
                };
                *tab = crate::ui::app::Tab::from_tree(&tree, pane);
            }
            let work = label(app, "work", cx);
            app.tabs[1].group.set(Some(work));
            let keys = app.sidebar_group_keys(cx);
            assert_eq!(
                keys[0],
                repo("/w/alpha"),
                "drawn from the hint, no probe yet"
            );
            assert_eq!(
                keys[1],
                Some(GroupKey::Pinned(work)),
                "a pinned group outranks any hint"
            );
            plant_repo(app, 0, "/w/beta/src", "/w/beta", cx);
            cx.notify();
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert_eq!(
                app.sidebar_group_keys(cx)[0],
                repo("/w/beta"),
                "the probe wins"
            );
            let (desired, _, _) = crate::ui::tree_sync::desired_tabs(app, cx);
            assert_eq!(
                desired[0].last_auto,
                Some(AutoKey::Repo(PathBuf::from("/w/beta"))),
                "and is what goes up as the next launch's hint"
            );
        });
    }

    /// What `harness_with_tabs` draws at `width`, drawn twice so the first
    /// frame's measurements have settled.
    fn draw_at(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) {
        app.update(vcx, |app, cx| {
            app.sidebar_width.set(width);
            cx.notify();
        });
        vcx.run_until_parked();
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
    }

    /// Two tabs pinned to one group whose long name and very long branch
    /// (with `counts`) both want the whole header line.
    fn long_branch_header(
        cx: &mut TestAppContext,
        counts: (u32, u32),
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 2);
        app.update(&mut vcx, |app, cx| {
            let work = label(app, "a-rather-long-group-name-for-the-rail", cx);
            for i in 0..2 {
                app.set_tab_group(i, Some(work), cx);
                plant_on(
                    app,
                    i,
                    "/w/probed/sub",
                    ("/w/probed", "/w/probed"),
                    (
                        "feature/checkout-flow-redesign-with-a-very-long-name",
                        counts,
                    ),
                    cx,
                );
            }
        });
        (app, vcx, streams)
    }

    /// A header with a long branch used to leave the heading as `DEL…` while
    /// the branch kept thirty characters. The branch group may take half the
    /// header's line and no more, the heading keeps the rest down to its
    /// floor, and the diff counts are never squeezed: they sit flush right at
    /// the narrowest rail and at the default one, at the same size.
    #[gpui::test]
    fn a_long_branch_takes_at_most_half_the_header_and_the_counts_stay_whole(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = long_branch_header(cx, (12, 3));
        let mut counts_widths = Vec::new();
        for width in [MIN_SIDEBAR_WIDTH, 260.] {
            draw_at(&app, &mut vcx, width);
            let painted = |vcx: &mut VisualTestContext, selector: &'static str| {
                vcx.debug_bounds(selector)
                    .unwrap_or_else(|| panic!("{selector} was not painted at {width}"))
            };
            let bar = painted(&mut vcx, "sidebar-group-0");
            let name = painted(&mut vcx, "sidebar-group-name-0");
            let git = painted(&mut vcx, "sidebar-group-git-0");
            let counts = painted(&mut vcx, "sidebar-group-diff-0");
            // `px_2` on the bar: the half is of what is inside it.
            let inner = bar.size.width - px(16.);
            assert!(
                git.size.width <= inner * 0.5 + px(0.5),
                "at {width}: the branch group is {:?} of a {inner:?} line",
                git.size.width
            );
            assert!(
                name.size.width >= px(HEADER_NAME_FLOOR) - px(0.5),
                "at {width}: the heading kept {:?}",
                name.size.width
            );
            assert!(
                name.right() <= git.left() + px(0.5),
                "at {width}: the heading and the branch overlap"
            );
            assert!(
                (counts.right() - git.right()).abs() < px(0.5)
                    && (bar.right() - px(8.) - counts.right()).abs() < px(0.5),
                "at {width}: the counts are not flush right: bar {bar:?}, name {name:?}, git {git:?}, counts {counts:?}, counts to padding edge {:?}",
                bar.right() - px(8.) - counts.right()
            );
            counts_widths.push(counts.size.width);
        }
        assert_eq!(
            counts_widths[0], counts_widths[1],
            "the counts are intrinsic and do not give way at the narrow rail"
        );
    }

    /// A plain (no repo) tab whose title is short and whose working directory
    /// is far longer than any row: the path rides on the title's own line.
    fn short_title_long_cwd(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        app.update(&mut vcx, |app, cx| {
            app.tabs[0].name = Some("zsh".into());
            plant_plain(
                app,
                0,
                "/w/scratch/experiments/long-nested-directory-name-for-truncation/and-one-more",
                cx,
            );
            cx.notify();
        });
        (app, vcx, streams)
    }

    /// The working directory beside a short title takes what it asks for up to
    /// half of the title line and no more: the title keeps the rest. The cap is
    /// what limits it here (its path is far longer), so it lands on the half.
    #[gpui::test]
    fn a_long_working_directory_takes_half_of_the_title_line_and_no_more(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = short_title_long_cwd(cx);
        draw_at(&app, &mut vcx, MIN_SIDEBAR_WIDTH);
        let title = vcx
            .debug_bounds("sidebar-row-title-0")
            .expect("the title was painted");
        let cwd = vcx
            .debug_bounds("sidebar-row-cwd-0")
            .expect("the working directory was painted beside the title");
        let line = cwd.right() - title.left();
        assert!(
            cwd.size.width <= line * 0.5 + px(0.5),
            "the path is {:?} of a {line:?} line",
            cwd.size.width
        );
        assert!(
            cwd.size.width >= line * 0.5 - px(1.),
            "the path asked for far more than half and got only {:?} of {line:?}",
            cwd.size.width
        );
    }

    /// Holding the tab chord (the real modifier event, and the real timer that
    /// raises the badges) takes nothing from the row: the title keeps its floor
    /// AND the room it had, and the working directory beside it keeps its own,
    /// before, while and after the hold. The badge used to be a flex child
    /// that took 30px from the line, which this test once required.
    #[gpui::test]
    fn a_row_title_keeps_its_floor_and_its_room_while_the_badge_shows(cx: &mut TestAppContext) {
        use super::a5alt_tests::{hold, release};

        let (app, mut vcx, _streams) = short_title_long_cwd(cx);
        draw_at(&app, &mut vcx, MIN_SIDEBAR_WIDTH);
        let bounds = |vcx: &mut VisualTestContext| {
            (
                vcx.debug_bounds("sidebar-row-title-0")
                    .expect("the title was painted"),
                vcx.debug_bounds("sidebar-row-cwd-0")
                    .expect("the working directory was painted beside the title"),
            )
        };
        let (roomy, roomy_cwd) = bounds(&mut vcx);
        hold(&app, &mut vcx);
        let (tight, tight_cwd) = bounds(&mut vcx);
        assert_eq!(
            tight, roomy,
            "the held badge moved or resized the title: {roomy:?} -> {tight:?}"
        );
        assert_eq!(
            tight_cwd, roomy_cwd,
            "the held badge moved or resized the working directory"
        );
        assert!(
            tight.size.width >= px(ROW_TITLE_FLOOR) - px(0.5),
            "the title was squeezed to {:?}",
            tight.size.width
        );
        release(&app, &mut vcx);
        assert_eq!(bounds(&mut vcx), (roomy, roomy_cwd), "release restores it");
    }

    /// A title takes the room a wider rail gives it.
    #[gpui::test]
    fn a_row_title_grows_with_the_rail(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update(&mut vcx, |app, cx| {
            app.tabs[0].name = Some("a-fairly-long-custom-tab-name-for-this-row".into());
            cx.notify();
        });
        draw_at(&app, &mut vcx, MIN_SIDEBAR_WIDTH);
        let narrow = vcx
            .debug_bounds("sidebar-row-title-0")
            .expect("the title was painted at the narrowest rail")
            .size
            .width;
        draw_at(&app, &mut vcx, 260.);
        let wide = vcx
            .debug_bounds("sidebar-row-title-0")
            .expect("the title was painted at the default rail")
            .size
            .width;
        assert!(
            wide > narrow,
            "the title takes the room a wider rail gives it: {narrow:?} -> {wide:?}"
        );
    }

    /// The six-character rule at the real consumer. At the narrowest rail and
    /// at 240 the branch has no room for six readable characters, so its label
    /// is drawn hidden (prepared with nothing, nothing painted) while the diff
    /// counts stay painted flush right; at a wide rail the same label paints.
    /// 240 is a measured width, not a guess: without the rule the label there
    /// shows `feat…  ·` (four of its characters), so a missing rule paints a
    /// fragment where this test requires nothing. The observer answers `None`
    /// for a label that was not drawn at all, which must not pass for hidden,
    /// so every state is asserted as present.
    #[gpui::test]
    fn the_header_hides_a_branch_without_six_characters_and_keeps_the_counts(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::text_elision::observe;

        let (app, mut vcx, _streams) = long_branch_header(cx, (1, 1));
        let branch = gpui::ElementId::from(("sidebar-group-branch", 0usize));
        let mut counts_widths = Vec::new();
        for (width, shown) in [(MIN_SIDEBAR_WIDTH, false), (240., false), (400., true)] {
            // Settle at the new width first, then forget what that drew, so
            // the frame inspected is the only source of the observation.
            draw_at(&app, &mut vcx, width);
            observe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let label = observe::label(&branch)
                .unwrap_or_else(|| panic!("at {width}: the branch label was not drawn at all"));
            match shown {
                false => assert!(
                    label.text.is_empty() && !label.painted,
                    "at {width}: expected a present, hidden label, got {label:?}"
                ),
                true => assert!(
                    !label.text.is_empty() && label.painted,
                    "at {width}: expected a painted branch, got {label:?}"
                ),
            }
            let bar = vcx.debug_bounds("sidebar-group-0").expect("the header");
            let counts = vcx
                .debug_bounds("sidebar-group-diff-0")
                .unwrap_or_else(|| panic!("at {width}: the counts were not painted"));
            assert!(
                (bar.right() - px(8.) - counts.right()).abs() < px(0.5),
                "at {width}: the counts are not flush right: bar {bar:?}, name {:?}, git {:?}, counts {counts:?}, counts to padding edge {:?}",
                vcx.debug_bounds("sidebar-group-name-0"),
                vcx.debug_bounds("sidebar-group-git-0"),
                bar.right() - px(8.) - counts.right()
            );
            counts_widths.push(counts.size.width);
        }
        assert_eq!(
            counts_widths[0], counts_widths[1],
            "the counts keep their size whether or not the branch is drawn"
        );
    }

    /// An ended pane's row keeps the words its exit wrote, whole, in every
    /// shipped language, and elides only the name in front of them: the
    /// observer reads what the title label prepared and painted at the widest
    /// rail, where the head still has room beside the tail.
    #[gpui::test]
    fn an_ended_row_keeps_its_localized_tail_whole(cx: &mut TestAppContext) {
        use crate::ui::text_elision::observe;

        const NAME: &str = "a-pane-with-a-very-long-name-that-no-row-can-hold-whole-and-then-some-more-words-after-it-to-be-sure";
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let title = gpui::ElementId::from(("sidebar-row-title", 0usize));
        // The rail width per language is MEASURED, not computed: at each one the title slot
        // is too narrow for an ordinary label (the whole text, no included tail) to keep
        // the status whole, yet wide enough for the included tail plus a cut name.
        for (language, width, tail) in [
            ("en", 240., " — process exited"),
            ("ru-RU", 240., ": процесс завершён"),
            ("ja-JP", 220., " — プロセスが終了しました"),
            ("zh-CN", MIN_SIDEBAR_WIDTH, " — 进程已退出"),
        ] {
            crate::ui::i18n::set_locale(language);
            app.update(&mut vcx, |app, cx| {
                let leaf = app.tabs[0].pane.first_leaf().expect("a tab has a pane");
                leaf.terminal()
                    .expect("a terminal pane")
                    .update(cx, |view, cx| view.end_for_test(NAME, cx));
                cx.notify();
            });
            draw_at(&app, &mut vcx, width);
            observe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let label = observe::label(&title)
                .unwrap_or_else(|| panic!("{language}: the row title was not drawn"));
            assert!(label.painted, "{language}: {label:?}");
            assert!(
                label.text.ends_with(tail),
                "{language}: the tail was cut: {:?}",
                label.text
            );
            assert!(
                label.text.contains('…') && label.text.len() < NAME.len() + tail.len(),
                "{language}: the name did not elide: {:?}",
                label.text
            );
        }
        crate::ui::i18n::set_locale("en");
    }

    /// Double-clicking a row opens the same rename box the context menu does,
    /// on that row's tab. The box has to survive the click that opened it: a
    /// focus left behind on the terminal would blur it shut straight away.
    #[gpui::test]
    fn double_clicking_a_row_renames_its_tab(cx: &mut TestAppContext) {
        use crate::ui::app::test_window::double_click;
        use gpui::Focusable;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        vcx.simulate_resize(gpui::size(px(1200.), px(800.)));
        vcx.run_until_parked();

        let (row, id) = app.update(&mut vcx, |app, _| {
            assert!(drawn(app, 1), "the sidebar row is on screen to click");
            assert!(app.renaming.is_none());
            (app.sidebar_slots.borrow()[1], app.tabs[1].tree_id.get())
        });
        double_click(&mut vcx, row.center());

        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.active, 1, "the first click put the tab on screen");
            let renaming = app.renaming.as_ref().expect("the second opened a rename");
            assert_eq!(renaming.tab, id, "on the row that was clicked");
            assert!(
                renaming.input.focus_handle(cx).is_focused(window),
                "with the box focused for typing"
            );
        });
    }
}

/// A group header's pin and "+" targets, and its pinned mark: mounted bounds,
/// real pointer events and real Tab keystrokes. The numbers below are literal
/// oracles on purpose: they are the accepted geometry, not the production
/// constants, so a change to either side shows up as a failure.
#[cfg(test)]
mod header_target_tests {
    use super::*;
    use crate::daemon::transport::Stream;
    use crate::ui::app::test_window::harness_with_tabs;
    use gpui::{
        Bounds, Entity, Modifiers, Pixels, Point, TestAppContext, VisualTestContext, point, size,
    };

    const TARGET_W: f32 = 24.;
    const TARGET_H: f32 = 22.;
    const GLYPH: f32 = 12.;
    const GAP: f32 = 2.;

    /// A `'static` selector for `prefix` followed by `ix`: `debug_bounds` wants
    /// a literal, and building one per index from `concat!` needs neither a
    /// leak nor a lint allowance. A test that outgrows 16 rows or headers
    /// fails here with the index it asked for.
    macro_rules! numbered {
        ($prefix:literal, $ix:expr) => {
            match $ix {
                0 => concat!($prefix, "0"),
                1 => concat!($prefix, "1"),
                2 => concat!($prefix, "2"),
                3 => concat!($prefix, "3"),
                4 => concat!($prefix, "4"),
                5 => concat!($prefix, "5"),
                6 => concat!($prefix, "6"),
                7 => concat!($prefix, "7"),
                8 => concat!($prefix, "8"),
                9 => concat!($prefix, "9"),
                10 => concat!($prefix, "10"),
                11 => concat!($prefix, "11"),
                12 => concat!($prefix, "12"),
                13 => concat!($prefix, "13"),
                14 => concat!($prefix, "14"),
                15 => concat!($prefix, "15"),
                n => panic!("no selector literal for {}{}", $prefix, n),
            }
        };
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Target {
        Pin,
        Add,
    }

    impl Target {
        fn selector(self, ix: usize) -> &'static str {
            match self {
                Target::Pin => numbered!("sidebar-group-pin-", ix),
                Target::Add => numbered!("sidebar-group-add-", ix),
            }
        }

        fn action(self) -> &'static str {
            match self {
                Target::Pin => "pin",
                Target::Add => "add",
            }
        }
    }

    fn near(a: Pixels, b: f32) -> bool {
        (f32::from(a) - b).abs() < 0.51
    }

    fn alpha() -> AutoKey {
        AutoKey::Repo(PathBuf::from("/w/alpha"))
    }

    fn beta() -> AutoKey {
        AutoKey::Repo(PathBuf::from("/w/beta"))
    }

    fn draw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Two auto groups of two tabs each: headers 0 and 1, each with a pin and
    /// a "+".
    fn two_groups(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 4);
        app.update(&mut vcx, |app, cx| {
            for (i, key) in [(0, alpha()), (1, alpha()), (2, beta()), (3, beta())] {
                *app.tabs[i].auto_group.borrow_mut() = Some(key);
            }
            cx.notify();
        });
        draw(&mut vcx);
        (app, vcx, streams)
    }

    fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is drawn"))
    }

    /// Fold state of every group: the folded auto keys and each pinned flag.
    fn folds(app: &Tty59App) -> (Vec<AutoKey>, Vec<bool>) {
        (
            app.sidebar_groups.auto_collapsed.clone(),
            app.sidebar_groups
                .pinned
                .iter()
                .map(|g| g.collapsed)
                .collect(),
        )
    }

    /// Whether the auto group `key` is folded, wherever its fold is kept: in
    /// the auto list, or on the kept group that took its folder when pinned.
    fn is_folded(app: &Tty59App, key: &AutoKey) -> bool {
        let AutoKey::Repo(path) = key else {
            return false;
        };
        app.sidebar_groups.auto_collapsed.contains(key)
            || app
                .sidebar_groups
                .pinned
                .iter()
                .any(|g| g.collapsed && g.folder.as_deref() == path.to_str())
    }

    /// The title of each of the first `n` rows is drawn.
    fn rows_are_shown(vcx: &mut VisualTestContext, n: usize, label: &str) {
        for i in 0..n {
            assert!(
                vcx.debug_bounds(numbered!("sidebar-row-title-", i))
                    .is_some(),
                "{label}: row {i} is on screen"
            );
        }
    }

    /// One point just inside each of a target's four edges.
    fn inside_edges(b: Bounds<Pixels>) -> [(&'static str, Point<Pixels>); 4] {
        let half = px(0.5);
        [
            ("top", point(b.center().x, b.top() + half)),
            ("bottom", point(b.center().x, b.bottom() - half)),
            ("left", point(b.left() + half, b.center().y)),
            ("right", point(b.right() - half, b.center().y)),
        ]
    }

    /// Both targets of every header are 24 by the header's height, their glyphs
    /// are 12px and centred, and the two are 2px apart without overlapping, so
    /// the centres are 26px apart.
    #[gpui::test]
    fn header_targets_are_full_size_with_twelve_pixel_glyphs(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = two_groups(cx);
        for ix in 0..2 {
            let header = bounds(&mut vcx, numbered!("sidebar-group-", ix));
            let pin = bounds(&mut vcx, Target::Pin.selector(ix));
            let add = bounds(&mut vcx, Target::Add.selector(ix));
            let pin_glyph = bounds(&mut vcx, numbered!("sidebar-group-pin-glyph-", ix));
            let add_glyph = bounds(&mut vcx, numbered!("sidebar-group-add-glyph-", ix));

            for (name, target, glyph) in [("pin", pin, pin_glyph), ("add", add, add_glyph)] {
                assert!(
                    near(target.size.width, TARGET_W) && near(target.size.height, TARGET_H),
                    "header {ix} {name} target is {:?}, not 24 x 22",
                    target.size
                );
                assert!(
                    near(target.size.height, f32::from(header.size.height)),
                    "header {ix} {name} target is as tall as the header it sits in"
                );
                assert!(
                    near(target.top(), f32::from(header.top())),
                    "header {ix} {name} target starts at the header's top, not above it"
                );
                assert!(
                    near(glyph.size.width, GLYPH) && near(glyph.size.height, GLYPH),
                    "header {ix} {name} glyph is {:?}, not 12 x 12",
                    glyph.size
                );
                assert!(
                    near(glyph.center().x, f32::from(target.center().x))
                        && near(glyph.center().y, f32::from(target.center().y)),
                    "header {ix} {name} glyph is centred in its target"
                );
            }
            assert!(
                near(add.left() - pin.right(), GAP),
                "header {ix}: the targets are {:?} apart, not 2px",
                add.left() - pin.right()
            );
            assert!(
                pin.right() <= add.left(),
                "header {ix}: the two targets do not overlap"
            );
            assert!(
                near(add.center().x - pin.center().x, TARGET_W + GAP),
                "header {ix}: the centres are 26px apart"
            );
        }
    }

    /// A real press just inside each of a target's four edges reaches that
    /// target's callback exactly once, does not fold the header, and does not
    /// fall through to a row. The probe records each callback entry, because
    /// pinning twice leaves the same state as pinning once.
    #[gpui::test]
    fn a_press_inside_any_edge_of_a_target_fires_it_exactly_once(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = two_groups(cx);
        for ix in 0..2 {
            for target in [Target::Pin, Target::Add] {
                for edge in 0..4 {
                    draw(&mut vcx);
                    // The press points come from the accepted 24 x 22 box
                    // anchored at the mounted target's left and the header's
                    // top, not from the mounted size, so a target that came
                    // out smaller misses its own edge.
                    let header = bounds(&mut vcx, numbered!("sidebar-group-", ix));
                    let mounted = bounds(&mut vcx, target.selector(ix));
                    let expected = Bounds {
                        origin: point(mounted.left(), header.top()),
                        size: size(px(TARGET_W), px(TARGET_H)),
                    };
                    let (edge_name, at) = inside_edges(expected)[edge];
                    let label = format!("{target:?} of header {ix}, {edge_name} edge");

                    let (before_active, before_tabs) =
                        app.read_with(&vcx, |a, _| (a.active, a.tabs.len()));
                    app.read_with(&vcx, |a, _| {
                        assert!(
                            !is_folded(a, &alpha()) && !is_folded(a, &beta()),
                            "{label}: both groups start open"
                        );
                    });
                    rows_are_shown(&mut vcx, before_tabs, &label);
                    header_action_probe::take();
                    vcx.simulate_click(at, Modifiers::none());
                    draw(&mut vcx);

                    assert_eq!(
                        header_action_probe::take(),
                        vec![target.action()],
                        "{label}: the callback ran exactly once"
                    );
                    // The group is open before and after, whichever container
                    // (auto list or kept list) holds its fold now, and its
                    // rows are still on screen.
                    app.read_with(&vcx, |a, _| {
                        assert!(
                            !is_folded(a, &alpha()) && !is_folded(a, &beta()),
                            "{label}: the header did not fold"
                        );
                        if a.tabs.len() == before_tabs {
                            assert_eq!(a.active, before_active, "{label}: no row took the press");
                        } else {
                            assert_eq!(
                                a.tabs.len(),
                                before_tabs + 1,
                                "{label}: at most one tab opened"
                            );
                            assert!(
                                a.active >= before_tabs,
                                "{label}: only the new tab took the focus, no existing row"
                            );
                        }
                        match target {
                            Target::Pin => assert_eq!(
                                a.sidebar_groups.pinned.len(),
                                1,
                                "{label}: the group is kept, once"
                            ),
                            Target::Add => assert!(
                                a.sidebar_groups.pinned.is_empty(),
                                "{label}: nothing was pinned"
                            ),
                        }
                    });
                    rows_are_shown(&mut vcx, before_tabs, &label);

                    // Undo the pin so the next press meets the same header.
                    if target == Target::Pin {
                        let id = app.read_with(&vcx, |a, _| a.sidebar_groups.pinned[0].id);
                        app.update(&mut vcx, |a, cx| a.delete_group(id, cx));
                        draw(&mut vcx);
                    }
                }
            }
        }
    }

    /// Just outside a target is the header, not the target: the press folds the
    /// header exactly once and runs no action. The gap between the two targets
    /// is outside both, so they do not overlap in effect either.
    #[gpui::test]
    fn a_press_just_outside_a_target_reaches_the_header(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = two_groups(cx);
        let key = GroupKey::Auto(alpha());
        let half = px(0.5);
        let pin = bounds(&mut vcx, Target::Pin.selector(0));
        let add = bounds(&mut vcx, Target::Add.selector(0));
        let outside = [
            ("left of the pin", point(pin.left() - half, pin.center().y)),
            (
                "in the gap between the two",
                point(pin.right() + px(1.), pin.center().y),
            ),
            ("right of the +", point(add.right() + half, add.center().y)),
        ];
        for (name, at) in outside {
            let before = app.read_with(&vcx, |a, _| folds(a));
            header_action_probe::take();
            vcx.simulate_click(at, Modifiers::none());
            draw(&mut vcx);
            assert!(
                header_action_probe::take().is_empty(),
                "{name}: no header action ran"
            );
            app.read_with(&vcx, |a, _| {
                assert_ne!(folds(a), before, "{name}: the header took the press");
                assert!(a.sidebar_groups.pinned.is_empty(), "{name}: nothing pinned");
            });
            app.update(&mut vcx, |a, cx| a.toggle_sidebar_group(Some(&key), cx));
            draw(&mut vcx);
        }

        // Above and below the box is not the target either.
        for (name, at) in [
            ("above the pin", point(pin.center().x, pin.top() - half)),
            ("below the pin", point(pin.center().x, pin.bottom() + half)),
            ("below the +", point(add.center().x, add.bottom() + half)),
        ] {
            header_action_probe::take();
            vcx.simulate_click(at, Modifiers::none());
            draw(&mut vcx);
            assert!(
                header_action_probe::take().is_empty(),
                "{name}: no header action ran"
            );
            app.read_with(&vcx, |a, _| {
                assert!(a.sidebar_groups.pinned.is_empty(), "{name}: nothing pinned");
            });
            draw(&mut vcx);
        }
    }

    /// Acted on at the press: a press on a target followed by a drag never
    /// lifts the header. A drag started beside the targets does, which is what
    /// makes the first half mean something.
    #[gpui::test]
    fn pressing_a_target_never_picks_the_header_up(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = two_groups(cx);
        let lifted = |app: &Entity<Tty59App>, vcx: &VisualTestContext| {
            app.read_with(vcx, |a, _| a.reorder.borrow().is_some())
        };

        let header = bounds(&mut vcx, "sidebar-group-0");
        let beside = point(header.left() + px(2.), header.center().y);
        vcx.simulate_mouse_down(beside, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(
            point(beside.x, beside.y + px(20.)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        assert!(
            lifted(&app, &vcx),
            "positive control: a drag from the header's own edge lifts it"
        );
        vcx.simulate_mouse_up(
            point(beside.x, beside.y + px(20.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(&mut vcx);

        for target in [Target::Pin, Target::Add] {
            let b = bounds(&mut vcx, target.selector(0));
            let at = b.center();
            header_action_probe::take();
            vcx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
            vcx.simulate_mouse_move(
                point(at.x, at.y + px(20.)),
                Some(MouseButton::Left),
                Modifiers::none(),
            );
            assert!(
                !lifted(&app, &vcx),
                "{target:?}: a drag from a header action does not lift the header"
            );
            vcx.simulate_mouse_up(
                point(at.x, at.y + px(20.)),
                MouseButton::Left,
                Modifiers::none(),
            );
            draw(&mut vcx);
            header_action_probe::take();
        }
    }

    /// A pinned group's mark is status only, for a folder group and a label
    /// group alike: a press on it never unpins or deletes, it folds the header
    /// like the rest of it. Delete Group still removes a label group.
    #[gpui::test]
    fn a_pinned_marks_click_only_folds_and_keeps_the_group(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        let (kept, work) = app.update(&mut vcx, |app, cx| {
            app.pin_folder(PathBuf::from("/w/kept"), cx);
            let kept = app
                .sidebar_groups
                .pinned
                .iter()
                .find(|g| g.folder.as_deref() == Some("/w/kept"))
                .expect("just pinned")
                .id;
            let group = PinnedGroup::label("work");
            let work = group.id;
            app.edit_groups(cx, |groups| groups.pinned.push(group));
            app.set_tab_group(0, Some(kept), cx);
            app.set_tab_group(1, Some(work), cx);
            cx.notify();
            (kept, work)
        });
        draw(&mut vcx);

        for (ix, id) in [(0, kept), (1, work)] {
            let mark = bounds(&mut vcx, numbered!("sidebar-group-pin-mark-", ix));
            assert!(
                near(mark.size.width, GLYPH) && near(mark.size.height, GLYPH),
                "header {ix}: the mark is {:?}, not 12 x 12",
                mark.size
            );
            for press in 1..=2 {
                header_action_probe::take();
                vcx.simulate_click(mark.center(), Modifiers::none());
                draw(&mut vcx);
                assert!(header_action_probe::take().is_empty());
                app.read_with(&vcx, |a, _| {
                    assert!(
                        a.sidebar_groups.pinned.iter().any(|g| g.id == id),
                        "header {ix}: press {press} on the mark kept the group"
                    );
                    assert_eq!(a.tabs[ix].group.get(), Some(id), "and its tab");
                    assert_eq!(
                        a.sidebar_groups.pinned[ix].collapsed,
                        press == 1,
                        "header {ix}: press {press} folded or unfolded it, once"
                    );
                });
            }
        }

        // The explicit command is untouched: Delete Group lets a label go.
        app.update(&mut vcx, |a, cx| a.delete_group(work, cx));
        draw(&mut vcx);
        app.read_with(&vcx, |a, _| {
            assert!(a.sidebar_groups.pinned.iter().all(|g| g.id != work));
            assert_eq!(a.tabs[1].group.get(), None, "its tab is back to auto");
        });
    }

    /// The header menu is how a folder group is let go of now: right-click,
    /// then Up, Up, Enter. A fresh menu has nothing selected, the first Up
    /// takes the last item (Delete Group) and the second the one above it
    /// (Unpin), so the count holds however many items come before. Without the
    /// Unpin item the second Up lands on New Tab and the group stays, which is
    /// what the final assertion catches. The mark control: a press on the
    /// mark just before it never unpinned anything.
    ///
    /// A plain `#[test]` with a context it never tears down, for the same
    /// reason as `scm::rows_window_tests`' menu test: an open `ContextMenu`
    /// trips `#[gpui::test]`'s leak detector through a known toolkit cycle
    /// (`ui-audit/bugs/gpui-component-contextmenu-leak.md`). That is a
    /// limit of this harness, not proof that this menu releases its state.
    #[test]
    fn the_header_menu_still_unpins_a_folder_group() {
        let mut cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
        // Forgotten on every path, a failed assertion included: dropping the
        // context after a panic would run the leak teardown a second time and
        // abort, which would hide the assertion that actually failed.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            unpin_from_the_header_menu(&mut cx)
        }));
        if outcome.is_ok() {
            cx.run_until_parked();
        }
        std::mem::forget(cx);
        if let Err(failure) = outcome {
            std::panic::resume_unwind(failure);
        }
    }

    fn unpin_from_the_header_menu(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        let (kept, work) = app.update(&mut vcx, |app, cx| {
            app.pin_folder(PathBuf::from("/w/kept"), cx);
            let kept = app
                .sidebar_groups
                .pinned
                .iter()
                .find(|g| g.folder.as_deref() == Some("/w/kept"))
                .expect("just pinned")
                .id;
            let group = PinnedGroup::label("work");
            let work = group.id;
            app.edit_groups(cx, |groups| groups.pinned.push(group));
            app.set_tab_group(0, Some(kept), cx);
            app.set_tab_group(1, Some(work), cx);
            cx.notify();
            (kept, work)
        });
        draw(&mut vcx);

        // The mark is not the way out: pressed twice, the group is still kept.
        let mark = bounds(&mut vcx, "sidebar-group-pin-mark-0");
        for _ in 0..2 {
            vcx.simulate_click(mark.center(), Modifiers::none());
            draw(&mut vcx);
        }
        app.read_with(&vcx, |a, _| {
            assert!(
                a.sidebar_groups.pinned.iter().any(|g| g.id == kept),
                "pressing the mark did not unpin the folder group"
            );
        });

        let header = bounds(&mut vcx, "sidebar-group-0");
        let at = point(header.left() + px(2.), header.center().y);
        let before = vcx.update(|window, cx| window.focused(cx));
        vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        // Drawn, not just parked: parking only drains the dispatcher, and the
        // menu's focus and key context exist once its frame is mounted.
        draw(&mut vcx);
        let open = vcx.update(|window, cx| window.focused(cx));
        assert!(
            open.is_some() && open != before,
            "the right click opened a menu and focused it"
        );
        assert!(
            vcx.update(|window, _| window.context_stack())
                .iter()
                .any(|c| c.contains("PopupMenu")),
            "the focus is in a popup menu, so the keys below reach it"
        );

        vcx.simulate_keystrokes("up up enter");
        vcx.run_until_parked();
        draw(&mut vcx);

        app.read_with(&vcx, |a, _| {
            assert!(
                a.sidebar_groups.pinned.iter().all(|g| g.id != kept),
                "Up, Up, Enter chose Unpin, and the folder group is gone"
            );
            assert_eq!(a.tabs[0].group.get(), None, "its tab is back to auto");
            assert!(a.tabs.len() >= 2, "no tab was closed");
            assert!(
                a.sidebar_groups.pinned.iter().any(|g| g.id == work),
                "the other kept group is untouched"
            );
            assert_eq!(a.tabs[1].group.get(), Some(work));
        });
    }

    /// Tab presses from the rail's search field, answered with how many stops
    /// were reached before focus came back or stopped moving.
    fn walk(vcx: &mut VisualTestContext, key: &str) -> usize {
        let search = bounds(vcx, "sidebar-search");
        vcx.simulate_click(search.center(), Modifiers::none());
        vcx.run_until_parked();
        let start = vcx
            .update(|window, cx| window.focused(cx))
            .expect("clicking the search field focuses it");
        let mut seen = Vec::new();
        for _ in 0..64 {
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
            let Some(now) = vcx.update(|window, cx| window.focused(cx)) else {
                break;
            };
            if now == start || seen.contains(&now) {
                break;
            }
            seen.push(now);
        }
        seen.len()
    }

    /// The header targets are revealed by the pointer alone and stay out of
    /// the Tab order: more headers, or a header that loses its pin, change
    /// nothing about where Tab and Shift+Tab stop. A tab row's close button is
    /// a normal Button, and adding one does change it, so the count can move.
    #[gpui::test]
    fn header_targets_add_no_tab_stops(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        app.update(&mut vcx, |app, cx| {
            let group = PinnedGroup::label("work");
            let id = group.id;
            app.edit_groups(cx, |groups| groups.pinned.push(group));
            app.set_tab_group(0, Some(id), cx);
            *app.tabs[1].auto_group.borrow_mut() = Some(alpha());
            cx.notify();
        });
        draw(&mut vcx);
        let (fwd, back) = (walk(&mut vcx, "tab"), walk(&mut vcx, "shift-tab"));
        assert!(
            fwd > 0 && back > 0,
            "Tab and Shift+Tab move focus off the search field ({fwd}, {back})"
        );

        // More headers: three empty kept groups, each with a hidden "+".
        app.update(&mut vcx, |app, cx| {
            for name in ["one", "two", "three"] {
                let group = PinnedGroup::label(name);
                app.edit_groups(cx, |groups| groups.pinned.push(group));
            }
        });
        draw(&mut vcx);
        for ix in 0..4 {
            assert!(
                vcx.debug_bounds(Target::Add.selector(ix)).is_some(),
                "header {ix} draws its hidden +"
            );
        }
        assert_eq!(
            walk(&mut vcx, "tab"),
            fwd,
            "Tab: hidden + targets are no stops"
        );
        assert_eq!(
            walk(&mut vcx, "shift-tab"),
            back,
            "Shift+Tab: hidden + targets are no stops"
        );

        // A header that loses its pin: the auto group becomes a kept one.
        app.update(&mut vcx, |app, cx| app.pin_auto_group(alpha(), cx));
        draw(&mut vcx);
        assert_eq!(walk(&mut vcx, "tab"), fwd, "Tab: a hidden pin is no stop");
        assert_eq!(
            walk(&mut vcx, "shift-tab"),
            back,
            "Shift+Tab: a hidden pin is no stop"
        );

        // Positive control: two more rows, each with a normal close Button.
        app.update_in(&mut vcx, |app, window, cx| {
            for id in [100, 101] {
                let (view, stream) = crate::terminal::view::quiet_test_pane(id, window, cx);
                app.tabs
                    .push(crate::ui::app::Tab::new(crate::ui::pane::Pane::leaf(
                        crate::ui::pane::PaneSlot::Ready(view),
                    )));
                streams.push(stream);
            }
            cx.notify();
        });
        draw(&mut vcx);
        assert!(
            walk(&mut vcx, "tab") > fwd,
            "positive control: a normal Button in a new row is a Tab stop"
        );
        assert!(
            walk(&mut vcx, "shift-tab") > back,
            "positive control: and a Shift+Tab stop"
        );
    }
}

/// Shell F11: holding the tab chord raises the badges and moves nothing.
///
/// Every test here drives the REAL modifier event (`simulate_modifiers_change`
/// with the chord the keymap says the badges answer to, and the real 200ms
/// timer that raises them), never `mod_hint_badges` itself, and reads what
/// the title labels drew from the elision observer and the layout bounds.
/// Geometry, elision, behavior and status are collected as separate findings
/// per case so that one cannot mask another: a case lists every line that
/// failed rather than stopping at the first.
#[cfg(test)]
mod a5alt_tests {
    use super::*;
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
    use crate::core::config::ConfirmClose;
    use crate::daemon::protocol::DaemonMsg;
    use crate::daemon::transport::Stream;
    use crate::ui::app::test_window::harness_with_tabs_configured;
    use crate::ui::text_elision::observe;
    use gpui::{Bounds, ElementId, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

    const WIDTHS: [f32; 3] = [MIN_SIDEBAR_WIDTH, 240., 400.];

    /// `debug_bounds` wants a `'static` selector; a test leaks a few bytes.
    fn sel(prefix: &str, i: usize) -> &'static str {
        Box::leak(format!("{prefix}-{i}").into_boxed_str())
    }

    fn fixture(
        cx: &mut TestAppContext,
        tabs: usize,
        ui: f32,
    ) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
        harness_with_tabs_configured(cx, tabs, move |cfg| {
            cfg.ui_font_size = ui;
            cfg.confirm_close = ConfirmClose::Never;
        })
    }

    pub(super) fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
    }

    fn draw_at(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, width: f32) {
        app.update(vcx, |app, cx| {
            app.sidebar_width.set(width);
            cx.notify();
        });
        settle(vcx);
    }

    fn badges_up(app: &Entity<Tty59App>, vcx: &VisualTestContext) -> bool {
        app.read_with(vcx, |app, _| app.mod_hint_badges)
    }

    /// Presses the chord the badges answer to and waits, on the real timer,
    /// until they are up; the modifier stays down. Answers the chord.
    pub(super) fn hold(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Modifiers {
        let modifiers = vcx
            .update(|_, cx| crate::ui::keymap::tab_badge_hold(cx))
            .expect("the shipped tab chords are on a modifier");
        vcx.simulate_modifiers_change(modifiers);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            vcx.run_until_parked();
            if badges_up(app, vcx) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the badges never came up within 5s of the held chord"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !app.read_with(&*vcx, |app, _| app.mod_hint_group),
            "the ordinary hold, not the group hold"
        );
        settle(vcx);
        modifiers
    }

    /// Lets go of every modifier and waits for the badges to go.
    pub(super) fn release(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        vcx.simulate_modifiers_change(Modifiers::none());
        vcx.run_until_parked();
        assert!(!badges_up(app, vcx), "releasing the chord dropped no badge");
        settle(vcx);
    }

    fn near(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
        let d = |x: Pixels, y: Pixels| (x - y).abs() <= px(0.01);
        d(a.origin.x, b.origin.x)
            && d(a.origin.y, b.origin.y)
            && d(a.size.width, b.size.width)
            && d(a.size.height, b.size.height)
    }

    fn near_opt(a: Option<Bounds<Pixels>>, b: Option<Bounds<Pixels>>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => near(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    fn finish(report: Vec<String>) {
        assert!(report.is_empty(), "\n{}", report.join("\n"));
    }

    /// What a regular row drew in one frame.
    #[derive(Clone, Debug)]
    struct RowFrame {
        row: Bounds<Pixels>,
        title: Option<Bounds<Pixels>>,
        cwd: Option<Bounds<Pixels>>,
        badge: Option<Bounds<Pixels>>,
        title_text: Option<observe::Label>,
        cwd_text: Option<observe::Label>,
        branch_text: Option<observe::Label>,
    }

    fn frame(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, rows: &[usize]) -> Vec<RowFrame> {
        observe::clear();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let mut frames = Vec::new();
        for &i in rows {
            let row = app.read_with(&*vcx, |app, _| app.sidebar_slots.borrow()[i]);
            frames.push(RowFrame {
                row,
                title: vcx.debug_bounds(sel("sidebar-row-title", i)),
                cwd: vcx.debug_bounds(sel("sidebar-row-cwd", i)),
                badge: vcx.debug_bounds(sel("sidebar-row-badge", i)),
                title_text: observe::label(&ElementId::from(("sidebar-row-title", i))),
                cwd_text: observe::label(&ElementId::from(("sidebar-row-cwd", i))),
                branch_text: observe::label(&ElementId::from(("sidebar-row-branch", i))),
            });
        }
        frames
    }

    struct Cycle {
        before: Vec<RowFrame>,
        during: Vec<RowFrame>,
        after: Vec<RowFrame>,
    }

    /// One press and release of the chord around three frames.
    fn cycle(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, rows: &[usize]) -> Cycle {
        let before = frame(app, vcx, rows);
        hold(app, vcx);
        let during = frame(app, vcx, rows);
        release(app, vcx);
        let after = frame(app, vcx, rows);
        Cycle {
            before,
            during,
            after,
        }
    }

    /// Every finding of one cycle, each on its own line and labeled by kind:
    /// `behavior` (the badges came and went with the chord), `geometry` (a
    /// rectangle moved) and `elision` (a label drew other text).
    fn check(report: &mut Vec<String>, at: &str, rows: &[usize], c: &Cycle) {
        for (n, &i) in rows.iter().enumerate() {
            let (b, d, a) = (&c.before[n], &c.during[n], &c.after[n]);
            if b.badge.is_some() || d.badge.is_none() || a.badge.is_some() {
                report.push(format!(
                    "{at} row {i}: behavior: the badge must be up only while the chord is held \
                     (before {:?}, during {:?}, after {:?})",
                    b.badge, d.badge, a.badge
                ));
            }
            if b.title.is_none() {
                report.push(format!("{at} row {i}: setup: the title was not drawn"));
            }
            for (phase, p) in [("while Alt is held", d), ("after release", a)] {
                for (what, x, y) in [
                    ("row", Some(b.row), Some(p.row)),
                    ("title", b.title, p.title),
                    ("cwd", b.cwd, p.cwd),
                ] {
                    if !near_opt(x, y) {
                        report.push(format!(
                            "{at} row {i}: geometry: the {what} moved {phase}: {x:?} -> {y:?}"
                        ));
                    }
                }
                for (what, x, y) in [
                    ("title", &b.title_text, &p.title_text),
                    ("cwd", &b.cwd_text, &p.cwd_text),
                    ("branch", &b.branch_text, &p.branch_text),
                ] {
                    if x != y {
                        report.push(format!(
                            "{at} row {i}: elision: the {what} label drew other text {phase}: \
                             {x:?} -> {y:?}"
                        ));
                    }
                }
            }
        }
    }

    /// The matrix every geometry case runs: the narrowest, a middle and a wide
    /// rail. `elided` are the positions in `rows` (with their full title)
    /// whose painted title MUST differ from that full title at the narrowest
    /// rail, so that "nothing changed" cannot be vacuous. A path that falls
    /// back to its last segment is elided without an ellipsis, so the
    /// condition is "not the whole title", not "contains an ellipsis".
    fn matrix(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        rows: &[usize],
        elided: &[(usize, &str)],
        ui: f32,
        what: &str,
    ) -> Vec<String> {
        let mut report = Vec::new();
        for width in WIDTHS {
            draw_at(app, vcx, width);
            let c = cycle(app, vcx, rows);
            let at = format!("{what} UI{ui} width {width}");
            if width == MIN_SIDEBAR_WIDTH {
                for &(n, full) in elided {
                    let shown = c.before[n].title_text.as_ref();
                    if !shown.is_some_and(|l| l.painted && !l.text.is_empty() && &*l.text != full) {
                        report.push(format!(
                            "{at} row {}: setup: the title is not elided here ({shown:?}), \
                             the case would prove nothing",
                            rows[n]
                        ));
                    }
                }
            }
            check(&mut report, &at, rows, &c);
        }
        report
    }

    /// A tab's own plant, as the sibling module does it.
    fn plant_plain(app: &Tty59App, i: usize, cwd: &str, cx: &mut gpui::App) {
        use crate::terminal::git_status::GitStatusCache;
        use crate::ui::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(HostId::LOCAL, &cwd, None);
        });
    }

    /// Plants `branch` for tab `i`. The status cache is keyed by repo ROOT, so
    /// two tabs that must show different branches need different roots.
    fn plant_branch(
        app: &Tty59App,
        i: usize,
        (root, cwd): (&str, &str),
        branch: &str,
        cx: &mut gpui::App,
    ) {
        use crate::terminal::git_status::{GitStatusCache, RepoSnapshot};
        use crate::ui::host_ops::HostId;

        let cwd = PathBuf::from(cwd);
        let leaf = app.tabs[i].pane.first_leaf().expect("test tab has a pane");
        leaf.terminal()
            .expect("test pane is a terminal")
            .update(cx, |view, _| {
                view.set_git_status_cwd_for_test(Some(cwd.clone()))
            });
        cx.update_global::<GitStatusCache, _>(|cache, _| {
            cache.finish_probe(
                HostId::LOCAL,
                &cwd,
                Some(RepoSnapshot {
                    root: PathBuf::from(root),
                    home: PathBuf::from(root),
                    branch: branch.into(),
                    counts: Some((3, 1)),
                }),
            );
        });
    }

    fn name_tabs(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, names: &[&'static str]) {
        let names = names.to_vec();
        app.update(vcx, move |app, cx| {
            for (i, name) in names.iter().enumerate() {
                app.tabs[i].name = Some((*name).into());
            }
            cx.notify();
        });
    }

    fn report_agent(stream: &mut Stream, status: AgentStatus) {
        DaemonMsg::Agent(Some(CLIAgent::Codex))
            .encode(stream)
            .unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            ..Default::default()
        }))
        .encode(stream)
        .unwrap();
    }

    /// Waits for the daemon message to reach tab `tab`'s model.
    fn wait_tab_status(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        tab: usize,
        status: AgentStatus,
    ) {
        for _ in 0..400 {
            vcx.run_until_parked();
            if app.read_with(&*vcx, |app, cx| app.tabs[tab].agent_status(cx)) == Some(status) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("tab {tab} never reached {status:?}");
    }

    // ---- regular rows: geometry + elision -------------------------------

    const ORDINARY: [&str; 2] = [
        "a-fairly-long-custom-tab-name-for-this-row-that-no-rail-can-hold-whole-at-all",
        "second-tab-with-another-name-that-is-also-quite-long-indeed-and-then-some",
    ];

    fn ordinary_names_stay_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = fixture(cx, 2, ui);
        name_tabs(&app, &mut vcx, &ORDINARY);
        let full = [(0, ORDINARY[0]), (1, ORDINARY[1])];
        finish(matrix(&app, &mut vcx, &[0, 1], &full, ui, "ordinary"));
    }

    #[gpui::test]
    fn a5alt_ordinary_names_stay_put_ui16(cx: &mut TestAppContext) {
        ordinary_names_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_ordinary_names_stay_put_ui24(cx: &mut TestAppContext) {
        ordinary_names_stay_put(cx, 24.);
    }

    const PATH_TITLES: [&str; 2] = [
        "~/projects/acme/services/billing/crates/invoice-engine/src/pipeline/stages",
        "/w/scratch/experiments/long-nested-directory-name-for-truncation/and-more",
    ];

    fn path_titles_stay_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = fixture(cx, 2, ui);
        name_tabs(&app, &mut vcx, &PATH_TITLES);
        let full = [(0, PATH_TITLES[0]), (1, PATH_TITLES[1])];
        finish(matrix(&app, &mut vcx, &[0, 1], &full, ui, "path"));
    }

    #[gpui::test]
    fn a5alt_path_titles_stay_put_ui16(cx: &mut TestAppContext) {
        path_titles_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_path_titles_stay_put_ui24(cx: &mut TestAppContext) {
        path_titles_stay_put(cx, 24.);
    }

    const CJK_NAMES: [&str; 3] = [
        "Очень-длинное-имя-вкладки-которое-не-помещается-в-боковую-панель-целиком",
        "非常に長いタブの名前がサイドバーにまったく収まらないほど長く続いています",
        "这是一个非常长的标签页名称它无法完整地放进侧边栏里即使侧边栏很宽",
    ];

    fn russian_and_cjk_titles_stay_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = fixture(cx, 3, ui);
        name_tabs(&app, &mut vcx, &CJK_NAMES);
        let full = [(0, CJK_NAMES[0]), (1, CJK_NAMES[1]), (2, CJK_NAMES[2])];
        finish(matrix(&app, &mut vcx, &[0, 1, 2], &full, ui, "ru/ja/zh"));
    }

    #[gpui::test]
    fn a5alt_russian_and_cjk_titles_stay_put_ui16(cx: &mut TestAppContext) {
        russian_and_cjk_titles_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_russian_and_cjk_titles_stay_put_ui24(cx: &mut TestAppContext) {
        russian_and_cjk_titles_stay_put(cx, 24.);
    }

    fn two_line_rows_stay_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = fixture(cx, 3, ui);
        name_tabs(&app, &mut vcx, &[ORDINARY[0], ORDINARY[1], PATH_TITLES[1]]);
        app.update(&mut vcx, |app, cx| {
            // Two rows in one group whose branches differ keep a branch line
            // each (a group whose rows agree lifts it onto the header).
            let group = PinnedGroup::label("work");
            let id = group.id;
            app.edit_groups(cx, |groups| groups.pinned.push(group));
            for i in [0, 1] {
                app.set_tab_group(i, Some(id), cx);
            }
            plant_branch(
                app,
                0,
                ("/w/probed-a", "/w/probed-a/sub"),
                "feature/checkout-flow-redesign-with-a-very-long-name",
                cx,
            );
            plant_branch(
                app,
                1,
                ("/w/probed-b", "/w/probed-b/other"),
                "feature/another-long-branch-name-for-the-second-row-here",
                cx,
            );
            // A plain row outside it, whose working directory rides on the
            // title's own line.
            plant_plain(
                app,
                2,
                "/w/scratch/experiments/long-nested-directory-name-for-truncation/and-one-more",
                cx,
            );
            cx.notify();
        });
        let full = [(0, ORDINARY[0]), (1, ORDINARY[1]), (2, PATH_TITLES[1])];
        let mut report = matrix(&app, &mut vcx, &[0, 1, 2], &full, ui, "two-line + cwd");
        // The branch label and the row's own height are part of the case: a
        // two-line row stays two lines.
        draw_at(&app, &mut vcx, MIN_SIDEBAR_WIDTH);
        let rows = frame(&app, &mut vcx, &[0, 1, 2]);
        for (i, row) in rows.iter().enumerate().take(2) {
            if row.branch_text.is_none() || row.row.size.height != px(ROW_HEIGHT_TWO_LINE) {
                report.push(format!(
                    "two-line UI{ui}: setup: row {i} is not a two-line row ({:?})",
                    row.row
                ));
            }
        }
        if rows[2].cwd.is_none() {
            report.push(format!(
                "two-line UI{ui}: setup: row 2 has no cwd beside it"
            ));
        }
        finish(report);
    }

    #[gpui::test]
    fn a5alt_two_line_and_cwd_rows_stay_put_ui16(cx: &mut TestAppContext) {
        two_line_rows_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_two_line_and_cwd_rows_stay_put_ui24(cx: &mut TestAppContext) {
        two_line_rows_stay_put(cx, 24.);
    }

    /// A row that carries a status mark: the mark's own slot is in the line
    /// beside the title, and the hold used to drop it (and put 30px of badge
    /// in its place, a net 2px).
    fn status_rows_stay_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, mut streams) = fixture(cx, 2, ui);
        name_tabs(&app, &mut vcx, &ORDINARY);
        report_agent(&mut streams[0], AgentStatus::Waiting);
        wait_tab_status(&app, &mut vcx, 0, AgentStatus::Waiting);
        let full = [(0, ORDINARY[0]), (1, ORDINARY[1])];
        finish(matrix(&app, &mut vcx, &[0, 1], &full, ui, "status row"));
    }

    #[gpui::test]
    fn a5alt_status_rows_stay_put_ui16(cx: &mut TestAppContext) {
        status_rows_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_status_rows_stay_put_ui24(cx: &mut TestAppContext) {
        status_rows_stay_put(cx, 24.);
    }

    struct En;

    impl Drop for En {
        fn drop(&mut self) {
            crate::ui::i18n::set_locale("en");
        }
    }

    /// The ended pane's words stay whole, and nothing about the row moves.
    /// Widths are the measured ones of the B2 ended test (where the tail is
    /// kept whole and the name is cut), and they hold at UI16; at UI24 only
    /// the wide rail is asked, where the name still cannot fit.
    fn ended_rows_stay_put(cx: &mut TestAppContext, ui: f32) {
        const NAME: &str = "a-pane-with-a-very-long-name-that-no-row-can-hold-whole-and-then-some-more-words-after-it-to-be-sure";
        let _en = En;
        let (app, mut vcx, _streams) = fixture(cx, 1, ui);
        let cases: [(&str, f32, &str); 4] = [
            ("en", 240., " — process exited"),
            ("ru-RU", 240., ": процесс завершён"),
            ("ja-JP", 220., " — プロセスが終了しました"),
            ("zh-CN", MIN_SIDEBAR_WIDTH, " — 进程已退出"),
        ];
        let mut report = Vec::new();
        for (language, width, tail) in cases {
            let width = if ui == 16. { width } else { 400. };
            crate::ui::i18n::set_locale(language);
            app.update(&mut vcx, |app, cx| {
                let leaf = app.tabs[0].pane.first_leaf().expect("a tab has a pane");
                leaf.terminal()
                    .expect("a terminal pane")
                    .update(cx, |view, cx| view.end_for_test(NAME, cx));
                cx.notify();
            });
            draw_at(&app, &mut vcx, width);
            let c = cycle(&app, &mut vcx, &[0]);
            let at = format!("{language} UI{ui} width {width}");
            check(&mut report, &at, &[0], &c);
            for (phase, f) in [
                ("before", &c.before[0]),
                ("during", &c.during[0]),
                ("after", &c.after[0]),
            ] {
                let text = f.title_text.as_ref().map(|l| l.text.clone());
                if !text.as_ref().is_some_and(|t| t.ends_with(tail)) {
                    report.push(format!(
                        "{at}: elision: the ended tail was not whole {phase}: {text:?}"
                    ));
                }
            }
            let before = c.before[0].title_text.as_ref().map(|l| l.text.clone());
            if !before.as_ref().is_some_and(|t| t.contains('…')) {
                report.push(format!(
                    "{at}: setup: the ended name does not elide here: {before:?}"
                ));
            }
        }
        finish(report);
    }

    #[gpui::test]
    fn a5alt_ended_rows_stay_put_ui16(cx: &mut TestAppContext) {
        ended_rows_stay_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_ended_rows_stay_put_ui24(cx: &mut TestAppContext) {
        ended_rows_stay_put(cx, 24.);
    }

    // ---- regular rows: behavior and the trailing slot -------------------

    /// The badges are the existing order, raised and dropped by the chord.
    #[gpui::test]
    fn a5alt_the_chord_numbers_rows_in_order_and_release_drops_them(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = fixture(cx, 3, 16.);
        draw_at(&app, &mut vcx, 260.);
        let positions = |app: &Entity<Tty59App>, vcx: &VisualTestContext| {
            app.read_with(vcx, |app, cx| {
                let sections = sidebar_sections(&app.sidebar_group_keys(cx), &app.sidebar_groups);
                app.sidebar_badge_positions(&sections, cx)
            })
        };
        assert_eq!(positions(&app, &vcx), None, "no badge before the chord");
        let before: Vec<_> = (0..3)
            .map(|i| vcx.debug_bounds(sel("sidebar-row-badge", i)))
            .collect();
        assert!(before.iter().all(Option::is_none), "{before:?}");

        hold(&app, &mut vcx);
        assert_eq!(
            positions(&app, &vcx),
            Some(vec![Some(0), Some(1), Some(2)]),
            "the badges are the rows' visual order"
        );
        let mut tops = Vec::new();
        for i in 0..3 {
            let badge = vcx
                .debug_bounds(sel("sidebar-row-badge", i))
                .unwrap_or_else(|| panic!("row {i} drew no badge under the held chord"));
            tops.push(badge.top());
        }
        assert!(
            tops.windows(2).all(|w| w[0] < w[1]),
            "badge 1, 2, 3 stand top to bottom: {tops:?}"
        );

        release(&app, &mut vcx);
        assert_eq!(positions(&app, &vcx), None);
        for i in 0..3 {
            assert!(
                vcx.debug_bounds(sel("sidebar-row-badge", i)).is_none(),
                "row {i} kept its badge after the release"
            );
        }
    }

    /// The slot the hover × lives in: the live 24x24 close Button (the one
    /// `hit_target` returns), centred vertically in the row at its right
    /// inset, a 10px fade to its left, and the OUTER overlay (fade + button
    /// backing) the row's full height. Unchanged by this work; the control the
    /// Alt badge is compared with.
    #[gpui::test]
    fn a5alt_the_close_slot_is_24px_at_right_6_with_a_10px_fade(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = fixture(cx, 3, 16.);
        draw_at(&app, &mut vcx, 260.);
        let row = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
        vcx.simulate_mouse_move(row.center(), None::<gpui::MouseButton>, Modifiers::none());
        settle(&mut vcx);
        let outer = vcx.debug_bounds("sidebar-row-trailing-1").expect("outer");
        let hit = vcx.debug_bounds("sidebar-row-hit-1").expect("hit");
        let eq = |a: Pixels, b: Pixels| (a - b).abs() <= px(0.01);
        let target = px(crate::ui::tab_strip::MIN_TARGET);
        assert!(
            eq(hit.size.width, target) && eq(hit.size.height, target),
            "the live close hit box is 24x24: {hit:?}"
        );
        assert!(
            eq(hit.center().y, row.center().y),
            "the 24px hit box is centred vertically in the row: {hit:?} in {row:?}"
        );
        assert!(eq(hit.right(), outer.right()), "the hit box ends the slot");
        assert!(
            eq(outer.size.width - hit.size.width, px(10.)),
            "the fade: {outer:?} {hit:?}"
        );
        assert!(eq(row.right() - outer.right(), px(6.)), "right 6");
        assert!(
            eq(outer.size.height, row.size.height) && eq(outer.top(), row.top()),
            "the outer overlay is the row's full height: {outer:?} {row:?}"
        );
    }

    /// The finding: the Alt badge stands in THAT slot — the outer rectangle
    /// and the 24px hit box are the very ones the close button has, and the
    /// 20px badge is centred on the live 24px hit box (not on the 34px outer
    /// rectangle). The pointer rests on the row throughout.
    fn badge_stands_in_the_close_slot(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = fixture(cx, 3, ui);
        draw_at(&app, &mut vcx, 260.);
        let row = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
        vcx.simulate_mouse_move(row.center(), None::<gpui::MouseButton>, Modifiers::none());
        settle(&mut vcx);
        let outer = vcx
            .debug_bounds("sidebar-row-trailing-1")
            .expect("the close slot is laid out");
        let hit = vcx
            .debug_bounds("sidebar-row-hit-1")
            .expect("the close hit box is laid out");

        hold(&app, &mut vcx);
        let mut report = Vec::new();
        let outer_held = vcx.debug_bounds("sidebar-row-trailing-1");
        let hit_held = vcx.debug_bounds("sidebar-row-hit-1");
        let badge = vcx.debug_bounds("sidebar-row-badge-1");
        if badge.is_none() {
            report.push("behavior: no badge under the held chord".to_string());
        }
        if !near_opt(Some(outer), outer_held) {
            report.push(format!(
                "geometry: the held badge's slot is not the close slot: {outer:?} -> {outer_held:?}"
            ));
        }
        if !near_opt(Some(hit), hit_held) {
            report.push(format!(
                "geometry: the held badge's 24px hit box is not the close hit box: \
                 {hit:?} -> {hit_held:?}"
            ));
        }
        // The live close Button, read before the hold, is 24x24 and centred
        // in the row; the outer overlay is the row's full height. Setup
        // facts, kept apart from the finding.
        let target = px(crate::ui::tab_strip::MIN_TARGET);
        if (hit.size.width - target).abs() > px(0.01)
            || (hit.size.height - target).abs() > px(0.01)
            || (hit.center().y - row.center().y).abs() > px(0.01)
            || (outer.size.height - row.size.height).abs() > px(0.01)
        {
            report.push(format!(
                "setup: the close slot is not 24x24 centred in a full-height overlay: \
                 {hit:?} {outer:?} {row:?}"
            ));
        }
        if let Some(badge) = badge {
            if badge.size.width != px(20.) || badge.size.height != px(20.) {
                report.push(format!(
                    "geometry: the badge is not 20x20: {:?}",
                    badge.size
                ));
            }
            // Against the close Button's own centre, not the 34px outer
            // overlay's, and against the held hit box when one is drawn.
            for (what, of) in [("live close", Some(hit)), ("held hit", hit_held)] {
                let Some(of) = of else { continue };
                let (c, h) = (badge.center(), of.center());
                if (c.x - h.x).abs() > px(0.5) || (c.y - h.y).abs() > px(0.5) {
                    report.push(format!(
                        "geometry: the badge centre {c:?} is not the {what} 24px hit centre {h:?}"
                    ));
                }
            }
        }
        finish(report);
    }

    #[gpui::test]
    fn a5alt_the_badge_stands_in_the_close_slot_ui16(cx: &mut TestAppContext) {
        badge_stands_in_the_close_slot(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_the_badge_stands_in_the_close_slot_ui24(cx: &mut TestAppContext) {
        badge_stands_in_the_close_slot(cx, 24.);
    }

    /// What the slot does stays: while the chord is held a click there is the
    /// row's (it activates the row, closes nothing); with the chord up the
    /// same spot is the close button and closes the tab.
    #[gpui::test]
    fn a5alt_a_click_in_the_slot_activates_while_held_and_closes_when_not(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = fixture(cx, 3, 16.);
        draw_at(&app, &mut vcx, 260.);
        let row = app.read_with(&vcx, |app, _| app.sidebar_slots.borrow()[1]);
        vcx.simulate_mouse_move(row.center(), None::<gpui::MouseButton>, Modifiers::none());
        settle(&mut vcx);
        let spot = vcx
            .debug_bounds("sidebar-row-hit-1")
            .expect("the close hit box is laid out")
            .center();
        assert_eq!(app.read_with(&vcx, |app, _| app.active), 0);

        let modifiers = hold(&app, &mut vcx);
        vcx.simulate_click(spot, modifiers);
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| (app.active, app.tabs.len())),
            (1, 3),
            "a click on the held badge activates its row and closes nothing"
        );

        release(&app, &mut vcx);
        vcx.simulate_mouse_move(spot, None::<gpui::MouseButton>, Modifiers::none());
        settle(&mut vcx);
        vcx.simulate_click(spot, Modifiers::none());
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| app.tabs.len()),
            2,
            "without the chord the same spot is the close button"
        );
    }

    /// The status mark keeps its slot (and its value) under the hold: the
    /// layout is the same, the mark is not removed from the line. GEOMETRY AND
    /// VALUE ONLY: this says nothing about whether the mark is visible or
    /// hidden while held (opacity/paint); the native pixel gate proves that.
    #[gpui::test]
    fn a5alt_a_status_mark_keeps_its_place_and_value_under_the_hold(cx: &mut TestAppContext) {
        let (app, mut vcx, mut streams) = fixture(cx, 2, 16.);
        name_tabs(&app, &mut vcx, &ORDINARY);
        report_agent(&mut streams[0], AgentStatus::Waiting);
        wait_tab_status(&app, &mut vcx, 0, AgentStatus::Waiting);
        draw_at(&app, &mut vcx, 260.);
        let mark = |vcx: &mut VisualTestContext| vcx.debug_bounds("row-status-circle-0");
        let before = mark(&mut vcx).expect("the waiting row draws its mark");

        hold(&app, &mut vcx);
        let mut report = Vec::new();
        let during = mark(&mut vcx);
        if !near_opt(Some(before), during) {
            report.push(format!(
                "geometry: the status slot moved or was dropped under the hold: \
                 {before:?} -> {during:?}"
            ));
        }
        let value = app.read_with(&vcx, |app, cx| app.tabs[0].agent_status(cx));
        if value != Some(AgentStatus::Waiting) {
            report.push(format!(
                "value: the status changed under the hold: {value:?}"
            ));
        }
        release(&app, &mut vcx);
        let after = mark(&mut vcx);
        if !near_opt(Some(before), after) {
            report.push(format!(
                "geometry: the status mark is not restored: {before:?} -> {after:?}"
            ));
        }
        finish(report);
    }

    // ---- agent rows ------------------------------------------------------

    const AGENT_NAMES: [&str; 2] = [
        "an-agent-task-with-a-very-long-name-that-no-sidebar-width-can-hold-in-one-line-at-all",
        "another-agent-task-whose-name-is-just-as-long-as-the-first-one-and-then-a-little-more",
    ];

    /// `agents` rows in the Agents list, each a real daemon-reported Waiting
    /// agent, on tabs named for the long titles. The daemon streams are
    /// answered too and the caller keeps them for the whole test: dropping
    /// them ends the panes' feeds mid-test.
    fn agent_fixture(
        cx: &mut TestAppContext,
        agents: usize,
        ui: f32,
    ) -> (Entity<Tty59App>, VisualTestContext, Vec<Stream>) {
        let (app, mut vcx, mut streams) = fixture(cx, agents + 1, ui);
        app.update_in(&mut vcx, |app, window, cx| {
            for (i, name) in AGENT_NAMES.iter().enumerate().take(agents) {
                app.tabs[i].name = Some((*name).into());
            }
            app.select_sidebar_list(SidebarList::Agents, window, cx);
        });
        for stream in streams.iter_mut().take(agents) {
            report_agent(stream, AgentStatus::Waiting);
        }
        let mut ready = false;
        for _ in 0..400 {
            vcx.run_until_parked();
            ready = app.read_with(&vcx, |app, cx| app.agent_pane_summaries(cx).len() == agents);
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready, "the agent reports reached the collector");
        settle(&mut vcx);
        (app, vcx, streams)
    }

    #[derive(Debug)]
    struct AgentFrame {
        row: Option<Bounds<Pixels>>,
        title: Option<Bounds<Pixels>>,
        badge: Option<Bounds<Pixels>>,
        mark: Option<Bounds<Pixels>>,
    }

    fn agent_frame(vcx: &mut VisualTestContext, slot: usize) -> AgentFrame {
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        AgentFrame {
            row: vcx.debug_bounds(sel("sidebar-agent-row", slot)),
            title: vcx.debug_bounds(sel("sidebar-agent-title", slot)),
            badge: vcx.debug_bounds(sel("sidebar-agent-badge", slot)),
            mark: vcx.debug_bounds("row-status-circle-0"),
        }
    }

    /// The agent title is a plain truncating line, so there is no elided text
    /// to read: its drawn bounds are the proof here (same text, same font,
    /// same width, same truncation), and the native pixel check is the other.
    fn agent_title_stays_put(cx: &mut TestAppContext, ui: f32) {
        let (app, mut vcx, _streams) = agent_fixture(cx, 1, ui);
        let mut report = Vec::new();
        let mut narrow_title = None;
        for width in WIDTHS {
            draw_at(&app, &mut vcx, width);
            let before = agent_frame(&mut vcx, 0);
            hold(&app, &mut vcx);
            let during = agent_frame(&mut vcx, 0);
            release(&app, &mut vcx);
            let after = agent_frame(&mut vcx, 0);
            let at = format!("agent UI{ui} width {width}");
            if before.title.is_none() || before.row.is_none() {
                report.push(format!("{at}: setup: row or title not drawn ({before:?})"));
            }
            if before.badge.is_some() || during.badge.is_none() || after.badge.is_some() {
                report.push(format!(
                    "{at}: behavior: the badge must be up only while the chord is held \
                     ({:?} {:?} {:?})",
                    before.badge, during.badge, after.badge
                ));
            }
            for (phase, p) in [("while Alt is held", &during), ("after release", &after)] {
                for (what, x, y) in [("row", before.row, p.row), ("title", before.title, p.title)] {
                    if !near_opt(x, y) {
                        report.push(format!(
                            "{at}: geometry: the {what} moved {phase}: {x:?} -> {y:?}"
                        ));
                    }
                }
            }
            if width == MIN_SIDEBAR_WIDTH {
                narrow_title = before.title;
            } else if let (Some(narrow), Some(wide)) = (narrow_title, before.title)
                && wide.size.width <= narrow.size.width
            {
                report.push(format!(
                    "{at}: setup: the title does not grow with the rail ({narrow:?} -> {wide:?})"
                ));
            }
        }
        finish(report);
    }

    #[gpui::test]
    fn a5alt_agent_title_stays_put_ui16(cx: &mut TestAppContext) {
        agent_title_stays_put(cx, 16.);
    }

    #[gpui::test]
    fn a5alt_agent_title_stays_put_ui24(cx: &mut TestAppContext) {
        agent_title_stays_put(cx, 24.);
    }

    /// The agent's status mark keeps its slot and value while the chord is
    /// held and is where it was after release. GEOMETRY AND VALUE ONLY: whether
    /// the badge visually replaces it is proven by the native pixel gate.
    #[gpui::test]
    fn a5alt_an_agent_status_mark_keeps_its_place_and_value_under_the_hold(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = agent_fixture(cx, 1, 16.);
        draw_at(&app, &mut vcx, 260.);
        let before = agent_frame(&mut vcx, 0);
        let status = |app: &Entity<Tty59App>, vcx: &VisualTestContext| {
            app.read_with(vcx, |app, cx| {
                app.sidebar_agent_rows(&app.agent_pane_summaries(cx), cx)
                    .first()
                    .and_then(|row| row.status())
            })
        };
        assert_eq!(status(&app, &vcx), Some(AgentStatus::Waiting));
        assert!(before.mark.is_some(), "the agent row draws its mark");

        hold(&app, &mut vcx);
        let during = agent_frame(&mut vcx, 0);
        let mut report = Vec::new();
        if !near_opt(before.mark, during.mark) {
            report.push(format!(
                "geometry: the agent status slot moved or was dropped under the hold: \
                 {:?} -> {:?}",
                before.mark, during.mark
            ));
        }
        if status(&app, &vcx) != Some(AgentStatus::Waiting) {
            report.push("value: the agent status changed under the hold".to_string());
        }
        release(&app, &mut vcx);
        let after = agent_frame(&mut vcx, 0);
        if !near_opt(before.mark, after.mark) {
            report.push(format!(
                "geometry: the agent status mark is not restored: {:?} -> {:?}",
                before.mark, after.mark
            ));
        }
        finish(report);
    }

    /// Agent badges follow the list's order under the real chord, and a click
    /// on one activates its row, as before.
    #[gpui::test]
    fn a5alt_agent_badges_follow_the_list_and_a_click_on_one_activates_its_row(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = agent_fixture(cx, 2, 16.);
        draw_at(&app, &mut vcx, 260.);
        assert_eq!(app.read_with(&vcx, |app, _| app.active), 0);
        let modifiers = hold(&app, &mut vcx);
        let first = vcx
            .debug_bounds("sidebar-agent-badge-0")
            .expect("the first agent row draws badge 1");
        let second = vcx
            .debug_bounds("sidebar-agent-badge-1")
            .expect("the second agent row draws badge 2");
        assert!(first.top() < second.top(), "{first:?} above {second:?}");

        vcx.simulate_click(second.center(), modifiers);
        vcx.run_until_parked();
        assert_eq!(
            app.read_with(&vcx, |app, _| app.active),
            1,
            "a click on badge 2 activates the second agent's tab"
        );

        release(&app, &mut vcx);
        assert!(vcx.debug_bounds("sidebar-agent-badge-0").is_none());
        assert!(vcx.debug_bounds("sidebar-agent-badge-1").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_shift_moves_only_rows_out_of_view() {
        let view = (px(100.), px(400.));
        // On screen, or cut by an edge: a click there scrolls nothing.
        assert_eq!(reveal_shift((px(200.), px(230.)), px(0.), view), px(0.));
        assert_eq!(reveal_shift((px(90.), px(120.)), px(0.), view), px(0.));
        assert_eq!(reveal_shift((px(390.), px(420.)), px(0.), view), px(0.));
        // Above: its top lands on the top edge.
        assert_eq!(reveal_shift((px(10.), px(40.)), px(0.), view), px(90.));
        // Below: its bottom lands on the bottom edge.
        assert_eq!(reveal_shift((px(500.), px(530.)), px(0.), view), px(-130.));
    }

    #[test]
    fn a_first_row_revealed_from_above_brings_its_header() {
        let view = (px(100.), px(400.));
        // The header's top, not the row's, lands on the top edge.
        assert_eq!(reveal_shift((px(10.), px(40.)), px(23.), view), px(113.));
        // A header half under the top edge, row on screen: still nothing.
        assert_eq!(reveal_shift((px(110.), px(140.)), px(23.), view), px(0.));
        // Below: the header is already above the row, only the bottom counts.
        assert_eq!(reveal_shift((px(500.), px(530.)), px(23.), view), px(-130.));
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// The auto group keyed on repo root `s`.
    fn g(s: &str) -> GroupKey {
        GroupKey::Auto(AutoKey::Repo(p(s)))
    }

    fn host(s: &str) -> GroupKey {
        GroupKey::Auto(AutoKey::SshHost(s.into()))
    }

    fn none() -> WorkspaceGroups {
        WorkspaceGroups::default()
    }

    /// Two machines' `/home/ubuntu` are two directories: each host gets a
    /// group of its own, named for the host.
    #[test]
    fn ssh_tabs_on_two_hosts_land_in_two_groups() {
        let keys = [
            Some(host("ubuntu@alpha")),
            Some(host("ubuntu@beta")),
            Some(host("ubuntu@alpha")),
        ];
        let sections = sidebar_sections(&keys, &none());
        let shape: Vec<(Option<String>, Vec<usize>)> =
            sections.into_iter().map(|s| (s.name, s.tabs)).collect();
        assert_eq!(
            shape,
            vec![
                (Some("ubuntu@alpha".into()), vec![0, 2]),
                (Some("ubuntu@beta".into()), vec![1]),
            ]
        );
    }

    #[test]
    fn a_shell_that_sshd_onward_groups_by_host_and_wsl_does_not() {
        use crate::daemon::protocol::{RemoteContext, RemoteKind};
        let ctx = |kind| RemoteContext {
            kind,
            argv: vec![],
            target: "u@h".into(),
        };
        assert_eq!(
            ssh_host(Some(&ctx(RemoteKind::Ssh))).as_deref(),
            Some("u@h")
        );
        assert_eq!(
            ssh_host(Some(&ctx(RemoteKind::NativeSsh))).as_deref(),
            Some("u@h")
        );
        assert_eq!(ssh_host(Some(&ctx(RemoteKind::Wsl))), None);
        assert_eq!(ssh_host(None), None);
    }

    #[test]
    fn sections_order_groups_by_first_appearance_ungrouped_last() {
        let keys = vec![
            Some(g("/w/beta")),
            None,
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
        ];
        let sections = sidebar_sections(&keys, &none());
        let shape: Vec<(Option<GroupKey>, Option<String>, Vec<usize>)> = sections
            .into_iter()
            .map(|s| (s.key, s.name, s.tabs))
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some(g("/w/beta")), Some("beta".into()), vec![0, 3]),
                (Some(g("/w/alpha")), Some("alpha".into()), vec![2]),
                (None, Some("Ungrouped".into()), vec![1]),
            ]
        );

        let flat = sidebar_sections(&[None, None], &none());
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].name, None);
        assert_eq!(flat[0].tabs, vec![0, 1]);
    }

    /// Pinned groups come first, in the order the user gave them, whatever
    /// order their tabs are in — and an empty one keeps its place.
    #[test]
    fn pinned_groups_lead_in_their_own_order_empty_or_not() {
        let mut groups = none();
        let (a, b, c) = (
            PinnedGroup::label("a"),
            PinnedGroup::folder(Path::new("/w/b")),
            PinnedGroup::label("c"),
        );
        groups.pinned = vec![a.clone(), b.clone(), c.clone()];
        let keys = vec![
            Some(g("/w/r")),
            Some(GroupKey::Pinned(c.id)),
            Some(GroupKey::Pinned(a.id)),
            None,
        ];
        let sections = sidebar_sections(&keys, &groups);
        let shape: Vec<(Option<String>, Vec<usize>)> =
            sections.into_iter().map(|s| (s.name, s.tabs)).collect();
        assert_eq!(
            shape,
            vec![
                (Some("a".into()), vec![2]),
                (Some("b".into()), vec![]),
                (Some("c".into()), vec![1]),
                (Some("r".into()), vec![0]),
                (Some("Ungrouped".into()), vec![3]),
            ]
        );
    }

    /// "Move to Group" offers every group the sidebar draws — auto groups
    /// too, not just the pinned ones — in sidebar order.
    #[test]
    fn move_targets_list_every_sidebar_group_in_order() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let keys = vec![
            Some(g("/w/r")),
            Some(GroupKey::Pinned(work.id)),
            Some(host("u@h")),
            None,
        ];
        let shape = |index| -> Vec<(GroupKey, String, bool)> {
            move_targets(&keys, &groups, index)
                .into_iter()
                .map(|m| (m.key, m.name, m.checked))
                .collect()
        };
        assert_eq!(
            shape(0),
            vec![
                (GroupKey::Pinned(work.id), "work".into(), false),
                (g("/w/r"), "r".into(), true),
                (host("u@h"), "u@h".into(), false),
            ]
        );
        let checked = |index| -> Vec<bool> { shape(index).into_iter().map(|m| m.2).collect() };
        assert_eq!(checked(1), vec![true, false, false]);
        assert_eq!(
            checked(3),
            vec![false, false, false],
            "an ungrouped tab is in none"
        );
        assert!(move_targets(&[None], &none(), 0).is_empty());
    }

    /// With pinned groups and nothing auto-grouped below them, the rest is
    /// just the list — a header reading "Ungrouped" over all of it would be a
    /// label on nothing.
    #[test]
    fn below_pinned_groups_the_rest_needs_no_header_on_its_own() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let keys = vec![Some(GroupKey::Pinned(work.id)), None];
        let sections = sidebar_sections(&keys, &groups);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].name, None);
        assert_eq!(sections[1].tabs, vec![1]);
    }

    /// The badge on a row and the tab ⌘N opens are two readings of one order,
    /// taken in two places. Grouping makes them diverge from `self.tabs`
    /// order — tab 3 sits in the second row here — so if they are ever read
    /// off different things, the badge names a chord that opens another tab.
    #[test]
    fn a_row_badge_names_the_chord_that_opens_that_row() {
        let keys = vec![
            Some(g("/w/beta")),
            None,
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
        ];
        // What `visual_tab_order` returns for a left tab bar.
        let order: Vec<usize> = sidebar_sections(&keys, &none())
            .into_iter()
            .flat_map(|s| s.tabs)
            .collect();
        assert_eq!(order, vec![0, 3, 2, 1]);

        let mut badge_pos = vec![0usize; keys.len()];
        for (n, i) in order.iter().copied().enumerate() {
            badge_pos[i] = n;
        }
        for (row, tab) in order.iter().copied().enumerate() {
            // ActivateTabN → activate_visual(N - 1) → order[N - 1].
            let chord = tab_badge_label(badge_pos[tab]);
            let opens = order[badge_pos[tab]];
            assert_eq!(
                opens, tab,
                "row {row} badges ⌘{chord}, which opens tab {opens}"
            );
        }
    }

    #[test]
    fn reordered_rows_moves_within_the_group_only() {
        let keys = vec![
            Some(g("/w/alpha")),
            Some(g("/w/beta")),
            Some(g("/w/alpha")),
            None,
        ];
        let alpha = Some(g("/w/alpha"));
        assert_eq!(
            reordered_rows(&keys, &none(), &alpha, &[0, 2], 0, 1),
            Some(vec![2, 0, 1, 3])
        );
        assert_eq!(
            reordered_rows(&keys, &none(), &alpha, &[0, 2], 1, 0),
            Some(vec![2, 0, 1, 3])
        );
        assert_eq!(reordered_rows(&keys, &none(), &alpha, &[0, 2], 1, 1), None);
    }

    #[test]
    fn reordered_rows_leaves_filtered_out_rows_alone() {
        let keys = vec![Some(g("/w/a")), Some(g("/w/a")), Some(g("/w/a"))];
        let a = Some(g("/w/a"));
        assert_eq!(
            reordered_rows(&keys, &none(), &a, &[0, 2], 0, 1),
            Some(vec![1, 2, 0])
        );
    }

    /// A pinned group is where a tab was put, and rows move within it the
    /// same way they move within a repo group.
    #[test]
    fn rows_reorder_inside_a_pinned_group_too() {
        let mut groups = none();
        let work = PinnedGroup::label("work");
        groups.pinned = vec![work.clone()];
        let w = Some(GroupKey::Pinned(work.id));
        let keys = vec![w.clone(), Some(g("/w/beta")), w.clone()];
        assert_eq!(
            reordered_rows(&keys, &groups, &w, &[0, 2], 0, 1),
            Some(vec![2, 0, 1])
        );
    }

    #[test]
    fn regrouped_order_moves_the_group_into_the_target_slot() {
        let keys = vec![
            Some(g("/w/alpha")),
            None,
            Some(g("/w/beta")),
            Some(g("/w/alpha")),
            Some(g("/w/gamma")),
        ];
        let r = |s: &str| AutoKey::Repo(p(s));
        assert_eq!(
            regrouped_order(&keys, &r("/w/gamma"), &r("/w/alpha")),
            Some(vec![4, 0, 3, 2, 1])
        );
        assert_eq!(
            regrouped_order(&keys, &r("/w/alpha"), &r("/w/gamma")),
            Some(vec![2, 4, 0, 3, 1])
        );
    }

    #[test]
    fn regrouped_order_ignores_self_and_unknown_roots() {
        let keys = vec![Some(g("/w/alpha")), Some(g("/w/beta"))];
        let r = |s: &str| AutoKey::Repo(p(s));
        assert_eq!(regrouped_order(&keys, &r("/w/alpha"), &r("/w/alpha")), None);
        assert_eq!(regrouped_order(&keys, &r("/w/gone"), &r("/w/beta")), None);
        assert_eq!(regrouped_order(&keys, &r("/w/alpha"), &r("/w/gone")), None);
    }

    #[test]
    fn reordered_pinned_moves_a_header_among_the_ones_shown() {
        let ids: Vec<GroupId> = (0..4).map(|_| GroupId::new()).collect();
        // All four shown: dragging the first onto the third.
        assert_eq!(reordered_pinned(&ids, &ids, 0, 2), Some(vec![1, 2, 0, 3]));
        assert_eq!(reordered_pinned(&ids, &ids, 3, 0), Some(vec![3, 0, 1, 2]));
        assert_eq!(reordered_pinned(&ids, &ids, 1, 1), None);
        // A search hides the second; it keeps its place.
        let shown = [ids[0], ids[2], ids[3]];
        assert_eq!(reordered_pinned(&ids, &shown, 2, 0), Some(vec![3, 0, 1, 2]));
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use crate::core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
    use crate::core::session::WorkspaceId;
    use crate::ui::agent_vision::PaneLocation;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::host_ops::HostId;
    use gpui::Entity;
    use tty59_core::core::machine::TabId;

    fn summary(workspace: WorkspaceId, tab: TabId, host: HostId, pane: u64) -> PaneSummary {
        PaneSummary::from_live(
            PaneLocation {
                key: PaneKey { host, pane },
                workspace,
                tab,
                tab_title: "Task".into(),
                group: None,
            },
            Some(CLIAgent::Codex),
            Some(&AgentSessionState {
                status: AgentStatus::Waiting,
                ..Default::default()
            }),
            false,
        )
        .unwrap()
    }

    #[test]
    fn agent_rows_follow_grouped_tabs_and_match_machine_pane_and_workspace() {
        let workspace = WorkspaceId::new();
        let first = TabId::new();
        let second = TabId::new();
        let local = PaneKey {
            host: HostId::LOCAL,
            pane: 7,
        };
        let remote = PaneKey {
            host: HostId(42),
            pane: 7,
        };
        let shell = PaneKey {
            host: HostId::LOCAL,
            pane: 8,
        };
        let summaries = vec![
            summary(workspace, first, local.host, local.pane),
            summary(workspace, second, remote.host, remote.pane),
            summary(WorkspaceId::new(), first, shell.host, shell.pane),
        ];
        // Tab 1 is drawn before tab 0 because it belongs to the first group.
        let layout = [(1, second, vec![remote]), (0, first, vec![shell, local])];
        let rows = build_agent_rows(workspace, &layout, &summaries);
        assert_eq!(rows.len(), 2, "the shell and other workspace are excluded");
        assert_eq!(rows.iter().map(|row| row.tab).collect::<Vec<_>>(), [1, 0]);
        assert_eq!(rows[0].summary.location.key, remote);
        assert_eq!(rows[1].summary.location.key, local);
        assert_eq!(rows[1].status(), Some(AgentStatus::Waiting));
    }

    #[gpui::test]
    fn agent_rows_use_the_real_sidebar_project_order(cx: &mut gpui::TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
        app.update_in(&mut vcx, |app, _, cx| {
            let project = PinnedGroup::label("Project");
            app.tabs[2].group.set(Some(project.id));
            app.sidebar_groups.pinned.push(project);
            let summaries: Vec<_> = [0, 2]
                .into_iter()
                .map(|i| {
                    let pane = app.tabs[i].pane.first_leaf().unwrap();
                    let view = pane.terminal().unwrap().read(cx);
                    summary(
                        app.workspace,
                        app.tabs[i].tree_id.get(),
                        view.host_id(),
                        view.pane_id,
                    )
                })
                .collect();
            let rows = app.sidebar_agent_rows(&summaries, cx);
            assert_eq!(rows.iter().map(|row| row.tab).collect::<Vec<_>>(), [2, 0]);
            let keys = app.sidebar_group_keys(cx);
            assert!(matches!(keys[2], Some(GroupKey::Pinned(_))));
            assert_eq!(rows[0].summary.location.tab, app.tabs[2].tree_id.get());
        });
    }

    #[gpui::test]
    fn agent_selection_activates_the_tab_and_sets_maximize_idempotently(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::pane::{Pane, PaneSlot};
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        app.update_in(&mut vcx, |app, window, cx| {
            let (sibling, _stream) = quiet_test_pane(9, window, cx);
            let first = app.tabs[1].pane.first_leaf().unwrap();
            let original = first.terminal().unwrap().clone();
            app.tabs[1].pane = Pane::split_node(
                gpui::Axis::Horizontal,
                0.5,
                Pane::leaf(first),
                Pane::leaf(PaneSlot::Ready(sibling.clone())),
            );
            let key = PaneKey {
                host: original.read(cx).host_id(),
                pane: original.read(cx).pane_id,
            };
            app.activate_sidebar_agent(key, window, cx);
            assert_eq!(app.active, 1);
            assert_eq!(app.maximized.as_ref(), Some(&original));
            app.activate_sidebar_agent(key, window, cx);
            assert_eq!(
                app.maximized.as_ref(),
                Some(&original),
                "repeated row selection keeps its maximize"
            );
            let sibling_key = PaneKey {
                host: sibling.read(cx).host_id(),
                pane: 9,
            };
            app.activate_sidebar_agent(sibling_key, window, cx);
            assert_eq!(
                app.maximized.as_ref(),
                Some(&sibling),
                "selecting a sibling replaces the maximize"
            );
            assert!(sibling.read(cx).focus_handle.is_focused(window));
            // A key from another host must never resolve to the local pane.
            app.activate_sidebar_agent(
                PaneKey {
                    host: HostId(42),
                    pane: 9,
                },
                window,
                cx,
            );
            assert_eq!(app.maximized.as_ref(), Some(&sibling));
        });
    }

    #[gpui::test]
    fn sidebar_list_is_window_local_defaults_to_terminal_and_settings_disable_agents(
        cx: &mut gpui::TestAppContext,
    ) {
        let (first, mut vcx, _streams) = harness_with_tabs(cx, 1);
        first.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.sidebar_list, SidebarList::Terminal);
            assert!(!app.sidebar_agents_mode(cx));
            app.select_sidebar_list(SidebarList::Agents, window, cx);
            assert!(app.sidebar_agents_mode(cx));
            app.set_sidebar_agents_toggle(false, cx);
            assert_eq!(app.sidebar_list, SidebarList::Terminal);
            assert!(!app.sidebar_agents_mode(cx));
            app.set_sidebar_agents_toggle(true, cx);
            assert!(!app.sidebar_agents_mode(cx));
            app.select_sidebar_list(SidebarList::Agents, window, cx);
        });
        let (second, mut second_vcx, _other_streams) = harness_with_tabs(cx, 1);
        second.update_in(&mut second_vcx, |app, _, _| {
            assert_eq!(app.sidebar_list, SidebarList::Terminal)
        });
        assert_eq!(
            first.read_with(&vcx, |app, _| app.sidebar_list),
            SidebarList::Agents
        );
    }

    fn settle_and_draw(vcx: &mut gpui::VisualTestContext) {
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
    }

    /// A window on the Agents list with no agent running, the keymap's
    /// New Agent Tab binding replaced by `binding` when one is given.
    fn empty_agents_rail(
        cx: &mut gpui::TestAppContext,
        binding: Option<crate::core::config::KeybindingOverride>,
    ) -> (
        Entity<Tty59App>,
        gpui::VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) =
            crate::ui::app::test_window::harness_with_tabs_configured(cx, 1, |cfg| {
                if let Some(binding) = binding {
                    cfg.keybindings.insert("NewAgentTab".to_string(), binding);
                }
            });
        app.update_in(&mut vcx, |app, window, cx| {
            app.select_sidebar_list(SidebarList::Agents, window, cx)
        });
        settle_and_draw(&mut vcx);
        (app, vcx, streams)
    }

    fn strokes(specs: &[&str]) -> Vec<(String, gpui::Modifiers)> {
        specs
            .iter()
            .map(|spec| {
                let stroke = gpui::Keystroke::parse(spec).expect("the expected stroke parses");
                (stroke.key, stroke.modifiers)
            })
            .collect()
    }

    /// The chord New Agent Tab ships on, spelled out per platform rather than
    /// read from the keymap: macOS cmd-shift-a; Windows ctrl-alt-shift-insert
    /// (Insert avoids the tested Polish AltGr printable shapes); Linux
    /// ctrl-alt-shift-a.
    fn shipped_new_agent_chord() -> Vec<(String, gpui::Modifiers)> {
        if cfg!(target_os = "macos") {
            strokes(&["cmd-shift-a"])
        } else if cfg!(target_os = "windows") {
            strokes(&["ctrl-alt-shift-insert"])
        } else {
            strokes(&["ctrl-alt-shift-a"])
        }
    }

    fn rail_chord(
        app: &Entity<Tty59App>,
        vcx: &mut gpui::VisualTestContext,
    ) -> Vec<(String, gpui::Modifiers)> {
        app.update_in(vcx, |_, _, cx| {
            new_agent_chord(cx)
                .into_iter()
                .map(|stroke| (stroke.key, stroke.modifiers))
                .collect()
        })
    }

    #[gpui::test]
    fn an_empty_agents_rail_says_so_under_the_search_field(cx: &mut gpui::TestAppContext) {
        let (_app, mut vcx, _streams) = empty_agents_rail(cx, None);
        let search = vcx
            .debug_bounds("sidebar-search")
            .expect("the search field");
        let empty = vcx
            .debug_bounds("sidebar-empty-state")
            .expect("an empty Agents list paints its message");
        let toggle = vcx.debug_bounds("sidebar-list-toggle").expect("the switch");
        assert!(
            empty.top() >= search.bottom(),
            "the message starts below the search field: {empty:?} vs {search:?}"
        );
        assert!(
            empty.top() - search.bottom() < px(ROW_HEIGHT * 3.),
            "the message sits where the first row would, not at the bottom: {empty:?} vs {search:?}"
        );
        assert!(
            toggle.top() - empty.bottom() > px(ROW_HEIGHT * 3.),
            "the Terminal/Agents switch is far below it: {empty:?} vs {toggle:?}"
        );
        assert!(
            vcx.debug_bounds("sidebar-new-agent-cta").is_some(),
            "an empty list offers the way out"
        );
    }

    #[gpui::test]
    fn an_exact_binding_replaces_the_chord_on_the_new_agent_row(cx: &mut gpui::TestAppContext) {
        use crate::core::config::KeybindingOverride::Exact;
        let (app, mut vcx, _streams) =
            empty_agents_rail(cx, Some(Exact(vec!["ctrl-alt-n".into()])));
        assert_eq!(rail_chord(&app, &mut vcx), strokes(&["ctrl-alt-n"]));
        assert!(vcx.debug_bounds("sidebar-new-agent-chord").is_some());
    }

    #[gpui::test]
    fn a_two_stroke_binding_shows_every_stroke_in_order(cx: &mut gpui::TestAppContext) {
        use crate::core::config::KeybindingOverride::Exact;
        let (app, mut vcx, _streams) = empty_agents_rail(
            cx,
            Some(Exact(vec!["ctrl-b a".into(), "ctrl-alt-n".into()])),
        );
        assert_eq!(
            rail_chord(&app, &mut vcx),
            strokes(&["ctrl-b", "a"]),
            "every stroke of the first binding, in order"
        );
        assert!(vcx.debug_bounds("sidebar-new-agent-chord").is_some());
    }

    #[gpui::test]
    fn a_string_override_adds_a_chord_beside_the_default(cx: &mut gpui::TestAppContext) {
        use crate::core::config::KeybindingOverride::Add;
        let (app, mut vcx, _streams) = empty_agents_rail(cx, Some(Add("ctrl-alt-m".into())));
        let chord = rail_chord(&app, &mut vcx);
        // Every platform ships a New Agent Tab chord and the added one comes
        // after it, so the first binding the keymap holds is the shipped one.
        assert_eq!(
            chord,
            shipped_new_agent_chord(),
            "the first binding the keymap holds"
        );
        assert!(vcx.debug_bounds("sidebar-new-agent-chord").is_some());
    }

    #[gpui::test]
    fn an_unbound_new_agent_row_advertises_no_chord(cx: &mut gpui::TestAppContext) {
        use crate::core::config::KeybindingOverride::Exact;
        let (app, mut vcx, _streams) = empty_agents_rail(cx, Some(Exact(Vec::new())));
        assert!(rail_chord(&app, &mut vcx).is_empty());
        assert!(
            vcx.debug_bounds("sidebar-new-agent-cta").is_some(),
            "the action stays when it has no chord"
        );
        assert!(
            vcx.debug_bounds("sidebar-new-agent-chord").is_none(),
            "an unbound action advertises no chord"
        );
    }

    #[gpui::test]
    fn the_new_agent_row_follows_the_shipped_default(cx: &mut gpui::TestAppContext) {
        let (app, mut vcx, _streams) = empty_agents_rail(cx, None);
        assert_eq!(rail_chord(&app, &mut vcx), shipped_new_agent_chord());
        assert!(
            vcx.debug_bounds("sidebar-new-agent-chord").is_some(),
            "every platform ships New Agent Tab on a chord, so the row draws it"
        );
    }

    /// With no agent installed, New agent says so and opens nothing. The
    /// fixture makes that true on any machine without touching the process
    /// environment: an agent is looked up by the program its `agent_launch`
    /// entry runs, so pointing every entry at a program that does not exist
    /// leaves nothing installed. A plain New Tab would open a shell (a new
    /// tab, or `startup_error` if the spawn failed) and say nothing; installing
    /// an agent and actually launching it needs a real shell spawn, which this
    /// harness does not verify.
    #[gpui::test]
    fn the_new_agent_row_runs_new_agent_tab_not_new_tab(cx: &mut gpui::TestAppContext) {
        use gpui_component::WindowExt as _;
        let (app, mut vcx, _streams) =
            crate::ui::app::test_window::harness_with_tabs_configured(cx, 1, |cfg| {
                for agent in CLIAgent::ALL {
                    cfg.agent_launch.insert(
                        agent.slug().to_string(),
                        "tty59-test-no-such-agent".to_string(),
                    );
                }
            });
        app.update_in(&mut vcx, |app, window, cx| {
            app.select_sidebar_list(SidebarList::Agents, window, cx)
        });
        settle_and_draw(&mut vcx);
        let (tabs, notices) = app.update_in(&mut vcx, |app, window, cx| {
            assert!(
                app.offered_agents(cx).is_empty(),
                "the fixture leaves no agent installed"
            );
            assert!(app.startup_error.is_none());
            (app.tabs.len(), window.notifications(cx).len())
        });
        let cta = vcx.debug_bounds("sidebar-new-agent-cta").expect("the row");
        vcx.simulate_click(cta.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(
                window.notifications(cx).len(),
                notices + 1,
                "the existing no-agent notice appears"
            );
            assert_eq!(app.tabs.len(), tabs, "no agent, so no tab opened");
            assert!(
                app.startup_error.is_none(),
                "and no shell was attempted either"
            );
        });
    }

    /// Two agents are installed and the one launched last is not the one the
    /// list offers first: a click on New agent has to ask for the last one,
    /// in a new tab, with the command its `agent_launch` entry gives. The
    /// request is read where `launch_agent` is entered, before any shell is
    /// opened, so the shell spawn, the typed command and the frecency stamp
    /// are not covered here.
    #[gpui::test]
    fn the_new_agent_row_launches_the_most_recent_installed_agent(cx: &mut gpui::TestAppContext) {
        use crate::core::config::{ProfileUsage, unix_now};
        use gpui_component::WindowExt as _;
        let exe = std::env::current_exe()
            .expect("the test binary has a path")
            .display()
            .to_string();
        assert!(
            !exe.contains(char::is_whitespace),
            "the stub program is typed as a command line: {exe}"
        );
        let (offered_first, last_launched) = (CLIAgent::Claude, CLIAgent::Codex);
        let now = unix_now();
        let (app, mut vcx, _streams) =
            crate::ui::app::test_window::harness_with_tabs_configured(cx, 1, |cfg| {
                for agent in CLIAgent::ALL {
                    cfg.agent_launch.insert(
                        agent.slug().to_string(),
                        "tty59-test-no-such-agent".to_string(),
                    );
                }
                cfg.agent_launch
                    .insert(offered_first.slug().to_string(), format!("{exe} --stub-a"));
                cfg.agent_launch
                    .insert(last_launched.slug().to_string(), format!("{exe} --stub-b"));
                // Used a lot, a minute ago: the most frecent, so offered first.
                cfg.agent_frecency.insert(
                    offered_first.slug().to_string(),
                    ProfileUsage {
                        count: 100,
                        last_used: now - 60,
                    },
                );
                // Used once, just now: the most recent.
                cfg.agent_frecency.insert(
                    last_launched.slug().to_string(),
                    ProfileUsage {
                        count: 1,
                        last_used: now,
                    },
                );
            });
        app.update_in(&mut vcx, |app, window, cx| {
            app.select_sidebar_list(SidebarList::Agents, window, cx)
        });
        settle_and_draw(&mut vcx);
        let (tabs, notices) = app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(
                app.offered_agents(cx),
                [offered_first, last_launched],
                "the list offers the most frecent first, which is not the most recent"
            );
            assert!(app.startup_error.is_none());
            (app.tabs.len(), window.notifications(cx).len())
        });
        let launches = crate::ui::agent_launch::intercept_launches();
        let cta = vcx.debug_bounds("sidebar-new-agent-cta").expect("the row");
        vcx.simulate_click(cta.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
        let requests = launches.requests();
        assert_eq!(requests.len(), 1, "one launch request: {requests:?}");
        let (agent, at, command) = &requests[0];
        assert_eq!(
            *agent, last_launched,
            "the most recent agent, not the first offered"
        );
        assert!(
            matches!(at, crate::ui::app::SpawnWhere::NewTab),
            "in a new tab: {at:?}"
        );
        assert_eq!(
            command,
            &format!("{exe} --stub-b"),
            "its configured command"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.tabs.len(), tabs, "the request stopped before any shell");
            assert_eq!(window.notifications(cx).len(), notices, "no notice");
            assert!(app.startup_error.is_none());
        });
    }

    fn nothing_matches_is_top_aligned_and_quotes_the_query_as_typed(
        cx: &mut gpui::TestAppContext,
        agents: bool,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            if agents {
                app.select_sidebar_list(SidebarList::Agents, window, cx);
            }
            app.sidebar_search
                .update(cx, |state, cx| state.set_value("ZzQ", window, cx));
        });
        let _ = take_rendered_sidebar_nothing_matches();
        settle_and_draw(&mut vcx);
        let lines = take_rendered_sidebar_nothing_matches();
        assert!(!lines.is_empty(), "the rail drew its no-match sentence");
        for line in &lines {
            assert!(
                line.contains("ZzQ") && !line.contains("zzq"),
                "the sentence quotes what was typed, case kept: {line:?}"
            );
            assert!(
                line.contains('\u{201c}') && line.contains('\u{201d}'),
                "the typed text sits in real quotes: {line:?}"
            );
        }
        let search = vcx
            .debug_bounds("sidebar-search")
            .expect("the search field");
        let empty = vcx
            .debug_bounds("sidebar-nothing-matches")
            .expect("the no-match line is painted");
        let toggle = vcx.debug_bounds("sidebar-list-toggle").expect("the switch");
        assert!(
            empty.top() - search.bottom() < px(ROW_HEIGHT * 3.),
            "top-aligned under the search: {empty:?} vs {search:?}"
        );
        assert!(
            toggle.top() - empty.bottom() > px(ROW_HEIGHT * 3.),
            "not jammed against the switch: {empty:?} vs {toggle:?}"
        );
        assert!(
            vcx.debug_bounds("sidebar-new-agent-cta").is_none(),
            "a failed search offers no launch row"
        );
        assert!(vcx.debug_bounds("sidebar-empty-state").is_none());
    }

    #[gpui::test]
    fn an_agents_search_with_no_match_is_top_aligned_and_quoted(cx: &mut gpui::TestAppContext) {
        nothing_matches_is_top_aligned_and_quotes_the_query_as_typed(cx, true);
    }

    #[gpui::test]
    fn a_tabs_search_with_no_match_is_top_aligned_and_quoted(cx: &mut gpui::TestAppContext) {
        nothing_matches_is_top_aligned_and_quotes_the_query_as_typed(cx, false);
    }

    #[test]
    fn the_rail_search_is_agents_search_only_on_an_enabled_agents_list() {
        for (enabled, list, expected) in [
            (true, SidebarList::Agents, L10nKey::VisionSearch),
            (true, SidebarList::Terminal, L10nKey::SearchTabs),
            (false, SidebarList::Agents, L10nKey::SearchTabs),
            (false, SidebarList::Terminal, L10nKey::SearchTabs),
        ] {
            assert_eq!(
                sidebar_search_placeholder(enabled, list),
                expected,
                "enabled={enabled} list={list:?}"
            );
        }
    }

    /// The field's placeholder is written from events, and `set_placeholder`
    /// notifies the field, so a notification is the witness that each event
    /// reached it. A frame that only draws must not write it.
    #[gpui::test]
    fn the_rail_placeholder_is_written_by_events_and_never_by_a_draw(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let writes = Rc::new(Cell::new(0usize));
        let input = app.read_with(&vcx, |app, _| app.sidebar_search.clone());
        vcx.update(|_, cx| {
            let writes = writes.clone();
            cx.observe(&input, move |_, _| writes.set(writes.get() + 1))
                .detach();
        });
        settle_and_draw(&mut vcx);
        let idle = writes.get();
        settle_and_draw(&mut vcx);
        assert_eq!(writes.get(), idle, "drawing writes no placeholder");

        app.update_in(&mut vcx, |app, window, cx| {
            app.select_sidebar_list(SidebarList::Agents, window, cx)
        });
        vcx.run_until_parked();
        assert!(writes.get() > idle, "the list switch writes it");

        let before = writes.get();
        app.update_in(&mut vcx, |app, window, cx| {
            app.refresh_locale_state(window, cx)
        });
        vcx.run_until_parked();
        assert!(writes.get() > before, "a language change writes it");

        // Turned off from the file: the list is still Agents, but the rail
        // is not in agent mode, so the name has to go back.
        let before = writes.get();
        vcx.update(|_, cx| cx.global_mut::<Config>().sidebar_agents_toggle = false);
        vcx.run_until_parked();
        app.read_with(&vcx, |app, cx| {
            assert_eq!(app.sidebar_list, SidebarList::Agents);
            assert!(!app.sidebar_agents_mode(cx));
        });
        assert!(writes.get() > before, "an external config change writes it");
    }

    #[gpui::test]
    fn sidebar_list_toggle_stays_visible_while_the_tab_list_scrolls(cx: &mut gpui::TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 25);
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let before = vcx
            .debug_bounds("sidebar-list-toggle")
            .expect("the bottom selector was painted");
        assert!(before.size.height > px(0.));
        let viewport = vcx.update(|window, _| window.viewport_size());
        assert!(
            before.top() >= px(0.) && before.bottom() <= viewport.height,
            "the selector is inside the visible window"
        );
        app.update_in(&mut vcx, |app, _, cx| {
            app.sidebar_scroll
                .set_offset(gpui::point(px(0.), px(-400.)));
            cx.notify();
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let after = vcx.debug_bounds("sidebar-list-toggle").unwrap();
        assert!(
            app.read_with(&vcx, |app, _| app.sidebar_scroll.offset().y < px(0.)),
            "the test actually scrolled the overflowing list"
        );
        assert_eq!(
            after, before,
            "scrolling the tab rows does not move the selector"
        );
    }

    fn report_agent(stream: &mut crate::daemon::transport::Stream, status: AgentStatus) {
        use crate::daemon::protocol::DaemonMsg;
        DaemonMsg::Agent(Some(CLIAgent::Codex))
            .encode(stream)
            .unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            ..Default::default()
        }))
        .encode(stream)
        .unwrap();
    }

    #[gpui::test]
    fn agent_rows_render_live_and_clicks_use_the_maximize_path(cx: &mut gpui::TestAppContext) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::pane::{Pane, PaneSlot};
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        let mut sibling_stream = app.update_in(&mut vcx, |app, window, cx| {
            let (sibling, stream) = quiet_test_pane(9, window, cx);
            let (shell, shell_stream) = quiet_test_pane(10, window, cx);
            streams.push(shell_stream);
            let first = app.tabs[0].pane.first_leaf().unwrap();
            app.tabs[0].pane = Pane::split_node(
                gpui::Axis::Horizontal,
                0.5,
                Pane::leaf(first),
                Pane::split_node(
                    gpui::Axis::Vertical,
                    0.5,
                    Pane::leaf(PaneSlot::Ready(sibling)),
                    Pane::leaf(PaneSlot::Ready(shell)),
                ),
            );
            let shell_project = PinnedGroup::label("Shells");
            let agent_project = PinnedGroup::label("Project");
            app.tabs[1].group.set(Some(shell_project.id));
            app.tabs[0].group.set(Some(agent_project.id));
            app.sidebar_groups
                .pinned
                .extend([shell_project, agent_project]);
            app.select_sidebar_list(SidebarList::Agents, window, cx);
            stream
        });
        report_agent(&mut streams[0], AgentStatus::Working);
        report_agent(&mut sibling_stream, AgentStatus::Waiting);
        let mut ready = false;
        for _ in 0..200 {
            ready = app.read_with(&vcx, |app, cx| app.agent_pane_summaries(cx).len() == 2);
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            ready,
            "both daemon agent reports reached the shared collector"
        );
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let row = vcx
            .debug_bounds("sidebar-agent-row-1")
            .expect("second agent row was painted");
        vcx.simulate_click(row.center(), gpui::Modifiers::none());
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.active, 0);
            let maximized = app
                .maximized
                .as_ref()
                .expect("agent row click maximized the pane");
            assert_eq!(maximized.read(cx).pane_id, 9);
            assert!(maximized.read(cx).focus_handle.is_focused(window));
            let rows = app.sidebar_agent_rows(&app.agent_pane_summaries(cx), cx);
            assert_eq!(rows.len(), 2, "the unrelated shell is absent");
            assert_eq!(rows[1].status(), Some(AgentStatus::Waiting));
            assert!(
                rows[1].marker().unwrap().contains("2/3"),
                "split positions count the shell pane too"
            );
            app.activate_visual(0, window, cx);
            assert_eq!(
                app.maximized.as_ref().unwrap().read(cx).pane_id,
                1,
                "number shortcuts target agent panes"
            );
            app.cycle_sidebar_agent(true, window, cx);
            assert_eq!(
                app.maximized.as_ref().unwrap().read(cx).pane_id,
                9,
                "next skips shells"
            );
            app.set_tab_group_shortcuts(true, cx);
            app.activate(1, window, cx);
            app.activate_group_visual(0, window, cx);
            assert_eq!(
                app.active, 0,
                "group shortcuts skip the project with only a shell"
            );
            assert_eq!(app.maximized.as_ref().unwrap().read(cx).pane_id, 9);
            app.mod_hint_badges = true;
            app.mod_hint_group = true;
            let sections = sidebar_sections(&app.sidebar_group_keys(cx), &app.sidebar_groups);
            let positions = app.sidebar_badge_positions(&sections, cx).unwrap();
            assert_eq!(positions[0], Some(0));
            assert_eq!(
                positions[1], None,
                "the shell-only project's header has no shortcut badge"
            );
            app.mod_hint_badges = false;
        });
        report_agent(&mut sibling_stream, AgentStatus::Done);
        let mut updated = false;
        for _ in 0..200 {
            updated = app.read_with(&vcx, |app, cx| {
                app.sidebar_agent_rows(&app.agent_pane_summaries(cx), cx)
                    .get(1)
                    .is_some_and(|row| row.status() == Some(AgentStatus::Done))
            });
            if updated {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(updated, "daemon status changes reach the row model live");
        app.update_in(&mut vcx, |app, _, cx| {
            app.sidebar_groups.pinned.clear();
            for tab in &app.tabs {
                tab.group.set(None);
            }
            app.mod_hint_badges = true;
            app.mod_hint_group = true;
            cx.notify();
        });
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        assert!(
            vcx.debug_bounds("sidebar-agent-badge-0").is_some(),
            "a headerless project's shortcut badge appears on its first agent row"
        );
        assert!(
            vcx.debug_bounds("sidebar-agent-badge-1").is_none(),
            "the project badge appears only once"
        );
        vcx.dispatch_action(crate::core::actions::ToggleMaximizePane);
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(
                app.maximized.is_none(),
                "the existing maximize shortcut restores the split"
            );
            assert_eq!(
                app.tabs[0]
                    .pane
                    .focused_leaf(window, cx)
                    .unwrap()
                    .terminal()
                    .unwrap()
                    .read(cx)
                    .pane_id,
                9
            );
        });
    }

    #[gpui::test]
    fn sidebar_agent_row_keeps_exited_identity_without_dot(cx: &mut gpui::TestAppContext) {
        use crate::core::cli_agent::AgentDetectionSession;
        use crate::daemon::protocol::DaemonMsg;
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 2);
        app.update_in(&mut vcx, |app, window, cx| {
            app.select_sidebar_list(SidebarList::Agents, window, cx);
        });
        for (stream, (dir, agent, kind)) in streams.iter_mut().zip([
            ("/tmp/billing-service", CLIAgent::Codex, "codex"),
            ("/tmp/acme-web", CLIAgent::Claude, "claude"),
        ]) {
            DaemonMsg::Cwd(PathBuf::from(dir)).encode(stream).unwrap();
            DaemonMsg::Agent(Some(agent)).encode(stream).unwrap();
            DaemonMsg::AgentStatus(Some(AgentSessionState {
                status: AgentStatus::Working,
                since_ms: Some(crate::core::config::unix_now().saturating_sub(60) * 1000),
                detection: Some(AgentDetectionSession {
                    kind: kind.into(),
                    epoch: 1,
                    exited: false,
                    source: None,
                    rule: None,
                }),
                ..Default::default()
            }))
            .encode(stream)
            .unwrap();
        }
        let mut ready = false;
        for _ in 0..200 {
            ready = app.read_with(&vcx, |app, cx| app.agent_pane_summaries(cx).len() == 2);
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready, "both live agent panes reached the collector");

        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Idle,
            since_ms: Some(crate::core::config::unix_now().saturating_sub(180) * 1000),
            detection: Some(AgentDetectionSession {
                kind: "codex".into(),
                epoch: 2,
                exited: true,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(&mut streams[0])
        .unwrap();
        DaemonMsg::Agent(None).encode(&mut streams[0]).unwrap();
        let mut exited = false;
        for _ in 0..200 {
            exited = app.read_with(&vcx, |app, cx| {
                app.agent_pane_summaries(cx)
                    .iter()
                    .any(|pane| pane.location.key.pane == 1 && pane.state == State::Exited)
            });
            if exited {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(exited, "daemon exit transition reached the collector");

        let _ = take_rendered_sidebar_agent_rows();
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let rendered = take_rendered_sidebar_agent_rows();
        let exited_row = rendered
            .iter()
            .rfind(|row| row.key.pane == 1)
            .expect("exited sidebar agent row rendered");
        assert_eq!(exited_row.title, CLIAgent::Codex.display_name());
        assert!(
            vcx.debug_bounds("row-status-circle-0").is_some(),
            "live pane paints its status dot as positive control"
        );

        let live_row = rendered
            .iter()
            .rfind(|row| row.key.pane == 2)
            .expect("live sidebar agent row rendered");
        assert_eq!(live_row.title, CLIAgent::Claude.display_name());

        // Once the remaining pane also exits, no status dot is painted in the agent rail.
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status: AgentStatus::Idle,
            since_ms: None,
            detection: Some(AgentDetectionSession {
                kind: "claude".into(),
                epoch: 2,
                exited: true,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(&mut streams[1])
        .unwrap();
        DaemonMsg::Agent(None).encode(&mut streams[1]).unwrap();
        let mut all_exited = false;
        for _ in 0..200 {
            all_exited = app.read_with(&vcx, |app, cx| {
                app.agent_pane_summaries(cx)
                    .iter()
                    .all(|pane| pane.state == State::Exited)
            });
            if all_exited {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(all_exited, "second daemon exit reached the collector");
        let _ = take_rendered_sidebar_agent_rows();
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let rendered = take_rendered_sidebar_agent_rows();
        let exited_p2 = rendered
            .iter()
            .rfind(|row| row.key.pane == 2)
            .expect("second exited sidebar agent row rendered");
        assert_eq!(exited_p2.title, CLIAgent::Claude.display_name());
        assert!(vcx.debug_bounds("row-status-circle-0").is_none());
    }
}

/// A5.1 agents F2 in the rail's Agents list.
#[cfg(test)]
mod a51_sidebar_tests {
    use super::*;
    use crate::core::cli_agent::{AgentDetectionSession, AgentSessionState, AgentStatus, CLIAgent};
    use crate::daemon::protocol::DaemonMsg;
    use crate::ui::app::test_window::harness_with_tabs;

    fn report(stream: &mut crate::daemon::transport::Stream, agent: CLIAgent, status: AgentStatus) {
        DaemonMsg::Agent(Some(agent)).encode(stream).unwrap();
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            detection: Some(AgentDetectionSession {
                kind: agent.slug().into(),
                epoch: 1,
                exited: false,
                source: None,
                rule: None,
            }),
            ..Default::default()
        }))
        .encode(stream)
        .unwrap();
    }

    /// One tab split `[Claude | shell | Codex]`: the rail's Agents list titles
    /// each pane by its own agent and numbers the Codex pane 3/3, counting
    /// the shell, with the marker as the row's separate muted span.
    #[gpui::test]
    fn a51_agents_list_titles_each_split_pane_by_its_agent(cx: &mut gpui::TestAppContext) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::pane::{Pane, PaneSlot};
        crate::ui::i18n::set_locale("en");
        let (app, mut vcx, mut streams) = harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            let (shell, shell_stream) = quiet_test_pane(10, window, cx);
            let (sibling, sibling_stream) = quiet_test_pane(9, window, cx);
            streams.push(shell_stream);
            streams.push(sibling_stream);
            let first = app.tabs[0].pane.first_leaf().unwrap();
            app.tabs[0].pane = Pane::split_node(
                gpui::Axis::Horizontal,
                0.5,
                Pane::leaf(first),
                Pane::split_node(
                    gpui::Axis::Vertical,
                    0.5,
                    Pane::leaf(PaneSlot::Ready(shell)),
                    Pane::leaf(PaneSlot::Ready(sibling)),
                ),
            );
            app.select_sidebar_list(SidebarList::Agents, window, cx);
        });
        report(&mut streams[0], CLIAgent::Claude, AgentStatus::Working);
        report(&mut streams[2], CLIAgent::Codex, AgentStatus::Waiting);
        let mut ready = false;
        for _ in 0..200 {
            ready = app.read_with(&vcx, |app, cx| app.agent_pane_summaries(cx).len() == 2);
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(ready, "both agent reports reached the collector");
        let _ = take_rendered_sidebar_agent_rows();
        for _ in 0..2 {
            vcx.run_until_parked();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        let rendered = take_rendered_sidebar_agent_rows();
        let mut failures = Vec::new();
        for (pane, want) in [
            (1, CLIAgent::Claude.display_name()),
            (9, CLIAgent::Codex.display_name()),
        ] {
            match rendered.iter().rfind(|row| row.key.pane == pane) {
                Some(row) if row.title == want => {}
                Some(row) => failures.push(format!(
                    "pane {pane} rail title {:?}, want {want:?}",
                    row.title
                )),
                None => failures.push(format!("pane {pane} drew no rail row")),
            }
        }
        let markers = app.read_with(&vcx, |app, cx| {
            app.sidebar_agent_rows(&app.agent_pane_summaries(cx), cx)
                .iter()
                .map(|row| (row.summary.location.key.pane, row.marker()))
                .collect::<Vec<_>>()
        });
        for (pane, want) in [(1, "pane 1/3"), (9, "pane 3/3")] {
            let got = markers
                .iter()
                .find(|(p, _)| *p == pane)
                .and_then(|(_, marker)| marker.as_deref());
            if got != Some(want) {
                failures.push(format!("pane {pane} marker {got:?}, want {want:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} expectation(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
