//! POST-IMPLEMENTATION controls for the Info rows' keyboard, written AFTER
//! `info_row_key` and the entry in `right_panel.rs`. They are not RED-first:
//! the frozen `rows_keys_tests` pin the entry, the working-directory copy, Up
//! to the root and Escape (one test, `info_rows_are_reachable...`), and this
//! file adds what it did not: the other rows, held and released keys, Space,
//! clamping and Home/End, Tab, and a row that vanishes under the keyboard.
//!
//! Observability, honestly: Copy is observed end to end through the readable
//! test clipboard, always reset to a sentinel before the press, so "nothing
//! copied" is a real observation. A diff is observed by the overlay on the
//! active tab. Reveal is NOT observable in the test platform (`reveal_path`
//! is `unimplemented!()`, with no recorded argument): Space on a row without
//! Reveal is observed as inert (a stray call would panic). The one Space that
//! does reach `reveal_path` is not part of this suite: it is a review asset
//! outside the tree (a deliberately panicking witness, appended to a throwaway
//! copy on Boat), whose only claim is that the real key arrives at the
//! platform call. The pure mapping of what Reveal is given is in
//! `right_panel/info_keys_tests.rs`.
//!
//! The marker is the `key` value of the `InfoRow` key context on the focus
//! path; the rows of the big fixture's pane are Pane, Cwd, Shell, Branch,
//! Changes, of which Pane, Cwd, Branch and Changes are interactive. Rebase
//! adaptation: `main` put the copyable Pane address row before Cwd, so the
//! first row is Pane and every walk to Cwd, Branch or Changes takes one more
//! Down than before (and the way back to the root one more Up); each original
//! assertion is kept.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{
    Entity, Focusable as _, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, TestAppContext,
    VisualTestContext,
};
use tty59_core::core::config::RightPanelTab;

use super::rows_window_tests::{big_repo, draw, git_stdout, header, panel_on, scratch, serial};
use crate::core::actions::{ShowRightPanelChanges, ShowRightPanelInfo};
use crate::daemon::protocol::{ClientMsg, DaemonMsg};
use crate::ui::app::Tty59App;
use crate::ui::scm::state::ScmGroup;

type Stream = crate::daemon::transport::Stream;

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

/// The `row` value of the `ScmRows` key context on the focus path: the Changes
/// node that holds the keyboard.
fn scm_marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("ScmRows"))
            .and_then(|c| c.get("row"))
            .map(|v| v.to_string())
    })
}

fn press(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    draw(app, vcx);
}

/// A key-down with `is_held` set, the way the platform delivers a repeat.
fn held(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("a keystroke");
    vcx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: true,
                prefer_character_input: false,
            }),
            cx,
        );
    });
    vcx.run_until_parked();
    draw(app, vcx);
}

fn release(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("a keystroke");
    vcx.simulate_event(KeyUpEvent { keystroke });
    draw(app, vcx);
}

fn sentinel(vcx: &mut VisualTestContext) {
    vcx.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".to_string()))
    });
}

fn clipboard(vcx: &mut VisualTestContext) -> String {
    vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .unwrap_or_default()
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

fn clean_up(root: &Path) {
    let dir = match root.file_name().and_then(|n| n.to_str()) {
        Some("repo") => root.parent().unwrap_or(root),
        _ => root,
    };
    let _ = std::fs::remove_dir_all(dir);
}

fn overlay_open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
    app.update_in(vcx, |app, _, _| {
        app.tabs
            .get(app.active)
            .is_some_and(|tab| tab.diff_overlay.is_some())
    })
}

/// The big fixture's pane with the Info tab asked for by the real action, and
/// its branch row drawn (so the git rows are there).
fn info_on_repo(
    cx: &mut TestAppContext,
    name: &str,
) -> (
    Entity<Tty59App>,
    VisualTestContext,
    Stream,
    std::path::PathBuf,
) {
    let root = big_repo(name);
    let (app, mut vcx, pane) = panel_on(cx, &root);
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.dispatch_action(ShowRightPanelInfo);
    let landed = pump_until(&mut vcx, 60, |vcx| {
        app.update_in(vcx, |_, _, cx| cx.notify());
        draw(&app, vcx);
        vcx.debug_bounds("panel-info-branch-row").is_some()
    });
    assert!(landed, "the Info table never drew its branch row");
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds it, no row yet");
    (app, vcx, pane, root)
}

