//! Image documents in the editor's tabs and groups.
//!
//! A picture is not a text buffer and is not given a fake one: it is an
//! [`ImageData`] entity, whose id is a `BufferId`, so every strip, split group,
//! close and restore path that already speaks in those ids shows and closes it
//! unchanged. What those paths need to ask *about* a document — its name, its
//! place, whether it is unsaved — goes through the small lookups here, which
//! answer for either kind.
//!
//! The pixels are the document's; where they are drawn is a view's. A view is
//! one `(document, tab, group)`, so the viewport, the size laid out in and the
//! focus are never shared between two places the same picture is shown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, Image, ImageFormat, RenderImage,
    SharedString,
};

use super::*;
use crate::ui::clipboard_image::png_of_first_frame;
#[cfg(target_os = "linux")]
use crate::ui::clipboard_image::{ClipboardExport, write_clipboard_image};
use crate::ui::image_preview::view::Content;
use crate::ui::image_preview::{ImageData, ImageView};

/// One open picture.
pub(super) struct ImageDoc {
    /// The document: its entity id is the `BufferId` tabs refer to it by.
    pub(super) data: Entity<ImageData>,
    /// The machine the file is on, held like a text buffer's: duplicate
    /// detection keys on its id.
    pub(super) host: SharedHost,
    /// The canonical path the host answered with.
    pub(super) path: PathBuf,
    pub(super) name: SharedString,
}

impl ImageDoc {
    pub(super) fn id(&self) -> BufferId {
        self.data.entity_id()
    }
}

/// A picture someone asked to open, until its bytes arrive.
///
/// The request is what the arrival is checked against: it is gone when a
/// newer request for the same file supersedes it, when its tab closes, or when
/// the editor panel is closed, and a picture that arrives for a request that
/// is gone is dropped instead of being put somewhere nobody asked for.
pub(super) struct ImageRequest {
    pub(super) tab: TabId,
    pub(super) host: HostId,
    /// The path as it was asked for, which the pending cursor is keyed by.
    pub(super) path: PathBuf,
}

/// Where a picture is drawn: the document, the tab, the editor group.
pub(super) type ViewKey = (BufferId, TabId, usize);

/// What [`EditorPanelState`] keeps for pictures.
#[derive(Default)]
pub(super) struct ImageState {
    pub(super) docs: Vec<ImageDoc>,
    /// Views are made while the editor is drawn, which only has `&self`; the
    /// map is the one piece of drawing state that grows then.
    pub(super) views: RefCell<HashMap<ViewKey, Entity<ImageView>>>,
    pub(super) requests: HashMap<u64, ImageRequest>,
    pub(super) seq: u64,
    /// How a copied picture reaches the desktop's clipboard, where gpui's own
    /// write does not (Linux).
    #[cfg(target_os = "linux")]
    pub(super) export: ClipboardExport,
}

/// What the menu and the palette may offer for one picture.
pub(crate) struct ImageActions {
    pub(crate) copy: bool,
    pub(crate) download: bool,
}

impl Tty59App {
    pub(super) fn image(&self, id: BufferId) -> Option<&ImageDoc> {
        self.editor.images.docs.iter().find(|d| d.id() == id)
    }

    /// The picture already open for this file, if there is one.
    pub(super) fn image_at(&self, host: HostId, path: &Path) -> Option<BufferId> {
        self.editor
            .images
            .docs
            .iter()
            .find(|d| d.host.id() == host && d.path == path)
            .map(ImageDoc::id)
    }

    /// The name a document goes by in a strip, whatever kind it is.
    pub(super) fn doc_label(&self, id: BufferId) -> Option<SharedString> {
        match self.buffer(id) {
            Some(f) => Some(f.label()),
            None => self.image(id).map(|d| d.name.clone()),
        }
    }

    /// Where a saved document lives. `None` for an untitled one.
    pub(super) fn doc_location(&self, id: BufferId) -> Option<(HostId, &Path)> {
        if let Some(f) = self.buffer(id) {
            return f
                .untitled
                .is_none()
                .then(|| (f.host.id(), f.path.as_path()));
        }
        self.image(id).map(|d| (d.host.id(), d.path.as_path()))
    }

    /// Whether the document in front of the active tab's editor is any kind
    /// of document: the test for "the panel is not empty".
    pub(super) fn active_doc_present(&self) -> bool {
        self.tab_code()
            .and_then(TabCode::active_id)
            .is_some_and(|id| self.buffer(id).is_some() || self.image(id).is_some())
    }

