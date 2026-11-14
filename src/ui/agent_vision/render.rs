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

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedAgentRow {
    pub key: super::PaneKey,
    pub card: bool,
    pub title: String,
    pub reason: String,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedReaderHeader {
    pub key: super::PaneKey,
    pub title: String,
}

#[cfg(test)]
std::thread_local! {
    static RENDERED_ROWS: std::cell::RefCell<Vec<RenderedAgentRow>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static RENDERED_HEADERS: std::cell::RefCell<Vec<RenderedReaderHeader>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn take_rendered_rows() -> Vec<RenderedAgentRow> {
    RENDERED_ROWS.with(|rows| std::mem::take(&mut *rows.borrow_mut()))
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
        let label = SharedString::from(if active {
            t(L10nKey::VisionWorkbench).to_string()
        } else {
            format!("{} ({count})", t(L10nKey::VisionAgents))
        });
        // The label is our own child, not the button's, so its size, weight and
        // figures are the ones `StripMetrics` measured rather than whatever the
        // button gives its text.
        let text = div()
            .flex_none()
            .line_height(gpui::relative(1.))
            .text_size(gpui::rems(crate::ui::tab_strip::PILL_TEXT))
            .font_weight(FontWeight::NORMAL)
            .not_italic()
            .font_features(self.strip_metrics.features())
            .child(label.clone());
        let pill = Button::new("agent-mode-toggle")
            .accessible_label(label)
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
        let sf = cx.global::<crate::ui::presets::Surfaces>().window;
        let project = self
            .agent_vision
            .projects
            .iter()
            .find(|project| project.group == pane.location.group)
            .map_or("", |project| project.name.as_str());
        let exited = pane.state == State::Exited;
        let title = pane.heading();
        let now = crate::core::config::unix_now();
        let reason = pane.reason_line(now);
        let mut meta = format!("{} · {}", pane.agent_name(), project);
        if let Some(since) = pane.since_ms.filter(|_| !exited) {
            let age = now.saturating_sub(since / 1000);
            meta.push_str(&format!(" · {}", crate::ui::home::compact_age(age)));
        }
        #[cfg(test)]
        RENDERED_ROWS.with(|rows| {
            rows.borrow_mut().push(RenderedAgentRow {
                key,
                card,
                title: title.clone(),
                reason: reason.clone(),
            });
        });
        let status = match pane.state {
            State::Exited => None,
            State::Idle => Some(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .border_1()
                    .border_color(cx.theme().muted_foreground)
                    .bg(cx.theme().muted_foreground)
                    .map(|dot| {
                        #[cfg(test)]
                        let dot = dot.debug_selector(move || {
                            format!("agent-row-dot-{}-{}", key.host.0, key.pane)
                        });
                        dot
                    })
                    .into_any_element(),
            ),
            state => Some(
                self.row_status_dot(Some(state.status()), 0, rgb(sf.base).into())
                    .unwrap_or_else(|| div().into_any_element()),
            ),
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
            .child(
                div()
                    .flex_shrink_0()
                    .self_start()
                    .map(|icon| {
                        #[cfg(test)]
                        let icon = icon.debug_selector(move || {
                            format!("agent-row-icon-{}-{}", key.host.0, key.pane)
                        });
                        icon
                    })
                    .child(self.tab_avatar_plain(
                        SharedString::from(format!("agent-icon-{}-{}", key.host.0, key.pane)),
                        pane.agent,
                        Some(pane.state.status()),
                        None,
                        22.,
                        cx,
                    )),
            )
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
                            .when(exited, |title| {
                                title.text_color(cx.theme().muted_foreground)
                            })
                            .map(|title| {
                                #[cfg(test)]
                                let title = title.debug_selector(move || {
                                    format!("agent-row-title-{}-{}", key.host.0, key.pane)
                                });
                                title
                            })
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .when(exited, |reason| {
                                reason.text_color(cx.theme().muted_foreground)
                            })
                            .child(reason),
                    )
                    .when(card && !exited, |column| {
                        column.when_some(pane.last_line.clone(), |column, line| {
                            column.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .map(|preview| {
                                        #[cfg(test)]
                                        let preview = preview.debug_selector(move || {
                                            format!("agent-row-preview-{}-{}", key.host.0, key.pane)
                                        });
                                        preview
                                    })
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
                |row| row.children(status),
            )
            .on_click(
                cx.listener(move |app, _, window, cx| app.open_agent_terminal(key, window, cx)),
            );
        #[cfg(test)]
        let row =
            row.debug_selector(move || format!("agent-row-frame-{}-{}", key.host.0, key.pane));
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
