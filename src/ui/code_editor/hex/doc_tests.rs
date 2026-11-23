//! Hex documents in the editor: how a file comes to be one, what the tab, the menus and the
//! palette say about it, and how it is kept across a restart.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tty59_core::host::local::LocalHost;
use tty59_core::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, HostId, Meta, Output, SearchHit,
    ShellInventory, WatchSub,
};

use super::*;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::editor_session::{self, TabEditor};
use crate::ui::file_copy::REMOTE_FILE_MAX;
use crate::ui::search::CommandKind;

/// The real local host answering as another: only what a test sets differs, and what it is
/// asked is counted. With `ranged` off, `read_range` is the trait's `Unsupported`, as on a
/// remote machine.
struct Counting {
    inner: SharedHost,
    id: HostId,
    ranged: bool,
    /// Answered for the file's size instead of the real one.
    stat_len: Option<u64>,
    ranges: AtomicUsize,
    widest_range: AtomicUsize,
    full_reads: AtomicUsize,
}

impl Counting {
    fn local() -> Arc<Counting> {
        Arc::new(Counting {
            inner: LocalHost::new(),
            id: HostId::LOCAL,
            ranged: true,
            stat_len: None,
            ranges: AtomicUsize::new(0),
            widest_range: AtomicUsize::new(0),
            full_reads: AtomicUsize::new(0),
        })
    }

    fn remote(stat_len: Option<u64>) -> Arc<Counting> {
        Arc::new(Counting {
            inner: LocalHost::new(),
            id: HostId(1),
            ranged: false,
            stat_len,
            ranges: AtomicUsize::new(0),
            widest_range: AtomicUsize::new(0),
            full_reads: AtomicUsize::new(0),
        })
    }
}

impl Host for Counting {
    fn id(&self) -> HostId {
        self.id
    }
    fn separator(&self) -> char {
        self.inner.separator()
    }
    fn is_absolute(&self, p: &Path) -> bool {
        self.inner.is_absolute(p)
    }
    fn read_dir(&self, dir: &Path, root: Option<&Path>) -> std::io::Result<Vec<Entry>> {
        self.inner.read_dir(dir, root)
    }
    fn stat(&self, p: &Path) -> std::io::Result<Meta> {
        let mut meta = self.inner.stat(p)?;
        if let Some(len) = self.stat_len {
            meta.len = len;
        }
        Ok(meta)
    }
    fn read_file(&self, p: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.full_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read_file(p, max_bytes)
    }
    fn read_range(&self, p: &Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        if !self.ranged {
            return Err(std::io::ErrorKind::Unsupported.into());
        }
        self.ranges.fetch_add(1, Ordering::SeqCst);
        self.widest_range.fetch_max(len, Ordering::SeqCst);
        self.inner.read_range(p, offset, len)
    }
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(p)
    }
    fn search(
        &self,
        roots: &[PathBuf],
        query: &str,
        limit: usize,
        max_dirs: usize,
        show_hidden: bool,
    ) -> std::io::Result<Vec<SearchHit>> {
        self.inner
            .search(roots, query, limit, max_dirs, show_hidden)
    }
    fn search_content(
        &self,
        roots: &[PathBuf],
        query: &ContentQuery,
        limits: &ContentLimits,
    ) -> std::io::Result<ContentResults> {
        self.inner.search_content(roots, query, limits)
    }
    fn write_file(&self, p: &Path, bytes: &[u8]) -> std::io::Result<Meta> {
        self.inner.write_file(p, bytes)
    }
    fn create_file_new(&self, p: &Path) -> std::io::Result<()> {
        self.inner.create_file_new(p)
    }
    fn create_dir(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.create_dir(p, recursive)
    }
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.rename(from, to)
    }
    fn remove(&self, p: &Path, recursive: bool) -> std::io::Result<()> {
        self.inner.remove(p, recursive)
    }
    fn repo_root(&self, p: &Path) -> std::io::Result<Option<PathBuf>> {
        self.inner.repo_root(p)
    }
    fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<Output> {
        self.inner.git(cwd, args)
    }
    fn shells(&self) -> std::io::Result<ShellInventory> {
        self.inner.shells()
    }
    fn watch(&self, dirs: &[PathBuf]) -> std::io::Result<WatchSub> {
        self.inner.watch(dirs)
    }
}

const MIB: u64 = 1024 * 1024;

fn real_dir(dir: &tempfile::TempDir) -> PathBuf {
    tty59_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(dir.path()).unwrap())
}

