//! The editor's Problems list: every error, warning and note the language
//! servers have published for the open files, grouped by file.
//!
//! It lives at the foot of the code panel rather than as a right-panel tab:
//! problems belong to the files open in the editor, the status bar's error and
//! warning counts are where people look for them, and a click lands in the
//! editor right above it. The status bar counts toggle it, as does the
//! `ToggleEditorProblems` command.
//!
//! Hints are left out, as VS Code leaves them out: servers use them for
//! inlay-ish suggestions that would bury the real problems.
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, ScrollHandle, SharedString, Window, div, px};
use gpui_component::button::Button;
use gpui_component::kbd::Kbd;
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use lsp_types::DiagnosticSeverity;

#[cfg(test)]
use super::status::observe;
use super::status::probe;
use crate::ui::app::Tty59App;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::lsp::LspStore;

/// The pane's height: a handful of rows, leaving the editor the rest.
const PANE_HEIGHT: f32 = 180.;

/// The header's height and a row's, as the least they take: the text sets the
/// rest, so at a large interface size a row grows to hold it rather than cut it.
const HEADER_H: f32 = 28.;
const ROW_H: f32 = 24.;

/// The 11.5px token for counts, folders, sources and positions, relative to the
/// root, as the status bar sets its text. Titles, file names and messages are
/// the right panel's 12px `META`.
const SECONDARY_REM: f32 = 11.5 / 16.;

/// The command the pane's empty state names the chord of.
const TOGGLE_ACTION: &str = "ToggleEditorProblems";

/// Past this many rows the list stops drawing and says how many are left.
const MAX_ROWS: usize = 1000;

#[derive(Default)]
pub(crate) struct ProblemsPane {
    pub(crate) open: bool,
    scroll: ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    /// Zero-based, in editor columns.
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) severity: Severity,
    pub(crate) message: String,
    pub(crate) source: Option<String>,
}

/// One file's problems, in the order they appear in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileProblems {
    pub(crate) path: PathBuf,
    pub(crate) problems: Vec<Problem>,
}

