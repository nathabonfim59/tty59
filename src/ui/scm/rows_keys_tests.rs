#![cfg_attr(not(unix), allow(dead_code, unused_imports))]
//! Keyboard access to the Changes list and the Info rows (panels F5), written
//! BEFORE the production change.
//!
//! These tests use only what the app has today: the real `ShowRightPanelChanges`
//! and `ShowRightPanelInfo` actions, real key dispatch, the existing row and
//! header selectors, the existing diff overlay and git results, the pane's PTY
//! stream and the platform prompt. The one thing they need from the future
//! production code is a *key context* on the element that holds the keyboard:
//!
//! * a Changes node (group header, tree directory or file row) that has the
//!   keyboard carries `ScmRows` with `row = <that element's own id>`, so
//!   `scm-group-Staged`, `scm-dir-Staged-<key>` or `scm-row-Staged-<path>`;
//! * an Info row that has the keyboard carries `InfoRow` with `key = Cwd`.
//!
//! Until that exists no context is mounted, so every assertion that reads it
//! fails by name ("no `ScmRows` key context is mounted at the focus"); nothing
//! skips. Tests that only guard what must NOT happen (a late callback stealing
//! focus, a clean repository freezing keys) pass on today's code by absence and
//! are labelled "guard": each one first proves its scenario reached the state it
//! guards, and each is paired with a positive test that fails today.
//!
//! The `control_*` tests pass today. They prove the fixture, the PTY wire, the
//! prompt and the pointer paths work, so that a red test below is red for its
//! own reason.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, Focusable as _, KeyDownEvent, Keystroke, MouseButton, PlatformInput,
    TestAppContext, VisualTestContext, Window,
};
use tty59_core::core::config::RightPanelTab;
use tty59_core::core::git::status::WorkingTreeStatus;

use super::rows_window_tests::{
    EDITED, STAGED, big_repo, draw, file, flat_rows, git, git_ok, header, mouse, only_group,
    painted, panel_on, row_budget, scratch, scroll_to, serial, status, viewport, write,
};
use crate::core::actions::{ShowRightPanelChanges, ShowRightPanelInfo};
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::ui::app::{Tty59App, render_probe, test_window};
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::scm::panel::{RowVerb, group_diff_source, in_group, verb_op};
use crate::ui::scm::state::{RepoKey, ScmGroup};

type Stream = crate::daemon::transport::Stream;

// ---------------------------------------------------------------- helpers --

/// The id of the Changes node that holds the keyboard, read from the live
/// dispatch tree: the `row` value of the `ScmRows` key context on the path to
/// the focused element. `None` when no Changes node has the keyboard.
fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| marker_in(window))
}

/// `marker`, from inside an app/window event, where there is a `Window` and no
/// context to update: lets a test look at the keyboard in the very event that
/// made a request, before any frame or pump can have run.
fn marker_in(window: &Window) -> Option<String> {
    window
        .context_stack()
        .iter()
        .find(|c| c.contains("ScmRows"))
        .and_then(|c| c.get("row"))
        .map(|v| v.to_string())
}

/// Read at this very instant, inside the event about to ask for the tab:
/// `(status cached, root lookup started or answered)` for `root` and the pane's
/// directory. The cache is asked by the repository's actual root and host, not
/// inferred from `scm.repo` being unset, and the lookup state is the panel's own
/// (`roots` answered, `root_lookups` in flight).
fn cache_state(
    app: &Tty59App,
    window: &mut Window,
    cx: &mut Context<Tty59App>,
    root: &Path,
) -> (bool, bool) {
    let cached = crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_some();
    let (host, cwd) = app
        .scm_pane_target(window, cx)
        .expect("the pane reported its directory");
    let key = (host.id(), cwd);
    let looked_up = app.scm.roots.contains_key(&key) || app.scm.root_lookups.contains(&key);
    (cached, looked_up)
}

/// The `key` value of the `InfoRow` key context on the focus path.
fn info_marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("InfoRow"))
            .and_then(|c| c.get("key"))
            .map(|v| v.to_string())
    })
}

/// What the pane's PTY link received as input: a short bounded read, every
/// frame that is not `Input` skipped (the pattern the panel tests use).
fn pane_input(pane: &mut Stream) -> Vec<u8> {
    pane.set_read_timeout(Some(Duration::from_millis(150)))
        .expect("the pane link takes a read timeout");
    let mut out = Vec::new();
    loop {
        match ClientMsg::read(pane) {
            Ok(ClientMsg::Input(bytes)) => out.extend_from_slice(&bytes),
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => panic!("pane link failed: {e}"),
        }
    }
    out
}

/// One key the way this platform's terminal takes it: text through the input
/// handler on macOS, a keystroke elsewhere (the panel tests' own rule).
#[cfg(target_os = "macos")]
fn type_key(vcx: &mut VisualTestContext, key: &str) {
    if key.len() == 1 {
        vcx.simulate_input(key);
    } else {
        vcx.simulate_keystrokes(key);
    }
}

#[cfg(not(target_os = "macos"))]
fn type_key(vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
}

/// Type `key` (a keystroke name) and say whether the pane's PTY received
/// `byte` within a bounded number of pumps. Drains what was pending first.
fn reaches_pane(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    pane: &mut Stream,
    key: &str,
    byte: u8,
) -> bool {
    let _ = pane_input(pane);
    type_key(vcx, key);
    let mut got = Vec::new();
    for _ in 0..8 {
        draw(app, vcx);
        got.extend(pane_input(pane));
        if got.contains(&byte) {
            return true;
        }
    }
    false
}

fn pump_until(
    vcx: &mut VisualTestContext,
    secs: u64,
    mut until: impl FnMut(&mut VisualTestContext) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        vcx.run_until_parked();
        if until(vcx) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Put the keyboard on the active terminal, the way the app does on its own.
fn focus_terminal(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
}

/// The real keyboard entry into the Changes tab.
fn enter(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(app, vcx);
    draw(app, vcx);
}

/// A key event with `is_held` set the way the platform sets it for a repeat.
fn key_event(vcx: &mut VisualTestContext, key: &str, held: bool) {
    let keystroke = Keystroke::parse(key).expect("a keystroke");
    vcx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: held,
                prefer_character_input: false,
            }),
            cx,
        );
    });
    vcx.run_until_parked();
}