/// Something that is not text and not a picture: it has NULs from the start.
fn binary(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// A file of `len` bytes that takes the space of its `head` only: the rest is a hole.
fn sparse(path: &Path, len: u64, head: &[u8]) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(head).unwrap();
    file.set_len(len).unwrap();
}

fn text_head() -> Vec<u8> {
    "a line of plain words\n".repeat(5000).into_bytes()
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

fn hex_docs(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
    app.read_with(vcx, |app, _| app.editor.hex.docs.len())
}

fn local_host(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> SharedHost {
    app.read_with(vcx, |app, cx| app.active_host(cx).expect("a host"))
}

fn front(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<BufferId> {
    app.read_with(vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
}

fn files_in_tab(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> usize {
    app.read_with(vcx, |app, _| app.tab_code().map_or(0, |c| c.files.len()))
}

fn open_text(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, path: &Path) -> BufferId {
    app.update_in(vcx, |app, window, cx| {
        app.open_file_in_editor(path, window, cx);
    });
    wait(vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    front(app, vcx).expect("the file is in front")
}

fn open_hex(
    app: &Entity<Tty59App>,
    vcx: &mut VisualTestContext,
    host: SharedHost,
    path: &Path,
    want: usize,
) -> BufferId {
    let path = path.to_path_buf();
    app.update_in(vcx, |app, window, cx| {
        app.editor_open_path_as_hex(host, path, window, cx);
    });
    wait(vcx, |vcx| hex_docs(app, vcx) >= want);
    front(app, vcx).expect("the hex tab is in front")
}

fn palette_kinds(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Vec<CommandKind> {
    app.read_with(vcx, |app, cx| {
        app.editor_palette_items(cx)
            .into_iter()
            .map(|i| i.kind)
            .collect()
    })
}

// ---- what a large file turns out to be -----------------------------------------------------

/// Guards `sniff_large`'s local branch: a big binary costs one prefix read, never the file.
#[test]
fn a_64_mib_sparse_binary_opens_as_a_windowed_hex_tab_after_one_prefix_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("big.bin");
    sparse(&path, 64 * MIB, &binary(100));
    let host = Counting::local();

    let got = load_document(&*host, path);
    match got {
        Ok(Opened::Hex(io::HexLoaded::File(file))) => {
            assert!(!file.opens_as_text);
            match file.source {
                io::HexSource::Paged { len, first_page } => {
                    assert_eq!(len, 64 * MIB);
                    assert_eq!(first_page.len(), SNIFF_PREFIX, "the prefix is page 0");
                    assert_eq!(first_page[..100], binary(100)[..]);
                }
                io::HexSource::Whole(_) => panic!("a local file is windowed"),
            }
        }
        _ => panic!("a big binary opens as hex"),
    }
    assert_eq!(host.ranges.load(Ordering::SeqCst), 1);
    assert!(host.widest_range.load(Ordering::SeqCst) <= SNIFF_PREFIX);
    assert_eq!(
        host.full_reads.load(Ordering::SeqCst),
        0,
        "never read whole"
    );
}

/// Guards the `TooLargeForText` answer: text keeps the words, which now carry the way out.
#[test]
fn a_64_mib_text_file_still_says_too_large_and_names_the_file_for_the_offer() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("big.log");
    sparse(&path, 64 * MIB, &text_head());
    let host = Counting::local();

    match load_document(&*host, path.clone()) {
        Err(EditorOpenError::TooLarge {
            path: offered,
            message,
        }) => {
            assert_eq!(offered, path, "the offer opens this file");
            assert!(
                message.contains("64 MB"),
                "the words are the ones it always had: {message}"
            );
        }
        _ => panic!("too large, with the file named"),
    }
    assert_eq!(host.full_reads.load(Ordering::SeqCst), 0);
}

/// Guards the bound on what `NotText` carries: a non-text file over the text limit takes the
/// hex routes, so a multi-gigabyte file is never held by the default open.
#[test]
fn a_non_text_file_over_the_text_limit_never_comes_back_as_not_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("five.bin");
    let len = usize::try_from(MAX_FILE_BYTES).unwrap() + 1;
    std::fs::write(&path, binary(len)).unwrap();

    assert!(
        matches!(
            load_document(&*Counting::local(), path.clone()),
            Ok(Opened::Hex(io::HexLoaded::File(_)))
        ),
        "a local one is windowed"
    );
    let remote = Counting::remote(None);
    match load_document(&*remote, path) {
        Ok(Opened::Hex(io::HexLoaded::File(file))) => {
            assert!(matches!(file.source, io::HexSource::Whole(_)));
        }
        _ => panic!("a remote one is held whole, once"),
    }
    assert_eq!(remote.full_reads.load(Ordering::SeqCst), 1);
}

/// And one that is not over the limit is still `NotText`: that is the route the notice
/// branch takes its bytes from.
#[test]
fn a_non_text_file_under_the_text_limit_is_still_not_text_with_its_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("small.bin");
    std::fs::write(&path, binary(3000)).unwrap();

    match load_document(&*Counting::local(), path) {
        Err(EditorOpenError::NotText { bytes, mtime, .. }) => {
            assert_eq!(bytes.len(), 3000);
            assert!(mtime.is_some());
        }
        _ => panic!("not text"),
    }
}

/// Guards the remote rules: text is too large (the file is read once to tell), and over the
/// transfer limit nothing is read at all.
#[test]
fn a_remote_file_over_the_text_limit_is_sniffed_once_and_over_the_transfer_limit_not_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("big.txt");
    let len = usize::try_from(MAX_FILE_BYTES).unwrap() + 1;
    std::fs::write(&path, "words and words\n".repeat(len / 16 + 1)).unwrap();

    let remote = Counting::remote(None);
    assert!(
        matches!(
            load_document(&*remote, path.clone()),
            Err(EditorOpenError::TooLarge { .. })
        ),
        "remote text over the limit"
    );
    assert_eq!(remote.full_reads.load(Ordering::SeqCst), 1);

    let beyond = Counting::remote(Some(REMOTE_FILE_MAX + 1));
    assert!(
        matches!(
            load_document(&*beyond, path),
            Err(EditorOpenError::TooLarge { .. })
        ),
        "over the transfer limit it cannot be told apart, so it gets the words"
    );
    assert_eq!(
        beyond.full_reads.load(Ordering::SeqCst),
        0,
        "and nothing was read"
    );
}

/// Guards the `Picture` branch: a file with a picture's bytes keeps the picture's rules
/// whatever its name, however large.
#[test]
fn a_large_file_with_a_pictures_signature_takes_the_picture_route() {
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("not-named-like-one");
    let mut head = b"\x89PNG\r\n\x1a\n".to_vec();
    head.extend(binary(64));
    sparse(&path, 5 * MIB, &head);

    assert!(
        matches!(
            load_document(&*Counting::local(), path),
            Ok(Opened::ImageFailed(_))
        ),
        "a picture that does not decode is an error in its tab, not a hex tab"
    );
}

/// Guards `goes_to_hex_by_default`: hex only where the notice used to show.
#[test]
fn the_default_route_is_hex_only_where_the_desktop_cannot_or_must_not_be_asked() {
    // (local, can spawn, program) -> hex
    let table = [
        ((false, true, false), true),
        ((false, false, false), true),
        ((true, false, false), true),
        ((true, true, true), true),
        ((true, true, false), false),
    ];
    for ((local, can_spawn, program), hex) in table {
        assert_eq!(
            goes_to_hex_by_default(local, can_spawn, program),
            hex,
            "local {local}, can spawn {can_spawn}, program {program}"
        );
    }
}

// ---- the status bar ------------------------------------------------------------------------

/// Guards `readouts_of`: offset in both bases, selection when there is one, only the values
/// the bytes can say, and the size in the shared helper's words.
#[test]
fn the_status_readouts_spell_the_offset_the_values_and_the_size() {
    crate::ui::i18n::set_locale("en");
    let status = |offset, selection, bytes: &[u8], len| view::HexStatus {
        offset,
        selection,
        values: model::inspect(bytes),
        len,
    };

    let full = readouts_of(&status(31, None, &[0xDE, 0xAD, 0xBE, 0xEF], 1234));
    assert_eq!(full.cursor.to_string(), "Offset 0x1F (31)");
    assert_eq!(
        full.values.to_string(),
        "u8 222 · i8 -34 · u16 LE 44510 BE 57005 · u32 LE 4022250974 BE 3735928559"
    );
    assert_eq!(full.size.to_string(), "Size 1.2 KiB");

    let selecting = readouts_of(&status(31, Some(4), &[0xDE], 1234));
    assert_eq!(selecting.cursor.to_string(), "Offset 0x1F (31) · Sel 4");
    assert_eq!(
        selecting.values.to_string(),
        "u8 222 · i8 -34",
        "one byte left: no wider values"
    );

    let empty = readouts_of(&status(0, None, &[], 0));
    assert_eq!(empty.values.to_string(), "", "an empty file has none");
}

// ---- opening as hex from an open file --------------------------------------------------------

/// Guards the program rule: a local executable would be launched by the desktop, so its bytes
/// open in a hex tab (from the bytes already read) and it is not handed over.
#[cfg(unix)]
#[gpui::test]
fn a_program_like_binary_opens_as_a_hex_tab_and_is_not_handed_to_the_desktop(
    cx: &mut TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt as _;

    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("tool");
    std::fs::write(&path, binary(3000)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx);
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) >= 1);

    let host = local_host(&app, &mut vcx).id();
    app.read_with(&vcx, |app, cx| {
        let id = app.hex_at(host, &path).expect("a hex tab of the program");
        assert!(app.buffer_at(host, &path).is_none(), "no text buffer");
        assert_eq!(app.hex(id).unwrap().data.read(cx).len(), Some(3000));
    });
}