/// Move the pane to `elsewhere` (a directory that is not a repository) and
/// wait for the CAUSE, not the effect. The `Cwd` message only moves the
/// terminal's own working directory; the pane's git directory, which the Info
/// branch and changes rows read, is moved by its poll, whose 300ms timer is on
/// the test platform's VIRTUAL clock and fires only when that clock is
/// advanced. So the clock is advanced in 300ms steps, with the executor run to
/// rest after each, until BOTH the pane's own git directory is `elsewhere` and
/// the git cache holds an answer for it (a negative answer counts), under a
/// real-time deadline. `quiesce` is not used: it also requires the window to
/// stop drawing, which it never does once the fallback has given the keyboard
/// to the pane (its caret and timers keep drawing as the clock advances).
/// Nothing here polls the branch row or the focus: after the barrier the
/// pane's own state is asserted to have followed, and then the row is asserted
/// gone.
fn pane_moves_away(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    pane: &mut Stream,
    elsewhere: &Path,
) {
    DaemonMsg::Cwd(elsewhere.to_path_buf())
        .encode(pane)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        vcx.executor().advance_clock(Duration::from_millis(300));
        vcx.run_until_parked();
        let followed = app.update_in(vcx, |app, window, cx| {
            let pane_here = app
                .tabs
                .get(app.active)
                .and_then(|tab| tab.detail_pane(window, cx))
                .is_some_and(|leaf| leaf.read(cx).git_status_cwd() == Some(elsewhere));
            let answered = cx
                .try_global::<crate::terminal::git_status::GitStatusCache>()
                .and_then(|cache| {
                    cache.known_repo_for(crate::ui::host_ops::HostId::LOCAL, elsewhere)
                })
                .is_some();
            pane_here && answered
        });
        if followed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pane's git directory never followed its working directory to the cache's answer"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    draw(app, vcx);
    let (git_cwd, effective) = app.update_in(vcx, |app, window, cx| {
        let leaf = app
            .tabs
            .get(app.active)
            .and_then(|tab| tab.detail_pane(window, cx))
            .expect("the active tab has a detail pane");
        let view = leaf.read(cx);
        (
            view.git_status_cwd().map(Path::to_path_buf),
            view.effective_cwd(),
        )
    });
    assert_eq!(
        git_cwd.as_deref(),
        Some(elsewhere),
        "the pane's git directory did not follow its working directory"
    );
    assert_eq!(effective.as_deref(), Some(elsewhere));
    assert!(
        vcx.debug_bounds("panel-info-branch-row").is_none(),
        "the branch row never went"
    );
}

fn branch_of(root: &Path) -> String {
    git_stdout(root, &["branch", "--show-current"])
        .trim()
        .to_string()
}

/// Enter on the Branch row copies the branch name, Enter on the Changes row
/// opens the diff and copies nothing; Shell, which has no action, is not a
/// stop.
#[gpui::test]
fn enter_copies_the_branch_and_opens_the_diff_from_the_changes_row(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-enter");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));
    press(&app, &mut vcx, "down");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Branch"),
        "Shell has no action and is skipped"
    );
    sentinel(&mut vcx);
    press(&app, &mut vcx, "enter");
    let branch = branch_of(&root);
    assert!(!branch.is_empty());
    assert_eq!(clipboard(&mut vcx), branch, "Enter copies the branch name");

    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Changes"));
    sentinel(&mut vcx);
    assert!(!overlay_open(&app, &mut vcx));
    press(&app, &mut vcx, "enter");
    assert!(overlay_open(&app, &mut vcx), "Enter opened no diff");
    assert_eq!(
        clipboard(&mut vcx),
        "sentinel",
        "the Changes row copies nothing"
    );
    clean_up(&root);
}

