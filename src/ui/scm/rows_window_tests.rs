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

/// The directory name `scratch` never hands out: it is the seed's, and
/// `scratch` empties a directory it is asked for before it returns it.
const SEED_NAME: &str = "immutable-seed";

pub(super) fn scratch(name: &str) -> PathBuf {
    assert_ne!(
        name, SEED_NAME,
        "`{SEED_NAME}` is the fixture seed's directory; a test asking for it would empty the seed"
    );
    scratch_dir(name)
}

fn scratch_dir(name: &str) -> PathBuf {
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
        // Pinned for the child alone: the fixture's explicit `git status`
        // refresh has to write the index even when the parent's environment
        // says otherwise. A `--no-optional-locks` on a command line still wins.
        .env("GIT_OPTIONAL_LOCKS", "1")
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
///
/// Built once per test process (`seed_repo`) and handed to every test as its
/// own copy: `git add` of the ~1000 files this takes dominates a test's cost on
/// a slow filesystem, and a copy of the finished tree is far cheaper than
/// running git again. The caller holds `serial()`, so the seed is built by one
/// test while the others wait, as they wait for each other's panel already.
pub(super) fn big_repo(name: &str) -> PathBuf {
    let seed = seed_repo();
    let seed_base = seed.parent().expect("the seed repo has a base directory");
    let base = scratch(name);
    let root = base.join("repo");
    copy_tree(&seed, &root);
    copy_tree(&seed_base.join("sub-src"), &base.join("sub-src"));
    // The copy's submodule URL still names the seed's `sub-src`; point it (and
    // the module's own `origin`) at this copy's, so nothing a test does to its
    // submodule can reach the seed or a sibling.
    git(&root, &["submodule", "sync", "--recursive"]);
    // The copied index carries the seed's file stamps. The panel reads status
    // without taking the index lock, so without this every read of this copy
    // would hash every file again.
    git(&root, &["status", "-s"]);
    root
}

/// The finished fixture every `big_repo` copies, built on first use.
fn seed_repo() -> PathBuf {
    static SEED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SEED.get_or_init(|| build_big_repo(scratch_dir(SEED_NAME)))
        .clone()
}

/// Copy the directory `from` to `to`: files byte for byte, nothing else. The
/// fixture holds no symbolic link, so meeting one is a bug in the fixture.
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
            std::fs::copy(&src, &dst)
                .unwrap_or_else(|e| panic!("copying {} to {}: {e}", src.display(), dst.display()));
        } else {
            panic!(
                "the fixture seed holds a non-regular entry: {}",
                src.display()
            );
        }
    }
}

/// The fixture itself, built with git from nothing inside the empty directory
/// `base`: the ground truth the seed is built from and the copies are compared
/// with. Returns the superproject's root; its submodule's source is the sibling
/// `sub-src`, named by the relative URL `../sub-src` so that `.gitmodules` is
/// the same in every copy.
fn build_big_repo(base: PathBuf) -> PathBuf {
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
    git(&root, &["submodule", "add", "-q", "../sub-src", "ext/sub"]);
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

// ---- The seed and its copies ------------------------------------------------
//
// `big_repo` hands out copies of one seed. What a test relies on is what a
// fresh build would have given it, and that nothing it does to its copy
// reaches the seed or another copy: these pin both. No commit id is compared
// (the commits carry their timestamps); the comparison is of what the panel and
// the keys read: statuses, index entries, history, the merge, the submodule.

/// `p` as git and the rest of the fixture spell it: existing, canonical, plain.
fn canon(p: &Path) -> PathBuf {
    tty59_core::core::path_spelling::local_spelling_buf(
        std::fs::canonicalize(p)
            .unwrap_or_else(|e| panic!("{} does not resolve: {e}", p.display())),
    )
}

/// What a fresh build and a copy have to agree on.
struct View {
    status: String,
    index: Vec<String>,
    log: String,
    branch: String,
    merge_head_is_side: bool,
    gitmodules: String,
    submodules: Vec<(char, String)>,
}

fn view(root: &Path) -> View {
    // The gitlink's object id is the submodule's HEAD commit, which carries
    // the time it was made: its mode, stage and path are compared, not that id.
    let index = git_stdout(root, &["ls-files", "-s"])
        .lines()
        .map(|line| match line.split_once('\t') {
            Some((meta, path)) if meta.starts_with("160000 ") => {
                let stage = meta.rsplit(' ').next().expect("a stage");
                format!("160000 <gitlink> {stage}\t{path}")
            }
            _ => line.to_string(),
        })
        .collect();
    let submodules = git_stdout(root, &["submodule", "status"])
        .lines()
        .map(|line| {
            (
                line.chars().next().expect("a status line"),
                line.split_whitespace()
                    .nth(1)
                    .expect("a submodule path")
                    .to_string(),
            )
        })
        .collect();
    View {
        status: git_stdout(root, &["status", "--porcelain=v1", "-z"]),
        index,
        log: git_stdout(root, &["log", "--format=%s", "main"]),
        branch: branch_name(root),
        merge_head_is_side: git_stdout(root, &["rev-parse", "MERGE_HEAD"])
            == git_stdout(root, &["rev-parse", "refs/heads/side"]),
        gitmodules: std::fs::read_to_string(root.join(".gitmodules")).expect(".gitmodules"),
        submodules,
    }
}

fn branch_name(root: &Path) -> String {
    git_stdout(root, &["branch", "--show-current"])
        .trim()
        .to_string()
}

/// The stages `conflict.txt` has in the index, in index order.
fn conflict_stages(index: &[String]) -> Vec<String> {
    index
        .iter()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            (path == "conflict.txt").then(|| meta.rsplit(' ').next().unwrap_or("").to_string())
        })
        .collect()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