/// Guards the remote rule and that the bytes `load_document` read are the ones shown: the
/// file is read once, whole.
#[gpui::test]
fn a_remote_binary_opens_as_a_hex_tab_from_the_bytes_already_read(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("remote.bin");
    std::fs::write(&path, binary(3000)).unwrap();
    let remote = Counting::remote(None);
    let host: SharedHost = remote.clone();

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_on_host(host, &path, window, cx);
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) >= 1);

    app.read_with(&vcx, |app, cx| {
        let id = app
            .hex_at(HostId(1), &path)
            .expect("a hex tab of the remote file");
        assert_eq!(app.hex(id).unwrap().data.read(cx).len(), Some(3000));
    });
    assert_eq!(remote.full_reads.load(Ordering::SeqCst), 1, "read once");
}

/// Guards the D2 card: a remote file past the transfer limit is a hex tab that says so, with
/// the limit, instead of nothing.
#[gpui::test]
fn a_remote_file_over_the_transfer_limit_shows_the_error_card_with_the_limit(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("huge.bin");
    std::fs::write(&path, binary(100)).unwrap();
    let host: SharedHost = Counting::remote(Some(REMOTE_FILE_MAX + 1));

    open_hex(&app, &mut vcx, host, &path, 1);

    app.read_with(&vcx, |app, cx| {
        let id = app.hex_at(HostId(1), &path).expect("a tab for it");
        assert_eq!(
            app.hex(id).unwrap().data.read(cx).too_large(),
            Some((REMOTE_FILE_MAX + 1, REMOTE_FILE_MAX))
        );
    });
    assert!(
        vcx.debug_bounds("hex-too-large-0").is_some(),
        "the card is what is painted"
    );
}

