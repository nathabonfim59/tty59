//! File paths a program wrapped across rows by itself.
//!
//! The terminal's own soft wraps, and a hard break at the last column, are
//! already read as one line by `smart_select::logical_line_at`. What that
//! cannot see is a path an application broke *before* the right edge, with a
//! hard newline, and often put an indent, a border or a line-number gutter in
//! front of the continuation. This module reads those rows back into
//! candidate text: [`windows_at`] hands out bounded [`Window`]s, each the text
//! of a few consecutive rows with the gutters, indentation and padding taken
//! out, together with the grid point of every character that is left.
//!
//! Nothing here looks at the filesystem. A window is only a *reading*: whether
//! the joined text names something is settled by
//! [`search::joined_file_link`](super::search::joined_file_link), through the
//! same probe a single row goes through. The checks here decide whether a
//! reading is worth asking about at all, and they are deliberately strict,
//! because a filesystem hit alone must never excuse a join.
//!
//! What the structure allows. Two adjacent rows are joinable at a *seam* when
//! all of these hold:
//!
//! - neither row is soft-wrapped, a wrap continuation or anything with an
//!   OSC 8 hyperlink (those have their own readings);
//! - both rows carry the same frame: no frame, or the same left border (and
//!   the same right border column), or the same numbered gutter. A `│` over a
//!   `┃` is not the same frame, and nothing is stripped across the mismatch;
//! - the lower row starts with a token that can be the rest of a path: not
//!   another absolute or `./`-style path, not a URL;
//! - there is evidence the break was the program's: both rows hold nothing but
//!   that one token (a quote opened on the upper row and closed on the lower
//!   one counts as one token, interior spaces included), or the rows share a
//!   border or numbered gutter, or the lower row is indented deeper.
//!
//! Eligibility of a seam and anchoring of a window are separate questions. A
//! seam only has to be *structurally* a break; a window, to be read at all,
//! has to start at a token that looks like a path (a `/`, a leading `~`, a
//! drive or a backslash, and not a URL). Later fragments of the same path
//! carry no such shape of their own, so a basename broken in three
//! (`src/a-very-long-` / `filename-middle-` / `component.rs`) joins from any of
//! its rows, while a bare word over a path (`read` / `notes/second.rs`) is
//! anchored on nothing and does not.
//!
//! Limits, stated because they are chosen: at most [`MAX_JOIN_ROWS`] rows in
//! total per window, at most [`MAX_JOIN_CHARS`] characters, and every span of
//! those rows that holds the row under the pointer, longest first
//! ([`MAX_JOIN_WINDOWS`] is how many that can be). Prose around the token
//! without a gutter or indent, a break *at* a space, prompt-style markers
//! (`>`, `⎿`) and soft wraps mixed with hard ones are not read.

use tty59_vt::VtTermView;
use tty59_vt::model::{CellWidth, GridPoint};

/// Rows in one window, the row under the pointer included.
pub(super) const MAX_JOIN_ROWS: usize = 8;

/// Characters in one window's text.
pub(super) const MAX_JOIN_CHARS: usize = 4096;

/// The most windows one lookup can hand out: every span of at most
/// [`MAX_JOIN_ROWS`] rows that holds the row under the pointer and is not that
/// row alone. With the pointer on row 3 or 4 of eight that is 4 x 5 - 1. They
/// come longest first, and none is dropped: a valid two-row join must not
/// vanish because independent path rows happen to sit next to it. What really
/// bounds the work is [`search::MAX_JOIN_PROBES`](super::search::MAX_JOIN_PROBES).
pub(super) const MAX_JOIN_WINDOWS: usize = 19;

/// What a table or a box draws down its edges. The same set `table_cell_at`
/// reads; a bare `|` is a shell pipe far more often than a border.
const BORDERS: [char; 3] = ['│', '┃', '║'];

/// A few consecutive rows, read as one piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Window {
    /// The kept cells of every row, in order, with nothing inserted between
    /// rows. Gutters, indentation and padding are not in it.
    pub text: String,
    /// The grid point of every character of `text`, one for one.
    pub points: Vec<GridPoint>,
    /// Which character of `text` the pointer is over.
    pub click_idx: usize,
    /// For each seam, the index of the first character after it.
    pub seams: Vec<usize>,
}

