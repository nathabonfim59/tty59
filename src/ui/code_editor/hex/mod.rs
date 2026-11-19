//! The hex editor: a byte-level view of any file, next to the text editor and the image
//! viewer. `model` is the pure part (pages, edits, undo, search, parsers); `io` is the host's
//! half (reading pages, saving); the per-tab state and the view build on them.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{App, FocusHandle};
use gpui_component::notification::Notification;
use tty59_core::host::{Host, Meta};

use self::data::HexData;
use self::view::HexView;
use super::images::ViewKey;
use super::*;
use crate::ui::editor_text;
use crate::ui::file_copy::REMOTE_FILE_MAX;
use crate::ui::i18n::{L10nKey, t_fmt};
use crate::ui::image_preview::decode as image_decode;
use crate::ui::image_preview::view::{ErrorCard, error_card};

pub(super) mod data;
pub(super) mod find;
pub(super) mod io;
pub(super) mod model;
mod save;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod test_support;
pub(super) mod view;

/// Bytes read from the start of a file too large for the text editor to decide what it is,
/// which is also the size of a hex page, so the same read serves as page 0.
pub(in crate::ui::code_editor) const SNIFF_PREFIX: usize = model::PAGE;

/// Whether the first bytes of a file say it is text.
///
/// The prefix is a cut of a bigger file, so it can end in the middle of a character: an
/// incomplete sequence at the very end is not evidence of anything. Anything else that is not
/// UTF-8, and any NUL byte, is: the whole-file rules of `editor_text::decode` call a NUL
/// binary, and a prefix that already breaks UTF-8 earlier would send the file down its
/// legacy-encoding guess, which for a file too large to open is not worth the guess. A file
/// that declares itself UTF-8 or UTF-16 with a byte-order mark is text, as it is there.
pub(in crate::ui::code_editor) fn prefix_is_text(prefix: &[u8]) -> bool {
    let Some(decoded) = editor_text::decode(prefix) else {
        return false;
    };
    if decoded.format.bom {
        return true;
    }
    match std::str::from_utf8(prefix) {
        Ok(_) => true,
        // `error_len` is `None` when the input only ran out inside a character.
        Err(e) => e.error_len().is_none(),
    }
}

/// What a file over the text editor's limit, not named like a picture, turns out to be.
pub(in crate::ui::code_editor) enum Large {
    /// Binary: a hex tab, from what was read.
    Hex(io::HexFile),
    /// A picture by its bytes: the picture rules take over. `bytes` is the whole file when the
    /// host had to read it whole to tell, so it is not read twice.
    Picture { bytes: Option<Vec<u8>> },
    /// Text, or a file nothing can be told about: "too large", with an offer of Open as Hex.
    TooLargeForText,
}

