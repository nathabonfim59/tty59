//! Go to offset and find, through the editor's bar, over real files and the counting host. Each
//! test says which production line it guards.

use std::io::{Seek as _, SeekFrom, Write as _};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_component::input::InputEvent;

use super::super::io::{HexFile, HexLoaded, HexSource};
use super::super::model::{FIND_CHUNK, PAGE};
use super::super::test_support::{Counting, wait};
use super::*;
use crate::ui::app::test_window::harness_with_tabs;

const MIB: u64 = 1024 * 1024;

struct Rig {
    app: Entity<Tty59App>,
    vcx: VisualTestContext,
    host: Arc<Counting>,
    id: BufferId,
    _dir: tempfile::TempDir,
    _streams: Vec<crate::daemon::transport::Stream>,
}

fn real_dir(dir: &tempfile::TempDir) -> PathBuf {
    tty59_core::core::path_spelling::local_spelling_buf(std::fs::canonicalize(dir.path()).unwrap())
}

/// A hex tab over a file of `len` bytes, zeros except for `spots`.
fn rig(cx: &mut TestAppContext, len: u64, spots: &[(u64, &[u8])]) -> Rig {
    cx.executor().allow_parking();
    let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let dir = tempfile::tempdir().unwrap();
    let path = real_dir(&dir).join("f.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    for (at, bytes) in spots {
        file.seek(SeekFrom::Start(*at)).unwrap();
        file.write_all(bytes).unwrap();
    }
    drop(file);
    let host = Counting::new();
    let opened = host.clone();
    app.update_in(&mut vcx, |app, window, cx| {
        app.editor_open_path_as_hex(opened, path, window, cx)
    });
    wait(&mut vcx, |vcx| {
        app.read_with(vcx, |app, _| !app.editor.hex.docs.is_empty())
    });
    let id = app
        .read_with(&vcx, |app, _| app.tab_code().and_then(TabCode::active_id))
        .expect("the hex tab is in front");
    Rig {
        app,
        vcx,
        host,
        id,
        _dir: dir,
        _streams: streams,
    }
}

impl Rig {
    fn data(&self) -> Entity<HexData> {
        let id = self.id;
        self.app
            .read_with(&self.vcx, |app, _| app.hex(id).unwrap().data.clone())
    }

    fn caret(&self) -> (u64, Option<Range<u64>>) {
        self.data()
            .read_with(&self.vcx, |data, _| match &data.content {
                HexContent::Ready(m) => (m.caret.offset, m.caret.selection()),
                HexContent::TooLarge { .. } => panic!("too large"),
            })
    }

    fn set_caret(&mut self, offset: u64) {
        self.data().update(&mut self.vcx, |data, _| {
            if let HexContent::Ready(m) = &mut data.content {
                m.caret.offset = offset;
                m.caret.anchor = None;
            }
        });
    }

    fn note(&self) -> Option<Note> {
        self.app.read_with(&self.vcx, |app, _| {
            app.editor.bar.as_ref().and_then(|b| b.find.note.clone())
        })
    }

    fn bar_is(&self, want: impl Fn(&BarKind) -> bool) -> bool {
        self.app.read_with(&self.vcx, |app, _| {
            app.editor.bar.as_ref().is_some_and(|b| want(&b.kind))
        })
    }

    fn open_find(&mut self) {
        self.app.update_in(&mut self.vcx, |app, window, cx| {
            app.hex_find_open(window, cx)
        });
    }

    fn open_go_to(&mut self) {
        self.app.update_in(&mut self.vcx, |app, window, cx| {
            app.hex_go_to_offset(window, cx)
        });
    }

    /// Types into the bar and presses Enter (Shift+Enter with `backwards`), as the keys do.
    fn enter(&mut self, text: &str, backwards: bool) {
        let input = self.app.read_with(&self.vcx, |app, _| {
            app.editor.bar.as_ref().expect("a bar is up").input.clone()
        });
        let text = text.to_string();
        input.update_in(&mut self.vcx, |st, window, cx| {
            st.set_value(text, window, cx)
        });
        input.update(&mut self.vcx, |_, cx| {
            cx.emit(InputEvent::PressEnter {
                secondary: false,
                shift: backwards,
            })
        });
        self.vcx.run_until_parked();
    }