/// Guards Open as Hex's two rules: the same path as text and as hex are two tabs, and asking
/// again only brings the hex one forward.
#[gpui::test]
fn open_as_hex_adds_a_second_tab_of_a_text_file_and_asking_again_only_brings_it_forward(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let text = open_text(&app, &mut vcx, &path);

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_as_hex(text, window, cx)
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) >= 1);
    let hex = front(&app, &mut vcx).expect("the hex tab is in front");
    assert_ne!(hex, text);
    assert_eq!(files_in_tab(&app, &mut vcx), 2, "text and hex");

    // Back to the text tab, and ask once more.
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_activate(0, window, cx);
        app.editor_open_as_hex(text, window, cx);
    });
    wait(&mut vcx, |vcx| front(&app, vcx) == Some(hex));
    assert_eq!(hex_docs(&app, &mut vcx), 1, "no second hex document");
    assert_eq!(files_in_tab(&app, &mut vcx), 2);
}

/// Guards `editor_open_as_hex` for a picture: any saved document, not only text.
#[gpui::test]
fn open_as_hex_works_on_a_picture(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("shot.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 90, 200, 255]))
        .save(&path)
        .unwrap();
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_file_in_editor(&path, window, cx);
    });
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| !app.editor.images.docs.is_empty())
    });
    let picture = front(&app, &mut vcx).expect("the picture is in front");

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_as_hex(picture, window, cx)
    });
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) >= 1);

    let host = local_host(&app, &mut vcx).id();
    app.read_with(&vcx, |app, _| {
        assert!(app.hex_at(host, &path).is_some());
        assert!(app.image_at(host, &path).is_some(), "the picture stays");
    });
}