fn press(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    draw(app, vcx);
}

/// Press `key` until the keyboard is on the node with id `target`. The first
/// look names the failure when no node holds the keyboard at all.
fn walk_to(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    key: &str,
    target: &str,
    max: usize,
) {
    for step in 0..=max {
        let now = marker(vcx);
        if now.as_deref() == Some(target) {
            return;
        }
        assert!(
            step > 0 || now.is_some(),
            "no `ScmRows` key context is mounted at the focus: no Changes node holds the keyboard"
        );
        if step == max {
            break;
        }
        press(app, vcx, key);
    }
    panic!(
        "pressing {key} {max} times never reached {target}; the keyboard is on {:?}",
        marker(vcx)
    );
}

/// Is `sel` drawn and inside the panel's scroll viewport?
fn visible(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, sel: &'static str) -> bool {
    let view = viewport(app, vcx);
    vcx.debug_bounds(sel).is_some_and(|b| {
        b.top().as_f32() >= view.top().as_f32() - 0.5
            && b.bottom().as_f32() <= view.bottom().as_f32() + 0.5
    })
}

fn path_of(row_id: &str) -> String {
    let rest = row_id.strip_prefix("scm-row-").expect("a file row id");
    rest.split_once('-').expect("group-path").1.to_string()
}

/// A repository like `big_repo`, with `file(0)` edited again after it was
/// staged, so the same path is in both the Staged and the Changes group.
fn keys_repo(name: &str) -> std::path::PathBuf {
    let root = big_repo(name);
    write(&root, &file(0), "line 0\nedited\nagain\n");
    root
}

fn rows_of(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    group: ScmGroup,
) -> Vec<&'static str> {
    let st = status(app, vcx);
    flat_rows(&st, group)
}

/// Ask for a fresh read until `done` holds for the status the panel has.
fn refresh_until(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    root: &Path,
    what: &str,
    done: impl Fn(&WorkingTreeStatus) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        app.update_in(vcx, |app, _, cx| {
            app.scm_invalidate(HostId::LOCAL, root, cx)
        });
        vcx.executor().advance_clock(Duration::from_millis(300));
        vcx.run_until_parked();
        let st = app.update_in(vcx, |app, _, cx| {
            let repo = app.scm.repo.clone()?;
            crate::terminal::git_data::status_of(cx, repo.host, &repo.root)
        });
        if st.as_deref().is_some_and(&done) {
            break;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
    draw(app, vcx);
    draw(app, vcx);
}

/// A window whose Changes status has NOT been read: the pane knows its
/// directory, the right panel is hidden, and nothing has asked git yet.
fn unloaded(cx: &mut TestAppContext, root: &Path) -> (Entity<Tty59App>, VisualTestContext, Stream) {
    crate::core::config::pin_test_config_dir();
    crate::ui::i18n::set_locale("en");
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    DaemonMsg::Cwd(root.to_path_buf())
        .encode(&mut pane)
        .expect("the pane's socket takes the cwd");
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = false;
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    let known = pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |app, window, cx| {
            cx.notify();
            app.scm_pane_target(window, cx).is_some()
        })
    });
    assert!(known, "the pane never reported its directory");
    focus_terminal(&app, &mut vcx);
    (app, vcx, pane)
}

/// Wait for the big fixture's status to land in the panel and draw it.
fn landed(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, root: &Path) {
    let ok = pump_until(vcx, 60, |vcx| {
        app.update_in(vcx, |app, _, cx| {
            cx.notify();
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.entries.len() > STAGED + EDITED)
            })
        })
    });
    assert!(ok, "the status never landed");
    test_window::quiesce(vcx, Some(root));
    draw(app, vcx);
}

/// Remove a fixture: a repository directory called `repo` lives in a scratch
/// directory of its own, anything else is the scratch directory itself.
fn clean_up(root: &Path) {
    let scratch_dir = match root.file_name().and_then(|n| n.to_str()) {
        Some("repo") => root.parent().unwrap_or(root),
        _ => root,
    };
    let _ = std::fs::remove_dir_all(scratch_dir);
}

