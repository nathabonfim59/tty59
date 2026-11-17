//! The Changes list over a working tree with hundreds of changed files.
//!
//! A frame used to lay out every expanded row — ~620 of them with the
//! per-group cap — whether or not the panel could show them, and each frame
//! of a large repository cost a few hundred milliseconds of layout: long
//! enough for keystrokes into the terminal next to it to queue for seconds.
//! These tests pin that a frame only builds the rows on screen, and that
//! nothing a user can see or reach moved while getting there: row positions,
//! the header above them, folding, a click on a deep row, a context menu, the
//! UI font size, and a panel that goes quiet once it has drawn.

use std::path::{Path, PathBuf};

use gpui::{
    Bounds, Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, PlatformInput,
    ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, point, px, size,
};

use crate::daemon::protocol::DaemonMsg;
use crate::ui::app::{Tty59App, render_probe, test_window};
use crate::ui::scm::panel::{ROW_H, group_diff_source, in_group};
use crate::ui::scm::state::ScmGroup;
use tty59_core::core::config::RightPanelTab;
use tty59_core::core::git::diff::MAX_RENDERED_FILES;
use tty59_core::core::git::status::WorkingTreeStatus;

/// Files committed to the fixture. The first `STAGED` of them are edited and
/// staged, the next `EDITED` edited and left in the working tree; the rest
/// stay as they are, one of them to be renamed.
pub(super) const BASE: usize = 660;
pub(super) const STAGED: usize = 325;
pub(super) const EDITED: usize = 325;
/// The file `git mv` renames, staged.
pub(super) const RENAMED: usize = 655;

/// A name long enough that the row has to truncate it.
fn long_name() -> String {
    format!("long/{}.txt", "a_very_long_file_name_segment".repeat(8))
}

pub(super) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tty59-rows-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // `canonicalize` answers `\\?\C:\…` on Windows. Git takes the submodule
    // source path from its command line and reads that prefix as a UNC
    // hostname ("hostname contains invalid characters"), so the root every
    // fixture path derives from is handed out in the plain local spelling.
    tty59_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(&dir).unwrap())
}

/// One `git` child, run once; its whole `Output` is kept so a failing setup
/// step can say what git said.
fn git_output(root: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .expect("git runs")
}

// Keyboard-action fixtures also need the status predicate; setup failures use
// git_output's full diagnostics through git() on every supported platform.
pub(super) fn git_ok(root: &Path, args: &[&str]) -> bool {
    git_output(root, args).status.success()
}

pub(super) fn git(root: &Path, args: &[&str]) {
    git_stdout(root, args);
}

/// `git` for the answer, not the side effect: what it printed on stdout. A
/// failure says everything git said, the way `git` does.
pub(super) fn git_stdout(root: &Path, args: &[&str]) -> String {
    let out = git_output(root, args);
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}\nstdout: {}\nstderr: {}",
        root.display(),
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `git init` on branch `main`, with the two repository settings that make a
/// fixture the same on every platform pinned BEFORE any data is written: no
/// line-ending conversion (Git for Windows defaults to `autocrlf=true`, which
/// rewrites what the fixtures write and reads back) and long paths.
pub(super) fn init_repo(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "core.longpaths", "true"]);
    git(root, &["config", "core.autocrlf", "false"]);
}

pub(super) fn file(i: usize) -> String {
    format!("pkg_{:02}/file_{i:03}.txt", i / 25)
}

