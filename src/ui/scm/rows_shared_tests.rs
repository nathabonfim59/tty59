//! Reviewer tests for a keyboard entry waiting in one window while a status
//! landing is started by ANOTHER window on the same repository.
//!
//! SOURCE FACTS (not proved by the tests): every window is its own `Tty59App`
//! with its own `ScmPanelState`, all reading one global `ScmData`. A read is
//! started once per repository (`probes.begin`); its landing runs
//! `scm_landed` on the app that started it only, while `refresh_windows`
//! redraws every window. So a waiting entry in a window that did NOT start the
//! landing is not given the chance to be dropped when the landing says there is
//! nothing to enter.
//!
//! WHAT THE TESTS DO. Two real windows, each with a quiet pane whose directory
//! is the same real repository. With nothing cached and both panels hidden
//! (so nothing else can start a read), ONE outer app event (`TestAppContext::update`,
//! with no flush, draw or pump inside it, each window reached through its own
//! `AnyWindowHandle::update`) does all of: sample the status as uncached, sample
//! that no read of the repository is running (`ScmData::read_in_flight`, a
//! test-only accessor), have window A start the read with `scm_refresh`, sample
//! that one is running now (A's call began it: the direct initiator witness) and
//! that nothing is cached yet, and have the waiting window ask to enter through
//! the real keyboard path (`show_right_panel_tab`), sampling the focus it has at
//! that moment (the origin) and that its entry is parked. Nothing can land
//! between those steps.
//!
//! A's `scm.repo` is set by hand before the event (the repository A's list is
//! for, which its render would store; not a pick). A's `layout_epoch` is sampled
//! at the start and the end of the event (it must not move: neither the call nor
//! the request bumps it) and again after the landing (it must have moved). That
//! is a DELIVERY-TO-A'S-LIST witness only: a window's epoch moves for a landing
//! when its list is for the landed repository, whichever window started the
//! read, so it says nothing about who the initiator was; `read_in_flight` does.
//! The repository is clean, so there is nothing to enter; the test then makes it
//! dirty and has A start a second read, the bump and the call again in one outer
//! event.
//!
//! * `the initiating window ...` parks the entry in A. Control: it passes on
//!   today's code (`scm_landed` clears it).
//! * `a window that did not start the landing ...` parks the entry in B.
//!   The wanted behaviour: the clean landing drops it, and the later dirty
//!   landing does not pull B's keyboard into the list. Expected RED today.
//!
//! WHAT THEY DO NOT PROVE: that a user reaches this path in practice, or which
//! fix is right. No timer is used for the behaviour; the only waiting is for the
//! git read to land and for the waiting window's own root lookup to answer (a
//! fixture barrier: a waiting entry is judged at both, so the outcome does not
//! depend on which came first), as every status fixture does. Whether the entry
//! was dropped is only ever read afterwards, never polled.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Entity, FocusHandle, TestAppContext, VisualContext as _, VisualTestContext,
};
use tty59_core::core::config::RightPanelTab;

use super::rows_window_tests::{git, scratch, serial, write};
use crate::core::session::Session;
use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::{Tab, Tty59App, test_window};
use crate::ui::host_ops::HostId;
use crate::ui::host_registry::HostRegistry;
use crate::ui::pane::{Pane, PaneSlot};
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

/// A real repository with one committed file and nothing changed.
fn clean_repo(name: &str) -> std::path::PathBuf {
    let root = scratch(name).join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    root
}

/// A second window in the same test app, built the way `harness_configured`
/// builds its window, without redoing the globals the first window already set,
/// and with a quiet pane of its own.
fn second_window(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext, Stream) {
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
        let (view, stream) = quiet_test_pane(2, window, cx);
        app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        app.active = 0;
        cx.notify();
        stream
    });
    vcx.background_executor.run_until_parked();
    (app, vcx, stream)
}

fn cached(vcx: &mut VisualTestContext, root: &Path) -> bool {
    vcx.update(|_, cx| crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_some())
}

fn clean(vcx: &mut VisualTestContext, root: &Path) -> Option<bool> {
    vcx.update(|_, cx| {
        crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).map(|s| s.is_clean())
    })
}

/// Whether `app`'s root lookup for its pane directory has answered with the
/// real repository root (`ScmPanelState::roots`).
fn lookup_resolved(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, root: &Path) -> bool {
    app.update_in(vcx, |app, _, _| {
        matches!(
            app.scm.roots.get(&(HostId::LOCAL, root.to_path_buf())),
            Some((_, Some(resolved))) if resolved.as_path() == root
        )
    })
}

/// Pump until `done`, or fail after `secs`. Waits for a git read to land.
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

/// What one outer app event sampled: no flush, draw or pump ran inside it.
struct Sampled {
    cached_before: bool,
    cached_after_call: bool,
    /// No read of the repository was running before A's call, and one is
    /// right after it: A's call is the one that began the read.
    in_flight_before: bool,
    in_flight_after_call: bool,
    epoch_before: u64,
    epoch_after_event: u64,
    origin: Option<FocusHandle>,
    entry: bool,
}