fn tracked_edits(root: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(root)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

// --------------------------------------------------------------- controls --
// All of these pass today.

/// The PTY wire works: with the terminal focused a typed key reaches the pane.
#[cfg(unix)]
#[gpui::test]
fn control_a_key_typed_at_the_terminal_reaches_the_pane(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("ctl-pty");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    for (key, byte) in [("space", b' '), ("backspace", 0x7f), ("enter", b'\r')] {
        assert!(
            reaches_pane(&app, &mut vcx, &mut pane, key, byte),
            "{key} reaches the PTY while the terminal holds the keyboard"
        );
    }
    for key in ["down", "up"] {
        assert!(
            reaches_pane(&app, &mut vcx, &mut pane, key, 0x1b),
            "{key} reaches the PTY as an escape sequence"
        );
    }
    clean_up(&root);
}

/// The approval path works and is cancellable: the callee of every pointer
/// discard (`scm_op`) asks, Cancel keeps the file, the verb discards exactly it.
#[cfg(unix)]
#[gpui::test]
fn control_discard_asks_cancel_keeps_the_file_and_the_verb_discards_only_it(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = keys_repo("ctl-discard");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let st = status(&app, &mut vcx);
    let entry = st
        .entries
        .iter()
        .filter(|e| in_group(e, ScmGroup::Changes))
        .nth(5)
        .expect("a Changes row");
    let rel = entry.path.as_str().to_string();
    let op = verb_op(
        RowVerb::Discard,
        ScmGroup::Changes,
        vec![entry.path.clone()],
    )
    .expect("discard is an operation of the Changes group");
    let repo: RepoKey = app.update_in(&mut vcx, |app, _, _| app.scm.repo.clone().unwrap());
    let before = tracked_edits(&root);
    assert!(before.contains(&rel));

    let ask = |vcx: &mut VisualTestContext| {
        let (repo, op) = (repo.clone(), op.clone());
        app.update_in(vcx, |app, window, cx| app.scm_op(repo, op, window, cx));
        assert!(
            pump_until(vcx, 5, |vcx| vcx.has_pending_prompt()),
            "the discard never asked"
        );
    };
    ask(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    let _ = pump_until(&mut vcx, 2, |_| false);
    assert!(!vcx.has_pending_prompt());
    assert_eq!(tracked_edits(&root), before, "Cancel changed nothing");

    ask(&mut vcx);
    vcx.simulate_prompt_answer(t(L10nKey::ScmDiscard));
    assert!(
        pump_until(&mut vcx, 30, |_| !tracked_edits(&root).contains(&rel)),
        "the discard never ran"
    );
    assert_eq!(
        tracked_edits(&root).len(),
        before.len() - 1,
        "exactly that one file was discarded"
    );
    clean_up(&root);
}

/// The real action selects the Changes tab (today it does only that).
#[cfg(unix)]
#[gpui::test]
fn control_the_changes_action_shows_the_changes_tab(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("ctl-tab");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    draw(&app, &mut vcx);
    vcx.dispatch_action(ShowRightPanelChanges);
    draw(&app, &mut vcx);
    let shown = app.update_in(&mut vcx, |app, _, _| {
        (app.right_panel_visible, app.right_panel_tab)
    });
    assert_eq!(shown, (true, RightPanelTab::Scm));
    clean_up(&root);
}

/// A pointer click on a Changes header leaves the keyboard where it was, and
/// typing still reaches the pane. Must stay true after the change: the pointer
/// never moves focus into the list.
#[cfg(unix)]
#[gpui::test]
fn control_a_pointer_click_on_a_header_leaves_the_keyboard_on_the_terminal(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = keys_repo("ctl-click");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    let before = vcx.update(|window, cx| window.focused(cx));
    let at = vcx
        .debug_bounds(header(ScmGroup::Merge))
        .expect("the first header is drawn")
        .center();
    mouse(&mut vcx, MouseButton::Left, at);
    draw(&app, &mut vcx);
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), before);
    assert_eq!(marker(&mut vcx), None);
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}

/// The landing fixture works: from a hidden panel the real action shows the
/// tab, the status is NOT cached at that moment, and the rows draw after the
/// read lands.
#[cfg(unix)]
#[gpui::test]
fn control_an_uncached_status_loads_after_the_action_and_draws(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("ctl-load");
    let (app, mut vcx, _pane) = unloaded(cx, &root);
    let (cached, looked_up) = app.update_in(&mut vcx, |app, window, cx| {
        cache_state(app, window, cx, &root)
    });
    assert!(!cached, "nothing has read the status of {root:?} yet");
    assert!(
        !looked_up,
        "nothing has asked for the repository root of the pane's directory yet"
    );
    enter(&app, &mut vcx);
    landed(&app, &mut vcx, &root);
    assert!(
        vcx.debug_bounds(header(ScmGroup::Merge)).is_some(),
        "the rows drew after the landing"
    );
    clean_up(&root);
}

// ------------------------------------------------------- entry (preloaded) --

/// RED today: the real action puts the keyboard on the first node (the first
/// group's header) and the live dispatch tree says so.
#[cfg(unix)]
#[gpui::test]
fn the_changes_action_puts_the_keyboard_on_the_first_header(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("entry");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "no `ScmRows` key context is mounted at the focus"
    );
    clean_up(&root);
}

/// RED today: two requests in one event are one entry, and it lands once.
#[cfg(unix)]
#[gpui::test]
fn two_entry_requests_in_one_event_still_enter_once(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("entry-twice");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "no `ScmRows` key context is mounted at the focus"
    );
    clean_up(&root);
}

/// RED today: Down from the first header walks into rows that were never built
/// (more than a viewport and its overscan away), the target is scrolled into
/// view, and only a bounded number of rows are mounted.
#[cfg(unix)]
#[gpui::test]
fn down_reaches_a_row_that_was_never_built_and_scrolls_it_into_view(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("walk");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let budget = row_budget(&app, &mut vcx);
    let k = budget + 15;
    assert!(k + 2 < staged.len(), "the fixture's list is long enough");
    assert!(
        vcx.debug_bounds(staged[k]).is_none(),
        "the target starts outside what a frame builds"
    );
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", staged[k], k + 8);
    assert!(
        visible(&app, &mut vcx, staged[k]),
        "the target is drawn inside the viewport"
    );
    assert!(
        painted(&mut vcx, &staged).len() <= budget + 2,
        "a bounded number of rows is mounted"
    );
    assert!(vcx.update(|window, cx| window.focused(cx)).is_some());
    clean_up(&root);
}

