use super::{
    FocusZone, InboxReadState, KanbanColumns, NavigationKey, NavigationResult, PaneKey,
    PaneSummary, PaneTarget, Project, ViewMode, VisionState,
};

impl VisionState {
    pub fn focus_list_item(&mut self, key: PaneKey) {
        self.focus = FocusZone::List;
        self.selected = Some(key);
    }

    pub fn reconcile_focus(&mut self, panes: &[PaneSummary], reads: &InboxReadState) {
        match self.mode {
            ViewMode::Workbench => {}
            ViewMode::Inbox => {
                let rows = self.inbox_rows(panes, reads);
                if self.focus == FocusZone::Terminal {
                    if self
                        .selected
                        .is_some_and(|key| rows.keep_selection(Some(key)) == Some(key))
                    {
                        return;
                    }
                    self.focus = FocusZone::List;
                }
                let mut visible = rows.visible(self.show_other);
                if !self
                    .selected
                    .is_some_and(|key| visible.any(|pane| pane.location.key == key))
                {
                    self.selected = rows
                        .visible(self.show_other)
                        .next()
                        .map(|pane| pane.location.key);
                }
            }
            ViewMode::Kanban => {
                let board = self.kanban_columns(panes);
                self.selected = board
                    .position(self.selected)
                    .map(|(column, row)| board.0[column][row].location.key);
            }
        }
    }

    pub fn navigate(
        &mut self,
        key: NavigationKey,
        panes: &[PaneSummary],
        reads: &InboxReadState,
        projects: &[Project],
    ) -> NavigationResult {
        if self.mode == ViewMode::Workbench {
            return NavigationResult::Unhandled;
        }
        if self.focus == FocusZone::Terminal {
            if key != NavigationKey::Escape {
                return NavigationResult::Unhandled;
            }
            self.focus = FocusZone::List;
            self.show_other |= self.selected.is_some_and(|key| {
                self.inbox_rows(panes, reads)
                    .other
                    .iter()
                    .any(|pane| pane.location.key == key)
            });
            self.reconcile_focus(panes, reads);
            return NavigationResult::List;
        }

        if matches!(
            key,
            NavigationKey::PreviousProject | NavigationKey::NextProject
        ) {
            let filter = self
                .project_filter
                .cycle(projects, key == NavigationKey::NextProject);
            self.set_project_filter(filter);
            self.reconcile_focus(panes, reads);
            return NavigationResult::List;
        }

        match self.mode {
            ViewMode::Inbox => self.navigate_inbox(key, panes, reads),
            ViewMode::Kanban => self.navigate_kanban(key, panes, reads),
            ViewMode::Workbench => NavigationResult::Unhandled,
        }
    }

    fn navigate_inbox(
        &mut self,
        key: NavigationKey,
        panes: &[PaneSummary],
        reads: &InboxReadState,
    ) -> NavigationResult {
        if !matches!(
            key,
            NavigationKey::Up | NavigationKey::Down | NavigationKey::Enter
        ) {
            return NavigationResult::Unhandled;
        }
        let rows = self.inbox_rows(panes, reads);
        let items: Vec<_> = rows.visible(self.show_other).collect();
        let selected = self
            .selected
            .and_then(|selected| items.iter().position(|pane| pane.location.key == selected));
        let Some(first) = items.first() else {
            self.selected = None;
            return NavigationResult::List;
        };
        if key == NavigationKey::Enter {
            let pane = selected.map_or(*first, |index| items[index]);
            return self.open_inbox_terminal(pane, reads);
        }
        let index = selected.map_or(0, |index| {
            step(index, items.len(), key == NavigationKey::Down)
        });
        self.selected = Some(items[index].location.key);
        NavigationResult::List
    }

    fn navigate_kanban(
        &mut self,
        key: NavigationKey,
        panes: &[PaneSummary],
        reads: &InboxReadState,
    ) -> NavigationResult {
        if !matches!(
            key,
            NavigationKey::Up
                | NavigationKey::Down
                | NavigationKey::Left
                | NavigationKey::Right
                | NavigationKey::Enter
        ) {
            return NavigationResult::Unhandled;
        }
        let board = self.kanban_columns(panes);
        let Some((column, row)) = board.position(self.selected) else {
            self.selected = None;
            return NavigationResult::List;
        };
        let (column, row) = match key {
            NavigationKey::Enter => return self.open_inbox_terminal(board.0[column][row], reads),
            NavigationKey::Left | NavigationKey::Right => {
                let next = if key == NavigationKey::Right {
                    (column + 1..board.0.len()).find(|index| !board.0[*index].is_empty())
                } else {
                    (0..column).rev().find(|index| !board.0[*index].is_empty())
                }
                .unwrap_or(column);
                (next, row.min(board.0[next].len() - 1))
            }
            _ => (
                column,
                step(row, board.0[column].len(), key == NavigationKey::Down),
            ),
        };
        self.selected = Some(board.0[column][row].location.key);
        NavigationResult::List
    }

    pub fn open_inbox_terminal(
        &mut self,
        pane: &PaneSummary,
        reads: &InboxReadState,
    ) -> NavigationResult {
        if pane.location.workspace != self.workspace
            || !self.project_filter.contains(&pane.location.group)
        {
            return NavigationResult::Unhandled;
        }
        self.mode = ViewMode::Inbox;
        self.focus = FocusZone::Terminal;
        self.selected = Some(pane.location.key);
        self.show_other |= reads.attention(pane).is_none();
        NavigationResult::Terminal(PaneTarget::from(&pane.location))
    }
}

impl KanbanColumns<'_> {
    fn position(&self, selected: Option<PaneKey>) -> Option<(usize, usize)> {
        selected
            .and_then(|key| {
                self.0.iter().enumerate().find_map(|(column, cards)| {
                    cards
                        .iter()
                        .position(|pane| pane.location.key == key)
                        .map(|row| (column, row))
                })
            })
            .or_else(|| {
                self.0
                    .iter()
                    .position(|cards| !cards.is_empty())
                    .map(|column| (column, 0))
            })
    }
}

fn step(index: usize, len: usize, forward: bool) -> usize {
    if forward {
        (index + 1).min(len - 1)
    } else {
        index.saturating_sub(1)
    }
}
