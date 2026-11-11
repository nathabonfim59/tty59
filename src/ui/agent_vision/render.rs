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
            .ghost()
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
            .child(
                v_flex()
                    .px_4()
                    .py_3()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .gap_2()
                    .child(t(L10nKey::VisionListHelp))
                    .when_some(
                        crate::ui::keymap::effective_key("FocusNextAgentNeedingAttention", cx),
                        |footer, key| {
                            footer.child(format!(
                                "{} · {}",
                                t(L10nKey::VisionNext),
                                crate::ui::keymap::key_tokens(&key).join("")
                            ))
                        },
                    ),
            )
            .into_any_element()
    }

    fn agent_row(&self, pane: &PaneSummary, card: bool, cx: &mut Context<Self>) -> AnyElement {
        let key = pane.location.key;
        let selected = self.agent_vision.state.selected() == Some(key);
        let sf = cx.global::<crate::ui::presets::Surfaces>().window;
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
            self.row_status_dot(Some(pane.state.status()), 0, rgb(sf.base).into())
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
                row.bg(rgb(sf.selected)).text_color(rgb(sf.text_selected))
            })
            .when(!selected, |row| {
                row.text_color(rgb(sf.text_resting))
                    .hover(|style| style.bg(rgb(sf.hover)))
            })
            .anchor_scroll(selected.then(|| {
                if card {
                    self.agent_vision.board_anchor[pane.state.kanban_index()].clone()
                } else {
                    self.agent_vision.inbox_anchor.clone()
                }
            }))
            .child(self.tab_avatar_plain(
                SharedString::from(format!("agent-icon-{}-{}", key.host.0, key.pane)),
                pane.agent,
                Some(pane.state.status()),
                None,
                22.,
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .when(selected, |row| row.font_weight(FontWeight::MEDIUM))
                            .child(title),
                    )
                    .child(div().truncate().text_xs().child(reason.to_string()))
                    .when(card, |column| {
                        column.when_some(pane.last_line.clone(), |column, line| {
                            column.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(line),
                            )
                        })
                    })
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
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
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t(L10nKey::VisionTerminalHelp)),
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
            .when(self.agent_vision.state.mode() == ViewMode::Kanban, |view| {
                view.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t(L10nKey::VisionBoardHelp)),
                )
            })
            .into_any_element()
    }
}