/// RED today: the focused row stays built and focused after the list is
/// scrolled far away, a B5 context menu on another row is built beside it, and
/// Escape gives the focus back to the row.
#[cfg(unix)]
#[gpui::test]
fn the_focused_row_and_a_menu_owner_both_stay_built_far_from_the_viewport(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("pins");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let budget = row_budget(&app, &mut vcx);
    let k = budget + 15;
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", staged[k], k + 8);

    // A B5 menu on a row beside the keyboard row, which must really be on
    // screen. BUILT is not VISIBLE: after a downward walk the minimal reveal
    // leaves row k flush with the viewport's bottom edge, so row k + 1 is built
    // (overscan) but lies below the scroller's clip, and a click at its centre
    // lands outside the scroller where no row is hovered. So the row below is
    // used only when it is visible, else the row above (inside the viewport).
    let view = viewport(&app, &mut vcx);
    let next_bounds = vcx.debug_bounds(staged[k + 1]);
    let owner = if visible(&app, &mut vcx, staged[k + 1]) {
        staged[k + 1]
    } else {
        staged[k - 1]
    };
    let owner_bounds = vcx.debug_bounds(owner);
    assert!(
        visible(&app, &mut vcx, owner),
        "setup: the menu owner {owner} must be on screen before the right-click; \
         viewport {view:?}, owner bounds {owner_bounds:?}, row below bounds {next_bounds:?}"
    );
    let neighbour = owner_bounds.expect("the menu owner is drawn");
    mouse(&mut vcx, MouseButton::Right, neighbour.center());
    draw(&app, &mut vcx);
    let recorded = app.update_in(&mut vcx, |app, _, _| app.scm.menu_row.clone());
    assert_eq!(
        recorded.map(|(g, p)| format!("scm-row-{g:?}-{p}")),
        Some(owner.to_string()),
        "the menu is open on the neighbour"
    );

    scroll_to(&app, &mut vcx, 1e7);
    assert!(
        vcx.debug_bounds(owner).is_some(),
        "the menu owner stays built far away"
    );
    assert!(
        vcx.debug_bounds(staged[k]).is_some(),
        "so does the row the keyboard was on"
    );
    assert!(
        painted(&mut vcx, &staged).len() <= budget + 2,
        "viewport, overscan and the two pins only"
    );

    vcx.simulate_keystrokes("escape");
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(staged[k]),
        "closing the menu returns the keyboard to the row"
    );
    clean_up(&root);
}

/// RED today: Enter opens the diff of the row that holds the keyboard, an
/// unbuilt row when it was reached.
#[cfg(unix)]
#[gpui::test]
fn enter_opens_the_diff_of_a_row_that_was_never_built(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("enter");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let k = row_budget(&app, &mut vcx) + 15;
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", staged[k], k + 8);
    press(&app, &mut vcx, "enter");
    let want = path_of(staged[k]);
    let got = app.update_in(&mut vcx, |app, _, _| {
        let repo = app.scm.repo.clone().unwrap();
        app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(ScmGroup::Staged))
            .map(str::to_string)
    });
    assert_eq!(got, Some(want));
    clean_up(&root);
}

/// RED today: Space on the staged copy unstages exactly that path; its
/// neighbours stay staged and the worktree keeps its edit.
#[cfg(unix)]
#[gpui::test]
fn space_unstages_exactly_the_staged_copy_it_is_on(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("space-staged");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let k = row_budget(&app, &mut vcx) + 15;
    let (here, before, after) = (
        path_of(staged[k]),
        path_of(staged[k - 1]),
        path_of(staged[k + 1]),
    );
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", staged[k], k + 8);
    press(&app, &mut vcx, "space");
    let unstaged = |p: &str| git_ok(&root, &["diff", "--cached", "--quiet", "--", p]);
    assert!(
        pump_until(&mut vcx, 30, |_| unstaged(&here)),
        "Space never unstaged {here}"
    );
    assert!(!unstaged(&before), "{before} is still staged");
    assert!(!unstaged(&after), "{after} is still staged");
    clean_up(&root);
}

/// RED today: with the same path in both groups, Space on the Changes copy
/// stages that path: the index takes the worktree edit (so the worktree and the
/// index now agree), and the worktree bytes themselves are untouched.
#[cfg(unix)]
#[gpui::test]
fn space_stages_the_changes_copy_of_a_path_that_is_also_staged(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("space-changes");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Changes);
    let changes = rows_of(&app, &mut vcx, ScmGroup::Changes);
    let id = format!("scm-row-Changes-{}", file(0));
    let target = *changes
        .iter()
        .find(|r| **r == id)
        .expect("file(0) is edited both staged and unstaged");
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", target, changes.len() + 8);
    assert!(
        !git_ok(&root, &["diff", "--quiet", "--", &file(0)]),
        "the worktree differs from the index before Space"
    );
    press(&app, &mut vcx, "space");
    assert!(
        pump_until(&mut vcx, 30, |_| git_ok(
            &root,
            &["diff", "--quiet", "--", &file(0)]
        )),
        "Space never staged the Changes copy"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(file(0))).unwrap(),
        "line 0\nedited\nagain\n",
        "the worktree bytes are untouched"
    );
    clean_up(&root);
}

/// RED today: in the tree, Left folds a directory, Down then skips its hidden
/// children, Right opens it again; Space and Backspace on a directory do
/// nothing at all (no prompt, no git change, no PTY bytes).
#[cfg(unix)]
#[gpui::test]
fn the_tree_folds_with_left_right_and_a_directory_ignores_space_and_backspace(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let root = keys_repo("tree");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    only_group(&app, &mut vcx, ScmGroup::Staged);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    let folded = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| {
            app.scm
                .folded_dirs
                .iter()
                .filter(|(g, _)| *g == ScmGroup::Staged)
                .count()
        })
    };
    // Merge header, then the Staged header, then the first directory.
    let mut dir = None;
    for _ in 0..6 {
        let now = marker(&mut vcx);
        assert!(
            now.is_some(),
            "no `ScmRows` key context is mounted at the focus"
        );
        if now
            .as_deref()
            .is_some_and(|m| m.starts_with("scm-dir-Staged-"))
        {
            dir = now;
            break;
        }
        press(&app, &mut vcx, "down");
    }
    let dir = dir.expect("Down reached a Staged directory row");

    let entries_before = status(&app, &mut vcx).entries.len();
    let staged_before = tracked_edits(&root).len();
    let _ = pane_input(&mut pane);
    press(&app, &mut vcx, "space");
    press(&app, &mut vcx, "backspace");
    let _ = pump_until(&mut vcx, 2, |_| false);
    assert!(!vcx.has_pending_prompt(), "Backspace on a directory asked");
    assert_eq!(status(&app, &mut vcx).entries.len(), entries_before);
    assert_eq!(tracked_edits(&root).len(), staged_before);
    assert_eq!(marker(&mut vcx).as_deref(), Some(dir.as_str()));
    assert!(
        pane_input(&mut pane).is_empty(),
        "the keys were consumed by the directory row, not typed into the pane"
    );

    assert_eq!(folded(&app, &mut vcx), 0);
    press(&app, &mut vcx, "left");
    assert_eq!(folded(&app, &mut vcx), 1, "Left folded the directory");
    press(&app, &mut vcx, "down");
    let next = marker(&mut vcx).expect("the keyboard is on a node");
    assert!(
        next.starts_with("scm-dir-Staged-") && next != dir,
        "with the directory folded, Down goes to the next directory, got {next}"
    );
    press(&app, &mut vcx, "up");
    assert_eq!(marker(&mut vcx).as_deref(), Some(dir.as_str()));
    press(&app, &mut vcx, "right");
    assert_eq!(folded(&app, &mut vcx), 0, "Right opened it again");
    press(&app, &mut vcx, "down");
    assert!(
        marker(&mut vcx)
            .as_deref()
            .is_some_and(|m| m.starts_with("scm-row-Staged-")),
        "Down from an open directory enters its first file"
    );
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    clean_up(&root);
}

