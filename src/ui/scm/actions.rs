//! Where the source control actions and palette commands land.
//!
//! One `ScmIntent` match, so the four ways of asking for a verb — the action,
//! the key binding, the palette entry and the button on the row — cannot drift
//! into meaning different things.

use gpui::{Context, PromptButton, PromptLevel, Window};

use tty59_core::core::git::ops::{Destructive, GitOp, PullMode};

use crate::core::config::DiffViewMode;
use crate::ui::app::Tty59App;
use crate::ui::host_registry::HostRegistry;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::scm::state::RepoKey;

/// One entry point for every source control verb.
///
/// A single enum rather than fourteen methods: the actions, the palette and
/// the row buttons all want the same behaviour, and routing them through one
/// match is what keeps the three from drifting apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScmIntent {
    Commit,
    CommitAmend,
    /// Commit, then send it on. Two operations rather than one, so the commit
    /// still stands if the network half fails — and strictly in that order:
    /// the send rides in the commit's [`ScmFollowUp`], because a push
    /// dispatched alongside the commit resolves the branch tip whenever the
    /// pool gets to it, and pushing the *old* tip reports success while
    /// sending nothing.
    CommitAndPush,
    CommitAndSync,
    StageAll,
    UnstageAll,
    DiscardAll,
    Refresh,
    Sync,
    Push,
    Pull,
    Fetch,
    CheckoutBranch,
    CreateBranch,
}

/// What to run once an operation has landed *successfully*.
///
/// Compound verbs — commit-and-push, pull-then-push, discard-all's two halves
/// — are sequences, not bundles: the second operation only makes sense against
/// the repository the first one produced. Dispatching both into the worker
/// pool at once lets them race, so the second rides here and is started from
/// the first one's landing closure instead. A failed or cancelled first half
/// drops the follow-up.
#[derive(Clone, Debug)]
pub(crate) enum ScmFollowUp {
    /// Push the current branch, re-reading the (by then updated) status.
    Push,
    /// Pull, then push — the whole sync sequence.
    Sync,
    /// One more operation, run without a second confirmation: the prompt that
    /// approved the first half covered this one too.
    Op(GitOp),
}

impl Tty59App {
    /// Fold the history section open or shut and remember it.
    pub(crate) fn scm_toggle_graph(&mut self, cx: &mut Context<Self>) {
        let next = !self.scm.graph.expanded;
        self.scm.graph.expanded = next;
        self.update_config(cx, |cfg| cfg.scm_graph_expanded = next);
        cx.notify();
    }

    /// Switch the changed-file list between flat and tree, and remember it.
    ///
    /// Global like `diff_view`: someone who reads changes by directory reads
    /// every repository's that way.
    pub(crate) fn scm_toggle_changes_tree(&mut self, cx: &mut Context<Self>) {
        let next = !cx.global::<crate::core::config::Config>().scm_changes_tree;
        self.update_config(cx, |cfg| cfg.scm_changes_tree = next);
        cx.notify();
    }

    /// Flip the diff overlay between side-by-side and unified.
    ///
    /// Global rather than per-overlay, matching `diffEditor.renderSideBySide`:
    /// someone who prefers unified prefers it for every file.
    pub(crate) fn toggle_diff_view_mode(&mut self, cx: &mut Context<Self>) {
        let next = match cx.global::<crate::core::config::Config>().diff_view {
            DiffViewMode::Split => DiffViewMode::Unified,
            DiffViewMode::Unified => DiffViewMode::Split,
        };
        self.update_config(cx, |cfg| cfg.diff_view = next);
        cx.notify();
    }

    /// Run one operation against the panel's repository, asking first when it
    /// can lose work.
    ///
    /// The gate lives here rather than in `run_git_op` because
    /// [`GitOp::destructive`] is advice about what the user stands to lose,
    /// and only a window can ask them. `window.prompt` is the project's one
    /// confirmation mechanism — deleting a file in the tree already uses it —
    /// so no modal component is introduced for this.
    pub(crate) fn scm_op(
        &mut self,
        repo: RepoKey,
        op: GitOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scm_op_then(repo, op, None, window, cx);
    }