/// What happened to the waiting window.
struct Outcome {
    origin: Option<FocusHandle>,
    epoch_moved_by_landing: bool,
    entry_after_clean: bool,
    marker_after_clean: Option<String>,
    focus_after_clean: Option<FocusHandle>,
    marker_after_dirty: Option<String>,
    focus_after_dirty: Option<FocusHandle>,
}

/// The whole scenario, with the entry waiting in B (`in_b`) or in A.
fn scenario(cx: &mut TestAppContext, name: &str, in_b: bool) -> Outcome {
    crate::core::config::pin_test_config_dir();
    crate::ui::i18n::set_locale("en");
    let root = clean_repo(name);
    assert!(root.join(".git").exists(), "a real repository");
    let (a, mut va, mut sa) = test_window::harness_with_pane(cx);
    let (b, mut vb, mut sb) = second_window(cx);
    for v in [&mut va, &mut vb] {
        v.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    }
    DaemonMsg::Cwd(root.clone())
        .encode(&mut sa)
        .expect("A's pane takes the cwd");
    DaemonMsg::Cwd(root.clone())
        .encode(&mut sb)
        .expect("B's pane takes the cwd");
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, _, cx| {
            app.right_panel_visible = false;
            app.right_panel_tab = RightPanelTab::Info;
            cx.notify();
        });
    }
    // Both panes report the same real root.
    pump(&mut va, 30, "A's pane never reported its directory", |v| {
        target(&a, v).is_some()
    });
    pump(&mut vb, 30, "B's pane never reported its directory", |v| {
        target(&b, v).is_some()
    });
    let (ta, tb) = (target(&a, &mut va), target(&b, &mut vb));
    assert_eq!(
        ta,
        Some((HostId::LOCAL, root.clone())),
        "A targets the repository"
    );
    assert_eq!(tb, ta, "B targets the same directory as A");
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
    }

    // No panel is visible, so nothing but A's explicit call can start the read.
    let visible = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| app.right_panel_visible)
    };
    assert!(
        !visible(&a, &mut va) && !visible(&b, &mut vb),
        "no panel can start a read"
    );
    let (ha, hb) = (va.window_handle(), vb.window_handle());
    let host = va.update(|_, cx| HostRegistry::get(cx, HostId::LOCAL).expect("the local host"));
    // FIXTURE STATE, set before the event and while A's panel is hidden: the
    // repository A's list is for (not an explicit pick: `repo_override` stays
    // unset, so a waiting A still parks through the lookup path). It is what
    // A's render stores once its own root lookup answers (panel.rs sets
    // `scm.repo` there; other fixtures set it by hand the same way), and a
    // window's epoch moves for a landing only when its list is for the landed
    // repository, so without it A's epoch could not show the delivery. B's
    // state, the unknown-root request and the single-event sampling are
    // untouched by it.
    a.update_in(&mut va, |app, _, _| {
        app.scm.repo = Some(RepoKey {
            host: HostId::LOCAL,
            root: root.clone(),
        });
    });
    let read_in_flight = |cx: &gpui::App| {
        cx.try_global::<crate::terminal::git_data::ScmData>()
            .is_some_and(|data| data.read_in_flight(HostId::LOCAL, &root))
    };

    // ONE outer app event, with no flush, draw or pump inside it, so that no
    // landing can come between the steps: the uncached check, A starting the
    // read, the same check right after, and the waiting window asking to enter
    // with its origin focus and its parked entry sampled. Each window is
    // updated through its own window handle (the shape `windows::open_at_tab`
    // uses): `AnyWindowHandle::update` hands out the `Window`, and the app
    // entity is updated inside it.
    let sampled = cx.update(|cx| {
        let cached_before =
            crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root).is_some();
        let epoch_before = a.read(cx).scm.layout_epoch;
        let in_flight_before = read_in_flight(cx);
        a.update(cx, |app, cx| {
            app.scm_refresh(host.clone(), root.clone(), cx)
        });
        let in_flight_after_call = read_in_flight(cx);
        let cached_after_call =
            crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root).is_some();
        let (waiting, handle) = if in_b { (&b, hb) } else { (&a, ha) };
        let (origin, entry) = handle
            .update(cx, |_, window, cx| {
                waiting.update(cx, |app, cx| {
                    let origin = window.focused(cx);
                    app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
                    (origin, app.scm.entry.is_some())
                })
            })
            .expect("the waiting window is open");
        let epoch_after_event = a.read(cx).scm.layout_epoch;
        Sampled {
            cached_before,
            cached_after_call,
            in_flight_before,
            in_flight_after_call,
            epoch_before,
            epoch_after_event,
            origin,
            entry,
        }
    });
    assert!(
        !sampled.cached_before,
        "the status is uncached before A starts the read"
    );
    assert!(
        !sampled.cached_after_call,
        "the read A started has not landed when the entry is requested"
    );
    // Direct witness that A is the window that started the read: none was
    // running before A's call, one is right after it. (Not an outcome oracle:
    // it says nothing about what the landing does.)
    assert!(
        !sampled.in_flight_before,
        "a read of the repository was already running before A's call"
    );
    assert!(
        sampled.in_flight_after_call,
        "A's scm_refresh did not begin a read"
    );
    assert!(
        sampled.origin.is_some(),
        "the waiting window had a real focus to protect"
    );
    assert!(
        sampled.entry,
        "the entry is waiting: the status was not cached when it was asked"
    );
    // Neither starting the read nor asking to enter moves A's epoch.
    assert_eq!(
        sampled.epoch_after_event, sampled.epoch_before,
        "starting the read or requesting the entry moved A's layout epoch"
    );
    let origin = sampled.origin;

    // The clean landing.
    let wvcx = if in_b { &mut vb } else { &mut va };
    pump(wvcx, 60, "the clean status never landed", |v| {
        cached(v, &root)
    });
    assert_eq!(clean(wvcx, &root), Some(true), "the repository is clean");
    // FIXTURE BARRIER, not a behaviour wait: the waiting window's own root
    // lookup has answered with the real root. A waiting entry is judged at
    // both the status landing and that lookup answer, so once both have
    // happened the outcome below does not depend on which came first. Only the
    // lookup's completion is waited for; whether the entry was dropped is read
    // afterwards, never polled.
    let waiting_app = if in_b { &b } else { &a };
    pump(
        wvcx,
        60,
        "the waiting window's root lookup never answered",
        |v| lookup_resolved(waiting_app, v, &root),
    );
    wvcx.run_until_parked();
    // The epoch moved BECAUSE of the landing: it did not move in the event
    // above, and does now that the read A started has landed.
    let epoch_moved_by_landing =
        a.read_with(&va, |app, _| app.scm.layout_epoch) > sampled.epoch_after_event;
    let (waiting, wvcx) = if in_b { (&b, &mut vb) } else { (&a, &mut va) };
    let entry_after_clean = waiting.update_in(wvcx, |app, _, _| app.scm.entry.is_some());
    let marker_after_clean = marker(wvcx);
    let focus_after_clean = wvcx.update(|window, cx| window.focused(cx));

    // Then the repository gets a change, and A starts a second read. The bump
    // and A's call are one outer event too: a draw between them (the waiting
    // window shows the list now) could let that window start the read instead.
    write(&root, "a.txt", "changed\n");
    cx.update(|cx| {
        cx.default_global::<crate::terminal::git_data::ScmData>()
            .bump(HostId::LOCAL, &root);
        a.update(cx, |app, cx| {
            app.scm_refresh(host.clone(), root.clone(), cx)
        });
    });
    let (waiting, wvcx) = if in_b { (&b, &mut vb) } else { (&a, &mut va) };
    pump(wvcx, 60, "the dirty status never landed", |v| {
        clean(v, &root) == Some(false)
    });
    for _ in 0..3 {
        waiting.update_in(wvcx, |_, _, cx| cx.notify());
        wvcx.run_until_parked();
    }
    let marker_after_dirty = marker(wvcx);
    let focus_after_dirty = wvcx.update(|window, cx| window.focused(cx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
    Outcome {
        origin,
        epoch_moved_by_landing,
        entry_after_clean,
        marker_after_clean,
        focus_after_clean,
        marker_after_dirty,
        focus_after_dirty,
    }
}

fn expect_dropped_and_never_resurfaces(o: &Outcome) {
    assert!(
        o.epoch_moved_by_landing,
        "the landing was not delivered to A's list: its layout epoch did not move"
    );
    assert!(
        !o.entry_after_clean,
        "the clean landing left the waiting entry in place"
    );
    assert_eq!(o.marker_after_clean, None);
    assert_eq!(
        o.focus_after_clean, o.origin,
        "the clean landing moved the keyboard"
    );
    assert_eq!(
        o.marker_after_dirty, None,
        "a later dirty landing pulled the keyboard into the list"
    );
    assert_eq!(
        o.focus_after_dirty, o.origin,
        "a later dirty landing moved the keyboard"
    );
}

/// Control: the window that started the landing drops its waiting entry on the
/// clean landing, and the dirty landing does not resurrect it. Passes today.
#[cfg(unix)]
#[gpui::test]
fn the_initiating_window_drops_a_waiting_entry_on_a_clean_landing(cx: &mut TestAppContext) {
    let _serial = serial();
    let outcome = scenario(cx, "shared-initiator", false);
    expect_dropped_and_never_resurfaces(&outcome);
}

/// A window that did NOT start the landing: its waiting entry must also be
/// dropped by the clean landing, so a later dirty landing cannot pull its
/// keyboard into the list. Expected RED today.
#[cfg(unix)]
#[gpui::test]
fn a_window_that_did_not_start_the_landing_drops_a_waiting_entry_too(cx: &mut TestAppContext) {
    let _serial = serial();
    let outcome = scenario(cx, "shared-noninitiator", true);
    expect_dropped_and_never_resurfaces(&outcome);
}