pub(super) fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A repository in the middle of a merge with a conflict, ~325 staged and
/// ~327 unstaged changes — a staged rename, a modified submodule and a very
/// long name among them. Every group past the cap, so each one draws
/// `MAX_RENDERED_FILES` rows and its "…and N more".
pub(super) fn big_repo(name: &str) -> PathBuf {
    let base = scratch(name);
    let root = base.join("repo");
    let sub = base.join("sub-src");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&sub).unwrap();

    init_repo(&sub);
    write(&sub, "readme.txt", "sub\n");
    git(&sub, &["add", "-A"]);
    git(&sub, &["commit", "-qm", "sub"]);

    init_repo(&root);
    for i in 0..BASE {
        write(&root, &file(i), &format!("line {i}\n"));
    }
    write(&root, &long_name(), "long\n");
    write(&root, "conflict.txt", "base\n");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "base"]);
    git(
        &root,
        &["submodule", "add", "-q", sub.to_str().unwrap(), "ext/sub"],
    );
    git(&root, &["commit", "-qm", "submodule"]);

    git(&root, &["checkout", "-q", "-b", "side"]);
    write(&root, "conflict.txt", "side\n");
    git(&root, &["commit", "-qam", "side"]);
    git(&root, &["checkout", "-q", "main"]);
    write(&root, "conflict.txt", "main\n");
    git(&root, &["commit", "-qam", "main"]);
    let merge = git_output(&root, &["merge", "-q", "side"]);
    assert!(
        !merge.status.success(),
        "the merge stops on the conflict: {}\nstdout: {}\nstderr: {}",
        merge.status,
        String::from_utf8_lossy(&merge.stdout),
        String::from_utf8_lossy(&merge.stderr),
    );

    for i in 0..STAGED + EDITED {
        write(&root, &file(i), &format!("line {i}\nedited\n"));
    }
    let staged: Vec<String> = (0..STAGED).map(file).collect();
    let mut add = vec!["add", "--"];
    add.extend(staged.iter().map(String::as_str));
    git(&root, &add);
    std::fs::create_dir_all(root.join("moved")).unwrap();
    git(&root, &["mv", &file(RENAMED), "moved/renamed_file.txt"]);
    write(&root, &long_name(), "long\nedited\n");
    write(&root, "ext/sub/readme.txt", "sub\nedited\n");
    root
}

pub(super) fn panel_on(
    cx: &mut TestAppContext,
    root: &Path,
) -> (
    Entity<Tty59App>,
    VisualTestContext,
    crate::daemon::transport::Stream,
) {
    crate::core::config::pin_test_config_dir();
    let (app, mut vcx, mut pane) = test_window::harness_with_pane(cx);
    vcx.simulate_resize(size(px(1440.), px(900.)));
    DaemonMsg::Cwd(root.to_path_buf())
        .encode(&mut pane)
        .expect("the pane's socket takes the cwd");
    app.update_in(&mut vcx, |app, _, cx| {
        app.right_panel_visible = true;
        app.right_panel_tab = RightPanelTab::Scm;
        cx.notify();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.background_executor.run_until_parked();
        let ready = app.update_in(&mut vcx, |app, _, cx| {
            app.scm.repo.as_ref().is_some_and(|r| {
                crate::terminal::git_data::status_of(cx, r.host, &r.root)
                    .is_some_and(|s| s.entries.len() > STAGED + EDITED)
            })
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the panel never read the fixture's status"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    test_window::quiesce(&mut vcx, Some(root));
    draw(&app, &mut vcx);
    (app, vcx, pane)
}

pub(super) fn draw(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
    app.update_in(vcx, |_, _, cx| cx.notify());
    vcx.run_until_parked();
}

pub(super) fn status(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
) -> std::sync::Arc<WorkingTreeStatus> {
    app.update_in(vcx, |app, _, cx| {
        let repo = app.scm.repo.clone().expect("a repository is on screen");
        crate::terminal::git_data::status_of(cx, repo.host, &repo.root).expect("status read")
    })
}

/// The flat list's rows of `group`, in the order the panel draws them, as the
/// selectors the rows carry. Leaked: `debug_bounds` takes `&'static str`, and
/// a test process can spare a few kilobytes.
pub(super) fn flat_rows(status: &WorkingTreeStatus, group: ScmGroup) -> Vec<&'static str> {
    status
        .entries
        .iter()
        .filter(|e| in_group(e, group))
        .take(MAX_RENDERED_FILES)
        .map(|e| &*Box::leak(format!("scm-row-{group:?}-{}", e.path.as_str()).into_boxed_str()))
        .collect()
}

pub(super) fn header(group: ScmGroup) -> &'static str {
    Box::leak(format!("scm-group-{group:?}").into_boxed_str())
}

/// Which of `rows` the last frame painted, by index, with their bounds.
pub(super) fn painted(
    vcx: &mut VisualTestContext,
    rows: &[&'static str],
) -> Vec<(usize, Bounds<Pixels>)> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, sel)| vcx.debug_bounds(sel).map(|b| (i, b)))
        .collect()
}

/// A file row's height plus the 1px between rows, at the rem in force now.
pub(super) fn pitch(vcx: &mut VisualTestContext) -> f32 {
    let rem = vcx.update(|window, _| window.rem_size().as_f32());
    ROW_H * rem / 16. + 1.
}