/// What stands left of a row's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Plain,
    Border { ch: char, col: u16 },
    Numbered { ch: char, col: u16 },
}

struct Row {
    line: i32,
    /// The text cells that are left once the frame, indentation and padding
    /// are off, as the character and its column. Wide characters appear once,
    /// at the column they start at.
    cells: Vec<(char, u16)>,
    indent: usize,
    frame: Frame,
    right: Option<(char, u16)>,
    linked: bool,
}

fn read_row(term: &VtTermView, line: i32) -> Row {
    let mut all: Vec<(char, u16)> = Vec::with_capacity(term.columns());
    let mut linked = false;
    for col in 0..term.columns() {
        let glyph = term.glyph(GridPoint {
            line,
            col: col as u16,
        });
        linked |= glyph.hyperlink.is_some();
        if matches!(glyph.wide, CellWidth::SpacerTail | CellWidth::SpacerHead) {
            continue;
        }
        all.push((glyph.c, col as u16));
    }
    while all.last().is_some_and(|&(c, _)| c.is_whitespace()) {
        all.pop();
    }

    let mut at = 0;
    while all.get(at).is_some_and(|&(c, _)| c == ' ') {
        at += 1;
    }
    let indent = at;
    let mut frame = Frame::Plain;
    if let Some(&(c, col)) = all.get(at) {
        if BORDERS.contains(&c) {
            frame = Frame::Border { ch: c, col };
            at += 1;
        } else if c.is_ascii_digit() {
            let mut k = at;
            while all.get(k).is_some_and(|&(d, _)| d.is_ascii_digit()) {
                k += 1;
            }
            while all.get(k).is_some_and(|&(d, _)| d == ' ') {
                k += 1;
            }
            if let Some(&(b, bcol)) = all.get(k)
                && BORDERS.contains(&b)
            {
                frame = Frame::Numbered { ch: b, col: bcol };
                at = k + 1;
            }
        }
    }
    if frame != Frame::Plain {
        while all.get(at).is_some_and(|&(c, _)| c == ' ') {
            at += 1;
        }
    }

    let mut end = all.len();
    let mut right = None;
    if frame != Frame::Plain && end > at {
        let (c, col) = all[end - 1];
        if BORDERS.contains(&c) {
            right = Some((c, col));
            end -= 1;
            while end > at && all[end - 1].0 == ' ' {
                end -= 1;
            }
        }
    }
    let cells = all[at.min(end)..end].to_vec();
    Row {
        line,
        cells,
        indent,
        frame,
        right,
        linked,
    }
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

/// A row that opens a quote and does not close it. Its interior spaces belong
/// to the quoted text, so the row is one token however many words it holds.
fn open_quote(cells: &[(char, u16)]) -> bool {
    match cells.first() {
        Some(&(q, _)) if is_quote(q) => !cells[1..].iter().any(|&(c, _)| c == q),
        _ => false,
    }
}

/// Whether the row is nothing but one token, which is what makes a break
/// before the right edge the program's doing and not prose that ended early.
fn is_pure(cells: &[(char, u16)]) -> bool {
    !cells.is_empty() && (!cells.iter().any(|&(c, _)| c.is_whitespace()) || open_quote(cells))
}

/// The token the row ends in.
fn last_token(cells: &[(char, u16)]) -> String {
    if open_quote(cells) {
        return cells[1..].iter().map(|&(c, _)| c).collect();
    }
    let start = cells
        .iter()
        .rposition(|&(c, _)| c.is_whitespace())
        .map_or(0, |i| i + 1);
    cells[start..].iter().map(|&(c, _)| c).collect()
}

/// The token the row starts with.
fn first_token(cells: &[(char, u16)]) -> String {
    cells
        .iter()
        .map(|&(c, _)| c)
        .take_while(|c| !c.is_whitespace())
        .collect()
}

fn path_shaped(token: &str) -> bool {
    !token.contains("://")
        && (token.contains('/') || token.contains('\\') || token.starts_with('~'))
}

/// Whether the token can be the rest of a path that began on the row above:
/// something that goes on a name, not something that starts one.
fn is_tail(token: &str) -> bool {
    let head: Vec<char> = token.chars().take(3).collect();
    let Some(&first) = head.first() else {
        return false;
    };
    let second = head.get(1).copied();
    let third = head.get(2).copied();
    let starts_a_path = matches!(first, '/' | '\\' | '~')
        || (first == '.' && matches!(second, Some('/' | '.')))
        || (first.is_ascii_alphabetic()
            && second == Some(':')
            && matches!(third, Some('/' | '\\')));
    !starts_a_path
        && !token.contains("://")
        && (first.is_alphanumeric()
            || matches!(first, '.' | '_' | '-' | ':' | '+' | '=' | '%' | '#' | ','))
}

/// Whether the row ends in a token a path could start at. Where a window
/// *starts* is the only place this is asked: the fragments after it are not
/// path-shaped by themselves.
fn anchors_a_path(row: &Row) -> bool {
    path_shaped(&last_token(&row.cells))
}

/// Whether `upper` and the row under it can be one path broken in two,
/// judged by the rows' structure alone.
fn seam_ok(term: &VtTermView, top: i32, upper: &Row, lower: &Row) -> bool {
    if upper.linked || lower.linked || upper.cells.is_empty() || lower.cells.is_empty() {
        return false;
    }
    let wraps = |line: i32| term.row_wraps(line);
    if wraps(upper.line) || wraps(lower.line) || (upper.line > top && wraps(upper.line - 1)) {
        return false;
    }
    if upper.frame != lower.frame || upper.right != lower.right {
        return false;
    }
    let both_pure = is_pure(&upper.cells) && is_pure(&lower.cells);
    let framed = upper.frame != Frame::Plain;
    if !both_pure && !framed && lower.indent <= upper.indent {
        return false;
    }
    is_tail(&first_token(&lower.cells))
}

fn build(rows: &[Row], from: usize, to: usize, click_line: i32, click_col: u16) -> Option<Window> {
    if !anchors_a_path(&rows[from]) {
        return None;
    }
    let mut text = String::new();
    let mut points = Vec::new();
    let mut seams = Vec::new();
    let mut click_idx = None;
    for (k, row) in rows[from..=to].iter().enumerate() {
        if k > 0 {
            seams.push(points.len());
        }
        for &(c, col) in &row.cells {
            if row.line == click_line && col == click_col {
                click_idx = Some(points.len());
            }
            text.push(c);
            points.push(GridPoint {
                line: row.line,
                col,
            });
            if points.len() > MAX_JOIN_CHARS {
                return None;
            }
        }
    }
    Some(Window {
        text,
        points,
        click_idx: click_idx?,
        seams,
    })
}

/// The windows the cell under `click` can be part of, longest first, none of
/// them shorter than two rows. Empty when the row has no joinable neighbour,
/// which is the case for nearly every cell on a screen.
pub(super) fn windows_at(term: &VtTermView, click: GridPoint) -> Vec<Window> {
    let top = term.topmost_line();
    let bottom = term.bottommost_line();
    if click.line < top || click.line > bottom || usize::from(click.col) >= term.columns() {
        return Vec::new();
    }
    // A click on the second column of a wide character is the character.
    let col = match term.glyph(click).wide {
        CellWidth::SpacerTail if click.col > 0 => click.col - 1,
        _ => click.col,
    };
    let home = read_row(term, click.line);
    if home.linked || !home.cells.iter().any(|&(_, c)| c == col) {
        return Vec::new();
    }

    let mut rows = vec![home];
    let (mut lo, mut hi) = (click.line, click.line);
    let (mut up, mut down) = (true, true);
    while rows.len() < MAX_JOIN_ROWS && (up || down) {
        if up {
            up = false;
            if lo > top {
                let row = read_row(term, lo - 1);
                if seam_ok(term, top, &row, &rows[0]) {
                    rows.insert(0, row);
                    lo -= 1;
                    up = true;
                }
            }
        }
        if down && rows.len() < MAX_JOIN_ROWS {
            down = false;
            if hi < bottom {
                let row = read_row(term, hi + 1);
                if seam_ok(term, top, &rows[rows.len() - 1], &row) {
                    rows.push(row);
                    hi += 1;
                    down = true;
                }
            }
        }
    }
    if rows.len() < 2 {
        return Vec::new();
    }

    let at = (click.line - lo) as usize;
    // A window starts at or above the pointer's row; if none of those rows can
    // anchor a path there is nothing to build.
    if !rows[..=at].iter().any(anchors_a_path) {
        return Vec::new();
    }
    let mut spans: Vec<(usize, usize)> = (0..=at)
        .flat_map(|a| {
            (at..rows.len())
                .filter(move |&b| b > a)
                .map(move |b| (a, b))
        })
        .collect();
    spans.sort_by_key(|&(a, b)| (std::cmp::Reverse(b - a), a));
    spans
        .iter()
        .filter_map(|&(a, b)| build(&rows, a, b, click.line, col))
        .take(MAX_JOIN_WINDOWS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_with(cols: usize, rows: usize, input: &str) -> tty59_vt::VtTerm {
        let mut term = tty59_vt::VtTerm::new(cols as u16, rows as u16, 10_000);
        term.feed(input.as_bytes());
        term
    }

    fn at(term: &tty59_vt::VtTerm, line: i32, col: u16) -> Vec<Window> {
        windows_at(&term.view(), GridPoint { line, col })
    }

    fn p(line: i32, col: u16) -> GridPoint {
        GridPoint { line, col }
    }

    #[test]
    fn a_path_broken_before_the_edge_is_one_window_with_an_exact_point_map() {
        let term = term_with(40, 6, "sources/aaa/bb\r\ncccc/dd.rs\r\n");
        for (line, col) in [(0, 3), (0, 13), (1, 0), (1, 9)] {
            let windows = at(&term, line, col);
            assert_eq!(windows.len(), 1, "click ({line},{col})");
            let w = &windows[0];
            assert_eq!(w.text, "sources/aaa/bbcccc/dd.rs");
            assert_eq!(w.seams, vec![14]);
            let expected: Vec<GridPoint> = (0..14)
                .map(|c| p(0, c))
                .chain((0..10).map(|c| p(1, c)))
                .collect();
            assert_eq!(w.points, expected);
            assert_eq!(w.points[w.click_idx], p(line, col));
        }
    }

    #[test]
    fn indentation_and_borders_are_not_part_of_the_window() {
        let term = term_with(40, 6, "sources/aaa/bb\r\n    cccc/dd.rs\r\n");
        let w = &at(&term, 1, 5)[0];
        assert_eq!(w.text, "sources/aaa/bbcccc/dd.rs");
        assert_eq!(w.points[14], p(1, 4), "the indent is skipped");

        let term = term_with(
            40,
            6,
            "│ sources/aaa/bb      │\r\n│ cccc/dd.rs          │\r\n",
        );
        let w = &at(&term, 0, 4)[0];
        assert_eq!(w.text, "sources/aaa/bbcccc/dd.rs");
        assert_eq!(w.points[0], p(0, 2));
        assert_eq!(w.points[13], p(0, 15));
        assert_eq!(w.points[14], p(1, 2));
        assert_eq!(w.points[23], p(1, 11));

        let term = term_with(40, 6, " 1 │ sources/aaa/bb\r\n 2 │ cccc/dd.rs\r\n");
        let w = &at(&term, 0, 6)[0];
        assert_eq!(w.text, "sources/aaa/bbcccc/dd.rs");
        assert_eq!(w.points[0], p(0, 5));
        assert_eq!(w.points[14], p(1, 5));
    }

    #[test]
    fn different_frames_are_never_stripped_into_a_window() {
        let term = term_with(40, 6, "│ sources/aaa/bb\r\n┃ cccc/dd.rs\r\n");
        assert!(at(&term, 0, 4).is_empty());
        assert!(at(&term, 1, 4).is_empty());

        // A border over no border is a different frame too.
        let term = term_with(40, 6, "│ sources/aaa/bb\r\ncccc/dd.rs\r\n");
        assert!(at(&term, 0, 4).is_empty());

        // The right border has to sit in the same column.
        let term = term_with(40, 6, "│ sources/aaa/bb │\r\n│ cccc/dd.rs      │\r\n");
        assert!(at(&term, 0, 4).is_empty());
    }

    #[test]
    fn a_blank_row_ends_a_window() {
        let term = term_with(40, 6, "sources/aaa/bb\r\n\r\ncccc/dd.rs\r\n");
        assert!(at(&term, 0, 3).is_empty());
        assert!(at(&term, 2, 3).is_empty());
    }

    #[test]
    fn prose_without_a_gutter_or_an_indent_is_not_a_continuation() {
        let term = term_with(20, 6, "see a/bb/ccc/dddd/\r\nnotes.md here\r\n");
        assert!(at(&term, 0, 6).is_empty());
        assert!(at(&term, 1, 2).is_empty());
    }

    #[test]
    fn prose_with_an_indented_continuation_is_read() {
        let term = term_with(40, 6, "edit src/aaa/bb\r\n    cc/dd.rs and more\r\n");
        let w = &at(&term, 0, 7)[0];
        assert_eq!(w.text, "edit src/aaa/bbcc/dd.rs and more");
        assert_eq!(w.seams, vec![15]);
    }

    #[test]
    fn a_continuation_that_starts_a_new_path_or_a_url_is_not_one() {
        for lower in [
            "/abs/path.rs",
            "./rel/path.rs",
            "../up.rs",
            "~/home.rs",
            "C:\\x.rs",
        ] {
            let term = term_with(40, 6, &format!("sources/aaa/bb\r\n{lower}\r\n"));
            assert!(at(&term, 0, 3).is_empty(), "{lower}");
        }
        let term = term_with(40, 6, "https://example.test/aaa/\r\nbbb/ccc.rs\r\n");
        assert!(at(&term, 0, 3).is_empty(), "a URL row is not a path row");
    }

    #[test]
    fn the_terminals_own_wraps_are_not_this_modules() {
        let term = term_with(20, 6, "see a/bb/ccc/dddd/notes.md here\r\n");
        assert!(term.view().row_wraps(0));
        assert!(at(&term, 0, 6).is_empty());
        assert!(at(&term, 1, 2).is_empty());
    }

    #[test]
    fn a_window_has_at_most_eight_rows_in_total_and_contains_the_click() {
        let rows: String = (0..10).map(|i| format!("d{i:02}/\r\n")).collect();
        let term = term_with(40, 12, &rows);
        let windows = at(&term, 5, 1);
        assert!(!windows.is_empty() && windows.len() <= MAX_JOIN_WINDOWS);
        for w in &windows {
            assert!(w.seams.len() < MAX_JOIN_ROWS, "{} seams", w.seams.len());
            assert_eq!(w.points[w.click_idx], p(5, 1));
        }
        let longest = &windows[0];
        assert_eq!(longest.seams.len() + 1, MAX_JOIN_ROWS, "eight rows in all");
        let lines: Vec<i32> = longest.points.iter().map(|g| g.line).collect();
        assert_eq!(lines.iter().min(), Some(&1));
        assert_eq!(lines.iter().max(), Some(&8));
    }

    #[test]
    fn a_window_never_holds_more_than_the_character_cap() {
        let row = format!("d/{}\r\n", "a".repeat(598));
        let term = term_with(600, 12, &row.repeat(8));
        assert!(!term.view().row_wraps(0), "fixture rows are hard breaks");
        let windows = at(&term, 4, 3);
        assert!(!windows.is_empty());
        assert!(windows.iter().all(|w| w.points.len() <= MAX_JOIN_CHARS));
        assert_eq!(
            windows.iter().map(|w| w.seams.len() + 1).max(),
            Some(6),
            "six rows of 600 is the most that fits in 4096"
        );
    }

    #[test]
    fn a_wide_character_at_a_seam_keeps_one_point_and_both_columns_underline() {
        let term = term_with(20, 4, "src/文\r\n档/a.rs\r\n");
        for (line, col) in [(0, 4), (0, 5), (1, 0), (1, 1)] {
            let windows = at(&term, line, col);
            assert_eq!(windows.len(), 1, "click ({line},{col})");
            let w = &windows[0];
            assert_eq!(w.text, "src/文档/a.rs");
            assert_eq!((w.points[4], w.points[5]), (p(0, 4), p(1, 0)));
            let runs = crate::terminal::smart_select::row_runs(&term.view(), &w.points[0..=5]);
            assert_eq!(
                runs,
                vec![(p(0, 0), p(0, 5)), (p(1, 0), p(1, 1))],
                "a wide glyph takes its spacer column with it"
            );
        }
    }

    #[test]
    fn a_quote_opened_on_one_row_and_closed_on_the_next_is_one_token() {
        let term = term_with(
            80,
            4,
            "\"sources/reports/neutral folder/qu\r\noted-preview-target.rs\"\r\n",
        );
        let w = &at(&term, 0, 25)[0];
        assert_eq!(
            w.text,
            "\"sources/reports/neutral folder/quoted-preview-target.rs\""
        );
        assert_eq!(w.seams, vec![34]);
    }

    #[test]
    fn a_line_and_column_split_across_rows_is_one_window() {
        let term = term_with(40, 4, "sources/location.rs:12:\r\n7\r\n");
        let w = &at(&term, 1, 0)[0];
        assert_eq!(w.text, "sources/location.rs:12:7");
        assert_eq!(w.seams, vec![23]);
    }

    #[test]
    fn a_hyperlink_row_is_left_to_its_declared_extent() {
        let term = term_with(
            40,
            4,
            "\x1b]8;;https://e.test\x1b\\sources/aaa/bb\x1b]8;;\x1b\\\r\ncccc/dd.rs\r\n",
        );
        assert!(at(&term, 0, 3).is_empty(), "the linked row");
        assert!(at(&term, 1, 3).is_empty(), "its neighbour");
    }

    #[test]
    fn a_click_on_the_gutter_or_the_padding_has_no_window() {
        let term = term_with(
            40,
            4,
            "│ sources/aaa/bb      │\r\n│ cccc/dd.rs          │\r\n",
        );
        assert!(at(&term, 0, 0).is_empty(), "the border");
        assert!(at(&term, 0, 30).is_empty(), "the padding");
        assert!(at(&term, 0, 39).is_empty(), "past the border");
    }

    #[test]
    fn a_click_outside_the_grid_has_no_window() {
        let term = term_with(40, 4, "sources/aaa/bb\r\ncccc/dd.rs\r\n");
        assert!(at(&term, 0, 40).is_empty());
        assert!(at(&term, -1, 0).is_empty());
        assert!(at(&term, 99, 0).is_empty());
    }

    /// The rows are structurally a break, but nothing on them starts a path:
    /// a bare word over a path is two things.
    #[test]
    fn a_bare_word_over_a_path_anchors_nothing() {
        let term = term_with(40, 4, "read\r\nnotes/second.rs\r\n");
        assert!(at(&term, 0, 1).is_empty());
        assert!(at(&term, 1, 3).is_empty());

        let term = term_with(40, 4, "alpha\r\nbeta\r\ngamma.rs\r\n");
        for line in 0..3 {
            assert!(at(&term, line, 1).is_empty(), "row {line}");
        }
    }

    /// Only the window's first token has to look like a path. The fragments
    /// after it are bare names, and a click on any of them has the anchor.
    #[test]
    fn a_basename_broken_in_three_is_read_from_every_row() {
        let term = term_with(
            40,
            4,
            "src/a-very-long-\r\nfilename-middle-\r\ncomponent.rs\r\n",
        );
        for (line, col) in [(0, 4), (1, 4), (2, 4)] {
            let windows = at(&term, line, col);
            let longest = windows.first().expect("a window");
            assert_eq!(longest.text, "src/a-very-long-filename-middle-component.rs");
            assert_eq!(longest.seams, vec![16, 32]);
        }
        // From the last row the span that would start at the middle row has
        // no anchor of its own, so only the whole path is a window.
        let windows = at(&term, 2, 4);
        assert_eq!(windows.len(), 1, "{windows:?}");
        // From the middle row: the whole path, and the first two rows alone.
        let windows = at(&term, 1, 4);
        assert_eq!(windows.len(), 2, "{windows:?}");
    }

    /// Independent path rows above a real two-row join must not use up the
    /// windows: the minimal span holding the click is always offered.
    #[test]
    fn crowding_rows_do_not_hide_the_minimal_span() {
        let term = term_with(
            80,
            12,
            "notes/one.rs\r\nnotes/two.rs\r\nnotes/three.rs\r\nsrc/a-very-long-\r\nfilename.rs\r\n",
        );
        for line in [3, 4] {
            let windows = at(&term, line, 4);
            assert!(windows.len() <= MAX_JOIN_WINDOWS);
            let shortest = windows.last().expect("windows");
            assert_eq!(shortest.text, "src/a-very-long-filename.rs", "row {line}");
            assert_eq!(shortest.seams, vec![16]);
        }
    }

    #[test]
    fn the_window_count_bound_matches_the_row_bound() {
        // Every span of MAX_JOIN_ROWS rows that holds one given row and is
        // longer than that row alone, at its worst position.
        let worst = (0..MAX_JOIN_ROWS)
            .map(|at| (at + 1) * (MAX_JOIN_ROWS - at) - 1)
            .max();
        assert_eq!(worst, Some(MAX_JOIN_WINDOWS));
    }
}

// Reviewer VT fixtures, not claimed as additional recorded application traces.
#[cfg(all(test, unix))]
mod lead_path_join_review {
    use super::*;
    use crate::terminal::search::{LinkRoots, LinkTarget, PathStyle, Probe, joined_file_link};
    use std::path::{Path, PathBuf};

    fn term(bytes: &[u8]) -> tty59_vt::VtTerm {
        let mut term = tty59_vt::VtTerm::new(80, 12, 1000);
        term.feed(bytes);
        term
    }

    fn resolve(term: &tty59_vt::VtTerm, row: i32, col: u16, files: &[&str]) -> Option<LinkTarget> {
        let windows = windows_at(&term.view(), GridPoint { line: row, col });
        let roots = LinkRoots {
            dirs: vec![PathBuf::from("/r")],
            local_home: false,
            style: PathStyle::Posix,
        };
        joined_file_link(&windows, &roots, &mut |path, _| {
            if files.iter().any(|file| path == Path::new("/r").join(file)) {
                Probe::Hit { is_dir: false }
            } else {
                Probe::Miss
            }
        })
        .map(|(_, link)| link.target)
    }

    fn expected(path: &str) -> Option<LinkTarget> {
        Some(LinkTarget::File {
            path: PathBuf::from("/r").join(path),
            line: None,
            column: None,
            is_dir: false,
        })
    }

    #[test]
    fn lead_long_filename_continuations_keep_the_path_anchor() {
        let term = term(b"src/a-very-long-\r\nfilename-middle-\r\ncomponent.rs\r\n");
        let target = "src/a-very-long-filename-middle-component.rs";
        for (row, col) in [(0, 4), (1, 4), (2, 4)] {
            assert_eq!(
                resolve(&term, row, col, &[target]),
                expected(target),
                "every fragment of a three-row basename belongs to its anchored path, including ({row},{col})"
            );
        }
    }

    #[test]
    fn lead_neighboring_files_do_not_hide_a_valid_two_row_path() {
        let term = term(b"notes/one.rs\r\nnotes/two.rs\r\nnotes/three.rs\r\nsrc/a-very-long-\r\nfilename.rs\r\n");
        let target = "src/a-very-long-filename.rs";
        let files = ["notes/one.rs", "notes/two.rs", "notes/three.rs", target];
        for (row, col) in [(3, 4), (4, 4)] {
            assert_eq!(
                resolve(&term, row, col, &files),
                expected(target),
                "independent preceding file rows must not consume every candidate window for ({row},{col})"
            );
        }
    }

    #[test]
    fn lead_an_existing_file_prefix_without_a_suffix_stays_independent() {
        let term = term(b"notes/tool\r\nunrelated\r\n");
        for (row, col) in [(0, 3), (1, 3)] {
            assert_eq!(
                resolve(&term, row, col, &["notes/tool", "notes/toolunrelated"]),
                None,
                "the FILE-prefix veto applies to files without extensions too"
            );
        }
    }
}
