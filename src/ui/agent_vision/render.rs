use gpui::{
    AnyElement, Context, FontWeight, MouseButton, SharedString, Window, div, prelude::*, px, rgb,
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

impl Tty59App {
    pub(in crate::ui) fn agent_mode_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.agent_mode_active();
        let count = self
            .agent_vision
            .state
            .attention_count(&self.agent_vision.panes, &self.agent_vision.reads)
            .total();
        let label = if active {
            t(L10nKey::VisionWorkbench).to_string()
        } else {
            format!("{} ({count})", t(L10nKey::VisionAgents))
        };
        Button::new("agent-mode-toggle")
            .label(label)
            .small()
            .custom(
                gpui_component::button::ButtonCustomVariant::new(cx)
                    .color(crate::ui::tab_strip::titlebar_search_fill(cx))
                    .foreground(cx.theme().foreground)
                    .hover(rgb(cx.global::<crate::ui::presets::Surfaces>().rail.hover).into())
                    .active(rgb(cx.global::<crate::ui::presets::Surfaces>().rail.pressed).into()),
            )
            .h(px(crate::ui::tab_strip::TITLEBAR_SEARCH_H))
            .px(px(10.))
            .rounded(px(crate::ui::tab_strip::TITLEBAR_SEARCH_RADIUS))
            .bg(crate::ui::tab_strip::titlebar_search_fill(cx))
            .text_size(gpui::rems(0.8125))
            .tooltip_element(crate::ui::tab_strip::chord_tooltip(
                t(L10nKey::VisionToggle),
                "ToggleAgentVision",
                cx,
            ))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|app, _, window, cx| app.toggle_agent_vision(window, cx)))
            .into_any_element()
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
                    .px_4()
                    .pb_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t(L10nKey::VisionProjects)),
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
                                .child(div().flex_1().min_w_0().truncate().child(name))
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

    fn agent_row(&self, pane: &PaneSummary, card: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = pane.location.key;
        let selected = self.agent_vision.state.selected() == Some(key);
        let paints = agent_row_paints(card, selected, cx);
        let project = self
            .agent_vision
            .projects
            .iter()
            .find(|project| project.group == pane.location.group)
            .map_or("", |project| project.name.as_str());
        let title = match pane.pane_marker() {
            Some(marker) => format!("{} · {marker}", pane.location.tab_title),
            None => pane.location.tab_title.clone(),
        };
        let reason = pane.reason.as_deref().unwrap_or(pane.state.label());
        let mut meta = format!("{} · {}", pane.agent_name(), project);
        if let Some(since) = pane.since_ms {
            let age = crate::core::config::unix_now().saturating_sub(since / 1000);
            meta.push_str(&format!(" · {}", crate::ui::home::compact_age(age)));
        }
        let status = if pane.state == State::Exited || pane.state == State::Idle {
            div()
                .size(px(6.))
                .rounded_full()
                .border_1()
                .border_color(cx.theme().muted_foreground)
                .when(pane.state == State::Idle, |dot| {
                    dot.bg(cx.theme().muted_foreground)
                })
                .into_any_element()
        } else {
            self.row_status_dot(
                Some(pane.state.status()),
                0,
                rgb(paints.rungs.base).into(),
                cx,
            )
            .unwrap_or_else(|| div().into_any_element())
        };
        let row = h_flex()
            .id(SharedString::from(format!(
                "agent-{}-{}",
                key.host.0, key.pane
            )))
            .role(gpui::Role::ListItem)
            .aria_label(SharedString::from(format!("{title}, {reason}")))
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
                    .child({
                        let title_el = div()
                            .truncate()
                            .text_sm()
                            .text_color(paints.title)
                            .when(selected, |row| row.font_weight(FontWeight::MEDIUM))
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
                    .child({
                        let reason_el = div()
                            .truncate()
                            .text_xs()
                            .text_color(paints.caption)
                            .child(reason.to_string());
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
                    .when(card, |column| {
                        column.when_some(pane.last_line.clone(), |column, line| {
                            column.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(paints.caption)
                                    .child(line),
                            )
                        })
                    })
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(paints.caption)
                            .child(meta),
                    ),
            )
            // Kanban uses its column for state. Only an unseen Done card has
            // an additional status dot; Inbox always has the trailing circle.
            .when(
                !card
                    || (pane.state == State::Done
                        && self.agent_vision.reads.attention(pane).is_some()),
                |row| row.child(status),
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panes = self.agent_vision.filtered(cx);
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
                                        .px_2()
                                        .py_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(state.label()),
                                        )
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
                                                .map(|pane| self.agent_row(pane, true, cx)),
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
            let selected = self.agent_vision.state.selected().and_then(|key| {
                self.agent_vision
                    .panes
                    .iter()
                    .find(|pane| pane.location.key == key)
            });
            let other_count = rows.other.len();
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
                        .px_3()
                        .py_3()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(t(L10nKey::VisionInbox)),
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
                                .child(t(L10nKey::VisionNeedsYou)),
                        )
                        .when(rows.attention.is_empty(), |list| {
                            list.child(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t(L10nKey::VisionEmpty)),
                            )
                        })
                        .children(
                            rows.attention
                                .into_iter()
                                .map(|pane| self.agent_row(pane, false, cx)),
                        )
                        .when(other_count > 0, |list| {
                            list.child(
                                Button::new("agent-other-toggle")
                                    .label(format!("{} ({other_count})", t(L10nKey::VisionOther)))
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(|app, _, window, cx| {
                                        app.agent_vision.state.show_other =
                                            !app.agent_vision.state.show_other;
                                        app.focus_agent_list(window, cx);
                                    })),
                            )
                        })
                        .when(self.agent_vision.state.show_other, |list| {
                            list.children(
                                rows.other
                                    .into_iter()
                                    .map(|pane| self.agent_row(pane, false, cx)),
                            )
                        }),
                );
            let reader = v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .when_some(selected, |reader, pane| {
                    let key = pane.location.key;
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
                                                .child(match pane.pane_marker() {
                                                    Some(marker) => format!(
                                                        "{} · {marker}",
                                                        pane.location.tab_title
                                                    ),
                                                    None => pane.location.tab_title.clone(),
                                                }),
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
                                .when_some(self.agent_terminal(key, cx), |reader, terminal| {
                                    reader.child(terminal)
                                }),
                        )
                })
                .when(selected.is_none(), |reader| {
                    reader.child(
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::VisionSelect)),
                    )
                });
            h_flex()
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(list)
                .child(reader)
                .into_any_element()
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
            .into_any_element()
    }
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
        cx.update(|cx| crate::ui::presets::load_registry(cx));
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
