//! Multi-row path links, driven by recorded PTY output.
//!
//! The `.pty` files under `../fixtures/path_links/` are raw bytes captured on a
//! Boat VM through a real PTY (80x24); `manifest.json` next to them records the
//! command, tool versions and provenance of every one. Two kinds are mixed on
//! purpose and labelled in the manifest: engineered GNU formatter recordings
//! (`fold`, `sed`, `awk`, `nl`) and real application traces (`rustc`, `git`,
//! `rg`, `ls`). None of them is an AI TUI capture.
//!
//! Every case here goes through the code a user's pointer reaches: the grid is
//! filled by feeding the captured bytes to a pane, then the *same* resolver
//! answers a hover (`hover_link_at`), a resolve (`resolve_link_at`) and a click
//! (`open_link_at` -> `OpenFileRequested`). Expected underlines are written
//! out as literal row segments, and each case checks that those segments spell
//! the expected path, so no expectation is computed by the code under test.
//!
//! Cases that must keep working today are controls (soft wrap, a hard break at
//! the terminal width, real tool output, the negatives); the rest are the
//! multi-row misses.

use super::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

macro_rules! capture {
    ($name:literal) => {
        &include_bytes!(concat!("../fixtures/path_links/", $name, ".pty"))[..]
    };
}

const TARGET: &str =
    "sources/reports/component-with-a-very-long-name/release-notes/neutral-preview-target.rs";
const LOCATION: &str = "sources/location.rs";
const QUOTED: &str = "sources/reports/neutral folder/quoted-preview-target.rs";

/// The neutral fixture files of the recording, including the concatenated
/// spellings the negatives would wrongly produce: a filesystem hit alone must
/// never excuse a join.
const TREE: [&str; 8] = [
    TARGET,
    LOCATION,
    QUOTED,
    "notes/first.rs",
    "notes/second.rs",
    "notes/first.rsnotes/second.rs",
    "notes/first.rsunrelated",
    "read the outputnotes/second.rs",
];

const LISTING: [&str; 4] = [
    "a-neutral-file.txt",
    "another-neutral-file.txt",
    "component-with-a-very-long-name-neutral-fixture-file.rs",
    "z-notes.txt",
];

#[derive(Clone, Copy)]
enum Cwd {
    Root,
    Real,
    Listing,
}