fn remove_fixture(root: &Path) {
    let _ = std::fs::remove_dir_all(root.parent().unwrap_or(root));
}

/// A copy is the fixture a fresh build makes: the same changes in the same
/// groups, the same conflict, the same history and the same submodule.
#[test]
fn a_copy_of_the_seed_is_what_a_fresh_build_makes() {
    let _serial = serial();
    let copy = big_repo("cmp-copy");
    let fresh = build_big_repo(scratch("cmp-fresh"));
    let (a, b) = (view(&copy), view(&fresh));
    assert_eq!(a.status, b.status, "the same status, kind and path");
    assert!(
        a.status.matches('\0').count() > STAGED + EDITED,
        "the fixture is past the per-group cap"
    );
    assert!(a.status.contains("moved/renamed_file.txt"), "the rename");
    assert!(a.status.contains("ext/sub"), "the modified submodule");
    assert_eq!(a.index, b.index, "the same index entries, modes and stages");
    for (what, v) in [("the copy", &a), ("a fresh build", &b)] {
        assert_eq!(
            conflict_stages(&v.index),
            ["1", "2", "3"],
            "conflict.txt is unmerged at exactly stages 1, 2 and 3 in {what}"
        );
        assert!(
            v.index.iter().any(|l| l == "160000 <gitlink> 0\text/sub"),
            "ext/sub is a gitlink at stage 0 in {what}"
        );
    }
    assert_eq!(a.log, b.log, "the same history");
    assert_eq!((a.branch.as_str(), b.branch.as_str()), ("main", "main"));
    assert!(
        a.merge_head_is_side && b.merge_head_is_side,
        "mid-merge with side"
    );
    assert_eq!(
        a.gitmodules, b.gitmodules,
        ".gitmodules is the same in every copy"
    );
    assert!(
        a.gitmodules.contains("url = ../sub-src"),
        "{}",
        a.gitmodules
    );
    assert_eq!(a.submodules, b.submodules, "the same submodule state");
    remove_fixture(&copy);
    remove_fixture(&fresh);
}

/// What a copy's submodule is bound to: its own module directory, its own
/// worktree and its own `sub-src`, never the seed's.
fn assert_bound_to_itself(root: &Path) {
    let base = root.parent().expect("a base directory");
    let sub = root.join("ext/sub");
    let from = |dir: &Path, args: &[&str]| canon(Path::new(git_stdout(dir, args).trim()));
    assert_eq!(
        from(&sub, &["rev-parse", "--absolute-git-dir"]),
        canon(&root.join(".git/modules/ext/sub")),
        "the module directory is the copy's"
    );
    assert_eq!(
        from(&sub, &["rev-parse", "--show-toplevel"]),
        canon(&sub),
        "the submodule's worktree is the copy's"
    );
    assert_eq!(
        from(&sub, &["config", "remote.origin.url"]),
        canon(&base.join("sub-src")),
        "the module's origin is the copy's sub-src"
    );
    assert_eq!(
        from(root, &["config", "submodule.ext/sub.url"]),
        canon(&base.join("sub-src")),
        "the superproject's recorded url is the copy's sub-src"
    );
    git(root, &["submodule", "status"]);
}