/// Sort the store's snapshot into what the list draws: files by path, each
/// file's problems by position, hints dropped, files left with nothing gone.
pub(crate) fn collect(snapshot: Vec<(PathBuf, Vec<lsp_types::Diagnostic>)>) -> Vec<FileProblems> {
    let mut files: Vec<FileProblems> = snapshot
        .into_iter()
        .map(|(path, diagnostics)| {
            let mut problems: Vec<Problem> = diagnostics
                .into_iter()
                .filter_map(|d| {
                    let severity = match d.severity {
                        // A server that says nothing means an error (LSP leaves
                        // it to the client; every editor reads it that way).
                        None | Some(DiagnosticSeverity::ERROR) => Severity::Error,
                        Some(DiagnosticSeverity::WARNING) => Severity::Warning,
                        Some(DiagnosticSeverity::INFORMATION) => Severity::Info,
                        _ => return None,
                    };
                    Some(Problem {
                        line: d.range.start.line,
                        column: d.range.start.character,
                        severity,
                        message: d.message,
                        source: d.source,
                    })
                })
                .collect();
            problems.sort_by(|a, b| {
                (a.line, a.column, a.severity).cmp(&(b.line, b.column, b.severity))
            });
            FileProblems { path, problems }
        })
        .filter(|f| !f.problems.is_empty())
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// The first line of a message: the list is one row per problem, and the
/// rest is a hover away in the editor.
fn headline(message: &str) -> &str {
    message.lines().next().unwrap_or("").trim_end()
}

impl Tty59App {
    pub(crate) fn toggle_editor_problems(&mut self, cx: &mut Context<Self>) {
        self.editor.problems.open = !self.editor.problems.open;
        cx.notify();
    }

    /// Where a file's name sits in the list: relative to the project the
    /// tab shows, when it is inside it.
    fn problems_label(&self, path: &Path, cx: &App) -> (String, String) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let spelled = self.project_spelling(path, cx);
        let dir = spelled.parent().map(|parent| {
            self.tab_code()
                .and_then(|c| {
                    c.roots
                        .iter()
                        .find_map(|r| parent.strip_prefix(r).ok().map(|p| (r, p)))
                })
                .map(|(root, rel)| {
                    let repo = root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    match rel.as_os_str().is_empty() {
                        true => repo,
                        false => format!("{repo}/{}", rel.display()),
                    }
                })
                .unwrap_or_else(|| parent.display().to_string())
        });
        (name, dir.unwrap_or_default())
    }

    pub(crate) fn render_editor_problems(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.editor.problems.open {
            return None;
        }
        let files = collect(LspStore::diagnostics_snapshot(cx));
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (errors, warnings) =
            files
                .iter()
                .flat_map(|f| &f.problems)
                .fold((0, 0), |(e, w), p| match p.severity {
                    Severity::Error => (e + 1, w),
                    Severity::Warning => (e, w + 1),
                    Severity::Info => (e, w),
                });
        let header = probe(
            h_flex()
                .flex_none()
                .min_h(px(HEADER_H))
                .px(px(crate::ui::app::CONTENT_INSET))
                .gap_2()
                .items_center()
                .border_t(crate::ui::theme::hairline(window))
                .border_color(theme.sidebar_border),
            "editor-problems-header",
        )
        .child(probe(
            div()
                .text_size(gpui::rems(crate::ui::right_panel::META))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(t(L10nKey::EditorProblemsTitle)),
            "editor-problems-title",
        ))
        .child(probe(
            // Shrinks, with an ellipsis, before it can push the close tile out
            // of a narrow header at a large interface size.
            div()
                .min_w_0()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(gpui::rems(SECONDARY_REM))
                .text_color(muted)
                .child(crate::ui::lsp::editor::problems_summary(errors, warnings)),
            "editor-problems-summary",
        ))
        .child(div().flex_1())
        .child(probe(
            div().flex_none().child(
                crate::ui::tab_strip::chrome_tile_sized(
                    Button::new("editor-problems-close").icon(IconName::Close),
                    crate::ui::tab_strip::RAIL_TILE,
                    crate::ui::tab_strip::RAIL_TILE_GLYPH,
                    false,
                    cx,
                )
                .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_editor_problems(cx))),
            ),
            "editor-problems-close",
        ));

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut left = 0usize;
        for (file_ix, file) in files.iter().enumerate() {
            if rows.len() >= MAX_ROWS {
                left += file.problems.len();
                continue;
            }
            let (name, dir) = self.problems_label(&file.path, cx);
            rows.push(
                probe(
                    h_flex()
                        .id(("editor-problems-file", file_ix))
                        .min_h(px(ROW_H))
                        .px(px(crate::ui::app::CONTENT_INSET))
                        .gap_2()
                        .items_center()
                        .text_size(gpui::rems(crate::ui::right_panel::META)),
                    "editor-problems-file",
                )
                .child(probe(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(name),
                    "editor-problems-file-name",
                ))
                .child(probe(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .text_size(gpui::rems(SECONDARY_REM))
                        .text_color(muted)
                        .child(dir),
                    "editor-problems-file-dir",
                ))
                .child(
                    div()
                        .flex_none()
                        .text_size(gpui::rems(SECONDARY_REM))
                        .text_color(muted)
                        .child(file.problems.len().to_string()),
                )
                .into_any_element(),
            );
            for (ix, problem) in file.problems.iter().enumerate() {
                if rows.len() >= MAX_ROWS {
                    left += file.problems.len() - ix;
                    break;
                }
                let (icon, color) = match problem.severity {
                    Severity::Error => (IconName::CircleX, theme.danger),
                    Severity::Warning => (IconName::TriangleAlert, theme.warning),
                    Severity::Info => (IconName::Info, theme.info),
                };
                let at = t_fmt(
                    L10nKey::EditorLnCol,
                    &[
                        ("line", &(problem.line + 1).to_string()),
                        ("column", &(problem.column + 1).to_string()),
                    ],
                );
                let path = file.path.clone();
                let (line, column) = (problem.line + 1, problem.column + 1);
                let row_name = match problem.severity {
                    Severity::Error => "editor-problem-error",
                    Severity::Warning => "editor-problem-warning",
                    Severity::Info => "editor-problem-info",
                };
                rows.push(
                    probe(
                        h_flex()
                            .id(("editor-problem", rows.len()))
                            .min_h(px(ROW_H))
                            .pl(px(crate::ui::app::CONTENT_INSET + 12.))
                            .pr(px(crate::ui::app::CONTENT_INSET))
                            .gap_2()
                            .items_center()
                            .text_size(gpui::rems(crate::ui::right_panel::META))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.list_hover)),
                        row_name,
                    )
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(probe(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(headline(&problem.message).to_string()),
                        "editor-problem-message",
                    ))
                    .children(problem.source.clone().map(|source| {
                        probe(
                            div()
                                .flex_none()
                                .text_size(gpui::rems(SECONDARY_REM))
                                .text_color(muted)
                                .child(source),
                            "editor-problem-source",
                        )
                    }))
                    .child(probe(
                        div()
                            .flex_none()
                            .text_size(gpui::rems(SECONDARY_REM))
                            .text_color(muted)
                            .child(at),
                        "editor-problem-at",
                    ))
                    // The whole message: every line of it, wrapped inside the
                    // window, where the row shows only the first line cut to
                    // its width.
                    .tooltip({
                        let message = SharedString::from(problem.message.clone());
                        move |window, cx| {
                            crate::ui::dialog::bounded_text_tooltip(
                                message.clone(),
                                "editor-problem-tooltip-text",
                                window,
                                cx,
                            )
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file_in_editor_at(&path, Some(line), Some(column), window, cx);
                    }))
                    .into_any_element(),
                );
            }
        }
        if left > 0 {
            rows.push(
                div()
                    .px(px(crate::ui::app::CONTENT_INSET))
                    .py_1()
                    .text_size(gpui::rems(SECONDARY_REM))
                    .text_color(muted)
                    .child(t_fmt(
                        L10nKey::EditorProblemsMore,
                        &[("n", &left.to_string())],
                    ))
                    .into_any_element(),
            );
        }
        let body: AnyElement = if files.is_empty() {
            #[cfg(test)]
            observe::say("editor-problems-empty", t(L10nKey::EditorProblemsNone));
            // The sentence, and beside it the chord the keymap has for the
            // pane: the effective one, so a rebinding moves it and an unbound
            // command has none to show.
            let hint = crate::ui::home::key_stroke(TOGGLE_ACTION, cx);
            #[cfg(test)]
            if let Some(stroke) = &hint {
                observe::say("editor-problems-empty-hint", &Kbd::format(stroke));
            }
            probe(
                h_flex()
                    .w_full()
                    .px(px(crate::ui::app::CONTENT_INSET))
                    .py_2()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .text_size(gpui::rems(crate::ui::right_panel::META))
                    .text_color(muted),
                "editor-problems-empty",
            )
            // The whole sentence, wrapped in the pane however long its
            // translation is; when the chord has no room beside it, the chord
            // drops under it whole.
            .child(probe(
                div()
                    .min_w_0()
                    .max_w_full()
                    .whitespace_normal()
                    .child(t(L10nKey::EditorProblemsNone)),
                "editor-problems-empty-text",
            ))
            .children(hint.map(|stroke| {
                probe(
                    div().flex_none().child(Kbd::new(stroke)),
                    "editor-problems-empty-hint",
                )
            }))
            .into_any_element()
        } else {
            let scroll = self.editor.problems.scroll.clone();
            crate::ui::scrollbar::with_vertical_scrollbar(
                "editor-problems-scrollbar",
                v_flex()
                    .id("editor-problems-list")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .children(rows),
                &scroll,
            )
        };
        Some(
            probe(
                v_flex()
                    .id("editor-problems")
                    .flex_none()
                    .h(px(PANE_HEIGHT))
                    .w_full(),
                "editor-problems",
            )
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Diagnostic, Position, Range};

    fn d(line: u32, character: u32, severity: Option<DiagnosticSeverity>, msg: &str) -> Diagnostic {
        Diagnostic {
            range: Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            ),
            severity,
            message: msg.into(),
            ..Default::default()
        }
    }

    #[test]
    fn problems_are_sorted_by_file_then_position_without_hints() {
        let files = collect(vec![
            (
                PathBuf::from("/p/src/b.rs"),
                vec![
                    d(9, 0, Some(DiagnosticSeverity::WARNING), "w"),
                    d(2, 4, Some(DiagnosticSeverity::ERROR), "e"),
                    d(2, 1, None, "unspecified is an error"),
                    d(5, 0, Some(DiagnosticSeverity::HINT), "hint"),
                ],
            ),
            (
                PathBuf::from("/p/src/a.rs"),
                vec![d(0, 0, Some(DiagnosticSeverity::INFORMATION), "i")],
            ),
            (
                PathBuf::from("/p/src/c.rs"),
                vec![d(0, 0, Some(DiagnosticSeverity::HINT), "only a hint")],
            ),
        ]);
        assert_eq!(files.len(), 2, "a file with only hints has no problems");
        assert_eq!(files[0].path, PathBuf::from("/p/src/a.rs"));
        assert_eq!(files[0].problems[0].severity, Severity::Info);
        let b: Vec<_> = files[1]
            .problems
            .iter()
            .map(|p| (p.line, p.column, p.severity))
            .collect();
        assert_eq!(
            b,
            vec![
                (2, 1, Severity::Error),
                (2, 4, Severity::Error),
                (9, 0, Severity::Warning)
            ]
        );
    }

    #[test]
    fn a_row_shows_the_first_line_of_the_message() {
        assert_eq!(
            headline("mismatched types\nexpected `u8`"),
            "mismatched types"
        );
        assert_eq!(headline(""), "");
    }

    /// The pane opens from its command and draws with nothing to show.
    #[gpui::test]
    fn the_pane_toggles_and_draws_empty(cx: &mut gpui::TestAppContext) {
        use crate::ui::app::test_window;

        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_new_file(window, cx);
            app.toggle_editor_problems(cx);
        });
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.editor.problems.open);
            assert!(app.render_editor_problems(window, cx).is_some());
            app.toggle_editor_problems(cx);
            assert!(app.render_editor_problems(window, cx).is_none());
        });
    }
}