    /// The view of `id` for this tab and group, made when it is first asked
    /// for.
    pub(super) fn image_view(
        &self,
        id: BufferId,
        tab: TabId,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ImageView>> {
        let doc = self.image(id)?;
        let mut views = self.editor.images.views.borrow_mut();
        Some(
            views
                .entry((id, tab, slot))
                .or_insert_with(|| {
                    let data = doc.data.clone();
                    cx.new(|cx| ImageView::new(data, slot, cx))
                })
                .clone(),
        )
    }

    /// The picture in front of a group, drawn: `None` when `id` is not one.
    pub(super) fn render_image_body(
        &self,
        id: BufferId,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self.tabs.get(self.active)?.tree_id.get();
        let view = self.image_view(id, tab, slot, cx)?;
        let app = cx.entity().downgrade();
        Some(
            div()
                .id(("image-preview-menu", slot))
                .relative()
                .size_full()
                .child(view)
                .context_menu(move |menu, _window, cx| Self::editor_image_menu(menu, &app, id, cx))
                .into_any_element(),
        )
    }

    /// What can be done to the picture `id`: the one answer the right-click
    /// menu and the palette both read. `None` when `id` is not a picture.
    pub(crate) fn image_actions(&self, id: BufferId, cx: &App) -> Option<ImageActions> {
        let doc = self.image(id)?;
        Some(ImageActions {
            // Only decoded pixels can be copied; a picture that failed has none.
            copy: doc.data.read(cx).image().is_some(),
            // TODO(download): B contract. M2 turns it on for any open picture,
            // a failed one included.
            download: false,
        })
    }

    /// The picture in front of the active tab's editor, while the panel is
    /// showing.
    pub(crate) fn editor_active_image(&self) -> Option<BufferId> {
        if !self.code_panel_visible() {
            return None;
        }
        let id = self.tab_code().and_then(TabCode::active_id)?;
        self.image(id).map(ImageDoc::id)
    }

    /// Right-click on a picture: copy it, then what any file offers.
    fn editor_image_menu(
        menu: PopupMenu,
        app: &gpui::WeakEntity<Self>,
        id: BufferId,
        cx: &App,
    ) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(actions) = this.image_actions(id, cx) else {
            return menu;
        };
        let menu = menu.min_w(px(220.)).item(
            PopupMenuItem::new(t(L10nKey::ImageCopy))
                .disabled(!actions.copy)
                .on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.editor_copy_image(id, window, cx));
                    }
                }),
        );
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// The focus of the active tab's front picture in the focused group.
    pub(super) fn image_focus(&self, cx: &mut Context<Self>) -> Option<FocusHandle> {
        let code = self.tab_code()?;
        let id = code.active_id()?;
        let slot = code.focused_slot();
        let tab = self.tabs.get(self.active)?.tree_id.get();
        let view = self.image_view(id, tab, slot, cx)?;
        Some(view.read(cx).focus.clone())
    }

    /// Whether the focus is inside a view of the active tab's front picture.
    pub(super) fn image_has_focus(&self, window: &Window, cx: &App) -> bool {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return false;
        };
        let Some(tab) = self.tabs.get(self.active).map(|t| t.tree_id.get()) else {
            return false;
        };
        self.view_has_focus(id, tab, window, cx)
    }

    /// Whether the focus is inside any view of `id` in `tab`.
    pub(super) fn view_has_focus(
        &self,
        id: BufferId,
        tab: TabId,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.editor
            .images
            .views
            .borrow()
            .iter()
            .filter(|((view_id, view_tab, _), _)| *view_id == id && *view_tab == tab)
            .any(|(_, view)| view.read(cx).focus.contains_focused(window, cx))
    }

    /// Shows `id` in `tab`, and only in `tab`: `false` when the tab is gone.
    /// `editor_show_in_tab` falls back to whichever tab is active, which is
    /// right for a text file asked for by a click and wrong for a picture
    /// that arrives for a tab that has since closed.
    pub(super) fn show_in_tab_strict(
        &mut self,
        tab: TabId,
        id: BufferId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab_index_of(tab).is_none() {
            return false;
        }
        self.editor_show_in_tab(tab, id, front, window, cx);
        true
    }

    /// Puts a decoded picture (or the failure that stands in for one) in
    /// `tab`'s strip as a document of its own. `None`, and no document, when
    /// the tab is gone.
    pub(super) fn editor_install_image(
        &mut self,
        host: SharedHost,
        data: ImageData,
        tab: TabId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<BufferId> {
        self.tab_index_of(tab)?;
        let path = data.path.clone();
        let name = data.name();
        let entity = cx.new(|_| data);
        let id = entity.entity_id();
        self.editor.images.docs.push(ImageDoc {
            data: entity,
            host,
            path,
            name,
        });
        self.editor_show_in_tab(tab, id, front, window, cx);
        Some(id)
    }

    /// Notes that `path` is being opened for `tab`, and returns what the
    /// answer must present to be taken. A request for the same file in the
    /// same tab replaces an earlier one.
    pub(super) fn image_request(&mut self, tab: TabId, host: HostId, path: &Path) -> u64 {
        self.editor
            .images
            .requests
            .retain(|_, r| !(r.tab == tab && r.host == host && r.path == path));
        self.editor.images.seq += 1;
        let seq = self.editor.images.seq;
        self.editor.images.requests.insert(
            seq,
            ImageRequest {
                tab,
                host,
                path: path.to_path_buf(),
            },
        );
        seq
    }

    /// Forgets what tab `tab` was waiting for: it closed, or so did its editor.
    pub(super) fn image_requests_cancel(&mut self, tab: TabId) {
        self.editor.images.requests.retain(|_, r| r.tab != tab);
    }

    /// The reader closed `tab`'s editor panel (or the tab itself went): what
    /// was being opened for it was asked for by a panel that is no longer
    /// there, and a restore still on its way must not put the panel back.
    pub(super) fn note_user_hide(&mut self, tab: TabId) {
        *self.editor.panel_epoch.entry(tab).or_insert(0) += 1;
        self.image_requests_cancel(tab);
    }

    /// How many times the reader has hidden `tab`'s panel: captured when a
    /// restore starts, compared when it lands.
    pub(super) fn panel_epoch(&self, tab: TabId) -> u64 {
        self.editor.panel_epoch.get(&tab).copied().unwrap_or(0)
    }

    /// Drops a pending `:line:column` for a file that turned out not to have
    /// lines: left alone it would send the cursor of the next text file opened
    /// at that path.
    pub(super) fn clear_pending_cursor(&mut self, requested: &Path) {
        if self
            .editor
            .pending_cursor
            .as_ref()
            .is_some_and(|(path, ..)| path == requested)
        {
            self.editor.pending_cursor = None;
        }
    }

    /// An answer for request `seq`: a picture, or the failure that stands in
    /// for one. Taken only if the request is still wanted.
    pub(super) fn editor_land_image(
        &mut self,
        seq: u64,
        host: SharedHost,
        data: ImageData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.editor.images.requests.remove(&seq) else {
            return;
        };
        self.clear_pending_cursor(&request.path);
        // Not `editor_show_in_tab`'s fallback to whichever tab is active now:
        // a picture for a tab that has gone is not put in another.
        if self.tab_index_of(request.tab).is_none() {
            return;
        }
        // The same file asked for under another spelling can have landed
        // first: show that one rather than a second copy.
        if let Some(id) = self.image_at(host.id(), &data.path) {
            self.show_in_tab_strict(request.tab, id, true, window, cx);
            return;
        }
        self.editor_install_image(host, data, request.tab, true, window, cx);
    }

    /// The request for `seq` is answered by something that is not a picture;
    /// it is finished either way.
    pub(super) fn image_request_done(&mut self, seq: u64) {
        self.editor.images.requests.remove(&seq);
    }

    /// A file or folder was renamed through tty59: pictures at or under the
    /// old path follow it, in the document's own record and in what the views
    /// and the error card say. Nothing is read again.
    pub(super) fn image_path_moved(
        &mut self,
        host: HostId,
        from: &Path,
        to: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut moved = false;
        for doc in self.editor.images.docs.iter_mut() {
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
            doc.name = new
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| new.display().to_string())
                .into();
            doc.path = new.clone();
            doc.data.update(cx, |data, cx| {
                data.path = new;
                cx.notify();
            });
            moved = true;
        }
        moved
    }

    /// Forgets a picture everywhere. Its pixels leave the window's atlas on
    /// the next turn, once no window is being updated: this runs inside one.
    pub(super) fn editor_drop_image(&mut self, id: BufferId, cx: &mut Context<Self>) {
        for code in self.tabs.iter_mut().filter_map(|t| t.code.as_deref_mut()) {
            code.forget_everywhere(id);
        }
        self.editor
            .images
            .views
            .borrow_mut()
            .retain(|(view_id, ..), _| *view_id != id);
        let Some(at) = self.editor.images.docs.iter().position(|d| d.id() == id) else {
            return;
        };
        let doc = self.editor.images.docs.remove(at);
        let image = doc.data.read(cx).image();
        release_images(image.into_iter().collect(), cx);
        cx.notify();
    }

    /// Copies the picture `id` to the clipboard as a PNG of what the viewer
    /// holds: the decoded first frame. So an SVG goes as its raster, an
    /// animation as its first frame and a picture too large to hold whole as
    /// the reduced pixels; the file's own bytes are not kept, and a copy that
    /// is not what is on screen would surprise more than one that is. A
    /// picture that failed to decode has nothing to copy.
    ///
    /// The encode, and the file the Linux tools read, are made off the UI
    /// thread (a large picture's PNG takes long enough to be felt); only the
    /// clipboard write comes back to it. gpui's write reaches other
    /// applications on macOS and Windows; on Linux it pastes into tty59 only,
    /// so the staged file is also exported (see `clipboard_image`).
    pub(crate) fn editor_copy_image(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.image(id) else {
            return;
        };
        let Content::Ready(shown) = &doc.data.read(cx).content else {
            return;
        };
        let (pixels, name) = (shown.image.clone(), doc.name.clone());
        #[cfg(target_os = "linux")]
        let export = self.editor.images.export.clone();
        cx.spawn_in(window, async move |app, cx| {
            let staged = cx
                .background_spawn(async move {
                    let png = png_of_first_frame(&pixels)?;
                    let image = Image::from_bytes(ImageFormat::Png, png);
                    #[cfg(target_os = "linux")]
                    let staged = Some(write_clipboard_image(&image)?);
                    #[cfg(not(target_os = "linux"))]
                    let staged: Option<PathBuf> = None;
                    Some((image, staged))
                })
                .await;
            let _ = app.update_in(cx, |_, window, cx| match staged {
                Some((image, staged)) => {
                    cx.write_to_clipboard(ClipboardItem::new_image(&image));
                    #[cfg(target_os = "linux")]
                    if let Some(file) = staged {
                        export.submit(file, window, cx);
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = staged;
                }
                None => window
                    .push_notification(t_fmt(L10nKey::ImageCopyFailed, &[("name", &name)]), cx),
            });
        })
        .detach();
    }

    /// What the draw loop tidies about pictures: documents no tab shows any
    /// more go, and so do views for places that are gone.
    pub(super) fn editor_sweep_images(&mut self, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        let shown: HashSet<BufferId> = self
            .tabs
            .iter()
            .filter_map(|t| t.code.as_deref())
            .flat_map(TabCode::all_files)
            .collect();
        let orphans: Vec<BufferId> = self
            .editor
            .images
            .docs
            .iter()
            .map(ImageDoc::id)
            .filter(|id| !shown.contains(id))
            .collect();
        for id in orphans {
            self.editor_drop_image(id, cx);
        }
        // A view lives while its group of its tab lists its picture: the left
        // group is slot 0, the right one slot 1.
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
            .images
            .views
            .borrow_mut()
            .retain(|key, _| live.contains(key));
    }
}

