//! REBASE-INTEGRATION control for the Info table's first keyboard stop.
//!
//! This is a NEW requirement that arrives with the rebase onto the current
//! `main`, not a regression of the accepted 785 work: `main` adds a copyable
//! Pane address row BEFORE the working directory, so the first row Down enters
//! on the Info table is the pane's address and Enter on it puts exactly
//! `%<pane_id>` on the clipboard. The 785 foundation has no such row: its first
//! row is the working directory, so this test is expected to FAIL there, and to
//! fail at its OWN clipboard assertion (the clipboard then holds the directory),
//! never earlier. Nothing here names the Pane row's key: the stop that Down
//! reached is observed only through what Enter does to the clipboard, so a
//! marker assertion cannot mask that failure.
//!
//! What is real: the pane's working directory is delivered over its own
//! stream, the panel is asked for by the real `ShowRightPanelInfo` action, and
//! Down and Enter are real keystrokes through the window. What is read from the
//! pane's own entity rather than from the panel: the pane's id, which is the
//! oracle for the address. The clipboard is the readable test clipboard, put
//! to a sentinel before the press so that "nothing copied" is an observation.

use gpui::{TestAppContext, VisualTestContext};

use super::*;
use crate::core::actions::ShowRightPanelInfo;
use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::test_window;

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tty59-rebase-info-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // `canonicalize` answers `\\?\C:\…` on Windows, while the pane's copy tile
    // writes the plain native spelling, so the directory the pane is sent (and
    // the test compares against) is handed out in the plain local spelling.
    tty59_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(&dir).unwrap())
}

fn draw(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |_, _, cx| cx.notify());
    vcx.run_until_parked();
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

fn press(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext, key: &str) {
    vcx.simulate_keystrokes(key);
    draw(app, vcx);
}

/// The id of the pane the active tab shows, read from the pane's OWN entity and
/// not from the panel's rows: the panel is what is being checked against it.
fn pane_id_of_entity(app: &gpui::Entity<Tty59App>, vcx: &mut VisualTestContext) -> u64 {
    app.update_in(vcx, |app, window, cx| {
        app.tabs
            .get(app.active)
            .and_then(|tab| tab.detail_pane(window, cx))
            .map(|leaf| leaf.read(cx).pane_id)
    })
    .expect("the active tab has a pane entity")
}

/// Down from the Info root enters the first row, and Enter on it copies the
/// pane's address, `%` and the pane's own id, and nothing else.
///
/// The fixture is the second of two tabs, so its pane id is neither the first
/// pane's nor its tab's index, and the address cannot be matched by a constant
/// or by a position.
///
/// The setup is established before the keys, and passes on the 785 foundation
/// too: the directory has reached the pane (its working-directory row holds
/// exactly the directory the pane was sent) and the panel describes the pane
/// the entity says it does. The only assertion that can differ between the two
/// trees is the last one.
#[gpui::test]
fn enter_on_the_first_info_row_copies_the_pane_address(cx: &mut TestAppContext) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut streams) = test_window::harness_with_tabs(cx, 2);
    vcx.simulate_resize(gpui::size(px(1440.), px(900.)));

    // The directory goes to the pane of the SECOND tab, over its own stream.
    let cwd = scratch_dir("pane-address");
    DaemonMsg::Cwd(cwd.clone())
        .encode(&mut streams[1])
        .expect("the pane takes its directory");
    app.update_in(&mut vcx, |app, window, cx| {
        app.right_panel_visible = true;
        app.activate(1, window, cx);
        cx.notify();
    });

    // Delivery: wait for the EXPECTED directory, not for any. The cause is read
    // from the live pane entity (its effective working directory is the one the
    // message sent), and the virtual clock is advanced in 300ms steps so that a
    // directory the pane held before cannot keep the barrier waiting. Bounded by
    // a real deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        draw(&app, &mut vcx);
        let taken = app.update_in(&mut vcx, |app, window, cx| {
            app.tabs
                .get(app.active)
                .and_then(|tab| tab.detail_pane(window, cx))
                .is_some_and(|leaf| leaf.read(cx).effective_cwd().as_deref() == Some(&*cwd))
        });
        if taken {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "setup: the pane never took the directory it was sent"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    draw(&app, &mut vcx);
    // And the panel's working-directory row holds exactly that directory.
    let copy = app.update_in(&mut vcx, |app, window, cx| {
        app.info_rows(window, cx)
            .rows
            .iter()
            .find(|(key, _)| *key == InfoKey::Cwd)
            .and_then(|(_, row)| row.copy.clone())
    });
    assert_eq!(
        copy.as_deref(),
        Some(cwd.display().to_string().as_str()),
        "setup: the panel's working-directory row is not the directory the pane was sent"
    );

    // The oracle, from the pane's own entity, and the panel agreeing that this
    // is the pane it describes.
    let pane_id = pane_id_of_entity(&app, &mut vcx);
    app.update_in(&mut vcx, |app, window, cx| {
        assert_eq!(
            app.info_rows(window, cx).pane,
            Some(pane_id),
            "setup: the panel describes another pane than the active tab's"
        );
    });

    // Info by name, as the real action does it: the keyboard is on the root.
    app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
    vcx.run_until_parked();
    vcx.dispatch_action(ShowRightPanelInfo);
    draw(&app, &mut vcx);
    draw(&app, &mut vcx);
    assert!(
        app.update_in(&mut vcx, |app, window, _| app.info_focus.is_focused(window)),
        "setup: ShowRightPanelInfo did not put the keyboard on the Info root"
    );

    // Real keys: Down enters the first row, Enter acts on it.
    press(&app, &mut vcx, "down");
    draw(&app, &mut vcx);
    // CONTROL, before the sentinel and with no claim about WHICH row it is: the
    // keyboard really is on a row's stop. Without it a failure that leaves the
    // sentinel could be an entry that never happened, misread as the Pane RED.
    assert!(
        app.update_in(&mut vcx, |app, window, _| {
            app.info_kb.roving.handle.is_focused(window)
        }),
        "setup: Down did not enter a row of the Info table"
    );
    sentinel(&mut vcx);
    press(&app, &mut vcx, "enter");
    assert_eq!(
        clipboard(&mut vcx),
        format!("%{pane_id}"),
        "NEW rebase-integration requirement (not a 785 regression): Enter on the first \
         Info row must copy the pane's address, `%` and its own id"
    );

    let _ = std::fs::remove_dir_all(cwd);
}