/// The Problems pane in the real app, fed the way a language server feeds it:
/// the store's own `publish_diagnostics`, read back through the snapshot the
/// pane draws from.
#[cfg(test)]
mod gpui_tests {
    use gpui::{
        Bounds, Entity, Keystroke, Modifiers, Pixels, TestAppContext, VisualTestContext, size,
    };
    use lsp_types::{Diagnostic, Position, Range};

    use super::super::nav::gpui_tests::{caret, open_rs};
    use super::super::status::observe;
    use super::*;
    use crate::core::config::{Config, KeybindingOverride};
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::lsp::{LspStore, TestState};

    const PATH: &str = "/problems-test/a.rs";
    const MULTILINE: &str = "mismatched types\nexpected `u8`, found `u16`\nnote: a cast would do";
    const ACTION: &str = "ToggleEditorProblems";

    fn long_message() -> String {
        "this diagnostic keeps going well past any row ".repeat(7)
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    fn diagnostic(
        line: u32,
        severity: DiagnosticSeverity,
        message: &str,
        source: Option<&str>,
    ) -> Diagnostic {
        Diagnostic {
            range: Range::new(Position::new(line, 0), Position::new(line, 3)),
            severity: Some(severity),
            message: message.into(),
            source: source.map(str::to_string),
            ..Default::default()
        }
    }

    /// An open `a.rs` with a running server that published an error with a
    /// three-line message, a warning with a very long one and a note with a
    /// source, then the pane open and drawn.
    fn with_problems(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        diagnostics: Vec<Diagnostic>,
    ) {
        // Absolute on every platform, which the store's `file://` URI needs.
        let path = crate::ui::lsp::test_path(PATH);
        open_rs(app, vcx, path.to_str().expect("a UTF-8 path"));
        let input = app.read_with(vcx, |app, _| app.active_buffer().unwrap().input.clone());
        assert_eq!(
            app.read_with(vcx, |app, _| app.active_buffer().unwrap().path.clone()),
            path,
            "the buffer under test is the file the server is told about"
        );
        let expected = diagnostics.len();
        vcx.update(|_, cx| {
            LspStore::set_state_for_test(
                &path,
                "rust",
                "rust-analyzer",
                TestState::Running {
                    errors: 1,
                    warnings: 1,
                },
                &input,
                cx,
            );
            LspStore::publish_diagnostics_for_test(&path, diagnostics, cx);
        });
        let published: Vec<usize> = vcx.update(|_, cx| {
            LspStore::diagnostics_snapshot(cx)
                .into_iter()
                .map(|(_, d)| d.len())
                .collect()
        });
        // What the pane draws from: one file with exactly what was sent, or,
        // when nothing was sent, no file at all.
        let want: Vec<usize> = if expected > 0 { vec![expected] } else { vec![] };
        assert_eq!(published, want, "the store holds what was sent");
        app.update(vcx, |app, cx| app.toggle_editor_problems(cx));
        settle(vcx);
    }

    /// One note with a source: the pane's only message, source and position,
    /// so a selector for them names exactly one row.
    fn lone_note() -> Vec<Diagnostic> {
        vec![diagnostic(
            3,
            DiagnosticSeverity::INFORMATION,
            "unused",
            Some("rust-analyzer"),
        )]
    }

    /// The widths a pane is checked at: the real 720px window minimum and a
    /// wide one, and, labelled as such, a synthetic one below the minimum.
    const WIDTHS: [(f32, &str); 3] = [
        (1280., "wide"),
        (720., "the real 720px minimum"),
        (520., "synthetic, below the 720px minimum"),
    ];

    fn fixture() -> Vec<Diagnostic> {
        vec![
            diagnostic(1, DiagnosticSeverity::ERROR, MULTILINE, None),
            diagnostic(2, DiagnosticSeverity::WARNING, &long_message(), None),
            diagnostic(
                3,
                DiagnosticSeverity::INFORMATION,
                "unused",
                Some("rust-analyzer"),
            ),
        ]
    }

    fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"))
    }

