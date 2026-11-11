//! One snapshot shared by the workbench agent rows, Inbox and Kanban.
use gpui::App;

use crate::core::group_key::{GroupKey, auto_names, pinned_names};
use crate::ui::app::Tty59App;

use super::{PaneKey, PaneLocation, PaneSummary, Project};

impl Tty59App {
    /// Agent panes in this window's current workspace, in tab/tree order.
    /// Local, routed and mirrored terminals all use their real host + pane id.
    /// Hibernated tabs have no live grid and do not appear. A tab's group and
    /// display title are the same ones used by its workbench row.
    pub(in crate::ui) fn agent_pane_summaries(&self, cx: &App) -> Vec<PaneSummary> {
        let groups = self.sidebar_group_keys(cx);
        let mut panes = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            if tab.is_asleep() {
                continue;
            }
            let (view, home) = tab.label_view(None, cx);
            let title = crate::ui::tab_strip::label_of(&view, index, home.as_deref());
            let start = panes.len();
            for terminal in tab.pane.terminals() {
                let terminal = terminal.read(cx);
                let session = terminal.agent_session();
                let location = PaneLocation {
                    key: PaneKey {
                        host: terminal.host_id(),
                        pane: terminal.pane_id,
                    },
                    workspace: self.workspace,
                    tab: tab.tree_id.get(),
                    tab_title: title.clone(),
                    group: groups[index].clone(),
                };
                if let Some(pane) = PaneSummary::from_live(
                    location,
                    terminal.agent(),
                    session.as_ref(),
                    terminal.agent_result_unread(),
                ) {
                    panes.push(pane);
                }
            }
            label_agent_panes(&mut panes[start..]);
        }
        panes
    }

    /// Current workspace's actual sidebar groups, including empty pinned
    /// groups. Auto names share the workbench's collision disambiguation.
    pub(in crate::ui) fn agent_projects(&self, cx: &App) -> Vec<Project> {
        let keys = self.sidebar_group_keys(cx);
        let mut projects: Vec<Project> = self
            .sidebar_groups
            .pinned
            .iter()
            .zip(pinned_names(&self.sidebar_groups.pinned))
            .map(|(group, name)| Project {
                group: Some(GroupKey::Pinned(group.id)),
                name,
            })
            .collect();
        let mut autos = Vec::new();
        for key in keys.iter().flatten().filter_map(GroupKey::auto) {
            if !autos.contains(&key) {
                autos.push(key);
            }
        }
        for (key, name) in autos.iter().zip(auto_names(&autos)) {
            projects.push(Project {
                group: Some(GroupKey::Auto((*key).clone())),
                name,
            });
        }
        if keys.iter().any(Option::is_none) {
            projects.push(Project {
                group: None,
                name: crate::ui::i18n::t(crate::ui::i18n::L10nKey::SidebarUngroupedGroup).into(),
            });
        }
        projects
    }
}

fn label_agent_panes(panes: &mut [PaneSummary]) {
    let count = panes.len();
    if count > 1 {
        for (index, pane) in panes.iter_mut().enumerate() {
            pane.location.tab_title =
                format!("{} · pane {}/{}", pane.location.tab_title, index + 1, count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cli_agent::CLIAgent;
    use crate::core::session::WorkspaceId;
    use crate::ui::host_ops::HostId;
    use tty59_core::core::machine::TabId;

    #[test]
    fn split_agent_titles_distinguish_panes_without_changing_identity() {
        let location = PaneLocation {
            key: PaneKey {
                host: HostId::LOCAL,
                pane: 7,
            },
            workspace: WorkspaceId::new(),
            tab: TabId::new(),
            tab_title: "Review".into(),
            group: None,
        };
        let a =
            PaneSummary::from_live(location.clone(), Some(CLIAgent::Codex), None, false).unwrap();
        let mut b = a.clone();
        b.location.key.pane = 8;
        let mut panes = [a, b];
        label_agent_panes(&mut panes[..1]);
        assert_eq!(panes[0].location.tab_title, "Review");
        label_agent_panes(&mut panes);
        assert_eq!(panes[0].location.tab_title, "Review · pane 1/2");
        assert_eq!(panes[1].location.tab_title, "Review · pane 2/2");
        assert_eq!(panes[0].location.key, location.key);
        assert_ne!(panes[0].location.key, panes[1].location.key);
    }

    #[gpui::test]
    fn collector_omits_plain_shells_and_keeps_empty_pinned_projects(cx: &mut gpui::TestAppContext) {
        let (app, mut vcx, _streams) = crate::ui::app::test_window::harness_with_tabs(cx, 2);
        app.update_in(&mut vcx, |app, _, cx| {
            app.sidebar_groups
                .pinned
                .push(crate::core::group_key::PinnedGroup::label("Project"));
            assert!(app.agent_pane_summaries(cx).is_empty());
            let projects = app.agent_projects(cx);
            assert_eq!(projects[0].name, "Project");
            assert_eq!(projects.len(), 2);
            assert_eq!(projects[1].group, None);
        });
    }
}