/// RED today: Escape from a row hands the keyboard back to the pane.
#[cfg(unix)]
#[gpui::test]
fn escape_from_a_row_returns_the_keyboard_to_the_pane(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("escape");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    assert!(
        marker(&mut vcx).is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    press(&app, &mut vcx, "escape");
    assert_eq!(marker(&mut vcx), None);
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}

// ---------------------------------------------------------------- D7 / keys --

/// The Changes row `k` of the fixture, with the keyboard on it.
fn on_changes_row(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    k: usize,
) -> Vec<&'static str> {
    only_group(app, vcx, ScmGroup::Changes);
    let changes = rows_of(app, vcx, ScmGroup::Changes);
    focus_terminal(app, vcx);
    enter(app, vcx);
    walk_to(app, vcx, "down", changes[k], k + 8);
    changes
}

/// RED today: Backspace asks first; Cancel leaves the file, the row and the
/// keyboard exactly as they were.
#[cfg(unix)]
#[gpui::test]
fn backspace_then_cancel_keeps_the_file_the_row_and_the_keyboard(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("d7-cancel");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let changes = on_changes_row(&app, &mut vcx, 5);
    let target = path_of(changes[5]);
    let before = tracked_edits(&root);
    assert!(before.contains(&target));

    press(&app, &mut vcx, "backspace");
    assert!(
        pump_until(&mut vcx, 5, |vcx| vcx.has_pending_prompt()),
        "Backspace on a Changes row never asked for approval"
    );
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[5]),
        "asking does not move the keyboard"
    );
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    let _ = pump_until(&mut vcx, 2, |_| false);
    draw(&app, &mut vcx);
    assert!(!vcx.has_pending_prompt());
    assert_eq!(tracked_edits(&root), before, "Cancel discarded nothing");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[5]),
        "Cancel leaves the keyboard on the same row"
    );
    clean_up(&root);
}

/// RED today: confirming discards exactly that file, and only then the keyboard
/// moves to the next row; it is not handed to the pane.
#[cfg(unix)]
#[gpui::test]
fn backspace_then_confirm_discards_one_file_and_moves_to_the_next_row(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("d7-confirm");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let changes = on_changes_row(&app, &mut vcx, 5);
    let target = path_of(changes[5]);
    let before = tracked_edits(&root);
    let _ = pane_input(&mut pane);

    press(&app, &mut vcx, "backspace");
    assert!(
        pump_until(&mut vcx, 5, |vcx| vcx.has_pending_prompt()),
        "Backspace on a Changes row never asked for approval"
    );
    vcx.simulate_prompt_answer(t(L10nKey::ScmDiscard));
    assert!(
        pump_until(&mut vcx, 30, |_| !tracked_edits(&root).contains(&target)),
        "the discard never ran"
    );
    let landed_target = target.clone();
    refresh_until(
        &app,
        &mut vcx,
        &root,
        "the list never dropped the row",
        move |st| {
            !st.entries
                .iter()
                .any(|e| in_group(e, ScmGroup::Changes) && e.path.as_str() == landed_target)
        },
    );
    assert_eq!(
        tracked_edits(&root).len(),
        before.len() - 1,
        "exactly one file was discarded"
    );
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(changes[6]),
        "the keyboard moved to the next row, not to the pane"
    );
    assert!(
        pane_input(&mut pane).is_empty(),
        "nothing was typed into the pane"
    );
    clean_up(&root);
}

/// RED today: a held (repeated) Space and Backspace never act; a fresh press
/// does. One prompt, not one per repeat. (Enter's held guard is not exercised
/// here: a held Enter would open the same diff again, which this fixture cannot
/// tell from one.)
#[cfg(unix)]
#[gpui::test]
fn held_keys_never_act_and_a_held_backspace_asks_once(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("held");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    crate::ui::i18n::set_locale("en");
    let changes = on_changes_row(&app, &mut vcx, 5);
    let path = path_of(changes[5]);

    key_event(&mut vcx, "space", true);
    let _ = pump_until(&mut vcx, 2, |_| false);
    assert!(
        !git_ok(&root, &["diff", "--quiet", "--", &path]),
        "a held Space staged the file"
    );
    key_event(&mut vcx, "backspace", true);
    let _ = pump_until(&mut vcx, 1, |_| false);
    assert!(!vcx.has_pending_prompt(), "a held Backspace asked");

    key_event(&mut vcx, "backspace", false);
    assert!(
        pump_until(&mut vcx, 5, |vcx| vcx.has_pending_prompt()),
        "a fresh Backspace never asked for approval"
    );
    key_event(&mut vcx, "backspace", true);
    key_event(&mut vcx, "backspace", true);
    vcx.simulate_prompt_answer(t(L10nKey::Cancel));
    let _ = pump_until(&mut vcx, 2, |_| false);
    assert!(
        !vcx.has_pending_prompt(),
        "the repeats did not queue a second prompt"
    );
    key_event(&mut vcx, "space", false);
    assert!(
        pump_until(&mut vcx, 30, |_| git_ok(
            &root,
            &["diff", "--quiet", "--", &path]
        )),
        "a fresh Space stages the file"
    );
    clean_up(&root);
}