    fn height(vcx: &mut VisualTestContext, selector: &'static str) -> f32 {
        bounds(vcx, selector).size.height.as_f32()
    }

    /// One line at `rems` of the interface size as the text system lays it out.
    fn line_h(vcx: &mut VisualTestContext, rems: f32) -> f32 {
        vcx.update(|window, _| {
            let mut style = window.text_style();
            style.font_size = px(rems * window.rem_size().as_f32()).into();
            style.line_height_in_pixels(window.rem_size()).as_f32()
        })
    }

    /// The real tooltip of the row `selector`: the mouse moved onto it, the
    /// clock past the show delay, a frame drawn. Where the tooltip put its
    /// text, or `None`.
    fn hover_tooltip(
        vcx: &mut VisualTestContext,
        selector: &'static str,
    ) -> Option<Bounds<Pixels>> {
        vcx.simulate_mouse_move(gpui::point(px(2.), px(2.)), None, Modifiers::default());
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        let target = bounds(vcx, selector);
        vcx.simulate_mouse_move(target.center(), None, Modifiers::default());
        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.debug_bounds("editor-problem-tooltip-text")
    }

    /// `inner` inside `row` on all four sides.
    fn contained(inner: Bounds<Pixels>, row: Bounds<Pixels>) -> bool {
        inner.top() >= row.top()
            && inner.bottom() <= row.bottom()
            && inner.left() >= row.left()
            && inner.right() <= row.right()
    }

