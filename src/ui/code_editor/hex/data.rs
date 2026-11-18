//! The hex document: the bytes of one file as the model holds them, plus where the file is.
//! The entity is the document; what is shown of it, and where the caret is looking, belongs to
//! a [`super::view::HexView`], so a file open in two groups shares its pages and its edits.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{Context, SharedString, Window};
use tty59_core::host::{MTime, SharedHost};

use super::io::{self, HexLoaded, HexSource};
use super::model::{HexModel, Source};
use crate::ui::code_editor::DiskConflict;
use crate::ui::host_ops::HostOps;

/// What the document holds.
pub(super) enum HexContent {
    Ready(Box<HexModel>),
    /// Remote file over the transfer limit (D2): the tab shows an error card.
    TooLarge {
        len: u64,
        limit: u64,
    },
}

/// One file open as bytes.
pub(crate) struct HexData {
    /// Canonical, as the host answered.
    pub(crate) path: PathBuf,
    pub(crate) host: SharedHost,
    // `HexModel` is private to the hex module, so the field is too; the rest of the editor asks
    // through the methods below.
    pub(super) content: HexContent,
    /// The disk mtime the model was read at, and a save's check that the file is still that one.
    pub(crate) mtime: Option<MTime>,
    pub(crate) opens_as_text: bool,
    /// Pages being read, with the generation they were requested under: a reload makes the
    /// same page number a new request, so an older read must not hold it back.
    in_flight: BTreeSet<(u64, u64)>,
    /// A save is on its way. Like a text buffer's: a second save waits for it, and an external
    /// change seen meanwhile is the save's own.
    pub(super) saving: bool,
    pub(super) save_pending: bool,
    pub(super) save_then_close: bool,
    /// Why the document and the file no longer agree, for the banner.
    pub(super) conflict: Option<DiskConflict>,
    /// Counts reloads, so the answer of one that has since been superseded is dropped.
    pub(super) reload_seq: u64,
}

impl HexData {
    pub(super) fn from_loaded(host: SharedHost, loaded: HexLoaded) -> Self {
        let (path, content, mtime, opens_as_text) = match loaded {
            HexLoaded::File(file) => {
                let model = match file.source {
                    HexSource::Paged { len, first_page } => {
                        let mut model = HexModel::new(Source::Paged { len });
                        let generation = model.store.generation();
                        // An empty file has no first page; the store refuses it.
                        model.insert_page(generation, 0, first_page);
                        model
                    }
                    HexSource::Whole(bytes) => HexModel::new(Source::Whole(bytes)),
                };
                (
                    file.path,
                    HexContent::Ready(Box::new(model)),
                    file.mtime,
                    file.opens_as_text,
                )
            }
            HexLoaded::TooLarge { path, len, limit } => {
                (path, HexContent::TooLarge { len, limit }, None, false)
            }
        };
        Self {
            path,
            host,
            content,
            mtime,
            opens_as_text,
            in_flight: BTreeSet::new(),
            saving: false,
            save_pending: false,
            save_then_close: false,
            conflict: None,
            reload_seq: 0,
        }
    }

    /// The file name, for the strip.
    pub(crate) fn name(&self) -> SharedString {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
            .into()
    }

    /// Which reading of the file the cached pages belong to; 0 for one never read.
    pub(crate) fn generation(&self) -> u64 {
        match &self.content {
            HexContent::Ready(model) => model.store.generation(),
            HexContent::TooLarge { .. } => 0,
        }
    }

    /// `None` for a file that is too large to open.
    pub(crate) fn len(&self) -> Option<u64> {
        match &self.content {
            HexContent::Ready(model) => Some(model.store.len()),
            HexContent::TooLarge { .. } => None,
        }
    }

    /// The size and the limit of a file too large to open, for the error card.
    pub(crate) fn too_large(&self) -> Option<(u64, u64)> {
        match &self.content {
            HexContent::Ready(_) => None,
            HexContent::TooLarge { len, limit } => Some((*len, *limit)),
        }
    }

    pub(crate) fn dirty(&self) -> bool {
        match &self.content {
            HexContent::Ready(model) => model.store.dirty(),
            HexContent::TooLarge { .. } => false,
        }
    }