pub(super) fn viewport(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Bounds<Pixels> {
    app.update_in(vcx, |app, _, _| app.scm.scroll.bounds())
}

pub(super) fn scroll_to(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, y: f32) {
    app.update_in(vcx, |app, _, _| {
        app.scm.scroll.set_offset(point(px(0.), px(-y)))
    });
    draw(app, vcx);
    // Twice: the first frame clamps the offset and may re-measure.
    draw(app, vcx);
}

pub(super) fn scroll_top(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> f32 {
    app.update_in(vcx, |app, _, _| -app.scm.scroll.offset().y.as_f32())
}

/// Collapse every group but `keep`, so `keep`'s rows start near the top.
pub(super) fn only_group(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, keep: ScmGroup) {
    app.update_in(vcx, |app, _, cx| {
        for g in ScmGroup::ORDER {
            app.scm.set_group_collapsed(g, g != keep);
        }
        cx.notify();
    });
    draw(app, vcx);
}

/// How many rows a frame may build for one viewport: what fits, a partial row
/// at each edge, and an overscan margin either side for each of the (at most
/// two) runs that can share the viewport. Main built every row — 600-odd.
pub(super) fn row_budget(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
    let fits = (viewport(app, vcx).size.height.as_f32() / pitch(vcx)).ceil() as usize;
    fits + 2 + 2 * 2 * 12
}

/// Every painted row of a run stands exactly `k` pitches below the run's
/// first row, and the first row one gap under its header — wherever the list
/// is scrolled and whichever rows were built.
fn assert_rows_on_pitch(
    vcx: &mut VisualTestContext,
    rows: &[&'static str],
    origin: f32,
    what: &str,
) {
    let pitch = pitch(vcx);
    let drawn = painted(vcx, rows);
    assert!(!drawn.is_empty(), "{what}: some rows are on screen");
    for (k, b) in drawn {
        let want = origin + k as f32 * pitch;
        assert!(
            (b.top().as_f32() - want).abs() <= 0.5,
            "{what}: row {k} ({}) at {} instead of {want}",
            rows[k],
            b.top().as_f32()
        );
        assert!(
            (b.size.height.as_f32() - (pitch - 1.)).abs() <= 0.5,
            "{what}: row {k} ({}) is {} tall instead of {}",
            rows[k],
            b.size.height.as_f32(),
            pitch - 1.
        );
    }
}

/// Where the first row of `group` sits in window space at the current
/// scroll, from its header — which is drawn whatever the scroll.
pub(super) fn first_row_top(vcx: &mut VisualTestContext, group: ScmGroup) -> f32 {
    let h = vcx
        .debug_bounds(header(group))
        .expect("the group's header is drawn");
    h.bottom().as_f32() + 1.
}

/// The bug. With 600-odd rows expanded, a frame builds what one viewport can
/// show — at the top of the list and at its bottom.
#[gpui::test]
fn a_long_changes_list_builds_only_the_rows_on_screen(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("budget");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let st = status(&app, &mut vcx);
    let merge = flat_rows(&st, ScmGroup::Merge);
    let staged = flat_rows(&st, ScmGroup::Staged);
    let changes = flat_rows(&st, ScmGroup::Changes);
    assert_eq!(merge.len(), 1);
    assert_eq!(staged.len(), MAX_RENDERED_FILES);
    assert_eq!(changes.len(), MAX_RENDERED_FILES);
    let all: Vec<&'static str> = [merge, staged.clone(), changes.clone()].concat();

    let budget = row_budget(&app, &mut vcx);
    let top = painted(&mut vcx, &all);
    assert!(
        top.len() <= budget,
        "at the top a frame painted {} of {} rows; one viewport holds at most {budget}",
        top.len(),
        all.len()
    );
    assert!(
        vcx.debug_bounds(staged[0]).is_some(),
        "the first rows are drawn"
    );

    scroll_to(&app, &mut vcx, 1e7);
    let bottom = painted(&mut vcx, &all);
    assert!(
        bottom.len() <= budget,
        "at the bottom a frame painted {} of {} rows; one viewport holds at most {budget}",
        bottom.len(),
        all.len()
    );
    assert!(
        vcx.debug_bounds(changes[MAX_RENDERED_FILES - 1]).is_some(),
        "the last row is drawn once the list is scrolled to it"
    );
    assert!(
        vcx.debug_bounds(staged[0]).is_none(),
        "and the first one is not"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// Built or not, every row is where it always was: on the pitch under its
/// header, at the top, part way down and at the bottom of the list, at the
/// default UI size and the largest. The fixture's rows cover every variant a
/// row can take — a conflict, a staged rename, a submodule, a name that
/// truncates.
#[gpui::test]
fn every_row_stands_on_the_pitch_at_any_scroll_and_ui_size(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("pitch");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let st = status(&app, &mut vcx);
    let merge = flat_rows(&st, ScmGroup::Merge);
    let staged = flat_rows(&st, ScmGroup::Staged);
    let changes = flat_rows(&st, ScmGroup::Changes);
    assert!(staged.iter().any(|s| s.ends_with("moved/renamed_file.txt")));
    assert!(changes.iter().any(|s| s.ends_with("ext/sub")));
    assert!(changes.iter().any(|s| s.ends_with(&long_name())));

    for ui in [
        crate::core::config::UI_FONT_SIZE_DEFAULT,
        crate::core::config::UI_FONT_SIZE_MAX,
    ] {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<crate::core::config::Config>().clone();
            cfg.ui_font_size = ui;
            cx.set_global(cfg);
        });
        scroll_to(&app, &mut vcx, 0.);
        let p = pitch(&mut vcx);
        assert_eq!(p, ROW_H * ui / 16. + 1., "the rem follows the UI size");

        let o = first_row_top(&mut vcx, ScmGroup::Merge);
        assert_rows_on_pitch(&mut vcx, &merge, o, &format!("merge at the top, ui {ui}"));
        let o = first_row_top(&mut vcx, ScmGroup::Staged);
        assert_rows_on_pitch(&mut vcx, &staged, o, &format!("staged at the top, ui {ui}"));

        // Part way into the staged run: its header is far above the viewport.
        let view = viewport(&app, &mut vcx);
        let at0 = first_row_top(&mut vcx, ScmGroup::Staged) - view.top().as_f32();
        scroll_to(&app, &mut vcx, at0 + 150. * p);
        let o = first_row_top(&mut vcx, ScmGroup::Staged);
        assert_rows_on_pitch(&mut vcx, &staged, o, &format!("staged mid-run, ui {ui}"));
        assert!(
            vcx.debug_bounds(staged[150]).is_some(),
            "ui {ui}: the row scrolled to is drawn"
        );

        scroll_to(&app, &mut vcx, 1e7);
        let o = first_row_top(&mut vcx, ScmGroup::Changes);
        assert_rows_on_pitch(
            &mut vcx,
            &changes,
            o,
            &format!("changes at the bottom, ui {ui}"),
        );
        assert!(scroll_top(&app, &mut vcx) > 0.);
    }
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// Folding a group or a tree directory hides exactly its rows and moves
/// everything after it up by exactly their height; unfolding puts every row
/// back where it was.
#[gpui::test]
fn folding_hides_exactly_the_folded_rows(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("fold");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let st = status(&app, &mut vcx);
    let staged = flat_rows(&st, ScmGroup::Staged);
    let changes = flat_rows(&st, ScmGroup::Changes);

    // A folded group: none of its rows, the next header right under it.
    app.update_in(&mut vcx, |app, _, cx| {
        app.scm.set_group_collapsed(ScmGroup::Staged, true);
        cx.notify();
    });
    draw(&app, &mut vcx);
    assert!(
        painted(&mut vcx, &staged).is_empty(),
        "a folded group draws no rows"
    );
    let o = first_row_top(&mut vcx, ScmGroup::Changes);
    assert_rows_on_pitch(&mut vcx, &changes, o, "changes under a folded staged group");
    assert!(vcx.debug_bounds(changes[0]).is_some());

    app.update_in(&mut vcx, |app, _, cx| {
        app.scm.set_group_collapsed(ScmGroup::Staged, false);
        cx.notify();
    });
    draw(&app, &mut vcx);
    let o = first_row_top(&mut vcx, ScmGroup::Staged);
    assert_rows_on_pitch(&mut vcx, &staged, o, "staged unfolded again");
    assert!(vcx.debug_bounds(staged[0]).is_some());

    // A folded directory in the tree view, with the run it is in on screen —
    // all of it: the directory after the folded one starts ~31 rows down,
    // past what a 900px window shows, and has to be visible both before the
    // fold and after it.
    vcx.simulate_resize(size(px(1440.), px(2000.)));
    only_group(&app, &mut vcx, ScmGroup::Changes);
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    draw(&app, &mut vcx);
    let dir = "scm-dir-Changes-pkg_13";
    let next = "scm-dir-Changes-pkg_14";
    let inside: Vec<&'static str> = (STAGED..350)
        .map(|i| &*Box::leak(format!("scm-row-Changes-{}", file(i)).into_boxed_str()))
        .collect();
    let open_next = vcx
        .debug_bounds(next)
        .expect("the next directory is on screen");
    assert_eq!(painted(&mut vcx, &inside).len(), inside.len());

    app.update_in(&mut vcx, |app, _, cx| {
        app.scm.toggle_dir(ScmGroup::Changes, "pkg_13".into());
        cx.notify();
    });
    draw(&app, &mut vcx);
    assert!(
        vcx.debug_bounds(dir).is_some(),
        "the folded directory keeps its row"
    );
    assert!(
        painted(&mut vcx, &inside).is_empty(),
        "and hides what is under it"
    );
    let folded_next = vcx
        .debug_bounds(next)
        .expect("the next directory is still drawn");
    let moved = open_next.top().as_f32() - folded_next.top().as_f32();
    assert!(
        (moved - inside.len() as f32 * pitch(&mut vcx)).abs() <= 0.5,
        "the rows after it moved up by exactly the folded rows: {moved}"
    );

    app.update_in(&mut vcx, |app, _, cx| {
        app.scm.toggle_dir(ScmGroup::Changes, "pkg_13".into());
        cx.notify();
    });
    draw(&app, &mut vcx);
    assert_eq!(painted(&mut vcx, &inside).len(), inside.len());
    assert_eq!(
        vcx.debug_bounds(next),
        Some(open_next),
        "and come back to where they were"
    );
    app.update(&mut vcx, |app, cx| app.scm_toggle_changes_tree(cx));
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

pub(super) fn mouse(vcx: &mut VisualTestContext, button: MouseButton, at: gpui::Point<Pixels>) {
    vcx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Modifiers::none(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
            }),
            cx,
        );
        window.refresh();
    });
    vcx.run_until_parked();
}

/// A row deep in the list is clicked where it is drawn, opens its own diff,
/// and is still the selected row after the list scrolls away and back.
/// Selection is the overlay's focused path, so nothing about it depends on
/// which rows a frame built.
#[gpui::test]
fn a_deep_row_is_clicked_where_it_is_drawn_and_stays_selected(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("select");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let st = status(&app, &mut vcx);
    let staged = flat_rows(&st, ScmGroup::Staged);
    let p = pitch(&mut vcx);
    let view = viewport(&app, &mut vcx);
    let at0 = first_row_top(&mut vcx, ScmGroup::Staged) - view.top().as_f32();
    let k = 240;
    scroll_to(
        &app,
        &mut vcx,
        at0 + k as f32 * p - view.size.height.as_f32() / 2.,
    );
    let row = vcx
        .debug_bounds(staged[k])
        .expect("the row scrolled to is drawn");
    mouse(&mut vcx, MouseButton::Left, row.center());

    let path = staged[k].trim_start_matches("scm-row-Staged-").to_string();
    let focus = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| {
            let repo = app.scm.repo.clone().unwrap();
            app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(ScmGroup::Staged))
                .map(str::to_string)
        })
    };
    assert_eq!(
        focus(&app, &mut vcx),
        Some(path.clone()),
        "the click opened that row"
    );

    let back = scroll_top(&app, &mut vcx);
    scroll_to(&app, &mut vcx, 0.);
    assert!(scroll_top(&app, &mut vcx) < 1., "scrolled back to the top");
    scroll_to(&app, &mut vcx, back);
    assert_eq!(focus(&app, &mut vcx), Some(path));
    assert_eq!(
        vcx.debug_bounds(staged[k]),
        Some(row),
        "back where it was clicked"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// A context menu open on a row that the list then scrolls far away from
/// stays up over its row, the way it did when every row was built; Escape
/// closes it and the panel answers the next click.
///
/// The row owns its menu (`row_menu::RowMenu`), with nothing pointing back
/// at it, so this is an ordinary `#[gpui::test]` under gpui's strict
/// teardown: a menu the panel left behind would fail it as a leaked handle.
/// The row stays built because the panel records it as the one a menu is
/// open on (`menu_row`), and that record is what the run's pin reads.
#[gpui::test]
fn a_context_menu_on_a_row_scrolled_far_away_stays_with_its_row(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("menu");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let st = status(&app, &mut vcx);
    let staged = flat_rows(&st, ScmGroup::Staged);
    let row = vcx.debug_bounds(staged[3]).expect("an early row is drawn");

    let staged_path = staged[3].trim_start_matches("scm-row-Staged-").to_string();
    let recorded = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, _, _| app.scm.menu_row.clone())
    };

    let before = vcx.update(|window, cx| window.focused(cx));
    mouse(&mut vcx, MouseButton::Right, row.center());
    draw(&app, &mut vcx);
    let open = vcx.update(|window, cx| window.focused(cx));
    assert!(
        open.is_some() && open != before,
        "the right click opened a menu"
    );
    assert_eq!(
        recorded(&app, &mut vcx),
        Some((ScmGroup::Staged, staged_path.clone())),
        "the panel records the row the menu is open on: that is what pins it"
    );

    scroll_to(&app, &mut vcx, 1e7);
    let wheel_at = viewport(&app, &mut vcx).center();
    vcx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: wheel_at,
                delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                modifiers: Modifiers::none(),
                touch_phase: TouchPhase::Moved,
            }),
            cx,
        );
    });
    draw(&app, &mut vcx);
    let after = vcx.update(|window, cx| window.focused(cx));
    assert_eq!(after, open, "the menu is still up, and still focused");
    assert!(
        vcx.debug_bounds(staged[3]).is_some(),
        "over its row, which is still built"
    );
    assert_eq!(
        recorded(&app, &mut vcx),
        Some((ScmGroup::Staged, staged_path)),
        "and it is still the row on record"
    );

    // And the panel is still the panel: Escape is harmless, and a click on
    // a row on screen now opens that row.
    vcx.simulate_keystrokes("escape");
    draw(&app, &mut vcx);
    let closed = vcx.update(|window, cx| window.focused(cx));
    assert_ne!(closed, open, "Escape closed the menu");
    assert_eq!(
        recorded(&app, &mut vcx),
        None,
        "and the row is no longer on record"
    );
    let changes = flat_rows(&st, ScmGroup::Changes);
    let last = vcx
        .debug_bounds(changes[MAX_RENDERED_FILES - 1])
        .expect("the bottom of the list is on screen");
    mouse(&mut vcx, MouseButton::Left, last.center());
    let path = changes[MAX_RENDERED_FILES - 1]
        .trim_start_matches("scm-row-Changes-")
        .to_string();
    let focus = app.update_in(&mut vcx, |app, _, _| {
        let repo = app.scm.repo.clone().unwrap();
        app.diff_overlay_focus(repo.host, &repo.root, &group_diff_source(ScmGroup::Changes))
            .map(str::to_string)
    });
    assert_eq!(focus, Some(path), "a click after the menu reaches its row");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

