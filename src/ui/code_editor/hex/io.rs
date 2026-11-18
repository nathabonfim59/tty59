//! The host's half of a hex document: reading it in, one page at a time, and
//! writing it back.
//!
//! Everything here runs on the host's own thread (inside `HostOps::run_in`),
//! never in a frame, and speaks only to the [`Host`] trait, so a file on
//! another machine takes the same paths with a different answer: a local file
//! is *paged* (its size and first page now, the rest as it is scrolled to),
//! while a file the host cannot read in parts is read whole, once, up to what
//! the connection can carry.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tty59_core::host::{Changed, Expect, Host, MTime};

use super::model::PAGE;
use crate::ui::code_editor::{EditorOpenError, MAX_FILE_BYTES, SaveOutcome};
use crate::ui::editor_text;
use crate::ui::file_copy::REMOTE_FILE_MAX;
use crate::ui::i18n::{L10nKey, t_fmt};
use crate::ui::image_preview::decode as image_decode;

/// [`PAGE`] as a file offset.
const PAGE_BYTES: u64 = PAGE as u64;

/// Where a document's bytes come from, in plain data: the model's own `Source`
/// is built from this where the two meet.
pub(in crate::ui::code_editor) enum HexSource {
    /// A host that reads in parts: the size, and the first page so the view
    /// has something to draw before any further read answers.
    Paged { len: u64, first_page: Vec<u8> },
    /// Everything, read once.
    Whole(Arc<[u8]>),
}

/// A file read for the hex view.
pub(in crate::ui::code_editor) struct HexFile {
    pub(in crate::ui::code_editor) path: PathBuf,
    pub(in crate::ui::code_editor) source: HexSource,
    pub(in crate::ui::code_editor) mtime: Option<MTime>,
    #[cfg_attr(not(test), expect(dead_code, reason = "used by M3 save"))]
    pub(in crate::ui::code_editor) len: u64,
    /// Whether the text editor would have taken this file, which decides
    /// whether the tab offers "Open as Text".
    pub(in crate::ui::code_editor) opens_as_text: bool,
}

/// What loading a file for the hex view came to.
pub(in crate::ui::code_editor) enum HexLoaded {
    File(HexFile),
    /// A file a host that cannot read in parts has no way to bring over: the
    /// tab says so and names `limit`, rather than showing nothing.
    TooLarge {
        path: PathBuf,
        len: u64,
        limit: u64,
    },
}

fn cant_open(path: &Path, e: &io::Error) -> EditorOpenError {
    EditorOpenError::Message(t_fmt(
        L10nKey::EditorCantOpen,
        &[("path", &path.display().to_string()), ("e", &e.to_string())],
    ))
}

fn cant_read(path: &Path, e: &io::Error) -> EditorOpenError {
    EditorOpenError::Message(t_fmt(
        L10nKey::EditorCantRead,
        &[("path", &path.display().to_string()), ("e", &e.to_string())],
    ))
}