/// One scratch tree per test: a shared, PID-only name would let two tests that
/// run side by side delete each other's files.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(tag: &str, extra: &[&str]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "tty59-path-links-{}-{}-{tag}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let touch = |base: &Path, rel: &str| {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("create dirs");
            std::fs::write(&path, b"neutral fixture\n").expect("create file");
        };
        for rel in TREE.iter().chain(extra) {
            touch(&root, rel);
        }
        touch(&root.join("real"), TARGET);
        for name in LISTING {
            touch(&root.join("listing"), name);
        }
        Self { root }
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn cwd(&self, cwd: Cwd) -> PathBuf {
        match cwd {
            Cwd::Root => self.root.clone(),
            Cwd::Real => self.root.join("real"),
            Cwd::Listing => self.root.join("listing"),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    cwd: Cwd,
    /// The logical rows (soft wraps joined) the capture must leave on the
    /// grid. Checked before any link is asked about, so a pane that never
    /// received the bytes fails as a setup error and not as a missing link.
    rows: Vec<String>,
}

fn case(name: &'static str, bytes: &[u8], cwd: Cwd, rows: &[&str]) -> Case {
    Case {
        name,
        bytes: bytes.to_vec(),
        cwd,
        rows: rows.iter().map(|r| (*r).to_owned()).collect(),
    }
}

type Seg = (i32, u16, String);
type Runs = Vec<(i32, u16, u16)>;
type Opened = (PathBuf, Option<u32>, Option<u32>, bool);

fn seg(line: i32, col: u16, text: &str) -> Seg {
    (line, col, text.to_owned())
}

fn runs_of_segs(segs: &[Seg]) -> Runs {
    segs.iter()
        .map(|(line, col, text)| (*line, *col, *col + text.chars().count() as u16 - 1))
        .collect()
}

fn spelled(segs: &[Seg]) -> String {
    segs.iter().map(|(_, _, text)| text.as_str()).collect()
}

/// The first and last cell of every segment, as `(col, row)`: a click on any
/// row of a continued link has to land on the same target.
fn cells(segs: &[Seg]) -> Vec<(usize, usize)> {
    segs.iter()
        .flat_map(|(line, col, text)| {
            let first = usize::from(*col);
            let last = first + text.chars().count() - 1;
            [(first, *line as usize), (last, *line as usize)]
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum Out {
    File {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        runs: Runs,
    },
    Url {
        url: String,
        runs: Runs,
    },
    /// No link at all: nothing found, or a path-shaped token nothing answered.
    Unlinked,
}

fn file(root: &Path, rel: &str, segs: &[Seg]) -> Out {
    file_with(root, rel, "", None, None, segs)
}

/// A file link whose underline also covers a written `:line:column`.
fn file_with(
    root: &Path,
    rel: &str,
    suffix: &str,
    line: Option<u32>,
    column: Option<u32>,
    segs: &[Seg],
) -> Out {
    assert_eq!(
        spelled(segs),
        format!("{rel}{suffix}"),
        "fixture: the expected segments must spell the path they underline"
    );
    Out::File {
        path: root.join(rel),
        line,
        column,
        is_dir: false,
        runs: runs_of_segs(segs),
    }
}

fn runs_of(runs: &[(GridPoint, GridPoint)]) -> Runs {
    runs.iter()
        .map(|(a, b)| {
            assert_eq!(a.line, b.line, "an underline run stays on one row");
            (a.line, a.col, b.col)
        })
        .collect()
}

/// The pane's grid as logical rows: soft-wrapped rows joined, padding off.
fn logical_rows(view: &TerminalView) -> Vec<String> {
    let term = view.terminal.term.lock();
    let grid = term.view();
    let cols = grid.columns();
    let mut rows: Vec<String> = Vec::new();
    let mut continued = false;
    for line in 0..=grid.bottommost_line() {
        let mut text = String::new();
        for col in 0..cols {
            let cell = grid.glyph(GridPoint {
                line,
                col: col as u16,
            });
            if matches!(cell.wide, CellWidth::SpacerTail | CellWidth::SpacerHead) {
                continue;
            }
            text.push(cell.c);
        }
        match (continued, rows.last_mut()) {
            (true, Some(last)) => last.push_str(&text),
            _ => rows.push(text),
        }
        continued = grid.row_wraps(line);
    }
    rows.iter().map(|r| r.trim_end().to_owned()).collect()
}

/// An empty window child. The pane under test is held beside it, never put in
/// the window's element tree: a rendered pane takes the window's size for its
/// grid, and these recordings only mean something at the 80 columns they were
/// captured at.
struct Blank;

impl gpui::Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// A pane that is not rendered, in a window whose root is a
/// `gpui_component::Root` so a notification (an unresolved path click) has
/// somewhere to go.
fn held_harness(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<gpui_component::Root>,
    gpui::Entity<TerminalView>,
    Stream,
) {
    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = super::super::test_stream_pair();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(Config::default());
    });
    let built: Rc<RefCell<Option<gpui::Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
            .expect("socketpair-backed terminal");
        *out.borrow_mut() = Some(cx.new(|cx| TerminalView::with_terminal(terminal, 1, window, cx)));
        let blank = cx.new(|_| Blank);
        gpui_component::Root::new(blank, window, cx)
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.background_executor.run_until_parked();
    let view = built.borrow_mut().take().expect("the pane was built");
    (window, view, daemon_side)
}

/// Every map in this file assumes the capture's own 80x24. A pane that
/// reflowed to some other size would move every expected cell, so it is a
/// setup failure, not a link that went missing.
fn assert_80x24(cx: &mut TestAppContext, view: &gpui::Entity<TerminalView>, name: &str) {
    let (cols, last_line) = view.update(cx, |view, _| {
        let term = view.terminal.term.lock();
        let grid = term.view();
        (grid.columns(), grid.bottommost_line())
    });
    assert_eq!(
        (cols, last_line),
        (80, 23),
        "SETUP FAILURE: `{name}` is not on an 80x24 grid (columns, last line)"
    );
}

fn settle(cx: &mut TestAppContext, view: &gpui::Entity<TerminalView>, name: &str, rows: &[String]) {
    let mut seen = Vec::new();
    for _ in 0..600 {
        seen = view.update(cx, |view, _| logical_rows(view));
        if seen.len() >= rows.len() && seen[..rows.len()] == *rows {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!(
        "SETUP FAILURE (not a missing link): the grid for `{name}` never showed the recorded \
         output.\nexpected: {rows:#?}\nseen: {seen:#?}"
    );
}

struct Replay {
    name: &'static str,
    window: gpui::WindowHandle<gpui_component::Root>,
    view: gpui::Entity<TerminalView>,
    daemon: Stream,
    opened: Rc<RefCell<Vec<Opened>>>,
}

impl Replay {
    fn new(cx: &mut TestAppContext, scratch: &Scratch, case: &Case) -> Self {
        let (window, view, mut daemon) = held_harness(cx);
        assert_80x24(cx, &view, case.name);
        cx.update(|cx| {
            // The click goes to the built-in editor whatever a developer's own
            // settings say: nothing here may reach a system opener.
            let mut config = cx.global::<Config>().clone();
            config.link_file_open = Some(LinkFileOpen::Internal);
            config.link_file_command = None;
            cx.set_global(config);
        });

        let opened = Rc::new(RefCell::new(Vec::new()));
        let sink = opened.clone();
        view.update(cx, |_, cx| {
            let this = cx.entity();
            cx.subscribe(&this, move |_, _, event: &OpenFileRequested, _| {
                sink.borrow_mut().push((
                    event.path.clone(),
                    event.line,
                    event.column,
                    event.is_dir,
                ));
            })
            .detach();
        });

        let cwd = scratch.cwd(case.cwd);
        DaemonMsg::Cwd(cwd.clone()).encode(&mut daemon).unwrap();
        for _ in 0..600 {
            if view.update(cx, |view, _| view.cwd().as_deref() == Some(cwd.as_path())) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            view.update(cx, |view, _| view.cwd()),
            Some(cwd),
            "SETUP FAILURE: the pane never learned its directory"
        );

        let mut replay = Self {
            name: case.name,
            window,
            view,
            daemon,
            opened,
        };
        replay.feed(cx, &case.bytes, &case.rows);
        replay
    }

    /// Feeds raw bytes and waits until the grid shows `rows`.
    fn feed(&mut self, cx: &mut TestAppContext, bytes: &[u8], rows: &[String]) {
        DaemonMsg::Output(bytes.to_vec())
            .encode(&mut self.daemon)
            .unwrap();
        settle(cx, &self.view, self.name, rows);
        assert_80x24(cx, &self.view, self.name);
    }

    fn resolve(&self, cx: &mut TestAppContext, col: usize, row: usize) -> Out {
        self.view.update(cx, |view, cx| {
            match view.resolve_link_at(col, row, true, false, cx) {
                LinkAt::Found(
                    LinkTarget::File {
                        path,
                        line,
                        column,
                        is_dir,
                    },
                    runs,
                ) => Out::File {
                    path,
                    line,
                    column,
                    is_dir,
                    runs: runs_of(&runs),
                },
                LinkAt::Found(LinkTarget::Url(url), runs) => Out::Url {
                    url,
                    runs: runs_of(&runs),
                },
                LinkAt::Unresolved { .. } | LinkAt::None => Out::Unlinked,
            }
        })
    }

    /// What the pointer resting on a cell underlines, one run per row.
    fn hover(&self, cx: &mut TestAppContext, col: usize, row: usize) -> Option<Runs> {
        self.view.update(cx, |view, cx| {
            view.clear_hovered_link(cx);
            view.hover_link_at(col, row, true, cx).then(|| {
                runs_of(
                    &view
                        .hovered_link
                        .as_ref()
                        .expect("a hovered link was reported")
                        .runs,
                )
            })
        })
    }

    /// The file-open requests a modifier-click on a cell emits.
    fn click(&self, cx: &mut TestAppContext, col: usize, row: usize) -> Vec<Opened> {
        self.opened.borrow_mut().clear();
        let mut vcx = gpui::VisualTestContext::from_window(self.window.into(), cx);
        self.view.update_in(&mut vcx, |view, window, cx| {
            view.open_link_at(col, row, window, cx);
        });
        cx.run_until_parked();
        self.opened.borrow().clone()
    }

    /// Resolve, hover and click have to tell the same story about one cell.
    /// A URL is not clicked: that would ask the platform to open a browser.
    fn expect(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        (col, row): (usize, usize),
        expected: &Out,
    ) {
        let name = self.name;
        assert_80x24(cx, &self.view, name);
        assert_eq!(
            &self.resolve(cx, col, row),
            expected,
            "{name}: {what}: resolving ({col},{row})"
        );
        let underline = match expected {
            Out::File { runs, .. } | Out::Url { runs, .. } => Some(runs.clone()),
            Out::Unlinked => None,
        };
        assert_eq!(
            self.hover(cx, col, row),
            underline,
            "{name}: {what}: hovering ({col},{row})"
        );
        if let Out::File {
            path,
            line,
            column,
            is_dir,
            ..
        } = expected
        {
            assert_eq!(
                self.click(cx, col, row),
                vec![(path.clone(), *line, *column, *is_dir)],
                "{name}: {what}: clicking ({col},{row})"
            );
        }
    }

    fn expect_cells(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        at: &[(usize, usize)],
        expected: &Out,
    ) {
        for &cell in at {
            self.expect(cx, what, cell, expected);
        }
    }
}

fn target_rows(at: &[usize]) -> Vec<String> {
    let mut cuts = vec![0];
    cuts.extend_from_slice(at);
    cuts.push(TARGET.len());
    cuts.windows(2)
        .map(|w| TARGET[w[0]..w[1]].to_owned())
        .collect()
}

fn target_case(
    name: &'static str,
    bytes: &[u8],
    cuts: &[usize],
    decorate: impl Fn(usize, &str) -> String,
) -> Case {
    let rows: Vec<String> = target_rows(cuts)
        .iter()
        .enumerate()
        .map(|(i, row)| decorate(i, row))
        .collect();
    Case {
        name,
        bytes: bytes.to_vec(),
        cwd: Cwd::Root,
        rows,
    }
}

fn plain(_: usize, row: &str) -> String {
    row.to_owned()
}

/// Replays `case` in a fresh scratch tree and expects the whole of `TARGET`
/// to be one link, spelled by `segs`, from every cell of every row.
fn expect_one_link(cx: &mut TestAppContext, case: Case, segs: Vec<Seg>) {
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let expected = file(&scratch.root, TARGET, &segs);
    replay.expect_cells(cx, "the recorded path", &cells(&segs), &expected);
}

// ---------------------------------------------------------------------------
// Controls that are already green: seams the terminal itself makes.
// ---------------------------------------------------------------------------

#[gpui::test]
fn control_a_soft_wrapped_path_is_one_link(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        case("soft-wrap", capture!("soft-wrap"), Cwd::Root, &[TARGET]),
        vec![seg(0, 0, &TARGET[..80]), seg(1, 0, &TARGET[80..])],
    );
}

#[gpui::test]
fn control_a_hard_break_at_the_terminal_width_is_one_link(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "hard-wrap-at-width",
            capture!("hard-wrap-at-width"),
            &[80],
            plain,
        ),
        vec![seg(0, 0, &TARGET[..80]), seg(1, 0, &TARGET[80..])],
    );
}

// ---------------------------------------------------------------------------
// Real application traces. Each is a control: what a tool prints already
// links, and nothing a multi-row reader adds may glue a heading to its rows.
// ---------------------------------------------------------------------------

#[gpui::test]
fn control_a_real_rustc_location_wrapped_by_the_terminal(cx: &mut TestAppContext) {
    let arrow = format!(" --> {TARGET}:2:22");
    let case = case(
        "real-rustc-diagnostic",
        capture!("real-rustc-diagnostic"),
        Cwd::Real,
        &[
            "error[E0308]: mismatched types",
            arrow.as_str(),
            "  |",
            "2 |     let value: u32 = \"neutral\";",
            "  |                ---   ^^^^^^^^^ expected `u32`, found `&str`",
            "  |                |",
            "  |                expected due to this",
            "",
            "error: aborting due to 1 previous error",
            "",
            "For more information about this error, try `rustc --explain E0308`.",
        ],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![
        seg(1, 5, &TARGET[..75]),
        seg(2, 0, &format!("{}:2:22", &TARGET[75..])),
    ];
    let expected = file_with(
        &scratch.cwd(Cwd::Real),
        TARGET,
        ":2:22",
        Some(2),
        Some(22),
        &segs,
    );
    replay.expect_cells(cx, "rustc --> location", &cells(&segs), &expected);
}

#[gpui::test]
fn control_a_real_git_status_path_wrapped_by_the_terminal(cx: &mut TestAppContext) {
    let status = format!("?? {TARGET}");
    let case = case(
        "real-git-status",
        capture!("real-git-status"),
        Cwd::Real,
        &[status.as_str()],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 3, &TARGET[..77]), seg(1, 0, &TARGET[77..])];
    let expected = file(&scratch.cwd(Cwd::Real), TARGET, &segs);
    replay.expect_cells(cx, "git status path", &cells(&segs), &expected);
}

#[gpui::test]
fn control_a_real_rg_heading_is_one_link_and_its_match_rows_are_not(cx: &mut TestAppContext) {
    let case = case(
        "real-rg-heading",
        capture!("real-rg-heading"),
        Cwd::Real,
        &[TARGET, "2:    let value: u32 = \"neutral\";"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, &TARGET[..80]), seg(1, 0, &TARGET[80..])];
    let expected = file(&scratch.cwd(Cwd::Real), TARGET, &segs);
    replay.expect_cells(cx, "rg heading", &cells(&segs), &expected);
    // The numbered match row is its own line: `2:` never extends the heading
    // and the heading never becomes a prefix of anything.
    for cell in [(0, 2), (1, 2), (10, 2)] {
        replay.expect(cx, "rg match row", cell, &Out::Unlinked);
    }
}

#[gpui::test]
fn control_real_ls_names_stay_whole_and_are_never_glued(cx: &mut TestAppContext) {
    let case = case(
        "real-ls-narrow-columns",
        capture!("real-ls-narrow-columns"),
        Cwd::Listing,
        &LISTING,
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    for (row, name) in LISTING.iter().enumerate() {
        let segs = vec![seg(row as i32, 0, name)];
        let expected = file(&scratch.cwd(Cwd::Listing), name, &segs);
        replay.expect_cells(cx, "ls entry", &cells(&segs), &expected);
    }
}

#[gpui::test]
fn control_a_real_git_diff_stat_elision_is_not_a_path(cx: &mut TestAppContext) {
    let case = case(
        "real-git-diff-stat",
        capture!("real-git-diff-stat"),
        Cwd::Root,
        &[
            " .../neutral-preview-target.rs          | 3 ++-",
            " 1 file changed, 2 insertions(+), 1 deletion(-)",
        ],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    for cell in [(1, 0), (10, 0), (28, 0)] {
        replay.expect(cx, "elided stat path", cell, &Out::Unlinked);
    }
}

// ---------------------------------------------------------------------------
// The misses: a path a program wrapped itself. Each is its own test so the
// failures stay distinct.
// ---------------------------------------------------------------------------

#[gpui::test]
fn a_path_wrapped_before_the_terminal_width_is_one_link(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "program-wrap-before-width",
            capture!("program-wrap-before-width"),
            &[46],
            plain,
        ),
        vec![seg(0, 0, &TARGET[..46]), seg(1, 0, &TARGET[46..])],
    );
}

#[gpui::test]
fn an_indented_continuation_is_one_link_and_the_indent_is_not_underlined(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "indented-continuation",
            capture!("indented-continuation"),
            &[46],
            |i, row| match i {
                0 => row.to_owned(),
                _ => format!("    {row}"),
            },
        ),
        vec![seg(0, 0, &TARGET[..46]), seg(1, 4, &TARGET[46..])],
    );
}