/// Tells a large file apart without holding more of it than it takes.
///
/// A host that reads in parts gives the first [`SNIFF_PREFIX`] bytes and nothing more, so a
/// multi-gigabyte binary costs one page; its hex tab is paged and reuses that read as page 0.
/// Any other host must send the file whole to be read at all, which it does once, up to what the
/// connection carries ([`REMOTE_FILE_MAX`]): past that the file cannot be told apart and gets
/// the "too large" words like text does.
pub(in crate::ui::code_editor) fn sniff_large(
    h: &dyn Host,
    path: &std::path::Path,
    meta: &Meta,
) -> Result<Large, EditorOpenError> {
    let read_failed = |e: &std::io::Error| {
        EditorOpenError::Message(t_fmt(
            L10nKey::EditorCantRead,
            &[("path", &path.display().to_string()), ("e", &e.to_string())],
        ))
    };
    if h.id().is_local() {
        match h.read_range(path, 0, SNIFF_PREFIX) {
            Ok(prefix) => {
                if !matches!(
                    image_decode::sniff(&prefix),
                    image_decode::Sniffed::NotImage
                ) {
                    return Ok(Large::Picture { bytes: None });
                }
                if prefix_is_text(&prefix) {
                    return Ok(Large::TooLargeForText);
                }
                return Ok(Large::Hex(io::HexFile {
                    path: path.to_path_buf(),
                    source: io::HexSource::Paged {
                        len: meta.len,
                        first_page: prefix,
                    },
                    mtime: meta.mtime,
                    opens_as_text: false,
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {}
            Err(e) => return Err(read_failed(&e)),
        }
    }
    if meta.len > REMOTE_FILE_MAX {
        return Ok(Large::TooLargeForText);
    }
    let bytes = match h.read_file(path, REMOTE_FILE_MAX) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => {
            return Ok(Large::TooLargeForText);
        }
        Err(e) => return Err(read_failed(&e)),
    };
    let head = &bytes[..bytes.len().min(SNIFF_PREFIX)];
    if !matches!(image_decode::sniff(head), image_decode::Sniffed::NotImage) {
        return Ok(Large::Picture { bytes: Some(bytes) });
    }
    if prefix_is_text(head) {
        return Ok(Large::TooLargeForText);
    }
    Ok(Large::Hex(io::hex_from_not_text(
        false,
        path.to_path_buf(),
        bytes,
        meta.mtime,
    )))
}

/// One file open as bytes.
pub(super) struct HexDoc {
    /// The document: its entity id is the `BufferId` tabs refer to it by.
    pub(super) data: Entity<HexData>,
    /// The machine the file is on: duplicate detection keys on its id.
    pub(super) host: SharedHost,
    /// The canonical path the host answered with.
    pub(super) path: PathBuf,
    pub(super) name: SharedString,
}

impl HexDoc {
    pub(super) fn id(&self) -> BufferId {
        self.data.entity_id()
    }
}

/// A hex tab someone asked for, until the host answers. Gone when its tab closes or the panel
/// is hidden, and an answer for a request that is gone is dropped.
pub(super) struct HexRequest {
    pub(super) tab: TabId,
}

/// What [`EditorPanelState`] keeps for hex documents.
#[derive(Default)]
pub(super) struct HexState {
    pub(super) docs: Vec<HexDoc>,
    /// Made while the editor is drawn, which only has `&self`, like the picture views.
    pub(super) views: RefCell<HashMap<ViewKey, Entity<HexView>>>,
    pub(super) requests: HashMap<u64, HexRequest>,
    pub(super) seq: u64,
    /// What go to offset and find remember between bars.
    pub(super) find: find::FindState,
}

/// What the menus and the palette may offer for one document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HexOffer {
    /// A saved text, picture or failed picture: show its bytes.
    AsHex,
    /// A hex tab of a file the text editor would take.
    AsText,
}

/// Marks the "too large" notice that carries an Open as Hex button, so it can be dismissed
/// when the button is pressed.
struct OpenAsHexOffer;

/// The three readouts the status bar shows for a hex view.
pub(super) struct HexReadouts {
    pub(super) cursor: SharedString,
    pub(super) values: SharedString,
    pub(super) size: SharedString,
}

fn loaded_path(loaded: &io::HexLoaded) -> &Path {
    match loaded {
        io::HexLoaded::File(file) => &file.path,
        io::HexLoaded::TooLarge { path, .. } => path,
    }
}

/// `u8 222 · i8 -34 · u16 LE 1 BE 2 · u32 LE 3 BE 4`, leaving out what the bytes at the caret
/// are too few to say.
fn values_text(values: &model::Values) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = values.u8 {
        parts.push(format!("u8 {v}"));
    }
    if let Some(v) = values.i8 {
        parts.push(format!("i8 {v}"));
    }
    let pair = |name: &str, le: Option<String>, be: Option<String>| {
        let mut ends = Vec::new();
        if let Some(le) = le {
            ends.push(format!("LE {le}"));
        }
        if let Some(be) = be {
            ends.push(format!("BE {be}"));
        }
        (!ends.is_empty()).then(|| format!("{name} {}", ends.join(" ")))
    };
    parts.extend(pair(
        "u16",
        values.u16le.map(|v| v.to_string()),
        values.u16be.map(|v| v.to_string()),
    ));
    parts.extend(pair(
        "u32",
        values.u32le.map(|v| v.to_string()),
        values.u32be.map(|v| v.to_string()),
    ));
    parts.join(" · ")
}

/// The status bar's words for what a hex view reports.
pub(super) fn readouts_of(status: &view::HexStatus) -> HexReadouts {
    let mut cursor = t_fmt(
        L10nKey::HexStatusOffset,
        &[
            ("hex", &format!("0x{:X}", status.offset)),
            ("dec", &status.offset.to_string()),
        ],
    );
    if let Some(selected) = status.selection {
        cursor.push_str(" · ");
        cursor.push_str(&t_fmt(
            L10nKey::HexStatusSelection,
            &[("count", &selected.to_string())],
        ));
    }
    HexReadouts {
        cursor: cursor.into(),
        values: values_text(&status.values).into(),
        size: t_fmt(
            L10nKey::HexStatusSize,
            &[(
                "size",
                &crate::ui::image_preview::view::human_size(status.len),
            )],
        )
        .into(),
    }
}

impl Tty59App {
    pub(super) fn hex(&self, id: BufferId) -> Option<&HexDoc> {
        self.editor.hex.docs.iter().find(|d| d.id() == id)
    }

    /// The hex document already open for this file, if there is one. A text buffer or a picture
    /// of the same file does not count: they are tabs of their own.
    pub(super) fn hex_at(&self, host: HostId, path: &Path) -> Option<BufferId> {
        self.editor
            .hex
            .docs
            .iter()
            .find(|d| d.host.id() == host && d.path == path)
            .map(HexDoc::id)
    }

    /// Whether `id` is a hex document with edits that are not on disk. Nothing edits one yet.
    pub(super) fn hex_dirty(&self, id: BufferId, cx: &App) -> bool {
        self.hex(id).is_some_and(|d| d.data.read(cx).dirty())
    }

    /// Whether `id` is a hex document that can be dropped without losing anything.
    pub(super) fn hex_clean(&self, id: BufferId, cx: &App) -> bool {
        self.hex(id).is_some_and(|d| !d.data.read(cx).dirty())
    }

    /// The view of `id` for this tab and group, made when it is first asked for.
    pub(super) fn hex_view(
        &self,
        id: BufferId,
        tab: TabId,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<HexView>> {
        let doc = self.hex(id)?;
        let mut views = self.editor.hex.views.borrow_mut();
        Some(
            views
                .entry((id, tab, slot))
                .or_insert_with(|| {
                    let data = doc.data.clone();
                    cx.new(|cx| HexView::new(data, slot, cx))
                })
                .clone(),
        )
    }

    /// The hex document in front of a group, drawn: `None` when `id` is not one.
    pub(super) fn render_hex_body(
        &self,
        id: BufferId,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let doc = self.hex(id)?;
        let tab = self.tabs.get(self.active)?.tree_id.get();
        let view = self.hex_view(id, tab, slot, cx)?;
        let too_large = doc.data.read(cx).too_large();
        let name = doc.name.clone();
        let path = doc.path.clone();
        let app = cx.entity().downgrade();
        let body = match too_large {
            // The view draws nothing for a file that was never read; the card is the tab.
            Some((len, limit)) => {
                let focus = view.read(cx).focus.clone();
                const MIB: u64 = 1024 * 1024;
                let message = t_fmt(
                    L10nKey::HexTooLarge,
                    &[
                        ("name", &name),
                        ("size", &(len / MIB).to_string()),
                        ("limit", &(limit / MIB).to_string()),
                    ],
                );
                Self::render_hex_too_large(id, slot, message, &path, &focus, cx)
            }
            None => view.into_any_element(),
        };
        Some(
            div()
                .id(("hex-body-menu", slot))
                .relative()
                .size_full()
                .child(body)
                .context_menu(move |menu, _window, cx| Self::editor_hex_menu(menu, &app, id, cx))
                .into_any_element(),
        )
    }

    /// The card that says a file is too large to bring over, with the way to get it anyway.
    fn render_hex_too_large(
        id: BufferId,
        slot: usize,
        message: String,
        path: &Path,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The picture that cannot be shown has this card too; here it also carries the one
        // thing that can be done about it.
        let download = Button::new(("hex-too-large-download", slot))
            .label(t(L10nKey::Download))
            .on_click(
                cx.listener(move |this, _, window, cx| this.editor_download_hex(id, window, cx)),
            )
            .into_any_element();
        error_card(
            ErrorCard {
                id: ("hex-too-large", slot),
                selector: format!("hex-too-large-{slot}"),
                focus,
                title: None,
                message,
                detail: path.display().to_string(),
                action: Some(download),
            },
            cx,
        )
    }

    /// Right-click on a hex document: what any file offers.
    fn editor_hex_menu(
        menu: PopupMenu,
        app: &gpui::WeakEntity<Self>,
        id: BufferId,
        cx: &App,
    ) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let menu = menu.min_w(px(220.));
        this.read(cx).editor_file_menu_items(menu, id, app, cx)
    }

    /// Downloads the file behind the hex document `id` to the Downloads folder, the way the
    /// file tree's Download does.
    pub(crate) fn editor_download_hex(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (host, path) = (doc.host.clone(), doc.path.clone());
        self.download_to_downloads(host, &path, window, cx);
    }

    /// The focus of the active tab's front hex view in the focused group.
    pub(super) fn hex_focus(&self, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let code = self.tab_code()?;
        let id = code.active_id()?;
        let slot = code.focused_slot();
        let tab = self.tabs.get(self.active)?.tree_id.get();
        let view = self.hex_view(id, tab, slot, cx)?;
        Some(view.read(cx).focus.clone())
    }

    /// Whether the focus is inside any hex view of `id` in `tab`.
    pub(super) fn hex_view_has_focus(
        &self,
        id: BufferId,
        tab: TabId,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.editor
            .hex
            .views
            .borrow()
            .iter()
            .filter(|((view_id, view_tab, _), _)| *view_id == id && *view_tab == tab)
            .any(|(_, view)| view.read(cx).focus.contains_focused(window, cx))
    }

    /// What the status bar says while a hex document is in front: `None` for anything else, and
    /// for a file that was never read.
    pub(super) fn hex_readouts(&self, cx: &App) -> Option<HexReadouts> {
        let code = self.tab_code()?;
        let id = code.active_id()?;
        let tab = self.tabs.get(self.active)?.tree_id.get();
        let views = self.editor.hex.views.borrow();
        let view = views.get(&(id, tab, code.focused_slot()))?;
        view.read(cx).status(cx).map(|s| readouts_of(&s))
    }

    /// What Open as Hex / Open as Text the document `id` is offered, if either: the one answer
    /// the menus, the palette and the actions read.
    pub(crate) fn hex_offer(&self, id: BufferId, cx: &App) -> Option<HexOffer> {
        match self.hex(id) {
            Some(doc) => doc.data.read(cx).opens_as_text.then_some(HexOffer::AsText),
            // Anything saved, whatever it is; an untitled buffer is not anywhere yet.
            None => self.doc_location(id).map(|_| HexOffer::AsHex),
        }
    }

    /// The saved document in front of the editor that Open as Hex applies to, while the panel is
    /// showing.
    pub(crate) fn editor_open_as_hex_target(&self) -> Option<BufferId> {
        if !self.code_panel_visible() {
            return None;
        }
        let id = self.tab_code().and_then(TabCode::active_id)?;
        (self.hex(id).is_none() && self.doc_location(id).is_some()).then_some(id)
    }

    /// The hex tab in front that Open as Text applies to: one whose file the text editor takes.
    pub(crate) fn editor_open_as_text_target(&self, cx: &App) -> Option<BufferId> {
        if !self.code_panel_visible() {
            return None;
        }
        let id = self.tab_code().and_then(TabCode::active_id)?;
        (self.hex_offer(id, cx) == Some(HexOffer::AsText)).then_some(id)
    }

    /// Opens the bytes of the saved document `id` in a tab of their own; a hex tab of the same
    /// file already open only comes forward.
    pub(crate) fn editor_open_as_hex(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.doc_host(id) else {
            return;
        };
        let Some((_, path)) = self.doc_location(id) else {
            return;
        };
        let path = path.to_path_buf();
        self.editor_open_path_as_hex(host, path, window, cx);
    }

    /// Open as Hex for a file known by its place: the file tree's row, the "too large" notice.
    pub(crate) fn editor_open_path_as_hex(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active).map(|t| t.tree_id.get()) else {
            return;
        };
        self.raise_code_overlay();
        if let Some(id) = self.hex_at(host.id(), &path) {
            self.editor_show_in_tab(tab, id, true, window, cx);
            return;
        }
        self.editor.hex.seq += 1;
        let seq = self.editor.hex.seq;
        self.editor.hex.requests.insert(seq, HexRequest { tab });
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| io::load_hex(h, path),
            move |app, loaded, window, cx| {
                let Some(request) = app.editor.hex.requests.remove(&seq) else {
                    return;
                };
                match loaded {
                    Ok(loaded) => {
                        app.editor_land_hex(request.tab, host, loaded, true, window, cx);
                    }
                    Err(EditorOpenError::Message(message))
                    | Err(EditorOpenError::TooLarge { message, .. }) => {
                        window.push_notification(message, cx);
                    }
                    // `load_hex` never decides a file is not text.
                    Err(EditorOpenError::NotText { .. }) => {}
                }
            },
        );
    }

    /// Opens the file behind the hex tab `id` as text, or brings its text tab forward.
    pub(crate) fn editor_open_as_text(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.hex(id) else {
            return;
        };
        let (host, path) = (doc.host.clone(), doc.path.clone());
        self.editor_open_on_host(host, &path, window, cx);
    }

    /// The words for a text file too large for the editor, with a button that opens it as hex.
    pub(super) fn editor_too_large_notice(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        let key: SharedString = path.display().to_string().into();
        let note = Notification::new()
            .message(message)
            .id1::<OpenAsHexOffer>(key.clone())
            .action(move |_, _, _| {
                let (app, host, path, key) = (app.clone(), host.clone(), path.clone(), key.clone());
                Button::new("editor-too-large-open-as-hex")
                    .label(t(L10nKey::CmdOpenAsHex))
                    .on_click(move |_, window, cx| {
                        window.remove_notification1::<OpenAsHexOffer>(key.clone(), cx);
                        let (host, path) = (host.clone(), path.clone());
                        let _ = app.update(cx, |this, cx| {
                            this.editor_open_path_as_hex(host, path, window, cx)
                        });
                    })
            });
        window.push_notification(note, cx);
    }

    /// Puts a loaded file in `tab`'s strip as a hex document of its own, or finds the one
    /// already open for it. `None`, and no document, when the tab is gone: a hex tab for a tab
    /// that has since closed is not put in another.
    pub(super) fn editor_land_hex(
        &mut self,
        tab: TabId,
        host: SharedHost,
        loaded: io::HexLoaded,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<BufferId> {
        self.tab_index_of(tab)?;
        // The same file asked for under another spelling can have landed first.
        if let Some(id) = self.hex_at(host.id(), loaded_path(&loaded)) {
            self.show_in_tab_strict(tab, id, front, window, cx);
            return Some(id);
        }
        self.editor_install_hex(host, loaded, tab, front, window, cx)
    }

    pub(super) fn editor_install_hex(
        &mut self,
        host: SharedHost,
        loaded: io::HexLoaded,
        tab: TabId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<BufferId> {
        self.tab_index_of(tab)?;
        let data = HexData::from_loaded(host.clone(), loaded);
        let (path, name) = (data.path.clone(), data.name());
        let entity = cx.new(|_| data);
        let id = entity.entity_id();
        self.editor.hex.docs.push(HexDoc {
            data: entity,
            host,
            path,
            name,
        });
        self.editor_rebuild_watcher(cx);
        self.editor_show_in_tab(tab, id, front, window, cx);
        Some(id)
    }

    /// A file or folder was renamed through tty59: hex documents at or under the old path follow
    /// it. Nothing is read again.
    pub(super) fn hex_path_moved(
        &mut self,
        host: HostId,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut moved = false;
        for doc in self.editor.hex.docs.iter_mut() {
            if doc.host.id() != host {
                continue;
            }
            let Ok(rest) = doc.path.strip_prefix(from) else {
                continue;
            };
            let new = if rest.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(rest)
            };
            doc.path = new.clone();
            doc.data.update(cx, |data, cx| data.set_path(new, cx));
            doc.name = doc.data.read(cx).name();
            moved = true;
        }
        // The caller rebuilds the watcher when anything moved.
        moved
    }

    /// Forgets a hex document everywhere.
    pub(super) fn editor_drop_hex(&mut self, id: BufferId, cx: &mut Context<Self>) {
        for code in self.tabs.iter_mut().filter_map(|t| t.code.as_deref_mut()) {
            code.forget_everywhere(id);
        }
        self.editor
            .hex
            .views
            .borrow_mut()
            .retain(|(view_id, ..), _| *view_id != id);
        self.editor.hex.docs.retain(|d| d.id() != id);
        // A bar asking about this document has nothing left to ask about, and dropping it
        // cancels the search it was waiting on.
        if self.editor.bar.as_ref().is_some_and(|b| {
            matches!(b.kind, BarKind::GoToOffset { id: shown } | BarKind::HexFind { id: shown, .. } if shown == id)
        }) {
            self.editor.bar = None;
        }
        // Whatever was waiting on this document to be saved is not going to see that now.
        self.editor_saves_failed(id);
        self.editor_rebuild_watcher(cx);
        cx.notify();
    }

    /// What the draw loop tidies about hex documents: documents no tab shows any more go, and so
    /// do views for places that are gone.
    pub(super) fn editor_sweep_hex(&mut self, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let shown: HashSet<BufferId> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        // A document with edits is never swept: `hex_adopt_orphans` hands it to the tab in
        // front, as a text buffer's are, rather than losing it.
        let orphans: Vec<BufferId> = self
            .editor
            .hex
            .docs
            .iter()
            .map(HexDoc::id)
            .filter(|id| !shown.contains(id) && self.hex_clean(*id, cx))
            .collect();
        for id in orphans {
            self.editor_drop_hex(id, cx);
        }
        let live: HashSet<ViewKey> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref().map(|c| (t.tree_id.get(), c.sides())))
            .flat_map(|(tab, sides)| {
                sides.into_iter().enumerate().flat_map(move |(slot, side)| {
                    side.files.into_iter().map(move |id| (id, tab, slot))
                })
            })
            .collect();
        self.editor
            .hex
            .views
            .borrow_mut()
            .retain(|key, _| live.contains(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_is_text_by_these_rules() {
        let cut = &"é".as_bytes()[..1];
        let mut cut_at_end = b"plain ascii then a cut ".to_vec();
        cut_at_end.extend_from_slice(&"€".as_bytes()[..2]);
        let mut broken_early = b"plain ".to_vec();
        broken_early.push(0xFF);
        broken_early.extend_from_slice(b" and more text after it");
        let mut broken_in_last_three = b"text ".to_vec();
        broken_in_last_three.extend_from_slice(&[0xE2, 0x28, 0xA1]);
        let table: [(&str, &[u8], bool); 9] = [
            ("empty", b"", true),
            ("ascii", b"fn main() {}\n", true),
            ("utf-8", "naïve café, 日本語\n".as_bytes(), true),
            ("a character cut at the end", &cut_at_end, true),
            ("a lone lead byte", cut, true),
            ("a NUL byte", b"text\0text", false),
            ("a utf-8 break before the end", &broken_early, false),
            (
                "an invalid sequence at the end",
                &broken_in_last_three,
                false,
            ),
            (
                "a utf-16 byte-order mark",
                &[0xFF, 0xFE, b'a', 0, b'b', 0],
                true,
            ),
        ];
        for (what, bytes, want) in table {
            assert_eq!(prefix_is_text(bytes), want, "{what}");
        }
    }

    #[test]
    fn an_executable_header_is_not_text() {
        assert!(!prefix_is_text(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0"));
    }
}

#[cfg(test)]
mod doc_tests;