/// A press acts once: a repeat of the key-down copies nothing, and neither
/// does a key-up on its own; a fresh press copies again.
#[gpui::test]
fn a_held_enter_and_a_release_copy_nothing_more(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-held");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    let branch = branch_of(&root);

    sentinel(&mut vcx);
    press(&app, &mut vcx, "enter");
    assert_eq!(clipboard(&mut vcx), branch);
    sentinel(&mut vcx);
    release(&app, &mut vcx, "enter");
    assert_eq!(clipboard(&mut vcx), "sentinel", "the key-up copied");
    held(&app, &mut vcx, "enter");
    assert_eq!(clipboard(&mut vcx), "sentinel", "a held Enter copied");
    press(&app, &mut vcx, "enter");
    assert_eq!(clipboard(&mut vcx), branch, "a fresh press copies again");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    clean_up(&root);
}

/// Space is Reveal only: on a row without one (Branch, Changes) it does
/// nothing at all. A stray `reveal_path` would panic in the test platform, so
/// reaching the end of the test is the observation; the clipboard and the
/// keyboard are untouched and no diff opens.
#[gpui::test]
fn space_is_inert_on_rows_without_reveal(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-space");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    sentinel(&mut vcx);
    press(&app, &mut vcx, "space");
    assert_eq!(clipboard(&mut vcx), "sentinel");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Changes"));
    press(&app, &mut vcx, "space");
    assert_eq!(clipboard(&mut vcx), "sentinel");
    assert!(!overlay_open(&app, &mut vcx), "Space opened the diff");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Changes"));
    clean_up(&root);
}

/// Down clamps at the last stop, Home and End go to the ends, Up goes back,
/// and Up from the first row returns the keyboard to the root; from the root
/// End enters the last row.
#[gpui::test]
fn the_rows_are_walked_clamped_and_left_by_up_from_the_first(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-walk");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Changes"));
    press(&app, &mut vcx, "down");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Changes"),
        "Down wrapped past the last stop"
    );
    press(&app, &mut vcx, "home");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    press(&app, &mut vcx, "end");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Changes"));
    press(&app, &mut vcx, "up");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    press(&app, &mut vcx, "up");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));
    press(&app, &mut vcx, "up");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "Up from Cwd is the Pane row, not the root"
    );
    press(&app, &mut vcx, "up");
    assert_eq!(
        info_marker(&mut vcx),
        None,
        "Up from the first row is the root"
    );
    press(&app, &mut vcx, "end");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Changes"),
        "End from the root enters the last row"
    );
    press(&app, &mut vcx, "escape");
    assert_eq!(info_marker(&mut vcx), None);
    clean_up(&root);
}

/// Tab from the root lands on the first interactive row with no key handling
/// of its own: the row tracks the stop while none is chosen. From there the
/// keys work as everywhere.
#[gpui::test]
fn tab_from_the_root_reaches_the_first_row(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-tab");
    press(&app, &mut vcx, "tab");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "Tab did not reach the first interactive row"
    );
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    press(&app, &mut vcx, "up");
    press(&app, &mut vcx, "up");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    press(&app, &mut vcx, "up");
    assert_eq!(info_marker(&mut vcx), None);
    clean_up(&root);
}

/// The row the keyboard is on disappears (the pane moves to a directory that
/// is not a repository, so the branch and changes rows go): the keyboard is
/// handed to the pane by the app's own fallback, a typed key reaches the
/// terminal, and asking for Info again then Down enters the rows afresh.
#[gpui::test]
fn a_row_that_vanishes_under_the_keyboard_hands_it_to_the_pane(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, mut pane, root) = info_on_repo(cx, "info-vanish");
    let elsewhere = scratch("info-vanish-away");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));

    pane_moves_away(&app, &mut vcx, &mut pane, &elsewhere);
    assert_eq!(
        info_marker(&mut vcx),
        None,
        "a stop with no row holds the keyboard"
    );
    assert!(
        reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'),
        "the keyboard did not reach the pane"
    );

    vcx.dispatch_action(ShowRightPanelInfo);
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds it again");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Pane"));
    clean_up(&root);
    clean_up(&elsewhere);
}