    /// F10: both row levels are 24px, at a wide window, at the real 720px
    /// minimum and at a synthetic one below it.
    #[gpui::test]
    fn file_and_problem_rows_are_24px(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, fixture());
        for (width, what) in WIDTHS {
            vcx.simulate_resize(size(px(width), px(700.)));
            settle(&mut vcx);
            for row in [
                "editor-problems-file",
                "editor-problem-error",
                "editor-problem-warning",
                "editor-problem-info",
            ] {
                assert_eq!(height(&mut vcx, row), 24., "{width} ({what}): {row}");
            }
        }
    }

    /// F10: the text of a row is inside it at the default size and at UI 24,
    /// where the 12px token is 18px and a fixed 24px row would cut it. A lone
    /// note, so each selector names one element.
    #[gpui::test]
    fn row_content_fits_its_row_at_the_default_size_and_ui24(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, lone_note());
        for ui in [16., 24.] {
            vcx.update(|_, cx| {
                let mut cfg = cx.global::<Config>().clone();
                cfg.ui_font_size = ui;
                cx.set_global(cfg);
            });
            for (width, what) in WIDTHS {
                vcx.simulate_resize(size(px(width), px(700.)));
                settle(&mut vcx);
                // The header: at least 28px, and its title, count and close
                // tile inside it, never cut by it.
                let header = bounds(&mut vcx, "editor-problems-header");
                assert!(
                    header.size.height.as_f32() >= 28.,
                    "UI{ui} {width} ({what}): the header is {:?}, under 28px",
                    header.size.height
                );
                for inner in [
                    "editor-problems-title",
                    "editor-problems-summary",
                    "editor-problems-close",
                ] {
                    let inner_b = bounds(&mut vcx, inner);
                    assert!(
                        contained(inner_b, header),
                        "UI{ui} {width} ({what}): {inner} {inner_b:?} is cut by the header {header:?}"
                    );
                }
                // The numbers, for the report (run with --nocapture).
                eprintln!(
                    "A3.2 UI{ui} {width} ({what}): header {}px, file row {}px, note row {}px, \
                     message {}px, close {}px",
                    header.size.height,
                    height(&mut vcx, "editor-problems-file"),
                    height(&mut vcx, "editor-problem-info"),
                    height(&mut vcx, "editor-problem-message"),
                    height(&mut vcx, "editor-problems-close"),
                );
                for (row, inner) in [
                    ("editor-problems-file", "editor-problems-file-name"),
                    ("editor-problem-info", "editor-problem-message"),
                    ("editor-problem-info", "editor-problem-source"),
                    ("editor-problem-info", "editor-problem-at"),
                ] {
                    let (row_b, inner_b) = (bounds(&mut vcx, row), bounds(&mut vcx, inner));
                    assert!(
                        row_b.size.height.as_f32() >= 24.,
                        "UI{ui} {width} ({what}): {row} is {:?} tall, under 24px",
                        row_b.size.height
                    );
                    assert!(
                        contained(inner_b, row_b),
                        "UI{ui} {width} ({what}): {inner} {inner_b:?} is cut by {row} {row_b:?}"
                    );
                }
            }
        }
    }

    /// F10: a 28px header whose close is the 26px chrome tile, the same
    /// button (id, click) as before.
    #[gpui::test]
    fn the_header_is_28px_and_closes_with_the_26px_tile(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, fixture());
        let header = bounds(&mut vcx, "editor-problems-header");
        assert_eq!(header.size.height.as_f32(), 28., "the header is 28px");
        let close = bounds(&mut vcx, "editor-problems-close");
        let tile = crate::ui::tab_strip::RAIL_TILE;
        assert_eq!(
            (close.size.width.as_f32(), close.size.height.as_f32()),
            (tile, tile),
            "the close is the chrome tile"
        );
        assert!(
            close.top() >= header.top() && close.bottom() <= header.bottom(),
            "the 26px tile fits the 28px header: {close:?} in {header:?}"
        );
        assert!(
            close.right() <= header.right(),
            "and sits inside its right edge"
        );

        vcx.simulate_click(close.center(), Modifiers::default());
        settle(&mut vcx);
        assert!(
            !app.read_with(&vcx, |app, _| app.editor.problems.open),
            "clicking the close closes the pane"
        );
    }

    /// F10: titles, file names and messages are the 12px token; counts,
    /// directories, sources and positions the 11.5px one.
    #[gpui::test]
    fn text_is_set_in_the_12_and_11_5_tokens(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        // A lone note: its message, source and position are the only ones.
        with_problems(&app, &mut vcx, lone_note());
        let primary = line_h(&mut vcx, crate::ui::right_panel::META);
        let secondary = line_h(&mut vcx, 11.5 / 16.);
        for (selector, want) in [
            ("editor-problems-title", primary),
            ("editor-problems-summary", secondary),
            ("editor-problems-file-name", primary),
            ("editor-problems-file-dir", secondary),
            ("editor-problem-message", primary),
            ("editor-problem-source", secondary),
            ("editor-problem-at", secondary),
        ] {
            let got = height(&mut vcx, selector);
            assert!(
                (got - want).abs() < 0.6,
                "{selector} is {got}px tall, its token's line is {want}px"
            );
        }
    }

    /// F10: a click on a problem lands the caret on it, as before.
    #[gpui::test]
    fn a_problem_row_still_jumps_to_its_line(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, fixture());
        let row = bounds(&mut vcx, "editor-problem-warning");
        vcx.simulate_click(row.center(), Modifiers::default());
        for _ in 0..3 {
            settle(&mut vcx);
        }
        assert_eq!(caret(&app, &mut vcx).line, 2, "the warning is on row 2");
    }

    /// How many lines `text` takes in `width` pixels as the real text system
    /// lays it out: the whole text shaped in the tooltip card's font (the
    /// interface family, normal weight, 0.875 of the interface size) in one
    /// run, with `width` as the wrap width. Every `\n` starts a line, and a
    /// word wider than the width is broken, as the drawn text does.
    fn tooltip_lines(vcx: &mut VisualTestContext, text: &str, width: f32) -> usize {
        use gpui_component::ActiveTheme as _;
        vcx.update(|window, cx| {
            let font = gpui::Font {
                family: cx.theme().font_family.clone(),
                features: Default::default(),
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            };
            let run = gpui::TextRun {
                len: text.len(),
                font,
                color: gpui::Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let size = px(0.875 * window.rem_size().as_f32());
            window
                .text_system()
                .shape_text(
                    gpui::SharedString::from(text.to_string()),
                    size,
                    &[run],
                    Some(px(width)),
                    None,
                )
                .expect("the text shapes")
                .iter()
                .map(|line| line.wrap_boundaries.len() + 1)
                .sum()
        })
    }

    /// The tooltip of the row `selector`, hovered in the real window of
    /// `window_w` by `window_h`: its content is the whole `text` (as tall as
    /// every line of it takes at the painted width, so no tail can be missing),
    /// and the whole card, content with its padding and border and the outer
    /// margin, is inside the window on all four sides and at most 420px wide.
    fn assert_whole_message_tooltip(
        vcx: &mut VisualTestContext,
        selector: &'static str,
        text: &str,
        (window_w, window_h): (f32, f32),
        what: &str,
    ) {
        let rem = vcx.update(|window, _| window.rem_size().as_f32());
        // Per side, from the component's card (`tooltip.rs`): the outer margin
        // `m_3` (0.75 rem), the padding `px_2p5` (0.625 rem) / `py_1`
        // (0.25 rem) and the 1px border.
        let chrome_h = 0.75 * rem + 0.625 * rem + 1.;
        let chrome_v = 0.75 * rem + 0.25 * rem + 1.;
        let line_h = line_h(vcx, 0.875);
        let content = hover_tooltip(vcx, selector)
            .unwrap_or_else(|| panic!("{what}: hovering {selector} drew no tooltip text"));
        assert!(
            content.left().as_f32() - chrome_h >= -0.5
                && content.right().as_f32() + chrome_h <= window_w + 0.5,
            "{what}: the card, {content:?} plus {chrome_h}px a side, is not inside the \
             {window_w}px window"
        );
        assert!(
            content.top().as_f32() - chrome_v >= -0.5
                && content.bottom().as_f32() + chrome_v <= window_h + 0.5,
            "{what}: the card, {content:?} plus {chrome_v}px above and below, is not inside \
             the {window_h}px window"
        );
        let width = content.size.width.as_f32();
        let card = width + 2. * (0.625 * rem + 1.);
        assert!(
            card <= 420.5,
            "{what}: the card is {card}px wide, over the 420px hover maximum"
        );
        let lines = tooltip_lines(vcx, text, width);
        eprintln!("A3.2 tooltip {what}: content {content:?}, card {card}px wide, {lines} lines");
        // The allowance below stays a claim about rounding, not a loophole: the
        // most it ever forgives is under half a line, whatever the fixture.
        assert!(
            0.6 * (lines as f32) < 0.5 * line_h,
            "{what}: {lines} lines make the rounding allowance {}px, half a line or more",
            0.6 * lines as f32
        );
        // The computed line is a whole 23px where the card draws 22.5px (the
        // Boat run measured 3 lines as 67.5px), so the allowance is per line:
        // a line missing or added is a whole line off, far outside it.
        assert!(
            (content.size.height.as_f32() - lines as f32 * line_h).abs() <= 0.6 * lines as f32,
            "{what}: the content {content:?} is not the {lines} lines of {line_h}px the whole \
             message takes in {width}px"
        );
    }

    /// F10: hovering a row shows the whole message inside the window: the
    /// three-line one and the one too long for a line, at a wide window, at the
    /// real 720px minimum and at a synthetic one below it.
    #[gpui::test]
    fn a_row_tooltip_shows_the_whole_message_inside_the_window(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, fixture());
        let long = long_message();
        for (width, what) in WIDTHS {
            vcx.simulate_resize(size(px(width), px(700.)));
            settle(&mut vcx);
            for (row, text) in [
                ("editor-problem-error", MULTILINE),
                ("editor-problem-warning", long.as_str()),
            ] {
                assert_whole_message_tooltip(
                    &mut vcx,
                    row,
                    text,
                    (width, 700.),
                    &format!("{width} ({what}), {row}"),
                );
            }
        }
    }

    /// Binds (or, with an empty list, unbinds) the Problems command in the live
    /// config; `None` is the shipped chord.
    fn bind_problems(vcx: &mut VisualTestContext, binding: Option<&[&str]>) {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            match binding {
                Some(keys) => cfg.keybindings.insert(
                    ACTION.to_string(),
                    KeybindingOverride::Exact(keys.iter().map(|k| k.to_string()).collect()),
                ),
                None => cfg.keybindings.remove(ACTION),
            };
            cx.set_global(cfg);
        });
    }

    /// F10: at UI 24, in English and in Russian, at the real 720px minimum and
    /// at 1280, the empty pane keeps its whole sentence and, when the command
    /// is bound, the whole chip inside the pane, and the two never overlap: the
    /// chip wraps below a sentence that has no room for it beside it.
    #[gpui::test]
    fn the_empty_state_sentence_and_whole_chip_stay_inside_the_pane_at_ui24(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        with_problems(&app, &mut vcx, Vec::new());
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.ui_font_size = 24.;
            cx.set_global(cfg);
        });
        let bindings: [(&str, Option<&[&str]>); 3] = [
            ("shipped", None),
            ("rebound", Some(&["alt-k", "alt-j"])),
            ("unbound", Some(&[])),
        ];
        for locale in ["en", "ru-RU"] {
            crate::ui::i18n::set_locale(locale);
            for (name, binding) in bindings {
                bind_problems(&mut vcx, binding);
                for (width, what) in [
                    (1280., "wide"),
                    (960., "the native capture's 960px"),
                    (720., "the real 720px minimum"),
                    (520., "synthetic, below the 720px minimum"),
                ] {
                    vcx.simulate_resize(size(px(width), px(700.)));
                    app.update(&mut vcx, |_, cx| cx.notify());
                    settle(&mut vcx);
                    let at = format!("{locale} UI24 {name} {width} ({what})");
                    let pane = bounds(&mut vcx, "editor-problems");
                    let row = bounds(&mut vcx, "editor-problems-empty");
                    let text = bounds(&mut vcx, "editor-problems-empty-text");
                    assert!(
                        contained(text, pane),
                        "{at}: the sentence {text:?} is cut by the pane {pane:?}"
                    );
                    assert!(
                        contained(text, row),
                        "{at}: the sentence {text:?} is outside its row {row:?}"
                    );
                    assert!(
                        contained(row, pane),
                        "{at}: the row {row:?} is cut by the pane {pane:?}"
                    );
                    match (binding, vcx.debug_bounds("editor-problems-empty-hint")) {
                        (Some([]), None) => {}
                        (Some([]), Some(chip)) => panic!("{at}: unbound, yet a chip {chip:?}"),
                        (_, None) => panic!("{at}: bound, yet no chip"),
                        (_, Some(chip)) => {
                            assert!(
                                contained(chip, pane),
                                "{at}: the chip {chip:?} is cut by the pane {pane:?}"
                            );
                            assert!(
                                contained(chip, row),
                                "{at}: the chip {chip:?} is outside its row {row:?}"
                            );
                            assert!(
                                !text.intersects(&chip),
                                "{at}: the chip {chip:?} overlaps the sentence {text:?}"
                            );
                        }
                    }
                }
            }
        }
        crate::ui::i18n::set_locale("en");
    }

    /// F10: with nothing to show the pane says the existing sentence, and
    /// beside it the chord the keymap has for the pane: the default, a
    /// rebinding, and nothing at all once it is unbound.
    #[gpui::test]
    fn the_empty_pane_says_its_chord_from_the_keymap(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        observe::clear();
        with_problems(&app, &mut vcx, Vec::new());
        assert_eq!(
            observe::last("editor-problems-empty").as_deref(),
            Some(t(L10nKey::EditorProblemsNone)),
            "the empty sentence is the localized one, not a new one"
        );
        let spelled =
            |keys: &str| gpui_component::kbd::Kbd::format(&Keystroke::parse(keys).unwrap());
        let redraw = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            observe::clear();
            app.update(vcx, |_, cx| cx.notify());
            settle(vcx);
        };
        let configure = |vcx: &mut VisualTestContext, binding: Option<Vec<&str>>| {
            vcx.update(|_, cx| {
                let mut cfg = cx.global::<Config>().clone();
                match binding {
                    Some(keys) => cfg.keybindings.insert(
                        ACTION.to_string(),
                        KeybindingOverride::Exact(keys.into_iter().map(String::from).collect()),
                    ),
                    None => cfg.keybindings.remove(ACTION),
                };
                cx.set_global(cfg);
            });
        };

        redraw(&app, &mut vcx);
        assert_eq!(
            observe::last("editor-problems-empty-hint"),
            Some(spelled("secondary-shift-m")),
            "the shipped chord"
        );
        assert!(vcx.debug_bounds("editor-problems-empty-hint").is_some());

        configure(&mut vcx, Some(vec!["alt-k", "alt-j"]));
        redraw(&app, &mut vcx);
        assert_eq!(
            observe::last("editor-problems-empty-hint"),
            Some(spelled("alt-k")),
            "a rebinding, its first chord"
        );

        configure(&mut vcx, Some(Vec::new()));
        redraw(&app, &mut vcx);
        assert_eq!(
            observe::last("editor-problems-empty-hint"),
            None,
            "unbound: no hint is invented"
        );
        assert!(vcx.debug_bounds("editor-problems-empty-hint").is_none());
        assert!(
            vcx.debug_bounds("editor-problems-empty").is_some(),
            "the sentence stays"
        );
    }
}
