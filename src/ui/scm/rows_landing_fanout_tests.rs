//! POST-IMPLEMENTATION tests for delivering a landed status read to every
//! window (`scm_landed_in_all_windows`), written after it. They are not RED-first
//! tests: the frozen `rows_shared_tests` are the RED-first witness of the
//! defect (a window that did not start the landing kept its waiting entry), and
//! this file pins the properties the fix must keep. A later guard mutation of the
//! repository match in `scm_landed` is what shows these can fail.
//!
//! HANDLER PROPERTIES, honestly labelled: where a test calls
//! `scm_landed_in_all_windows` directly, inside one outer app event and before
//! any draw, it exercises the delivery handler on purpose, so that no real
//! landing (or a window of its own) can answer first. It does not claim to
//! reproduce a real landing for another repository; the real landing is the
//! frozen shared test and the final step of the first test here (a real clean
//! landing of the waiting window's own repository).

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, Render, TestAppContext,
    VisualContext as _, VisualTestContext, Window,
};
use tty59_core::core::config::RightPanelTab;

use super::rows_window_tests::{git, scratch, serial, write};
use crate::core::session::Session;
use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::{Tab, Tty59App, test_window};
use crate::ui::host_ops::HostId;
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::scm::panel::scm_landed_in_all_windows;
use crate::ui::scm::state::RepoKey;

type Stream = crate::daemon::transport::Stream;

fn marker(vcx: &mut VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("ScmRows"))
            .and_then(|c| c.get("row"))
            .map(|v| v.to_string())
    })
}

fn clean_repo(name: &str) -> std::path::PathBuf {
    let root = scratch(name).join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    root
}

/// A second window built the way `harness_configured` builds its window,
/// without redoing the globals, with a quiet pane of its own.
fn second_window(
    cx: &mut TestAppContext,
    pane_id: u64,
) -> (Entity<Tty59App>, VisualTestContext, Stream) {
    use crate::terminal::view::quiet_test_pane;

    let window = cx.add_window(|window, cx| {
        let app = cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
        gpui_component::Root::new(app, window, cx)
    });
    cx.background_executor.run_until_parked();
    let app = window
        .update(cx, |root, _, _| {
            root.view()
                .clone()
                .downcast::<Tty59App>()
                .ok()
                .expect("window root wraps a Tty59App")
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let stream = app.update_in(&mut vcx, |app, window, cx| {
        let (view, stream) = quiet_test_pane(pane_id, window, cx);
        app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        app.active = 0;
        cx.notify();
        stream
    });
    vcx.background_executor.run_until_parked();
    (app, vcx, stream)
}

/// A window whose root is not an app: not a `Root` around a `Tty59App`.
struct Bare;

impl Render for Bare {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

fn cached(vcx: &mut VisualTestContext, root: &Path) -> bool {
    vcx.update(|_, cx| crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_some())
}

fn pump(
    vcx: &mut VisualTestContext,
    secs: u64,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        vcx.run_until_parked();
        if done(vcx) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn target(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
) -> Option<(HostId, std::path::PathBuf)> {
    app.update_in(vcx, |app, window, cx| {
        cx.notify();
        app.scm_pane_target(window, cx)
            .map(|(host, cwd)| (host.id(), cwd))
    })
}

/// What a window held at one point of one outer event.
#[derive(Debug, PartialEq)]
struct Held {
    ticket: Option<u64>,
    epoch: u64,
    focus: Option<FocusHandle>,
}

fn pick(root: &Path) -> RepoKey {
    RepoKey {
        host: HostId::LOCAL,
        root: root.to_path_buf(),
    }
}

/// Two windows, each with a quiet pane on its own repository, panels hidden,
/// both reporting the directory they were given and holding the keyboard.
fn two_windows(
    cx: &mut TestAppContext,
    root_a: &Path,
    root_b: &Path,
) -> (
    (Entity<Tty59App>, VisualTestContext),
    (Entity<Tty59App>, VisualTestContext),
) {
    crate::core::config::pin_test_config_dir();
    crate::ui::i18n::set_locale("en");
    let (a, mut va, mut sa) = test_window::harness_with_pane(cx);
    let (b, mut vb, mut sb) = second_window(cx, 2);
    for v in [&mut va, &mut vb] {
        v.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    }
    DaemonMsg::Cwd(root_a.to_path_buf())
        .encode(&mut sa)
        .expect("A's pane takes the cwd");
    DaemonMsg::Cwd(root_b.to_path_buf())
        .encode(&mut sb)
        .expect("B's pane takes the cwd");
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_visible = false;
            app.right_panel_tab = RightPanelTab::Info;
            cx.notify();
        });
    }
    pump(&mut va, 30, "A's pane never reported its directory", |v| {
        target(&a, v).is_some()
    });
    pump(&mut vb, 30, "B's pane never reported its directory", |v| {
        target(&b, v).is_some()
    });
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
    }
    ((a, va), (b, vb))
}