    /// Waits for the search in the bar to land.
    fn searched(&mut self) {
        let app = self.app.clone();
        wait(&mut self.vcx, |vcx| {
            app.read_with(vcx, |app, _| {
                app.editor
                    .bar
                    .as_ref()
                    .is_none_or(|b| b.find.search.is_none())
            })
        });
    }

    fn find(&mut self, hex: &str, backwards: bool) {
        self.open_find();
        self.enter(hex, backwards);
        self.searched();
    }

    /// Asks for a forward find in one turn, so nothing can land before the caller looks: the
    /// search's cancel flag and the note the bar shows while it runs.
    fn start(&mut self, hex: &str) -> (Arc<AtomicBool>, Option<Note>) {
        self.open_find();
        let id = self.id;
        let hex = hex.to_string();
        self.app.update_in(&mut self.vcx, |app, window, cx| {
            app.hex_find_submit(id, FindMode::Hex, &hex, true, window, cx);
            let bar = app.editor.bar.as_ref().expect("the bar");
            let flag = bar.find.search.as_ref().map(|s| s.cancel.clone());
            (flag.expect("a search is running"), bar.find.note.clone())
        })
    }
}

/// Guards `hex_go_to_offset_submit`'s `Ok` arm and `parse_offset` being fed the caret: every
/// accepted form lands on its byte, the selection goes, and the bar closes.
#[gpui::test]
fn go_to_moves_the_caret_for_each_accepted_form(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 4096, &[]);
    // The text, where the caret is, and where it should end up: relative forms count from it.
    let forms = [
        ("0x1F", 100, 31),
        ("1fh", 100, 31),
        ("31", 100, 31),
        ("+16", 31, 47),
        ("-16", 47, 31),
        ("0x0", 100, 0),
    ];
    for (text, from, want) in forms {
        rig.data().update(&mut rig.vcx, |data, _| {
            if let HexContent::Ready(m) = &mut data.content {
                m.caret.anchor = Some(5);
                m.caret.offset = from;
            }
        });
        rig.open_go_to();
        rig.enter(text, false);
        assert_eq!(rig.caret(), (want, None), "{text}");
        assert!(!rig.bar_is(|_| true), "{text} closes the bar");
    }
}

/// Guards the `Err` arm: out of range and junk keep the bar up, say why, and move nothing.
#[gpui::test]
fn go_to_refuses_out_of_range_and_junk_and_says_why(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 4096, &[]);
    rig.set_caret(7);
    rig.open_go_to();
    let table = [
        ("0x1000", Note::Offset(OffsetError::OutOfRange, 4096)),
        ("-100", Note::Offset(OffsetError::OutOfRange, 4096)),
        ("zz", Note::Offset(OffsetError::Invalid, 4096)),
        ("", Note::Offset(OffsetError::Empty, 4096)),
    ];
    for (text, want) in table {
        rig.enter(text, false);
        assert_eq!(rig.note(), Some(want), "{text:?}");
        assert!(
            rig.bar_is(|k| matches!(k, BarKind::GoToOffset { .. })),
            "{text:?} keeps the bar"
        );
        assert_eq!(rig.caret(), (7, None), "{text:?} moves nothing");
    }
    assert_eq!(
        Note::Offset(OffsetError::OutOfRange, 4096).text(),
        "That is outside the file, which has 4096 bytes"
    );
}

/// Guards the note being about the last Enter: typing clears it (`hex_bar_edited`).
#[gpui::test]
fn typing_clears_what_the_bar_said(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 4096, &[]);
    rig.open_go_to();
    rig.enter("zz", false);
    assert!(rig.note().is_some());
    let input = rig.app.read_with(&rig.vcx, |app, _| {
        app.editor.bar.as_ref().unwrap().input.clone()
    });
    input.update(&mut rig.vcx, |_, cx| cx.emit(InputEvent::Change));
    rig.vcx.run_until_parked();
    assert_eq!(rig.note(), None);
}