    /// [`scm_op`], with something to run once this operation has succeeded.
    /// Cancelling the confirmation drops the follow-up along with the op.
    pub(crate) fn scm_op_then(
        &mut self,
        repo: RepoKey,
        op: GitOp,
        then: Option<ScmFollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = HostRegistry::get(cx, repo.host) else {
            return;
        };
        let Some(loss) = op.destructive() else {
            self.run_git_op(host, repo.root, op, then, window, cx);
            return;
        };
        // A discard says what it does to which files; every other operation
        // keeps the question and verb it has always had.
        let (message, detail, verb) = match discard_copy(std::slice::from_ref(&op)) {
            Some(copy) => (copy.message, copy.detail, copy.verb),
            None => (confirm_question(&op, loss), None, confirm_verb(&op, loss)),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            detail.as_deref(),
            &guarded_answers(verb),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            let Ok(1) = answer.await else { return };
            let _ = app.update_in(cx, |app, window, cx| {
                app.run_git_op(host, repo.root, op, then, window, cx)
            });
        })
        .detach();
    }

    pub(crate) fn run_scm_action(
        &mut self,
        intent: ScmIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.scm.active_repo().cloned() else {
            return;
        };
        match intent {
            ScmIntent::Refresh => {
                self.scm_invalidate(repo.host, &repo.root, cx);
                cx.notify();
            }
            ScmIntent::StageAll => self.scm_op(repo, GitOp::StageAll, window, cx),
            ScmIntent::UnstageAll => self.scm_op(repo, GitOp::UnstageAll, window, cx),
            ScmIntent::DiscardAll => self.scm_discard_all(repo, window, cx),
            ScmIntent::Commit => {
                let amend = self.scm.amend;
                self.scm_commit(repo, amend, None, window, cx);
            }
            ScmIntent::CommitAmend => self.scm_commit(repo, true, None, window, cx),
            ScmIntent::CommitAndPush => {
                let amend = self.scm.amend;
                self.scm_commit(repo, amend, Some(ScmFollowUp::Push), window, cx);
            }
            ScmIntent::CommitAndSync => {
                let amend = self.scm.amend;
                self.scm_commit(repo, amend, Some(ScmFollowUp::Sync), window, cx);
            }
            ScmIntent::Sync => self.scm_sync(repo, window, cx),
            ScmIntent::Push => self.scm_push(repo, false, window, cx),
            ScmIntent::Pull => self.scm_op(
                repo,
                GitOp::Pull {
                    mode: PullMode::FfOnly,
                },
                window,
                cx,
            ),
            ScmIntent::Fetch => self.scm_op(
                repo,
                GitOp::Fetch {
                    remote: None,
                    prune: false,
                },
                window,
                cx,
            ),
            ScmIntent::CreateBranch => self.scm_begin_create_branch(window, cx),
            ScmIntent::CheckoutBranch => self.scm_begin_checkout_branch(window, cx),
        }
    }

    /// Commit whatever the message box holds.
    ///
    /// The message comes from the box when it is the one on screen and from
    /// the saved draft otherwise, so the key binding and the palette entry
    /// commit the same text the user can see.
    pub(crate) fn scm_commit(
        &mut self,
        repo: RepoKey,
        amend: bool,
        then: Option<ScmFollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = crate::terminal::git_data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let message = self.scm_message(&repo, cx);
        let plan = crate::ui::scm::panel::commit_plan(&status, amend, &message);
        if !plan.enabled {
            // The panel's own button exposes this through disabled + tooltip;
            // the palette and the key binding have no tooltip to hover, so
            // the toast has to carry the reason itself — "write a message
            // first" and "nothing to commit" call for opposite actions, and
            // answering the first with the second sends the user staging
            // files they already staged (#546).
            gpui_component::WindowExt::push_notification(window, plan.reason_text(), cx);
            // The follow-up dies with the commit: "commit and push" with
            // nothing to commit must not push whatever the branch holds.
            return;
        }
        let all = crate::ui::scm::panel::commit_stages_everything(&status, amend);
        // The amend toggle is *not* cleared here: `run_git_op` clears it
        // where it arms `scm.committing`, at dispatch — after the amend
        // confirmation, not before it — so a cancelled prompt leaves the
        // toggle and the armed flag exactly as the user set them. Clearing
        // here made Cancel quietly switch amend off, and the next Commit
        // became the new-commit the user had just declined to risk. See
        // `scm_commit_landed`.
        self.scm_op_then(
            repo,
            GitOp::Commit {
                message,
                amend,
                signoff: false,
                no_verify: false,
                all,
            },
            then,
            window,
            cx,
        );
    }

    /// What the commit box holds for a repository, whether or not it is the
    /// one currently on screen.
    fn scm_message(&self, repo: &RepoKey, cx: &gpui::App) -> String {
        match (&self.scm.commit_input, &self.scm.commit_repo) {
            (Some(input), Some(showing)) if showing == repo => input.read(cx).value().to_string(),
            _ => self.scm.draft(repo).to_string(),
        }
    }

    /// Park everything, including the files git does not track yet.
    ///
    /// `-u`, because a stash that silently leaves new files behind is a stash
    /// that did not do what "stash all" says.
    pub(crate) fn scm_stash_all(
        &mut self,
        repo: RepoKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match self.scm_message(&repo, cx) {
            m if m.trim().is_empty() => None,
            m => Some(m),
        };
        self.scm_op(
            repo,
            GitOp::Stash {
                message,
                include_untracked: true,
            },
            window,
            cx,
        );
    }

    /// Throw away every change in the worktree: unstaged edits and untracked
    /// files alike.
    ///
    /// Two operations, because git has no single command for it —
    /// `checkout --` cannot touch a file it has never heard of, and `clean`
    /// cannot touch one it has. One confirmation and one sequence, though:
    /// the second half rides in the first one's [`ScmFollowUp`], so the user
    /// answers a single dialog and the two gits never race each other.
    ///
    /// Only *unstaged* paths go to `checkout --`: it restores from the index,
    /// so a staged edit would survive it anyway, and a staged deletion — a
    /// path in neither index nor worktree — would make git reject the whole
    /// batch as an unmatched pathspec. What is staged stays staged, which is
    /// also what the button's own group implies.
    fn scm_discard_all(&mut self, repo: RepoKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = crate::terminal::git_data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let (first, second) = match &discard_all_ops(&status)[..] {
            [] => return,
            [one] => (one.clone(), None),
            [a, b, ..] => (a.clone(), Some(b.clone())),
        };
        let Some(host) = HostRegistry::get(cx, repo.host) else {
            return;
        };
        // Its own prompt rather than `scm_op_then`'s: the question follows
        // what the sweep really holds, and one that also deletes untracked
        // files must say so rather than ask about changes to one file.
        let ops: Vec<GitOp> = std::iter::once(first.clone())
            .chain(second.clone())
            .collect();
        let (message, detail, verb) = match discard_copy(&ops) {
            Some(copy) => (copy.message, copy.detail, copy.verb),
            None => (
                t(L10nKey::ScmDiscardAllConfirm).to_string(),
                None,
                t(L10nKey::ScmDiscard),
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            detail.as_deref(),
            &guarded_answers(verb),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            let Ok(1) = answer.await else { return };
            let _ = app.update_in(cx, |app, window, cx| {
                app.run_git_op(
                    host,
                    repo.root,
                    first,
                    second.map(ScmFollowUp::Op),
                    window,
                    cx,
                )
            });
        })
        .detach();
    }

    /// Push the current branch to its upstream, or publish it if it has none.
    pub(crate) fn scm_push(
        &mut self,
        repo: RepoKey,
        force_with_lease: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = crate::terminal::git_data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let name = match crate::ui::scm::panel::pushable_branch(&status.head) {
            Ok(name) => name.to_string(),
            Err(reason) => {
                // "A swallowed click on Push looks exactly like a push that
                // finished instantly" — git_data.rs says it out loud about
                // the busy slot, and the same holds here. The tile and the
                // menu disable themselves, but the key binding, the palette
                // and the follow-up half of "Commit and Push" all land in
                // this guard, so the toast is the only place they can say
                // why nothing moved (#545).
                gpui_component::WindowExt::push_notification(window, t(reason).to_string(), cx);
                return;
            }
        };
        let (remote, branch) = match status.upstream.as_deref().and_then(split_upstream) {
            Some((remote, branch)) => (remote.to_string(), branch.to_string()),
            None => ("origin".to_string(), name),
        };
        let set_upstream = status.upstream.is_none();
        self.scm_op(
            repo,
            GitOp::Push {
                remote,
                branch,
                set_upstream,
                force_with_lease,
            },
            window,
            cx,
        );
    }

    /// Pull then push, which is what "sync" means everywhere else — and
    /// strictly in that order: the push rides in the pull's [`ScmFollowUp`],
    /// because a push racing the pull it was waiting for reads the pre-pull
    /// tip and earns a non-fast-forward rejection from the very sync that
    /// was fixing it. A failed pull stops the sequence.
    ///
    /// A branch with no upstream has nothing to pull, so sync is a publish.
    pub(crate) fn scm_sync(&mut self, repo: RepoKey, window: &mut Window, cx: &mut Context<Self>) {
        let has_upstream = crate::terminal::git_data::status_of(cx, repo.host, &repo.root)
            .is_some_and(|s| s.upstream.is_some());
        if has_upstream {
            self.scm_op_then(
                repo,
                GitOp::Pull {
                    mode: PullMode::FfOnly,
                },
                Some(ScmFollowUp::Push),
                window,
                cx,
            );
        } else {
            self.scm_push(repo, false, window, cx);
        }
    }

    /// Run the second half of a compound verb, from the first half's landing.
    pub(crate) fn scm_follow_up(
        &mut self,
        host: tty59_core::host::HostId,
        root: std::path::PathBuf,
        follow: ScmFollowUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = RepoKey { host, root };
        match follow {
            ScmFollowUp::Push => self.scm_push(repo, false, window, cx),
            ScmFollowUp::Sync => self.scm_sync(repo, window, cx),
            ScmFollowUp::Op(op) => {
                let Some(shared) = HostRegistry::get(cx, repo.host) else {
                    return;
                };
                self.run_git_op(shared, repo.root, op, None, window, cx);
            }
        }
    }

    /// A dispatched commit came back with an error: disarm the latch that
    /// would otherwise clear the message box on the next unrelated HEAD move.
    /// The message itself stays where the user can see it.
    pub(crate) fn scm_commit_failed(
        &mut self,
        host: tty59_core::host::HostId,
        root: &std::path::Path,
    ) {
        if self
            .scm
            .committing
            .as_ref()
            .is_some_and(|(r, _, _)| r.host == host && r.root == root)
        {
            self.scm.committing = None;
        }
    }
}

