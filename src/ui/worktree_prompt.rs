use gpui::{
    AnyElement, Context, Div, Entity, PromptLevel, Subscription, Window, div, prelude::*, px, rems,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme as _, WindowExt as _, h_flex};

use crate::core::cli_agent::CLIAgent;
use crate::core::config::Config;
use crate::core::worktree::{NewWorktree, WorktreeDefaults, WorktreeRequest, setup};
use crate::ui::app::Tty59App;
use crate::ui::dialog::{self, Tone};
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::right_panel::META_MONO;
use crate::ui::text_elision::{ElidedLabel, Mode};

pub(crate) struct WorktreePrompt {
    host: crate::ui::host_ops::SharedHost,
    cwd: std::path::PathBuf,
    dir: std::path::PathBuf,
    name: Entity<InputState>,
    branch: Entity<InputState>,
    /// The person has edited Branch, clearing it included. Until then it
    /// follows Name. Only a `Change` on Branch sets it: the mirror writes with
    /// `set_value`, which emits none.
    branch_manual: bool,
    base: Entity<InputState>,
    task: Entity<InputState>,
    agents: Vec<CLIAgent>,
    /// Which of `agents` the first pane starts; `None` is a plain shell.
    agent: Option<usize>,
    /// The Start dropdown is open.
    agent_menu: bool,
    has_setup: bool,
    setup_hint: Option<&'static str>,
    busy: bool,
    _subs: Vec<Subscription>,
}

/// What the new worktree's first pane was asked to start.
struct Start {
    agent: Option<CLIAgent>,
    task: String,
}

