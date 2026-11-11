use gpui::prelude::*;
use gpui::{App, Context, Entity, FocusHandle, Focusable as _, KeyDownEvent, Subscription, Window};
use gpui_component::WindowExt as _;
use gpui_component::input::{InputEvent, InputState};

use crate::core::session::WorkspaceId;
use crate::terminal::view::TerminalView;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t};

use super::{
    FocusZone, InboxReadState, NavigationKey, NavigationResult, PaneKey, PaneSummary, Project,
    ViewMode, VisionState,
};

pub(in crate::ui) struct AgentVision {
    pub state: VisionState,
    pub reads: InboxReadState,
    pub panes: Vec<PaneSummary>,
    pub projects: Vec<Project>,
    pub input: Entity<InputState>,
    pub list_focus: FocusHandle,
    pub inbox_scroll: gpui::ScrollHandle,
    pub inbox_anchor: gpui::ScrollAnchor,
    pub board_scroll: [gpui::ScrollHandle; 5],
    pub board_anchor: [gpui::ScrollAnchor; 5],
    pub board_horizontal: gpui::ScrollHandle,
    pub board_lane_anchor: gpui::ScrollAnchor,
    pub last_mode: ViewMode,
    _input_sub: Subscription,
}

impl AgentVision {
    pub fn new(workspace: WorkspaceId, window: &mut Window, cx: &mut Context<Tty59App>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::VisionSearch)));
        let input_sub = cx.subscribe_in(&input, window, |app, _, ev, window, cx| match ev {
            InputEvent::Change => {
                app.agent_vision.state.reset_list_focus();
                cx.notify();
            }
            InputEvent::PressEnter { .. } => app.focus_agent_list(window, cx),
            _ => {}
        });
        let mut state = VisionState::new(workspace);
        if cx
            .global::<crate::core::config::Config>()
            .agent_vision_default
        {
            state.set_mode(ViewMode::Inbox);
        }
        let inbox_scroll = gpui::ScrollHandle::new();
        let board_horizontal = gpui::ScrollHandle::new();
        let board_scroll = std::array::from_fn(|_| gpui::ScrollHandle::new());
        let board_anchor = std::array::from_fn(|index| {
            gpui::ScrollAnchor::for_handle(board_scroll[index].clone())
        });
        Self {
            state,
            reads: InboxReadState::default(),
            panes: Vec::new(),
            projects: Vec::new(),
            input,
            list_focus: cx.focus_handle(),
            inbox_anchor: gpui::ScrollAnchor::for_handle(inbox_scroll.clone()),
            inbox_scroll,
            board_scroll,
            board_anchor,
            board_lane_anchor: gpui::ScrollAnchor::for_handle(board_horizontal.clone()),
            board_horizontal,
            last_mode: ViewMode::Inbox,
            _input_sub: input_sub,
        }
    }

    pub fn filtered(&self, cx: &App) -> Vec<PaneSummary> {
        let query = self.input.read(cx).value().to_lowercase();
        self.panes
            .iter()
            .filter(|pane| {
                let project = self
                    .projects
                    .iter()
                    .find(|project| project.group == pane.location.group)
                    .map_or("", |project| project.name.as_str());
                pane.matches_search(&query, project)
            })
            .cloned()
            .collect()
    }
}

impl Tty59App {
    pub(in crate::ui) fn agent_mode_active(&self) -> bool {
        self.agent_vision.state.mode() != ViewMode::Workbench
    }

    pub(in crate::ui) fn refresh_agent_vision(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_changed = self.agent_vision.state.workspace != self.workspace;
        self.agent_vision.state.set_workspace(self.workspace);
        if workspace_changed {
            self.agent_vision
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
            if self.agent_mode_active() {
                window.focus(&self.agent_vision.list_focus, cx);
            }
        }
        self.agent_vision.panes = self.agent_pane_summaries(cx);
        self.agent_vision.projects = self.agent_projects(cx);
        self.agent_vision.reads.observe(&self.agent_vision.panes);
        self.agent_vision
            .state
            .project_filter
            .reconcile(&self.agent_vision.projects);
        let panes = self.agent_vision.filtered(cx);
        let previous = self.agent_vision.state.focus();
        self.agent_vision
            .state
            .reconcile_focus(&panes, &self.agent_vision.reads);
        if previous == FocusZone::Terminal && self.agent_vision.state.focus() == FocusZone::List {
            window.focus(&self.agent_vision.list_focus, cx);
        }
    }

    pub(in crate::ui) fn set_agent_mode(
        &mut self,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.agent_vision.state.selected();
        self.agent_vision.state.set_mode(mode);
        if mode == ViewMode::Workbench {
            if let Some(key) = selected {
                self.reveal_agent_in_workbench(key, window, cx);
            } else if let Some(tab) = self.tabs.get(self.active)
                && let Some(leaf) = tab.pane.first_leaf()
            {
                window.focus(&leaf.focus_handle(cx), cx);
            }
        } else {
            self.agent_vision.last_mode = mode;
            self.focus_agent_list(window, cx);
        }
        cx.notify();
    }

    pub(in crate::ui) fn toggle_agent_vision(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = if self.agent_mode_active() {
            ViewMode::Workbench
        } else {
            self.agent_vision.last_mode
        };
        self.set_agent_mode(mode, window, cx);
    }

