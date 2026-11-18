//! Saving hex documents: what is written, the conflicts, the other tab of the same file, the
//! close prompts and the reload after an outside change. Edits go in through the model, so
//! these do not depend on the view's key handling.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tty59_core::host::local::LocalHost;
use tty59_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, Expect, Meta, Output, SearchHit,
    ShellInventory, WatchSub,
};

use super::data::HexContent;
use super::model::Byte;
use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::code_editor::status::observe;
use crate::ui::file_copy::REMOTE_FILE_MAX;

/// The real local host answering as another, counting what it is asked to write.
struct Fake {
    inner: SharedHost,
    id: HostId,
    /// With ranged reads and writes, as a local host; without, as a remote one.
    ranged: bool,
    /// Answered for the file's size instead of the real one.
    stat_len: Option<u64>,
    /// `write_ranges` lands this many runs and then fails, like a full disk. `usize::MAX`: never.
    fail_after_runs: AtomicUsize,
    /// Runs and bytes handed to `write_ranges` calls that went through.
    runs_written: AtomicUsize,
    bytes_written: AtomicUsize,
    whole_writes: AtomicUsize,
}

impl Fake {
    fn new(id: HostId, ranged: bool, stat_len: Option<u64>) -> Arc<Fake> {
        Arc::new(Fake {
            inner: LocalHost::new(),
            id,
            ranged,
            stat_len,
            fail_after_runs: AtomicUsize::new(usize::MAX),
            runs_written: AtomicUsize::new(0),
            bytes_written: AtomicUsize::new(0),
            whole_writes: AtomicUsize::new(0),
        })
    }

    fn local() -> Arc<Fake> {
        Fake::new(HostId::LOCAL, true, None)
    }