/// Guards `HostBytes::read` and the chunk overlap: a pattern across a 64 KiB page edge and
/// across a 1 MiB read edge of a paged file is found, and selected from its first byte.
#[gpui::test]
fn find_crosses_a_page_edge_and_a_read_edge(cx: &mut TestAppContext) {
    let edge = FIND_CHUNK as u64;
    let mut rig = rig(
        cx,
        3 * MIB,
        &[
            (PAGE as u64 - 2, &[0xde, 0xad, 0xbe, 0xef]),
            (edge - 2, &[1, 2, 3, 4]),
        ],
    );
    rig.find("de ad be ef", false);
    let at = PAGE as u64 - 2;
    assert_eq!(rig.caret(), (at, Some(at..at + 4)));
    rig.find("01 02 03 04", false);
    assert_eq!(rig.caret(), (edge - 2, Some(edge - 2..edge + 2)));
    assert_eq!(rig.note(), None);
}

/// Guards the overlay snapshot: bytes that exist only as an unsaved edit are found.
#[gpui::test]
fn find_sees_an_unsaved_edit(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 64 * 1024, &[]);
    rig.set_caret(1000);
    rig.data().update(&mut rig.vcx, |data, _| {
        if let HexContent::Ready(m) = &mut data.content {
            m.type_hex(b'A').unwrap();
            m.type_hex(b'B').unwrap();
        }
    });
    rig.set_caret(0);
    rig.find("ab", false);
    assert_eq!(rig.caret(), (1000, Some(1000..1001)));
}

/// Guards `start_of` and the backward scan: previous finds the earlier match, Shift+Enter
/// is previous, and from the first match it wraps to the last.
#[gpui::test]
fn previous_finds_the_earlier_match_and_wraps(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[(100, &[0xaa]), (5000, &[0xaa])]);
    rig.set_caret(6000);
    rig.find("aa", true);
    assert_eq!(rig.caret().0, 5000);
    rig.enter("aa", true);
    rig.searched();
    assert_eq!(rig.caret().0, 100);
    rig.enter("aa", true);
    rig.searched();
    assert_eq!(rig.caret().0, 5000, "wrapped back to the last match");
}

/// Guards forward wrapping: from the last match Enter goes round to the first.
#[gpui::test]
fn next_wraps_from_the_last_match_to_the_first(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[(100, &[0xaa]), (5000, &[0xaa])]);
    rig.find("aa", false);
    assert_eq!(rig.caret().0, 100);
    rig.enter("aa", false);
    rig.searched();
    assert_eq!(rig.caret().0, 5000);
    rig.enter("aa", false);
    rig.searched();
    assert_eq!(rig.caret().0, 100, "wrapped round to the first match");
}

/// Guards `start_of`'s `u64::MAX`: previous from a caret on byte 0 searches from the end.
#[gpui::test]
fn previous_from_the_first_byte_wraps_to_the_last_match(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[(0, &[0xaa]), (5000, &[0xaa])]);
    rig.find("aa", true);
    assert_eq!(rig.caret().0, 5000);
}

/// Guards `pattern_of`'s text arm: the UTF-8 bytes of the query are searched, and the same
/// query in hex mode is refused as digits.
#[gpui::test]
fn text_mode_finds_utf8_and_hex_mode_refuses_it(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 1000, &[(200, "héllo wörld".as_bytes())]);
    rig.find("é", false);
    assert_eq!(rig.note(), Some(Note::Pattern(PatternError::InvalidDigit)));
    assert_eq!(rig.caret(), (0, None));
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_mode(FindMode::Text, window, cx)
    });
    rig.enter("é", false);
    rig.searched();
    assert_eq!(rig.caret(), (201, Some(201..203)));
    rig.enter("ö", false);
    rig.searched();
    assert_eq!(rig.caret().0, 200 + "héllo w".len() as u64);
}

/// Guards the odd-digit and empty refusals and that a refused query starts no search.
#[gpui::test]
fn a_bad_hex_query_says_why_and_reads_nothing(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 1000, &[]);
    let before = rig.host.reads().len();
    rig.open_find();
    rig.enter("abc", false);
    assert_eq!(rig.note(), Some(Note::Pattern(PatternError::OddDigits)));
    rig.enter("", false);
    assert_eq!(rig.note(), Some(Note::Pattern(PatternError::Empty)));
    assert_eq!(rig.host.reads().len(), before);
}