/// Guards the entry the file tree's row menu calls: a file that is not open anywhere opens as
/// hex on the active host.
#[gpui::test]
fn the_file_tree_entry_opens_a_file_as_hex(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("a.dat");
    std::fs::write(&path, binary(500)).unwrap();
    let host = local_host(&app, &mut vcx);

    open_hex(&app, &mut vcx, host.clone(), &path, 1);

    app.read_with(&vcx, |app, _| {
        assert!(app.hex_at(host.id(), &path).is_some());
        assert!(app.buffer_at(host.id(), &path).is_none());
    });
}

/// Guards the file menu's content: a hex document has one, like any saved file.
#[gpui::test]
fn a_hex_tab_menu_has_the_file_items(cx: &mut TestAppContext) {
    use gpui_component::menu::PopupMenu;

    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("a.dat");
    std::fs::write(&path, binary(500)).unwrap();
    let host = local_host(&app, &mut vcx);
    let id = open_hex(&app, &mut vcx, host, &path, 1);

    let weak = app.downgrade();
    let menu = vcx.update(|window, cx| {
        PopupMenu::build(window, cx, move |menu, _window, cx| match weak.upgrade() {
            Some(app) => app.read(cx).editor_file_menu_items(menu, id, &weak, cx),
            None => menu,
        })
    });
    assert!(!vcx.read_entity(&menu, |menu, _| menu.is_empty()));
}

// ---- the palette and the actions ------------------------------------------------------------

/// Guards `hex_offer`/the palette gate: Open as Hex with a saved document in front, nothing
/// with no file or an untitled buffer.
#[gpui::test]
fn the_palette_offers_open_as_hex_for_a_saved_document_only(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();

    assert!(
        !palette_kinds(&app, &mut vcx).contains(&CommandKind::EditorOpenAsHex),
        "no file in front"
    );

    app.update_in(&mut vcx, |app, window, cx| app.editor_new_file(window, cx));
    settle(&mut vcx);
    assert!(
        !palette_kinds(&app, &mut vcx).contains(&CommandKind::EditorOpenAsHex),
        "an untitled buffer is not anywhere yet"
    );

    open_text(&app, &mut vcx, &path);
    assert!(
        palette_kinds(&app, &mut vcx).contains(&CommandKind::EditorOpenAsHex),
        "a saved text file"
    );
}

/// Guards the reverse row: Open as Text in a hex tab of a text file, not of a binary, and it
/// brings the text tab forward.
#[gpui::test]
fn the_palette_offers_open_as_text_in_a_hex_tab_of_a_text_file_only(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let (text, bin) = (
        real_dir(&dir).join("notes.txt"),
        real_dir(&dir).join("a.dat"),
    );
    std::fs::write(&text, "hello\n").unwrap();
    std::fs::write(&bin, binary(500)).unwrap();
    let host = local_host(&app, &mut vcx);

    open_hex(&app, &mut vcx, host.clone(), &text, 1);
    let kinds = palette_kinds(&app, &mut vcx);
    assert!(
        kinds.contains(&CommandKind::EditorOpenAsText),
        "a text file's bytes"
    );
    assert!(
        !kinds.contains(&CommandKind::EditorOpenAsHex),
        "already hex"
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.run_command(CommandKind::EditorOpenAsText, window, cx)
    });
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| app.active_buffer().is_some())
    });
    app.read_with(&vcx, |app, _| {
        assert!(
            app.buffer_at(host.id(), &text).is_some(),
            "the text tab opened"
        );
    });

    open_hex(&app, &mut vcx, host, &bin, 2);
    let kinds = palette_kinds(&app, &mut vcx);
    assert!(
        !kinds.contains(&CommandKind::EditorOpenAsText),
        "a binary has no text form"
    );
    assert!(!kinds.contains(&CommandKind::EditorOpenAsHex));
}

/// Guards the root handler for Open as Hex: it acts on a saved document in front.
#[gpui::test]
fn the_open_as_hex_action_acts_on_the_saved_document_in_front(cx: &mut TestAppContext) {
    use crate::core::actions::EditorOpenAsHex;

    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();
    open_text(&app, &mut vcx, &path);

    vcx.dispatch_action(EditorOpenAsHex);
    wait(&mut vcx, |vcx| hex_docs(&app, vcx) >= 1);
    assert_eq!(hex_docs(&app, &mut vcx), 1);
}

// ---- close, rename, session --------------------------------------------------------------------

