use gpui::{
    AnyElement, Context, Focusable as _, FontWeight, MouseButton, SharedString, Window, div,
    prelude::*, px, rems, rgb,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use crate::core::cli_agent::AgentStatus;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t};

use super::{FocusZone, PaneSummary, ProjectFilter, State, ViewMode};

/// What an agent row paints with: `rungs`, the surface ladder its hover and
/// selected fills step through — the panel pair, not the window's, because
/// the Inbox and the board each list many rows at once — `title`, the row's
/// top rung, and `caption`, the one under it that the reason, preview and
/// meta lines share.
struct AgentRowPaints {
    rungs: crate::ui::presets::Surface,
    title: gpui::Hsla,
    caption: gpui::Hsla,
}

fn agent_row_paints(card: bool, selected: bool, cx: &gpui::App) -> AgentRowPaints {
    let sf = cx.global::<crate::ui::presets::Surfaces>();
    // A card carries the rail's tinted fill, so it steps on the rail's
    // rungs; a list row sits on the window fill, where `sidebar`'s ladder
    // is the same panel pair (the sidebar neutral *is* the window fill).
    let rungs = match card {
        true => sf.rail,
        false => sf.sidebar,
    };
    let chrome = crate::ui::theme::chrome_ink(cx);
    AgentRowPaints {
        rungs,
        // The panel title ink: authored foreground floored once per theme
        // across both panel ladders' base/hover/selected fills, so the title
        // stays the top rung on every row state of both panels.
        title: gpui::rgb(chrome.panel_title).into(),
        // Twelve pixels of regular caption lose enough edge contrast to
        // antialiasing that the muted ink, floored for the panel's resting
        // fills, falls under 4.5 on the stronger selected fill — selected
        // rows draw the caption ink cached against that fill instead.
        caption: if selected {
            gpui::rgb(chrome.selected_caption).into()
        } else {
            cx.theme().muted_foreground
        },
    }
}