#[gpui::test]
fn a_path_wrapped_inside_a_box_is_one_link_without_the_borders(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "box-inset-wrap",
            capture!("box-inset-wrap"),
            &[46],
            |_, row| format!("│ {row:<66} │"),
        ),
        vec![seg(0, 2, &TARGET[..46]), seg(1, 2, &TARGET[46..])],
    );
}

#[gpui::test]
fn a_path_behind_a_left_gutter_is_one_link_without_the_gutter(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "left-gutter-wrap",
            capture!("left-gutter-wrap"),
            &[46],
            |_, row| format!("│   {row}"),
        ),
        vec![seg(0, 4, &TARGET[..46]), seg(1, 4, &TARGET[46..])],
    );
}

#[gpui::test]
fn a_path_behind_a_numbered_gutter_is_one_link_without_the_numbers(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "numbered-gutter-wrap",
            capture!("numbered-gutter-wrap"),
            &[46],
            |i, row| format!(" {} │ {row}", i + 1),
        ),
        vec![seg(0, 5, &TARGET[..46]), seg(1, 5, &TARGET[46..])],
    );
}

#[gpui::test]
fn a_path_split_after_a_slash_is_one_link(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "split-after-slash",
            capture!("split-after-slash"),
            &[62],
            plain,
        ),
        vec![seg(0, 0, &TARGET[..62]), seg(1, 0, &TARGET[62..])],
    );
}

