//! Saving a hex document, and keeping it honest about the file: a save that finds the file
//! changed, an outside change seen by the watcher, and the other tabs of the same file.
//!
//! It follows the text editor's save path (`editor_save_file`) one for one, so Ctrl+S, Save on
//! close, Save All and the quit guard all reach it through that function.

use tty59_core::host::Expect;

use super::data::HexContent;
use super::io::{HexFile, HexLoaded, HexSource, HexWrite};
use super::model::{SaveSnapshot, Source};
use super::*;

/// Whether a failed in-place write may have changed the file before it failed. A refusal
/// that comes before the first byte (the host cannot patch, the file is not the one the
/// patches were made against, a patch past its end, no such file, no permission) leaves it as
/// it was; anything else (a full disk, an I/O error) may have landed some runs and not others.
fn may_be_partly_written(e: &std::io::Error) -> bool {
    !matches!(
        e.kind(),
        std::io::ErrorKind::Unsupported
            | std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
    )
}

impl HexData {
    /// Freezes the edits for a save and marks it as running: what to write, and what the file
    /// must still look like for the write to be allowed. `None` when there is nothing to save.
    fn begin_save(&mut self, then_close: bool) -> Option<(HexWrite, SaveSnapshot, Expect)> {
        let HexContent::Ready(model) = &mut self.content else {
            return None;
        };
        if !model.store.dirty() {
            return None;
        }
        let expect = Expect {
            len: model.store.len(),
            mtime: self.mtime,
        };
        let snapshot = model.save_snapshot();
        let what = match &snapshot.whole {
            Some(bytes) => HexWrite::Whole(bytes.clone()),
            None => HexWrite::Patches(snapshot.patches()),
        };
        self.saving = true;
        self.save_then_close |= then_close;
        Some((what, snapshot, expect))
    }

    /// The save did not happen: the edits stay as they are, to be saved again.
    fn abort_save(&mut self) {
        if let HexContent::Ready(model) = &mut self.content {
            model.abort_save();
        }
        self.save_then_close = false;
    }

    /// The file on disk was read again: its bytes replace the document's, edits and history
    /// included. The caret stays where it was, in range, and reads still in flight for the
    /// old bytes are dropped by the generation.
    pub(super) fn apply_reload(&mut self, file: HexFile, cx: &mut Context<Self>) {
        let HexContent::Ready(model) = &mut self.content else {
            return;
        };
        match file.source {
            HexSource::Paged { len, first_page } => {
                model.reload(Source::Paged { len });
                let generation = model.store.generation();
                model.insert_page(generation, 0, first_page);
            }
            HexSource::Whole(bytes) => model.reload(Source::Whole(bytes)),
        }
        self.mtime = file.mtime;
        self.opens_as_text = file.opens_as_text;
        self.conflict = None;
        cx.notify();
    }
}

impl Tty59App {
    /// Whether `id` is a document with edits that are not on disk, of any kind.
    pub(in crate::ui::code_editor) fn doc_dirty(&self, id: BufferId, cx: &gpui::App) -> bool {
        match self.buffer(id) {
            Some(f) => f.dirty,
            None => self.hex_dirty(id, cx),
        }
    }

    /// Whether another document of the same file (a text buffer against a hex document, or
    /// the reverse) has edits that are not on disk. Saving over those would silently drop them
    /// from the file, so it is asked about first.
    pub(in crate::ui::code_editor) fn sibling_unsaved(&self, id: BufferId, cx: &gpui::App) -> bool {
        let Some((host, path)) = self.doc_location(id) else {
            return false;
        };
        self.editor
            .buffers
            .iter()
            .any(|b| b.dirty && b.id() != id && b.is_at(host, path))
            || self.editor.hex.docs.iter().any(|d| {
                d.id() != id && d.host.id() == host && d.path == path && d.data.read(cx).dirty()
            })
    }

