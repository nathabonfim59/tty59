//! POST-IMPLEMENTATION property test for the typed landing (`Landed`) that is
//! delivered to every window, written after it. It is paired with the real
//! witness `rows_notarepo_tests` (a real NotARepo landing drops an entry parked
//! on an explicit pick); this one covers what that one does not: a window whose
//! waiting entry has NO pick and a resolved lookup mapping, and a window on
//! another repository that must be left alone. A later guard mutation of the
//! repository match, or of the order (forget before judging), is what shows it
//! can fail.
//!
//! HANDLER PROPERTY, honestly labelled: the landing is delivered by calling
//! `scm_landed_in_all_windows_with` directly with `Landed::NotARepo`, inside ONE
//! outer app event that also sets up the windows, so that no real read, lookup
//! or draw can answer first. It does not claim that R stopped being a
//! repository (R is a real repository here), or that a real NotARepo read
//! produces this delivery: that is the paired real test. The lookup mappings
//! are stored by hand exactly as the lookup stores them (`roots[(host, cwd)] =
//! (Instant, Some(root))`), with the real roots.
//!
//! Windows: B's pane is on real repository R, A's on real repository X.
//! Each window records its mapping and asks to enter by the real keyboard path
//! (`show_right_panel_tab`): the entry parks on the mapping (status uncached),
//! with no explicit pick. Then `Landed::NotARepo` for R is delivered, and a
//! `Landed::Status` for X while X is still uncached.
//!
//! * B (entry on R, no pick): the entry is dropped and its mapping to R is
//!   forgotten.
//! * A (entry on X): the entry stays and its mapping to X stays, after both
//!   deliveries (the second is a genuine Loading: it stays parked).

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext, VisualContext as _, VisualTestContext};
use tty59_core::core::config::RightPanelTab;

use super::rows_window_tests::{git, init_repo, scratch, serial, write};
use crate::core::session::Session;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::git_data::Landed;
use crate::ui::app::{Tab, Tty59App, test_window};
use crate::ui::host_ops::HostId;
use crate::ui::pane::{Pane, PaneSlot};
use crate::ui::scm::panel::scm_landed_in_all_windows_with;

type Stream = crate::daemon::transport::Stream;

fn clean_repo(name: &str) -> std::path::PathBuf {
    let root = scratch(name).join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    root
}

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

fn cached(vcx: &mut VisualTestContext, root: &Path) -> bool {
    vcx.update(|_, cx| crate::terminal::git_data::status_of(cx, HostId::LOCAL, root).is_some())
}

/// What one window holds at one point of the outer event.
#[derive(Debug, PartialEq)]
struct Held {
    ticket: Option<u64>,
    mapped_to: Option<std::path::PathBuf>,
}

#[gpui::test]
fn a_not_a_repository_landing_clears_the_matching_unpicked_entry_and_mapping_only(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    crate::core::config::pin_test_config_dir();
    crate::ui::i18n::set_locale("en");
    let r = clean_repo("nrf-r");
    let x = clean_repo("nrf-x");
    let (a, mut va, mut sa) = test_window::harness_with_pane(cx);
    let (b, mut vb, mut sb) = second_window(cx);
    for v in [&mut va, &mut vb] {
        v.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(900.)));
    }
    DaemonMsg::Cwd(x.clone())
        .encode(&mut sa)
        .expect("A's pane takes the cwd");
    DaemonMsg::Cwd(r.clone())
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
    assert_eq!(target(&a, &mut va), Some((HostId::LOCAL, x.clone())));
    assert_eq!(target(&b, &mut vb), Some((HostId::LOCAL, r.clone())));
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
    }
    assert!(
        !cached(&mut va, &r) && !cached(&mut va, &x),
        "neither status is cached"
    );
    let (ha, hb) = (va.window_handle(), vb.window_handle());

    // ONE outer app event: no flush, draw, pump or lookup inside it.
    let (b_start, a_start, b_after, a_after, a_after_loading) = cx.update(|cx| {
        // Each window stores the lookup answer for its own directory as the
        // lookup would, then asks to enter: the entry parks on that mapping
        // (the status is uncached), with no explicit pick.
        let held = |cx: &mut gpui::App,
                    app: &Entity<Tty59App>,
                    handle: gpui::AnyWindowHandle,
                    key: &Path|
         -> Held {
            let key = (HostId::LOCAL, key.to_path_buf());
            handle
                .update(cx, |_, _, cx| {
                    app.update(cx, |app, _| Held {
                        ticket: app.scm.entry.as_ref().map(|e| e.ticket),
                        mapped_to: app.scm.roots.get(&key).and_then(|(_, root)| root.clone()),
                    })
                })
                .expect("the window is open")
        };
        for (app, handle, dir) in [(&a, ha, &x), (&b, hb, &r)] {
            handle
                .update(cx, |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.scm.roots.insert(
                            (HostId::LOCAL, dir.clone()),
                            (Instant::now(), Some(dir.clone())),
                        );
                        app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
                    })
                })
                .expect("the window is open");
        }
        let b_start = held(cx, &b, hb, &r);
        let a_start = held(cx, &a, ha, &x);
        scm_landed_in_all_windows_with(cx, HostId::LOCAL, &r, true, Landed::NotARepo);
        let b_after = held(cx, &b, hb, &r);
        let a_after = held(cx, &a, ha, &x);
        // A genuine Loading for X (uncached, a status answer): stays parked.
        scm_landed_in_all_windows_with(cx, HostId::LOCAL, &x, true, Landed::Status);
        let a_after_loading = held(cx, &a, ha, &x);
        (b_start, a_start, b_after, a_after, a_after_loading)
    });

    assert!(b_start.ticket.is_some(), "B's entry parked on its mapping");
    assert_eq!(b_start.mapped_to, Some(r.clone()), "B's mapping is R");
    assert!(a_start.ticket.is_some(), "A's entry parked on its mapping");
    assert_eq!(a_start.mapped_to, Some(x.clone()), "A's mapping is X");

    assert_eq!(
        b_after.ticket, None,
        "a not-a-repository landing for R left B's unpicked entry waiting"
    );
    assert_eq!(
        b_after.mapped_to, None,
        "B kept its lookup mapping to a root that is not a repository"
    );
    assert_eq!(
        a_after, a_start,
        "a landing for another repository changed A's entry or mapping"
    );
    assert_eq!(
        a_after_loading, a_start,
        "a status landing that found X still loading dropped A's entry or mapping"
    );

    // Let the reads the panels started when they were shown settle, so no git
    // work outlives the test.
    pump(&mut vb, 60, "R's status never landed", |v| cached(v, &r));
    pump(&mut va, 60, "X's status never landed", |v| cached(v, &x));
    for root in [&r, &x] {
        let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
    }
}