    fn remote() -> Arc<Fake> {
        Fake::new(HostId(1), false, None)
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
        self.inner.read_file(p, max_bytes)
    }
    fn read_range(&self, p: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if !self.ranged {
            return Err(io::ErrorKind::Unsupported.into());
        }
        self.inner.read_range(p, offset, len)
    }
    fn write_ranges(
        &self,
        p: &Path,
        patches: &[(u64, Vec<u8>)],
        expect: Expect,
    ) -> io::Result<Meta> {
        if !self.ranged {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let after = self.fail_after_runs.load(Ordering::SeqCst);
        if after != usize::MAX {
            self.inner
                .write_ranges(p, &patches[..after.min(patches.len())], expect)?;
            return Err(io::Error::other("injected: no space left"));
        }
        // Counted only when the host took the write: a call it refuses as a conflict (the
        // un-forced retry after a half write) wrote nothing.
        let meta = self.inner.write_ranges(p, patches, expect)?;
        self.runs_written.fetch_add(patches.len(), Ordering::SeqCst);
        self.bytes_written.fetch_add(
            patches.iter().map(|(_, run)| run.len()).sum(),
            Ordering::SeqCst,
        );
        Ok(meta)
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
        self.whole_writes.fetch_add(1, Ordering::SeqCst);
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

type App = Entity<Tty59App>;

fn real_dir(dir: &tempfile::TempDir) -> PathBuf {
    tty59_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(dir.path()).unwrap())
}

fn binary(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// Puts the file's modification time an hour back, so any later write is plainly newer.
fn age(path: &Path) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(3600))
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

fn hex_docs(app: &App, vcx: &mut VisualTestContext) -> usize {
    app.read_with(vcx, |app, _| app.editor.hex.docs.len())
}

fn front(app: &App, vcx: &mut VisualTestContext) -> BufferId {
    app.read_with(vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
        .expect("something is in front")
}

fn open_hex(app: &App, vcx: &mut VisualTestContext, host: SharedHost, path: &Path) -> BufferId {
    let path = path.to_path_buf();
    let want = hex_docs(app, vcx) + 1;
    app.update_in(vcx, |app, window, cx| {
        app.editor_open_path_as_hex(host, path, window, cx);
    });
    wait(vcx, |vcx| hex_docs(app, vcx) >= want);
    front(app, vcx)
}

fn open_text(app: &App, vcx: &mut VisualTestContext, path: &Path) -> BufferId {
    app.update_in(vcx, |app, window, cx| {
        app.open_file_in_editor(path, window, cx);
    });
    wait(vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    front(app, vcx)
}

/// The text buffer's text becomes `text`, as typing would leave it.
fn type_text(app: &App, vcx: &mut VisualTestContext, id: BufferId, text: &str) {
    app.update_in(vcx, |app, window, cx| {
        let input = app.buffer(id).expect("a text buffer").input.clone();
        replace_buffer_text(&input, text, window, cx);
        app.editor_note_edit(id, cx);
    });
}

fn data(app: &App, vcx: &mut VisualTestContext, id: BufferId) -> Entity<HexData> {
    app.read_with(vcx, |app, _| {
        app.hex(id).expect("a hex document").data.clone()
    })
}

/// Types the hex digits `digits` at `offset`, through the model.
fn edit(app: &App, vcx: &mut VisualTestContext, id: BufferId, offset: u64, digits: &str) {
    let data = data(app, vcx, id);
    data.update(vcx, |d, _| {
        let HexContent::Ready(m) = &mut d.content else {
            panic!("a document that was read");
        };
        m.caret.offset = offset;
        m.caret.low_nibble = false;
        for c in digits.bytes() {
            m.type_hex(c).expect("typed");
        }
    });
}

fn byte_at(app: &App, vcx: &mut VisualTestContext, id: BufferId, off: u64) -> Byte {
    data(app, vcx, id).read_with(vcx, |d, _| match &d.content {
        HexContent::Ready(m) => m.store.byte(off),
        HexContent::TooLarge { .. } => panic!("too large"),
    })
}

fn dirty(app: &App, vcx: &mut VisualTestContext, id: BufferId) -> bool {
    app.read_with(vcx, |app, cx| app.hex_dirty(id, cx))
}

fn saving(app: &App, vcx: &mut VisualTestContext, id: BufferId) -> bool {
    data(app, vcx, id).read_with(vcx, |d, _| d.saving)
}

fn conflict(app: &App, vcx: &mut VisualTestContext, id: BufferId) -> Option<DiskConflict> {
    data(app, vcx, id).read_with(vcx, |d, _| d.conflict)
}

fn disk_mtime(path: &Path) -> Option<MTime> {
    LocalHost::new().stat(path).unwrap().mtime
}

fn save(app: &App, vcx: &mut VisualTestContext, id: BufferId) {
    app.update_in(vcx, |app, window, cx| {
        app.editor_save_file(id, false, false, window, cx);
    });
}

fn answer(vcx: &mut VisualTestContext, label: L10nKey) {
    wait(vcx, |vcx| vcx.has_pending_prompt());
    assert!(vcx.has_pending_prompt(), "it never asked");
    vcx.simulate_prompt_answer(t(label));
    settle(vcx);
}

/// A hex document of a 3000-byte binary on `host`, its mtime an hour back.
fn rig(
    app: &App,
    vcx: &mut VisualTestContext,
    host: Arc<Fake>,
) -> (BufferId, PathBuf, Vec<u8>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("a.bin");
    let original = binary(3000);
    std::fs::write(&path, &original).unwrap();
    age(&path);
    let id = open_hex(app, vcx, host, &path);
    (id, path, original, dir)
}

/// Ctrl+S writes the changed bytes and nothing else: the file is the original with exactly
/// those bytes different, its length is the same, its modification time moved on, the host was
/// handed two runs of one byte each, and the dirty dot is gone. Guards `hex_save` handing
/// `save_hex` the snapshot's patches and `hex_save_landed` folding them in.
#[gpui::test]
fn ctrl_s_writes_only_the_changed_bytes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let host = Fake::local();
    let (id, path, original, _dir) = rig(&app, &mut vcx, host.clone());
    let before = data(&app, &mut vcx, id).read_with(&vcx, |d, _| d.mtime);
    edit(&app, &mut vcx, id, 5, "41");
    edit(&app, &mut vcx, id, 2000, "00");
    assert!(dirty(&app, &mut vcx, id));

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_save_active(window, cx)
    });
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));

    let mut want = original;
    want[5] = 0x41;
    want[2000] = 0x00;
    assert_eq!(std::fs::read(&path).unwrap(), want, "byte for byte");
    assert_eq!(host.runs_written.load(Ordering::SeqCst), 2);
    assert_eq!(host.bytes_written.load(Ordering::SeqCst), 2);
    let after = data(&app, &mut vcx, id).read_with(&vcx, |d, _| d.mtime);
    assert!(after.is_some() && after != before, "the mtime moved on");
    assert_eq!(after, disk_mtime(&path));
    assert_eq!(
        byte_at(&app, &mut vcx, id, 5),
        Byte::Loaded {
            value: 0x41,
            edited: false
        },
        "what was written is the disk now"
    );
}

