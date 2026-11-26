//! One question for every worktree a bulk close left behind.
//!
//! Closing tabs one at a time asks about each tab's worktree as it goes. A
//! bulk close (the sidebar selection's Close, Close Other Tabs, Close Tabs to
//! the Right) would ask the same question once per tab, so it gathers the
//! worktrees first and asks once, with a Keep toggle per worktree. Every row
//! starts kept: nothing is deleted unless the person says so.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, Context, Div, FocusHandle, FontWeight, Window, div, prelude::*, px, rems};
use gpui_component::checkbox::Checkbox;
use gpui_component::{ActiveTheme as _, Sizable as _, Size, WindowExt as _, h_flex, v_flex};

use crate::core::worktree::{ManagedWorktree, Removed};
use crate::ui::app::Tty59App;
use crate::ui::dialog::{self, Tone};
use crate::ui::host_ops::{HostId, HostOps, SharedHost};
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::right_panel::{META_MONO, TAB_TEXT, TEXT};
use crate::ui::text_elision::{ElidedLabel, Mode};

/// The tallest the row list grows before it scrolls.
const ROWS_MAX_H: f32 = 320.;

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

/// What the removal jobs have answered so far, shared by the one job per host
/// until the last of them lands.
#[derive(Default)]
struct Outcome {
    /// Hosts whose job has not answered yet.
    waiting: usize,
    removed: usize,
    /// Branches `git branch -d` refused, whose commits nothing else has.
    branches_kept: Vec<String>,
    /// One `WorktreeBatchFailed` line per worktree that could not go.
    failures: Vec<String>,
}

/// The word on the remove button: singular at exactly one, the count otherwise
/// (zero included, when the button is disabled).
fn remove_button_label(unkept: usize) -> String {
    if unkept == 1 {
        t(L10nKey::WorktreeBatchRemoveButtonOne).to_string()
    } else {
        let count = unkept.to_string();
        t_fmt(L10nKey::WorktreeBatchRemoveButton, &[("count", &count)])
    }
}

/// Everything the removal jobs did, as the one notification they earn: the
/// count, then the branches kept, then one line per failure.
fn removed_notification(out: &Outcome) -> String {
    let mut lines = Vec::with_capacity(out.failures.len() + 2);
    lines.push(if out.removed == 1 {
        t(L10nKey::WorktreeBatchRemovedOne).to_string()
    } else {
        let count = out.removed.to_string();
        t_fmt(L10nKey::WorktreeBatchRemoved, &[("count", &count)])
    });
    if !out.branches_kept.is_empty() {
        let branches = out.branches_kept.join(", ");
        lines.push(t_fmt(
            L10nKey::WorktreeBatchBranchesKept,
            &[("branches", &branches)],
        ));
    }
    lines.extend(out.failures.iter().cloned());
    lines.join("\n")
}

