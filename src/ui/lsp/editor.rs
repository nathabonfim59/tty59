//! Where the code editor meets the language servers: the few calls
//! `code_editor.rs` makes when its buffers change, the commands (go to
//! definition, format, rename), and the status bar's word on the server.

use std::path::PathBuf;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Entity, IntoElement, SharedString, WeakEntity, Window, div, px, rems,
};
use gpui_component::input::InputState;
use gpui_component::menu::PopupMenu;
use gpui_component::popover::Popover;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, RopeExt as _, Selectable, Sizable as _, Size, WindowExt as _,
    h_flex, v_flex,
};
use lsp_types::Position;

use super::convert::{self, Encoding};
use super::{Freshen, LspStatus, LspStore, servers};
use crate::ui::app::Tty59App;
use crate::ui::code_editor::status::{self, Face};
use crate::ui::code_editor::{BufferId, LocalFile};
use crate::ui::i18n::{L10nKey, t, t_fmt, t_plural};
use crate::ui::right_panel::TEXT;
use crate::ui::text_elision::{ElidedLabel, Mode};

/// One buffer offered to the language servers.
pub(crate) struct LspBuffer {
    pub(crate) input: Entity<InputState>,
    pub(crate) path: PathBuf,
    pub(crate) language: &'static str,
    pub(crate) app: WeakEntity<Tty59App>,
}

impl Tty59App {
    fn lsp_enabled(cx: &gpui::App) -> bool {
        cx.global::<crate::core::config::Config>().editor_lsp
    }