/// A byte typed while the write is in flight is not part of it: it stays an edit with its
/// value, the file has only the first, and a second save writes it. Guards `hex_save_landed`
/// folding the snapshot (`apply_saved`) rather than clearing the overlay.
#[gpui::test]
fn edits_typed_while_a_save_is_in_flight_stay_dirty(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    edit(&app, &mut vcx, id, 5, "41");

    save(&app, &mut vcx, id);
    assert!(saving(&app, &mut vcx, id), "the write is on its way");
    edit(&app, &mut vcx, id, 10, "42");
    wait(&mut vcx, |vcx| !saving(&app, vcx, id));

    assert!(dirty(&app, &mut vcx, id), "the second edit is not saved");
    assert_eq!(
        byte_at(&app, &mut vcx, id, 10),
        Byte::Loaded {
            value: 0x42,
            edited: true
        }
    );
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk[5], 0x41);
    assert_eq!(on_disk[10], original[10]);

    save(&app, &mut vcx, id);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));
    assert_eq!(std::fs::read(&path).unwrap()[10], 0x42);
}

/// A file someone else rewrote asks before it is written over, writes nothing while the
/// answer is Cancel, and with Overwrite writes only the document's own bytes (their byte
/// stays). Guards the mtime check reaching `write_ranges` and `hex_save_landed`'s Conflict arm.
#[gpui::test]
fn a_changed_file_asks_and_overwrite_writes_only_our_bytes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    edit(&app, &mut vcx, id, 5, "41");
    let mut theirs = original;
    theirs[100] = 0xEE;
    std::fs::write(&path, &theirs).unwrap();

    save(&app, &mut vcx, id);
    answer(&mut vcx, L10nKey::Cancel);
    assert_eq!(std::fs::read(&path).unwrap(), theirs, "nothing written");
    assert!(dirty(&app, &mut vcx, id));
    assert!(matches!(
        conflict(&app, &mut vcx, id),
        Some(DiskConflict::Changed(_))
    ));

    save(&app, &mut vcx, id);
    answer(&mut vcx, L10nKey::EditorOverwrite);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));
    let mut want = theirs;
    want[5] = 0x41;
    assert_eq!(std::fs::read(&path).unwrap(), want);
    assert_eq!(conflict(&app, &mut vcx, id), None);
}

/// A resized file refuses even an Overwrite: the offsets mean nothing there. Nothing is
/// written, the document stays dirty, and the failure is not worded as a partial write.
/// Guards the length check in `save_hex`'s forced Patches arm.
#[gpui::test]
fn a_resized_file_refuses_overwrite(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    edit(&app, &mut vcx, id, 5, "41");
    let mut theirs = original;
    theirs.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    std::fs::write(&path, &theirs).unwrap();
    observe::clear();

    save(&app, &mut vcx, id);
    answer(&mut vcx, L10nKey::EditorOverwrite);
    wait(&mut vcx, |vcx| {
        observe::last("hex-save-failed").is_some() || !dirty(&app, vcx, id)
    });

    assert_eq!(std::fs::read(&path).unwrap(), theirs, "nothing written");
    assert!(dirty(&app, &mut vcx, id));
    let said = observe::last("hex-save-failed").expect("a failure notice");
    assert!(!said.contains(t(L10nKey::HexSavePartlyWritten)), "{said}");
}