/// Guards `editor_close_file`/`editor_drop_hex`: closing the tab forgets the document and its
/// views.
#[gpui::test]
fn closing_a_hex_tab_drops_its_document_and_views(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("a.dat");
    std::fs::write(&path, binary(500)).unwrap();
    let host = local_host(&app, &mut vcx);
    let id = open_hex(&app, &mut vcx, host, &path, 1);
    assert!(
        app.read_with(&vcx, |app, _| !app.editor.hex.views.borrow().is_empty()),
        "the body made its view"
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    settle(&mut vcx);

    app.read_with(&vcx, |app, _| {
        assert!(app.hex(id).is_none());
        assert!(app.editor.hex.views.borrow().is_empty());
    });
    assert_eq!(files_in_tab(&app, &mut vcx), 0);
}

/// Guards `hex_path_moved`: a rename through tty59 moves the document, its name and the
/// lookup, and reads nothing.
#[gpui::test]
fn renaming_the_file_moves_the_hex_tab_with_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let (from, to) = (real_dir(&dir).join("a.dat"), real_dir(&dir).join("b.dat"));
    std::fs::write(&from, binary(500)).unwrap();
    let host = local_host(&app, &mut vcx);
    let id = open_hex(&app, &mut vcx, host.clone(), &from, 1);

    app.update(&mut vcx, |app, cx| {
        app.editor_path_moved(host.id(), &from, &to, cx)
    });
    settle(&mut vcx);

    app.read_with(&vcx, |app, cx| {
        assert_eq!(app.hex_at(host.id(), &to), Some(id));
        assert!(app.hex_at(host.id(), &from).is_none());
        let doc = app.hex(id).unwrap();
        assert_eq!(doc.name.to_string(), "b.dat");
        assert_eq!(doc.data.read(cx).name().to_string(), "b.dat");
    });
}

/// Guards the session record and restore: a path open as text and as hex comes back as two
/// tabs, in order, and recording writes the same thing down.
#[gpui::test]
fn a_path_open_as_text_and_as_hex_comes_back_as_two_tabs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let root = real_dir(&dir);
    let path = root.join("notes.txt");
    std::fs::write(&path, "hello\n").unwrap();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    crate::ui::i18n::set_locale("en");
    let state = TabEditor {
        files: vec![path.clone(), path.clone()],
        active: 1,
        visible: true,
        split: None,
        hex: vec![1],
    };
    let tab = app.read_with(&vcx, |app, _| app.tabs[1].tree_id.get());
    vcx.update(|_, cx| editor_session::put(cx, tab, state.clone()));
    app.update_in(&mut vcx, |app, window, cx| {
        app.active = 1;
        app.editor.restored.remove(&tab);
        app.editor_restore_active(window, cx);
    });
    wait(&mut vcx, |vcx| {
        hex_docs(&app, vcx) >= 1 && files_in_tab(&app, vcx) >= 2
    });

    let host = local_host(&app, &mut vcx).id();
    app.read_with(&vcx, |app, _| {
        let code = app.tab_code().unwrap();
        assert_eq!(code.files.len(), 2);
        let text = app.buffer_at(host, &path).expect("the text tab");
        let hex = app.hex_at(host, &path).expect("the hex tab");
        assert_eq!(code.files, [text, hex], "in the order recorded");
        assert_eq!(code.active_id(), Some(hex));
    });
    app.update(&mut vcx, |app, cx| app.editor_record_sessions(cx));
    let recorded = vcx.update(|_, cx| editor_session::get(cx, tab)).unwrap();
    assert_eq!(recorded, state);
}

/// Guards the restore rule for a tab not recorded as bytes: a text file that has since become
/// too large for the editor is not put back as a hex tab nobody asked for.
#[gpui::test]
fn a_file_recorded_as_text_is_not_restored_as_hex(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("grew.bin");
    sparse(&path, 64 * MIB, &binary(100));
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    crate::ui::i18n::set_locale("en");
    let state = TabEditor {
        files: vec![path.clone()],
        active: 0,
        visible: true,
        split: None,
        hex: Vec::new(),
    };
    let tab = app.read_with(&vcx, |app, _| app.tabs[1].tree_id.get());
    vcx.update(|_, cx| editor_session::put(cx, tab, state));
    app.update_in(&mut vcx, |app, window, cx| {
        app.active = 1;
        app.editor.restored.remove(&tab);
        app.editor_restore_active(window, cx);
    });
    settle(&mut vcx);
    std::thread::sleep(Duration::from_millis(300));
    settle(&mut vcx);

    assert_eq!(hex_docs(&app, &mut vcx), 0);
}