/// A created worktree, and the setup script its checkout carries.
struct Created {
    wt: NewWorktree,
    setup: Option<(setup::Setup, Vec<(&'static str, String)>)>,
}

impl Tty59App {
    pub(crate) fn open_worktree_prompt(
        &mut self,
        host: crate::ui::host_ops::SharedHost,
        cwd: std::path::PathBuf,
        defaults: WorktreeDefaults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // All three open on a suggestion, which is a value to accept or type
        // over — not a prefix. `default_value` parked the caret in front of it,
        // so naming a worktree "login" over the suggested "feature" produced
        // "loginfeature".
        let prefill = crate::ui::prefill::filled_box;
        let name = prefill(defaults.name.clone(), window, cx);
        let branch = prefill(defaults.name, window, cx);
        let base = prefill(defaults.base, window, cx);
        let task = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::WorktreePromptTaskPlaceholder))
        });
        name.update(cx, |state, cx| state.focus(window, cx));
        let mut subs: Vec<Subscription> = [&name, &branch, &base, &task]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this, _, ev: &InputEvent, window, cx| match ev {
                        InputEvent::PressEnter { .. } => this.submit_worktree_prompt(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                )
            })
            .collect();
        subs.push(
            cx.subscribe_in(&name, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.mirror_name_into_branch(window, cx);
                }
            }),
        );
        subs.push(
            cx.subscribe_in(&branch, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.note_manual_branch_edit(window, cx);
                }
            }),
        );
        let agents = self.offered_agents(cx);
        // A worktree is usually for a task, so it opens on the agent last
        // used; Shell is one pick away.
        let agent =
            crate::ui::agent_launch::most_recent(&agents, &cx.global::<Config>().agent_frecency)
                .and_then(|a| agents.iter().position(|x| *x == a));
        self.worktree_prompt = Some(WorktreePrompt {
            host,
            cwd,
            dir: defaults.dir,
            name,
            branch,
            branch_manual: false,
            base,
            task,
            agents,
            agent,
            agent_menu: false,
            has_setup: defaults.has_setup,
            setup_hint: defaults.setup_hint,
            busy: false,
            _subs: subs,
        });
        cx.notify();
    }

    /// Branch follows Name until the person edits Branch. This runs in the
    /// Name's `Change` handler, never in render, and writes with `set_value`,
    /// which emits no `Change`: the write is not taken for a Branch edit and
    /// cannot call back into this.
    fn mirror_name_into_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.worktree_prompt.as_ref() else {
            return;
        };
        let name = p.name.read(cx).value().to_string();
        if !p.branch_manual {
            // Writing resets the caret, so leave an already equal box alone.
            if p.branch.read(cx).value().to_string() != name {
                p.branch
                    .update(cx, |branch, cx| branch.set_value(name, window, cx));
            }
            return;
        }
        // An emptied Branch is submitted as the name: say so in the box.
        if p.branch.read(cx).value().is_empty() {
            let hint = name.trim().to_string();
            p.branch
                .update(cx, |branch, cx| branch.set_placeholder(hint, window, cx));
        }
    }

    /// Any `Change` on Branch is the person's, since the mirror emits none.
    fn note_manual_branch_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.worktree_prompt.as_mut() else {
            return;
        };
        p.branch_manual = true;
        let hint = if p.branch.read(cx).value().is_empty() {
            p.name.read(cx).value().trim().to_string()
        } else {
            String::new()
        };
        p.branch
            .update(cx, |branch, cx| branch.set_placeholder(hint, window, cx));
    }

    fn cancel_worktree_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.worktree_prompt.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn submit_worktree_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.worktree_prompt.as_ref() else {
            return;
        };
        if p.busy {
            return;
        }
        let name = p.name.read(cx).value().trim().to_string();
        let branch = p.branch.read(cx).value().trim().to_string();
        let base = p.base.read(cx).value().trim().to_string();
        let (name, branch) = match (name.is_empty(), branch.is_empty()) {
            (true, true) => {
                window.push_notification(t(L10nKey::WorktreePromptNeedsName), cx);
                return;
            }
            (true, false) => (branch.clone(), branch),
            (false, true) => (name.clone(), name),
            (false, false) => (name, branch),
        };
        let req = WorktreeRequest {
            name,
            branch,
            base: if base.is_empty() {
                "HEAD".to_string()
            } else {
                base
            },
        };
        let start = Start {
            agent: p.agent.map(|i| p.agents[i]),
            task: p.task.read(cx).value().to_string(),
        };
        let p = self.worktree_prompt.as_mut().expect("checked above");
        p.busy = true;
        let cwd = p.cwd.clone();
        let host = p.host.clone();
        let host_id = host.id();
        cx.notify();
        crate::ui::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let wt = crate::core::worktree::create(h, &cwd, &req)?;
                // The script is a POSIX executable run through `env`; a
                // Windows host has neither.
                let setup = (h.separator() == '/')
                    .then(|| setup::find(h, &wt.path))
                    .flatten()
                    .zip(setup::env(h, &wt.main_root, &wt.path));
                Ok::<_, String>(Created { wt, setup })
            },
            move |this, result, window, cx| match result {
                Ok(created) => {
                    this.worktree_prompt = None;
                    this.start_worktree(host_id, created, start, window, cx);
                }
                Err(e) => {
                    if let Some(p) = this.worktree_prompt.as_mut() {
                        p.busy = false;
                    }
                    window.push_notification(
                        crate::ui::host_ops::failure(
                            t_fmt(L10nKey::AppNewWorktreeFailed, &[("error", &e.to_string())]),
                            &e,
                        ),
                        cx,
                    );
                    cx.notify();
                }
            },
        );
    }

    /// Open the worktree's tab, running its setup first — once the script's
    /// content has been approved for this repo.
    fn start_worktree(
        &mut self,
        host: HostId,
        created: Created,
        start: Start,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Created { wt, setup } = created;
        if !wt.carried.skipped.is_empty() {
            let paths = wt
                .carried
                .skipped
                .iter()
                .map(|(p, why)| format!("{p} ({why})"))
                .collect::<Vec<_>>()
                .join(", ");
            window.push_notification(
                t_fmt(L10nKey::AppWorktreeNotCarried, &[("paths", &paths)]),
                cx,
            );
        }
        let agent = start
            .agent
            .map(|a| setup::agent_line(a, &start.task, &cx.global::<Config>().agent_launch));
        let Some((script, env)) = setup else {
            let line = setup::launch_line(&wt.path, None, agent);
            self.open_worktree_tab(wt, line, window, cx);
            return;
        };
        let key = setup::trust_key(host, &wt.main_root);
        if cx.global::<Config>().worktree_setup_trust.get(&key) == Some(&script.digest) {
            let line = setup::launch_line(&wt.path, Some((&script.script, &env)), agent);
            self.open_worktree_tab(wt, line, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            t(L10nKey::AppWorktreeSetupTitle),
            Some(&t_fmt(
                L10nKey::AppWorktreeSetupDetail,
                &[("path", &script.script.display().to_string())],
            )),
            &crate::ui::confirm_answers(
                t(L10nKey::AppWorktreeSetupRun),
                t(L10nKey::AppWorktreeSetupSkip),
            ),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let run = matches!(answer.await, Ok(0));
            let _ = this.update_in(cx, |this, window, cx| {
                if run {
                    this.update_config(cx, |cfg| {
                        cfg.worktree_setup_trust.insert(key, script.digest.clone());
                    });
                }
                let setup = run.then_some((script.script.as_path(), env.as_slice()));
                let line = setup::launch_line(&wt.path, setup, agent);
                this.open_worktree_tab(wt, line, window, cx);
            });
        })
        .detach();
    }

    fn close_worktree_start_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.worktree_prompt.as_mut().filter(|p| p.agent_menu) {
            p.agent_menu = false;
            cx.notify();
        }
    }

    /// The Start dropdown: Shell, then every agent this machine offers.
    fn render_worktree_start(&self, p: &WorktreePrompt, cx: &mut Context<Self>) -> Div {
        use crate::ui::settings::kit;
        let tk = kit::Tk::of(cx);
        let current = p.agent.map_or(t(L10nKey::WorktreePromptShell), |i| {
            p.agents[i].display_name()
        });
        let trigger = dialog::select_well("worktree-start", current, cx)
            // Not a click on the form, which closes the menu this reopens.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.worktree_prompt.as_mut() {
                    p.agent_menu = !p.agent_menu;
                    cx.notify();
                }
            }));
        let menu = p.agent_menu.then(|| {
            let choices = std::iter::once((None, t(L10nKey::WorktreePromptShell))).chain(
                p.agents
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (Some(i), a.display_name())),
            );
            let mut panel = kit::menu_panel(&tk).min_w(px(220.));
            for (row, (choice, label)) in choices.enumerate() {
                panel = panel.child(
                    kit::menu_row(
                        gpui::ElementId::NamedInteger("worktree-start-item".into(), row as u64),
                        false,
                        &tk,
                    )
                    .child(kit::check_mark(choice == p.agent, &tk))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.worktree_prompt.as_mut() {
                            p.agent = choice;
                            p.agent_menu = false;
                            cx.notify();
                        }
                    })),
                );
            }
            // `popover_below` measures from a 26px settings trigger.
            kit::popover_below(false, dialog::FIELD_H - 26. + 4., panel)
        });
        dialog::labelled_control(
            t(L10nKey::WorktreePromptAgent),
            div().relative().child(trigger).children(menu),
            cx,
        )
    }

    pub(crate) fn render_worktree_prompt_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.worktree_prompt.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let name_now = p.name.read(cx).value().trim().to_string();
        let branch_now = p.branch.read(cx).value().trim().to_string();
        // Submitting falls back from one field to the other, so either alone
        // is enough; only both blank has nothing to name a worktree after.
        // Offering Create there is offering a click that can only fail.
        let nothing_to_name = name_now.is_empty() && branch_now.is_empty();
        // Preview what submitting would actually make, which is the same
        // fallback: with only a branch typed, the worktree takes its name, and
        // showing "…" there described a path that would never be created.
        let effective = match name_now.is_empty() {
            true => branch_now.as_str(),
            false => name_now.as_str(),
        };
        let preview = p
            .dir
            .join(if effective.is_empty() {
                "…"
            } else {
                effective
            })
            .display()
            .to_string();
        let preview = crate::ui::path_display::readable_path(&preview).into_owned();
        let mono = cx.theme().mono_font_family.clone();
        let meta_style = |line: Div| {
            line.text_size(rems(META_MONO))
                .font_family(mono.clone())
                .text_color(muted)
        };
        let meta = |text: String| meta_style(div().truncate()).child(text);
        // The name is what was just typed and the only part that tells two
        // worktrees apart, so the front of the path gives way to the width the
        // field's column actually has. Same mono/muted face as the other notes.
        let preview = meta_style(h_flex().w_full().min_w_0().overflow_hidden()).child(
            div().flex_1().min_w_0().child(ElidedLabel::new(
                "worktree-preview",
                preview,
                Mode::Path,
            )),
        );
        let setup_note = match (p.has_setup, p.setup_hint) {
            (true, _) => Some(t(L10nKey::WorktreePromptSetup).to_string()),
            (false, Some(command)) => Some(t_fmt(
                L10nKey::WorktreePromptSetupHint,
                &[("command", command)],
            )),
            (false, None) => None,
        };
        // Only an agent that can open on a first message gets the Task box;
        // for the rest it would be a field that does nothing.
        let takes_task = p
            .agent
            .is_some_and(|i| p.agents[i].prompt_args("").is_some());
        let start = (!p.agents.is_empty()).then(|| self.render_worktree_start(p, cx));

        let rungs = dialog::popover_rungs(cx);
        let card = dialog::card(
            "worktree-prompt-card",
            t(L10nKey::WorktreePromptTitle),
            440.,
            cx,
        )
        .child(dialog::header(t(L10nKey::WorktreePromptTitle), cx))
        .child(
            dialog::body()
                // A click anywhere else in the form closes the Start menu.
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                        this.close_worktree_start_menu(cx)
                    }),
                )
                // The path preview hangs off the Name field it follows,
                // closer to it than the next field is.
                .child(
                    dialog::labelled(t(L10nKey::WorktreePromptName), Input::new(&p.name), cx)
                        .child(preview),
                )
                .child(dialog::labelled(
                    t(L10nKey::WorktreePromptBranch),
                    Input::new(&p.branch),
                    cx,
                ))
                .child(
                    dialog::labelled(t(L10nKey::WorktreePromptBase), Input::new(&p.base), cx)
                        .children(setup_note.map(meta)),
                )
                .children(start)
                .when(takes_task, |body| {
                    body.child(dialog::labelled(
                        t(L10nKey::WorktreePromptTask),
                        Input::new(&p.task),
                        cx,
                    ))
                }),
        )
        .child(
            dialog::footer(cx)
                .child(dialog::button(
                    "worktree-cancel",
                    t(L10nKey::Cancel),
                    Tone::Secondary,
                    true,
                    rungs,
                    cx,
                    cx.listener(|this, _, window, cx| this.cancel_worktree_prompt(window, cx)),
                ))
                .child(dialog::button(
                    "worktree-create",
                    if p.busy {
                        t(L10nKey::WorktreePromptCreating)
                    } else {
                        t(L10nKey::WorktreePromptCreate)
                    },
                    Tone::Primary,
                    !(p.busy || nothing_to_name),
                    rungs,
                    cx,
                    cx.listener(|this, _, window, cx| this.submit_worktree_prompt(window, cx)),
                )),
        );

        Some(
            div()
                .absolute()
                .inset_0()
                // The backdrop already swallowed every click in the window; it
                // just did not look like it did. A scrim says the app is
                // waiting, and clicking it backs out — the same gesture the
                // palette and the switcher already answer to.
                .bg(crate::ui::presets::scrim_fill(cx))
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    if ev.keystroke.key != "escape" {
                        return;
                    }
                    // Escape backs out one level: an open menu first.
                    if this.worktree_prompt.as_ref().is_some_and(|p| p.agent_menu) {
                        this.close_worktree_start_menu(cx);
                    } else {
                        this.cancel_worktree_prompt(window, cx);
                    }
                }))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        this.cancel_worktree_prompt(window, cx)
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
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use gpui::{Entity, TestAppContext, VisualTestContext};
    use tty59_core::host::{ContentLimits, ContentQuery, ContentResults, ShellInventory};

    use super::*;
    use crate::core::config::Config;
    use crate::ui::app::test_window::harness;
    use crate::ui::host_ops::{Entry, Host, HostId, Meta, Output, SearchHit, SharedHost, WatchSub};
    use crate::ui::text_elision::observe;

    /// The local host with one difference: `git worktree add` is recorded and
    /// refused, so a submit that reaches `create` never makes a worktree or a
    /// terminal. Every other call, `git rev-parse` included, is the real one
    /// against the throwaway repository the test owns.
    struct RecordingGit {
        inner: SharedHost,
        adds: Arc<Mutex<Vec<Vec<String>>>>,
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
        fn git(&self, cwd: &std::path::Path, args: &[&str]) -> std::io::Result<Output> {
            if args.starts_with(&["worktree", "add"]) {
                self.adds
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|a| a.to_string()).collect());
                return Ok(Output {
                    status: Some(1),
                    stdout: Vec::new(),
                    stderr: b"refused by the test host".to_vec(),
                });
            }
            self.inner.git(cwd, args)
        }
        fn shells(&self) -> std::io::Result<ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
    }

    struct Form {
        app: Entity<Tty59App>,
        vcx: VisualTestContext,
        adds: Arc<Mutex<Vec<Vec<String>>>>,
        /// The repository's root in the plain local spelling every platform's
        /// `git` and `Path` agree on.
        root: PathBuf,
        _repo: tempfile::TempDir,
    }

    const DEFAULT_NAME: &str = "gold-crane";

    fn defaults(name: &str, dir: PathBuf) -> WorktreeDefaults {
        WorktreeDefaults {
            name: name.to_string(),
            base: "main".to_string(),
            dir,
            has_setup: false,
            setup_hint: None,
        }
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// One `git` child in `root`; a failing setup step says everything git
    /// said, on every platform.
    fn git_fixture(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}\nstdout: {}\nstderr: {}",
            root.display(),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// The initialized repository every `open` copies, built once per test
    /// process: `git init` is a child process per test otherwise, and a copy
    /// of the finished tree is far cheaper. The two settings that make a
    /// fixture the same on every platform are pinned before anything is
    /// written: no line-ending conversion (Git for Windows defaults to
    /// `autocrlf=true`) and long paths.
    fn seed_repo() -> &'static Path {
        static SEED: OnceLock<tempfile::TempDir> = OnceLock::new();
        SEED.get_or_init(|| {
            let seed = tempfile::Builder::new()
                .prefix("tty59-worktree-prompt-seed-")
                .tempdir()
                .expect("a temporary directory for the seed");
            git_fixture(seed.path(), &["init", "-q", "-b", "main"]);
            git_fixture(seed.path(), &["config", "core.longpaths", "true"]);
            git_fixture(seed.path(), &["config", "core.autocrlf", "false"]);
            seed
        })
        .path()
    }

    /// Copy the directory `from` into the existing directory `to`: files byte
    /// for byte, nothing else. A fresh repository holds no symbolic link, so
    /// meeting one is a bug in the fixture.
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to)
            .unwrap_or_else(|e| panic!("creating {} for the fixture copy: {e}", to.display()));
        let entries = std::fs::read_dir(from)
            .unwrap_or_else(|e| panic!("reading {} for the fixture copy: {e}", from.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("listing {}: {e}", from.display()));
            let (src, dst) = (entry.path(), to.join(entry.file_name()));
            let kind = entry
                .file_type()
                .unwrap_or_else(|e| panic!("typing {}: {e}", src.display()));
            if kind.is_dir() {
                copy_tree(&src, &dst);
            } else if kind.is_file() {
                std::fs::copy(&src, &dst).unwrap_or_else(|e| {
                    panic!("copying {} to {}: {e}", src.display(), dst.display())
                });
            } else {
                panic!(
                    "the fixture seed holds a non-regular entry: {}",
                    src.display()
                );
            }
        }
    }

    /// A window with the prompt open over a throwaway git repository: a fresh
    /// directory per test holding a copy of the seed.
    fn open(cx: &mut TestAppContext, name: &str) -> Form {
        let repo = tempfile::tempdir().expect("a temporary directory");
        copy_tree(seed_repo(), repo.path());
        // The plain local spelling: a `\\?\C:\…` root is read by git as a UNC
        // hostname, and `Path` does not compare it equal to the plain one.
        let root = tty59_core::core::path_spelling::local_spelling_buf(repo.path());
        let (app, mut vcx) = harness(cx);
        let adds = Arc::new(Mutex::new(Vec::new()));
        let host: SharedHost = Arc::new(RecordingGit {
            inner: tty59_core::host::local::LocalHost::new(),
            adds: Arc::clone(&adds),
        });
        let dir = root.join(".tty59").join("worktrees");
        let cwd = root.clone();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_worktree_prompt(host, cwd, defaults(name, dir), window, cx)
        });
        settle(&mut vcx);
        Form {
            app,
            vcx,
            adds,
            root,
            _repo: repo,
        }
    }

    impl Form {
        fn reopen(&mut self, name: &str) {
            let host: SharedHost = Arc::new(RecordingGit {
                inner: tty59_core::host::local::LocalHost::new(),
                adds: Arc::clone(&self.adds),
            });
            let cwd = self.root.clone();
            let dir = cwd.join(".tty59").join("worktrees");
            self.app.update_in(&mut self.vcx, |app, window, cx| {
                app.open_worktree_prompt(host, cwd, defaults(name, dir), window, cx)
            });
            settle(&mut self.vcx);
        }

        /// Close it the way Cancel and Escape do.
        fn cancel(&mut self) {
            self.app.update_in(&mut self.vcx, |app, window, cx| {
                app.cancel_worktree_prompt(window, cx)
            });
            settle(&mut self.vcx);
        }

        fn is_open(&mut self) -> bool {
            self.app
                .update_in(&mut self.vcx, |app, _, _| app.worktree_prompt.is_some())
        }

        fn focus(&mut self, field: fn(&WorktreePrompt) -> Entity<InputState>) {
            self.app.update_in(&mut self.vcx, |app, window, cx| {
                let input = field(app.worktree_prompt.as_ref().expect("the prompt is open"));
                input.update(cx, |state, cx| state.focus(window, cx));
            });
            settle(&mut self.vcx);
        }

        /// Real key events into the focused field: select what is there, then
        /// type over it.
        fn type_over(&mut self, text: &str) {
            self.vcx.simulate_keystrokes("secondary-a");
            self.vcx.simulate_input(text);
            settle(&mut self.vcx);
        }

        fn type_more(&mut self, text: &str) {
            self.vcx.simulate_input(text);
            settle(&mut self.vcx);
        }

        fn backspace(&mut self) {
            self.vcx.simulate_keystrokes("backspace");
            settle(&mut self.vcx);
        }

        fn clear(&mut self) {
            self.vcx.simulate_keystrokes("secondary-a");
            self.backspace();
        }

        /// Enter as a platform delivers it: a key down and a key up with no
        /// text. `simulate_keystrokes` would add the "\n" a simulated IME
        /// types for Enter, which a single-line Input never receives for real
        /// and panics on when it shapes it.
        fn press_enter(&mut self) {
            let keystroke = gpui::Keystroke::parse("enter").expect("enter parses");
            assert!(keystroke.key_char.is_none(), "Enter carries no text");
            self.vcx.simulate_event(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            self.vcx.simulate_event(gpui::KeyUpEvent { keystroke });
            settle(&mut self.vcx);
        }

        /// What the Name and Branch boxes show.
        fn shown(&mut self) -> (String, String) {
            self.app.update_in(&mut self.vcx, |app, _, cx| {
                let p = app.worktree_prompt.as_ref().expect("the prompt is open");
                (
                    p.name.read(cx).value().to_string(),
                    p.branch.read(cx).value().to_string(),
                )
            })
        }

        fn busy(&mut self) -> bool {
            self.app.update_in(&mut self.vcx, |app, _, _| {
                app.worktree_prompt.as_ref().is_some_and(|p| p.busy)
            })
        }

        /// Wait for `n` refused `git worktree add` calls, which is what a submit
        /// that reached `create` leaves behind.
        fn wait_for_adds(&mut self, n: usize) -> Vec<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                self.vcx.run_until_parked();
                let seen = self.adds.lock().unwrap().clone();
                if seen.len() >= n {
                    return seen;
                }
                assert!(
                    Instant::now() < deadline,
                    "the submit never reached `git worktree add`"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Wait for the refused create to land and give the form back.
        fn wait_until_idle(&mut self) {
            let deadline = Instant::now() + Duration::from_secs(30);
            while self.busy() {
                self.vcx.run_until_parked();
                assert!(Instant::now() < deadline, "the form stayed busy");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// No submit gets as far as `create`: pump for a while and look.
        fn assert_no_add(&mut self, why: &str) {
            for _ in 0..10 {
                self.vcx.run_until_parked();
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(self.adds.lock().unwrap().is_empty(), "{why}");
        }
    }

    fn name_box(p: &WorktreePrompt) -> Entity<InputState> {
        p.name.clone()
    }
    fn branch_box(p: &WorktreePrompt) -> Entity<InputState> {
        p.branch.clone()
    }

    /// The branch and the worktree directory a refused `worktree add` was
    /// asked for: `-b <branch> <path> <base>`.
    fn requested(add: &[String]) -> (String, String, String) {
        let at = add.iter().position(|a| a == "-b").expect("a new branch");
        let path = PathBuf::from(&add[at + 2]);
        (
            add[at + 1].clone(),
            path.file_name().unwrap().to_string_lossy().into_owned(),
            add[at + 3].clone(),
        )
    }

    #[gpui::test]
    fn the_prompt_opens_with_the_suggestion_in_both_boxes(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        assert_eq!(
            form.shown(),
            (DEFAULT_NAME.to_string(), DEFAULT_NAME.to_string())
        );
    }

    /// F9(b): the branch follows the name, through real keystrokes and every
    /// edit of it, not only the first.
    #[gpui::test]
    fn branch_follows_every_edit_of_the_name(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        assert_eq!(form.shown(), ("alpha".into(), "alpha".into()));
        form.type_more("x");
        assert_eq!(form.shown(), ("alphax".into(), "alphax".into()));
        form.backspace();
        assert_eq!(form.shown(), ("alpha".into(), "alpha".into()));
        form.type_over("beta");
        assert_eq!(form.shown(), ("beta".into(), "beta".into()));
    }

    /// Emptying the Name is a Name edit like another: the branch follows it
    /// down, and typing a name again brings it back. Nothing the form wrote
    /// itself counts as the person having edited the branch.
    #[gpui::test]
    fn clearing_the_name_does_not_stop_the_branch_following_it(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        form.clear();
        assert_eq!(form.shown(), (String::new(), String::new()));
        form.type_more("gamma");
        assert_eq!(form.shown(), ("gamma".into(), "gamma".into()));
    }

    /// A branch typed by hand is the person's, whatever the name does next.
    /// Guard: the old form never overwrote it either.
    #[gpui::test]
    fn a_branch_typed_by_hand_is_left_alone(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(branch_box);
        form.type_over("feat");
        form.focus(name_box);
        form.type_over("alpha");
        form.type_more("x");
        assert_eq!(form.shown(), ("alphax".into(), "feat".into()));
    }

    /// The same, once the name has been mirrored: the first manual edit is the
    /// one that ends the mirroring, and it ends it for good.
    #[gpui::test]
    fn the_first_manual_branch_edit_ends_the_mirroring(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        assert_eq!(form.shown(), ("alpha".into(), "alpha".into()));
        form.focus(branch_box);
        form.type_more("2");
        assert_eq!(form.shown(), ("alpha".into(), "alpha2".into()));
        form.focus(name_box);
        form.type_over("beta");
        form.type_more("x");
        assert_eq!(form.shown(), ("betax".into(), "alpha2".into()));
    }

    /// Clearing the branch counts as editing it: it stays empty, and the
    /// submit fallback (below) is what names it.
    #[gpui::test]
    fn a_cleared_branch_stays_cleared_as_the_name_changes(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        form.focus(branch_box);
        form.clear();
        assert_eq!(form.shown(), ("alpha".into(), String::new()));
        form.focus(name_box);
        form.type_over("beta");
        form.type_more("x");
        assert_eq!(form.shown(), ("betax".into(), String::new()));
    }

    /// Guard: a branch cleared before the name was ever touched stays cleared.
    #[gpui::test]
    fn a_branch_cleared_first_stays_cleared(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(branch_box);
        form.clear();
        form.focus(name_box);
        form.type_over("alpha");
        assert_eq!(form.shown(), ("alpha".into(), String::new()));
    }

    /// Opening the prompt again starts the mirroring over, whatever the last
    /// one's branch was doing.
    #[gpui::test]
    fn reopening_starts_the_mirroring_over(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(branch_box);
        form.type_over("feat");
        form.cancel();
        assert!(!form.is_open(), "Cancel closes the prompt");

        form.reopen("silver-fox");
        assert_eq!(
            form.shown(),
            ("silver-fox".to_string(), "silver-fox".to_string())
        );
        form.focus(name_box);
        form.type_over("delta");
        assert_eq!(form.shown(), ("delta".into(), "delta".into()));
    }

    /// Guards, true before and after: with neither box filled there is nothing
    /// to name a worktree after, so Enter makes nothing and leaves the form
    /// free.
    #[gpui::test]
    fn enter_with_both_boxes_empty_makes_nothing(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.clear();
        form.focus(branch_box);
        form.clear();
        assert_eq!(form.shown(), (String::new(), String::new()));
        form.press_enter();
        form.assert_no_add("an empty form reached `git worktree add`");
        assert!(form.is_open());
        assert!(!form.busy());
    }

    /// Guard: a submit in flight is not repeated by another Enter.
    #[gpui::test]
    fn enter_while_creating_does_nothing(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.app.update_in(&mut form.vcx, |app, _, _| {
            app.worktree_prompt.as_mut().expect("open").busy = true;
        });
        form.focus(name_box);
        form.press_enter();
        form.assert_no_add("a busy form submitted again");
    }

    /// F9(b) end to end: the branch the box shows is the branch the submit
    /// asks git for, beside the directory the preview names.
    #[gpui::test]
    fn submit_asks_for_the_branch_the_form_shows(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        form.press_enter();
        let adds = form.wait_for_adds(1);
        assert_eq!(
            requested(&adds[0]),
            ("alpha".into(), "alpha".into(), "main".into()),
            "{:?}",
            adds[0]
        );
        form.wait_until_idle();
        assert!(form.is_open(), "a refused create leaves the form open");
    }

    /// Guard: a branch typed by hand and a name are submitted as shown.
    #[gpui::test]
    fn submit_keeps_a_branch_typed_by_hand(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        form.focus(branch_box);
        form.type_over("feat");
        form.press_enter();
        let adds = form.wait_for_adds(1);
        assert_eq!(
            requested(&adds[0]),
            ("feat".into(), "alpha".into(), "main".into()),
            "{:?}",
            adds[0]
        );
    }

    /// Guard: the fallback the preview and the button already promise. With
    /// the branch emptied by hand, the worktree's name is the branch.
    #[gpui::test]
    fn submit_names_an_emptied_branch_after_the_name(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(name_box);
        form.type_over("alpha");
        form.focus(branch_box);
        form.clear();
        form.press_enter();
        let adds = form.wait_for_adds(1);
        assert_eq!(
            requested(&adds[0]),
            ("alpha".into(), "alpha".into(), "main".into()),
            "{:?}",
            adds[0]
        );
    }

    /// Guard: and an emptied name the other way round.
    #[gpui::test]
    fn submit_names_an_emptied_name_after_the_branch(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.focus(branch_box);
        form.type_over("feat");
        form.focus(name_box);
        form.clear();
        form.press_enter();
        let adds = form.wait_for_adds(1);
        assert_eq!(
            requested(&adds[0]),
            ("feat".into(), "feat".into(), "main".into()),
            "{:?}",
            adds[0]
        );
    }

    fn set_ui_font_size(form: &mut Form, size: f32) {
        form.vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.ui_font_size = size;
            cx.set_global(cfg);
        });
        form.app.update_in(&mut form.vcx, |_, _, cx| cx.notify());
        settle(&mut form.vcx);
    }

    /// What the preview line painted, or `None` when no measured label drew it.
    fn painted_preview(form: &mut Form) -> Option<String> {
        observe::clear();
        form.app.update_in(&mut form.vcx, |_, _, cx| cx.notify());
        settle(&mut form.vcx);
        observe::label(&gpui::ElementId::from("worktree-preview")).map(|l| {
            assert!(
                l.painted,
                "the preview prepared a line and did not paint it"
            );
            l.text.to_string()
        })
    }

    /// Display fixtures: the directory's components, lexical and explicit, so
    /// the platform spelling is derived from them rather than baked in as a
    /// POSIX string.
    const DEEP_RU: &[&str] = &[
        "home",
        "разработчик",
        "Проекты",
        "очень-длинное-название-репозитория",
        ".tty59",
        "worktrees",
    ];
    const DEEP_ZH: &[&str] = &[
        "home",
        "开发者",
        "项目",
        "一个名字非常非常长的代码仓库目录",
        ".tty59",
        "worktrees",
    ];
    const SHORT_DIR: &[&str] = &["r", ".tty59", "worktrees"];

    /// A rooted directory built from explicit components with the platform's
    /// own separator. Not a `format!("/{..}")`: on Windows `PathBuf::from("/r")`
    /// joined to a name is the mixed `/r\name`, a path no shell shows.
    fn display_dir(parts: &[&str]) -> PathBuf {
        let mut dir = PathBuf::from(std::path::MAIN_SEPARATOR_STR);
        dir.extend(parts);
        dir
    }

    /// What the preview shows, before any elision, for `parts` and `name`:
    /// the components joined by the platform separator as a string. Built by
    /// hand, so the prompt's own `join` is not the oracle for itself.
    fn whole_path(parts: &[&str], name: &str) -> String {
        let sep = std::path::MAIN_SEPARATOR;
        let mut whole = String::new();
        for part in parts.iter().copied().chain([name]) {
            whole.push(sep);
            whole.push_str(part);
        }
        whole
    }

    /// F9(a): a preview wider than its field gives up its front, so the name
    /// that was just typed is the part still there. Russian and CJK, at the
    /// default size and at the largest.
    fn assert_tail_survives(cx: &mut TestAppContext, dir: &[&str], name: &str) {
        let mut form = open(cx, name);
        form.app.update_in(&mut form.vcx, |app, _, _| {
            app.worktree_prompt.as_mut().expect("open").dir = display_dir(dir);
        });
        for size in [16., 24.] {
            set_ui_font_size(&mut form, size);
            let full = whole_path(dir, name);
            let shown = painted_preview(&mut form)
                .unwrap_or_else(|| panic!("{name} at {size}: the preview is not a measured label"));
            assert_ne!(shown, full, "{name} at {size}: nothing was elided");
            assert!(
                shown.contains('…'),
                "{name} at {size}: no ellipsis in {shown:?}"
            );
            let chars: Vec<char> = name.chars().collect();
            let tail: String = chars[chars.len() - 6..].iter().collect();
            assert!(
                shown.ends_with(&tail),
                "{name} at {size}: the end of the path was cut: {shown:?}"
            );
        }
    }

    #[gpui::test]
    fn a_narrow_preview_keeps_the_end_of_a_russian_path(cx: &mut TestAppContext) {
        assert_tail_survives(cx, DEEP_RU, "исследование-нестабильных-тестов");
    }

    #[gpui::test]
    fn a_narrow_preview_keeps_the_end_of_a_cjk_path(cx: &mut TestAppContext) {
        assert_tail_survives(cx, DEEP_ZH, "调查持续集成中不稳定的测试失败原因");
    }

    /// The preview follows the name as it is typed, still ending in it, and
    /// shows the whole path when it fits.
    #[gpui::test]
    fn the_preview_ends_in_the_name_as_it_is_typed(cx: &mut TestAppContext) {
        let mut form = open(cx, DEFAULT_NAME);
        form.app.update_in(&mut form.vcx, |app, _, _| {
            app.worktree_prompt.as_mut().expect("open").dir = display_dir(SHORT_DIR);
        });
        form.focus(name_box);
        form.type_over("alpha");
        assert_eq!(
            painted_preview(&mut form).as_deref(),
            Some(whole_path(SHORT_DIR, "alpha").as_str()),
            "a path that fits is shown whole"
        );
        form.type_over("beta");
        assert_eq!(
            painted_preview(&mut form).as_deref(),
            Some(whole_path(SHORT_DIR, "beta").as_str())
        );
    }
}
