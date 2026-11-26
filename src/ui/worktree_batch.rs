//! One question for every worktree a bulk close left behind.
//!
//! Closing tabs one at a time asks about each tab's worktree as it goes. A
//! bulk close (the sidebar selection's Close, Close Other Tabs, Close Tabs to
//! the Right) would ask the same question once per tab, so it gathers the
//! worktrees first and asks once, with a Keep toggle per worktree. Every row
//! starts kept: nothing is deleted unless the person says so.

use gpui::{AnyElement, Context, FocusHandle, Window};

use crate::core::worktree::ManagedWorktree;
use crate::ui::app::Tty59App;
use crate::ui::host_ops::SharedHost;

/// A worktree a closed tab left, found managed by tty59 and used by no tab
/// still open, on the host it lives on.
pub(crate) struct LeftWorktree {
    pub(crate) host: SharedHost,
    pub(crate) wt: ManagedWorktree,
    /// Kept: left on disk. Starts `true`.
    pub(crate) keep: bool,
}

/// The open batch dialog.
pub(crate) struct WorktreeBatch {
    pub(crate) rows: Vec<LeftWorktree>,
    pub(crate) focus: FocusHandle,
}

impl Tty59App {
    /// Ask about `rows` (two or more) in one dialog, every row kept.
    pub(crate) fn open_worktree_batch(
        &mut self,
        rows: Vec<LeftWorktree>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.worktree_batch = Some(WorktreeBatch {
            rows: rows
                .into_iter()
                .map(|row| LeftWorktree { keep: true, ..row })
                .collect(),
            focus,
        });
        cx.notify();
    }

    /// The dialog over the whole window, while one is open. Package P2.
    pub(crate) fn render_worktree_batch_overlay(
        &self,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let _batch = self.worktree_batch.as_ref()?;
        None
    }
}