/// Drops pictures from every window's atlas. Deferred to when no window is
/// being updated, because the one that is closing the document is one of them
/// and an update of it from inside its own update finds nothing; windows that
/// never drew the picture, or have closed, answer harmlessly.
pub(super) fn release_images(images: Vec<Arc<RenderImage>>, cx: &mut Context<Tty59App>) {
    if images.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for handle in cx.windows() {
            let images = images.clone();
            let _ = handle.update(cx, move |_, window, _| {
                for image in images {
                    let _ = window.drop_image(image);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext};

    use super::super::split::OtherGroup;
    use super::super::{ImageLoaded, Opened};
    use super::*;
    use crate::core::actions::IncreaseFontSize;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::editor_session::TabEditor;
    use crate::ui::image_preview::decode as image_decode;

    fn real_dir(dir: &tempfile::TempDir) -> PathBuf {
        tty59_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(dir.path()).unwrap(),
        )
    }

    /// A PNG of one flat colour, so its size is the only thing about it.
    fn write_png(path: &Path, width: u32, height: u32) {
        image::RgbaImage::from_pixel(width, height, image::Rgba([40, 90, 200, 255]))
            .save(path)
            .unwrap();
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    /// Runs the host's blocking work until `done` holds, bounded.
    fn wait(vcx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(vcx) && Instant::now() < deadline {
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        settle(vcx);
    }

    fn docs(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
        app.read_with(vcx, |app, _| app.editor.images.docs.len())
    }

    fn open(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, path: &Path, want: usize) {
        app.update_in(vcx, |app, window, cx| {
            app.open_file_in_editor(path, window, cx);
        });
        wait(vcx, |vcx| docs(app, vcx) >= want);
    }

    fn id_of(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, path: &Path) -> BufferId {
        app.read_with(vcx, |app, cx| {
            let host = app.active_host(cx).expect("a host").id();
            app.image_at(host, path).expect("the picture is open")
        })
    }

    fn painted(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not painted"))
    }

    fn near(got: f32, want: f32, what: &str) {
        assert!((got - want).abs() < 0.75, "{what}: got {got}, want {want}");
    }

    /// Two groups, each with a picture of its own shape: each is fitted in
    /// its own group, zooming one leaves the other alone, and closing one
    /// leaves the other drawn and the closed one's pixels released.
    #[gpui::test]
    fn two_groups_fit_zoom_and_close_independently(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (wide, tall) = (
            real_dir(&dir).join("wide.png"),
            real_dir(&dir).join("tall.png"),
        );
        write_png(&wide, 400, 200);
        write_png(&tall, 100, 400);

        open(&app, &mut vcx, &wide, 1);
        open(&app, &mut vcx, &tall, 2);
        let (a, b) = (id_of(&app, &mut vcx, &wide), id_of(&app, &mut vcx, &tall));
        // The wide one in the focused (left) group, the tall one in the other.
        app.update_in(&mut vcx, |app, window, cx| {
            let code = app.tab_code_mut().expect("a tab with an editor");
            code.files = vec![a];
            code.active = 0;
            code.split = Some(OtherGroup {
                files: vec![b],
                active: 0,
                focus_left: true,
            });
            app.focus_editor(window, cx);
            cx.notify();
        });
        settle(&mut vcx);

        let (body_l, body_r) = (
            painted(&mut vcx, "image-preview-body"),
            painted(&mut vcx, "image-preview-body-1"),
        );
        let (img_l, img_r) = (
            painted(&mut vcx, "image-preview-img"),
            painted(&mut vcx, "image-preview-img-1"),
        );
        // Each picture has its own aspect ratio and sits inside its own group.
        near(
            img_l.size.width.as_f32() / img_l.size.height.as_f32(),
            2.0,
            "left aspect",
        );
        near(
            img_r.size.width.as_f32() / img_r.size.height.as_f32(),
            0.25,
            "right aspect",
        );
        for (img, body, what) in [(img_l, body_l, "left"), (img_r, body_r, "right")] {
            assert!(
                img.left().as_f32() >= body.left().as_f32() - 0.5,
                "{what} left"
            );
            assert!(
                img.right().as_f32() <= body.right().as_f32() + 0.5,
                "{what} right"
            );
            assert!(
                img.top().as_f32() >= body.top().as_f32() - 0.5,
                "{what} top"
            );
            assert!(
                img.bottom().as_f32() <= body.bottom().as_f32() + 0.5,
                "{what} bottom"
            );
        }
        // Two views, one per group.
        assert_eq!(
            app.read_with(&vcx, |app, _| app.editor.images.views.borrow().len()),
            2
        );

        // Zoom the focused group's picture: only that one grows.
        let before = (img_l.size.width.as_f32(), img_r.size.width.as_f32());
        vcx.dispatch_action(IncreaseFontSize);
        settle(&mut vcx);
        let after = (
            painted(&mut vcx, "image-preview-img").size.width.as_f32(),
            painted(&mut vcx, "image-preview-img-1").size.width.as_f32(),
        );
        assert!(
            after.0 > before.0 * 1.2,
            "the focused picture zooms: {before:?} -> {after:?}"
        );
        near(after.1, before.1, "the other group's picture is untouched");

        // Close the focused one: the other stays, and the closed one's pixels
        // are no longer held by anything.
        let weak = app.read_with(&vcx, |app, cx| {
            Arc::downgrade(&app.image(a).unwrap().data.read(cx).image().unwrap())
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        settle(&mut vcx);
        vcx.run_until_parked();
        assert_eq!(docs(&app, &mut vcx), 1);
        assert!(app.read_with(&vcx, |app, _| app.image(a).is_none()
            && app.image(b).is_some()));
        assert!(
            vcx.debug_bounds("image-preview-body").is_some(),
            "the survivor is drawn"
        );
        assert!(
            vcx.debug_bounds("image-preview-body-1").is_none(),
            "in a single group now"
        );
        assert!(
            weak.upgrade().is_none(),
            "the closed picture's pixels are released"
        );
        assert!(
            app.read_with(&vcx, |app, _| app
                .editor
                .images
                .views
                .borrow()
                .keys()
                .all(|(id, ..)| *id == b)),
            "its views went with it"
        );
    }

    /// A picture already open comes forward with the viewport it had, and
    /// nothing is read again.
    #[gpui::test]
    fn opening_an_open_picture_again_only_brings_it_forward(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (
            real_dir(&dir).join("one.png"),
            real_dir(&dir).join("two.png"),
        );
        write_png(&one, 300, 150);
        write_png(&two, 50, 50);
        open(&app, &mut vcx, &one, 1);
        vcx.dispatch_action(IncreaseFontSize);
        settle(&mut vcx);
        let zoomed = painted(&mut vcx, "image-preview-img").size.width.as_f32();
        open(&app, &mut vcx, &two, 2);
        // Asking for the first again, with a line a text file would use.
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor_at(&one, Some(7), Some(2), window, cx);
        });
        settle(&mut vcx);
        assert_eq!(docs(&app, &mut vcx), 2, "no second copy");
        assert!(
            app.read_with(&vcx, |app, _| app.editor.pending_cursor.is_none()),
            "a picture has no lines: the cursor request must not wait for the next file"
        );
        assert!(app.read_with(&vcx, |app, _| app.editor.images.requests.is_empty()));
        near(
            painted(&mut vcx, "image-preview-img").size.width.as_f32(),
            zoomed,
            "the viewport it had",
        );
    }

    /// A line asked for on a picture that has to be read first does not wait
    /// for the next text file either.
    #[gpui::test]
    fn a_line_on_a_picture_is_not_left_for_the_next_file(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 20, 20);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor_at(&png, Some(7), Some(2), window, cx);
        });
        wait(&mut vcx, |vcx| docs(&app, vcx) >= 1);
        assert!(app.read_with(&vcx, |app, _| app.editor.pending_cursor.is_none()));
    }

    /// An answer is taken only for a request that is still wanted.
    #[gpui::test]
    fn an_answer_for_a_request_that_is_gone_is_dropped(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let path = PathBuf::from("/nowhere/late.png");
        let data = || {
            ImageData::failed(
                PathBuf::from("/nowhere/late.png"),
                crate::ui::image_preview::decode::ImageError::Corrupt,
                None,
            )
        };
        let landed = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext, seq: u64| {
            app.update_in(vcx, |app, window, cx| {
                let host = app.active_host(cx).expect("a host");
                app.editor_land_image(seq, host, data(), window, cx);
            });
            docs(app, vcx)
        };

        // Cancelled by its tab closing (or its panel being closed).
        let (tab, host) = app.read_with(&vcx, |app, cx| {
            (
                app.tabs[0].tree_id.get(),
                app.active_host(cx).expect("a host").id(),
            )
        });
        let seq = app.update(&mut vcx, |app, _| app.image_request(tab, host, &path));
        app.update(&mut vcx, |app, _| app.image_requests_cancel(tab));
        assert_eq!(landed(&app, &mut vcx, seq), 0, "cancelled");

        // Superseded by a newer request for the same file.
        let old = app.update(&mut vcx, |app, _| app.image_request(tab, host, &path));
        let new = app.update(&mut vcx, |app, _| app.image_request(tab, host, &path));
        assert_eq!(landed(&app, &mut vcx, old), 0, "superseded");
        assert_eq!(landed(&app, &mut vcx, new), 1, "the newer one is taken");

        // The same answer twice, or the same file under another request: one
        // document.
        let again = app.update(&mut vcx, |app, _| app.image_request(tab, host, &path));
        assert_eq!(landed(&app, &mut vcx, again), 1, "no second copy");
        assert_eq!(
            landed(&app, &mut vcx, new),
            1,
            "an answered request is spent"
        );
    }

    /// A picture that cannot be shown has a tab, and says why.
    #[gpui::test]
    fn a_corrupt_picture_is_an_error_in_its_tab(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let bad = real_dir(&dir).join("bad.png");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png.truncate(png.len() / 2);
        std::fs::write(&bad, &png).unwrap();
        open(&app, &mut vcx, &bad, 1);
        assert!(vcx.debug_bounds("image-preview-error").is_some());
        assert!(vcx.debug_bounds("image-preview-body").is_none());
        assert!(app.read_with(&vcx, |app, _| {
            app.tab_code().is_some_and(|c| c.files.len() == 1)
        }));
    }

    /// A program with a picture's name is an error in the tab: it is never
    /// handed to the desktop, and so never run.
    #[cfg(unix)]
    #[gpui::test]
    fn a_program_named_like_a_picture_is_an_error_not_an_open(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let program = real_dir(&dir).join("shot.png");
        std::fs::write(
            &program,
            b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x03\0\x3e\0",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        open(&app, &mut vcx, &program, 1);
        assert!(
            vcx.debug_bounds("image-preview-error").is_some(),
            "the name is a picture's, the content is not: said in the tab"
        );
    }

    /// A PNG cut in half: it opens as a tab that says it cannot be shown.
    fn write_corrupt_png(path: &Path) {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png.truncate(png.len() / 2);
        std::fs::write(path, &png).unwrap();
    }

    /// Opens a text file and waits until it is the buffer in front.
    fn open_text(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, path: &Path) {
        app.update_in(vcx, |app, window, cx| {
            app.open_file_in_editor(path, window, cx);
        });
        wait(vcx, |vcx| {
            app.read_with(vcx, |app, _| app.active_buffer().is_some())
        });
    }

    fn palette_kinds(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
    ) -> Vec<crate::ui::search::CommandKind> {
        app.read_with(vcx, |app, cx| {
            app.editor_palette_items(cx)
                .into_iter()
                .map(|i| i.kind)
                .collect()
        })
    }

    /// What can be done to a picture: copy only what has pixels, nothing to
    /// anything that is not a picture.
    #[gpui::test]
    fn image_actions_follow_what_the_picture_is(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (good, bad, text) = (
            real_dir(&dir).join("good.png"),
            real_dir(&dir).join("bad.png"),
            real_dir(&dir).join("note.txt"),
        );
        write_png(&good, 20, 10);
        write_corrupt_png(&bad);
        std::fs::write(&text, "hello\n").unwrap();
        open(&app, &mut vcx, &good, 1);
        open(&app, &mut vcx, &bad, 2);
        open_text(&app, &mut vcx, &text);
        let (good, bad) = (id_of(&app, &mut vcx, &good), id_of(&app, &mut vcx, &bad));
        let text_id = app.read_with(&vcx, |app, _| {
            app.tab_code().and_then(TabCode::active_id).expect("a file")
        });
        app.read_with(&vcx, |app, cx| {
            let ready = app.image_actions(good, cx).expect("a picture");
            assert!(ready.copy);
            assert!(!ready.download, "download is not wired yet");
            let failed = app.image_actions(bad, cx).expect("a picture");
            assert!(!failed.copy, "nothing to copy from a picture that failed");
            assert!(app.image_actions(text_id, cx).is_none(), "not a picture");
        });
    }

    /// The palette offers Copy Image for the picture in front, and for nothing
    /// else: not a text file, not a hidden panel, not a picture that failed.
    #[gpui::test]
    fn the_palette_offers_copy_image_only_for_a_picture_in_front(cx: &mut TestAppContext) {
        use crate::ui::search::CommandKind;

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (good, bad, text) = (
            real_dir(&dir).join("good.png"),
            real_dir(&dir).join("bad.png"),
            real_dir(&dir).join("note.txt"),
        );
        write_png(&good, 20, 10);
        write_corrupt_png(&bad);
        std::fs::write(&text, "hello\n").unwrap();

        open(&app, &mut vcx, &good, 1);
        assert_eq!(
            palette_kinds(&app, &mut vcx),
            [CommandKind::EditorCopyImage],
            "a picture in front; no download row until it is wired"
        );

        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_code_panel(window, cx)
        });
        assert!(
            palette_kinds(&app, &mut vcx).is_empty(),
            "the panel is hidden"
        );
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_code_panel(window, cx)
        });

        open(&app, &mut vcx, &bad, 2);
        assert!(
            !palette_kinds(&app, &mut vcx).contains(&CommandKind::EditorCopyImage),
            "a failed picture has nothing to copy"
        );

        open_text(&app, &mut vcx, &text);
        assert!(
            !palette_kinds(&app, &mut vcx).contains(&CommandKind::EditorCopyImage),
            "a text file is in front"
        );
    }

    /// The palette row runs the copy for the picture in front: the clipboard
    /// then holds an image. Needs the real `editor_copy_image`, not the
    /// placeholder.
    #[gpui::test]
    fn the_palette_command_copies_the_picture_in_front(cx: &mut TestAppContext) {
        use crate::ui::search::CommandKind;

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 20, 10);
        open(&app, &mut vcx, &png, 1);
        app.update_in(&mut vcx, |app, window, cx| {
            app.run_command(CommandKind::EditorCopyImage, window, cx)
        });
        wait(&mut vcx, |vcx| vcx.read_from_clipboard().is_some());
        let copied = vcx.read_from_clipboard().expect("something was copied");
        assert!(
            matches!(
                copied.entries().first(),
                Some(gpui::ClipboardEntry::Image(_))
            ),
            "the clipboard holds an image"
        );
    }

    /// The bindable action copies the picture in front, and with a text file
    /// in front it leaves the clipboard alone (the key goes on).
    #[gpui::test]
    fn the_copy_action_acts_on_a_picture_only(cx: &mut TestAppContext) {
        use crate::core::actions::EditorCopyImage;

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (png, text) = (
            real_dir(&dir).join("shot.png"),
            real_dir(&dir).join("notes.txt"),
        );
        write_png(&png, 20, 10);
        std::fs::write(&text, "hello\n").unwrap();
        #[cfg(target_os = "linux")]
        standin::Standin::new(true, false).install(&app, &mut vcx);

        open_text(&app, &mut vcx, &text);
        vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("keep".into())));
        vcx.dispatch_action(EditorCopyImage);
        settle(&mut vcx);
        std::thread::sleep(Duration::from_millis(50));
        settle(&mut vcx);
        assert!(
            clipboard_png(&mut vcx).is_none(),
            "a text file copies nothing"
        );
        assert_eq!(
            vcx.update(|_, cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some("keep")
        );

        open(&app, &mut vcx, &png, 1);
        vcx.dispatch_action(EditorCopyImage);
        wait(&mut vcx, |vcx| clipboard_png(vcx).is_some());
        assert!(clipboard_png(&mut vcx).is_some(), "the picture was copied");
    }

    /// A right-click on the picture opens the menu: the focus moves into it.
    #[gpui::test]
    fn a_right_click_on_the_picture_opens_a_menu(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 300, 200);
        open(&app, &mut vcx, &png, 1);
        let at = painted(&mut vcx, "image-preview-body").center();
        click_at(&mut vcx, at);
        settle(&mut vcx);
        let before = vcx.update(|window, cx| window.focused(cx));

        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        settle(&mut vcx);
        let open = vcx.update(|window, cx| window.focused(cx));
        assert!(
            open.is_some() && open != before,
            "the right click opened a menu"
        );
    }

    fn click_at(vcx: &mut VisualTestContext, at: gpui::Point<Pixels>) {
        use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        });
        vcx.run_until_parked();
    }

    fn editor_has_focus(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| app.editor_has_focus(window, cx))
    }

    /// Opens `path`, then puts the focus in the terminal as a reader would by
    /// clicking there.
    fn open_then_focus_the_terminal(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        path: &Path,
    ) {
        open(app, vcx, path, 1);
        app.update_in(vcx, |app, window, cx| app.focus_active(window, cx));
        settle(vcx);
        assert!(!editor_has_focus(app, vcx), "the terminal has it now");
    }

    /// A click on the picture is a click into the editor: it takes the focus
    /// back from the terminal, so the panel's keys are the picture's.
    #[gpui::test]
    fn a_click_on_the_picture_takes_the_focus_from_the_terminal(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 300, 200);
        open_then_focus_the_terminal(&app, &mut vcx, &png);

        let body = painted(&mut vcx, "image-preview-body");
        click_at(&mut vcx, body.center());
        settle(&mut vcx);
        assert!(
            editor_has_focus(&app, &mut vcx),
            "the click focused the picture"
        );
    }

    /// The same for the card that says why there is no picture: a surface
    /// the panel's keys cannot reach is a dead tab.
    #[gpui::test]
    fn a_click_on_the_error_takes_the_focus_from_the_terminal(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let bad = real_dir(&dir).join("bad.png");
        std::fs::write(&bad, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        open_then_focus_the_terminal(&app, &mut vcx, &bad);

        let card = painted(&mut vcx, "image-preview-error");
        click_at(&mut vcx, card.center());
        settle(&mut vcx);
        assert!(
            editor_has_focus(&app, &mut vcx),
            "the click focused the card"
        );
    }

    /// ⌘W with a picture in front closes the picture, then the empty panel,
    /// and only then the tab: the same three presses a text file takes, and
    /// never the terminal first.
    #[gpui::test]
    fn close_with_a_picture_focused_closes_the_picture_before_the_terminal(
        cx: &mut TestAppContext,
    ) {
        use crate::core::actions::CloseActiveTab;

        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 300, 200);
        open_then_focus_the_terminal(&app, &mut vcx, &png);
        let body = painted(&mut vcx, "image-preview-body");
        click_at(&mut vcx, body.center());
        settle(&mut vcx);

        let state = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.read_with(vcx, |app, _| {
                (
                    app.tabs.len(),
                    app.tab_code().map(|c| (c.visible, c.files.len())),
                )
            })
        };
        let tab = app.read_with(&vcx, |app, _| app.tabs[0].tree_id.get());

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(
            state(&app, &mut vcx),
            (1, Some((true, 0))),
            "the picture closes first"
        );
        assert_eq!(docs(&app, &mut vcx), 0, "and is gone");

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert_eq!(
            state(&app, &mut vcx),
            (1, Some((false, 0))),
            "then the empty panel"
        );

        vcx.dispatch_action(CloseActiveTab);
        settle(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app
                .tabs
                .iter()
                .all(|t| t.tree_id.get() != tab)),
            "only now is it the terminal's"
        );
    }

    fn opened_image(path: &Path, width: u32, height: u32) -> Opened {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let decoded = image_decode::decode(&bytes, image_decode::sniff(&bytes)).expect("decodes");
        Opened::Image(ImageLoaded {
            path: path.to_path_buf(),
            decoded,
            has_alpha: false,
        })
    }

    fn session(files: &[PathBuf], visible: bool) -> TabEditor {
        TabEditor {
            files: files.iter().map(|p| p.to_path_buf()).collect(),
            active: 0,
            visible,
            split: None,
        }
    }

    /// Lands a restore of `files` for `tab` by hand, as the host's answer
    /// would, with the epoch captured when it started.
    fn land(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        tab: TabId,
        state: TabEditor,
        files: &[PathBuf],
        epoch: u64,
    ) {
        let requested: Vec<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();
        let loaded: Vec<Option<Opened>> = files
            .iter()
            .map(|p| Some(opened_image(p, 40, 20)))
            .collect();
        app.update_in(vcx, |app, window, cx| {
            let host = app.active_host(cx).expect("a host");
            app.editor_restore_land(tab, state, requested, loaded, epoch, host, window, cx);
        });
        settle(vcx);
    }

    /// The independent restore test: a picture recorded for a tab that has
    /// closed before the answer arrives is not put in another tab.
    #[gpui::test]
    fn a_restore_for_a_closed_tab_places_no_picture(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        let path = PathBuf::from("/restore/shot.png");
        let (gone, epoch) = app.read_with(&vcx, |app, _| {
            (
                app.tabs[1].tree_id.get(),
                app.panel_epoch(app.tabs[1].tree_id.get()),
            )
        });
        app.update(&mut vcx, |app, cx| {
            app.tabs.remove(1);
            cx.notify();
        });
        land(
            &app,
            &mut vcx,
            gone,
            session(std::slice::from_ref(&path), true),
            std::slice::from_ref(&path),
            epoch,
        );
        assert_eq!(
            docs(&app, &mut vcx),
            0,
            "no document for a tab that is gone"
        );
        assert!(
            app.read_with(&vcx, |app, _| app
                .tab_code()
                .is_none_or(|c| c.files.is_empty())),
            "and the tab in front is untouched"
        );
    }

    /// A tab that is still there gets its pictures, behind what is in front.
    #[gpui::test]
    fn a_restore_for_a_live_tab_installs_its_pictures_unfocused(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let (a, b) = (
            PathBuf::from("/restore/a.png"),
            PathBuf::from("/restore/b.png"),
        );
        let (tab, epoch) = app.read_with(&vcx, |app, _| {
            (
                app.tabs[0].tree_id.get(),
                app.panel_epoch(app.tabs[0].tree_id.get()),
            )
        });
        land(
            &app,
            &mut vcx,
            tab,
            session(&[a.clone(), b.clone()], true),
            &[a.clone(), b.clone()],
            epoch,
        );
        assert_eq!(docs(&app, &mut vcx), 2);
        assert!(app.read_with(&vcx, |app, _| {
            app.tab_code()
                .is_some_and(|c| c.files.len() == 2 && c.visible)
        }));
    }

    /// The reader hides the panel while the restore is still on its way: it
    /// lands its files, and does not put the panel back.
    #[gpui::test]
    fn a_user_hide_while_a_restore_is_pending_is_not_undone(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let path = PathBuf::from("/restore/shot.png");
        let (tab, epoch) = app.read_with(&vcx, |app, _| {
            (
                app.tabs[0].tree_id.get(),
                app.panel_epoch(app.tabs[0].tree_id.get()),
            )
        });
        // The reader opens the panel and closes it again.
        app.update(&mut vcx, |app, _| {
            app.tab_code_mut_or_init().unwrap().visible = true;
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_code_panel(window, cx)
        });
        assert!(
            app.read_with(&vcx, |app, _| !app.code_panel_visible()),
            "hidden"
        );
        assert_ne!(app.read_with(&vcx, |app, _| app.panel_epoch(tab)), epoch);

        land(
            &app,
            &mut vcx,
            tab,
            session(std::slice::from_ref(&path), true),
            std::slice::from_ref(&path),
            epoch,
        );
        assert_eq!(docs(&app, &mut vcx), 1, "the files are restored");
        assert!(
            app.read_with(&vcx, |app, _| app
                .tab_code()
                .is_some_and(|c| c.files.len() == 1)),
            "into the tab"
        );
        assert!(
            app.read_with(&vcx, |app, _| !app.code_panel_visible()),
            "and the panel the reader closed stays closed"
        );
    }

    /// With no hide in between, the same restore shows the panel.
    #[gpui::test]
    fn a_restore_nobody_interrupted_shows_the_panel(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let path = PathBuf::from("/restore/shot.png");
        let (tab, epoch) = app.read_with(&vcx, |app, _| {
            (
                app.tabs[0].tree_id.get(),
                app.panel_epoch(app.tabs[0].tree_id.get()),
            )
        });
        land(
            &app,
            &mut vcx,
            tab,
            session(std::slice::from_ref(&path), true),
            std::slice::from_ref(&path),
            epoch,
        );
        assert!(app.read_with(&vcx, |app, _| app.code_panel_visible()));
    }

    /// A session that was saved with the panel hidden is not shown: the
    /// guard only stops a restore from undoing a hide, it does not cancel
    /// what was recorded.
    #[gpui::test]
    fn a_session_saved_hidden_stays_hidden(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let path = PathBuf::from("/restore/shot.png");
        let (tab, epoch) = app.read_with(&vcx, |app, _| {
            (
                app.tabs[0].tree_id.get(),
                app.panel_epoch(app.tabs[0].tree_id.get()),
            )
        });
        land(
            &app,
            &mut vcx,
            tab,
            session(std::slice::from_ref(&path), false),
            std::slice::from_ref(&path),
            epoch,
        );
        assert_eq!(docs(&app, &mut vcx), 1, "its files are still restored");
        assert!(app.read_with(&vcx, |app, _| !app.code_panel_visible()));
    }

    /// One picture open in two tabs: each tab has its own viewport, closing it
    /// in one keeps it for the other, and closing it in the last releases it.
    #[gpui::test]
    fn the_same_picture_in_two_tabs_keeps_a_viewport_in_each(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shared.png");
        write_png(&png, 300, 150);

        open(&app, &mut vcx, &png, 1);
        let id = id_of(&app, &mut vcx, &png);
        let tab1 = app.read_with(&vcx, |app, _| app.tabs[1].tree_id.get());
        let width =
            |vcx: &mut VisualTestContext| painted(vcx, "image-preview-img").size.width.as_f32();
        let fitted = width(&mut vcx);
        vcx.dispatch_action(IncreaseFontSize);
        settle(&mut vcx);
        let zoomed = width(&mut vcx);
        assert!(zoomed > fitted * 1.2, "tab 0 zoomed: {fitted} -> {zoomed}");

        // The same document in tab 1, which is then the one in front.
        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.show_in_tab_strict(tab1, id, true, window, cx));
            app.active = 1;
            app.focus_editor(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        near(
            width(&mut vcx),
            fitted,
            "tab 1 starts fitted, not at tab 0's zoom",
        );
        vcx.dispatch_action(IncreaseFontSize);
        vcx.dispatch_action(IncreaseFontSize);
        settle(&mut vcx);
        let zoomed_more = width(&mut vcx);
        assert!(zoomed_more > zoomed * 1.2, "tab 1 zoomed on its own");

        // Back in tab 0, its viewport is where it was left.
        app.update_in(&mut vcx, |app, window, cx| {
            app.active = 0;
            app.focus_editor(window, cx);
            cx.notify();
        });
        settle(&mut vcx);
        near(width(&mut vcx), zoomed, "tab 0 kept its zoom");

        // Closing in tab 0 leaves the document, and its pixels, to tab 1.
        let weak = app.read_with(&vcx, |app, cx| {
            Arc::downgrade(&app.image(id).unwrap().data.read(cx).image().unwrap())
        });
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_file(0, window, cx)
        });
        settle(&mut vcx);
        assert_eq!(docs(&app, &mut vcx), 1, "tab 1 still shows it");
        assert!(weak.upgrade().is_some(), "so its pixels stay");

        // Closing it in the last tab that shows it releases it.
        app.update_in(&mut vcx, |app, window, cx| {
            app.active = 1;
            app.editor_close_file(0, window, cx);
        });
        settle(&mut vcx);
        vcx.run_until_parked();
        assert_eq!(docs(&app, &mut vcx), 0);
        assert!(weak.upgrade().is_none(), "released");
        assert!(app.read_with(&vcx, |app, _| app.editor.images.views.borrow().is_empty()));
    }

    /// The same picture in both groups' strips, front in one at a time: a
    /// group never draws through the other's view.
    #[gpui::test]
    fn the_same_picture_in_both_groups_never_shares_a_view(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (wide, tall) = (
            real_dir(&dir).join("wide.png"),
            real_dir(&dir).join("tall.png"),
        );
        write_png(&wide, 400, 200);
        write_png(&tall, 100, 400);
        open(&app, &mut vcx, &wide, 1);
        open(&app, &mut vcx, &tall, 2);
        let (a, b) = (id_of(&app, &mut vcx, &wide), id_of(&app, &mut vcx, &tall));
        let tab = app.read_with(&vcx, |app, _| app.tabs[0].tree_id.get());
        let arrange = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext, right_front: usize| {
            app.update_in(vcx, |app, window, cx| {
                let code = app.tab_code_mut().expect("an editor");
                code.files = vec![a];
                code.active = 0;
                code.split = Some(OtherGroup {
                    files: vec![b, a],
                    active: right_front,
                    focus_left: true,
                });
                app.focus_editor(window, cx);
                cx.notify();
            });
            settle(vcx);
        };

        arrange(&app, &mut vcx, 0);
        vcx.dispatch_action(IncreaseFontSize);
        settle(&mut vcx);
        let left = painted(&mut vcx, "image-preview-img").size.width.as_f32();

        // The wide picture comes to the front of the right group too: that
        // group shows the same-file note, and the left one is as it was.
        arrange(&app, &mut vcx, 1);
        assert!(
            vcx.debug_bounds("image-preview-body-1").is_none(),
            "no second view of it"
        );
        near(
            painted(&mut vcx, "image-preview-img").size.width.as_f32(),
            left,
            "left unchanged",
        );

        // And back: the right group draws its own picture, fitted by itself.
        arrange(&app, &mut vcx, 0);
        let right = painted(&mut vcx, "image-preview-img-1");
        near(
            right.size.width.as_f32() / right.size.height.as_f32(),
            0.25,
            "its own aspect",
        );
        near(
            painted(&mut vcx, "image-preview-img").size.width.as_f32(),
            left,
            "left kept its zoom",
        );
        let keys: std::collections::HashSet<ViewKey> = app.read_with(&vcx, |app, _| {
            app.editor.images.views.borrow().keys().copied().collect()
        });
        assert_eq!(keys, [(a, tab, 0), (b, tab, 1)].into_iter().collect());
    }

    /// A file renamed in the tree: the open picture follows it, and opening
    /// the new name finds that picture instead of reading a second.
    #[gpui::test]
    fn renaming_a_file_moves_an_open_picture(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (
            real_dir(&dir).join("old.png"),
            real_dir(&dir).join("new.png"),
        );
        write_png(&old, 60, 30);
        open(&app, &mut vcx, &old, 1);
        let id = id_of(&app, &mut vcx, &old);

        std::fs::rename(&old, &new).unwrap();
        app.update(&mut vcx, |app, cx| {
            let host = app.active_host(cx).expect("a host").id();
            app.editor_path_moved(host, &old, &new, cx);
        });
        settle(&mut vcx);
        app.read_with(&vcx, |app, cx| {
            let host = app.active_host(cx).expect("a host").id();
            assert_eq!(app.image_at(host, &new), Some(id));
            assert_eq!(app.image_at(host, &old), None);
            assert_eq!(app.doc_label(id).as_deref(), Some("new.png"));
            assert_eq!(app.image(id).unwrap().path, new);
            assert_eq!(app.image(id).unwrap().data.read(cx).path, new);
            assert_eq!(
                app.editor_active_location().map(|(_, p)| p),
                Some(new.as_path())
            );
        });

        // Opening the new name brings the picture forward; nothing is read.
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_file_in_editor(&new, window, cx)
        });
        settle(&mut vcx);
        assert_eq!(
            docs(&app, &mut vcx),
            1,
            "no second copy beside the stale one"
        );
        assert!(app.read_with(&vcx, |app, _| app.editor.images.requests.is_empty()));
    }

    /// What is on the clipboard as a picture: its PNG bytes.
    fn clipboard_png(vcx: &mut VisualTestContext) -> Option<Vec<u8>> {
        let item = vcx.update(|_, cx| cx.read_from_clipboard())?;
        item.entries().iter().find_map(|entry| match entry {
            gpui::ClipboardEntry::Image(image) if image.format == gpui::ImageFormat::Png => {
                Some(image.bytes.clone())
            }
            _ => None,
        })
    }

    fn decoded(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8()
    }

    fn copy(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, id: BufferId) {
        app.update_in(vcx, |app, window, cx| app.editor_copy_image(id, window, cx));
    }

    /// The system tools are never started by a test: this stands in for them,
    /// records what it was asked to export, and keeps an export open until it
    /// is released.
    #[cfg(target_os = "linux")]
    mod standin {
        use std::io;
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        use crate::ui::clipboard_image::{ClipboardExport, ExportBackend, ExportJob};

        pub(super) struct Standin {
            pub(super) asked: Mutex<Vec<PathBuf>>,
            held: Arc<AtomicBool>,
            has_tool: bool,
        }

        struct Job(Arc<AtomicBool>);

        impl ExportJob for Job {
            fn finished(&mut self) -> bool {
                !self.0.load(Ordering::SeqCst)
            }
        }

        impl ExportBackend for Standin {
            fn start(&self, file: &Path) -> io::Result<Option<Box<dyn ExportJob>>> {
                self.asked.lock().unwrap().push(file.to_path_buf());
                Ok(self
                    .has_tool
                    .then(|| Box::new(Job(self.held.clone())) as Box<dyn ExportJob>))
            }
        }

        impl Standin {
            /// A tool whose exports end at once, or stay open while `held`.
            pub(super) fn new(has_tool: bool, held: bool) -> Arc<Self> {
                Arc::new(Self {
                    asked: Mutex::default(),
                    held: Arc::new(AtomicBool::new(held)),
                    has_tool,
                })
            }

            pub(super) fn release(&self) {
                self.held.store(false, Ordering::SeqCst);
            }

            pub(super) fn asked(&self) -> Vec<PathBuf> {
                self.asked.lock().unwrap().clone()
            }

            pub(super) fn install(
                self: &Arc<Self>,
                app: &gpui::Entity<crate::ui::app::Tty59App>,
                vcx: &mut gpui::VisualTestContext,
            ) {
                let backend = self.clone();
                app.update(vcx, |app, _| {
                    app.editor.images.export =
                        ClipboardExport::with_backend(backend, Arc::default());
                });
            }
        }
    }

    /// Copy puts the picture on the clipboard as a PNG of the same size and
    /// colour, and on Linux hands the staged file to the export.
    #[gpui::test]
    fn copying_a_picture_puts_its_png_on_the_clipboard(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 40, 20);
        open(&app, &mut vcx, &png, 1);
        let id = id_of(&app, &mut vcx, &png);
        #[cfg(target_os = "linux")]
        let tool = {
            let tool = standin::Standin::new(true, false);
            tool.install(&app, &mut vcx);
            tool
        };

        copy(&app, &mut vcx, id);
        wait(&mut vcx, |vcx| {
            #[cfg(target_os = "linux")]
            let exported = !tool.asked().is_empty();
            #[cfg(not(target_os = "linux"))]
            let exported = true;
            clipboard_png(vcx).is_some() && exported
        });

        let bytes = clipboard_png(&mut vcx).expect("a picture is on the clipboard");
        let back = decoded(&bytes);
        assert_eq!(back.dimensions(), (40, 20));
        assert_eq!(back.get_pixel(7, 3).0, [40, 90, 200, 255]);
        #[cfg(target_os = "linux")]
        {
            let asked = tool.asked();
            assert_eq!(asked.len(), 1, "one export");
            assert_eq!(
                asked[0].parent(),
                Some(std::env::temp_dir().join("tty59-clipboard").as_path()),
                "staged where a terminal paste stages its pictures"
            );
            assert_eq!(std::fs::read(&asked[0]).unwrap(), bytes, "the same PNG");
        }
    }

    /// A picture that did not decode has nothing to copy: nothing is written
    /// and nothing is exported. The same steps with a picture that did decode
    /// are the control.
    #[gpui::test]
    fn a_failed_picture_copies_nothing(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let (good, bad) = (
            real_dir(&dir).join("good.png"),
            real_dir(&dir).join("bad.png"),
        );
        write_png(&good, 8, 8);
        std::fs::write(&bad, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        open(&app, &mut vcx, &good, 1);
        open(&app, &mut vcx, &bad, 2);
        let (ok, broken) = (id_of(&app, &mut vcx, &good), id_of(&app, &mut vcx, &bad));
        #[cfg(target_os = "linux")]
        let tool = {
            let tool = standin::Standin::new(true, false);
            tool.install(&app, &mut vcx);
            tool
        };

        copy(&app, &mut vcx, ok);
        wait(&mut vcx, |vcx| clipboard_png(vcx).is_some());
        assert!(clipboard_png(&mut vcx).is_some(), "the control copies");
        #[cfg(target_os = "linux")]
        wait(&mut vcx, |_| tool.asked().len() == 1);

        vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("keep".into())));
        copy(&app, &mut vcx, broken);
        settle(&mut vcx);
        std::thread::sleep(Duration::from_millis(50));
        settle(&mut vcx);
        assert!(clipboard_png(&mut vcx).is_none(), "nothing was written");
        assert_eq!(
            vcx.update(|_, cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some("keep")
        );
        #[cfg(target_os = "linux")]
        assert_eq!(tool.asked().len(), 1, "nothing was exported");
    }

    /// An SVG goes as the raster the viewer holds, not as its own size.
    #[gpui::test]
    fn an_svg_copies_its_raster(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let svg = real_dir(&dir).join("logo.svg");
        std::fs::write(
            &svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="30" height="10"><rect width="30" height="10" fill="red"/></svg>"#,
        )
        .unwrap();
        open(&app, &mut vcx, &svg, 1);
        let id = id_of(&app, &mut vcx, &svg);
        let raster = app.read_with(&vcx, |app, cx| {
            match &app.image(id).unwrap().data.read(cx).content {
                Content::Ready(shown) => shown.pixels,
                Content::Failed(_) => panic!("the SVG should decode"),
            }
        });
        #[cfg(target_os = "linux")]
        standin::Standin::new(true, false).install(&app, &mut vcx);

        copy(&app, &mut vcx, id);
        wait(&mut vcx, |vcx| clipboard_png(vcx).is_some());
        let back = decoded(&clipboard_png(&mut vcx).expect("a picture"));
        assert_eq!(back.dimensions(), raster);
        assert_eq!(back.get_pixel(1, 1).0, [255, 0, 0, 255]);
    }

    /// Advances the executor's clock a little at a time until `done`.
    #[cfg(target_os = "linux")]
    fn run_clock(vcx: &mut VisualTestContext, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() && Instant::now() < deadline {
            vcx.update(|_, cx| {
                cx.background_executor()
                    .advance_clock(Duration::from_millis(60))
            });
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Never two tools at once: while one export is open a newer copy waits
    /// in one slot, a still newer one replaces it, and the one that is left
    /// goes when the open one ends.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn one_export_runs_at_a_time_and_the_latest_copy_wins(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let shots: Vec<PathBuf> = [20, 21, 22]
            .iter()
            .map(|w| {
                let path = real_dir(&dir).join(format!("shot-{w}.png"));
                write_png(&path, *w, 20);
                path
            })
            .collect();
        for (n, path) in shots.iter().enumerate() {
            open(&app, &mut vcx, path, n + 1);
        }
        let ids: Vec<BufferId> = shots.iter().map(|p| id_of(&app, &mut vcx, p)).collect();
        let tool = standin::Standin::new(true, true);
        tool.install(&app, &mut vcx);

        copy(&app, &mut vcx, ids[0]);
        wait(&mut vcx, |_| tool.asked().len() == 1);
        // Two more while the first is open: nothing is started for them.
        copy(&app, &mut vcx, ids[1]);
        wait(&mut vcx, |vcx| {
            clipboard_png(vcx).is_some_and(|p| decoded(&p).width() == 21)
        });
        copy(&app, &mut vcx, ids[2]);
        wait(&mut vcx, |vcx| {
            clipboard_png(vcx).is_some_and(|p| decoded(&p).width() == 22)
        });
        std::thread::sleep(Duration::from_millis(50));
        settle(&mut vcx);
        assert_eq!(tool.asked().len(), 1, "one tool at a time");

        // The open one ends; the newest copy is exported, the middle one never.
        tool.release();
        run_clock(&mut vcx, || tool.asked().len() >= 2);
        run_clock(&mut vcx, || false);
        let asked = tool.asked();
        assert_eq!(
            asked.len(),
            2,
            "the replaced copy is never exported: {asked:?}"
        );
        let width = |path: &Path| decoded(&std::fs::read(path).unwrap()).width();
        assert_eq!((width(&asked[0]), width(&asked[1])), (20, 22));
    }

    /// With no tool the copy still happens in the app, and the reader is told
    /// once, not on every copy.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn no_tool_says_so_once(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let png = real_dir(&dir).join("shot.png");
        write_png(&png, 10, 10);
        open(&app, &mut vcx, &png, 1);
        let id = id_of(&app, &mut vcx, &png);
        let tool = standin::Standin::new(false, false);
        tool.install(&app, &mut vcx);
        let notices =
            |vcx: &mut VisualTestContext| vcx.update(|window, cx| window.notifications(cx).len());
        let before = notices(&mut vcx);

        copy(&app, &mut vcx, id);
        wait(&mut vcx, |_| tool.asked().len() == 1);
        settle(&mut vcx);
        assert!(
            clipboard_png(&mut vcx).is_some(),
            "the in-app copy happened"
        );
        assert_eq!(notices(&mut vcx), before + 1, "told once");

        copy(&app, &mut vcx, id);
        wait(&mut vcx, |_| tool.asked().len() == 2);
        settle(&mut vcx);
        assert_eq!(notices(&mut vcx), before + 1, "and not again");
    }

    /// A folder renamed: every picture under it follows, a folder whose name
    /// merely begins the same does not, and another machine's paths are not
    /// this machine's.
    #[gpui::test]
    fn renaming_a_folder_moves_the_pictures_under_it_and_only_them(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        let root = real_dir(&dir);
        let (d, d2, e) = (root.join("d"), root.join("d2"), root.join("e"));
        for folder in [&d, &d2] {
            std::fs::create_dir(folder).unwrap();
        }
        let (x, y, z) = (d.join("x.png"), d.join("y.png"), d2.join("z.png"));
        for path in [&x, &y, &z] {
            write_png(path, 20, 20);
        }
        open(&app, &mut vcx, &x, 1);
        open(&app, &mut vcx, &y, 2);
        open(&app, &mut vcx, &z, 3);

        // Another machine's /d is not this machine's /d.
        app.update(&mut vcx, |app, cx| {
            let other = HostId::from_connection_key("some-other-host");
            app.editor_path_moved(other, &d, &e, cx);
        });
        let here = app.read_with(&vcx, |app, cx| app.active_host(cx).expect("a host").id());
        assert!(app.read_with(&vcx, |app, _| app.image_at(here, &x).is_some()));

        std::fs::rename(&d, &e).unwrap();
        app.update(&mut vcx, |app, cx| app.editor_path_moved(here, &d, &e, cx));
        settle(&mut vcx);
        app.read_with(&vcx, |app, _| {
            assert!(app.image_at(here, &e.join("x.png")).is_some());
            assert!(app.image_at(here, &e.join("y.png")).is_some());
            assert!(app.image_at(here, &x).is_none() && app.image_at(here, &y).is_none());
            assert!(app.image_at(here, &z).is_some(), "d2 only begins like d");
        });
    }
}