/// Guards `hex_found`'s `Ok(None)` arm: not found says so, keeps the bar and moves nothing.
#[gpui::test]
fn not_found_says_so(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[(100, &[0xaa])]);
    rig.set_caret(3);
    rig.find("bb", false);
    assert_eq!(rig.note(), Some(Note::NotFound));
    assert_eq!(Note::NotFound.text(), "Not found");
    assert_eq!(rig.caret(), (3, None));
    assert!(rig.bar_is(|k| matches!(k, BarKind::HexFind { .. })));
}

/// Guards the seq check in `hex_found` and the `Drop` of `Search`: a query replaced before its
/// answer came has its flag set and its answer dropped, so only the newer one moves the caret.
#[gpui::test]
fn a_superseded_search_is_cancelled_and_its_answer_dropped(cx: &mut TestAppContext) {
    let mut rig = rig(
        cx,
        96 * MIB,
        &[
            (100_000, &[0x11, 0x22, 0x33, 0x44]),
            (90 * MIB, &[0x55, 0x66, 0x77, 0x88]),
        ],
    );
    rig.open_find();
    let id = rig.id;
    // Both asked in one turn, so the first cannot have landed before the second is asked.
    let first = rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_submit(id, FindMode::Hex, "55 66 77 88", true, window, cx);
        let first = app
            .editor
            .bar
            .as_ref()
            .and_then(|b| b.find.search.as_ref())
            .expect("the first search")
            .cancel
            .clone();
        assert!(!first.load(Ordering::Relaxed));
        app.hex_find_submit(id, FindMode::Hex, "11 22 33 44", true, window, cx);
        first
    });
    assert!(
        first.load(Ordering::Relaxed),
        "the first one was told to stop"
    );
    rig.searched();
    // Time for the first one's thread to answer, if it were going to be heard.
    for _ in 0..20 {
        rig.vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(rig.caret(), (100_000, Some(100_000..100_004)));
    assert_eq!(rig.note(), None, "the first one's not-found was not shown");
}

/// Guards `Search`'s `Drop`: closing the bar stops the search and nothing lands afterwards.
#[gpui::test]
fn closing_the_bar_cancels_the_search(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 96 * MIB, &[]);
    let (flag, _) = rig.start("aa bb cc dd");
    assert!(!flag.load(Ordering::Relaxed));
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_bar(window, cx)
    });
    assert!(flag.load(Ordering::Relaxed));
    for _ in 0..10 {
        rig.vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!rig.bar_is(|_| true));
    assert_eq!(rig.caret(), (0, None));
}

/// Guards the bar clearing in `editor_drop_hex`: closing the document stops its search.
#[gpui::test]
fn closing_the_document_cancels_the_search(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 96 * MIB, &[]);
    let (flag, _) = rig.start("aa bb cc dd");
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_file(0, window, cx)
    });
    rig.vcx.run_until_parked();
    let _ = rig.vcx.update(|window, cx| window.draw(cx));
    rig.vcx.run_until_parked();
    assert!(flag.load(Ordering::Relaxed));
    assert!(!rig.bar_is(|_| true));
}

/// Guards `SearchSnapshot::large`: a file over one read says "Searching…" while it reads, a
/// small one does not flash it.
#[gpui::test]
fn only_a_large_file_says_searching(cx: &mut TestAppContext) {
    let mut big = rig(cx, 4 * MIB, &[]);
    let (_, shown) = big.start("aa bb");
    assert_eq!(shown, Some(Note::Searching));
    assert_eq!(Note::Searching.text(), "Searching…");
    big.searched();
    assert_eq!(big.note(), Some(Note::NotFound));

    let mut small = rig(cx, 8000, &[]);
    let (_, shown) = small.start("aa bb");
    assert_eq!(shown, None, "a small file never says Searching");
    small.searched();
    assert_eq!(small.note(), Some(Note::NotFound));
}