    /// Reads the pages under `rows` that are not cached and not already being read, off the UI
    /// thread, and notifies when they land. Called from handlers, never from render.
    pub(crate) fn ensure_rows(
        &mut self,
        rows: Range<u64>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let HexContent::Ready(model) = &self.content else {
            return;
        };
        let generation = model.store.generation();
        let wanted: Vec<u64> = model
            .store
            .missing_pages(rows)
            .into_iter()
            .filter(|idx| !self.in_flight.contains(&(generation, *idx)))
            .collect();
        if wanted.is_empty() {
            return;
        }
        self.in_flight
            .extend(wanted.iter().map(|idx| (generation, *idx)));
        let path = self.path.clone();
        HostOps::run_in(
            self.host.clone(),
            window,
            cx,
            move |h| io::read_pages(h, &path, wanted),
            move |this, pages, _window, cx| this.land(generation, pages, cx),
        );
    }

    /// Pages arrived. A failed one stays pending and is asked for again only when the user
    /// scrolls or moves there again, not in a loop.
    fn land(
        &mut self,
        generation: u64,
        pages: Vec<(u64, std::io::Result<Vec<u8>>)>,
        cx: &mut Context<Self>,
    ) {
        for (idx, page) in pages {
            self.in_flight.remove(&(generation, idx));
            match page {
                Ok(bytes) => {
                    if let HexContent::Ready(model) = &mut self.content {
                        // A page from before a reload is refused by the store itself.
                        model.insert_page(generation, idx, bytes);
                    }
                }
                Err(e) => log::warn!(
                    "hex: could not read page {idx} of {}: {e}",
                    self.path.display()
                ),
            }
        }
        cx.notify();
    }