/// What "discard all" actually runs, in order. Pure so a test can hold it up
/// against a status without a window.
fn discard_all_ops(status: &tty59_core::core::git::status::WorkingTreeStatus) -> Vec<GitOp> {
    let unstaged: Vec<_> = status
        .unstaged()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect();
    let untracked: Vec<_> = status
        .untracked()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect();
    let mut ops = Vec::new();
    if !unstaged.is_empty() {
        ops.push(GitOp::DiscardWorktree { paths: unstaged });
    }
    if !untracked.is_empty() {
        let directories = untracked.iter().any(|p| p.as_str().ends_with('/'));
        ops.push(GitOp::DiscardUntracked {
            paths: untracked,
            directories,
        });
    }
    ops
}

/// `origin/main` → `("origin", "main")`.
///
/// The first component is the remote: a branch name may contain slashes, a
/// remote name may not.
pub(crate) fn split_upstream(upstream: &str) -> Option<(&str, &str)> {
    let (remote, branch) = upstream.split_once('/')?;
    (!remote.is_empty() && !branch.is_empty()).then_some((remote, branch))
}

/// The question a destructive operation has to answer before it runs.
fn confirm_question(op: &GitOp, loss: Destructive) -> String {
    // Its own question, not the discard one: a hard reset to an older commit
    // drops commits off the branch, and a dialog that says "Discard every
    // change in this repository?" never mentions the part that hurts.
    if matches!(op, GitOp::Reset { .. }) {
        return t(L10nKey::ScmResetHardConfirm).to_string();
    }
    match loss {
        Destructive::RewritesHistory => t(L10nKey::ScmAmendConfirm).to_string(),
        // One file gets named; a whole group does not, because a list of two
        // hundred paths in a system dialog says less than the count does.
        _ => match op.paths() {
            [only] => t_fmt(L10nKey::ScmDiscardConfirm, &[("path", only.as_str())]),
            _ => t(L10nKey::ScmDiscardAllConfirm).to_string(),
        },
    }
}