/// Guard (passes today by absence; paired with the RED entry tests): while the
/// terminal holds the keyboard, Space, Backspace, Enter and the arrows reach
/// it and the Changes list does nothing: no prompt, no stage.
#[cfg(unix)]
#[gpui::test]
fn guard_the_terminal_keeps_its_keys_while_the_panel_is_open(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("guard-terminal");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    assert!(
        vcx.debug_bounds(header(ScmGroup::Merge)).is_some(),
        "the list is on screen"
    );
    let before = tracked_edits(&root);
    for (key, byte) in [("space", b' '), ("backspace", 0x7f), ("enter", b'\r')] {
        assert!(reaches_pane(&app, &mut vcx, &mut pane, key, byte), "{key}");
    }
    let _ = pump_until(&mut vcx, 1, |_| false);
    assert!(!vcx.has_pending_prompt());
    assert_eq!(tracked_edits(&root), before);
    assert_eq!(marker(&mut vcx), None);
    clean_up(&root);
}

// -------------------------------------------------- lifecycle of an entry --

/// RED today: the external reorder case. Files staged elsewhere in the list
/// shift the row's index past the viewport; the keyboard stays on the same
/// path (identity, not index), that row stays built, the mounted-row count
/// stays bounded, and the panel goes quiet again.
#[cfg(unix)]
#[gpui::test]
fn an_external_reorder_beyond_the_viewport_keeps_the_row_and_the_budget(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("reorder");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    only_group(&app, &mut vcx, ScmGroup::Staged);
    let staged = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let budget = row_budget(&app, &mut vcx);
    let k = budget + 15;
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    walk_to(&app, &mut vcx, "down", staged[k], k + 8);
    let want = staged[k].to_string();

    // Five new staged files that sort first: every existing Staged row moves
    // down five places.
    for i in 0..5 {
        write(&root, &format!("aaa_new_{i}.txt"), "new\n");
    }
    git(
        &root,
        &[
            "add",
            "--",
            "aaa_new_0.txt",
            "aaa_new_1.txt",
            "aaa_new_2.txt",
            "aaa_new_3.txt",
            "aaa_new_4.txt",
        ],
    );
    refresh_until(&app, &mut vcx, &root, "the reorder never landed", |st| {
        st.entries
            .iter()
            .filter(|e| in_group(e, ScmGroup::Staged))
            .count()
            >= staged.len().min(299)
            && st
                .entries
                .iter()
                .any(|e| e.path.as_str() == "aaa_new_4.txt")
    });
    let now = rows_of(&app, &mut vcx, ScmGroup::Staged);
    let moved_to = now.iter().position(|r| *r == want).expect("still listed");
    assert!(moved_to > k, "the row's index changed under the keyboard");
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(want.as_str()),
        "the keyboard stayed on the same path"
    );
    assert!(
        vcx.debug_bounds(now[moved_to]).is_some(),
        "that row is still built after the refresh"
    );
    assert!(
        painted(&mut vcx, &now).len() <= budget + 2,
        "the mounted rows stay bounded"
    );
    test_window::quiesce(&mut vcx, None);
    render_probe::arm(200);
    for _ in 0..10 {
        vcx.executor().advance_clock(Duration::from_millis(500));
        vcx.run_until_parked();
    }
    assert_eq!(render_probe::draws(), 0, "a parked panel draws nothing");
    clean_up(&root);
}

/// RED today: when the row the keyboard is on goes away outside the panel
/// (git restores it), the keyboard is handed to the pane: no orphan, keys
/// alive; a later entry never lands on the vanished row.
#[cfg(unix)]
#[gpui::test]
fn an_externally_removed_row_never_leaves_the_keyboard_on_an_orphan(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("vanish");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    let changes = on_changes_row(&app, &mut vcx, 5);
    let gone = path_of(changes[5]);
    git(&root, &["checkout", "--", &gone]);
    let want_gone = gone.clone();
    refresh_until(
        &app,
        &mut vcx,
        &root,
        "the row never left the list",
        move |st| {
            !st.entries
                .iter()
                .any(|e| in_group(e, ScmGroup::Changes) && e.path.as_str() == want_gone)
        },
    );
    assert!(vcx.debug_bounds(changes[5]).is_none(), "the row is gone");
    assert_ne!(
        marker(&mut vcx).as_deref(),
        Some(changes[5]),
        "no orphan holds the keyboard"
    );
    assert!(
        reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'),
        "keys reach the pane after the row vanished"
    );
    enter(&app, &mut vcx);
    assert_ne!(marker(&mut vcx).as_deref(), Some(changes[5]));
    press(&app, &mut vcx, "escape");
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "y", b'y'));
    clean_up(&root);
}

/// RED today (guard half passes): a request, then the user's focus moves to
/// the filter field in the same event, before any frame: the late entry
/// callback must not take it back. A plain request lands first (RED), so the
/// scenario is known to be reachable.
#[cfg(unix)]
#[gpui::test]
fn a_request_does_not_steal_the_keyboard_from_a_field_that_took_it(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("late-focus");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    assert!(
        marker(&mut vcx).is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    press(&app, &mut vcx, "escape");

    let filter = app
        .update_in(&mut vcx, |app, _, cx| {
            app.scm.filter.as_ref().map(|f| f.read(cx).focus_handle(cx))
        })
        .expect("the filter field exists");
    app.update_in(&mut vcx, |app, window, cx| {
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        window.focus(&filter, cx);
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(filter),
        "the field keeps the keyboard"
    );
    assert_eq!(marker(&mut vcx), None);
    clean_up(&root);
}

/// RED today (guard half passes): a request, then the repository changes in
/// the same event (an explicit repository pick), before the frame: the entry
/// must not focus anything in the other repository.
#[cfg(unix)]
#[gpui::test]
fn a_request_is_dropped_when_the_repository_changes_before_it_lands(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("repo-a");
    let other = scratch("repo-b").join("repo");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    write(&other, "one.txt", "1\n");
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "one"]);
    write(&other, "one.txt", "2\n");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    assert!(
        marker(&mut vcx).is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    press(&app, &mut vcx, "escape");

    let b = RepoKey {
        host: HostId::LOCAL,
        root: std::fs::canonicalize(&other).unwrap(),
    };
    app.update_in(&mut vcx, |app, window, cx| {
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        app.scm.repo_override = Some(b.clone());
        app.scm.override_tab = Some(app.active);
        cx.notify();
    });
    let shown = pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |_, _, cx| {
            crate::terminal::git_data::status_of(cx, b.host, &b.root).is_some()
        })
    });
    assert!(shown, "the other repository's status never landed");
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx),
        None,
        "nothing in the other repository took the keyboard"
    );
    clean_up(&root);
    clean_up(&other);
}