    pub(in crate::ui) fn toggle_agent_board(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = if self.agent_vision.state.mode() == ViewMode::Kanban {
            ViewMode::Inbox
        } else {
            ViewMode::Kanban
        };
        self.set_agent_mode(mode, window, cx);
    }

    pub(in crate::ui) fn agent_terminal(
        &self,
        key: PaneKey,
        cx: &App,
    ) -> Option<Entity<TerminalView>> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .find(|leaf| {
                let view = leaf.read(cx);
                view.host_id() == key.host && view.pane_id == key.pane
            })
    }

    pub(in crate::ui) fn focus_agent_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_vision.state.focus = FocusZone::List;
        let panes = self.agent_vision.filtered(cx);
        self.agent_vision
            .state
            .reconcile_focus(&panes, &self.agent_vision.reads);
        window.focus(&self.agent_vision.list_focus, cx);
        if self.agent_vision.state.mode() == ViewMode::Inbox {
            self.agent_vision.inbox_anchor.scroll_to(window, cx);
        } else {
            self.agent_vision.board_lane_anchor.scroll_to(window, cx);
            for anchor in &self.agent_vision.board_anchor {
                anchor.scroll_to(window, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::ui) fn open_agent_terminal(
        &mut self,
        key: PaneKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .agent_vision
            .panes
            .iter()
            .find(|pane| pane.location.key == key)
        else {
            return;
        };
        self.agent_vision
            .state
            .open_inbox_terminal(pane, &self.agent_vision.reads);
        self.agent_vision.last_mode = ViewMode::Inbox;
        if let Some(terminal) = self.agent_terminal(key, cx) {
            self.handle_tray_action(
                crate::ui::tray::TrayAction::RevealPane {
                    leaf_id: terminal.entity_id().as_u64(),
                },
                window,
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::ui) fn reveal_agent_in_workbench(
        &mut self,
        key: PaneKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.tabs.iter().position(|tab| {
            tab.pane.terminals().iter().any(|leaf| {
                let leaf = leaf.read(cx);
                leaf.host_id() == key.host && leaf.pane_id == key.pane
            })
        });
        if let Some(index) = index {
            self.agent_vision.state.set_mode(ViewMode::Workbench);
            self.activate(index, window, cx);
            if let Some(terminal) = self.agent_terminal(key, cx) {
                self.maximized = Some(terminal.clone());
                window.focus(&terminal.focus_handle(cx), cx);
            }
            cx.notify();
        }
    }

    pub(in crate::ui) fn handle_agent_attention(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .agent_vision
            .panes
            .iter()
            .find(|pane| Some(pane.location.key) == self.agent_vision.state.selected())
        {
            self.agent_vision.reads.mark_read(pane);
            self.agent_vision.state.show_other = true;
        }
        if self.agent_vision.state.focus() == FocusZone::Terminal {
            if let Some(key) = self.agent_vision.state.selected() {
                self.open_agent_terminal(key, window, cx);
            }
        } else {
            self.focus_agent_list(window, cx);
        }
        cx.notify();
    }

    pub(in crate::ui) fn next_agent_attention(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .agent_vision
            .state
            .next_needing_attention(&self.agent_vision.panes, &mut self.agent_vision.reads);
        if let Some(target) = target {
            self.agent_vision
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.open_agent_terminal(target.key, window, cx);
        } else {
            window.push_notification(t(L10nKey::VisionNothingNext), cx);
        }
    }

    pub(in crate::ui) fn agent_vision_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.agent_mode_active()
            || self.search.is_some()
            || self.switcher.is_some()
            || self.has_settings()
        {
            return;
        }
        let mods = ev.keystroke.modifiers;
        if mods.control || mods.platform || mods.alt {
            return;
        }
        let key = ev.keystroke.key.as_str();
        if self
            .agent_vision
            .input
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            match key {
                "escape" => {
                    self.agent_vision
                        .input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    self.focus_agent_list(window, cx);
                }
                "down" | "enter" => self.focus_agent_list(window, cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        // The workspace name and modal fields live outside the list/reader.
        // Their Enter/Escape keys must not handle the selected agent.
        if !self.agent_vision.list_focus.contains_focused(window, cx) {
            return;
        }
        if self
            .agent_vision
            .state
            .selected()
            .and_then(|key| self.agent_terminal(key, cx))
            .is_some_and(|terminal| terminal.focus_handle(cx).contains_focused(window, cx))
        {
            self.agent_vision.state.focus = FocusZone::Terminal;
        }
        if self.agent_vision.state.focus() == FocusZone::Terminal && key == "enter" {
            // The key continues into the real terminal. Handling is local read
            // state; only the daemon decides the agent's actual status.
            self.handle_agent_attention(window, cx);
            return;
        }
        let Some(key) = NavigationKey::from_key(key) else {
            return;
        };
        let panes = self.agent_vision.filtered(cx);
        match self.agent_vision.state.navigate(
            key,
            &panes,
            &self.agent_vision.reads,
            &self.agent_vision.projects,
        ) {
            NavigationResult::Unhandled => return,
            NavigationResult::List => self.focus_agent_list(window, cx),
            NavigationResult::Terminal(target) => self.open_agent_terminal(target.key, window, cx),
        }
        cx.stop_propagation();
        cx.notify();
    }
}