/// REVIEWER PIN, tests only, written AFTER the source review of `info_tracked`
/// versus `info_row_key`. After a row vanishes under the keyboard and the
/// app's own fallback has handed it to the pane, `at` still names the vanished
/// row. Re-entering by Tab WITHOUT asking for Info by name (which `reset()`s
/// that choice, and so would mask this path) is the route observed here: the
/// first row tracks the stop while it is not focused, but once the stop is
/// focused the stale choice makes `info_tracked` give nothing, and the row
/// that Tab just reached is unmounted again.
///
/// The only setup that is not a real key: the Info ROOT is given the keyboard
/// directly (`focus` on `info_focus`), standing for any way the user could
/// have come back to the panel's root without `show_right_panel_tab` (a click
/// on its body is not one that is focusable here). No show, no reset.
///
/// If this fails on the marker after Tab, the stale choice is not repaired by
/// any event on this route, and the limitation is real: Tab re-entry only
/// works after Info is asked for by name. That is what it would pin, not a
/// claim that Tab works.
#[gpui::test]
fn tab_reaches_the_first_row_after_a_row_vanished_without_asking_for_info_again(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let (app, mut vcx, mut pane, root) = info_on_repo(cx, "info-stale-tab");
    let elsewhere = scratch("info-stale-tab-away");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));

    pane_moves_away(&app, &mut vcx, &mut pane, &elsewhere);
    assert!(
        reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'),
        "the existing fallback did not hand the keyboard to the pane"
    );

    // SETUP, not a key: the root alone, no show and no reset.
    app.update_in(&mut vcx, |app, window, cx| {
        let root = app.info_focus.clone();
        window.focus(&root, cx);
    });
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds it");

    press(&app, &mut vcx, "tab");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "Tab did not land on, or did not keep, the first row while `at` names a vanished row"
    );
    // The first row is the Pane address now; the working directory is the
    // next stop, where the original assertion on the current directory holds.
    press(&app, &mut vcx, "down");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Cwd"),
        "Down from the reached first row did not move to the working directory"
    );
    sentinel(&mut vcx);
    press(&app, &mut vcx, "enter");
    assert_eq!(
        clipboard(&mut vcx),
        elsewhere.display().to_string(),
        "Enter on the reached row did not copy the current directory"
    );
    clean_up(&root);
    clean_up(&elsewhere);
}

/// REVIEWER VARIANT, tests only, for the shape a cleanup done only on the
/// loss of the keyboard would miss. Here the keyboard has already LEFT the
/// stop (Escape to the pane) before the row goes, so no focus is lost when the
/// pane's directory changes and `at` keeps naming the vanished Branch row. The
/// Info root is then given the keyboard as the labelled SETUP (no show, no
/// reset), and a real Tab must land on, and keep, the first row (the Pane
/// address after the rebase); one Down more reaches the working directory,
/// whose Enter copies the CURRENT directory.
#[gpui::test]
fn tab_reaches_the_first_row_after_the_keyboard_had_already_left_and_the_row_vanished(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let (app, mut vcx, mut pane, root) = info_on_repo(cx, "info-stale-left");
    let elsewhere = scratch("info-stale-left-away");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Branch"));
    press(&app, &mut vcx, "escape");
    assert_eq!(info_marker(&mut vcx), None, "Escape left the stop");
    assert!(
        reaches_pane(&app, &mut vcx, &mut pane, "x", b'x'),
        "Escape did not hand the keyboard to the pane"
    );

    pane_moves_away(&app, &mut vcx, &mut pane, &elsewhere);

    // SETUP, not a key: the root alone, no show and no reset.
    app.update_in(&mut vcx, |app, window, cx| {
        let root = app.info_focus.clone();
        window.focus(&root, cx);
    });
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the root holds it");

    press(&app, &mut vcx, "tab");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Pane"),
        "Tab did not land on, or did not keep, the first row while `at` names a vanished row"
    );
    // The first row is the Pane address now; the working directory is the
    // next stop, where the original assertion on the current directory holds.
    press(&app, &mut vcx, "down");
    assert_eq!(
        info_marker(&mut vcx).as_deref(),
        Some("Cwd"),
        "Down from the reached first row did not move to the working directory"
    );
    sentinel(&mut vcx);
    press(&app, &mut vcx, "enter");
    assert_eq!(
        clipboard(&mut vcx),
        elsewhere.display().to_string(),
        "Enter on the reached row did not copy the current directory"
    );
    clean_up(&root);
    clean_up(&elsewhere);
}