/// RED today (guard half passes): a request, then the panel is hidden in the
/// same event; showing it again by pointer afterwards does not resurrect the
/// request.
#[cfg(unix)]
#[gpui::test]
fn a_request_is_dropped_when_the_panel_is_hidden_before_it_lands(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("hide");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    assert!(
        marker(&mut vcx).is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    press(&app, &mut vcx, "escape");

    app.update_in(&mut vcx, |app, window, cx| {
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        app.right_panel_visible = false;
        cx.notify();
    });
    draw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        app.set_right_panel_tab(RightPanelTab::Scm, cx)
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx),
        None,
        "the hidden request did not resurface"
    );
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}

/// RED today: from a window whose status has not been read, the real action
/// parks; when the read lands the keyboard goes to the first header.
#[cfg(unix)]
#[gpui::test]
fn an_entry_made_while_the_status_loads_completes_when_it_lands(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("loading");
    let (app, mut vcx, _pane) = unloaded(cx, &root);
    // The request, the "still loading" look and the "nothing holds the keyboard
    // yet" look are ONE event: no frame and no pump has run between them, so a
    // fast landing cannot have completed the entry before the assertion.
    let (cached, looked_up, during) = app.update_in(&mut vcx, |app, window, cx| {
        let (cached, looked_up) = cache_state(app, window, cx, &root);
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        (cached, looked_up, marker_in(window))
    });
    assert!(!cached, "the status was already cached at the request");
    assert!(
        !looked_up,
        "the root lookup had already started at the request"
    );
    assert_eq!(during, None, "nothing to focus in the requesting event");
    draw(&app, &mut vcx);
    landed(&app, &mut vcx, &root);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "no `ScmRows` key context is mounted at the focus"
    );
    clean_up(&root);
}

/// Guard: a request made while loading is dropped when the repository is
/// switched before the read lands (the pick is made in the same event).
#[cfg(unix)]
#[gpui::test]
fn guard_a_loading_request_is_dropped_by_a_repository_switch(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("loading-switch");
    let other = scratch("loading-b").join("repo");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    write(&other, "one.txt", "1\n");
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "one"]);
    write(&other, "one.txt", "2\n");
    let (app, mut vcx, _pane) = unloaded(cx, &root);
    let b = RepoKey {
        host: HostId::LOCAL,
        root: std::fs::canonicalize(&other).unwrap(),
    };
    // Scenario validity, read in the requesting event itself: the first
    // repository's status was not cached and its lookup not started, so the
    // request really is a loading request, and the pick follows in the same
    // event, before any frame can start a read.
    let (cached, looked_up) = app.update_in(&mut vcx, |app, window, cx| {
        let state = cache_state(app, window, cx, &root);
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        app.scm.repo_override = Some(b.clone());
        app.scm.override_tab = Some(app.active);
        cx.notify();
        state
    });
    assert!(!cached, "the first repository's status was already cached");
    assert!(
        !looked_up,
        "the first repository's lookup had already started"
    );
    let shown = pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |_, _, cx| {
            crate::terminal::git_data::status_of(cx, b.host, &b.root).is_some()
        })
    });
    assert!(shown, "the other repository's status never landed");
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(marker(&mut vcx), None, "the parked request was dropped");
    clean_up(&root);
    clean_up(&other);
}

/// Guard: a request made while loading is dropped when the panel is hidden and
/// shown again by pointer before the read lands.
#[cfg(unix)]
#[gpui::test]
fn guard_a_loading_request_is_dropped_by_hiding_the_panel(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("loading-hide");
    let (app, mut vcx, mut pane) = unloaded(cx, &root);
    // Request and hide are one event, so the request cannot have landed, and
    // the cache and lookup are read in that same event to prove it was loading.
    let (cached, looked_up) = app.update_in(&mut vcx, |app, window, cx| {
        let state = cache_state(app, window, cx, &root);
        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
        app.right_panel_visible = false;
        cx.notify();
        state
    });
    assert!(!cached, "the status was already cached at the request");
    assert!(
        !looked_up,
        "the root lookup had already started at the request"
    );
    draw(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        app.set_right_panel_tab(RightPanelTab::Scm, cx)
    });
    landed(&app, &mut vcx, &root);
    assert_eq!(marker(&mut vcx), None, "a pointer reveal never focuses");
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}

/// Guard: pointer reveal of the tab (the pill, `set_right_panel_tab`) never
/// takes the keyboard, with the status already cached.
#[cfg(unix)]
#[gpui::test]
fn guard_a_pointer_reveal_of_the_tab_never_takes_the_keyboard(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("pointer-reveal");
    let (app, mut vcx, mut pane) = panel_on(cx, &root);
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    draw(&app, &mut vcx);
    focus_terminal(&app, &mut vcx);
    app.update_in(&mut vcx, |app, _, cx| {
        app.set_right_panel_tab(RightPanelTab::Scm, cx)
    });
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(marker(&mut vcx), None);
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}