/// A landing for repository X, delivered to every window while window B waits
/// to enter repository Y, leaves B's entry and B's remembered positions alone;
/// a landing for Y that reaches B while Y is still loading leaves the entry
/// waiting but does invalidate B's positions (it is B's repository); and the
/// real clean landing of Y then drops it, with the keyboard where it was.
///
/// The two deliveries are handler calls, in one outer event with the request
/// and before any draw, so that no real landing can answer first (see the file
/// header). Y's repository comes from an explicit pick stored in the entry, not
/// from a lookup that could still be pending.
#[cfg(unix)]
#[gpui::test]
fn a_landing_for_another_repository_leaves_a_parked_entry_alone(cx: &mut TestAppContext) {
    let _serial = serial();
    let x = clean_repo("fanout-x");
    let y = clean_repo("fanout-y");
    let ((_a, _va), (b, mut vb)) = two_windows(cx, &x, &y);
    assert_eq!(
        target(&b, &mut vb),
        Some((HostId::LOCAL, y.clone())),
        "B's pane is on Y"
    );
    assert!(
        !cached(&mut vb, &y) && !cached(&mut vb, &x),
        "neither status is cached"
    );
    let hb = vb.window_handle();
    let held = |cx: &mut gpui::App| -> Held {
        hb.update(cx, |_, window, cx| {
            b.update(cx, |app, cx| Held {
                ticket: app.scm.entry.as_ref().map(|e| e.ticket),
                epoch: app.scm.layout_epoch,
                focus: window.focused(cx),
            })
        })
        .expect("B's window is open")
    };
    let (start, after_x, after_y) = cx.update(|cx| {
        hb.update(cx, |_, window, cx| {
            b.update(cx, |app, cx| {
                app.scm.repo_override = Some(pick(&y));
                app.scm.override_tab = Some(app.active);
                app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
            })
        })
        .expect("B's window is open");
        let start = held(cx);
        scm_landed_in_all_windows(cx, HostId::LOCAL, &x, true);
        let after_x = held(cx);
        scm_landed_in_all_windows(cx, HostId::LOCAL, &y, true);
        let after_y = held(cx);
        (start, after_x, after_y)
    });
    assert!(start.ticket.is_some(), "B's entry is parked on Y");
    assert!(start.focus.is_some(), "B had a real focus to protect");
    assert_eq!(
        after_x, start,
        "a landing for another repository changed B's entry, epoch or focus"
    );
    assert_eq!(
        after_y.ticket, start.ticket,
        "a landing for Y that found Y still loading dropped the entry"
    );
    assert_eq!(after_y.focus, start.focus, "the keyboard moved");
    assert!(
        after_y.epoch > start.epoch,
        "a landing for B's own repository did not reach B"
    );

    // The real clean landing of Y: the panel is up, so B's own render starts
    // the read, and the landing drops the entry for good.
    pump(&mut vb, 60, "Y's status never landed", |v| cached(v, &y));
    vb.run_until_parked();
    let end = cx.update(|cx| held(cx));
    assert_eq!(end.ticket, None, "the clean landing of Y left the entry");
    assert_eq!(marker(&mut vb), None);
    assert_eq!(end.focus, start.focus, "the keyboard moved");
    for root in [&x, &y] {
        let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
    }
}

/// A window that has been closed is not enumerated, so it is not told: its app
/// entity can still be alive here (it is held), and its state must not move,
/// while a live window still is told.
#[cfg(unix)]
#[gpui::test]
fn a_closed_window_is_skipped_and_the_others_are_still_told(cx: &mut TestAppContext) {
    let _serial = serial();
    let x = clean_repo("fanout-closed-x");
    let y = clean_repo("fanout-closed-y");
    let ((_a, _va), (b, mut vb)) = two_windows(cx, &x, &y);
    let (c, mut vc, _sc) = second_window(cx, 3);
    for (app, vcx) in [(&b, &mut vb), (&c, &mut vc)] {
        app.update_in(vcx, |app, _, _| {
            app.scm.repo_override = Some(pick(&y));
            app.scm.override_tab = Some(app.active);
        });
    }
    // Close the third window for real: `remove_window` marks it and the next
    // effect flush drops it from the app's window list.
    vc.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        2,
        "the closed window is gone from the app's window list"
    );
    let before_b = b.read_with(&vb, |app, _| app.scm.layout_epoch);
    let before_c = c.read_with(&vb, |app, _| app.scm.layout_epoch);
    cx.update(|cx| scm_landed_in_all_windows(cx, HostId::LOCAL, &y, true));
    assert!(
        b.read_with(&vb, |app, _| app.scm.layout_epoch) > before_b,
        "the live window was not told"
    );
    assert_eq!(
        c.read_with(&vb, |app, _| app.scm.layout_epoch),
        before_c,
        "the closed window was told"
    );
    for root in [&x, &y] {
        let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
    }
}

/// A window whose root is not an app (not a `Root` around a `Tty59App`) is
/// skipped without a panic, and the app windows are still told.
#[cfg(unix)]
#[gpui::test]
fn a_window_that_is_not_an_app_is_skipped_and_the_others_are_still_told(cx: &mut TestAppContext) {
    let _serial = serial();
    let x = clean_repo("fanout-bare-x");
    let y = clean_repo("fanout-bare-y");
    let ((_a, _va), (b, mut vb)) = two_windows(cx, &x, &y);
    b.update_in(&mut vb, |app, _, _| {
        app.scm.repo_override = Some(pick(&y));
        app.scm.override_tab = Some(app.active);
    });
    let _bare = cx.add_window(|_, _| Bare);
    cx.run_until_parked();
    assert_eq!(
        cx.update(|cx| cx.windows().len()),
        3,
        "the app windows and the bare window are all open"
    );
    let before = b.read_with(&vb, |app, _| app.scm.layout_epoch);
    cx.update(|cx| scm_landed_in_all_windows(cx, HostId::LOCAL, &y, true));
    assert!(
        b.read_with(&vb, |app, _| app.scm.layout_epoch) > before,
        "the app window was not told past the bare one"
    );
    for root in [&x, &y] {
        let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
    }
}