fn confirm_verb(op: &GitOp, loss: Destructive) -> &'static str {
    if matches!(op, GitOp::Reset { .. }) {
        return t(L10nKey::ScmReset);
    }
    match loss {
        Destructive::RewritesHistory => t(L10nKey::ScmAmendLastCommit),
        _ => t(L10nKey::ScmDiscard),
    }
}

/// Cancel first, so Return lands on it and not on the loss, then the action.
///
/// Cancel is marked as one rather than passed as a bare label: gpui only
/// recognises a cancel by its label when that label is the English word, so
/// in any other language Escape had nothing to answer and the dialog could
/// not tell which button was the safe one.
fn guarded_answers(action: &str) -> [PromptButton; 2] {
    [
        PromptButton::cancel(t(L10nKey::Cancel)),
        PromptButton::new(action),
    ]
}

/// What a discard prompt says: the question, the line under it, and the verb
/// on the button that runs it.
#[derive(Debug, PartialEq, Eq)]
struct PromptCopy {
    message: String,
    detail: Option<String>,
    verb: &'static str,
}

/// The copy for a prompt made only of discards, `None` for every other
/// operation — and for an empty list — so those keep [`confirm_question`] and
/// [`confirm_verb`].
///
/// Tracked edits are *discarded* and untracked files are *deleted*, and a
/// sweep that does both says both. A single file keeps its name on screen; a
/// group gets its count, because a list of two hundred paths says less.
fn discard_copy(ops: &[GitOp]) -> Option<PromptCopy> {
    let mut tracked: Vec<&str> = Vec::new();
    let mut untracked: Vec<&str> = Vec::new();
    let mut folders = false;
    for op in ops {
        match op {
            GitOp::DiscardWorktree { paths } => tracked.extend(paths.iter().map(|p| p.as_str())),
            GitOp::DiscardUntracked { paths, directories } => {
                untracked.extend(paths.iter().map(|p| p.as_str()));
                folders |= *directories;
            }
            // One other operation in the list and it is not a discard question.
            _ => return None,
        }
    }
    let count = |n: usize| n.to_string();
    Some(match (tracked.as_slice(), untracked.as_slice()) {
        ([], []) => return None,
        (_, []) => match tracked.as_slice() {
            [only] => PromptCopy {
                message: t_fmt(L10nKey::ScmDiscardConfirm, &[("path", *only)]),
                detail: None,
                verb: t(L10nKey::ScmDiscard),
            },
            many => PromptCopy {
                message: t_fmt(
                    L10nKey::ScmDiscardManyConfirm,
                    &[("count", &count(many.len()))],
                ),
                detail: None,
                verb: t(L10nKey::ScmDiscard),
            },
        },
        ([], _) => match untracked.as_slice() {
            [only] => {
                let key = if folders || only.ends_with('/') {
                    L10nKey::ScmDeleteUntrackedFolderConfirm
                } else {
                    L10nKey::ScmDeleteUntrackedConfirm
                };
                PromptCopy {
                    message: t_fmt(key, &[("path", *only)]),
                    detail: None,
                    verb: t(L10nKey::ScmDeleteUntracked),
                }
            }
            many => PromptCopy {
                message: t_fmt(
                    L10nKey::ScmDeleteUntrackedManyConfirm,
                    &[("count", &count(many.len()))],
                ),
                detail: None,
                verb: t(L10nKey::ScmDeleteUntracked),
            },
        },
        _ => PromptCopy {
            message: t(L10nKey::ScmDiscardEverythingConfirm).to_string(),
            detail: Some(t(L10nKey::ScmDiscardEverythingDetail).to_string()),
            verb: t(L10nKey::ScmDiscardAll),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tty59_core::core::git::status::{
        ChangeCode, EntryKind, HeadState, RepoPath, StatusEntry, WorkingTreeStatus,
    };

    fn entry(path: &str, index: ChangeCode, worktree: ChangeCode, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: RepoPath::from_bytes(path.as_bytes()),
            orig_path: None,
            index,
            worktree,
            kind,
            submodule: None,
            rename_score: None,
            conflict: None,
        }
    }

    fn status_with(entries: Vec<StatusEntry>) -> WorkingTreeStatus {
        WorkingTreeStatus {
            root: std::path::PathBuf::from("/repo"),
            home: std::path::PathBuf::from("/repo"),
            head: HeadState::Branch {
                name: "main".into(),
                oid: "0".repeat(40),
            },
            upstream: None,
            ahead_behind: None,
            total_entries: entries.len(),
            entries,
            truncated: false,
            stash_count: 0,
            operation: None,
            prefilled_message: None,
        }
    }

    /// `checkout --` restores from the *index*: a staged-only path either
    /// survives it (staged edit) or — a staged deletion, in neither index nor
    /// worktree — makes git reject the whole batch as an unmatched pathspec,
    /// taking every real discard down with it. Only unstaged paths go in.
    #[test]
    fn discard_all_sends_only_unstaged_paths_to_checkout() {
        let status = status_with(vec![
            // Staged edit, clean worktree: not `checkout --`'s business.
            entry(
                "staged.rs",
                ChangeCode::Modified,
                ChangeCode::None,
                EntryKind::Tracked,
            ),
            // Staged deletion: the pathspec that used to sink the batch.
            entry(
                "deleted.rs",
                ChangeCode::Deleted,
                ChangeCode::None,
                EntryKind::Tracked,
            ),
            // Staged and edited again: the worktree half is discardable.
            entry(
                "both.rs",
                ChangeCode::Modified,
                ChangeCode::Modified,
                EntryKind::Tracked,
            ),
            entry(
                "edited.rs",
                ChangeCode::None,
                ChangeCode::Modified,
                EntryKind::Tracked,
            ),
            entry(
                "new.rs",
                ChangeCode::None,
                ChangeCode::None,
                EntryKind::Untracked,
            ),
        ]);
        let ops = discard_all_ops(&status);
        assert_eq!(ops.len(), 2, "one checkout batch, one clean batch");
        match &ops[0] {
            GitOp::DiscardWorktree { paths } => {
                let names: Vec<_> = paths.iter().map(|p| p.as_str()).collect();
                assert_eq!(names, vec!["both.rs", "edited.rs"]);
            }
            other => panic!("expected DiscardWorktree first, got {:?}", other.label()),
        }
        match &ops[1] {
            GitOp::DiscardUntracked { paths, directories } => {
                let names: Vec<_> = paths.iter().map(|p| p.as_str()).collect();
                assert_eq!(names, vec!["new.rs"]);
                assert!(!directories);
            }
            other => panic!("expected DiscardUntracked second, got {:?}", other.label()),
        }

        // Nothing to discard means nothing to run — and no prompt to answer.
        assert!(discard_all_ops(&status_with(Vec::new())).is_empty());
    }

    fn paths(names: &[&str]) -> Vec<RepoPath> {
        names
            .iter()
            .map(|n| RepoPath::from_bytes(n.as_bytes()))
            .collect()
    }

    fn tracked(names: &[&str]) -> GitOp {
        GitOp::DiscardWorktree {
            paths: paths(names),
        }
    }

    fn untracked(names: &[&str], directories: bool) -> GitOp {
        GitOp::DiscardUntracked {
            paths: paths(names),
            directories,
        }
    }

    fn copy_of(ops: &[GitOp]) -> PromptCopy {
        discard_copy(ops).expect("a prompt made of discards has copy of its own")
    }

    fn copy(message: &str, detail: Option<&str>, verb: &'static str) -> PromptCopy {
        PromptCopy {
            message: message.to_string(),
            detail: detail.map(str::to_string),
            verb,
        }
    }

    const COMBINED_DETAIL: &str = "Unstaged edits to tracked files are reverted and untracked \
         files and folders are deleted. Staged changes are kept. This cannot be undone here.";

    /// Tracked edits are discarded, untracked files are deleted, and a prompt
    /// that mixes the two says both; each one names what it will touch.
    #[test]
    fn a_discard_prompt_says_what_it_will_do_to_what() {
        // The literals are English, and a reused test thread keeps whatever
        // locale the last test on it chose.
        crate::ui::i18n::set_locale("en");
        assert_eq!(
            copy_of(&[tracked(&["src/deep/a.rs"])]),
            copy(
                "Discard changes to src/deep/a.rs? This cannot be undone.",
                None,
                "Discard Changes"
            ),
            "one tracked file: the original wording, with its whole path"
        );
        assert_eq!(
            copy_of(&[tracked(&["a.rs", "b.rs", "c.rs"])]),
            copy(
                "Discard changes to 3 files? This cannot be undone.",
                None,
                "Discard Changes"
            ),
            "a group of tracked files: the count, not a list"
        );
        assert_eq!(
            copy_of(&[untracked(&["notes.txt"], false)]),
            copy(
                "Delete notes.txt? It is untracked and cannot be restored by Git.",
                None,
                "Delete Untracked"
            ),
            "an untracked file is deleted, not discarded"
        );
        assert_eq!(
            copy_of(&[untracked(&["build/"], true)]),
            copy(
                "Delete build/ and everything in it? It is untracked and cannot be restored by Git.",
                None,
                "Delete Untracked"
            ),
            "an untracked folder takes what is inside it"
        );
        assert_eq!(
            copy_of(&[untracked(&["a.log", "b.log"], false)]),
            copy(
                "Delete 2 untracked items? They cannot be restored by Git.",
                None,
                "Delete Untracked"
            )
        );
        assert_eq!(
            copy_of(&[tracked(&["a.rs", "b.rs"]), untracked(&["new.rs"], false)]),
            copy(
                "Discard all unstaged changes?",
                Some(COMBINED_DETAIL),
                "Discard All Changes"
            ),
            "both halves of discard-all in one question"
        );
    }

    /// Only the question that really covers both halves gets the "all" verb,
    /// and nothing claims an untracked file can be recovered.
    #[test]
    fn only_the_combined_discard_uses_the_all_verb_and_nothing_promises_recovery() {
        crate::ui::i18n::set_locale("en");
        let single_op_discard_all = [
            vec![tracked(&["a.rs", "b.rs"])],
            vec![untracked(&["new.rs"], false)],
        ];
        for ops in &single_op_discard_all {
            assert_ne!(copy_of(ops).verb, "Discard All Changes", "{ops:?}");
        }
        for ops in [
            vec![tracked(&["a.rs"])],
            vec![untracked(&["a.rs"], false)],
            vec![untracked(&["d/"], true)],
            vec![tracked(&["a.rs"]), untracked(&["b.rs"], false)],
        ] {
            let c = copy_of(&ops);
            assert!(
                !c.message.contains("recovered")
                    && !c.detail.as_deref().unwrap_or("").contains("recovered"),
                "{c:?}"
            );
        }
    }

    /// Reset, amend, a forced push and a branch delete keep the copy they had.
    /// Until `discard_copy` exists for real the `None` half passes trivially;
    /// the `confirm_*` half pins the strings those operations show today.
    #[test]
    fn operations_that_are_not_discards_keep_their_own_confirmation() {
        crate::ui::i18n::set_locale("en");
        let reset = GitOp::Reset {
            rev: "HEAD~3".into(),
            mode: tty59_core::core::git::ops::ResetMode::Hard,
        };
        let amend = GitOp::Commit {
            message: "m".into(),
            amend: true,
            signoff: false,
            no_verify: false,
            all: false,
        };
        let force_push = GitOp::Push {
            remote: "origin".into(),
            branch: "main".into(),
            set_upstream: false,
            force_with_lease: true,
        };
        let delete_branch = GitOp::DeleteBranch {
            name: "topic".into(),
            force: true,
        };
        for op in [&reset, &amend, &force_push, &delete_branch] {
            assert!(
                discard_copy(std::slice::from_ref(op)).is_none(),
                "{:?} is not a discard",
                op.label()
            );
        }
        assert!(
            discard_copy(&[tracked(&["a.rs"]), reset.clone()]).is_none(),
            "a discard next to anything else is not the discard question"
        );
        assert!(discard_copy(&[GitOp::StageAll]).is_none());

        let ask = |op: &GitOp| {
            let loss = op.destructive().expect("it can lose work");
            (confirm_question(op, loss), confirm_verb(op, loss))
        };
        assert_eq!(
            ask(&reset),
            (
                t(L10nKey::ScmResetHardConfirm).to_string(),
                t(L10nKey::ScmReset)
            )
        );
        for op in [&amend, &force_push] {
            assert_eq!(
                ask(op),
                (
                    t(L10nKey::ScmAmendConfirm).to_string(),
                    t(L10nKey::ScmAmendLastCommit)
                )
            );
        }
        assert_eq!(
            ask(&delete_branch),
            (
                t(L10nKey::ScmDiscardAllConfirm).to_string(),
                t(L10nKey::ScmDiscard)
            ),
            "a branch delete has no copy of its own today; this change leaves it alone"
        );
    }

    // ---- real window, real repository: the prompt a discard-all opens ----
    //
    // These run `git`, so they run on Boat only.

    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn git(root: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A scratch repository with one committed file edited in the worktree and
    /// one untracked file: exactly what discard-all sweeps. A second committed
    /// file, `s.txt`, is edited and staged with a clean worktree: the sentinel
    /// for "staged changes are kept".
    fn dirty_repository() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("tty59-discard-")
            .tempdir()
            .expect("a scratch directory");
        let root = std::fs::canonicalize(dir.path()).expect("it resolves");
        git(&root, &["init", "--quiet"]);
        // Repository-local, so the code under test inherits it too: a Windows
        // system `core.autocrlf=true` would turn the one-LF files into CRLF.
        git(&root, &["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("s.txt"), "base\n").unwrap();
        git(&root, &["add", "a.txt", "s.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "first",
            ],
        );
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::write(root.join("s.txt"), "staged\n").unwrap();
        git(&root, &["add", "s.txt"]);
        (dir, root)
    }

    /// What the index holds for `path`, as git prints it.
    fn index_bytes(root: &std::path::Path, path: &str) -> Vec<u8> {
        let out = std::process::Command::new("git")
            .args(["show", &format!(":{path}")])
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git show :{path} failed");
        out.stdout
    }

    fn pump_until(
        vcx: &mut VisualTestContext,
        secs: u64,
        mut until: impl FnMut(&mut VisualTestContext) -> bool,
    ) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            vcx.run_until_parked();
            if until(vcx) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The app on `root` with its status read, and the discard-all prompt open.
    fn discard_all_prompt_open(
        cx: &mut TestAppContext,
        root: &std::path::Path,
    ) -> (Entity<Tty59App>, VisualTestContext) {
        use crate::ui::host_ops::HostId;

        crate::core::config::pin_test_config_dir();
        // The prompt's words and the Cancel button's label are asserted in
        // English.
        crate::ui::i18n::set_locale("en");
        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let repo = RepoKey {
            host: HostId::LOCAL,
            root: root.to_path_buf(),
        };
        app.update(&mut vcx, |app, cx| {
            let host = HostRegistry::get(cx, HostId::LOCAL).expect("the local host");
            app.scm_refresh(host, root.to_path_buf(), cx);
        });
        assert!(
            pump_until(&mut vcx, 20, |vcx| vcx.update(|_, cx| {
                crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_some()
            })),
            "the status never loaded"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            app.scm_discard_all(repo, window, cx)
        });
        assert!(
            pump_until(&mut vcx, 5, |vcx| vcx.has_pending_prompt()),
            "discard-all never asked"
        );
        (app, vcx)
    }

    /// What the window really shows, not what a helper says it would.
    #[gpui::test]
    fn discard_all_opens_the_truthful_prompt(cx: &mut TestAppContext) {
        let _serial = serial();
        let (_dir, root) = dirty_repository();
        let (_app, vcx) = discard_all_prompt_open(cx, &root);

        assert_eq!(
            vcx.pending_prompt(),
            Some((
                "Discard all unstaged changes?".to_string(),
                COMBINED_DETAIL.to_string()
            ))
        );
        vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    }

    /// Answering the button named Cancel leaves the repository exactly as it
    /// was. (This answers by label; it does not exercise Return or Escape.)
    #[gpui::test]
    fn cancelling_discard_all_changes_nothing(cx: &mut TestAppContext) {
        let _serial = serial();
        let (_dir, root) = dirty_repository();
        let (_app, mut vcx) = discard_all_prompt_open(cx, &root);

        vcx.simulate_prompt_answer(t(L10nKey::Cancel));
        // The answer lands on the next turns of the executor; give a wrong
        // implementation every chance to run the discard before looking.
        let _ = pump_until(&mut vcx, 2, |_| false);

        assert!(!vcx.has_pending_prompt());
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(root.join("new.txt").exists(), "Cancel deleted a file");
    }

    /// The positive control for the test above: the same fixture and the same
    /// wait, answered with the action's own button, does run the discard. It
    /// is what makes the Cancel test's "nothing changed" mean something.
    #[gpui::test]
    fn answering_discard_all_with_its_verb_runs_the_guarded_discard(cx: &mut TestAppContext) {
        use crate::ui::host_ops::HostId;

        let _serial = serial();
        let (_dir, root) = dirty_repository();
        let (_app, mut vcx) = discard_all_prompt_open(cx, &root);

        let verb = vcx.update(|_, cx| {
            let status = crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root)
                .expect("the status is loaded");
            discard_copy(&discard_all_ops(&status)).map_or(t(L10nKey::ScmDiscard), |c| c.verb)
        });
        vcx.simulate_prompt_answer(verb);

        assert!(
            pump_until(&mut vcx, 20, |_| {
                std::fs::read_to_string(root.join("a.txt")).is_ok_and(|s| s == "one\n")
                    && !root.join("new.txt").exists()
            }),
            "the discard never ran: the tracked edit and the untracked file are still there"
        );
        // "Staged changes are kept", as the prompt's detail promises: the
        // staged-only file is neither reverted nor unstaged.
        assert_eq!(
            std::fs::read(root.join("s.txt")).unwrap(),
            b"staged\n",
            "the staged edit's worktree bytes changed"
        );
        assert_eq!(
            index_bytes(&root, "s.txt"),
            b"staged\n",
            "the staged edit left the index"
        );
    }

    #[test]
    fn an_upstream_splits_on_its_first_slash_only() {
        assert_eq!(split_upstream("origin/main"), Some(("origin", "main")));
        // Branch names carry slashes; remote names cannot.
        assert_eq!(
            split_upstream("origin/feature/auth-retry"),
            Some(("origin", "feature/auth-retry"))
        );
        assert_eq!(split_upstream("main"), None);
        assert_eq!(split_upstream("/main"), None);
        assert_eq!(split_upstream("origin/"), None);
    }
}
