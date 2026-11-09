//! The Editor group on the General page: the code editor's switches that
//! used to live only in config.json or in a status-bar toggle.

use super::*;

impl Tty59App {
    pub(super) fn render_editor_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let cfg = cx.global::<Config>();
        let (git_gutter, lsp, soft_wrap, markdown, mermaid_colors) = (
            cfg.editor_git_gutter,
            cfg.editor_lsp,
            cfg.editor_soft_wrap,
            cfg.editor_markdown_preview,
            cfg.editor_mermaid_colors,
        );
        let git_gutter =
            self.settings_switch("wt-editor-git-gutter", git_gutter, cx, |this, on, _, cx| {
                this.set_editor_git_gutter(on, cx)
            });
        let lsp = self.settings_switch("wt-editor-lsp", lsp, cx, |this, on, _, cx| {
            this.set_editor_lsp(on, cx)
        });
        let soft_wrap =
            self.settings_switch("wt-editor-soft-wrap", soft_wrap, cx, |this, on, _, cx| {
                this.set_editor_soft_wrap(on, cx)
            });
        let markdown =
            self.settings_switch("wt-editor-markdown", markdown, cx, |this, on, _, cx| {
                this.set_editor_markdown_preview(on, cx)
            });
        let mermaid_colors = self.settings_switch(
            "wt-editor-mermaid-colors",
            mermaid_colors,
            cx,
            |this, on, _, cx| this.set_editor_mermaid_colors(on, cx),
        );
        self.settings_group(
            Some(t(L10nKey::SettingsEditor)),
            None,
            [
                self.settings_row(
                    t(L10nKey::SettingsEditorGitGutter),
                    t(L10nKey::SettingsEditorGitGutterDesc),
                    git_gutter,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorLsp),
                    t(L10nKey::SettingsEditorLspDesc),
                    lsp,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorSoftWrap),
                    t(L10nKey::SettingsEditorSoftWrapDesc),
                    soft_wrap,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorMarkdownPreview),
                    t(L10nKey::SettingsEditorMarkdownPreviewDesc),
                    markdown,
                    cx,
                ),
                self.settings_row(
                    t(L10nKey::SettingsEditorMermaidColors),
                    t(L10nKey::SettingsEditorMermaidColorsDesc),
                    mermaid_colors,
                    cx,
                ),
            ]
            .map(IntoElement::into_any_element),
            cx,
        )
    }

    /// `LspStore` watches the config: off closes every document and lets
    /// its server go; on asks every window for the files it has open, which
    /// starts their servers again.
    pub(crate) fn set_editor_lsp(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_lsp = on);
        cx.notify();
    }

    /// What new files open with; files already open keep their own Wrap.
    pub(crate) fn set_editor_soft_wrap(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_soft_wrap = on);
    }

    /// What Markdown files open as; files already open keep their own mode.
    pub(crate) fn set_editor_markdown_preview(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_markdown_preview = on);
    }

    /// Whether the Markdown preview colors Mermaid diagram shapes. Unlike the
    /// other editor switches, this one reaches previews already on screen in
    /// every window, so they redraw with the new palette at once.
    pub(crate) fn set_editor_mermaid_colors(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_mermaid_colors = on);
        cx.refresh_windows();
    }
}
