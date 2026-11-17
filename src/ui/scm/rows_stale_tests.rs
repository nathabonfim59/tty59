//! Reviewer test for the keyboard entry's completion path, kept apart from the
//! frozen `rows_keys_tests`.
//!
//! A completion is the answer to ONE request. When it arrives it must only ever
//! consume the request it belongs to: a completion for an older request that is
//! delivered after a newer request has been made must leave the newer request
//! untouched, so the newer one can still be settled by the frame that mounts
//! its row.
//!
//! HONEST LABEL: the old completion is delivered by calling `scm_entry_complete`
//! directly, with a hand-built `Pending` of the older ticket, inside the same
//! event that registers the newer request. This does NOT reproduce how the
//! scheduler could interleave a deferred completion with a later request, and
//! the test does not claim to. It pins the one property that matters for any
//! such interleaving: delivering a stale completion never drops, replaces or
//! settles a newer request.

use gpui::TestAppContext;

use super::rows_window_tests::{big_repo, draw, header, panel_on, serial};
use crate::ui::roving::{Pending, PendingKind};
use crate::ui::scm::state::ScmGroup;

/// What the keyboard holds, read from the live dispatch tree: the `row` of the
/// `ScmRows` key context on the focus path (the contract the frozen tests use).
fn marker(vcx: &mut gpui::VisualTestContext) -> Option<String> {
    vcx.update(|window, _| {
        window
            .context_stack()
            .iter()
            .find(|c| c.contains("ScmRows"))
            .and_then(|c| c.get("row"))
            .map(|v| v.to_string())
    })
}

#[cfg(unix)]
#[gpui::test]
fn a_stale_completion_leaves_a_newer_entry_in_place_to_be_settled(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("stale-entry");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    let terminal = vcx.update(|window, cx| window.focused(cx));

    // One event: request A, request B (which replaces A), then A's completion
    // is delivered. `scm_request_entry` registers; nothing here draws.
    let (older, newer, kept, focused_after) = app.update_in(&mut vcx, |app, window, cx| {
        app.scm_request_entry(window, cx);
        let older = app.scm.entry.as_ref().map(|e| e.ticket);
        let stale = Pending {
            ticket: app.scm.nav_gen,
            window: window.window_handle(),
            origin: window.focused(cx),
            kind: PendingKind::Entry,
        };
        app.scm_request_entry(window, cx);
        let newer = app.scm.entry.as_ref().map(|e| e.ticket);
        app.scm_entry_complete(stale, window, cx);
        let kept = app.scm.entry.as_ref().map(|e| e.ticket);
        (older, newer, kept, window.focused(cx))
    });
    assert!(older.is_some(), "the first request was registered");
    assert!(newer.is_some(), "the second request was registered");
    assert_ne!(older, newer, "the second request has its own ticket");
    assert_eq!(
        kept, newer,
        "delivering the older request's completion dropped or replaced the newer entry"
    );
    assert_eq!(
        focused_after, terminal,
        "a stale completion moved the keyboard"
    );

    // The newer request is still waiting, so the normal flow settles it: the
    // frame that mounts the first header publishes the mount, takes the
    // request and completes it.
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert_eq!(
        marker(&mut vcx).as_deref(),
        Some(header(ScmGroup::Merge)),
        "the newer entry was never settled by the fresh mount"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(&root));
}