/// What a test could change in a copy and what must not move for it. Status is
/// read without optional locks, so taking a snapshot of the seed never
/// rewrites its index.
fn snapshot(root: &Path) -> Vec<String> {
    let sub = root.join("ext/sub");
    let src = root.parent().expect("a base directory").join("sub-src");
    vec![
        git_stdout(
            root,
            &["--no-optional-locks", "status", "--porcelain=v1", "-z"],
        ),
        git_stdout(root, &["rev-parse", "HEAD"]),
        git_stdout(&sub, &["rev-parse", "HEAD"]),
        git_stdout(&sub, &["--no-optional-locks", "status", "--porcelain=v1"]),
        git_stdout(&src, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        std::fs::read_to_string(root.join(".gitmodules")).expect(".gitmodules"),
    ]
}

/// Two copies and the seed are three repositories: a change, a commit in the
/// submodule and a push from it to its origin, all in one copy, leave the
/// other copy and the seed exactly as they were, and the other copy's own
/// fetch from its origin never sees it.
#[test]
fn copies_are_isolated_from_each_other_and_from_the_seed() {
    let _serial = serial();
    let one = big_repo("iso-one");
    let two = big_repo("iso-two");
    let seed = seed_repo();
    assert!(one != two && one != seed && two != seed, "three roots");
    for root in [&one, &two, &seed] {
        let root_canon = canon(root);
        for other in [&one, &two, &seed] {
            if other != root {
                assert!(
                    !root_canon.starts_with(canon(other)),
                    "no root inside another"
                );
            }
        }
    }
    assert_bound_to_itself(&one);
    assert_bound_to_itself(&two);

    // Files every copy starts with, the same bytes: the ones a shared file
    // (a link, not a copy) would carry from one repository into another.
    let (tracked, in_sub) = (file(0), "ext/sub/readme.txt");
    let (original, original_sub) = ("line 0\nedited\n", "sub\nedited\n");
    for root in [&one, &two, &seed] {
        assert_eq!(read(root, &tracked), original, "{}", root.display());
        assert_eq!(read(root, in_sub), original_sub, "{}", root.display());
    }

    let (two_before, seed_before) = (snapshot(&two), snapshot(&seed));
    let one_before = snapshot(&one);
    write(&one, "iso_new.txt", "new\n");
    write(&one, &tracked, "copy one only\n");
    write(&one, in_sub, "copy one only, in the submodule\n");
    git(&one, &["add", "iso_new.txt", &tracked]);
    let sub = one.join("ext/sub");
    assert_eq!(read(&one, &tracked), "copy one only\n");
    assert_eq!(read(&one, in_sub), "copy one only, in the submodule\n");
    git(&sub, &["add", "readme.txt"]);
    git(&sub, &["commit", "-qm", "iso"]);
    git(&sub, &["push", "-q", "origin", "HEAD:refs/heads/iso"]);

    let mine = snapshot(&one);
    assert_ne!(mine, one_before, "the changes were made in the first copy");
    assert!(mine[0].contains("A  iso_new.txt"), "{}", mine[0]);
    assert_eq!(
        git_stdout(
            &one.parent().unwrap().join("sub-src"),
            &["rev-parse", "refs/heads/iso"]
        ),
        git_stdout(&sub, &["rev-parse", "HEAD"]),
        "the push reached this copy's own sub-src"
    );
    assert_eq!(snapshot(&two), two_before, "the other copy did not move");
    assert_eq!(snapshot(&seed), seed_before, "the seed did not move");
    assert!(!two.join("iso_new.txt").exists());
    for root in [&two, &seed] {
        assert_eq!(read(root, &tracked), original, "{}", root.display());
        assert_eq!(read(root, in_sub), original_sub, "{}", root.display());
    }
    // The submodule's committed bytes, read back out of each repository.
    let committed = |root: &Path| git_stdout(&root.join("ext/sub"), &["show", "HEAD:readme.txt"]);
    assert_eq!(committed(&one), "copy one only, in the submodule\n");
    assert_eq!(committed(&two), "sub\n");
    assert_eq!(committed(&seed), "sub\n");

    git(&two.join("ext/sub"), &["fetch", "-q", "origin"]);
    assert!(
        !git_ok(
            &two.join("ext/sub"),
            &["rev-parse", "--verify", "-q", "refs/remotes/origin/iso"]
        ),
        "the other copy's origin has no such branch"
    );
    assert_eq!(
        snapshot(&two),
        two_before,
        "a fetch changed no tracked state"
    );
    remove_fixture(&one);
    remove_fixture(&two);
}