    /// Asked when the file has another tab with unsaved changes. Overwrite saves anyway.
    pub(in crate::ui::code_editor) fn editor_ask_sibling_overwrite(
        &mut self,
        id: BufferId,
        then_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.doc_label(id) else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorSaveSiblingTitle, &[("name", &name)]),
            Some(t(L10nKey::EditorSaveSiblingBody)),
            &crate::ui::confirm_answers(t(L10nKey::EditorOverwrite), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = app.update_in(cx, |app, window, cx| {
                app.editor_save_file(id, then_close, true, window, cx);
            });
        })
        .detach();
    }

    /// `id` was just written to disk with `mtime`: the other tab of that file hears of it as
    /// it would from the watcher (a remote host has none): a clean one reloads, one with
    /// edits of its own gets the banner.
    pub(in crate::ui::code_editor) fn editor_notify_siblings(
        &mut self,
        id: BufferId,
        mtime: Option<MTime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((host, path)) = self.doc_location(id) else {
            return;
        };
        let path = path.to_path_buf();
        let observed = Observed::Present(mtime);
        let text = self.buffer_at(host, &path).filter(|b| *b != id);
        let hex = self.hex_at(host, &path).filter(|h| *h != id);
        if let Some(text) = text {
            self.editor_buffer_external_change(text, observed, window, cx);
        }
        if let Some(hex) = hex {
            self.hex_apply_external_change(hex, observed, window, cx);
        }
    }

    /// Saves the hex document `id`: Ctrl+S, Save on close, Save All and the quit guard come
    /// here through `editor_save_file`. A clean document has nothing to write.
    ///
    /// A paged document whose file was deleted is not recreated: it never held all the bytes,
    /// so the write fails with NotFound and the Deleted banner already says the file is gone.
    pub(in crate::ui::code_editor) fn hex_save(
        &mut self,
        id: BufferId,
        then_close: bool,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (data, host, path) = (doc.data.clone(), doc.host.clone(), doc.path.clone());
        if data.read(cx).saving {
            data.update(cx, |d, _| {
                d.save_pending = true;
                d.save_then_close |= then_close;
            });
            return;
        }
        if !data.read(cx).dirty() {
            self.editor_saves_landed(window, cx);
            return;
        }
        if !force && self.sibling_unsaved(id, cx) {
            self.editor_saves_failed(id);
            self.editor_ask_sibling_overwrite(id, then_close, window, cx);
            return;
        }
        let Some((what, snapshot, expect)) = data.update(cx, |d, _| d.begin_save(then_close))
        else {
            return;
        };
        let paged = matches!(what, HexWrite::Patches(_));
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| io::save_hex(h, &path, what, expect, force),
            move |app, outcome: SaveOutcome, window, cx| {
                app.hex_save_landed(id, snapshot, paged, outcome, window, cx)
            },
        );
        cx.notify();
    }

    /// The host answered a save.
    fn hex_save_landed(
        &mut self,
        id: BufferId,
        snapshot: SaveSnapshot,
        paged: bool,
        outcome: SaveOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Closed while it was being written: nothing is left to fold into.
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (data, host_id, name) = (doc.data.clone(), doc.host.id(), doc.name.clone());
        let dir = doc.path.parent().map(Path::to_path_buf);
        let pending = data.update(cx, |d, _| {
            d.saving = false;
            std::mem::take(&mut d.save_pending)
        });
        match outcome {
            SaveOutcome::Saved(mtime) => {
                data.update(cx, |d, cx| {
                    if let HexContent::Ready(model) = &mut d.content {
                        model.apply_saved(&snapshot);
                    }
                    d.mtime = mtime;
                    d.conflict = None;
                    cx.notify();
                });
                // A save is a working-tree edit the `.git` watch cannot see.
                if let Some(dir) = &dir {
                    self.scm_invalidate_cwd(host_id, dir, cx);
                }
                self.editor_notify_siblings(id, mtime, window, cx);
                if pending {
                    self.hex_save(id, false, false, window, cx);
                    return;
                }
                let close = data.update(cx, |d, _| {
                    std::mem::take(&mut d.save_then_close) && !d.dirty()
                });
                if close {
                    self.editor_drop_buffer(id, cx);
                }
                self.editor_saves_landed(window, cx);
            }
            SaveOutcome::Conflict(observed) => {
                data.update(cx, |d, _| {
                    d.abort_save();
                    d.conflict = Some(DiskConflict::Changed(observed));
                });
                self.editor_saves_failed(id);
                self.editor_ask_overwrite(id, window, cx);
            }
            SaveOutcome::Failed(e) => {
                data.update(cx, |d, _| d.abort_save());
                let mut context = t_fmt(L10nKey::EditorSaveFailed, &[("name", &name)]);
                // A write in place that stopped partway leaves the file as it got to, and
                // the edits stay unsaved so a retry writes all of them again.
                if paged && may_be_partly_written(&e) {
                    context.push(' ');
                    context.push_str(t(L10nKey::HexSavePartlyWritten));
                }
                #[cfg(test)]
                crate::ui::code_editor::status::observe::say("hex-save-failed", &context);
                HostOps::notify_err(window, cx, &context, &e);
                self.editor_saves_failed(id);
            }
        }
        cx.notify();
    }

    /// What a change seen on disk means for the hex document `id`.
    pub(in crate::ui::code_editor) fn hex_apply_external_change(
        &mut self,
        id: BufferId,
        observed: Observed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let data = doc.data.clone();
        let (saving, dirty, mtime, readable) = {
            let d = data.read(cx);
            (d.saving, d.dirty(), d.mtime, d.too_large().is_none())
        };
        // A file that was never read has nothing to go stale.
        if !readable {
            return;
        }
        match classify_external_change(saving, dirty, mtime, observed) {
            ExternalChange::Ignore => {}
            ExternalChange::Conflict => {
                let Observed::Present(mtime) = observed else {
                    return;
                };
                data.update(cx, |d, cx| {
                    d.conflict = Some(DiskConflict::Changed(mtime));
                    cx.notify();
                });
                cx.notify();
            }
            ExternalChange::Deleted => {
                data.update(cx, |d, cx| {
                    d.conflict = Some(DiskConflict::Deleted);
                    d.mtime = None;
                    cx.notify();
                });
                cx.notify();
            }
            ExternalChange::Reload => self.hex_reload_from_disk(id, window, cx),
        }
    }

    /// Reads the file again and replaces the document with it, edits and all: a clean
    /// document that changed on disk, or the banner's Reload.
    pub(in crate::ui::code_editor) fn hex_reload_from_disk(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (data, host, path) = (doc.data.clone(), doc.host.clone(), doc.path.clone());
        let seq = data.update(cx, |d, _| {
            d.reload_seq = d.reload_seq.wrapping_add(1);
            d.reload_seq
        });
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| io::load_hex(h, path),
            move |app, loaded, _window, cx| app.hex_reloaded(id, seq, loaded, cx),
        );
    }

    fn hex_reloaded(
        &mut self,
        id: BufferId,
        seq: u64,
        loaded: Result<HexLoaded, EditorOpenError>,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let data = doc.data.clone();
        // A newer reload is on its way; this answer is for bytes that are gone already.
        if data.read(cx).reload_seq != seq {
            return;
        }
        match loaded {
            Ok(HexLoaded::File(file)) => data.update(cx, |d, cx| d.apply_reload(file, cx)),
            // Unreadable for a moment, mid-write say, is not a reason to touch the
            // document; the next change event tries again.
            Ok(HexLoaded::TooLarge { len, limit, .. }) => {
                log::warn!(
                    "hex: reload of {} skipped, {len} bytes is over {limit}",
                    doc.path.display()
                );
            }
            Err(EditorOpenError::Message(e))
            | Err(EditorOpenError::TooLarge { message: e, .. }) => {
                log::warn!("hex: reload of {} failed: {e}", doc.path.display());
            }
            Err(EditorOpenError::NotText { .. }) => {}
        }
        cx.notify();
    }

    /// Keep mine: the file as it is on disk is the one a save may replace.
    pub(in crate::ui::code_editor) fn hex_keep_mine(
        &mut self,
        id: BufferId,
        observed: Option<MTime>,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        doc.data.update(cx, |d, cx| {
            d.mtime = observed;
            d.conflict = None;
            cx.notify();
        });
        cx.notify();
    }

    /// A file or folder was deleted through tty59: hex documents at or under it say so rather
    /// than waiting for the watcher.
    pub(in crate::ui::code_editor) fn hex_path_removed(&mut self, host: HostId, path: &Path, cx: &mut Context<Self>) {
        for doc in &self.editor.hex.docs {
            if doc.host.id() == host && doc.path.starts_with(path) {
                doc.data.update(cx, |d, cx| {
                    d.conflict = Some(DiskConflict::Deleted);
                    d.mtime = None;
                    cx.notify();
                });
            }
        }
    }

    /// The banner for the document in front of the active tab, text or hex.
    pub(in crate::ui::code_editor) fn active_conflict(
        &self,
        cx: &gpui::App,
    ) -> Option<(BufferId, DiskConflict)> {
        let id = self.tab_code()?.active_id()?;
        match self.buffer(id) {
            Some(f) => f.conflict.map(|c| (id, c)),
            None => self.hex(id)?.data.read(cx).conflict.map(|c| (id, c)),
        }
    }

    /// Hex documents with edits that no tab shows any more (a shell that exited took its tab)
    /// go to the tab in front, as a text buffer's do: a close that could not ask must not lose
    /// them.
    pub(in crate::ui::code_editor) fn hex_adopt_orphans(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        let shown: HashSet<BufferId> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        let orphans: Vec<(BufferId, SharedString)> = self
            .editor
            .hex
            .docs
            .iter()
            .filter(|d| !shown.contains(&d.id()) && !self.hex_clean(d.id(), cx))
            .map(|d| (d.id(), d.name.clone()))
            .collect();
        if orphans.is_empty() {
            return;
        }
        let Some(tab) = self.tabs.get(self.active).map(|t| t.tree_id.get()) else {
            return;
        };
        for (id, name) in orphans {
            self.editor_show_in_tab(tab, id, false, window, cx);
            window.push_notification(t_fmt(L10nKey::EditorOrphanAdopted, &[("name", &name)]), cx);
        }
        cx.notify();
    }
}
