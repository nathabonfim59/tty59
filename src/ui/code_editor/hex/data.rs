//! The hex document: the bytes of one file as the model holds them, plus where the file is.
//! The entity is the document; what is shown of it, and where the caret is looking, belongs to
//! a [`super::view::HexView`], so a file open in two groups shares its pages and its edits.

// Used once the editor opens hex tabs (WP2.1); removed then.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{Context, SharedString, Window};
use tty59_core::host::{MTime, SharedHost};

use super::io::{self, HexLoaded, HexSource};
use super::model::{HexModel, Source};
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
    /// The disk mtime the model was read at.
    pub(crate) mtime: Option<MTime>,
    pub(crate) opens_as_text: bool,
    /// Pages being read, with the generation they were requested under: a reload makes the
    /// same page number a new request, so an older read must not hold it back.
    in_flight: BTreeSet<(u64, u64)>,
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

    /// The path moved (a rename through tty59): nothing is re-read.
    pub(crate) fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.path = path;
        cx.notify();
    }
}

/// A host that counts the ranged reads it is asked for, shared by the data and view tests.
#[cfg(test)]
pub(in crate::ui::code_editor::hex) mod test_support {
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use tty59_core::host::local::LocalHost;
    use tty59_core::host::{
        ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
        ShellInventory, WatchSub,
    };

    use super::*;
    use crate::ui::code_editor::hex::io::HexFile;
    use crate::ui::code_editor::hex::model::PAGE;

    pub(in crate::ui::code_editor::hex) struct Counting {
        inner: SharedHost,
        /// Offsets of every ranged read, in order.
        pub(in crate::ui::code_editor::hex) reads: Mutex<Vec<u64>>,
        /// Makes every ranged read fail.
        pub(in crate::ui::code_editor::hex) fail: AtomicBool,
    }

    impl Counting {
        pub(in crate::ui::code_editor::hex) fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: LocalHost::new(),
                reads: Mutex::new(Vec::new()),
                fail: AtomicBool::new(false),
            })
        }

        pub(in crate::ui::code_editor::hex) fn reads(&self) -> Vec<u64> {
            self.reads.lock().map(|r| r.clone()).unwrap_or_default()
        }
    }

    impl Host for Counting {
        fn id(&self) -> HostId {
            HostId::LOCAL
        }
        fn separator(&self) -> char {
            self.inner.separator()
        }
        fn is_absolute(&self, p: &Path) -> bool {
            self.inner.is_absolute(p)
        }
        fn read_dir(&self, dir: &Path, root: Option<&Path>) -> io::Result<Vec<Entry>> {
            self.inner.read_dir(dir, root)
        }
        fn stat(&self, p: &Path) -> io::Result<Meta> {
            self.inner.stat(p)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.inner.read_file(p, max_bytes)
        }
        fn read_range(&self, p: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(offset);
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(io::Error::other("injected"));
            }
            self.inner.read_range(p, offset, len)
        }
        fn canonicalize(&self, p: &Path) -> io::Result<PathBuf> {
            self.inner.canonicalize(p)
        }
        fn search(
            &self,
            roots: &[PathBuf],
            query: &str,
            limit: usize,
            max_dirs: usize,
            show_hidden: bool,
        ) -> io::Result<Vec<SearchHit>> {
            self.inner
                .search(roots, query, limit, max_dirs, show_hidden)
        }
        fn search_content(
            &self,
            roots: &[PathBuf],
            query: &ContentQuery,
            limits: &ContentLimits,
        ) -> io::Result<ContentResults> {
            self.inner.search_content(roots, query, limits)
        }
        fn write_file(&self, p: &Path, bytes: &[u8]) -> io::Result<Meta> {
            self.inner.write_file(p, bytes)
        }
        fn create_file_new(&self, p: &Path) -> io::Result<()> {
            self.inner.create_file_new(p)
        }
        fn create_dir(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.create_dir(p, recursive)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.inner.rename(from, to)
        }
        fn remove(&self, p: &Path, recursive: bool) -> io::Result<()> {
            self.inner.remove(p, recursive)
        }
        fn repo_root(&self, p: &Path) -> io::Result<Option<PathBuf>> {
            self.inner.repo_root(p)
        }
        fn git(&self, cwd: &Path, args: &[&str]) -> io::Result<Output> {
            self.inner.git(cwd, args)
        }
        fn shells(&self) -> io::Result<ShellInventory> {
            self.inner.shells()
        }
        fn watch(&self, dirs: &[PathBuf]) -> io::Result<WatchSub> {
            self.inner.watch(dirs)
        }
    }

    /// A paged document over a real file of `len` bytes whose start is `head` (the rest reads
    /// as zeros, so a sparse file of any size will do). Returns the file's directory too, which
    /// must outlive the document.
    pub(in crate::ui::code_editor::hex) fn paged(
        len: u64,
        head: &[u8],
        host: &Arc<Counting>,
    ) -> (HexData, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("a temp dir");
        let path = dir.path().join("f.bin");
        std::fs::write(&path, head).expect("the head");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_len(len))
            .expect("the length");
        let mut first_page = vec![0u8; usize::try_from(len).unwrap_or(PAGE).min(PAGE)];
        first_page[..head.len().min(PAGE)].copy_from_slice(&head[..head.len().min(PAGE)]);
        let loaded = HexLoaded::File(HexFile {
            path,
            source: HexSource::Paged { len, first_page },
            mtime: None,
            len,
            opens_as_text: false,
        });
        (HexData::from_loaded(host.clone(), loaded), dir)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use gpui::prelude::*;
    use gpui::{Entity, TestAppContext};

    use super::test_support::{Counting, paged};
    use super::*;
    use crate::ui::code_editor::hex::model::{Byte, PAGE};

    const PAGE_ROWS: u64 = (PAGE / 16) as u64;

    fn byte(data: &HexData, off: u64) -> Byte {
        match &data.content {
            HexContent::Ready(m) => m.store.byte(off),
            HexContent::TooLarge { .. } => panic!("too large"),
        }
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
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
        vcx.run_until_parked();
        assert!(host.reads().is_empty());
        data.read_with(vcx, |d, _| {
            assert_eq!(d.len(), None);
            assert_eq!(d.too_large(), Some((10, 5)));
            assert!(!d.dirty());
            assert_eq!(d.name().as_ref(), "big.bin");
        });
    }
}