/// A write that stops after its first run says the file may be partly written, keeps every
/// edit, and the retry writes all runs again (after the Overwrite the half-written file's new
/// mtime calls for). Guards `abort_save` on Failed and the partial-write sentence.
#[gpui::test]
fn a_write_that_fails_midway_says_so_and_the_retry_writes_every_run(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let host = Fake::local();
    let (id, path, original, _dir) = rig(&app, &mut vcx, host.clone());
    edit(&app, &mut vcx, id, 5, "41");
    edit(&app, &mut vcx, id, 2000, "00");
    host.fail_after_runs.store(1, Ordering::SeqCst);
    observe::clear();

    save(&app, &mut vcx, id);
    wait(&mut vcx, |_| observe::last("hex-save-failed").is_some());

    let said = observe::last("hex-save-failed").expect("a failure notice");
    assert!(said.contains(t(L10nKey::HexSavePartlyWritten)), "{said}");
    assert!(dirty(&app, &mut vcx, id), "every edit is still unsaved");
    let partial = std::fs::read(&path).unwrap();
    assert_eq!(partial[5], 0x41, "the first run landed");
    assert_eq!(partial[2000], original[2000], "the second did not");

    host.fail_after_runs.store(usize::MAX, Ordering::SeqCst);
    save(&app, &mut vcx, id);
    answer(&mut vcx, L10nKey::EditorOverwrite);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));
    let mut want = original;
    want[5] = 0x41;
    want[2000] = 0x00;
    assert_eq!(std::fs::read(&path).unwrap(), want);
    assert_eq!(
        host.runs_written.load(Ordering::SeqCst),
        2,
        "the retry wrote both runs"
    );
}

/// A host that cannot patch gets the whole file, once. Guards the `Whole` arm of `hex_save`.
#[gpui::test]
fn a_remote_document_is_written_whole(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let host = Fake::remote();
    let (id, path, original, _dir) = rig(&app, &mut vcx, host.clone());
    edit(&app, &mut vcx, id, 3, "41");

    save(&app, &mut vcx, id);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));

    let mut want = original;
    want[3] = 0x41;
    assert_eq!(std::fs::read(&path).unwrap(), want);
    assert_eq!(host.whole_writes.load(Ordering::SeqCst), 1);
    assert_eq!(host.runs_written.load(Ordering::SeqCst), 0, "no patching");
}

/// A clean document and a file too large to open have nothing to write. Guards the clean
/// early-out of `hex_save`.
#[gpui::test]
fn saving_a_clean_or_too_large_document_writes_nothing(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let host = Fake::local();
    let (id, path, original, _dir) = rig(&app, &mut vcx, host.clone());
    let before = disk_mtime(&path);
    save(&app, &mut vcx, id);
    settle(&mut vcx);
    assert_eq!(host.runs_written.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(disk_mtime(&path), before);

    let big = Fake::new(HostId(1), false, Some(REMOTE_FILE_MAX + 1));
    let large = open_hex(&app, &mut vcx, big.clone(), &path);
    save(&app, &mut vcx, large);
    settle(&mut vcx);
    assert_eq!(big.whole_writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

/// Closing a document with edits asks with the text editor's three buttons, and each answer
/// does what it says. Guards `editor_close_file` treating a hex document like a buffer.
#[gpui::test]
fn closing_a_dirty_document_asks_and_each_answer_does_what_it_says(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    edit(&app, &mut vcx, id, 5, "41");
    let close = |app: &App, vcx: &mut VisualTestContext| {
        app.update_in(vcx, |app, window, cx| app.editor_close_file(0, window, cx));
    };

    close(&app, &mut vcx);
    answer(&mut vcx, L10nKey::Cancel);
    assert_eq!(hex_docs(&app, &mut vcx), 1, "Cancel keeps it");
    assert!(dirty(&app, &mut vcx, id));

    close(&app, &mut vcx);
    answer(&mut vcx, L10nKey::Save);
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) == 0);
    let mut want = original.clone();
    want[5] = 0x41;
    assert_eq!(
        std::fs::read(&path).unwrap(),
        want,
        "Save wrote, then closed"
    );

    let again = open_hex(&app, &mut vcx, Fake::local(), &path);
    edit(&app, &mut vcx, again, 7, "42");
    close(&app, &mut vcx);
    answer(&mut vcx, L10nKey::EditorDiscard);
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) == 0);
    assert_eq!(std::fs::read(&path).unwrap(), want, "Discard wrote nothing");
}

