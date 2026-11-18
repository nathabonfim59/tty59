//! Reviewer test: a keyboard entry parked on an EXPLICIT repository pick must be
//! dropped when that repository's read lands as "not a repository".
//!
//! The approved policy is that a clean, not-a-repository or failed list has
//! nothing to enter and that its waiting entry cannot resurface when something
//! later puts a row there. A pick bypasses the directory lookup (the entry's
//! repository is the pick), so the lookup path that drops an entry for a
//! directory that is not a repository does not run for it, and the landing is
//! the only chance. A NotARepo landing removes the status and records a
//! definitive read (`ScmData::known_status == Some(None)`), which
//! `status_read_of` still reads as Loading.
//!
//! SOURCE FACTS (not proved here): `scm_landed` drops an entry only for a Ready
//! status with no rows or a Failed read, and keeps it for Loading; the
//! root-lookup closure judges an entry only when it has no pick.
//!
//! WHAT THE TESTS DO. Two real windows whose panes are on the same real
//! directory R, panels hidden. ONE outer app event, with no flush, draw or pump
//! inside it, samples that nothing is known or cached for R, has window A start
//! the actual read (`scm_refresh`), samples again that it has not landed, and has
//! window B ask to enter with R as its explicit pick (parking the entry,
//! capturing its origin focus). Then it waits for the read to land (the
//! fixture's `known_status` says a read of R has completed) and reads the
//! outcome afterwards.
//!
//! * `an_explicit_pick_on_a_directory_that_stops_being_a_repository ...`:
//!   R was a real repository and its `.git` is removed before the read, so the
//!   landing is a real NotARepo (asserted: `known_status == Some(None)`).
//!   Expected RED today: the entry is still waiting after the landing.
//! * `an_explicit_pick_on_a_clean_repository ...`: the same scenario with `.git`
//!   kept: a clean Ready landing. Control, expected GREEN today (the entry is
//!   dropped).
//!
//! It does not claim which fix is right, and the wait is only for the git read.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Entity, FocusHandle, TestAppContext, VisualContext as _, VisualTestContext,
};
use tty59_core::core::config::RightPanelTab;

use super::rows_window_tests::{git, init_repo, scratch, serial, write};
use crate::core::session::Session;
use crate::daemon::protocol::DaemonMsg;
use crate::terminal::git_data::ScmData;
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