// The notifications pushed in a test, in order, so a test can read the one
// line the removal jobs answer with (test builds only).
#[cfg(test)]
thread_local! {
    static PUSHED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn note_pushed(text: &str) {
    PUSHED.with(|pushed| pushed.borrow_mut().push(text.to_string()));
}

#[cfg(test)]
fn pushed_notifications() -> Vec<String> {
    PUSHED.with(|pushed| pushed.borrow().clone())
}

#[cfg(test)]
fn forget_pushed() {
    PUSHED.with(|pushed| pushed.borrow_mut().clear());
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

    /// Keep or remove one row. Out of range is a click on a row that is gone.
    pub(crate) fn set_worktree_batch_keep(
        &mut self,
        row: usize,
        keep: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(batch) = self.worktree_batch.as_mut() else {
            return;
        };
        let Some(entry) = batch.rows.get_mut(row) else {
            return;
        };
        if entry.keep != keep {
            entry.keep = keep;
            cx.notify();
        }
    }

    /// The Keep all / Remove all shortcuts: every row flips at once.
    pub(crate) fn set_worktree_batch_keep_all(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(batch) = self.worktree_batch.as_mut() else {
            return;
        };
        let mut changed = false;
        for row in &mut batch.rows {
            if row.keep != keep {
                row.keep = keep;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Keep All, and Escape: the dialog goes and nothing is removed.
    pub(crate) fn cancel_worktree_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktree_batch.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Remove every unkept row, in one background job per host it lives on, and
    /// answer once when the last job lands.
    pub(crate) fn confirm_worktree_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = self.worktree_batch.take() else {
            return;
        };
        self.focus_active(window, cx);
        // Group by host: one job per host, since each round trip is the cost
        // this dialog exists to pay once. Order is the first row's, so the
        // notification reads in the order the person saw the rows.
        let mut groups: Vec<(HostId, SharedHost, Vec<ManagedWorktree>)> = Vec::new();
        for row in batch.rows.into_iter().filter(|row| !row.keep) {
            let id = row.host.id();
            match groups.iter_mut().find(|(gid, _, _)| *gid == id) {
                Some((_, _, wts)) => wts.push(row.wt),
                None => groups.push((id, row.host, vec![row.wt])),
            }
        }
        if groups.is_empty() {
            cx.notify();
            return;
        }
        let outcome = Rc::new(RefCell::new(Outcome {
            waiting: groups.len(),
            ..Outcome::default()
        }));
        for (_id, host, wts) in groups {
            let shared = Rc::clone(&outcome);
            HostOps::run_in(
                host,
                window,
                cx,
                move |h| {
                    let mut answered = Vec::with_capacity(wts.len());
                    for wt in wts {
                        let branch = wt.branch.clone();
                        // Force only where there is something to lose: remove
                        // refuses a dirty worktree otherwise.
                        let result = crate::core::worktree::remove(h, &wt, wt.dirty);
                        answered.push((branch, result));
                    }
                    answered
                },
                move |_this, answered, window, cx| {
                    let notification = {
                        let mut outcome = shared.borrow_mut();
                        for (branch, result) in answered {
                            match result {
                                Ok(Removed { branch_kept, .. }) => {
                                    outcome.removed += 1;
                                    if branch_kept {
                                        outcome.branches_kept.push(branch);
                                    }
                                }
                                Err(e) => outcome.failures.push(t_fmt(
                                    L10nKey::WorktreeBatchFailed,
                                    &[("branch", &branch), ("error", &e)],
                                )),
                            }
                        }
                        outcome.waiting = outcome.waiting.saturating_sub(1);
                        (outcome.waiting == 0).then(|| removed_notification(&outcome))
                    };
                    if let Some(text) = notification {
                        #[cfg(test)]
                        note_pushed(&text);
                        window.push_notification(text, cx);
                    }
                    cx.notify();
                },
            );
        }
        cx.notify();
    }

    /// The dialog over the whole window, while one is open.
    pub(crate) fn render_worktree_batch_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let batch = self.worktree_batch.as_ref()?;
        let title = t_fmt(
            L10nKey::WorktreeBatchTitle,
            &[("count", &batch.rows.len().to_string())],
        );
        let unkept = batch.rows.iter().filter(|row| !row.keep).count();
        let rungs = dialog::popover_rungs(cx);
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let meta = |line: Div| {
            line.text_size(rems(META_MONO))
                .font_family(mono.clone())
                .text_color(muted)
        };

        let rows = batch
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let name = row
                    .wt
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let path =
                    crate::ui::path_display::readable_path(&row.wt.path.display().to_string())
                        .into_owned();
                let details = v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(rems(TAB_TEXT))
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.wt.branch.clone()),
                    )
                    .child(meta(div().truncate()).child(ElidedLabel::new(
                        ("worktree-batch-path", i),
                        path,
                        Mode::Path,
                    )))
                    .when(row.wt.dirty, |col| {
                        col.child(
                            meta(div())
                                .text_color(cx.theme().warning)
                                .child(t(L10nKey::WorktreeBatchDirty)),
                        )
                        .child(
                            meta(div())
                                .child(t_fmt(L10nKey::WorktreeBatchDirtyNote, &[("name", &name)])),
                        )
                    });
                let checkbox = Checkbox::new(("worktree-batch-keep", i))
                    .label(t(L10nKey::WorktreeBatchKeep))
                    .with_size(Size::Small)
                    .aria_label(t_fmt(
                        L10nKey::WorktreeBatchKeepToggle,
                        &[("branch", &row.wt.branch)],
                    ))
                    .checked(row.keep)
                    .on_click(cx.listener(move |this, keep: &bool, _window, cx| {
                        this.set_worktree_batch_keep(i, *keep, cx)
                    }));
                #[cfg(test)]
                let checkbox = checkbox.debug_selector(move || format!("worktree-batch-keep-{i}"));
                let row_el = h_flex()
                    .id(("worktree-batch-row", i))
                    .w_full()
                    .items_center()
                    .gap(px(12.))
                    .px(px(8.))
                    .py(px(7.))
                    .rounded(crate::ui::rounding::ROW_RADIUS)
                    .child(details)
                    .child(checkbox);
                #[cfg(test)]
                let row_el = row_el.debug_selector(move || format!("worktree-batch-row-{i}"));
                row_el.into_any_element()
            })
            .collect::<Vec<_>>();

        let keep_all_shortcut = dialog::button(
            "worktree-batch-keep-all-shortcut",
            t(L10nKey::WorktreeBatchKeepAllShortcut),
            Tone::Secondary,
            true,
            rungs,
            cx,
            cx.listener(|this, _, _, cx| this.set_worktree_batch_keep_all(true, cx)),
        );
        #[cfg(test)]
        let keep_all_shortcut =
            keep_all_shortcut.debug_selector(|| "worktree-batch-keep-all-shortcut".into());
        let remove_all_shortcut = dialog::button(
            "worktree-batch-remove-all-shortcut",
            t(L10nKey::WorktreeBatchRemoveAllShortcut),
            Tone::Secondary,
            true,
            rungs,
            cx,
            cx.listener(|this, _, _, cx| this.set_worktree_batch_keep_all(false, cx)),
        );
        #[cfg(test)]
        let remove_all_shortcut =
            remove_all_shortcut.debug_selector(|| "worktree-batch-remove-all-shortcut".into());
        let shortcuts = h_flex()
            .gap(px(6.))
            .child(keep_all_shortcut)
            .child(remove_all_shortcut);

        let keep_all = dialog::button(
            "worktree-batch-keep-all",
            t(L10nKey::WorktreeBatchKeepAllButton),
            Tone::Secondary,
            true,
            rungs,
            cx,
            cx.listener(|this, _, window, cx| this.cancel_worktree_batch(window, cx)),
        );
        #[cfg(test)]
        let keep_all = keep_all.debug_selector(|| "worktree-batch-keep-all".into());
        let remove = dialog::button(
            "worktree-batch-remove",
            remove_button_label(unkept),
            Tone::Danger,
            unkept > 0,
            rungs,
            cx,
            cx.listener(|this, _, window, cx| this.confirm_worktree_batch(window, cx)),
        );
        #[cfg(test)]
        let remove = remove.debug_selector(|| "worktree-batch-remove".into());

        let card = dialog::card(
            "worktree-batch-card",
            title.clone(),
            crate::ui::worktree_prompt::CARD_W,
            cx,
        )
        .child(dialog::header(title, cx))
        .child(
            dialog::body()
                .child(
                    div()
                        .whitespace_normal()
                        .text_size(rems(TEXT))
                        .text_color(muted)
                        .child(t(L10nKey::WorktreeBatchDetail)),
                )
                .child(shortcuts)
                .child(
                    div()
                        .id("worktree-batch-rows")
                        .w_full()
                        .max_h(px(ROWS_MAX_H))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(rows),
                ),
        )
        .child(dialog::footer(cx).child(keep_all).child(remove));

        Some(
            div()
                .id("worktree-batch-overlay")
                .track_focus(&batch.focus)
                .absolute()
                .inset_0()
                // Nothing under the scrim answers the pointer while the
                // question is up.
                .occlude()
                .bg(crate::ui::presets::scrim_fill(cx))
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    // Only the dialog's own focus answers: a keystroke that
                    // reaches here from a row's own control does not back the
                    // whole sheet out from under the person. Focus inside the
                    // overlay — a row's Keep box, say — still backs out.
                    if ev.keystroke.key != "escape"
                        || !this
                            .worktree_batch
                            .as_ref()
                            .is_some_and(|batch| batch.focus.contains_focused(window, cx))
                    {
                        return;
                    }
                    this.cancel_worktree_batch(window, cx);
                }))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        this.cancel_worktree_batch(window, cx)
                    }),
                )
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .pt(px(crate::ui::switcher::CARD_TOP))
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
    use tty59_core::host::{ContentLimits, ContentQuery, ContentResults, ShellInventory};

    use super::*;
    use crate::ui::app::test_window::harness;
    use crate::ui::host_ops::{Entry, Host, Meta, Output, SearchHit, WatchSub};

    /// One `git` the app ran: where, and with what.
    #[derive(Clone, Debug)]
    struct Call {
        cwd: PathBuf,
        args: Vec<String>,
    }

    /// The local host with two differences: every `git` is recorded, and
    /// `worktree remove` is refused, so a row's removal is countable and no
    /// checkout on this machine is ever deleted. The snapshot `remove` takes
    /// first gets a canned answer, so no test needs a real repository.
    struct RecordingGit {
        inner: SharedHost,
        calls: Arc<Mutex<Vec<Call>>>,
    }

    impl Host for RecordingGit {
        fn id(&self) -> HostId {
            self.inner.id()
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &std::path::Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(
            &self,
            dir: &std::path::Path,
            root: Option<&std::path::Path>,
        ) -> std::io::Result<Vec<Entry>> {
            self.inner.read_dir(dir, root)
        }
        fn stat(&self, p: &std::path::Path) -> std::io::Result<Meta> {
            self.inner.stat(p)
        }
        fn read_file(&self, p: &std::path::Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
            self.inner.read_file(p, max_bytes)
        }
        fn canonicalize(&self, p: &std::path::Path) -> std::io::Result<PathBuf> {
            self.inner.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> std::io::Result<Vec<SearchHit>> {
            self.inner
                .search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &ContentQuery,
            limits: &ContentLimits,
        ) -> std::io::Result<ContentResults> {
            self.inner.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &std::path::Path, bytes: &[u8]) -> std::io::Result<Meta> {
            self.inner.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &std::path::Path) -> std::io::Result<()> {
            self.inner.create_file_new(p)
        }
        fn create_dir(&self, p: &std::path::Path, recursive: bool) -> std::io::Result<()> {
            self.inner.create_dir(p, recursive)
        }
        fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
            self.inner.rename(from, to)
        }
        fn remove(&self, p: &std::path::Path, recursive: bool) -> std::io::Result<()> {
            self.inner.remove(p, recursive)
        }
        fn repo_root(&self, p: &std::path::Path) -> std::io::Result<Option<PathBuf>> {
            self.inner.repo_root(p)
        }
        fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
            self.calls.lock().unwrap().push(Call {
                cwd: cwd.to_path_buf(),
                args: args.iter().map(|a| a.to_string()).collect(),
            });
            let ok = |stdout: Vec<u8>| Output {
                status: Some(0),
                stdout,
                stderr: Vec::new(),
            };
            // `worktree::remove` files the checkout's last state first; the
            // snapshot's calls get a canned answer, and the removal itself is
            // refused so no checkout on this machine is ever deleted.
            if args.starts_with(&["rev-parse", "HEAD"]) {
                Ok(ok(b"1111111111111111111111111111111111111111".to_vec()))
            } else if args.starts_with(&["add", "-A"]) {
                Ok(ok(Vec::new()))
            } else if args.starts_with(&["stash", "create"]) {
                Ok(ok(b"2222222222222222222222222222222222222222".to_vec()))
            } else if args.starts_with(&["update-ref"]) || args.starts_with(&["branch", "-d"]) {
                Ok(ok(Vec::new()))
            } else if args.starts_with(&["worktree", "remove"]) {
                Ok(Output {
                    status: Some(1),
                    stdout: Vec::new(),
                    stderr: b"refused by the test host".to_vec(),
                })
            } else {
                self.inner.git(cwd, args)
            }
        }
        fn shells(&self) -> std::io::Result<ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
    }

    /// A rooted directory built from explicit components with the platform's
    /// own separator, so the same fixture reads the same on Windows.
    fn dir_parts(parts: &[&str]) -> PathBuf {
        let mut dir = PathBuf::from(std::path::MAIN_SEPARATOR_STR);
        dir.extend(parts);
        dir
    }

    /// `debug_bounds` takes a `&'static str`; a test leaks the one it builds.
    fn sel(prefix: &str, i: usize) -> &'static str {
        Box::leak(format!("{prefix}-{i}").into_boxed_str())
    }

    fn worktree(name: &str, dirty: bool) -> ManagedWorktree {
        ManagedWorktree {
            path: dir_parts(&["repo", ".tty59", "worktrees", name]),
            branch: format!("tty59/{name}"),
            main_root: dir_parts(&["repo"]),
            dirty,
        }
    }

    struct Form {
        app: Entity<Tty59App>,
        vcx: VisualTestContext,
        calls: Arc<Mutex<Vec<Call>>>,
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// A window with the batch dialog open over `specs` (name, dirty).
    fn open(cx: &mut TestAppContext, specs: &[(&str, bool)]) -> Form {
        let (app, mut vcx) = harness(cx);
        forget_pushed();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let host: SharedHost = Arc::new(RecordingGit {
            inner: tty59_core::host::local::LocalHost::new(),
            calls: Arc::clone(&calls),
        });
        let rows = specs
            .iter()
            .map(|(name, dirty)| LeftWorktree {
                host: host.clone(),
                wt: worktree(name, *dirty),
                keep: true,
            })
            .collect();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_worktree_batch(rows, window, cx)
        });
        settle(&mut vcx);
        Form { app, vcx, calls }
    }

    impl Form {
        fn keeps(&mut self) -> Vec<bool> {
            self.app.update_in(&mut self.vcx, |app, _, _| {
                app.worktree_batch
                    .as_ref()
                    .expect("the dialog is open")
                    .rows
                    .iter()
                    .map(|row| row.keep)
                    .collect()
            })
        }

        fn is_open(&mut self) -> bool {
            self.app
                .update_in(&mut self.vcx, |app, _, _| app.worktree_batch.is_some())
        }

        /// A real click at the centre of the element `selector` names.
        fn click(&mut self, selector: &'static str) {
            let at = self
                .vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("no bounds for {selector}"))
                .center();
            self.vcx.simulate_click(at, Modifiers::none());
            settle(&mut self.vcx);
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        /// The `git worktree remove` calls the app made, in order.
        fn removes(&self) -> Vec<Vec<String>> {
            self.calls()
                .into_iter()
                .filter(|call| {
                    call.args
                        .starts_with(&["worktree".to_string(), "remove".to_string()])
                })
                .map(|call| call.args)
                .collect()
        }

        fn wait_for_removes(&mut self, n: usize) -> Vec<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                self.vcx.run_until_parked();
                let seen = self.removes();
                if seen.len() >= n {
                    return seen;
                }
                assert!(
                    Instant::now() < deadline,
                    "only {} `git worktree remove` calls ever ran",
                    seen.len()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Wait for the one notification the jobs answer with.
        fn wait_for_notification(&mut self) -> String {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                self.vcx.run_until_parked();
                if let Some(text) = pushed_notifications().into_iter().next() {
                    return text;
                }
                assert!(Instant::now() < deadline, "the removal jobs never answered");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// No job gets as far as a removal: pump for a while and look.
        fn assert_no_remove(&mut self, why: &str) {
            for _ in 0..10 {
                self.vcx.run_until_parked();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(self.removes().is_empty(), "{why}");
        }
    }

    /// F9(b): three worktrees, one row each, every one kept — and a click on the
    /// disabled Remove does nothing, the dialog included.
    #[gpui::test]
    fn three_rows_open_all_kept_with_remove_inert(cx: &mut TestAppContext) {
        let mut form = open(cx, &[("alpha", false), ("beta", false), ("gamma", false)]);
        assert_eq!(form.keeps(), vec![true, true, true]);
        for i in 0..3 {
            assert!(
                form.vcx
                    .debug_bounds(sel("worktree-batch-row", i))
                    .is_some(),
                "row {i} is not painted"
            );
        }
        assert!(
            form.vcx
                .debug_bounds(sel("worktree-batch-row", 3))
                .is_none(),
            "a fourth row was painted"
        );
        assert_eq!(remove_button_label(0), "Remove 0 Worktrees");
        form.click("worktree-batch-remove");
        assert!(form.is_open(), "a disabled Remove closed the dialog");
        form.assert_no_remove("a disabled Remove reached `git worktree remove`");
    }

    /// F9(c): unkeeping one row enables the footer and the button names the one
    /// worktree; clicking it removes exactly that one, without force.
    #[gpui::test]
    fn unkeeping_one_row_removes_exactly_one(cx: &mut TestAppContext) {
        let mut form = open(cx, &[("alpha", false), ("beta", false), ("gamma", false)]);
        form.click("worktree-batch-keep-1");
        assert_eq!(form.keeps(), vec![true, false, true]);
        assert_eq!(remove_button_label(1), "Remove 1 Worktree");

        form.click("worktree-batch-remove");
        assert!(!form.is_open(), "Confirm keeps the dialog open");
        let removes = form.wait_for_removes(1);
        assert_eq!(removes.len(), 1, "{removes:?}");
        let beta = worktree("beta", false).path.display().to_string();
        assert!(removes[0].contains(&beta), "{:?}", removes[0]);
        assert!(
            !removes[0].contains(&"--force".to_string()),
            "a clean worktree was removed with --force: {:?}",
            removes[0]
        );
    }

    /// F9(d): Remove all flips every row; Keep all flips them back.
    #[gpui::test]
    fn remove_all_then_keep_all_flip_every_row(cx: &mut TestAppContext) {
        let mut form = open(cx, &[("alpha", false), ("beta", false), ("gamma", true)]);
        form.click("worktree-batch-remove-all-shortcut");
        assert_eq!(form.keeps(), vec![false, false, false]);
        assert_eq!(remove_button_label(3), "Remove 3 Worktrees");
        form.click("worktree-batch-keep-all-shortcut");
        assert_eq!(form.keeps(), vec![true, true, true]);
        assert_eq!(remove_button_label(0), "Remove 0 Worktrees");
    }

    /// Escape backs out of a dialog where everything is marked for removal, and
    /// nothing is removed.
    #[gpui::test]
    fn escape_closes_and_removes_nothing(cx: &mut TestAppContext) {
        let mut form = open(cx, &[("alpha", false), ("beta", false), ("gamma", false)]);
        // A row's own control takes focus; Escape must still back out from
        // inside the sheet.
        form.click("worktree-batch-keep-1");
        form.click("worktree-batch-remove-all-shortcut");
        assert_eq!(form.keeps(), vec![false, false, false]);
        form.vcx.simulate_keystrokes("escape");
        settle(&mut form.vcx);
        assert!(!form.is_open(), "Escape left the dialog open");
        form.assert_no_remove("Escape reached `git worktree remove`");
        assert!(
            pushed_notifications().is_empty(),
            "Escape answered with a notification"
        );
    }

    /// F9(e): rows 0 and 2 are unkept, row 2 is dirty. Exactly two removals,
    /// `--force` only on the dirty one, row 1 never touched — and the one
    /// notification names each removal the test host refused.
    #[gpui::test]
    fn confirming_removes_only_the_unkept_rows_and_forces_only_dirty(cx: &mut TestAppContext) {
        let mut form = open(cx, &[("alpha", false), ("beta", false), ("gamma", true)]);
        form.click("worktree-batch-keep-0");
        form.click("worktree-batch-keep-2");
        assert_eq!(form.keeps(), vec![false, true, false]);

        form.click("worktree-batch-remove");
        assert!(!form.is_open(), "Confirm keeps the dialog open");
        let removes = form.wait_for_removes(2);
        assert_eq!(removes.len(), 2, "{removes:?}");

        let forced: Vec<&Vec<String>> = removes
            .iter()
            .filter(|args| args.contains(&"--force".to_string()))
            .collect();
        assert_eq!(forced.len(), 1, "forced removals: {forced:?}");
        let gamma = worktree("gamma", true).path.display().to_string();
        assert!(
            forced[0].contains(&gamma),
            "the force went to the wrong row: {:?}",
            forced[0]
        );
        let alpha = worktree("alpha", false).path.display().to_string();
        assert!(
            removes.iter().any(|args| args.contains(&alpha)),
            "the clean unkept row was not removed: {removes:?}"
        );

        let kept = worktree("beta", false);
        let beta = kept.path.display().to_string();
        assert!(
            !form
                .calls()
                .iter()
                .any(|call| call.cwd == kept.path || call.args.contains(&beta)),
            "the kept row was touched"
        );

        let text = form.wait_for_notification();
        assert_eq!(
            pushed_notifications().len(),
            1,
            "one notification, not one per row"
        );
        assert!(
            text.starts_with("Removed 0 worktrees"),
            "nothing survived the refusing host, so nothing was removed: {text:?}"
        );
        let failures: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("Could not remove"))
            .collect();
        assert_eq!(failures.len(), 2, "{text:?}");
        for branch in ["tty59/alpha", "tty59/gamma"] {
            assert!(
                failures.iter().any(|line| line.contains(branch)),
                "{branch} is missing from {text:?}"
            );
        }
        assert!(
            text.contains("refused by the test host"),
            "the error text is missing: {text:?}"
        );
    }

    #[test]
    fn the_remove_button_counts_worktrees() {
        assert_eq!(remove_button_label(0), "Remove 0 Worktrees");
        assert_eq!(
            remove_button_label(1),
            t(L10nKey::WorktreeBatchRemoveButtonOne)
        );
        assert_eq!(
            remove_button_label(2),
            t_fmt(L10nKey::WorktreeBatchRemoveButton, &[("count", "2")])
        );
    }

    #[test]
    fn the_one_notification_lists_kept_branches_and_every_failure() {
        let failed = |branch: &str| {
            t_fmt(
                L10nKey::WorktreeBatchFailed,
                &[("branch", branch), ("error", "refused")],
            )
        };
        let out = Outcome {
            waiting: 0,
            removed: 2,
            branches_kept: vec!["beta".into(), "gamma".into()],
            failures: vec![failed("alpha")],
        };
        let text = removed_notification(&out);
        let removed = t_fmt(L10nKey::WorktreeBatchRemoved, &[("count", "2")]);
        let kept = t_fmt(
            L10nKey::WorktreeBatchBranchesKept,
            &[("branches", "beta, gamma")],
        );
        let failed_alpha = failed("alpha");
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(removed.as_str()));
        assert_eq!(lines.next(), Some(kept.as_str()));
        assert_eq!(lines.next(), Some(failed_alpha.as_str()));
        assert_eq!(lines.next(), None);

        let one = Outcome {
            removed: 1,
            ..Outcome::default()
        };
        assert_eq!(
            removed_notification(&one),
            t(L10nKey::WorktreeBatchRemovedOne)
        );
    }
}