/// Once the list has drawn, a parked panel draws nothing more — part way down
/// a long list as much as at its top. The window a measurement can ask to
/// redraw is the window a measurement can ask to redraw forever.
#[gpui::test]
fn a_scrolled_long_list_goes_quiet(cx: &mut TestAppContext) {
    let _serial = serial();
    let root = big_repo("idle");
    let (app, mut vcx, _pane) = panel_on(cx, &root);
    let p = pitch(&mut vcx);
    scroll_to(&app, &mut vcx, 200. * p);
    test_window::quiesce(&mut vcx, None);
    render_probe::arm(200);
    let asked = crate::ui::scm::rows_window::REDRAWS.with(|n| n.get());
    for _ in 0..20 {
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        vcx.background_executor.run_until_parked();
    }
    assert_eq!(render_probe::draws(), 0, "20 parked frames drew nothing");
    assert_eq!(
        crate::ui::scm::rows_window::REDRAWS.with(|n| n.get()),
        asked,
        "and no run asked for another frame"
    );
    // Forcing frames with nothing moved: the range already covers the
    // viewport, so not one of them asks for a follow-up.
    for _ in 0..20 {
        draw(&app, &mut vcx);
    }
    assert_eq!(
        crate::ui::scm::rows_window::REDRAWS.with(|n| n.get()),
        asked,
        "a converged range never asks again"
    );
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}