    /// The set of open buffers changed — one opened, closed, or was saved
    /// under a new name. Servers start and documents open or close to match.
    pub(crate) fn lsp_sync_buffers(&self, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        let buffers = if Self::lsp_enabled(cx) {
            self.editor_local_files()
                .into_iter()
                .map(|f| LspBuffer {
                    input: f.input,
                    path: f.path,
                    language: f.language,
                    app: app.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let app = cx.entity().downgrade();
        LspStore::register_window(
            cx.entity_id(),
            Rc::new(move |cx: &mut gpui::App| {
                app.update(cx, |app, cx| app.lsp_sync_buffers(cx)).is_ok()
            }),
            cx,
        );
        LspStore::sync_window(cx.entity_id(), buffers, cx);
    }

    /// The window is closing: every document it owns is closed now, so an
    /// idle server can wind down instead of outliving the window (with a
    /// tray icon the app itself keeps running).
    pub(crate) fn lsp_window_closed(&self, cx: &mut Context<Self>) {
        LspStore::sync_window(cx.entity_id(), Vec::new(), cx);
    }

    /// A buffer's text changed.
    pub(crate) fn lsp_buffer_edited(&self, id: BufferId, cx: &mut Context<Self>) {
        if let Some(f) = self.editor_local_file(id) {
            LspStore::edited(f.input.entity_id(), &f.path, cx);
            self.lsp_signature_help_after_edit(&f, cx);
        }
    }

    /// A buffer was written to disk.
    pub(crate) fn lsp_buffer_saved(&self, id: BufferId, cx: &mut Context<Self>) {
        if let Some(f) = self.editor_local_file(id) {
            LspStore::saved(f.input.entity_id(), &f.path, cx);
        }
    }

    /// The front file, if a server has it and the editor has the focus.
    fn lsp_active(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(LocalFile, super::DocContext)> {
        let f = self.editor_active_local_file()?;
        let text = f.input.read(cx).text().clone();
        let doc = LspStore::context(
            &f.path,
            f.input.entity_id(),
            Freshen::Text(&text),
            Some(window.window_handle()),
            cx,
        )?;
        Some((f, doc))
    }

    /// Opens `path` with the cursor on a server's `position` — through the
    /// same open-at-line path a `file.rs:12:5` link takes, so navigation
    /// history sees a definition jump like any other.
    pub(crate) fn lsp_open_location(
        &mut self,
        path: PathBuf,
        position: Position,
        encoding: Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The open path counts columns in chars, from one.
        let target = path.clone();
        let open = move |app: &mut Tty59App,
                         character: u32,
                         window: &mut Window,
                         cx: &mut Context<Tty59App>| {
            app.open_file_in_editor_at(
                &target,
                Some(position.line + 1),
                Some(character + 1),
                window,
                cx,
            );
        };
        let open_buffer = self
            .editor_local_files()
            .into_iter()
            .find(|f| f.path == path.as_path());
        if let Some(f) = open_buffer {
            let text = f.input.read(cx).text().clone();
            let character = convert::lsp_to_editor(&text, position, encoding).character;
            open(self, character, window, cx);
            return;
        }
        // Not open: the column is converted against the file on disk.
        let read_from = path.clone();
        let line = position.line as usize;
        cx.spawn_in(window, async move |app, cx| {
            let character = cx
                .background_spawn(async move {
                    let text = std::fs::read_to_string(&read_from).ok()?;
                    let line = text.lines().nth(line)?;
                    let byte = convert::byte_of_column(line, position.character, encoding);
                    Some(line[..byte].chars().count() as u32)
                })
                .await
                .unwrap_or(position.character);
            let _ = app.update_in(cx, |app, window, cx| open(app, character, window, cx));
        })
        .detach();
    }

    /// F12: the definition of the symbol under the cursor.
    pub(crate) fn lsp_go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((f, doc)) = self.lsp_active(window, cx) else {
            return;
        };
        let state = f.input.read(cx);
        let request = super::providers::definition_request(&doc, state.text(), state.cursor());
        let (encoding, root) = (doc.encoding, doc.root.clone());
        cx.spawn_in(window, async move |app, cx| {
            let links = match request.await {
                Ok(links) => links,
                Err(e) => {
                    log::debug!("lsp: definition: {e:#}");
                    return;
                }
            };
            let places: Vec<_> = links
                .into_iter()
                .filter_map(|l| {
                    Some((
                        super::uri_to_path(&l.target_uri)?,
                        l.target_selection_range.start,
                    ))
                })
                .collect();
            let _ = app.update_in(cx, |app, window, cx| match places.as_slice() {
                [] => {}
                // One answer is followed straight away, through the same path
                // a secondary-click takes.
                [(path, position)] => {
                    app.lsp_open_location(path.clone(), *position, encoding, window, cx)
                }
                _ => app.lsp_show_places(
                    L10nKey::SearchHeadingDefinitions,
                    places,
                    encoding,
                    root,
                    window,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// ⇧⌥F: the server's formatting of the whole file, applied as edits so
    /// it undoes like one.
    pub(crate) fn lsp_format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((f, doc)) = self.lsp_active(window, cx) else {
            return;
        };
        if doc.caps.document_formatting_provider.is_none() {
            return;
        }
        let before = f.input.read(cx).text().clone();
        let request = doc.client.request::<lsp_types::request::Formatting>(
            lsp_types::DocumentFormattingParams {
                text_document: lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
                options: lsp_types::FormattingOptions {
                    tab_size: f.indent.size as u32,
                    insert_spaces: !f.indent.hard_tabs,
                    trim_trailing_whitespace: Some(true),
                    insert_final_newline: Some(true),
                    ..Default::default()
                },
                work_done_progress_params: Default::default(),
            },
        );
        let input = f.input.clone();
        let encoding = doc.encoding;
        cx.spawn_in(window, async move |_, cx| {
            let edits = match request.await {
                Ok(Some(edits)) if !edits.is_empty() => edits,
                Ok(_) => return,
                Err(e) => {
                    log::info!("lsp: formatting: {e:#}");
                    return;
                }
            };
            let _ = cx.update(|window, cx| {
                input.update(cx, |state, cx| {
                    // Typed over while the server worked: its edits describe
                    // a text that is gone.
                    if *state.text() != before {
                        return;
                    }
                    let edits = convert::edits_to_editor(&before, &edits, encoding);
                    state.apply_lsp_edits(&edits, window, cx);
                });
            });
        })
        .detach();
    }

    /// F2: asks for a new name for the symbol under the cursor.
    pub(crate) fn lsp_rename_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((f, doc)) = self.lsp_active(window, cx) else {
            return;
        };
        if doc.caps.rename_provider.is_none() {
            return;
        }
        let state = f.input.read(cx);
        let offset = state.cursor();
        let Some(word) = state
            .text()
            .word_range(offset)
            .map(|r| state.text().slice(r).to_string())
        else {
            return;
        };
        if word.trim().is_empty() {
            return;
        }
        self.editor_open_rename_bar(f.id, offset, word, window, cx);
    }

    /// The rename prompt was answered.
    pub(crate) fn lsp_rename(
        &mut self,
        id: BufferId,
        offset: usize,
        new_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.editor_local_file(id) else {
            return;
        };
        let text = f.input.read(cx).text().clone();
        let Some(doc) = LspStore::context(
            &f.path,
            f.input.entity_id(),
            Freshen::Text(&text),
            Some(window.window_handle()),
            cx,
        ) else {
            return;
        };
        // Every open buffer as the server sees it now; the answer is applied
        // only if none of those it touches has changed by then.
        let baseline = LspStore::snapshot(cx);
        let old_name = text
            .word_range(offset)
            .map(|r| text.slice(r).to_string())
            .unwrap_or_default();
        let request = doc
            .client
            .request::<lsp_types::request::Rename>(lsp_types::RenameParams {
                text_document_position: lsp_types::TextDocumentPositionParams::new(
                    lsp_types::TextDocumentIdentifier::new(doc.uri.clone()),
                    convert::offset_to_lsp(&text, offset, doc.encoding),
                ),
                new_name,
                work_done_progress_params: Default::default(),
            });
        let encoding = doc.encoding;
        cx.spawn_in(window, async move |_, cx| {
            let result = request.await;
            let _ = cx.update(|window, cx| match result {
                Ok(Some(edit)) => {
                    super::apply_workspace_edit(edit, encoding, Some(baseline), cx);
                }
                Ok(None) => {}
                Err(e) => {
                    let context = t_fmt(L10nKey::LspRenameFailed, &[("name", &old_name)]);
                    window.push_notification(
                        crate::ui::host_ops::failure(context, &format!("{e:#}")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// ⌘.: the server's fixes and refactors at the cursor, in the editor's
    /// own code-action menu.
    pub(crate) fn lsp_code_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_active_local_file().is_none() {
            return;
        }
        window.dispatch_action(Box::new(gpui_component::input::ToggleCodeActions), cx);
    }

    /// The language-server commands for the editor's right-click menu, once
    /// the buffer's server is up.
    pub(crate) fn lsp_menu_items(
        &self,
        menu: PopupMenu,
        id: BufferId,
        cx: &gpui::App,
    ) -> PopupMenu {
        use crate::core::actions::{
            EditorFindReferences, EditorFormatDocument, EditorGoToDefinition, EditorQuickFix,
            EditorRenameSymbol,
        };
        let Some(f) = self.editor_local_file(id) else {
            return menu;
        };
        if !matches!(
            LspStore::status(&f.path, f.language, cx),
            Some(LspStatus::Running { .. })
        ) {
            return menu;
        }
        menu.separator()
            .menu(
                t(L10nKey::LspGoToDefinition),
                Box::new(EditorGoToDefinition),
            )
            .menu(
                t(L10nKey::LspFindReferences),
                Box::new(EditorFindReferences),
            )
            .menu(t(L10nKey::LspQuickFix), Box::new(EditorQuickFix))
            .menu(
                t(L10nKey::LspRenameSymbolAction),
                Box::new(EditorRenameSymbol),
            )
            .menu(
                t(L10nKey::LspFormatDocument),
                Box::new(EditorFormatDocument),
            )
    }

    /// The status bar's word on the front file's server, when there is one to
    /// speak of: it runs, is starting, is down or is missing.
    pub(crate) fn lsp_status_now(&self, cx: &gpui::App) -> Option<LspStatus> {
        if !Self::lsp_enabled(cx) {
            return None;
        }
        let f = self.editor_active_local_file()?;
        LspStore::status(&f.path, f.language, cx)
    }
}

/// The missing-server note: a warning dot and "No language server", which
/// opens what tty59 tried. Passive: it runs nothing and scans nothing.
pub(crate) fn render_lsp_missing(
    name: &'static str,
    label: SharedString,
    cx: &gpui::App,
) -> AnyElement {
    let dot = cx.theme().warning;
    let title = label.clone();
    let popover = Popover::new("lsp-missing")
        .anchor(gpui::Anchor::BottomLeft)
        .trigger(MissingTrigger {
            label,
            dot,
            selected: false,
        })
        // Built only while it is open.
        .content(move |_, _, cx| {
            status::probe(
                v_flex()
                    .max_w(px(HELP_MAX_W))
                    // The row this hangs from keeps its words on one line, and
                    // text style is inherited: this is prose and wraps.
                    .whitespace_normal()
                    .gap(px(4.))
                    .text_size(rems(TEXT))
                    .child(status::probe(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(title.clone()),
                        "editor-status-lsp-help-title",
                    ))
                    .child(status::probe(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(missing_help(name)),
                        "editor-status-lsp-help-body",
                    )),
                "editor-status-lsp-help",
            )
        });
    // Shrinkable, unlike the other readouts: when even this note alone is
    // wider than the row, its label gives way (below) rather than the row
    // overflowing.
    div().min_w_0().child(popover).into_any_element()
}

/// The widest the missing-server help is allowed to be.
pub(crate) const HELP_MAX_W: f32 = 320.;

/// What the missing-server popover says: the executables tried on `PATH`, from
/// the server's own table. Nothing is run or searched to say it.
pub(crate) fn missing_help(name: &str) -> String {
    let programs = servers::SERVERS
        .iter()
        .find(|spec| spec.name == name)
        .map(|spec| {
            spec.commands
                .iter()
                .map(|(program, _)| *program)
                .collect::<Vec<_>>()
                .join(t(L10nKey::TrayAgentSep))
        })
        .unwrap_or_default();
    t_fmt(
        L10nKey::LspMissingBody,
        &[("name", name), ("programs", &programs)],
    )
}

/// The popover's trigger, the dot and the label, as one button-sized readout.
#[derive(IntoElement)]
struct MissingTrigger {
    label: SharedString,
    dot: gpui::Hsla,
    selected: bool,
}

impl Selectable for MissingTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for MissingTrigger {
    fn render(self, _: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let fg = cx.theme().foreground;
        let hover = cx.theme().muted;
        status::probe(
            h_flex().id("status-lsp-missing"),
            "editor-status-lsp-missing",
        )
        .min_w_0()
        .h(px(20.))
        .px(px(4.))
        .mx(px(-4.))
        .gap(px(status::LEAD_GAP))
        .items_center()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(fg))
        .child(
            div()
                .flex_none()
                .size(px(status::DOT))
                .rounded_full()
                .bg(self.dot),
        )
        .child(div().min_w_0().child(ElidedLabel::new(
            "status-lsp-missing-label",
            self.label,
            Mode::End,
        )))
    }
}

/// The server's state when it is not running: starting or stopped.
pub(crate) fn render_lsp_state(text: SharedString) -> AnyElement {
    div().flex_none().child(text).into_any_element()
}

/// The running server's error and warning counts, with the sentence they
/// make as the tooltip.
pub(crate) fn render_lsp_counts(
    name: &str,
    errors: usize,
    warnings: usize,
    face: &Face,
    cx: &gpui::App,
) -> AnyElement {
    let tip = format!("{name} — {}", problems_summary(errors, warnings));
    let muted = cx.theme().muted_foreground;
    let icon = face.icon;
    let count = |glyph: IconName, n: usize, color: gpui::Hsla| {
        h_flex()
            .gap(px(status::COUNT_GAP))
            .items_center()
            .child(
                Icon::new(glyph)
                    .with_size(Size::Size(icon))
                    .text_color(if n > 0 { color } else { muted }),
            )
            .child(n.to_string())
    };
    h_flex()
        .id("lsp-status")
        .flex_none()
        .gap(px(status::PAIR_GAP))
        .items_center()
        .font_features(face.features(true))
        .child(count(IconName::CircleX, errors, cx.theme().danger))
        .child(count(IconName::TriangleAlert, warnings, cx.theme().warning))
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .into_any_element()
}

/// "1 error, 2 warnings", each count in its own plural form and the pair in
/// the locale's own order.
pub(crate) fn problems_summary(errors: usize, warnings: usize) -> String {
    t_fmt(
        L10nKey::LspProblemsTooltip,
        &[
            ("errors", &t_plural(L10nKey::LspErrorCount, errors, &[])),
            (
                "warnings",
                &t_plural(L10nKey::LspWarningCount, warnings, &[]),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counts as a person reads them, each in its own locale's plural
    /// form, independent of how the key is assembled.
    #[test]
    fn the_problem_counts_read_in_each_locales_plural_forms() {
        crate::ui::i18n::set_locale("en");
        assert_eq!(problems_summary(1, 1), "1 error, 1 warning");
        assert_eq!(problems_summary(2, 3), "2 errors, 3 warnings");
        assert_eq!(problems_summary(0, 0), "No errors, No warnings");

        crate::ui::i18n::set_locale("ru-RU");
        assert_eq!(problems_summary(1, 1), "1 ошибка, 1 предупреждение");
        assert_eq!(problems_summary(2, 5), "2 ошибки, 5 предупреждений");
        assert_eq!(problems_summary(11, 12), "11 ошибок, 12 предупреждений");
        assert_eq!(problems_summary(21, 22), "21 ошибка, 22 предупреждения");
    }
}