/// The quit, window-close and close-tab guards count a document with edits, and closing
/// several files asks about it. Guards `editor_unsaved(_in_tab)` and `editor_close_buffers`.
#[gpui::test]
fn the_guards_count_a_dirty_hex_document(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, _path, _original, _dir) = rig(&app, &mut vcx, Fake::local());
    let unsaved = |app: &App, vcx: &mut VisualTestContext| {
        app.read_with(vcx, |app, cx| {
            (app.editor_unsaved(cx), app.editor_unsaved_in_tab(0, cx))
        })
    };
    assert_eq!(unsaved(&app, &mut vcx), (vec![], vec![]));

    edit(&app, &mut vcx, id, 5, "41");
    assert_eq!(unsaved(&app, &mut vcx), (vec![id], vec![id]));

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_buffers(vec![id], window, cx)
    });
    answer(&mut vcx, L10nKey::Cancel);
    assert_eq!(hex_docs(&app, &mut vcx), 1);
}

/// With a text tab and a hex tab of one file, saving either while the other has edits asks
/// first and writes nothing; Overwrite writes, and the other tab then sees a changed file
/// (its banner). Guards `sibling_unsaved` in both `editor_save_file` and `hex_save`, and
/// `editor_notify_siblings`.
#[gpui::test]
fn saving_refuses_while_the_other_tab_of_the_file_has_edits_both_ways(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("notes.txt");
    std::fs::write(&path, "hello world\n").unwrap();
    age(&path);
    let text = open_text(&app, &mut vcx, &path);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_as_hex(text, window, cx)
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) == 1);
    let hex = front(&app, &mut vcx);
    let text_conflict = |app: &App, vcx: &mut VisualTestContext| {
        app.read_with(vcx, |app, _| app.buffer(text).and_then(|f| f.conflict))
    };

    // Hex saves while the text has edits.
    type_text(&app, &mut vcx, text, "changed\n");
    edit(&app, &mut vcx, hex, 0, "58");
    save(&app, &mut vcx, hex);
    answer(&mut vcx, L10nKey::Cancel);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello world\n", "refused");
    assert_eq!(conflict(&app, &mut vcx, hex), None, "not a disk conflict");
    save(&app, &mut vcx, hex);
    answer(&mut vcx, L10nKey::EditorOverwrite);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, hex));
    assert_eq!(std::fs::read(&path).unwrap(), b"Xello world\n");
    assert!(
        matches!(
            text_conflict(&app, &mut vcx),
            Some(DiskConflict::Changed(_))
        ),
        "the text tab, with edits, gets the banner"
    );

    // Text saves while the hex has edits.
    edit(&app, &mut vcx, hex, 1, "45");
    save(&app, &mut vcx, text);
    answer(&mut vcx, L10nKey::Cancel);
    assert_eq!(std::fs::read(&path).unwrap(), b"Xello world\n", "refused");
    save(&app, &mut vcx, text);
    answer(&mut vcx, L10nKey::EditorOverwrite);
    wait(&mut vcx, |_| {
        std::fs::read(&path).is_ok_and(|b| b == b"changed\n")
    });
    wait(&mut vcx, |vcx| conflict(&app, vcx, hex).is_some());
    assert!(
        matches!(
            conflict(&app, &mut vcx, hex),
            Some(DiskConflict::Changed(_))
        ),
        "the hex tab, with edits, gets the banner"
    );
}

/// A clean hex document reloads when its text sibling saves. Guards `editor_notify_siblings`
/// reaching a clean document's reload.
#[gpui::test]
fn a_clean_hex_document_reloads_after_its_text_sibling_saves(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("notes.txt");
    std::fs::write(&path, "hello world\n").unwrap();
    age(&path);
    let text = open_text(&app, &mut vcx, &path);
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_as_hex(text, window, cx)
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) == 1);
    let hex = front(&app, &mut vcx);

    type_text(&app, &mut vcx, text, "changed\n");
    save(&app, &mut vcx, text);
    wait(&mut vcx, |vcx| {
        byte_at(&app, vcx, hex, 0)
            == Byte::Loaded {
                value: b'c',
                edited: false,
            }
    });
    assert_eq!(
        data(&app, &mut vcx, hex).read_with(&vcx, |d, _| d.len()),
        Some(8)
    );
    assert!(!dirty(&app, &mut vcx, hex));
}