    /// Whether a page read is outstanding; the tests wait on it.
    #[cfg(test)]
    pub(super) fn reading(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// The path moved (a rename through tty59): nothing is re-read.
    pub(crate) fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = path;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use gpui::prelude::*;
    use gpui::{Entity, TestAppContext};

    use super::super::test_support::{Counting, paged, wait};
    use super::*;
    use crate::ui::code_editor::hex::model::{Byte, PAGE};

    const PAGE_ROWS: u64 = (PAGE / 16) as u64;

    fn byte(data: &HexData, off: u64) -> Byte {
        match &data.content {
            HexContent::Ready(m) => m.store.byte(off),
            HexContent::TooLarge { .. } => panic!("too large"),
        }
    }

    /// Waits until every page read has landed.
    fn landed(data: &Entity<HexData>, vcx: &mut gpui::VisualTestContext) {
        wait(vcx, |vcx| data.read_with(vcx, |d, _| !d.reading()));
    }

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    /// Opens a window around `data` so `ensure_rows` has one to run in.
    fn with_window(
        cx: &mut TestAppContext,
        data: HexData,
    ) -> (Entity<HexData>, &mut gpui::VisualTestContext) {
        let (_, vcx) = cx.add_window_view(|_, _| Empty);
        let entity = vcx.update(|_, cx| cx.new(|_| data));
        (entity, vcx)
    }

    /// Rows three pages in want only that page: the cached first page is not read again, the
    /// pages in between are not read at all. Guards `ensure_rows` using `missing_pages`.
    #[gpui::test]
    fn rows_ask_only_for_the_pages_they_touch(cx: &mut TestAppContext) {
        let host = Counting::new();
        let (data, _dir) = paged(PAGE as u64 * 8, &[1, 2, 3], &host);
        let (data, vcx) = with_window(cx, data);
        data.update_in(vcx, |d, window, cx| {
            d.ensure_rows(0..4, window, cx);
            d.ensure_rows(3 * PAGE_ROWS..3 * PAGE_ROWS + 4, window, cx);
        });
        landed(&data, vcx);
        assert_eq!(host.reads(), vec![3 * PAGE as u64]);
        data.read_with(vcx, |d, _| {
            assert!(matches!(byte(d, 3 * PAGE as u64), Byte::Loaded { .. }));
            assert_eq!(byte(d, 5 * PAGE as u64), Byte::Pending);
        });
    }

    /// A page already on its way is not asked for again. Guards the in-flight set.
    #[gpui::test]
    fn a_page_in_flight_is_not_requested_twice(cx: &mut TestAppContext) {
        let host = Counting::new();
        let (data, _dir) = paged(PAGE as u64 * 4, &[], &host);
        let (data, vcx) = with_window(cx, data);
        data.update_in(vcx, |d, window, cx| {
            d.ensure_rows(2 * PAGE_ROWS..2 * PAGE_ROWS + 1, window, cx);
            d.ensure_rows(2 * PAGE_ROWS..2 * PAGE_ROWS + 1, window, cx);
        });
        landed(&data, vcx);
        assert_eq!(host.reads().len(), 1);
    }

    /// A failed read leaves the page pending, asks nothing more on its own and does not
    /// panic. Guards the `Err` arm of `land`.
    #[gpui::test]
    fn a_failed_page_stays_pending_and_is_not_retried_in_a_loop(cx: &mut TestAppContext) {
        let host = Counting::new();
        host.fail.store(true, Ordering::SeqCst);
        let (data, _dir) = paged(PAGE as u64 * 4, &[], &host);
        let (data, vcx) = with_window(cx, data);
        data.update_in(vcx, |d, window, cx| {
            d.ensure_rows(PAGE_ROWS..PAGE_ROWS + 1, window, cx)
        });
        landed(&data, vcx);
        assert_eq!(host.reads().len(), 1, "asked once");
        data.read_with(vcx, |d, _| {
            assert_eq!(byte(d, PAGE as u64), Byte::Pending);
            assert!(
                d.in_flight.is_empty(),
                "the mark is dropped so a later ask works"
            );
        });
        host.fail.store(false, Ordering::SeqCst);
        data.update_in(vcx, |d, window, cx| {
            d.ensure_rows(PAGE_ROWS..PAGE_ROWS + 1, window, cx)
        });
        landed(&data, vcx);
        data.read_with(vcx, |d, _| {
            assert!(matches!(byte(d, PAGE as u64), Byte::Loaded { .. }));
        });
    }

    /// A page read for a generation the file has since left is dropped. Guards passing the
    /// generation captured at request time to `insert_page`.
    #[gpui::test]
    fn a_page_that_lands_after_a_reload_is_dropped(cx: &mut TestAppContext) {
        let host = Counting::new();
        let (data, _dir) = paged(PAGE as u64 * 4, &[], &host);
        let (data, vcx) = with_window(cx, data);
        data.update_in(vcx, |d, window, cx| {
            d.ensure_rows(PAGE_ROWS..PAGE_ROWS + 1, window, cx);
            if let HexContent::Ready(m) = &mut d.content {
                m.store.bump_generation();
            }
        });
        landed(&data, vcx);
        data.read_with(vcx, |d, _| assert_eq!(byte(d, PAGE as u64), Byte::Pending));
    }

    /// A whole (remote) document has no pages to ask for. Guards the `Whole` early-out of
    /// `missing_pages` reaching `ensure_rows`.
    #[gpui::test]
    fn a_whole_document_reads_nothing(cx: &mut TestAppContext) {
        let host = Counting::new();
        let loaded = HexLoaded::File(io::HexFile {
            path: PathBuf::from("/x/a.bin"),
            source: HexSource::Whole(Arc::from(vec![7u8; 100_000])),
            mtime: None,
            len: 100_000,
            opens_as_text: false,
        });
        let (data, vcx) = with_window(cx, HexData::from_loaded(host.clone(), loaded));
        data.update_in(vcx, |d, window, cx| d.ensure_rows(0..6_250, window, cx));
        landed(&data, vcx);
        assert!(host.reads().is_empty());
        data.read_with(vcx, |d, _| {
            assert_eq!(d.len(), Some(100_000));
            assert!(matches!(byte(d, 99_999), Byte::Loaded { value: 7, .. }));
        });
    }

    #[gpui::test]
    fn a_too_large_document_has_no_length_and_nothing_to_read(cx: &mut TestAppContext) {
        let host = Counting::new();
        let loaded = HexLoaded::TooLarge {
            path: PathBuf::from("/x/big.bin"),
            len: 10,
            limit: 5,
        };
        let (data, vcx) = with_window(cx, HexData::from_loaded(host.clone(), loaded));
        data.update_in(vcx, |d, window, cx| d.ensure_rows(0..10, window, cx));
        landed(&data, vcx);
        assert!(host.reads().is_empty());
        data.read_with(vcx, |d, _| {
            assert_eq!(d.len(), None);
            assert_eq!(d.too_large(), Some((10, 5)));
            assert!(!d.dirty());
            assert_eq!(d.name().as_ref(), "big.bin");
        });
    }
}