/// Appends one part of a row's meta line, separated from what is already
/// there and skipped when empty.
fn push_meta_part(meta: &mut String, part: &str) {
    if part.is_empty() {
        return;
    }
    if !meta.is_empty() {
        meta.push_str(" · ");
    }
    meta.push_str(part);
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedAgentRow {
    pub key: super::PaneKey,
    pub card: bool,
    pub title: String,
    pub reason: String,
}

/// A secondary line of a row, recorded where its element is built, so a line
/// the row does not build is absent rather than guessed at.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentLine {
    Meta,
    Preview,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedAgentLine {
    pub key: super::PaneKey,
    pub card: bool,
    pub line: AgentLine,
    pub text: String,
}

#[cfg(test)]
fn record_line(key: super::PaneKey, card: bool, line: AgentLine, text: &str) {
    RENDERED_LINES.with(|lines| {
        lines.borrow_mut().push(RenderedAgentLine {
            key,
            card,
            line,
            text: text.to_string(),
        });
    });
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedReaderHeader {
    pub key: super::PaneKey,
    pub title: String,
}

/// The key and localized string handed to the footer's text child in one
/// frame; `None` when that frame's footer carried no text.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedHelp {
    pub key: String,
    pub text: String,
}

#[cfg(test)]
std::thread_local! {
    static RENDERED_ROWS: std::cell::RefCell<Vec<RenderedAgentRow>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static RENDERED_HEADERS: std::cell::RefCell<Vec<RenderedReaderHeader>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static RENDERED_HELP: std::cell::RefCell<Vec<Option<RenderedHelp>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static RENDERED_LINES: std::cell::RefCell<Vec<RenderedAgentLine>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_rendered_help() -> Vec<Option<RenderedHelp>> {
    RENDERED_HELP.with(|help| std::mem::take(&mut *help.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_rendered_rows() -> Vec<RenderedAgentRow> {
    RENDERED_ROWS.with(|rows| std::mem::take(&mut *rows.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_rendered_lines() -> Vec<RenderedAgentLine> {
    RENDERED_LINES.with(|lines| std::mem::take(&mut *lines.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_rendered_headers() -> Vec<RenderedReaderHeader> {
    RENDERED_HEADERS.with(|headers| std::mem::take(&mut *headers.borrow_mut()))
}

impl State {
    pub(super) fn label(self) -> &'static str {
        t(match self {
            Self::Waiting => L10nKey::VisionWaiting,
            Self::Working => L10nKey::VisionWorking,
            Self::Done => L10nKey::VisionDone,
            Self::Idle => L10nKey::VisionIdle,
            Self::Exited => L10nKey::VisionExited,
        })
    }

    fn status(self) -> AgentStatus {
        match self {
            Self::Waiting => AgentStatus::Waiting,
            Self::Working => AgentStatus::Working,
            Self::Done => AgentStatus::Done,
            Self::Idle | Self::Exited => AgentStatus::Idle,
        }
    }
}

/// The pill's group, so its count can follow the pill's pressed state.
const PILL_GROUP: &str = "agent-mode-pill";

impl Tty59App {
    /// The Agents pill. `count` is what the strip already counted this frame,
    /// so the number it says and the width the strip budgeted for it are one
    /// scan of the panes.
    pub(in crate::ui) fn agent_mode_button(
        &self,
        count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.agent_mode_active();
        // The count is data beside the word, only above zero, and only while the
        // pill says Agents: Workbench is the mode, not a tally.
        let shown = (!active && count > 0).then_some(count);
        let word = SharedString::from(t(if active {
            L10nKey::VisionWorkbench
        } else {
            L10nKey::VisionAgents
        }));
        let accessible = match shown {
            Some(count) => SharedString::from(format!("{word} {count}")),
            None => word.clone(),
        };
        // The word and the count are our own children, not the button's, so
        // their size, weight and figures are the ones `StripMetrics` measured
        // rather than whatever the button gives its text.
        let run = || {
            div()
                .flex_none()
                .line_height(gpui::relative(1.))
                .text_size(gpui::rems(crate::ui::tab_strip::PILL_TEXT))
                .font_weight(FontWeight::NORMAL)
                .not_italic()
                .font_features(self.strip_metrics.features())
        };
        let word = run().child(word);
        #[cfg(test)]
        let word = word.debug_selector(|| "agent-mode-label".into());
        let tally = shown.map(|count| {
            // Muted at rest and hovered; the pressed fill reduces the muted
            // ink's contrast below the text floor, so it takes the foreground
            // the word beside it already has.
            let foreground = cx.theme().foreground;
            let count = run()
                .id("agent-mode-count")
                // gpui only listens for a press on an element that has a
                // hitbox, and `group_active` alone does not give it one; the
                // pill's own cursor does, and changes nothing on screen.
                .cursor_default()
                .text_color(cx.theme().muted_foreground)
                .group_active(PILL_GROUP, move |style| style.text_color(foreground))
                .child(count.to_string());
            #[cfg(test)]
            let count = count.debug_selector(|| "agent-mode-count".into()).child(
                gpui::canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        crate::ui::tab_strip::record_pill_count_ink(window.text_style().color)
                    },
                )
                .absolute()
                .size_0(),
            );
            count
        });
        let text = h_flex()
            .flex_none()
            .items_center()
            .gap(px(crate::ui::tab_strip::PILL_COUNT_GAP))
            .child(word)
            .children(tally);
        let pill = Button::new("agent-mode-toggle")
            .group(PILL_GROUP)
            .accessible_label(accessible)
            .child(text)
            .small()
            .custom(
                gpui_component::button::ButtonCustomVariant::new(cx)
                    .color(crate::ui::tab_strip::titlebar_search_fill(cx))
                    .foreground(cx.theme().foreground)
                    .hover(rgb(cx.global::<crate::ui::presets::Surfaces>().rail.hover).into())
                    .active(rgb(cx.global::<crate::ui::presets::Surfaces>().rail.pressed).into()),
            )
            .h(px(crate::ui::tab_strip::TITLEBAR_SEARCH_H))
            .px(px(crate::ui::tab_strip::PILL_PAD_X))
            .w(px(self.strip_metrics.pill_w(active, count)))
            .rounded(px(crate::ui::tab_strip::TITLEBAR_SEARCH_RADIUS))
            .bg(crate::ui::tab_strip::titlebar_search_fill(cx))
            .tooltip_element(crate::ui::tab_strip::chord_tooltip(
                t(L10nKey::VisionToggle),
                "ToggleAgentVision",
                cx,
            ))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|app, _, window, cx| app.toggle_agent_vision(window, cx)));
        #[cfg(test)]
        let pill = pill.debug_selector(|| "agent-mode-pill".into());
        pill.into_any_element()
    }

    pub(in crate::ui) fn agent_view_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.agent_vision.state.mode();
        h_flex()
            .items_center()
            .gap_1()
            .children(
                [
                    (ViewMode::Inbox, L10nKey::VisionInbox),
                    (ViewMode::Kanban, L10nKey::VisionKanban),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (view, label))| {
                    let sf = cx.global::<crate::ui::presets::Surfaces>().rail;
                    Button::new(("agent-view", index))
                        .label(t(label))
                        .small()
                        .ghost()
                        .when(mode == view, |button| button.bg(rgb(sf.selected)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.set_agent_mode(view, window, cx)
                        }))
                }),
            )
            .into_any_element()
    }

    /// Shared by Inbox and Kanban; only the content on its right changes.
    pub(in crate::ui) fn agent_projects_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = self.sidebar_width.get();
        let summary = self.agent_vision.state.workspace_summary(
            &self.agent_vision.projects,
            &self.agent_vision.panes,
            &self.agent_vision.reads,
        );
        let mut projects = vec![(
            ProjectFilter::All,
            t(L10nKey::VisionAll).to_string(),
            summary.all.total(),
        )];
        projects.extend(summary.projects.into_iter().map(|project| {
            (
                ProjectFilter::Group(project.project.group.clone()),
                project.project.name.clone(),
                project.needs_attention.total(),
            )
        }));
        let sf = cx.global::<crate::ui::presets::Surfaces>().rail;
        let head = h_flex()
            .h(px(crate::ui::app::TITLE_BAR_HEIGHT))
            .items_center()
            .px_3()
            .when_some(crate::ui::app::window_mark(), |row, mark| row.child(mark))
            .child(div().flex_1())
            .child(self.new_tab_button("agent-sidebar-add", cx));
        let search = h_flex()
            .h(px(28.))
            .items_center()
            .gap_2()
            .pl_2()
            .pr_1()
            .rounded(px(7.))
            .bg(cx.theme().muted)
            .child(
                Icon::new(IconName::Search)
                    .size(px(12.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.agent_vision.input)
                        .appearance(false)
                        .cleanable(true)
                        .pl_0(),
                ),
            );
        #[cfg(test)]
        let search = search.debug_selector(|| "agent-search-well".into());
        // The keyboard ring of the existing input around the whole well: the shared ring, inside the well's own 7px corners, on the
        // input's own focus handle (no new handle, no render-time repair).
        let search = crate::ui::theme::FocusRing::new(
            &self.agent_vision.input.focus_handle(cx),
            crate::ui::theme::RingPlacement::Inside,
            px(7.),
            search,
        );
        #[cfg(test)]
        let search = search.tagged("agent-search-ring");
        v_flex()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .bg(crate::ui::theme::rail_surface_color(cx))
            .border_r(crate::ui::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .child(crate::ui::app::title_bar_drag(
                head.id("agent-sidebar-drag"),
                "agent-sidebar-drag",
                window,
                cx,
            ))
            .child(
                v_flex()
                    .px_3()
                    .pt_1()
                    .pb_4()
                    .gap(px(10.))
                    .child(self.workspace_head(cx))
                    .child(search),
            )
            .child(
                div()
                    // The project names below sit 0.75rem (list) plus 0.5rem
                    // (row) in, so the heading starts there too.
                    .px(gpui::rems(1.25))
                    .pb_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child({
                        let label = t(L10nKey::VisionProjects);
                        // Test builds box the text so its left edge is
                        // measurable apart from the padding that insets it;
                        // release builds keep the bare text.
                        #[cfg(test)]
                        let label = div()
                            .child(label)
                            .debug_selector(|| "agent-projects-label".into());
                        label
                    }),
            )
            .child(
                v_flex()
                    .id("agent-projects-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .gap_1()
                    .children(projects.into_iter().enumerate().map(
                        |(index, (filter, name, count))| {
                            let selected = self.agent_vision.state.project_filter() == &filter;
                            h_flex()
                                .id(("agent-project", index))
                                .role(gpui::Role::Tab)
                                .aria_selected(selected)
                                .aria_label(SharedString::from(name.clone()))
                                .w_full()
                                .min_w_0()
                                .h(px(30.))
                                .px_2()
                                .gap_2()
                                .items_center()
                                .cursor_pointer()
                                .rounded(crate::ui::rounding::ROW_RADIUS)
                                .when(selected, |row| {
                                    row.bg(rgb(sf.selected)).text_color(rgb(sf.text_selected))
                                })
                                .when(!selected, |row| {
                                    row.text_color(rgb(sf.text_resting))
                                        .hover(|style| style.bg(rgb(sf.hover)))
                                })
                                .child({
                                    let name = div().flex_1().min_w_0().truncate().child(name);
                                    #[cfg(test)]
                                    let name = name.debug_selector(move || {
                                        format!("agent-project-name-{index}")
                                    });
                                    name
                                })
                                .when(count > 0, |row| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(count.to_string()),
                                    )
                                })
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.agent_vision.state.set_project_filter(filter.clone());
                                    app.focus_agent_list(window, cx);
                                }))
                        },
                    )),
            )
            .into_any_element()
    }

    /// The Inbox's "Other agents" section control: the library button, with
    /// its stable id, focus handle and callback, drawn as a left-aligned 28px
    /// section heading in the list's muted caption ink: the label, its count
    /// as separate tabular figures, and a chevron that says collapsed or open.
    fn other_agents_toggle(&self, count: usize, cx: &mut Context<Self>) -> Button {
        let open = self.agent_vision.state.show_other;
        // The list sits on the window fill, so it steps on the sidebar
        // ladder like its rows; the muted ink is floored against both rungs.
        let rungs = cx.global::<crate::ui::presets::Surfaces>().sidebar;
        let muted = cx.theme().muted_foreground;
        let caption = |text_size: f32| gpui::rems(text_size / 16.);
        let label = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_size(caption(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted)
            .child(t(L10nKey::VisionOther));
        let tally = div()
            .flex_none()
            .text_size(caption(11.5))
            .font_features(self.strip_metrics.features())
            .text_color(muted)
            .child(count.to_string());
        let chevron = div().flex_none().text_color(muted).child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall(),
        );
        #[cfg(test)]
        let (label, tally, chevron) = (
            label.debug_selector(|| "agent-other-toggle-label".into()),
            tally.debug_selector(|| "agent-other-toggle-count".into()),
            chevron.debug_selector(move || {
                if open {
                    "agent-other-chevron-expanded".into()
                } else {
                    "agent-other-chevron-collapsed".into()
                }
            }),
        );
        let toggle = Button::new("agent-other-toggle")
            .accessible_label(SharedString::from(format!(
                "{}, {count}",
                t(L10nKey::VisionOther)
            )))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(label)
                    .child(tally)
                    .child(chevron),
            )
            .small()
            .custom(
                gpui_component::button::ButtonCustomVariant::new(cx)
                    .color(gpui::transparent_black())
                    .foreground(muted)
                    .hover(rgb(rungs.hover).into())
                    .active(rgb(rungs.selected).into()),
            )
            .w_full()
            .h(px(28.))
            .px_2()
            .on_click(cx.listener(|app, _, window, cx| {
                app.agent_vision.state.show_other = !app.agent_vision.state.show_other;
                app.focus_agent_list(window, cx);
            }));
        #[cfg(test)]
        let toggle = toggle.debug_selector(|| "agent-other-toggle".into());
        toggle
    }

    /// One Inbox row (`card` false) or board card. `in_other` is true for a
    /// row of the Inbox's Other group: like a card in its column, such a row
    /// sits under a heading that already supplies its state.
    fn agent_row(
        &self,
        pane: &PaneSummary,
        card: bool,
        in_other: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = pane.location.key;
        let selected = self.agent_vision.state.selected() == Some(key);
        let paints = agent_row_paints(card, selected, cx);
        let project = self
            .agent_vision
            .projects
            .iter()
            .find(|project| project.group == pane.location.group)
            .map_or("", |project| project.name.as_str());
        let exited = pane.state == State::Exited;
        // The accessible name keeps the whole heading, marker included; the
        // drawn title carries the name alone and the marker its own span.
        let heading = pane.heading();
        let title = pane.title().to_string();
        let marker = pane.pane_marker();
        let now = crate::core::config::unix_now();
        let reason = pane.reason_line(now);
        let reason_hidden = (card || in_other) && pane.reason_is_bare_state();
        let mut meta = String::new();
        if !pane.title_supplies_agent() {
            push_meta_part(&mut meta, pane.agent_name());
        }
        push_meta_part(&mut meta, project);
        if let Some(since) = pane.since_ms.filter(|_| !exited) {
            let age = now.saturating_sub(since / 1000);
            push_meta_part(&mut meta, &crate::ui::home::compact_age(age));
        }
        #[cfg(test)]
        RENDERED_ROWS.with(|rows| {
            rows.borrow_mut().push(RenderedAgentRow {
                key,
                card,
                title: title.clone(),
                reason: if reason_hidden {
                    String::new()
                } else {
                    reason.clone()
                },
            });
        });
        // The rail's mark, so one agent reads the same in both places: Idle
        // and Exited have no status role and so no dot (the reason line says
        // the state), and the hollow ring stays Waiting's alone.
        let status = self.row_status_dot(
            Some(pane.state.status()),
            0,
            rgb(paints.rungs.base).into(),
            cx,
        );
        let row = h_flex()
            .id(SharedString::from(format!(
                "agent-{}-{}",
                key.host.0, key.pane
            )))
            .role(gpui::Role::ListItem)
            .aria_label(SharedString::from(format!("{heading}, {reason}")))
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .p_2()
            .rounded(crate::ui::rounding::ROW_RADIUS)
            .cursor_pointer()
            .when(card, |row| {
                row.bg(crate::ui::theme::rail_surface_color(cx))
                    .border_1()
                    .border_color(cx.theme().border)
            })
            .when(selected, |row| {
                row.bg(rgb(paints.rungs.selected))
                    .text_color(rgb(paints.rungs.text_selected))
            })
            .when(!selected, |row| {
                row.text_color(rgb(paints.rungs.text_resting))
                    .hover(|style| {
                        let style = style.bg(rgb(paints.rungs.hover));
                        #[cfg(test)]
                        let style = {
                            let mut style = style;
                            crate::ui::paint_probe::note("agent-row-hover", &mut style);
                            style
                        };
                        style
                    })
            })
            .anchor_scroll(selected.then(|| {
                if card {
                    self.agent_vision.board_anchor[pane.state.kanban_index()].clone()
                } else {
                    self.agent_vision.inbox_anchor.clone()
                }
            }))
            .child({
                let icon = div()
                    .flex_shrink_0()
                    .self_start()
                    .child(self.tab_avatar_plain(
                        SharedString::from(format!("agent-icon-{}-{}", key.host.0, key.pane)),
                        pane.agent,
                        Some(pane.state.status()),
                        None,
                        22.,
                        cx,
                    ));
                #[cfg(test)]
                let icon = icon
                    .debug_selector(move || format!("agent-row-icon-{}-{}", key.host.0, key.pane));
                icon
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    // The title and, beside it, the split's pane marker as its
                    // own muted span (the rail's recipe). Each span is bounded
                    // by the row on its own and is sized by its text first
                    // (the title's `flex_auto` is grow 1, shrink 1, basis
                    // auto): when the two cannot share a line the marker wraps
                    // under the name instead of taking the name's width, a
                    // name longer than the row is cut at it, and the title
                    // fills whatever line it ends up on.
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap_x_1p5()
                            .child({
                                let title_el = div()
                                    .flex_auto()
                                    .min_w_0()
                                    .max_w_full()
                                    .truncate()
                                    .text_sm()
                                    .text_color(paints.title)
                                    .when(selected, |row| row.font_weight(FontWeight::MEDIUM))
                                    .when(exited, |title| title.text_color(paints.caption))
                                    .child(title);
                                #[cfg(test)]
                                let title_el = {
                                    let mut title_el = title_el;
                                    crate::ui::paint_probe::note("agent-row-title", &mut title_el);
                                    title_el.debug_selector(move || {
                                        format!("agent-row-title-{}-{}", key.host.0, key.pane)
                                    })
                                };
                                title_el
                            })
                            .when_some(marker, |row, marker| {
                                let marker_el = div()
                                    .min_w_0()
                                    .max_w_full()
                                    .truncate()
                                    .text_xs()
                                    .text_color(paints.caption)
                                    .child(marker);
                                #[cfg(test)]
                                let marker_el = marker_el.debug_selector(move || {
                                    format!("agent-row-marker-{}-{}", key.host.0, key.pane)
                                });
                                row.child(marker_el)
                            }),
                    )
                    // A bare Working, Waiting or Done is not repeated where
                    // the column or the Other group already says it.
                    .when(!reason_hidden, |column| {
                        column.child({
                            let reason_el = div()
                                .truncate()
                                .text_xs()
                                .text_color(paints.caption)
                                .child(reason);
                            #[cfg(test)]
                            let reason_el = {
                                let mut reason_el = reason_el;
                                crate::ui::paint_probe::note("agent-row-reason", &mut reason_el);
                                crate::ui::paint_probe::note(
                                    if selected {
                                        "agent-row-reason-sel"
                                    } else {
                                        "agent-row-reason-plain"
                                    },
                                    &mut reason_el,
                                );
                                reason_el.debug_selector(move || {
                                    format!("agent-row-reason-{}-{}", key.host.0, key.pane)
                                })
                            };
                            reason_el
                        })
                    })
                    .when(card && !exited, |column| {
                        // A preview that is the reason again (up to layout
                        // whitespace) says nothing new; a continuation or any
                        // other output is kept whole.
                        column.when_some(
                            pane.last_line
                                .as_deref()
                                .filter(|line| !pane.preview_repeats_reason(line)),
                            |column, line| {
                                #[cfg(test)]
                                record_line(key, card, AgentLine::Preview, line);
                                column.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(paints.caption)
                                        .map(|preview| {
                                            #[cfg(test)]
                                            let preview = preview.debug_selector(move || {
                                                format!(
                                                    "agent-row-preview-{}-{}",
                                                    key.host.0, key.pane
                                                )
                                            });
                                            preview
                                        })
                                        .child(SharedString::from(line.to_string())),
                                )
                            },
                        )
                    })
                    .when(!meta.is_empty(), |column| {
                        column.child({
                            #[cfg(test)]
                            record_line(key, card, AgentLine::Meta, &meta);
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(paints.caption)
                                .child(meta)
                        })
                    }),
            )
            // Kanban uses its column for state. Only an unseen Done card has
            // an additional status dot; Inbox always has the trailing circle.
            .when(
                !card
                    || (pane.state == State::Done
                        && self.agent_vision.reads.attention(pane).is_some()),
                |row| row.children(status),
            )
            .on_click(
                cx.listener(move |app, _, window, cx| app.open_agent_terminal(key, window, cx)),
            );
        #[cfg(test)]
        let row = {
            let mut row =
                row.debug_selector(move || format!("agent-row-frame-{}-{}", key.host.0, key.pane));
            crate::ui::paint_probe::note("agent-row", &mut row);
            row
        };
        row.into_any_element()
    }

    pub(in crate::ui) fn agent_vision_body(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panes = self.agent_vision.filtered(cx);
        // Whether the reader terminal really holds the window's focus, read
        // from the one lookup the Inbox reader already makes this frame.
        let mut terminal_has_focus = false;
        let body = if self.agent_vision.state.mode() == ViewMode::Kanban {
            let board = self.agent_vision.state.kanban_columns(&panes);
            let selected_column = panes
                .iter()
                .find(|pane| Some(pane.location.key) == self.agent_vision.state.selected())
                .map(|pane| pane.state.kanban_index());
            h_flex()
                .id("agent-board")
                .track_scroll(&self.agent_vision.board_horizontal)
                .size_full()
                .min_w_0()
                .items_stretch()
                .overflow_x_scroll()
                .gap_3()
                .p_3()
                .children(
                    State::KANBAN_ORDER
                        .into_iter()
                        .zip(board.0)
                        .enumerate()
                        .map(|(index, (state, cards))| {
                            v_flex()
                                .id(("agent-column", index))
                                .anchor_scroll(
                                    (selected_column == Some(index))
                                        .then(|| self.agent_vision.board_lane_anchor.clone()),
                                )
                                .w(px(250.))
                                .min_w(px(210.))
                                .flex_1()
                                .h_full()
                                .min_h_0()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        // The title starts where a card's
                                        // content does: the card's own box
                                        // model (1px border, then `p_2`) with
                                        // the border left invisible.
                                        .border_l_1()
                                        .border_r_1()
                                        .border_color(gpui::transparent_black())
                                        .px_2()
                                        .py_1()
                                        .child({
                                            let heading = div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(state.label());
                                            #[cfg(test)]
                                            let heading = heading.debug_selector(move || {
                                                format!("agent-board-heading-{index}")
                                            });
                                            heading
                                        })
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(cards.len().to_string()),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .id(("agent-lane", index))
                                        .track_scroll(&self.agent_vision.board_scroll[index])
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .gap_2()
                                        .children(
                                            cards
                                                .into_iter()
                                                .map(|pane| self.agent_row(pane, true, false, cx)),
                                        ),
                                )
                        }),
                )
                .into_any_element()
        } else {
            let rows = self
                .agent_vision
                .state
                .inbox_rows(&panes, &self.agent_vision.reads);
            // Whether the list shows nothing at all, project filter included.
            // A search that matched nothing says so, and an empty list has no
            // "select an agent" instruction to give. (The selection itself is
            // already cleared by the focus reconcile that runs every frame.)
            let list_empty = rows.attention.is_empty() && rows.other.is_empty();
            let searching = !self.agent_vision.input.read(cx).value().trim().is_empty();
            let selected = self.agent_vision.state.selected().and_then(|key| {
                self.agent_vision
                    .panes
                    .iter()
                    .find(|pane| pane.location.key == key)
            });
            let empty_text = if list_empty && searching {
                L10nKey::VisionNoAgents
            } else {
                L10nKey::VisionEmpty
            };
            let other_count = rows.other.len();
            let reader_terminal =
                selected.and_then(|pane| self.agent_terminal(pane.location.key, cx));
            terminal_has_focus = reader_terminal
                .as_ref()
                .is_some_and(|terminal| terminal.focus_handle(cx).contains_focused(window, cx));
            let list = v_flex()
                .w(px(320.))
                .min_w(px(240.))
                .max_w(gpui::relative(0.42))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        // One edge with "Needs you" and the avatars below:
                        // the scroll's 0.5rem plus the row's 0.5rem.
                        .px_4()
                        .py_3()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child({
                            let heading = t(L10nKey::VisionInbox);
                            #[cfg(test)]
                            let heading = div()
                                .child(heading)
                                .debug_selector(|| "agent-inbox-heading".into());
                            heading
                        }),
                )
                .child(
                    v_flex()
                        .id("agent-inbox-scroll")
                        .track_scroll(&self.agent_vision.inbox_scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_2()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .py_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child({
                                    let label = t(L10nKey::VisionNeedsYou);
                                    #[cfg(test)]
                                    let label = div()
                                        .child(label)
                                        .debug_selector(|| "agent-needs-you".into());
                                    label
                                }),
                        )
                        .when(rows.attention.is_empty(), |list| {
                            let empty = div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(empty_text));
                            #[cfg(test)]
                            let empty = empty.debug_selector(move || {
                                format!("agent-inbox-empty-{empty_text:?}")
                            });
                            list.child(empty)
                        })
                        .children(
                            rows.attention
                                .into_iter()
                                .map(|pane| self.agent_row(pane, false, false, cx)),
                        )
                        .when(other_count > 0, |list| {
                            list.child(self.other_agents_toggle(other_count, cx))
                        })
                        .when(self.agent_vision.state.show_other, |list| {
                            list.children(
                                rows.other
                                    .into_iter()
                                    .map(|pane| self.agent_row(pane, false, true, cx)),
                            )
                        }),
                );
            #[cfg(test)]
            let list = list.debug_selector(|| "agent-inbox-list".into());
            let reader = v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .when_some(selected, |reader, pane| {
                    let key = pane.location.key;
                    let heading = pane.heading();
                    #[cfg(test)]
                    RENDERED_HEADERS.with(|headers| {
                        headers.borrow_mut().push(RenderedReaderHeader {
                            key,
                            title: heading.clone(),
                        });
                    });
                    reader
                        .child(
                            v_flex()
                                .px_3()
                                .py_2()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .when(pane.state == State::Exited, |title| {
                                                    title.text_color(cx.theme().muted_foreground)
                                                })
                                                .child(heading),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(pane.state.label()),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .flex_wrap()
                                        .gap_1()
                                        .child(
                                            Button::new("agent-handled")
                                                .label(t(L10nKey::VisionHandled))
                                                .small()
                                                .ghost()
                                                .on_click(cx.listener(|app, _, window, cx| {
                                                    app.handle_agent_attention(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-next")
                                                .label(t(L10nKey::VisionNext))
                                                .small()
                                                .ghost()
                                                .on_click(cx.listener(|app, _, window, cx| {
                                                    app.next_agent_attention(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("agent-workbench")
                                                .label(t(L10nKey::VisionOpenWorkbench))
                                                .small()
                                                .ghost()
                                                .on_click(cx.listener(
                                                    move |app, _, window, cx| {
                                                        app.reveal_agent_in_workbench(
                                                            key, window, cx,
                                                        )
                                                    },
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .when_some(reader_terminal, |reader, terminal| {
                                    reader.child(terminal)
                                }),
                        )
                })
                .when(selected.is_none() && !list_empty, |reader| {
                    let hint = div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::VisionSelect));
                    #[cfg(test)]
                    let hint = hint.debug_selector(|| "agent-reader-hint".into());
                    reader.child(hint)
                });
            h_flex()
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(list)
                .child(reader)
                .into_any_element()
        };
        // The zone is the window's real focus, never the `state.focus` mirror,
        // which a click or a plain `window.focus` leaves stale.
        let help = if terminal_has_focus {
            Some(L10nKey::VisionTerminalHelp)
        } else if self.agent_vision.list_focus.is_focused(window) {
            Some(if self.agent_vision.state.mode() == ViewMode::Kanban {
                L10nKey::VisionBoardHelp
            } else {
                L10nKey::VisionListHelp
            })
        } else {
            None
        };
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .track_focus(&self.agent_vision.list_focus)
            .when(self.agent_vision.state.focus() == FocusZone::List, |view| {
                view.key_context("AgentVisionList")
            })
            .child(div().flex_1().min_h_0().min_w_0().child(body))
            .child(agent_help_footer(help, cx))
            .into_any_element()
    }
}

/// The status-bar height of the design system: the footer never gets shorter,
/// and grows with the font because its text and padding are not fixed pixels.
const HELP_FOOTER_MIN_H: f32 = 26.;

/// The keyboard help strip under the list/board and the reader. The strip is
/// always built so the layout never jumps; `help` is `None` while focus is
/// outside the list and the reader (search, rename, a dialog), where no help
/// text would be true. 12/16rem muted text on the workspace fill.
fn agent_help_footer(help: Option<L10nKey>, cx: &gpui::App) -> AnyElement {
    // The one localized string this frame: the value the child is built from
    // is the value the test recorder captures.
    let label = help.map(|key| (key, t(key)));
    #[cfg(test)]
    RENDERED_HELP.with(|rendered| {
        rendered
            .borrow_mut()
            .push(label.map(|(key, text)| RenderedHelp {
                key: format!("{key:?}"),
                text: text.to_string(),
            }));
    });
    let strip = h_flex()
        .flex_none()
        .w_full()
        .min_h(px(HELP_FOOTER_MIN_H))
        .px_3()
        .py_1()
        .items_center()
        .overflow_hidden();
    #[cfg(test)]
    let strip = strip.debug_selector(|| "agent-help-footer".into());
    strip
        .children(label.map(|(key, text)| {
            let line = div()
                .min_w_0()
                .truncate()
                .text_size(rems(0.75))
                .line_height(rems(1.))
                .text_color(cx.theme().muted_foreground)
                .child(text);
            // The key names the selector only under test.
            #[cfg(not(test))]
            let _ = key;
            #[cfg(test)]
            let line = line.debug_selector(move || format!("agent-help-{key:?}"));
            line
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cli_agent::{AgentDetectionSession, AgentSessionState};
    use crate::core::config::Config;
    use crate::daemon::protocol::DaemonMsg;
    use crate::ui::paint_probe::{self, hsla24};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    /// Two live agents — one Waiting, one Working — rendered through the
    /// real `Tty59App` window, the same fixture `ui_tests` drives. The
    /// daemon streams come back with the window: dropping them ends the
    /// sessions and the agents vanish mid-test.
    fn agents_window(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, mut streams) = crate::ui::app::test_window::harness_with_tabs(cx, 3);
        for (index, stream) in streams.iter_mut().take(2).enumerate() {
            DaemonMsg::AgentStatus(Some(AgentSessionState {
                status: match index {
                    0 => crate::core::cli_agent::AgentStatus::Waiting,
                    _ => crate::core::cli_agent::AgentStatus::Working,
                },
                since_ms: Some(index as u64 + 1),
                message: Some(format!("Approve task {index}")),
                detection: Some(AgentDetectionSession {
                    kind: "codex".into(),
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
        for _ in 0..200 {
            if app.update_in(&mut vcx, |app, window, cx| {
                app.refresh_agent_vision(window, cx);
                app.agent_vision.panes.len()
            }) == 2
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.update_in(&mut vcx, |app, window, cx| {
            assert_eq!(app.agent_vision.panes.len(), 2);
            app.set_agent_mode(ViewMode::Inbox, window, cx);
            // Working agents raise no attention event, so they land in the
            // collapsed Other group — the row toggle flips the same field.
            app.agent_vision.state.show_other = true;
            let key = app.agent_vision.panes[0].location.key;
            app.agent_vision.state.focus_list_item(key);
        });
        vcx.run_until_parked();
        (app, vcx, streams)
    }

    /// The rows the Inbox and the board paint — read off the rendered
    /// elements, not a recomputed expectation — carry the panel ladder's
    /// fills and its fg-seeded title ink, with the muted caption a step
    /// under it. Asserted against the theme's own tokens per theme and
    /// mode, so a reverted `.text_color` or a wrong ladder fails by
    /// assertion.
    #[gpui::test]
    fn agent_rows_paint_theme_inks_and_panel_rungs(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = agents_window(cx);
        let _probe = paint_probe::scope();
        // The selected-caption assert only bites where the lifted ink differs
        // from muted; the fixture must reach at least one such theme.
        let mut distinguished = false;
        for id in ["light", "dark"] {
            vcx.update(|_, cx| {
                cx.set_global(Config(crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: id.into(),
                    ..Default::default()
                }));
                crate::ui::theme::apply_theme(None, cx);
            });
            let (sf, muted, muted_alpha, semantics, panel_title, sel_caption) =
                vcx.update(|_, cx| {
                    (
                        cx.global::<crate::ui::presets::Surfaces>().clone(),
                        hsla24(cx.theme().muted_foreground),
                        cx.theme().muted_foreground.a,
                        crate::ui::theme::semantics(cx),
                        crate::ui::theme::chrome_ink(cx).panel_title,
                        crate::ui::theme::chrome_ink(cx).selected_caption,
                    )
                });
            distinguished |= sel_caption != muted;
            for (mode, rungs, cards) in [
                (ViewMode::Inbox, sf.sidebar, false),
                (ViewMode::Kanban, sf.rail, true),
            ] {
                app.update_in(&mut vcx, |app, window, cx| {
                    app.set_agent_mode(mode, window, cx);
                    let key = app.agent_vision.panes[0].location.key;
                    app.agent_vision.state.focus_list_item(key);
                });
                vcx.run_until_parked();
                paint_probe::clear();
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });

                let kind = if cards { "card" } else { "row" };
                let titles = paint_probe::all("agent-row-title");
                let reasons = paint_probe::all("agent-row-reason");
                assert_eq!(titles.len(), 2, "{id}/{kind}: two agents render two titles");
                assert_eq!(
                    reasons.len(),
                    2,
                    "{id}/{kind}: two agents render two reasons"
                );
                for title in &titles {
                    assert_eq!(
                        title.text,
                        Some(panel_title),
                        "{id}/{kind}: title is not the floored panel title ink"
                    );
                    assert_eq!(
                        title.text_alpha,
                        Some(1.0),
                        "{id}/{kind}: title ink lost its alpha"
                    );
                    assert_eq!(
                        title.opacity, None,
                        "{id}/{kind}: the title div must not carry opacity"
                    );
                }
                let sel_reasons = paint_probe::all("agent-row-reason-sel");
                assert_eq!(
                    sel_reasons.len(),
                    1,
                    "{id}/{kind}: the fixture marks exactly one selected row"
                );
                assert_eq!(
                    sel_reasons[0].text,
                    Some(sel_caption),
                    "{id}/{kind}: a selected caption is not the lifted ink"
                );
                assert_eq!(
                    sel_reasons[0].text_alpha,
                    Some(1.0),
                    "{id}/{kind}: the lifted caption lost its alpha"
                );
                let plain_reasons = paint_probe::all("agent-row-reason-plain");
                assert_eq!(
                    plain_reasons.len(),
                    1,
                    "{id}/{kind}: the fixture leaves one row unselected"
                );
                for reason in &plain_reasons {
                    assert_eq!(
                        reason.text,
                        Some(muted),
                        "{id}/{kind}: reason is not the muted caption ink"
                    );
                    assert_eq!(
                        reason.text_alpha,
                        Some(muted_alpha),
                        "{id}/{kind}: reason alpha drifted from the theme's"
                    );
                }
                let rows = paint_probe::all("agent-row");
                assert_eq!(
                    rows.len(),
                    2,
                    "{id}/{kind}: the Waiting and the Working agent must both render"
                );
                let resting = match cards {
                    true => Some(rungs.base),
                    false => None,
                };
                for row in &rows {
                    assert!(
                        row.fill == resting || row.fill == Some(rungs.selected),
                        "{id}/{kind}: row fill {:?} is neither the card base nor selected",
                        row.fill
                    );
                }
                assert!(
                    rows.iter().any(|row| row.fill == Some(rungs.selected)),
                    "{id}/{kind}: no row paints the selected rung"
                );
                let hovers = paint_probe::all("agent-row-hover");
                assert!(
                    !hovers.is_empty(),
                    "{id}/{kind}: no hover refinements were observed"
                );
                for hover in &hovers {
                    assert_eq!(
                        hover.fill,
                        Some(rungs.hover),
                        "{id}/{kind}: hover reached a non-panel rung"
                    );
                }

                if cards {
                    continue;
                }
                let dots = paint_probe::all("row-status-dot");
                assert!(!dots.is_empty(), "{id}: no row status marks were observed");
                let marks = [
                    semantics.warning.ink,
                    semantics.info.ink,
                    semantics.success.ink,
                ];
                let mut inks = Vec::new();
                for dot in &dots {
                    let ink = dot.fill.or(dot.border).expect("a mark is a fill or a ring");
                    inks.push(ink);
                    assert!(
                        marks.contains(&ink),
                        "{id}: row mark {ink:#08x} is not a semantic role ink"
                    );
                    let mark_alpha = match dot.fill {
                        Some(_) => dot.fill_alpha,
                        None => dot.border_alpha,
                    };
                    assert_eq!(
                        mark_alpha,
                        Some(1.0),
                        "{id}: a status mark must stay opaque"
                    );
                }
                // The fixture's two Inbox rows are a Waiting ring and a
                // Working mark — both must be among the observed marks.
                for want in [semantics.warning.ink, semantics.info.ink] {
                    assert!(
                        inks.contains(&want),
                        "{id}: the Waiting ring and the Working mark must both render"
                    );
                }
            }
        }
        assert!(
            distinguished,
            "no fixture theme distinguishes the selected caption from muted, so its wiring cannot be proven"
        );
    }

    /// The panel ladders themselves — every builtin plus a weak custom
    /// palette forked onto disk and reloaded through the theme registry —
    /// keep the cached panel title ink at the 4.5 text contract on base,
    /// hover and selected, the muted caption at the same floor, and the
    /// panel rungs distinct. The fork's grey foreground clears nothing, so
    /// `panel_title` has to lift rather than read the palette raw; a second
    /// phase forks generated-grid palettes whose resolved foreground
    /// actually misses a panel fill, where a raw-foreground title would
    /// fail where `panel_title` still clears.
    #[gpui::test]
    fn panel_ladders_hold_ink_floors_on_builtins_and_customs(cx: &mut TestAppContext) {
        // The external contract is WCAG AA for text; the presets constant it
        // is derived from is private to presets.
        const FLOOR: f32 = 4.5;
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut weak = crate::ui::presets::by_id(cx, "light");
            weak.id = "b1weak".into();
            weak.name = "B1 Weak".into();
            weak.foreground = 0x8a8a8a;
            weak.path = None;
            crate::ui::presets::fork_to_file(&weak).expect("the weak theme writes to disk");
            crate::ui::presets::load_registry(cx);
        });
        let themes = cx.update(|cx| crate::ui::presets::all(cx));
        assert!(
            themes.iter().any(|theme| theme.id.ends_with("-custom")),
            "the forked custom theme must be in the sweep"
        );
        for theme in themes {
            cx.update(|cx| {
                cx.set_global(Config(crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: theme.id.clone(),
                    ..Default::default()
                }));
                crate::ui::theme::apply_theme(None, cx);
            });
            let (sf, muted, sel_caption, title) = cx.update(|cx| {
                (
                    cx.global::<crate::ui::presets::Surfaces>().clone(),
                    theme.neutrals().muted_foreground,
                    crate::ui::theme::chrome_ink(cx).selected_caption,
                    crate::ui::theme::chrome_ink(cx).panel_title,
                )
            });
            for (kind, rungs) in [("row", sf.sidebar), ("card", sf.rail)] {
                for (name, surface, caption) in [
                    ("base", rungs.base, muted),
                    ("hover", rungs.hover, muted),
                    ("selected", rungs.selected, sel_caption),
                ] {
                    assert!(
                        crate::ui::presets::contrast(title, surface) >= FLOOR,
                        "{}/{kind}: title {title:#08x} unreadable on {name} {surface:#08x}",
                        theme.id
                    );
                    assert!(
                        crate::ui::presets::contrast(caption, surface) >= FLOOR,
                        "{}/{kind}: caption {caption:#08x} unreadable on {name} {surface:#08x}",
                        theme.id
                    );
                }
                assert!(
                    crate::ui::presets::contrast(title, rungs.base)
                        >= crate::ui::presets::contrast(muted, rungs.base),
                    "{}/{kind}: the title must never trail its caption",
                    theme.id
                );
                assert!(
                    crate::ui::presets::contrast(rungs.selected, rungs.base)
                        > crate::ui::presets::contrast(rungs.hover, rungs.base),
                    "{}/{kind}: selected must stay the stronger mark",
                    theme.id
                );
            }
        }

        // A raw-`m.foreground` title only diverges from `panel_title` on a
        // palette whose resolved foreground — already floored on the window
        // base — still misses 4.5 on a panel state fill. Fork a couple of
        // those grid cases per polarity so the sweep's title assertion can
        // tell the lifted ink apart from the seed.
        let mut witnesses = 0usize;
        let mut per_side = [0usize; 2];
        for theme in crate::ui::presets::generated_theme_grid() {
            let side = usize::from(theme.dark);
            if per_side[side] >= 2 {
                continue;
            }
            let r = theme.resolve();
            let panel = [
                r.sf.sidebar.base,
                r.sf.sidebar.hover,
                r.sf.sidebar.selected,
                r.sf.rail.base,
                r.sf.rail.hover,
                r.sf.rail.selected,
            ];
            if panel
                .iter()
                .all(|&fill| crate::ui::presets::contrast(r.m.foreground, fill) >= FLOOR)
            {
                continue;
            }
            let mut witness = theme;
            witness.id = format!("b1grid{witnesses}");
            witness.path = None;
            crate::ui::presets::fork_to_file(&witness).expect("the witness theme writes to disk");
            per_side[side] += 1;
            witnesses += 1;
            if witnesses >= 4 {
                break;
            }
        }
        cx.update(crate::ui::presets::load_registry);
        let loaded: Vec<_> = cx
            .update(|cx| crate::ui::presets::all(cx))
            .into_iter()
            .filter(|theme| theme.id.starts_with("b1grid"))
            .collect();
        assert!(
            witnesses > 0 && loaded.len() == witnesses,
            "the grid must yield weak-panel palettes and every fork must load ({witnesses} forked, {} loaded)",
            loaded.len()
        );
        for theme in loaded {
            cx.update(|cx| {
                cx.set_global(Config(crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: theme.id.clone(),
                    ..Default::default()
                }));
                crate::ui::theme::apply_theme(None, cx);
            });
            let title = cx.update(|cx| crate::ui::theme::chrome_ink(cx).panel_title);
            let r = theme.resolve();
            for (kind, rungs) in [("row", r.sf.sidebar), ("card", r.sf.rail)] {
                for (name, surface) in [
                    ("base", rungs.base),
                    ("hover", rungs.hover),
                    ("selected", rungs.selected),
                ] {
                    assert!(
                        crate::ui::presets::contrast(title, surface) >= FLOOR,
                        "{}/{kind}: weak-panel title {title:#08x} unreadable on {name} {surface:#08x}",
                        theme.id
                    );
                }
            }
        }
    }
}