/// REVIEWER PROPERTY, observed not assumed: the public Info to Changes
/// transition. With the keyboard on a live Info row (Cwd), the real
/// `ShowRightPanelChanges` action must leave the keyboard on the Changes list's
/// first header, and a typed key must not reach the pane's PTY.
///
/// The question it asks of the source: the Changes entry request captures the
/// handle that has the keyboard as its origin, and that handle is the Info
/// stop, which the tab switch unmounts. The app's existing focus-lost fallback
/// may then hand the keyboard to the pane before the deferred completion of the
/// request runs, and the completion, which requires the keyboard to still be
/// where the request found it, would drop the entry. If so this fails on the
/// marker, and that failure is the finding; no guard is loosened here.
#[gpui::test]
fn the_changes_action_from_a_live_info_row_lands_on_the_first_header(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, mut pane, root) = info_on_repo(cx, "info-to-changes");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));

    vcx.dispatch_action(ShowRightPanelChanges);
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(info_marker(&mut vcx), None, "the Info row still holds it");
    assert_eq!(
        scm_marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "the keyboard is not on the Changes list's first header"
    );

    let _ = pane_input(&mut pane);
    type_key(&mut vcx, "x");
    draw(&app, &mut vcx);
    let got = pane_input(&mut pane);
    assert!(
        !got.contains(&b'x'),
        "a typed key reached the pane, so the keyboard is not in the list"
    );
    clean_up(&root);
}

/// Same transition, from the Info ROOT: the keyboard is on the root (nothing
/// pressed inside the panel), whose handle the tab switch also takes out of
/// the tree. The Changes list's first header must get it, and a typed key must
/// not reach the pane.
#[gpui::test]
fn the_changes_action_from_the_info_root_lands_on_the_first_header(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, mut pane, root) = info_on_repo(cx, "info-root-to-changes");
    assert_eq!(info_marker(&mut vcx), None, "the root holds it");

    vcx.dispatch_action(ShowRightPanelChanges);
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        scm_marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "the keyboard is not on the Changes list's first header"
    );
    let _ = pane_input(&mut pane);
    type_key(&mut vcx, "x");
    draw(&app, &mut vcx);
    assert!(
        !pane_input(&mut pane).contains(&b'x'),
        "a typed key reached the pane, so the keyboard is not in the list"
    );
    clean_up(&root);
}

/// PARKED USER MOVE: the keyboard is on a live Info row, the Changes tab is
/// asked for by name, and in the SAME event the user's keyboard goes to the
/// Changes filter field. The entry that was parked for the list must be
/// dropped by its unchanged origin guard and must not move the keyboard back:
/// it stays in the field, no list row holds it, and a typed key reaches the
/// field.
#[gpui::test]
fn a_user_move_while_the_changes_entry_is_parked_is_not_overridden(cx: &mut TestAppContext) {
    let _serial = serial();
    let (app, mut vcx, _pane, root) = info_on_repo(cx, "info-parked-move");
    press(&app, &mut vcx, "down");
    press(&app, &mut vcx, "down");
    assert_eq!(info_marker(&mut vcx).as_deref(), Some("Cwd"));

    let filter = app
        .update_in(&mut vcx, |app, window, cx| {
            app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
            // SETUP, not the property: an entry really is parked for the list
            // at this point. Without it the assertions below would pass for a
            // request that was never made.
            assert!(
                app.scm.entry.is_some(),
                "no entry was parked for the Changes list"
            );
            let filter = app
                .scm
                .filter
                .as_ref()
                .map(|f| f.read(cx).focus_handle(cx))?;
            window.focus(&filter, cx);
            Some(filter)
        })
        .expect("the Changes filter field exists");
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        scm_marker(&mut vcx),
        None,
        "the parked entry took the keyboard from the field"
    );
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(filter),
        "the keyboard is not where the user put it"
    );
    type_key(&mut vcx, "q");
    let typed = app.update_in(&mut vcx, |app, _, cx| {
        app.scm
            .filter
            .as_ref()
            .map(|f| f.read(cx).value().to_string())
    });
    assert_eq!(
        typed.as_deref(),
        Some("q"),
        "typing does not reach the field"
    );
    clean_up(&root);
}