/// Three rows, and the first two joined are `name/.../neutral-preview-target.r`:
/// a window that ends in what looks like an extension and misses on disk. Only
/// the whole path is a file, and it has to win over the partial windows.
#[gpui::test]
fn a_path_split_after_a_hyphen_over_three_rows_is_one_link(cx: &mut TestAppContext) {
    expect_one_link(
        cx,
        target_case(
            "split-after-hyphen",
            capture!("split-after-hyphen"),
            &[43, 86],
            plain,
        ),
        vec![
            seg(0, 0, &TARGET[..43]),
            seg(1, 0, &TARGET[43..86]),
            seg(2, 0, &TARGET[86..]),
        ],
    );
}

#[gpui::test]
fn a_line_and_column_split_across_rows_still_navigates(cx: &mut TestAppContext) {
    let case = case(
        "split-location-suffix",
        capture!("split-location-suffix"),
        Cwd::Root,
        &["sources/location.rs:12:", "7"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, "sources/location.rs:12:"), seg(1, 0, "7")];
    let expected = file_with(&scratch.root, LOCATION, ":12:7", Some(12), Some(7), &segs);
    replay.expect_cells(cx, "line:column suffix", &cells(&segs), &expected);
}

/// The spaces of an explicitly quoted path are part of it, and the break falls
/// inside a later component. The quotes themselves are not underlined.
#[gpui::test]
fn a_quoted_path_with_spaces_wrapped_inside_a_component_is_one_link(cx: &mut TestAppContext) {
    let case = case(
        "quoted-spaces-wrap",
        capture!("quoted-spaces-wrap"),
        Cwd::Root,
        &[
            "\"sources/reports/neutral folder/qu",
            "oted-preview-target.rs\"",
        ],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![
        seg(0, 1, "sources/reports/neutral folder/qu"),
        seg(1, 0, "oted-preview-target.rs"),
    ];
    let mut at = cells(&segs);
    at.push((25, 0)); // inside `folder`, across the interior space
    let expected = file(&scratch.root, QUOTED, &segs);
    replay.expect_cells(cx, "quoted path", &at, &expected);
}

// ---------------------------------------------------------------------------
// Negatives. Each recorded spelling exists on disk in the scratch tree, so a
// join that is only excused by a filesystem hit would be caught here.
// ---------------------------------------------------------------------------

#[gpui::test]
fn two_independent_paths_stay_two_links_though_their_concatenation_exists(cx: &mut TestAppContext) {
    let case = case(
        "two-independent-paths",
        capture!("two-independent-paths"),
        Cwd::Root,
        &["notes/first.rs", "notes/second.rs"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    for (row, rel) in ["notes/first.rs", "notes/second.rs"].iter().enumerate() {
        let segs = vec![seg(row as i32, 0, rel)];
        let expected = file(&scratch.root, rel, &segs);
        replay.expect_cells(cx, "one of two paths", &cells(&segs), &expected);
    }
}

#[gpui::test]
fn prose_before_a_path_is_not_a_continuation(cx: &mut TestAppContext) {
    let case = case(
        "prose-before-path",
        capture!("prose-before-path"),
        Cwd::Root,
        &["read the output", "notes/second.rs"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(1, 0, "notes/second.rs")];
    let expected = file(&scratch.root, "notes/second.rs", &segs);
    replay.expect_cells(cx, "the path under the prose", &cells(&segs), &expected);
    for cell in [(0, 0), (6, 0), (12, 0)] {
        replay.expect(cx, "the prose", cell, &Out::Unlinked);
    }
}

#[gpui::test]
fn an_unrelated_word_after_a_path_is_not_a_continuation(cx: &mut TestAppContext) {
    let case = case(
        "path-before-word",
        capture!("path-before-word"),
        Cwd::Root,
        &["notes/first.rs", "unrelated"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, "notes/first.rs")];
    let expected = file(&scratch.root, "notes/first.rs", &segs);
    replay.expect_cells(cx, "the path", &cells(&segs), &expected);
    for cell in [(0, 1), (8, 1)] {
        replay.expect(cx, "the word", cell, &Out::Unlinked);
    }
}

#[gpui::test]
fn indentation_alone_does_not_make_a_word_a_continuation(cx: &mut TestAppContext) {
    let case = case(
        "path-before-indented-word",
        capture!("path-before-indented-word"),
        Cwd::Root,
        &["notes/first.rs", "    unrelated"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, "notes/first.rs")];
    let expected = file(&scratch.root, "notes/first.rs", &segs);
    replay.expect_cells(cx, "the path", &cells(&segs), &expected);
    for cell in [(4, 1), (12, 1)] {
        replay.expect(cx, "the indented word", cell, &Out::Unlinked);
    }
}

#[gpui::test]
fn a_blank_row_ends_a_path(cx: &mut TestAppContext) {
    let case = case(
        "blank-row-separator",
        capture!("blank-row-separator"),
        Cwd::Root,
        &["notes/first.rs", "", "notes/second.rs"],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    for (row, rel) in [(0, "notes/first.rs"), (2, "notes/second.rs")] {
        let segs = vec![seg(row, 0, rel)];
        let expected = file(&scratch.root, rel, &segs);
        replay.expect_cells(cx, "a path beside a blank row", &cells(&segs), &expected);
    }
}

#[gpui::test]
fn different_gutters_are_not_stripped_into_a_joined_path(cx: &mut TestAppContext) {
    let case = case(
        "mismatched-gutters",
        capture!("mismatched-gutters"),
        Cwd::Root,
        &[
            "│ sources/reports/component-with-a-very-long-nam",
            "┃ e/release-notes/neutral-preview-target.rs",
        ],
    );
    let scratch = Scratch::new(case.name, &[]);
    let replay = Replay::new(cx, &scratch, &case);
    for cell in [(2, 0), (30, 0), (2, 1), (20, 1)] {
        replay.expect(cx, "mismatched gutters", cell, &Out::Unlinked);
    }
}

// ---------------------------------------------------------------------------
// Positive guards the structural rules must not break.
// ---------------------------------------------------------------------------

/// The first row alone is a real directory, and the tail alone is a real file
/// in the pane's own directory. The joined path is also real, and it is what
/// the text says: it wins.
#[gpui::test]
fn a_joined_file_beats_a_directory_prefix_and_a_cwd_file_of_the_tail_name(cx: &mut TestAppContext) {
    let case = target_case(
        "split-after-slash-with-decoy",
        capture!("split-after-slash"),
        &[62],
        plain,
    );
    let scratch = Scratch::new(case.name, &["neutral-preview-target.rs"]);
    let replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, &TARGET[..62]), seg(1, 0, &TARGET[62..])];
    let expected = file(&scratch.root, TARGET, &segs);
    replay.expect_cells(
        cx,
        "joined beats prefix and decoy",
        &cells(&segs),
        &expected,
    );
}

/// A URL keeps winning over a file reading of the same cells, even when the
/// next row would complete an existing file path out of it.
#[gpui::test]
fn a_url_still_wins_over_a_joined_file_reading(cx: &mut TestAppContext) {
    // Not a capture: a labelled synthetic fixture, with a file that exists at
    // the path a careless join of the two rows would carve out of the URL.
    let case = case(
        "url-then-path-tail",
        b"https://example.test/sources/\r\nlocation.rs\r\n",
        Cwd::Root,
        &["https://example.test/sources/", "location.rs"],
    );
    let scratch = Scratch::new(case.name, &["example.test/sources/location.rs"]);
    let replay = Replay::new(cx, &scratch, &case);
    for cell in [(0, 0), (10, 0), (20, 0)] {
        match replay.resolve(cx, cell.0, cell.1) {
            Out::Url { url, runs } => {
                assert!(
                    url.starts_with("https://example.test/sources"),
                    "the URL is the whole of the first row, not a file: {url}"
                );
                assert!(
                    runs.iter().all(|(line, _, _)| *line == 0),
                    "and it underlines only its own row: {runs:?}"
                );
            }
            other => panic!("a URL must keep winning at {cell:?}, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The grid changing under a link.
// ---------------------------------------------------------------------------

#[gpui::test]
fn a_joined_link_does_not_outlive_the_text_it_was_read_from(cx: &mut TestAppContext) {
    let case = target_case(
        "program-wrap-then-cleared",
        capture!("program-wrap-before-width"),
        &[46],
        plain,
    );
    let scratch = Scratch::new(case.name, &[]);
    let mut replay = Replay::new(cx, &scratch, &case);
    let segs = vec![seg(0, 0, &TARGET[..46]), seg(1, 0, &TARGET[46..])];
    let expected = file(&scratch.root, TARGET, &segs);
    replay.expect(cx, "before the screen changes", (3, 0), &expected);

    replay.feed(cx, b"\x1b[2J\x1b[Hchanged\r\n", &["changed".to_owned()]);
    replay.view.update(cx, |view, cx| {
        view.refresh_link_hover(true, cx);
    });
    for cell in [(3, 0), (3, 1)] {
        replay.expect(cx, "after the screen changes", cell, &Out::Unlinked);
    }
}

// ---------------------------------------------------------------------------
// Remote panes: the host answers later, and this machine's files say nothing.
// ---------------------------------------------------------------------------

// These cases replay relative paths under the native temp dir and inject the
// remote probe replies; neither their stream pair nor their cwd requires Unix.
mod remote {
    use super::*;
    use crate::terminal::link_probe::Existence;

    /// A pane whose paths live on another machine, over the program-wrap
    /// capture. The scratch tree exists on THIS machine too: a join validated
    /// against it would be validating against the wrong filesystem.
    fn remote_replay(cx: &mut TestAppContext, scratch: &Scratch) -> Replay {
        let case = target_case(
            "program-wrap-before-width-remote",
            capture!("program-wrap-before-width"),
            &[46],
            plain,
        );
        let replay = Replay::new(cx, scratch, &case);
        replay.view.update(cx, |view, cx| {
            let _ = bind_to_a_disconnected_remote_workspace(view, cx);
        });
        assert!(
            !replay.view.update(cx, |view, _| view.host_id().is_local()),
            "SETUP FAILURE: the pane is not remote"
        );
        assert_eq!(
            replay.view.update(cx, |view, _| view.cwd()),
            Some(scratch.root.clone()),
            "SETUP FAILURE: binding the workspace moved the pane's directory"
        );
        replay
    }

    fn land(cx: &mut TestAppContext, replay: &Replay, answers: Vec<(PathBuf, Existence)>) {
        replay.view.update(cx, |view, cx| {
            if view.link_probes.land(answers) {
                view.recompute_link_hover(cx);
            }
        });
    }

    fn segs() -> Vec<Seg> {
        vec![seg(0, 0, &TARGET[..46]), seg(1, 0, &TARGET[46..])]
    }

    /// Unknown stays unjoined: nothing underlines, even though the same path
    /// exists on the laptop. What the join needs from the host is asked for.
    #[gpui::test]
    fn an_unanswered_join_is_not_underlined_but_is_asked_for(cx: &mut TestAppContext) {
        let scratch = Scratch::new("remote-unknown", &[]);
        let replay = remote_replay(cx, &scratch);
        for cell in [(3, 0), (3, 1)] {
            replay.expect(cx, "before the host answers", cell, &Out::Unlinked);
        }
        let wanted = replay
            .view
            .update(cx, |view, _| view.link_probes.take_wanted());
        assert!(
            wanted.contains(&scratch.at(TARGET)),
            "the joined path has to be asked for, so the next hover can find it: {wanted:?}"
        );
    }

    /// A reply that says the joined path is a file joins exactly like a local
    /// hit, on every row.
    #[gpui::test]
    fn a_remote_hit_joins_like_a_local_one(cx: &mut TestAppContext) {
        let scratch = Scratch::new("remote-hit", &[]);
        let replay = remote_replay(cx, &scratch);
        let _ = replay.resolve(cx, 3, 0);
        land(
            cx,
            &replay,
            vec![
                (scratch.at(TARGET), Existence::File),
                (scratch.at(&TARGET[..46]), Existence::Missing),
                (scratch.at(&TARGET[46..]), Existence::Missing),
            ],
        );
        let expected = file(&scratch.root, TARGET, &segs());
        // Clicking on a remote pane never reaches a local opener.
        replay.expect_cells(cx, "after a remote hit", &cells(&segs()), &expected);
    }

    #[gpui::test]
    fn a_remote_miss_leaves_the_rows_unlinked(cx: &mut TestAppContext) {
        let scratch = Scratch::new("remote-miss", &[]);
        let replay = remote_replay(cx, &scratch);
        let _ = replay.resolve(cx, 3, 0);
        land(
            cx,
            &replay,
            vec![
                (scratch.at(TARGET), Existence::Missing),
                (scratch.at(&TARGET[..46]), Existence::Missing),
                (scratch.at(&TARGET[46..]), Existence::Missing),
            ],
        );
        for cell in [(3, 0), (3, 1)] {
            replay.expect(cx, "after a remote miss", cell, &Out::Unlinked);
        }
    }

    /// A late reply cannot link text that is no longer there.
    #[gpui::test]
    fn a_late_remote_hit_does_not_link_changed_content(cx: &mut TestAppContext) {
        let scratch = Scratch::new("remote-late", &[]);
        let mut replay = remote_replay(cx, &scratch);
        let _ = replay.resolve(cx, 3, 0);
        replay.feed(cx, b"\x1b[2J\x1b[Hchanged\r\n", &["changed".to_owned()]);
        land(cx, &replay, vec![(scratch.at(TARGET), Existence::File)]);
        for cell in [(3, 0), (3, 1)] {
            replay.expect(cx, "after the screen changed", cell, &Out::Unlinked);
        }
    }
}