/// Guards the `Whole` branch of `SearchSnapshot::find`: a remote document is searched from the
/// bytes it already holds, and its host is never asked to read.
#[gpui::test]
fn a_whole_document_is_searched_without_reading(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    crate::ui::i18n::set_locale("en");
    let host = Counting::new();
    let mut bytes = vec![0u8; 200_000];
    bytes[70_000..70_003].copy_from_slice(&[9, 8, 7]);
    let loaded = HexLoaded::File(HexFile {
        path: PathBuf::from("/x/remote.bin"),
        source: HexSource::Whole(Arc::from(bytes)),
        mtime: None,
        opens_as_text: false,
    });
    let installed = host.clone();
    let id = app.update_in(&mut vcx, |app, window, cx| {
        let tab = app.tabs[app.active].tree_id.get();
        app.editor_install_hex(installed, loaded, tab, true, window, cx)
            .expect("installed")
    });
    let mut rig = Rig {
        app,
        vcx,
        host: host.clone(),
        id,
        _dir: tempfile::tempdir().unwrap(),
        _streams,
    };
    rig.find("09 08 07", false);
    assert_eq!(rig.caret(), (70_000, Some(70_000..70_003)));
    rig.find("09 08 07", true);
    assert_eq!(
        rig.caret().0,
        70_000,
        "the only match, found again by wrapping"
    );
    assert!(
        host.reads().is_empty(),
        "no read_range for a whole document"
    );
}

/// Guards `hex_find_step`: F3 with the bar gone brings it back with the last query and goes to
/// the next match.
#[gpui::test]
fn f3_repeats_the_last_query_after_the_bar_closed(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[(100, &[0xaa]), (5000, &[0xaa])]);
    rig.find("aa", false);
    assert_eq!(rig.caret().0, 100);
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_bar(window, cx)
    });
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_step(true, window, cx)
    });
    rig.searched();
    assert_eq!(rig.caret().0, 5000);
    assert!(rig.bar_is(|k| matches!(k, BarKind::HexFind { .. })));
    let text = rig.app.read_with(&rig.vcx, |app, cx| {
        app.editor
            .bar
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(text, "aa");
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_step(false, window, cx)
    });
    rig.searched();
    assert_eq!(rig.caret().0, 100);
}

/// Guards `hex_find_step` with nothing searched yet: it asks for the bar.
#[gpui::test]
fn f3_with_no_query_opens_the_bar(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[]);
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_step(true, window, cx)
    });
    assert!(rig.bar_is(|k| matches!(k, BarKind::HexFind { .. })));
}

/// Guards `hex_find_mode` and the mode kept in `FindState`: the toggle changes what the bar
/// takes, and the next bar opens in it.
#[gpui::test]
fn the_mode_toggle_sticks_for_the_next_bar(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[]);
    rig.open_find();
    assert!(rig.bar_is(|k| matches!(
        k,
        BarKind::HexFind {
            mode: FindMode::Hex,
            ..
        }
    )));
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.hex_find_mode(FindMode::Text, window, cx)
    });
    assert!(rig.bar_is(|k| matches!(
        k,
        BarKind::HexFind {
            mode: FindMode::Text,
            ..
        }
    )));
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_bar(window, cx)
    });
    rig.open_find();
    assert!(rig.bar_is(|k| matches!(
        k,
        BarKind::HexFind {
            mode: FindMode::Text,
            ..
        }
    )));
}

/// Guards `hex_bar_tail`: a find bar draws its mode toggle and, when there is something to say,
/// the note; a go-to bar has no toggle.
#[gpui::test]
fn the_bar_draws_the_toggle_and_the_note(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[]);
    let draw = |rig: &mut Rig| {
        rig.vcx.run_until_parked();
        let _ = rig.vcx.update(|window, cx| window.draw(cx));
    };
    rig.open_go_to();
    draw(&mut rig);
    assert!(rig.vcx.debug_bounds("hex-find-mode").is_none());
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_bar(window, cx)
    });
    rig.find("bb", false);
    draw(&mut rig);
    assert!(rig.vcx.debug_bounds("hex-find-mode").is_some());
    assert!(rig.vcx.debug_bounds("editor-bar-note").is_some());
}

