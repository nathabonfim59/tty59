//! Test doubles shared by the hex data and view tests: a host that counts ranged reads, a
//! paged document over a real (sparse) file, and a wait for page reads, which run on a real
//! background thread. It lives in its own file, with the fixtures it writes to a local temp
//! dir, like the other test-only files `check-host-boundary.sh` knows about.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::VisualTestContext;
use tty59_core::host::local::LocalHost;
use tty59_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Host, HostId, Meta, Output, SearchHit,
    SharedHost, ShellInventory, WatchSub,
};

use super::data::HexData;
use super::io::{HexFile, HexLoaded, HexSource};
use super::model::PAGE;

pub(super) struct Counting {
    inner: SharedHost,
    /// Offsets of every ranged read, in order.
    pub(super) reads: Mutex<Vec<u64>>,
    /// Makes every ranged read fail.
    pub(super) fail: AtomicBool,
}

impl Counting {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: LocalHost::new(),
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }

    pub(super) fn reads(&self) -> Vec<u64> {
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
pub(super) fn paged(len: u64, head: &[u8], host: &Arc<Counting>) -> (HexData, tempfile::TempDir) {
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
        opens_as_text: false,
    });
    (HexData::from_loaded(host.clone(), loaded), dir)
}

/// Runs the host's blocking work until `done` holds, bounded at ten seconds. Page reads go
/// through `HostOps::run_in`, which uses a real thread, so the test scheduler must be allowed
/// to park and the wait polls instead of assuming `run_until_parked` is enough.
pub(super) fn wait(vcx: &mut VisualTestContext, done: impl Fn(&mut VisualTestContext) -> bool) {
    vcx.executor().allow_parking();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(vcx) && Instant::now() < deadline {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    vcx.run_until_parked();
}