/// RED today (guard half passes): re-entering after the filter hid the row the
/// keyboard was on never lands on an orphan: it is a node that exists in the
/// filtered list.
#[cfg(unix)]
#[gpui::test]
fn a_filter_that_hides_the_remembered_row_never_leaves_an_orphan_on_entry(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = keys_repo("filter");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let changes = on_changes_row(&app, &mut vcx, 5);
    press(&app, &mut vcx, "escape");

    let filter = app
        .update_in(&mut vcx, |app, _, cx| {
            app.scm.filter.as_ref().map(|f| f.read(cx).focus_handle(cx))
        })
        .expect("the filter field exists");
    vcx.update(|window, cx| window.focus(&filter, cx));
    vcx.simulate_input("file_01");
    draw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds(changes[5]).is_none(),
        "the filter hid the remembered row"
    );
    focus_terminal(&app, &mut vcx);
    enter(&app, &mut vcx);
    let now = marker(&mut vcx);
    assert!(
        now.is_some(),
        "no `ScmRows` key context is mounted at the focus"
    );
    assert_ne!(now.as_deref(), Some(changes[5]));
    let id: &'static str = Box::leak(now.unwrap().into_boxed_str());
    assert!(
        vcx.debug_bounds(id).is_some(),
        "the keyboard is on a node that is drawn: {id}"
    );
    clean_up(&root);
}

// ---------------------------------------------- empty / broken / not a repo --

fn assert_entry_is_a_no_op(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    pane: &mut Stream,
    what: &str,
) {
    enter(app, vcx);
    assert_eq!(marker(vcx), None, "{what}: nothing holds the keyboard");
    assert!(
        reaches_pane(app, vcx, pane, "x", b'x'),
        "{what}: typing reaches the pane"
    );
    for (key, byte) in [("space", b' '), ("backspace", 0x7f), ("enter", b'\r')] {
        assert!(reaches_pane(app, vcx, pane, key, byte), "{what}: {key}");
    }
    press(app, vcx, "escape");
    assert!(
        reaches_pane(app, vcx, pane, "y", b'y'),
        "{what}: Escape leaves the keyboard on the pane"
    );
}

/// Guard: a clean repository has nothing to enter. The action is a no-op; no
/// key freezes; a later change in the repository does not pull the keyboard in.
#[cfg(unix)]
#[gpui::test]
fn guard_a_clean_repository_has_nothing_to_enter_and_nothing_freezes(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = scratch("clean").join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    assert!(pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |app, window, cx| {
            cx.notify();
            app.scm_pane_target(window, cx).is_some()
        })
    }));
    focus_terminal(&app, &mut vcx);
    assert_entry_is_a_no_op(&app, &mut vcx, &mut pane, "a clean repository");

    write(&root, "a.txt", "b\n");
    let r = root.clone();
    refresh_until(&app, &mut vcx, &r, "the change never landed", |st| {
        !st.entries.is_empty()
    });
    assert_eq!(marker(&mut vcx), None, "no late focus appeared");
    clean_up(&root);
}

/// Guard: a directory that is not a repository.
#[cfg(unix)]
#[gpui::test]
fn guard_a_directory_that_is_not_a_repository_has_nothing_to_enter(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = scratch("norepo");
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    assert!(pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |app, window, cx| {
            cx.notify();
            app.scm_pane_target(window, cx).is_some()
        })
    }));
    focus_terminal(&app, &mut vcx);
    assert_entry_is_a_no_op(&app, &mut vcx, &mut pane, "not a repository");
    clean_up(&root);
}

/// Guard: a repository whose status cannot be read (a broken index).
#[cfg(unix)]
#[gpui::test]
fn guard_a_repository_with_a_broken_index_has_nothing_to_enter(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = scratch("broken").join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    std::fs::write(root.join(".git").join("index"), b"not an index").unwrap();
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Info;
        cx.notify();
    });
    assert!(pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |app, window, cx| {
            cx.notify();
            app.scm_pane_target(window, cx).is_some()
        })
    }));
    focus_terminal(&app, &mut vcx);
    assert_entry_is_a_no_op(&app, &mut vcx, &mut pane, "a broken index");
    clean_up(&root);
}

// ------------------------------------------------------------------ Info --

/// RED today: with the Info root holding the keyboard, Down enters the
/// interactive rows (the pane address row first, then the working-directory
/// row), Enter runs that row's copy action (on the working-directory row the
/// clipboard gets the path), Up goes back through the pane row to the root and
/// Escape hands the keyboard to the pane.
///
/// Rebase adaptation: `main` put the pane address row before the working
/// directory, so the working-directory steps below take one more Down and its
/// Up one more; every assertion of the original case is kept.
///
/// Not covered here, deliberately: Space on the working-directory row is
/// Reveal, whose platform call is `unimplemented!()` in the test platform; that
/// is a post-implementation control with its own requirements.
#[cfg(unix)]
#[gpui::test]
fn info_rows_are_reachable_and_enter_copies_the_working_directory(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = scratch("info");
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    DaemonMsg::Cwd(root.clone()).encode(&mut pane).unwrap();
    assert!(pump_until(&mut vcx, 30, |vcx| {
        app.update_in(vcx, |app, window, cx| {
            cx.notify();
            app.scm_pane_target(window, cx).is_some()
        })
    }));
    vcx.dispatch_action(ShowRightPanelInfo);
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds it, no row yet");

    press(&app, &mut vcx, "down");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "no `InfoRow` key context is mounted at the focus"
    );
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));
    vcx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".to_string()))
    });
    press(&app, &mut vcx, "enter");
    let copied = vcx
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .unwrap_or_default();
    assert_eq!(copied, root.display().to_string());

    press(&app, &mut vcx, "up");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "Up from the working directory is the pane row"
    );
    press(&app, &mut vcx, "up");
    assert_eq!(
        info_marker(&mut vcx),
        None,
        "Up from the first row is the root"
    );
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    press(&app, &mut vcx, "escape");
    assert_eq!(info_marker(&mut vcx), None);
    assert!(reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'));
    clean_up(&root);
}