fn clean_repo(name: &str) -> std::path::PathBuf {
    let root = scratch(name).join("repo");
    std::fs::create_dir_all(&root).unwrap();
    init_repo(&root);
    write(&root, "a.txt", "a\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "a"]);
    root
}

/// A second window built the way `harness_configured` builds its window,
/// without redoing the globals, with a quiet pane of its own.
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

/// The three-valued answer the data layer records for a repository: `None`
/// never read, `Some(None)` read and there is no repository, `Some(Some(_))`
/// read and there is a status.
fn known(cx: &gpui::App, root: &Path) -> Option<Option<()>> {
    cx.try_global::<ScmData>()?
        .known_status(HostId::LOCAL, root)
        .map(|status| status.map(|_| ()))
}

struct Observed {
    origin: Option<FocusHandle>,
    ticket_parked: Option<u64>,
    known_after: Option<Option<()>>,
    entry_after: bool,
    marker_after: Option<String>,
    focus_after: Option<FocusHandle>,
}

fn scenario(cx: &mut TestAppContext, name: &str, remove_git: bool) -> Observed {
    crate::core::config::pin_test_config_dir();
    crate::ui::i18n::set_locale("en");
    let root = clean_repo(name);
    assert!(root.join(".git").exists(), "R starts as a real repository");
    if remove_git {
        std::fs::remove_dir_all(root.join(".git")).expect("the repository's .git is removed");
        assert!(!root.join(".git").exists(), "R is no longer a repository");
    }
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
    pump(&mut va, 30, "A's pane never reported its directory", |v| {
        target(&a, v).is_some()
    });
    pump(&mut vb, 30, "B's pane never reported its directory", |v| {
        target(&b, v).is_some()
    });
    let want = Some((HostId::LOCAL, root.clone()));
    assert_eq!(target(&a, &mut va), want, "A is on R");
    assert_eq!(target(&b, &mut vb), want, "B is on R");
    for (app, vcx) in [(&a, &mut va), (&b, &mut vb)] {
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
    }
    let hb = vb.window_handle();
    let host = va.update(|_, cx| HostRegistry::get(cx, HostId::LOCAL).expect("the local host"));
    let pick = RepoKey {
        host: HostId::LOCAL,
        root: root.clone(),
    };

    // ONE outer app event: nothing can land between its steps.
    let (known_before, known_after_call, origin, ticket_parked) = cx.update(|cx| {
        let known_before = known(cx, &root);
        let cached_before =
            crate::terminal::git_data::status_of(cx, HostId::LOCAL, &root).is_some();
        a.update(cx, |app, cx| {
            app.scm_refresh(host.clone(), root.clone(), cx)
        });
        let known_after_call = known(cx, &root);
        let (origin, ticket_parked) = hb
            .update(cx, |_, window, cx| {
                b.update(cx, |app, cx| {
                    app.scm.repo_override = Some(pick.clone());
                    app.scm.override_tab = Some(app.active);
                    let origin = window.focused(cx);
                    app.show_right_panel_tab(RightPanelTab::Scm, window, cx);
                    (origin, app.scm.entry.as_ref().map(|e| e.ticket))
                })
            })
            .expect("B's window is open");
        assert!(!cached_before, "nothing is cached for R before the read");
        (known_before, known_after_call, origin, ticket_parked)
    });
    assert_eq!(known_before, None, "R had never been read");
    assert_eq!(
        known_after_call, None,
        "the read A started has not landed when B asks"
    );
    assert!(origin.is_some(), "B had a real focus to protect");
    assert!(
        ticket_parked.is_some(),
        "B's entry parked on its explicit pick: R's status was not known"
    );

    // Wait for the read of R to land (fixture: the data layer records it).
    pump(&mut vb, 60, "the read of R never landed", |v| {
        v.update(|_, cx| known(cx, &root).is_some())
    });
    vb.run_until_parked();
    let known_after = vb.update(|_, cx| known(cx, &root));
    let entry_after = b.read_with(&vb, |app, _| app.scm.entry.is_some());
    let marker_after = marker(&mut vb);
    let focus_after = vb.update(|window, cx| window.focused(cx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
    Observed {
        origin,
        ticket_parked,
        known_after,
        entry_after,
        marker_after,
        focus_after,
    }
}

/// R was a repository and is not one when the read lands: the entry parked on
/// the explicit pick must be dropped, and the keyboard must not have moved.
/// Expected RED today: a NotARepo landing reads as Loading, which keeps it.
#[gpui::test]
fn an_explicit_pick_on_a_directory_that_stops_being_a_repository_is_dropped(
    cx: &mut TestAppContext,
) {
    let _serial = serial();
    let o = scenario(cx, "notarepo-pick", true);
    assert_eq!(
        o.known_after,
        Some(None),
        "the landing was a real NotARepo: a read is recorded and holds no status"
    );
    assert!(o.ticket_parked.is_some());
    assert!(
        !o.entry_after,
        "a NotARepo landing left the entry waiting on the explicit pick"
    );
    assert_eq!(o.marker_after, None);
    assert_eq!(o.focus_after, o.origin, "the keyboard moved");
}

/// Control: the same scenario with the repository kept, so the landing is a
/// clean Ready status with nothing to enter. Expected GREEN today.
#[gpui::test]
fn an_explicit_pick_on_a_clean_repository_is_dropped_by_its_landing(cx: &mut TestAppContext) {
    let _serial = serial();
    let o = scenario(cx, "clean-pick", false);
    assert_eq!(
        o.known_after,
        Some(Some(())),
        "the landing was a Ready status"
    );
    assert!(o.ticket_parked.is_some());
    assert!(
        !o.entry_after,
        "a clean landing left the entry waiting on the explicit pick"
    );
    assert_eq!(o.marker_after, None);
    assert_eq!(o.focus_after, o.origin, "the keyboard moved");
}