fn len_of(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Whether the text editor would take these bytes: the rules of
/// `load_document`, in its order. A picture is sniffed first because an SVG is
/// valid text and would otherwise count.
fn would_open_as_text(bytes: &[u8]) -> bool {
    len_of(bytes) <= MAX_FILE_BYTES
        && matches!(image_decode::sniff(bytes), image_decode::Sniffed::NotImage)
        && editor_text::decode(bytes).is_some()
}

/// [`would_open_as_text`] for a file that is not in memory. Past the text
/// limit the answer is no and nothing is read; below it the file is read once
/// for the decision, which is at most [`MAX_FILE_BYTES`].
fn paged_opens_as_text(h: &dyn Host, path: &Path, len: u64) -> Result<bool, EditorOpenError> {
    if len > MAX_FILE_BYTES {
        return Ok(false);
    }
    match h.read_file(path, MAX_FILE_BYTES) {
        Ok(bytes) => Ok(would_open_as_text(&bytes)),
        // It grew past the limit since it was measured: that is a no, not a
        // failure to open.
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => Ok(false),
        Err(e) => Err(cant_read(path, &e)),
    }
}

/// Reads `requested` for the hex view.
///
/// A local host is paged. Any other, and a local one that answers
/// `Unsupported` to a ranged read, is read whole in a single request, so what
/// the connection can carry bounds it: over [`REMOTE_FILE_MAX`], by its size
/// or by the read itself refusing, is [`HexLoaded::TooLarge`].
pub(in crate::ui::code_editor) fn load_hex(
    h: &dyn Host,
    requested: PathBuf,
) -> Result<HexLoaded, EditorOpenError> {
    let path = h.canonicalize(&requested).unwrap_or(requested);
    let meta = h.stat(&path).map_err(|e| cant_open(&path, &e))?;

    if h.id().is_local() {
        match h.read_range(&path, 0, PAGE) {
            Ok(first_page) => {
                let opens_as_text = paged_opens_as_text(h, &path, meta.len)?;
                return Ok(HexLoaded::File(HexFile {
                    path,
                    source: HexSource::Paged {
                        len: meta.len,
                        first_page,
                    },
                    mtime: meta.mtime,
                    len: meta.len,
                    opens_as_text,
                }));
            }
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            Err(e) => return Err(cant_read(&path, &e)),
        }
    }

    let too_large = |path: PathBuf| HexLoaded::TooLarge {
        path,
        len: meta.len,
        limit: REMOTE_FILE_MAX,
    };
    if meta.len > REMOTE_FILE_MAX {
        return Ok(too_large(path));
    }
    match h.read_file(&path, REMOTE_FILE_MAX) {
        Ok(bytes) => {
            let len = len_of(&bytes);
            let opens_as_text = would_open_as_text(&bytes);
            Ok(HexLoaded::File(HexFile {
                path,
                source: HexSource::Whole(Arc::from(bytes)),
                mtime: meta.mtime,
                len,
                opens_as_text,
            }))
        }
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => Ok(too_large(path)),
        Err(e) => Err(cant_read(&path, &e)),
    }
}

/// The default-open path: `load_document` already read `bytes` and found them
/// not to be text, so the hex view is made from those and the file is not read
/// again. `local` says whether the host reads in parts, as [`load_hex`] asks
/// of the host itself.
///
/// The bytes are whatever `load_document` read, which is the whole file up to
/// its own limit: a larger file never gets this far and comes through
/// [`load_hex`].
pub(in crate::ui::code_editor) fn hex_from_not_text(
    local: bool,
    path: PathBuf,
    bytes: Vec<u8>,
    mtime: Option<MTime>,
) -> HexFile {
    let len = len_of(&bytes);
    let source = if local {
        let end = bytes.len().min(PAGE);
        HexSource::Paged {
            len,
            first_page: bytes[..end].to_vec(),
        }
    } else {
        HexSource::Whole(Arc::from(bytes))
    };
    HexFile {
        path,
        source,
        mtime,
        len,
        // It is here because the text editor declined it.
        opens_as_text: false,
    }
}

/// Reads the pages numbered `idx`, each answering for itself so one bad page
/// does not hide the others. A page past the end is empty, not an error.
pub(super) fn read_pages(
    h: &dyn Host,
    path: &Path,
    idx: Vec<u64>,
) -> Vec<(u64, io::Result<Vec<u8>>)> {
    idx.into_iter()
        .map(|i| {
            let read = i
                .checked_mul(PAGE_BYTES)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
                .and_then(|offset| h.read_range(path, offset, PAGE));
            (i, read)
        })
        .collect()
}

/// What a save writes.
#[cfg_attr(not(test), expect(dead_code, reason = "used by M3 save"))]
pub(super) enum HexWrite {
    /// The edited runs of a paged document, written in place.
    Patches(Vec<(u64, Vec<u8>)>),
    /// A document held whole, written as one file.
    Whole(Arc<[u8]>),
}

/// Saves a hex document, first making sure the file is the one it was read
/// from: `expect` is what it looked like then. The check and the write are one
/// call on the host's thread, so nothing slips between them.
///
/// `force` is the user's Overwrite after a conflict: the modification time no
/// longer has to match. The size still does for patches, because a patch is an
/// offset into the file it was made against and a file that has been resized
/// is not that file; that refuses with `InvalidData` and nothing is written.
/// A whole-file write replaces the file outright, so there is nothing for it
/// to be wrong about.
#[cfg_attr(not(test), expect(dead_code, reason = "used by M3 save"))]
pub(super) fn save_hex(
    h: &dyn Host,
    path: &Path,
    what: HexWrite,
    expect: Expect,
    force: bool,
) -> SaveOutcome {
    match what {
        HexWrite::Patches(patches) => {
            let expect = if force {
                match h.stat(path) {
                    Ok(meta) if meta.len == expect.len => Expect {
                        len: meta.len,
                        mtime: meta.mtime,
                    },
                    Ok(meta) => {
                        return SaveOutcome::Failed(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "{} is {} bytes now, was {}: edits to its bytes no longer apply",
                                path.display(),
                                meta.len,
                                expect.len
                            ),
                        ));
                    }
                    Err(e) => return SaveOutcome::Failed(e),
                }
            } else {
                expect
            };
            match h.write_ranges(path, &patches, expect) {
                Ok(meta) => SaveOutcome::Saved(meta.mtime),
                Err(e) => match Changed::of(&e) {
                    Some(changed) => SaveOutcome::Conflict(changed.mtime),
                    None => SaveOutcome::Failed(e),
                },
            }
        }
        HexWrite::Whole(bytes) => {
            if !force {
                match h.stat(path) {
                    Ok(meta) if meta.len != expect.len || meta.mtime != expect.mtime => {
                        return SaveOutcome::Conflict(meta.mtime);
                    }
                    Ok(_) => {}
                    // Gone since it was read: saving puts it back, as it does
                    // for a text buffer.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return SaveOutcome::Failed(e),
                }
            }
            match h.write_file(path, &bytes) {
                Ok(meta) => SaveOutcome::Saved(meta.mtime),
                Err(e) => SaveOutcome::Failed(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tty59_core::host::local::LocalHost;
    use tty59_core::host::{
        ContentLimits, ContentQuery, ContentResults, Entry, HostId, Meta, Output, SearchHit,
        SharedHost, ShellInventory, WatchSub,
    };

    use super::*;

    /// The real local host behind another host's face: only what a test sets
    /// differs. `read_range` and `write_ranges` are left at the trait's
    /// `Unsupported` defaults, like a remote host's.
    struct Fake {
        inner: SharedHost,
        id: HostId,
        /// Answered for the file's size instead of the real one.
        stat_len: Option<u64>,
        /// Answered by `read_file` instead of reading.
        read_error: Option<io::ErrorKind>,
        full_reads: AtomicUsize,
    }

    impl Fake {
        fn remote() -> Fake {
            Fake {
                inner: LocalHost::new(),
                id: HostId(1),
                stat_len: None,
                read_error: None,
                full_reads: AtomicUsize::new(0),
            }
        }

        fn local_without_ranges() -> Fake {
            Fake {
                id: HostId::LOCAL,
                ..Fake::remote()
            }
        }
    }

    impl Host for Fake {
        fn id(&self) -> HostId {
            self.id
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
            let mut meta = self.inner.stat(p)?;
            if let Some(len) = self.stat_len {
                meta.len = len;
            }
            Ok(meta)
        }
        fn read_file(&self, p: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
            self.full_reads.fetch_add(1, Ordering::SeqCst);
            match self.read_error {
                Some(kind) => Err(kind.into()),
                None => self.inner.read_file(p, max_bytes),
            }
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

    fn file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn loaded(got: Result<HexLoaded, EditorOpenError>) -> HexFile {
        match got {
            Ok(HexLoaded::File(file)) => file,
            Ok(HexLoaded::TooLarge { .. }) => panic!("too large"),
            Err(_) => panic!("an error"),
        }
    }

    /// A pattern that is not text and not a picture: it opens with a NUL.
    fn binary(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    // --- load_hex ---------------------------------------------------------

    #[test]
    fn a_local_file_is_paged_with_its_size_and_first_page() {
        let dir = tempfile::TempDir::new().unwrap();
        let bytes = binary(PAGE * 2 + 100);
        let path = file(&dir, "a.bin", &bytes);

        let got = loaded(load_hex(&*LocalHost::new(), path));
        assert_eq!(got.len, bytes.len() as u64);
        assert!(!got.opens_as_text);
        assert!(got.mtime.is_some());
        match got.source {
            HexSource::Paged { len, first_page } => {
                assert_eq!(len, bytes.len() as u64);
                assert_eq!(first_page, bytes[..PAGE]);
            }
            HexSource::Whole(_) => panic!("a local file is paged"),
        }
    }

    #[test]
    fn a_host_with_no_ranged_reads_gets_the_whole_file_once() {
        let dir = tempfile::TempDir::new().unwrap();
        let bytes = binary(1000);
        let path = file(&dir, "a.bin", &bytes);

        for host in [Fake::remote(), Fake::local_without_ranges()] {
            let got = loaded(load_hex(&host, path.clone()));
            match got.source {
                HexSource::Whole(all) => assert_eq!(&*all, &bytes[..]),
                HexSource::Paged { .. } => panic!("no ranged reads, so whole"),
            }
            assert_eq!(got.len, 1000);
            assert_eq!(host.full_reads.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn a_file_over_what_the_connection_carries_is_too_large_and_is_not_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = file(&dir, "a.bin", &binary(10));
        let host = Fake {
            stat_len: Some(REMOTE_FILE_MAX + 1),
            ..Fake::remote()
        };

        match load_hex(&host, path.clone()) {
            Ok(HexLoaded::TooLarge {
                path: got,
                len,
                limit,
            }) => {
                assert_eq!(got, LocalHost::new().canonicalize(&path).unwrap());
                assert_eq!(len, REMOTE_FILE_MAX + 1);
                assert_eq!(limit, REMOTE_FILE_MAX);
            }
            _ => panic!("too large"),
        }
        assert_eq!(host.full_reads.load(Ordering::SeqCst), 0);

        // Exactly the limit is still read.
        let host = Fake {
            stat_len: Some(REMOTE_FILE_MAX),
            ..Fake::remote()
        };
        assert!(matches!(load_hex(&host, path), Ok(HexLoaded::File(_))));
    }

    #[test]
    fn a_read_that_refuses_for_size_is_too_large_too() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = file(&dir, "a.bin", &binary(10));
        // The size said fine; the file grew before the read.
        let host = Fake {
            read_error: Some(io::ErrorKind::FileTooLarge),
            ..Fake::remote()
        };
        match load_hex(&host, path.clone()) {
            Ok(HexLoaded::TooLarge { len, limit, .. }) => {
                assert_eq!(len, 10);
                assert_eq!(limit, REMOTE_FILE_MAX);
            }
            _ => panic!("too large"),
        }

        // Any other refusal stays an error.
        let host = Fake {
            read_error: Some(io::ErrorKind::PermissionDenied),
            ..Fake::remote()
        };
        assert!(matches!(
            load_hex(&host, path),
            Err(EditorOpenError::Message(_))
        ));
    }

    #[test]
    fn a_file_that_is_not_there_is_a_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let got = load_hex(&*LocalHost::new(), dir.path().join("missing"));
        assert!(matches!(got, Err(EditorOpenError::Message(_))));
        // A directory is refused by the read, not shown as bytes.
        let got = load_hex(&*LocalHost::new(), dir.path().to_path_buf());
        assert!(matches!(got, Err(EditorOpenError::Message(_))));
    }

    #[test]
    fn opens_as_text_follows_the_editors_own_rules() {
        let dir = tempfile::TempDir::new().unwrap();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
        let png = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
        ];
        let cases: [(&str, Vec<u8>, bool); 5] = [
            ("utf8", "héllo\nworld\n".as_bytes().to_vec(), true),
            ("empty", Vec::new(), true),
            ("binary", binary(300), false),
            // An SVG is valid text but is a picture first.
            ("svg", svg.to_vec(), false),
            ("png", png.to_vec(), false),
        ];
        let host = LocalHost::new();
        for (name, bytes, want) in cases {
            let path = file(&dir, name, &bytes);
            let got = loaded(load_hex(&*host, path.clone()));
            assert_eq!(got.opens_as_text, want, "{name} paged");
            let whole = loaded(load_hex(&Fake::remote(), path));
            assert_eq!(whole.opens_as_text, want, "{name} whole");
        }
    }

    #[test]
    fn text_over_the_editors_limit_is_not_text_and_is_not_read_for_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let big = vec![b'a'; usize::try_from(MAX_FILE_BYTES).unwrap() + 1];
        let path = file(&dir, "big.txt", &big);
        let host = Fake {
            id: HostId::LOCAL,
            ..Fake::remote()
        };
        // Ranged reads are what the local host does itself; ask it directly.
        let got = loaded(load_hex(&*LocalHost::new(), path.clone()));
        assert!(!got.opens_as_text);
        assert!(matches!(got.source, HexSource::Paged { .. }));

        // And through a paged path the decision read is skipped: a size over
        // the limit answers without any full read.
        assert!(matches!(
            paged_opens_as_text(&host, &path, MAX_FILE_BYTES + 1),
            Ok(false)
        ));
        assert_eq!(host.full_reads.load(Ordering::SeqCst), 0);
        assert!(paged_opens_as_text(&host, &path, MAX_FILE_BYTES).is_ok());
        assert_eq!(host.full_reads.load(Ordering::SeqCst), 1);
    }

    // --- hex_from_not_text, read_pages ------------------------------------

    #[test]
    fn the_default_open_path_reuses_the_bytes_it_was_given() {
        let bytes = binary(PAGE + 10);
        let mtime = Some(MTime { secs: 5, nanos: 6 });

        let local = hex_from_not_text(true, PathBuf::from("/x/a.bin"), bytes.clone(), mtime);
        assert_eq!(local.len, bytes.len() as u64);
        assert_eq!(local.mtime, mtime);
        assert!(!local.opens_as_text);
        match local.source {
            HexSource::Paged { len, first_page } => {
                assert_eq!(len, bytes.len() as u64);
                assert_eq!(first_page, bytes[..PAGE]);
            }
            HexSource::Whole(_) => panic!("local is paged"),
        }

        let remote = hex_from_not_text(false, PathBuf::from("/x/a.bin"), bytes.clone(), mtime);
        match remote.source {
            HexSource::Whole(all) => assert_eq!(&*all, &bytes[..]),
            HexSource::Paged { .. } => panic!("remote is whole"),
        }

        // A file shorter than a page has a short first page.
        let small = hex_from_not_text(true, PathBuf::from("/x/b"), vec![1, 2, 3], None);
        match small.source {
            HexSource::Paged { first_page, .. } => assert_eq!(first_page, vec![1, 2, 3]),
            HexSource::Whole(_) => panic!("local is paged"),
        }
    }

    #[test]
    fn read_pages_answers_each_page_for_itself() {
        let dir = tempfile::TempDir::new().unwrap();
        let bytes = binary(PAGE * 2 + 5);
        let path = file(&dir, "a.bin", &bytes);

        let got = read_pages(&*LocalHost::new(), &path, vec![2, 0, 7, u64::MAX]);
        let [(i2, p2), (i0, p0), (i7, p7), (imax, pmax)] = &got[..] else {
            panic!("one answer per page, in order");
        };
        assert_eq!((*i2, *i0, *i7, *imax), (2, 0, 7, u64::MAX));
        assert_eq!(p2.as_ref().unwrap(), &bytes[PAGE * 2..]);
        assert_eq!(p0.as_ref().unwrap(), &bytes[..PAGE]);
        assert!(p7.as_ref().unwrap().is_empty(), "past the end is empty");
        assert_eq!(
            pmax.as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidInput,
            "an offset that does not fit is refused, not wrapped"
        );

        let none = read_pages(&Fake::remote(), &path, vec![0]);
        assert_eq!(
            none[0].1.as_ref().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    // --- save_hex ---------------------------------------------------------

    fn expecting(host: &dyn Host, path: &Path) -> Expect {
        let meta = host.stat(path).unwrap();
        Expect {
            len: meta.len,
            mtime: meta.mtime,
        }
    }

    fn stale(expect: Expect) -> Expect {
        Expect {
            mtime: Some(MTime { secs: 1, nanos: 0 }),
            ..expect
        }
    }

    #[test]
    fn patches_are_written_in_place() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[0u8; 8]);
        let expect = expecting(&*host, &path);

        let out = save_hex(
            &*host,
            &path,
            HexWrite::Patches(vec![(1, vec![7, 8])]),
            expect,
            false,
        );
        assert!(matches!(out, SaveOutcome::Saved(Some(_))));
        assert_eq!(std::fs::read(&path).unwrap(), [0, 7, 8, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn patches_on_a_changed_file_conflict_and_write_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[0u8; 8]);
        let expect = expecting(&*host, &path);

        let out = save_hex(
            &*host,
            &path,
            HexWrite::Patches(vec![(0, vec![9])]),
            stale(expect),
            false,
        );
        match out {
            SaveOutcome::Conflict(observed) => assert_eq!(observed, expect.mtime),
            _ => panic!("a conflict carrying the mtime the file has"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), [0u8; 8]);
    }

    #[test]
    fn a_forced_patch_save_ignores_the_mtime_but_not_a_resize() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[0u8; 8]);
        let expect = expecting(&*host, &path);

        let out = save_hex(
            &*host,
            &path,
            HexWrite::Patches(vec![(0, vec![9])]),
            stale(expect),
            true,
        );
        assert!(matches!(out, SaveOutcome::Saved(_)));
        assert_eq!(std::fs::read(&path).unwrap()[0], 9);

        // Someone grew the file: the offsets mean nothing now.
        std::fs::write(&path, [0u8; 12]).unwrap();
        let out = save_hex(
            &*host,
            &path,
            HexWrite::Patches(vec![(0, vec![5])]),
            expect,
            true,
        );
        match out {
            SaveOutcome::Failed(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            _ => panic!("a resized file refuses a patch save"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), [0u8; 12]);
    }

    #[test]
    fn a_patch_past_the_end_fails_and_is_not_a_conflict() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[0u8; 4]);
        let expect = expecting(&*host, &path);

        let out = save_hex(
            &*host,
            &path,
            HexWrite::Patches(vec![(3, vec![1, 2])]),
            expect,
            false,
        );
        match out {
            SaveOutcome::Failed(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidInput),
            _ => panic!("invalid input, not a conflict"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), [0u8; 4]);
    }

    #[test]
    fn a_host_that_cannot_patch_says_so() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = file(&dir, "a.bin", &[0u8; 4]);
        let host = Fake::remote();
        let expect = expecting(&host, &path);
        let out = save_hex(
            &host,
            &path,
            HexWrite::Patches(vec![(0, vec![1])]),
            expect,
            false,
        );
        match out {
            SaveOutcome::Failed(e) => assert_eq!(e.kind(), io::ErrorKind::Unsupported),
            _ => panic!("unsupported"),
        }
    }

    #[test]
    fn a_whole_save_refuses_a_changed_file_and_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[1u8, 2, 3]);
        let expect = expecting(&*host, &path);
        let new: Arc<[u8]> = Arc::from(vec![9u8, 9, 9]);

        // A changed time.
        let out = save_hex(
            &*host,
            &path,
            HexWrite::Whole(new.clone()),
            stale(expect),
            false,
        );
        match out {
            SaveOutcome::Conflict(observed) => assert_eq!(observed, expect.mtime),
            _ => panic!("conflict"),
        }
        // A changed size under the same time.
        let out = save_hex(
            &*host,
            &path,
            HexWrite::Whole(new.clone()),
            Expect {
                len: expect.len + 1,
                ..expect
            },
            false,
        );
        assert!(matches!(out, SaveOutcome::Conflict(_)));
        assert_eq!(std::fs::read(&path).unwrap(), [1, 2, 3]);

        // Matching: written.
        let out = save_hex(&*host, &path, HexWrite::Whole(new.clone()), expect, false);
        assert!(matches!(out, SaveOutcome::Saved(Some(_))));
        assert_eq!(std::fs::read(&path).unwrap(), [9, 9, 9]);
    }

    #[test]
    fn a_forced_whole_save_overwrites_and_a_deleted_file_comes_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = LocalHost::new();
        let path = file(&dir, "a.bin", &[1u8, 2, 3]);
        let expect = expecting(&*host, &path);
        let new: Arc<[u8]> = Arc::from(vec![4u8, 5]);

        let out = save_hex(
            &*host,
            &path,
            HexWrite::Whole(new.clone()),
            stale(expect),
            true,
        );
        assert!(matches!(out, SaveOutcome::Saved(_)));
        assert_eq!(std::fs::read(&path).unwrap(), [4, 5]);

        std::fs::remove_file(&path).unwrap();
        let out = save_hex(&*host, &path, HexWrite::Whole(new), expect, false);
        assert!(matches!(out, SaveOutcome::Saved(_)));
        assert_eq!(std::fs::read(&path).unwrap(), [4, 5]);
    }
}