/// Guards the note's `flex_shrink(1.).min_w_0().truncate()` in `hex_bar_tail` and the field's
/// `min_w` in `render_editor_bar`: in a bar too narrow for a long note, the field keeps its
/// minimum width and the toggle stays whole, and the note is cut to what is left, all inside
/// the bar. With the note back to `flex_none` the field is squeezed to nothing and the note
/// ends past the bar's right edge. The bar is as wide as the docked editor is at the default
/// window, about 340pt.
#[gpui::test]
fn a_long_note_shrinks_instead_of_pushing_the_field_out(cx: &mut TestAppContext) {
    const DOCKED_BAR_W: f32 = 340.;
    fn bounds(vcx: &mut VisualTestContext, name: &'static str) -> gpui::Bounds<gpui::Pixels> {
        vcx.debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is drawn"))
    }
    let mut rig = rig(cx, 8000, &[]);
    let note_kind = Note::Offset(OffsetError::OutOfRange, u64::MAX);
    rig.vcx.simulate_resize(gpui::size(px(600.), px(600.)));
    rig.open_find();
    rig.app.update_in(&mut rig.vcx, |app, _, cx| {
        if let Some(bar) = app.editor.bar.as_mut() {
            bar.find.note = Some(note_kind.clone());
        }
        cx.notify();
    });
    rig.vcx.run_until_parked();
    let _ = rig.vcx.update(|window, cx| window.draw(cx));
    // The editor column is what the window leaves it: size the window so the bar, not the
    // window, is the realistic width.
    let first = bounds(&mut rig.vcx, "editor-bar").size.width.as_f32();
    rig.vcx
        .simulate_resize(gpui::size(px(600. + DOCKED_BAR_W - first), px(600.)));
    rig.vcx.run_until_parked();
    let _ = rig.vcx.update(|window, cx| window.draw(cx));
    let bar = bounds(&mut rig.vcx, "editor-bar");
    assert!(
        (bar.size.width.as_f32() - DOCKED_BAR_W).abs() < 1.,
        "the bar is {:?} wide, not the docked {DOCKED_BAR_W}",
        bar.size.width
    );
    let input = bounds(&mut rig.vcx, "editor-bar-input");
    let toggle = bounds(&mut rig.vcx, "hex-find-mode");
    let note = bounds(&mut rig.vcx, "editor-bar-note");
    let min = rig.vcx.update(|window, _| {
        gpui::rems(
            crate::ui::right_panel::META * crate::ui::code_editor::EDITOR_BAR_FIELD_MIN_CHARS,
        )
        .to_pixels(window.rem_size())
        .as_f32()
    });
    let full = rig.vcx.update(|window, _| {
        let size = gpui::rems(crate::ui::right_panel::META).to_pixels(window.rem_size());
        crate::ui::text_elision::measure_text(
            window.text_system(),
            &window.text_style().font(),
            size.as_f32(),
            &note_kind.text(),
        )
    });
    let at = format!(
        "bar {:?}..{:?}, field {:?}..{:?} (min {min}), toggle ..{:?}, note {:?}..{:?} (text {full})",
        bar.left(),
        bar.right(),
        input.left(),
        input.right(),
        toggle.right(),
        note.left(),
        note.right()
    );
    assert!(
        input.size.width.as_f32() >= min - 0.5,
        "field too narrow: {at}"
    );
    assert!(input.right() <= bar.right(), "field leaves the bar: {at}");
    assert!(toggle.right() <= bar.right(), "toggle leaves the bar: {at}");
    assert!(
        note.right() <= bar.right() + px(0.5),
        "note leaves the bar: {at}"
    );
    assert!(note.left() >= input.right(), "note over the field: {at}");
    assert!(
        note.size.width.as_f32() < full - 1.,
        "the note is not cut, so this proves nothing: {at}"
    );
}

/// Guards an empty file: nothing to go to, nothing to find, and no panic.
#[gpui::test]
fn an_empty_file_has_no_offset_and_no_match(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 0, &[]);
    rig.open_go_to();
    rig.enter("0", false);
    assert_eq!(rig.note(), Some(Note::Offset(OffsetError::OutOfRange, 0)));
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_close_bar(window, cx)
    });
    rig.find("aa", false);
    assert_eq!(rig.note(), Some(Note::NotFound));
}

/// Guards `editor_go_to_line` routing: the go-to-line command in front of a hex document asks
/// for an offset instead.
#[gpui::test]
fn go_to_line_in_a_hex_tab_asks_for_an_offset(cx: &mut TestAppContext) {
    let mut rig = rig(cx, 8000, &[]);
    rig.app.update_in(&mut rig.vcx, |app, window, cx| {
        app.editor_go_to_line(window, cx)
    });
    assert!(rig.bar_is(|k| matches!(k, BarKind::GoToOffset { .. })));
}