/// A clean document follows an outside change: the bytes are read again, the caret stays, the
/// old pages are gone. Guards `hex_apply_external_change`'s Reload arm and `apply_reload`.
#[gpui::test]
fn a_clean_document_reloads_on_an_outside_change_and_keeps_the_caret(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    data(&app, &mut vcx, id).update(&mut vcx, |d, _| {
        if let HexContent::Ready(m) = &mut d.content {
            m.caret.offset = 40;
        }
    });
    let generation = data(&app, &mut vcx, id).read_with(&vcx, |d, _| d.generation());
    let mut theirs = original;
    theirs[7] = 0xEE;
    std::fs::write(&path, &theirs).unwrap();
    let observed = Observed::Present(disk_mtime(&path));

    app.update_in(&mut vcx, |app, window, cx| {
        app.hex_apply_external_change(id, observed, window, cx)
    });
    wait(&mut vcx, |vcx| {
        byte_at(&app, vcx, id, 7)
            == Byte::Loaded {
                value: 0xEE,
                edited: false,
            }
    });
    data(&app, &mut vcx, id).read_with(&vcx, |d, _| {
        assert!(d.generation() > generation);
        assert_eq!(d.mtime, disk_mtime(&path));
        let HexContent::Ready(m) = &d.content else {
            panic!("ready");
        };
        assert_eq!(m.caret.offset, 40);
    });
}

/// A document with edits gets the banner instead; Reload drops the edits and reads the file;
/// Keep mine lets a save replace that version. Guards the Conflict arm, `hex_reload_from_disk`
/// via `editor_reload_from_disk`, and `editor_keep_mine`'s hex branch.
#[gpui::test]
fn a_dirty_document_gets_the_banner_and_reload_or_keep_mine_answer_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, path, original, _dir) = rig(&app, &mut vcx, Fake::local());
    edit(&app, &mut vcx, id, 5, "41");
    let mut theirs = original;
    theirs[100] = 0xEE;
    std::fs::write(&path, &theirs).unwrap();
    let observed = Observed::Present(disk_mtime(&path));

    app.update_in(&mut vcx, |app, window, cx| {
        app.hex_apply_external_change(id, observed, window, cx)
    });
    assert!(matches!(
        app.read_with(&vcx, |app, cx| app.active_conflict(cx)),
        Some((got, DiskConflict::Changed(_))) if got == id
    ));
    assert!(dirty(&app, &mut vcx, id), "the edits are not touched");

    // Keep mine: the save goes through without asking and patches their file.
    app.update_in(&mut vcx, |app, _, cx| {
        app.editor_keep_mine(id, disk_mtime(&path), cx)
    });
    assert_eq!(conflict(&app, &mut vcx, id), None);
    save(&app, &mut vcx, id);
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));
    let mut want = theirs.clone();
    want[5] = 0x41;
    assert_eq!(std::fs::read(&path).unwrap(), want);

    // Reload: a new outside change, new edits, and the edits are dropped.
    edit(&app, &mut vcx, id, 9, "42");
    let mut newer = want;
    newer[200] = 0xDD;
    std::fs::write(&path, &newer).unwrap();
    let observed = Observed::Present(disk_mtime(&path));
    app.update_in(&mut vcx, |app, window, cx| {
        app.hex_apply_external_change(id, observed, window, cx)
    });
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_reload_from_disk(id, window, cx)
    });
    wait(&mut vcx, |vcx| !dirty(&app, vcx, id));
    assert_eq!(
        byte_at(&app, &mut vcx, id, 9),
        Byte::Loaded {
            value: newer[9],
            edited: false
        }
    );
    assert_eq!(conflict(&app, &mut vcx, id), None);
}

/// A file that is gone shows the Deleted banner. Guards the Deleted arm.
#[gpui::test]
fn a_deleted_file_shows_the_deleted_banner(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let (id, _path, _original, _dir) = rig(&app, &mut vcx, Fake::local());
    app.update_in(&mut vcx, |app, window, cx| {
        app.hex_apply_external_change(id, Observed::Missing, window, cx)
    });
    assert!(matches!(
        app.read_with(&vcx, |app, cx| app.active_conflict(cx)),
        Some((got, DiskConflict::Deleted)) if got == id
    ));
}
