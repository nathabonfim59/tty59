//! Pure model of the hex editor: a paged byte store with an edit overlay, the caret and its
//! moves, nibble/ASCII/paste editing with undo, patches for saving, the offset and pattern
//! parsers, chunked find, the value inspector and allocation-free row formatting. No gpui and
//! no IO: the host reads pages and the view paints rows from here.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Bytes per cached page.
pub(super) const PAGE: usize = 64 * 1024;
/// Bytes per displayed row.
pub(super) const ROW: u64 = 16;
/// Pages kept in the cache (16 MiB) before the least recently used go.
pub(super) const PAGE_CACHE_CAP: usize = 256;
/// Bytes read per step while searching.
pub(super) const FIND_CHUNK: usize = 1024 * 1024;

const PAGE_U64: u64 = PAGE as u64;
const HEX_DIGITS: [u8; 16] = *b"0123456789ABCDEF";

/// Widening conversion; `usize` is at most 64 bits on every supported target.
fn wide(n: usize) -> u64 {
    n as u64
}

// ---------------------------------------------------------------------------------------------
// Byte store

/// Where the bytes live: local files are paged in on demand, remote ones arrive in one read.
#[derive(Clone, Debug)]
pub(super) enum Source {
    Paged { len: u64 },
    Whole(Arc<[u8]>),
}

/// One byte as the view sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Byte {
    Loaded {
        value: u8,
        edited: bool,
    },
    /// Its page is not cached yet (or the offset is past the end).
    Pending,
}

/// Why an edit did not happen. The UI localizes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditRefusal {
    /// The file is empty, so there is no byte to edit.
    NothingToEdit,
    /// The byte's page has not been read yet; load it and retry.
    NotLoaded,
    NotHexDigit,
    /// Only printable ASCII (0x20..=0x7E) can be typed in the ASCII column.
    NotPrintable,
}

/// The file's bytes plus the user's unsaved edits.
///
/// `overlay` holds only bytes that differ from the disk, so `dirty()` is exact: writing the
/// original value back removes the entry.
pub(super) struct ByteStore {
    source: Source,
    pages: HashMap<u64, Arc<[u8]>>,
    /// Cached page indices, least recently used first.
    lru: VecDeque<u64>,
    cap: usize,
    generation: u64,
    overlay: BTreeMap<u64, u8>,
    caret_page: Option<u64>,
    /// Pages of a save in flight, kept cached until it lands.
    saving_pages: BTreeSet<u64>,
}

impl ByteStore {
    pub(super) fn new(source: Source) -> Self {
        Self::with_cache_cap(source, PAGE_CACHE_CAP)
    }

    pub(super) fn with_cache_cap(source: Source, cap: usize) -> Self {
        Self {
            source,
            pages: HashMap::new(),
            lru: VecDeque::new(),
            cap,
            generation: 0,
            overlay: BTreeMap::new(),
            caret_page: None,
            saving_pages: BTreeSet::new(),
        }
    }

    pub(super) fn len(&self) -> u64 {
        match &self.source {
            Source::Paged { len } => *len,
            Source::Whole(bytes) => wide(bytes.len()),
        }
    }

    pub(super) fn dirty(&self) -> bool {
        !self.overlay.is_empty()
    }

    pub(super) fn overlay(&self) -> &BTreeMap<u64, u8> {
        &self.overlay
    }

    pub(super) fn source(&self) -> &Source {
        &self.source
    }

    /// Page reads carry the generation they were requested under; see `insert_page`.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Drops every cached page and invalidates reads still in flight. Called when the file
    /// changed on disk. The overlay is untouched: the caller decides whether to keep it.
    pub(super) fn bump_generation(&mut self) {
        self.generation += 1;
        self.saving_pages.clear();
        self.pages.clear();
        self.lru.clear();
    }

    /// A reload with new contents: new generation, no pages, no edits.
    pub(super) fn set_source(&mut self, source: Source) {
        self.source = source;
        self.overlay.clear();
        self.bump_generation();
    }

    /// Tells the store which page holds the caret so it is never evicted.
    pub(super) fn pin_caret(&mut self, offset: u64) {
        self.caret_page = Some(offset / PAGE_U64);
    }

    /// The byte on disk, if its page is loaded.
    fn disk_byte(&self, off: u64) -> Option<u8> {
        match &self.source {
            Source::Whole(bytes) => bytes.get(usize::try_from(off).ok()?).copied(),
            Source::Paged { len } => {
                if off >= *len {
                    return None;
                }
                let page = self.pages.get(&(off / PAGE_U64))?;
                page.get(usize::try_from(off % PAGE_U64).ok()?).copied()
            }
        }
    }

    /// The byte with edits applied, if its page is loaded.
    pub(super) fn effective(&self, off: u64) -> Option<u8> {
        let disk = self.disk_byte(off)?;
        Some(self.overlay.get(&off).copied().unwrap_or(disk))
    }

    pub(super) fn byte(&self, off: u64) -> Byte {
        match self.disk_byte(off) {
            None => Byte::Pending,
            Some(disk) => match self.overlay.get(&off) {
                Some(&value) => Byte::Loaded {
                    value,
                    edited: true,
                },
                None => Byte::Loaded {
                    value: disk,
                    edited: false,
                },
            },
        }
    }

    /// Pages of the file that the given rows touch (empty for a whole buffer).
    fn page_span(&self, rows: Range<u64>) -> Range<u64> {
        let Source::Paged { len } = &self.source else {
            return 0..0;
        };
        let first = rows.start.saturating_mul(ROW);
        let last = rows.end.saturating_mul(ROW).min(*len);
        if first >= last {
            return 0..0;
        }
        first / PAGE_U64..(last - 1) / PAGE_U64 + 1
    }

    /// Pages the given rows need that are not cached.
    pub(super) fn missing_pages(&self, rows: Range<u64>) -> Vec<u64> {
        self.page_span(rows)
            .filter(|idx| !self.pages.contains_key(idx))
            .collect()
    }

    /// Marks the cached pages under `rows` as recently used. Painting only reads the cache, so
    /// the scroll and caret handlers call this to keep the visible pages alive.
    pub(super) fn touch_rows(&mut self, rows: Range<u64>) {
        for idx in self.page_span(rows) {
            if self.pages.contains_key(&idx) {
                self.touch(idx);
            }
        }
    }

    fn touch(&mut self, idx: u64) {
        if let Some(pos) = self.lru.iter().position(|&p| p == idx) {
            self.lru.remove(pos);
        }
        self.lru.push_back(idx);
    }

    /// Stores a page read for `generation`. A page from an older generation (the file was
    /// reloaded meanwhile), one past the end, or of the wrong length is ignored; the last page
    /// of a file is legitimately short. Returns whether it was stored.
    pub(super) fn insert_page(
        &mut self,
        generation: u64,
        idx: u64,
        bytes: impl Into<Arc<[u8]>>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let Source::Paged { len } = &self.source else {
            return false;
        };
        let len = *len;
        let start = idx.saturating_mul(PAGE_U64);
        if start >= len {
            return false;
        }
        let bytes: Arc<[u8]> = bytes.into();
        if wide(bytes.len()) != (len - start).min(PAGE_U64) {
            return false;
        }
        self.pages.insert(idx, bytes);
        self.touch(idx);
        self.evict();
        true
    }

    /// A page holding the caret or any edit must stay: edits can only be reverted against the
    /// disk byte, and the caret page is what the user is looking at.
    fn is_pinned(&self, idx: u64) -> bool {
        if self.caret_page == Some(idx) || self.saving_pages.contains(&idx) {
            return true;
        }
        let start = idx.saturating_mul(PAGE_U64);
        let end = start.saturating_add(PAGE_U64);
        self.overlay.range(start..end).next().is_some()
    }

    fn evict(&mut self) {
        while self.pages.len() > self.cap {
            let Some(pos) = self.lru.iter().position(|&idx| !self.is_pinned(idx)) else {
                return;
            };
            if let Some(idx) = self.lru.remove(pos) {
                self.pages.remove(&idx);
            }
        }
    }

    /// Sets the edited value of one byte; the original value removes the edit.
    fn set(&mut self, off: u64, value: u8) -> Result<(), EditRefusal> {
        let disk = self.disk_byte(off).ok_or(EditRefusal::NotLoaded)?;
        if value == disk {
            self.overlay.remove(&off);
        } else {
            self.overlay.insert(off, value);
        }
        Ok(())
    }

    /// Restores a byte to a recorded overlay state: `None` is "not edited".
    fn set_state(&mut self, off: u64, state: Option<u8>) -> Result<(), EditRefusal> {
        match state {
            Some(value) => self.set(off, value),
            None => {
                self.overlay.remove(&off);
                Ok(())
            }
        }
    }

    fn page_missing(&self, idx: u64) -> bool {
        matches!(self.source, Source::Paged { .. }) && !self.pages.contains_key(&idx)
    }

    /// Freezes the current edits for a save and keeps their pages cached until `fold_written`
    /// or `end_save`: a save is asynchronous, and by the time it lands the user may have
    /// reverted a byte (dropping its overlay entry and so its page's pin), yet the fold needs
    /// the old disk byte to keep that revert.
    fn begin_save(&mut self) -> SaveSnapshot {
        let written = self.overlay.clone();
        self.saving_pages = written.keys().map(|off| off / PAGE_U64).collect();
        let whole = match &self.source {
            Source::Whole(buf) => {
                let mut bytes = buf.to_vec();
                for (&off, &value) in &written {
                    let slot = usize::try_from(off).ok().and_then(|i| bytes.get_mut(i));
                    if let Some(slot) = slot {
                        *slot = value;
                    }
                }
                Some(Arc::from(bytes))
            }
            Source::Paged { .. } => None,
        };
        SaveSnapshot {
            written,
            whole,
            generation: self.generation,
        }
    }

    /// The save failed or was abandoned: the pages need no more protection.
    fn end_save(&mut self) {
        self.saving_pages.clear();
    }

    /// Makes the written bytes the disk contents of the cached pages (or the whole buffer),
    /// returning the disk byte each had before. The overlay is reconciled against the new disk
    /// value, per offset: an edit equal to what was written is now just the disk (removed); a
    /// different edit made while the save ran stays; a byte reverted to its old disk value in
    /// the meantime (no overlay entry) becomes an edit again, since the disk no longer has it.
    /// Offsets that were not written are untouched. Bytes whose page is not cached are skipped;
    /// `begin_save` keeps those pages, so that does not happen through the normal paths.
    fn fold_written(&mut self, written: &BTreeMap<u64, u8>) -> BTreeMap<u64, u8> {
        let mut old = BTreeMap::new();
        if let Source::Whole(buf) = &mut self.source {
            let mut bytes = buf.to_vec();
            for (&off, &value) in written {
                let slot = usize::try_from(off).ok().and_then(|i| bytes.get_mut(i));
                if let Some(slot) = slot {
                    old.insert(off, *slot);
                    *slot = value;
                }
            }
            *buf = Arc::from(bytes);
        } else {
            let mut touched: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
            for (&off, &value) in written {
                let idx = off / PAGE_U64;
                let Some(page) = self.pages.get(&idx) else {
                    continue;
                };
                let Ok(i) = usize::try_from(off % PAGE_U64) else {
                    continue;
                };
                let bytes = touched.entry(idx).or_insert_with(|| page.to_vec());
                if let Some(slot) = bytes.get_mut(i) {
                    old.insert(off, *slot);
                    *slot = value;
                }
            }
            for (idx, bytes) in touched {
                self.pages.insert(idx, Arc::from(bytes));
            }
        }
        for (&off, &previous) in &old {
            let Some(&value) = written.get(&off) else {
                continue;
            };
            let current = self.overlay.get(&off).copied().unwrap_or(previous);
            if current == value {
                self.overlay.remove(&off);
            } else {
                self.overlay.insert(off, current);
            }
        }
        self.saving_pages.clear();
        old
    }

    /// The overlay as contiguous runs, ascending. A save writes its snapshot's runs instead,
    /// so only the tests read the live overlay this way.
    #[cfg(test)]
    pub(super) fn patches(&self) -> Vec<(u64, Vec<u8>)> {
        runs(&self.overlay)
    }
}

/// What a save writes: the edits as they were when it started. `apply_saved` folds exactly
/// these, so edits made while the save is in flight are not mistaken for saved ones.
pub(super) struct SaveSnapshot {
    written: BTreeMap<u64, u8>,
    /// For a whole-buffer source: the bytes with the edits applied, to write as the file.
    pub(super) whole: Option<Arc<[u8]>>,
    /// A reload in between makes the snapshot stale: it describes bytes that are gone.
    generation: u64,
}

impl SaveSnapshot {
    /// The written bytes as contiguous runs, ascending, for a paged save.
    pub(super) fn patches(&self) -> Vec<(u64, Vec<u8>)> {
        runs(&self.written)
    }
}

fn runs(bytes: &BTreeMap<u64, u8>) -> Vec<(u64, Vec<u8>)> {
    let mut runs: Vec<(u64, Vec<u8>)> = Vec::new();
    for (&off, &value) in bytes {
        match runs.last_mut() {
            Some((start, run)) if *start + wide(run.len()) == off => run.push(value),
            _ => runs.push((off, vec![value])),
        }
    }
    runs
}

// ---------------------------------------------------------------------------------------------
// Caret

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Side {
    #[default]
    Hex,
    Ascii,
}

/// The caret, its nibble and column, and the selection anchor. All moves are pure and take the
/// file length; with `extend` they keep or set the anchor, without it they drop the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Caret {
    pub(super) offset: u64,
    /// The next hex digit typed is the low nibble of `offset`.
    pub(super) low_nibble: bool,
    pub(super) side: Side,
    pub(super) anchor: Option<u64>,
}

impl Caret {
    /// The selected bytes, both ends included, so an anchor on the caret selects that one byte
    /// (shift+arrow can copy exactly one byte). A plain move clears the anchor.
    pub(super) fn selection(&self) -> Option<Range<u64>> {
        let anchor = self.anchor?;
        Some(anchor.min(self.offset)..anchor.max(self.offset) + 1)
    }

    pub(super) fn toggle_side(&mut self) {
        self.side = match self.side {
            Side::Hex => Side::Ascii,
            Side::Ascii => Side::Hex,
        };
        self.low_nibble = false;
    }

    fn go(&mut self, target: u64, extend: bool, len: u64) {
        if len == 0 {
            *self = Caret {
                side: self.side,
                ..Caret::default()
            };
            return;
        }
        if extend {
            self.anchor.get_or_insert(self.offset);
        } else {
            self.anchor = None;
        }
        self.offset = target.min(len - 1);
        self.low_nibble = false;
    }

    fn go_rows(&mut self, up: bool, rows: u64, extend: bool, len: u64) {
        let row = self.offset / ROW;
        let col = self.offset % ROW;
        let last_row = len.saturating_sub(1) / ROW;
        let row = if up {
            row.saturating_sub(rows)
        } else {
            row.saturating_add(rows).min(last_row)
        };
        self.go(row * ROW + col, extend, len);
    }

    pub(super) fn left(&mut self, len: u64, extend: bool) {
        self.go(self.offset.saturating_sub(1), extend, len);
    }

    pub(super) fn right(&mut self, len: u64, extend: bool) {
        self.go(self.offset.saturating_add(1), extend, len);
    }

    pub(super) fn up(&mut self, len: u64, extend: bool) {
        self.go_rows(true, 1, extend, len);
    }

    pub(super) fn down(&mut self, len: u64, extend: bool) {
        self.go_rows(false, 1, extend, len);
    }

    pub(super) fn page_up(&mut self, rows: u64, len: u64, extend: bool) {
        self.go_rows(true, rows, extend, len);
    }

    pub(super) fn page_down(&mut self, rows: u64, len: u64, extend: bool) {
        self.go_rows(false, rows, extend, len);
    }

    pub(super) fn row_start(&mut self, len: u64, extend: bool) {
        self.go(self.offset - self.offset % ROW, extend, len);
    }

    pub(super) fn row_end(&mut self, len: u64, extend: bool) {
        self.go(self.offset - self.offset % ROW + (ROW - 1), extend, len);
    }

    pub(super) fn doc_start(&mut self, len: u64, extend: bool) {
        self.go(0, extend, len);
    }

    pub(super) fn doc_end(&mut self, len: u64, extend: bool) {
        self.go(u64::MAX, extend, len);
    }
}

// ---------------------------------------------------------------------------------------------
// Editing

/// What a paste did: how many bytes it wrote and whether the rest ran past the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PasteResult {
    pub(super) written: usize,
    pub(super) truncated: bool,
}

/// Caret position that decides whether typing continues the open undo step.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Pos {
    offset: u64,
    low_nibble: bool,
    side: Side,
}

impl Pos {
    fn of(caret: &Caret) -> Self {
        Self {
            offset: caret.offset,
            low_nibble: caret.low_nibble,
            side: caret.side,
        }
    }
}

/// One undo step: the overlay state of a run of bytes before and after. Overlay state, not
/// byte values, so undo returns a byte to "not edited" exactly (`None`).
struct Step {
    offset: u64,
    before: Vec<Option<u8>>,
    after: Vec<Option<u8>>,
    caret_before: Caret,
    caret_after: Caret,
}

impl Step {
    fn end(&self) -> u64 {
        self.offset + wide(self.before.len())
    }

    /// Folds a run that starts inside or right after this step into it, keeping the first
    /// `before` of every byte. False when the run is not adjacent.
    fn try_merge(
        &mut self,
        start: u64,
        before: &[Option<u8>],
        after: &[Option<u8>],
        caret_after: Caret,
    ) -> bool {
        if start < self.offset || start > self.end() {
            return false;
        }
        let Ok(base) = usize::try_from(start - self.offset) else {
            return false;
        };
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            match self.after.get_mut(base + i) {
                Some(slot) => *slot = *a,
                None => {
                    self.before.push(*b);
                    self.after.push(*a);
                }
            }
        }
        self.caret_after = caret_after;
        true
    }
}

#[derive(Default)]
struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Where the caret must be for the next typing to extend `undo.last()`.
    open: Option<Pos>,
}

/// The store, the caret and the undo history of one hex document.
pub(super) struct HexModel {
    pub(super) store: ByteStore,
    pub(super) caret: Caret,
    history: History,
}

impl HexModel {
    pub(super) fn new(source: Source) -> Self {
        Self {
            store: ByteStore::new(source),
            caret: Caret::default(),
            history: History::default(),
        }
    }

    /// The file changed on disk: new contents, no edits, no history, caret kept in range.
    pub(super) fn reload(&mut self, source: Source) {
        self.store.set_source(source);
        self.history = History::default();
        let len = self.store.len();
        self.caret.go(self.caret.offset, false, len);
    }

    /// Like `ByteStore::insert_page`, with the caret's page pinned first.
    pub(super) fn insert_page(
        &mut self,
        generation: u64,
        idx: u64,
        bytes: impl Into<Arc<[u8]>>,
    ) -> bool {
        self.store.pin_caret(self.caret.offset);
        self.store.insert_page(generation, idx, bytes)
    }

    /// Closes the open undo step, so the next typing starts a new one.
    pub(super) fn end_step(&mut self) {
        self.history.open = None;
    }

    /// Switches column. A side toggle closes the undo step even when the caret lands on the
    /// same offset, which comparing positions alone would not notice.
    pub(super) fn toggle_side(&mut self) {
        self.caret.toggle_side();
        self.end_step();
    }

    /// Types one hex digit given as its ASCII character: the high nibble first, then the low
    /// one, after which the caret advances (and stays on the last byte at the end).
    pub(super) fn type_hex(&mut self, digit: u8) -> Result<(), EditRefusal> {
        let value = hex_value(digit).ok_or(EditRefusal::NotHexDigit)?;
        let len = self.store.len();
        if len == 0 {
            return Err(EditRefusal::NothingToEdit);
        }
        let off = self.caret.offset.min(len - 1);
        let current = self.store.effective(off).ok_or(EditRefusal::NotLoaded)?;
        let before = self.caret;
        let mut after = before;
        after.anchor = None;
        let new = if before.low_nibble {
            after.offset = (off + 1).min(len - 1);
            after.low_nibble = false;
            (current & 0xF0) | value
        } else {
            after.low_nibble = true;
            (value << 4) | (current & 0x0F)
        };
        self.commit(off, &[new], true, before, after)
    }

    /// Types one printable ASCII character over the caret's byte and advances.
    pub(super) fn type_ascii(&mut self, ch: char) -> Result<(), EditRefusal> {
        let byte = u8::try_from(ch)
            .ok()
            .filter(|b| (0x20..=0x7E).contains(b))
            .ok_or(EditRefusal::NotPrintable)?;
        let len = self.store.len();
        if len == 0 {
            return Err(EditRefusal::NothingToEdit);
        }
        let off = self.caret.offset.min(len - 1);
        let before = self.caret;
        let mut after = before;
        after.anchor = None;
        after.offset = (off + 1).min(len - 1);
        after.low_nibble = false;
        self.commit(off, &[byte], true, before, after)
    }

    /// Overwrites from the selection start (or the caret) without growing the file; what does
    /// not fit is cut and reported. One undo step. Refused whole when a target page is not
    /// loaded yet.
    pub(super) fn paste(&mut self, bytes: &[u8]) -> Result<PasteResult, EditRefusal> {
        let len = self.store.len();
        if len == 0 {
            return Err(EditRefusal::NothingToEdit);
        }
        if bytes.is_empty() {
            return Ok(PasteResult {
                written: 0,
                truncated: false,
            });
        }
        let start = self
            .caret
            .selection()
            .map_or(self.caret.offset, |range| range.start)
            .min(len - 1);
        let room = usize::try_from(len - start).map_or(bytes.len(), |r| r.min(bytes.len()));
        let mut before = self.caret;
        before.offset = start;
        let mut after = before;
        after.anchor = None;
        after.low_nibble = false;
        after.offset = (start + wide(room)).min(len - 1);
        self.commit(start, &bytes[..room], false, before, after)?;
        Ok(PasteResult {
            written: room,
            truncated: room < bytes.len(),
        })
    }

    /// Writes `new` from `start`, records it for undo and moves the caret. All target bytes must
    /// be loaded; nothing is changed otherwise. `coalesce` lets typing extend the open step.
    fn commit(
        &mut self,
        start: u64,
        new: &[u8],
        coalesce: bool,
        mut caret_before: Caret,
        caret_after: Caret,
    ) -> Result<(), EditRefusal> {
        for i in 0..new.len() {
            if self.store.effective(start + wide(i)).is_none() {
                return Err(EditRefusal::NotLoaded);
            }
        }
        caret_before.anchor = None;
        let mut before = Vec::with_capacity(new.len());
        let mut after = Vec::with_capacity(new.len());
        for (i, &value) in new.iter().enumerate() {
            let off = start + wide(i);
            before.push(self.store.overlay.get(&off).copied());
            self.store.set(off, value)?;
            after.push(self.store.overlay.get(&off).copied());
        }
        self.caret = caret_after;
        if before == after {
            self.history.open = None;
            return Ok(());
        }
        self.history.redo.clear();
        let extend = coalesce && self.history.open == Some(Pos::of(&caret_before));
        let merged = extend
            && self
                .history
                .undo
                .last_mut()
                .is_some_and(|top| top.try_merge(start, &before, &after, caret_after));
        if !merged {
            self.history.undo.push(Step {
                offset: start,
                before,
                after,
                caret_before,
                caret_after,
            });
        }
        self.history.open = coalesce.then(|| Pos::of(&caret_after));
        Ok(())
    }

    /// Pages the next undo needs and does not have: restoring an edited value is checked
    /// against the disk byte, so its page must be cached.
    pub(super) fn undo_missing_pages(&self) -> Vec<u64> {
        self.history
            .undo
            .last()
            .map_or_else(Vec::new, |s| self.pages_needed(s.offset, &s.before))
    }

    /// Same for the next redo.
    pub(super) fn redo_missing_pages(&self) -> Vec<u64> {
        self.history
            .redo
            .last()
            .map_or_else(Vec::new, |s| self.pages_needed(s.offset, &s.after))
    }

    fn pages_needed(&self, offset: u64, states: &[Option<u8>]) -> Vec<u64> {
        let mut missing = Vec::new();
        for (i, state) in states.iter().enumerate() {
            let idx = (offset + wide(i)) / PAGE_U64;
            if state.is_some() && self.store.page_missing(idx) && !missing.contains(&idx) {
                missing.push(idx);
            }
        }
        missing
    }

    fn apply_states(&mut self, offset: u64, states: &[Option<u8>]) -> Result<(), EditRefusal> {
        if !self.pages_needed(offset, states).is_empty() {
            return Err(EditRefusal::NotLoaded);
        }
        for (i, &state) in states.iter().enumerate() {
            self.store.set_state(offset + wide(i), state)?;
        }
        Ok(())
    }

    /// Undoes the last step. `Ok(false)` when there is none; `NotLoaded` (nothing changed) when
    /// a page must be read first, see `undo_missing_pages`.
    pub(super) fn undo(&mut self) -> Result<bool, EditRefusal> {
        let Some(step) = self.history.undo.pop() else {
            return Ok(false);
        };
        if let Err(refusal) = self.apply_states(step.offset, &step.before) {
            self.history.undo.push(step);
            return Err(refusal);
        }
        self.caret = step.caret_before;
        self.history.open = None;
        self.history.redo.push(step);
        Ok(true)
    }

    pub(super) fn redo(&mut self) -> Result<bool, EditRefusal> {
        let Some(step) = self.history.redo.pop() else {
            return Ok(false);
        };
        if let Err(refusal) = self.apply_states(step.offset, &step.after) {
            self.history.redo.push(step);
            return Err(refusal);
        }
        self.caret = step.caret_after;
        self.history.open = None;
        self.history.undo.push(step);
        Ok(true)
    }

    #[cfg(test)]
    pub(super) fn patches(&self) -> Vec<(u64, Vec<u8>)> {
        self.store.patches()
    }

    /// Freezes the edits for a save: patches for a paged file, the edited buffer for a whole
    /// one. Edits made afterwards are not part of it; hand it back to `apply_saved` when the
    /// write succeeded or to `abort_save` when it did not.
    pub(super) fn save_snapshot(&mut self) -> SaveSnapshot {
        self.store.begin_save()
    }

    /// The save failed or was dropped; the edits stay as they are.
    pub(super) fn abort_save(&mut self) {
        self.store.end_save();
    }

    /// After a successful save: only the bytes the snapshot wrote become disk contents (see
    /// `ByteStore::fold_written` for how the overlay is reconciled), so edits typed while the
    /// save ran stay dirty with their values.
    ///
    /// The undo history stays usable. Its entries are overlay states, and `None` ("not edited")
    /// would now mean the *saved* byte; so every `None` at an offset that was just written is
    /// rewritten to the byte the disk held before, an absolute value. Undoing then marks the
    /// byte edited again (dirty), and redoing back to the saved value clears the edit.
    pub(super) fn apply_saved(&mut self, snapshot: &SaveSnapshot) {
        if snapshot.generation != self.store.generation() {
            return;
        }
        let old = self.store.fold_written(&snapshot.written);
        for step in self.history.undo.iter_mut().chain(&mut self.history.redo) {
            let offset = step.offset;
            for states in [&mut step.before, &mut step.after] {
                for (i, state) in states.iter_mut().enumerate() {
                    if state.is_none()
                        && let Some(&disk) = old.get(&(offset + wide(i)))
                    {
                        *state = Some(disk);
                    }
                }
            }
        }
        self.history.open = None;
    }
}

fn hex_value(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
}

// ---------------------------------------------------------------------------------------------
// Parsers

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OffsetError {
    Empty,
    Invalid,
    /// Past the end of the file, before its start, or too large for 64 bits.
    OutOfRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PatternError {
    Empty,
    OddDigits,
    InvalidDigit,
}

fn parse_number(text: &str) -> Result<u64, OffsetError> {
    let (digits, radix) = if let Some(rest) = text.strip_prefix("0x").or(text.strip_prefix("0X")) {
        (rest, 16)
    } else if let Some(rest) = text.strip_suffix('h').or(text.strip_suffix('H')) {
        (rest, 16)
    } else {
        (text, 10)
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || !digits.chars().all(|c| c == '_' || c.is_digit(radix))
    {
        return Err(OffsetError::Invalid);
    }
    let clean: String = digits.chars().filter(|&c| c != '_').collect();
    // Every digit is valid for the radix, so the only failure left is overflow.
    u64::from_str_radix(&clean, radix).map_err(|_| OffsetError::OutOfRange)
}

/// Go-to input: `0x1F`, `1Fh`, `31`, or `+16` / `-16` relative to the caret; surrounding
/// whitespace and `_` between digits are ignored. The result must be inside the file.
pub(super) fn parse_offset(input: &str, caret: u64, len: u64) -> Result<u64, OffsetError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(OffsetError::Empty);
    }
    let target = if let Some(rest) = text.strip_prefix('+') {
        caret
            .checked_add(parse_number(rest.trim_start())?)
            .ok_or(OffsetError::OutOfRange)?
    } else if let Some(rest) = text.strip_prefix('-') {
        caret
            .checked_sub(parse_number(rest.trim_start())?)
            .ok_or(OffsetError::OutOfRange)?
    } else {
        parse_number(text)?
    };
    if target >= len {
        return Err(OffsetError::OutOfRange);
    }
    Ok(target)
}

/// Find input: hex bytes separated by spaces or `:`, or run together; `0x` prefixes allowed.
/// Every group must hold whole bytes.
pub(super) fn parse_hex_pattern(input: &str) -> Result<Vec<u8>, PatternError> {
    let mut out = Vec::new();
    let mut any = false;
    for token in input.split(|c: char| c.is_whitespace() || c == ':') {
        if token.is_empty() {
            continue;
        }
        any = true;
        let digits = token
            .strip_prefix("0x")
            .or(token.strip_prefix("0X"))
            .unwrap_or(token);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PatternError::InvalidDigit);
        }
        if digits.len() % 2 != 0 {
            return Err(PatternError::OddDigits);
        }
        for pair in digits.as_bytes().chunks(2) {
            let (Some(hi), Some(lo)) = (
                pair.first().copied().and_then(hex_value),
                pair.get(1).copied().and_then(hex_value),
            ) else {
                return Err(PatternError::InvalidDigit);
            };
            out.push((hi << 4) | lo);
        }
    }
    if any {
        Ok(out)
    } else {
        Err(PatternError::Empty)
    }
}

// ---------------------------------------------------------------------------------------------
// Find

/// Random-access bytes for searching: a slice, or the host's file.
pub(super) trait ByteSource {
    fn len(&self) -> u64;
    /// Reads at `off` into `buf`; fewer bytes than asked only at the end (0 at or past it).
    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize>;
}

impl ByteSource for [u8] {
    fn len(&self) -> u64 {
        wide(<[u8]>::len(self))
    }

    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Ok(start) = usize::try_from(off) else {
            return Ok(0);
        };
        let Some(rest) = self.get(start..) else {
            return Ok(0);
        };
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> u64 {
        wide(<[u8]>::len(self))
    }

    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.as_slice().read(off, buf)
    }
}

/// A source with the unsaved edits applied, so find sees what the view shows.
pub(super) struct Overlaid<'a, S: ByteSource + ?Sized> {
    pub(super) src: &'a S,
    pub(super) overlay: &'a BTreeMap<u64, u8>,
}

impl<S: ByteSource + ?Sized> ByteSource for Overlaid<'_, S> {
    fn len(&self) -> u64 {
        self.src.len()
    }

    fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read(off, buf)?;
        let end = off.saturating_add(wide(n));
        for (&at, &value) in self.overlay.range(off..end) {
            let slot = usize::try_from(at - off).ok().and_then(|i| buf.get_mut(i));
            if let Some(slot) = slot {
                *slot = value;
            }
        }
        Ok(n)
    }
}

fn read_full(src: &dyn ByteSource, off: u64, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match src.read(off + wide(got), &mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// The first match starting in `lo..hi`. Chunks overlap by `pat.len() - 1` so a match across a
/// chunk boundary is seen. A cancelled search reports no match.
fn scan_forward(
    src: &dyn ByteSource,
    pat: &[u8],
    lo: u64,
    hi: u64,
    cancel: &AtomicBool,
    buf: &mut [u8],
) -> io::Result<Option<u64>> {
    let m = pat.len();
    let mut pos = lo;
    while pos < hi {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let n = read_full(src, pos, buf)?;
        if n < m {
            return Ok(None);
        }
        if let Some(i) = buf[..n].windows(m).position(|w| w == pat) {
            let found = pos + wide(i);
            return Ok((found < hi).then_some(found));
        }
        if n < buf.len() {
            return Ok(None);
        }
        pos += wide(n - (m - 1));
    }
    Ok(None)
}

/// The last match starting in `lo..hi`, scanning chunks from the end.
fn scan_backward(
    src: &dyn ByteSource,
    pat: &[u8],
    lo: u64,
    hi: u64,
    cancel: &AtomicBool,
    buf: &mut [u8],
) -> io::Result<Option<u64>> {
    if lo >= hi {
        return Ok(None);
    }
    let m = wide(pat.len());
    let mut end = hi.saturating_add(m - 1).min(src.len());
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let start = end.saturating_sub(wide(buf.len())).max(lo);
        if end.saturating_sub(start) < m {
            return Ok(None);
        }
        let span = usize::try_from(end - start)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let got = read_full(src, start, &mut buf[..span])?;
        if let Some(i) = buf[..got].windows(pat.len()).rposition(|w| w == pat) {
            return Ok(Some(start + wide(i)));
        }
        if start == lo {
            return Ok(None);
        }
        end = start + (m - 1);
    }
}

/// Finds `pat` starting at `from` (itself included) going forward or backward, optionally
/// wrapping around the end. Reads 1 MiB at a time and checks `cancel` before each chunk; a
/// cancelled search returns `Ok(None)`, so the caller looks at the flag to tell it from "none".
pub(super) fn find(
    src: &dyn ByteSource,
    pat: &[u8],
    from: u64,
    forward: bool,
    wrap: bool,
    cancel: &AtomicBool,
) -> io::Result<Option<u64>> {
    let len = src.len();
    let m = wide(pat.len());
    if pat.is_empty() || m > len {
        return Ok(None);
    }
    // Match starts are `0..end`: a match must fit before the end of the file.
    let end = len - m + 1;
    let mut buf = vec![0u8; FIND_CHUNK.max(pat.len() * 2)];
    let from = from.min(end);
    if forward {
        if let Some(found) = scan_forward(src, pat, from, end, cancel, &mut buf)? {
            return Ok(Some(found));
        }
        if wrap {
            return scan_forward(src, pat, 0, from, cancel, &mut buf);
        }
    } else {
        let top = from.saturating_add(1).min(end);
        if let Some(found) = scan_backward(src, pat, 0, top, cancel, &mut buf)? {
            return Ok(Some(found));
        }
        if wrap {
            return scan_backward(src, pat, top, end, cancel, &mut buf);
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------------------------
// Value inspector and row formatting

/// The bytes at the caret read as numbers; a field is `None` when the bytes run out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Values {
    pub(super) u8: Option<u8>,
    pub(super) i8: Option<i8>,
    pub(super) u16le: Option<u16>,
    pub(super) u16be: Option<u16>,
    pub(super) u32le: Option<u32>,
    pub(super) u32be: Option<u32>,
}

fn head<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    bytes.get(..N)?.try_into().ok()
}

pub(super) fn inspect(bytes: &[u8]) -> Values {
    Values {
        u8: bytes.first().copied(),
        i8: head::<1>(bytes).map(i8::from_le_bytes),
        u16le: head::<2>(bytes).map(u16::from_le_bytes),
        u16be: head::<2>(bytes).map(u16::from_be_bytes),
        u32le: head::<4>(bytes).map(u32::from_le_bytes),
        u32be: head::<4>(bytes).map(u32::from_be_bytes),
    }
}

/// Appends the offset as 8 hex digits, or 16 when `wide` (files over 4 GiB). Digits beyond
/// the width are cut.
pub(super) fn write_offset(out: &mut String, off: u64, wide: bool) {
    let digits: u32 = if wide { 16 } else { 8 };
    for i in (0..digits).rev() {
        // Masked to a nibble, so the index is in range.
        let nibble = ((off >> (i * 4)) & 0xF) as usize;
        out.push(char::from(HEX_DIGITS[nibble]));
    }
}

pub(super) fn hex_byte(b: u8) -> [u8; 2] {
    [
        HEX_DIGITS[usize::from(b >> 4)],
        HEX_DIGITS[usize::from(b & 0x0F)],
    ]
}

/// The ASCII column's character: printable ASCII as is, `.` for everything else.
pub(super) fn ascii_char(b: u8) -> char {
    if (0x20..=0x7E).contains(&b) {
        char::from(b)
    } else {
        '.'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(bytes: &[u8]) -> HexModel {
        HexModel::new(Source::Whole(Arc::from(bytes)))
    }

    fn value(m: &HexModel, off: u64) -> u8 {
        match m.store.byte(off) {
            Byte::Loaded { value, .. } => value,
            Byte::Pending => panic!("byte {off} is pending"),
        }
    }

    fn edited(m: &HexModel, off: u64) -> bool {
        matches!(m.store.byte(off), Byte::Loaded { edited: true, .. })
    }

    fn type_all(m: &mut HexModel, digits: &str) {
        for d in digits.bytes() {
            m.type_hex(d).unwrap();
        }
    }

    fn page_rows() -> u64 {
        PAGE_U64 / ROW
    }

    fn paged(len: u64, cap: usize) -> ByteStore {
        ByteStore::with_cache_cap(Source::Paged { len }, cap)
    }

    fn page(store: &mut ByteStore, idx: u64) -> bool {
        let g = store.generation();
        store.insert_page(g, idx, vec![0u8; PAGE])
    }

    // --- overlay -------------------------------------------------------------------------------

    #[test]
    fn typing_a_different_byte_makes_it_an_edit_and_the_store_dirty() {
        let mut m = model(&[0x10, 0x20]);
        assert!(!m.store.dirty());
        type_all(&mut m, "2");
        assert!(m.store.dirty());
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x20,
                edited: true
            }
        );
        assert_eq!(
            m.store.byte(1),
            Byte::Loaded {
                value: 0x20,
                edited: false
            }
        );
    }

    #[test]
    fn writing_the_original_byte_back_is_not_an_edit() {
        let mut m = model(&[0x10, 0x20]);
        type_all(&mut m, "20");
        assert!(m.store.dirty());
        m.caret.doc_start(2, false);
        type_all(&mut m, "10");
        assert!(!m.store.dirty());
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x10,
                edited: false
            }
        );
        assert!(m.patches().is_empty());
    }

    // --- typing --------------------------------------------------------------------------------

    #[test]
    fn typing_two_hex_digits_sets_high_then_low_nibble_and_advances() {
        let mut m = model(&[0, 0, 0]);
        m.type_hex(b'4').unwrap();
        assert_eq!(value(&m, 0), 0x40);
        assert_eq!((m.caret.offset, m.caret.low_nibble), (0, true));
        m.type_hex(b'1').unwrap();
        assert_eq!(value(&m, 0), 0x41);
        assert_eq!((m.caret.offset, m.caret.low_nibble), (1, false));
    }

    #[test]
    fn typing_keeps_the_other_nibble_of_the_byte() {
        let mut m = model(&[0xAB]);
        m.type_hex(b'f').unwrap();
        assert_eq!(value(&m, 0), 0xFB);
        m.type_hex(b'0').unwrap();
        assert_eq!(value(&m, 0), 0xF0);
    }

    #[test]
    fn typing_on_the_last_byte_stays_there_after_the_low_nibble() {
        let mut m = model(&[0, 0]);
        m.caret.right(2, false);
        type_all(&mut m, "ab");
        assert_eq!(value(&m, 1), 0xAB);
        assert_eq!((m.caret.offset, m.caret.low_nibble), (1, false));
        type_all(&mut m, "cd");
        assert_eq!(value(&m, 1), 0xCD);
        assert_eq!(m.caret.offset, 1);
    }

    #[test]
    fn typing_something_that_is_not_a_hex_digit_is_refused() {
        let mut m = model(&[0]);
        assert_eq!(m.type_hex(b'g'), Err(EditRefusal::NotHexDigit));
        assert_eq!(m.type_hex(b' '), Err(EditRefusal::NotHexDigit));
        assert!(!m.store.dirty());
        m.type_hex(b'F').unwrap();
        assert_eq!(value(&m, 0), 0xF0);
    }

    #[test]
    fn ascii_typing_writes_printable_characters_and_refuses_the_rest() {
        let mut m = model(&[0, 0, 0, 0]);
        m.type_ascii('A').unwrap();
        assert_eq!(value(&m, 0), 0x41);
        assert_eq!(m.caret.offset, 1);
        m.type_ascii(' ').unwrap();
        m.type_ascii('~').unwrap();
        assert_eq!((value(&m, 1), value(&m, 2)), (0x20, 0x7E));
        let before = m.caret;
        for ch in ['\n', '\t', '\u{7f}', '\u{1f}', 'é', '日'] {
            assert_eq!(m.type_ascii(ch), Err(EditRefusal::NotPrintable), "{ch:?}");
        }
        assert_eq!(m.caret, before);
        assert_eq!(value(&m, 3), 0);
    }

    #[test]
    fn an_empty_file_has_nothing_to_edit() {
        let mut m = model(&[]);
        assert_eq!(m.type_hex(b'1'), Err(EditRefusal::NothingToEdit));
        assert_eq!(m.type_ascii('a'), Err(EditRefusal::NothingToEdit));
        assert_eq!(m.paste(&[1]), Err(EditRefusal::NothingToEdit));
        assert_eq!(m.caret.offset, 0);
    }

    #[test]
    fn editing_a_byte_whose_page_is_not_loaded_is_refused_until_it_is() {
        let mut m = HexModel::new(Source::Paged { len: 100 });
        assert_eq!(m.store.byte(0), Byte::Pending);
        assert_eq!(m.type_hex(b'1'), Err(EditRefusal::NotLoaded));
        assert_eq!(m.type_ascii('a'), Err(EditRefusal::NotLoaded));
        assert_eq!(m.paste(&[1]), Err(EditRefusal::NotLoaded));
        assert!(!m.store.dirty());
        assert!(m.insert_page(0, 0, vec![0u8; 100]));
        m.type_hex(b'1').unwrap();
        assert_eq!(value(&m, 0), 0x10);
    }

    // --- paste ---------------------------------------------------------------------------------

    #[test]
    fn paste_overwrites_from_the_caret_and_is_cut_at_the_end() {
        let mut m = model(&[0, 0, 0, 0]);
        m.caret.offset = 2;
        let r = m.paste(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(
            r,
            PasteResult {
                written: 2,
                truncated: true
            }
        );
        assert_eq!([value(&m, 2), value(&m, 3)], [1, 2]);
        assert_eq!(m.store.len(), 4);
        let r = m.paste(&[9]).unwrap();
        assert_eq!(
            r,
            PasteResult {
                written: 1,
                truncated: false
            }
        );
    }

    #[test]
    fn paste_starts_at_the_selection_start() {
        let mut m = model(&[0; 8]);
        m.caret.offset = 1;
        m.caret.right(8, true);
        m.caret.right(8, true);
        assert_eq!(m.caret.selection(), Some(1..4));
        m.caret.offset = 3;
        m.paste(&[7, 7]).unwrap();
        assert_eq!(
            [value(&m, 0), value(&m, 1), value(&m, 2), value(&m, 3)],
            [0, 7, 7, 0]
        );
        assert_eq!(m.caret.anchor, None);
    }

    #[test]
    fn pasting_nothing_changes_nothing() {
        let mut m = model(&[0; 2]);
        let r = m.paste(&[]).unwrap();
        assert_eq!(
            r,
            PasteResult {
                written: 0,
                truncated: false
            }
        );
        assert_eq!(m.undo(), Ok(false));
    }

    // --- undo ----------------------------------------------------------------------------------

    #[test]
    fn undo_returns_bytes_to_not_edited_and_redo_replays_them() {
        let mut m = model(&[1, 2, 3]);
        type_all(&mut m, "99");
        assert!(edited(&m, 0));
        assert_eq!(m.undo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 1,
                edited: false
            }
        );
        assert!(!m.store.dirty());
        assert_eq!((m.caret.offset, m.caret.low_nibble), (0, false));
        assert_eq!(m.redo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x99,
                edited: true
            }
        );
        assert_eq!(m.caret.offset, 1);
        assert_eq!(m.redo(), Ok(false));
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut m = model(&[0, 0]);
        type_all(&mut m, "11");
        m.undo().unwrap();
        m.type_hex(b'2').unwrap();
        assert_eq!(m.redo(), Ok(false));
    }

    #[test]
    fn undo_with_nothing_to_undo_is_a_no_op() {
        let mut m = model(&[0]);
        assert_eq!(m.undo(), Ok(false));
        assert_eq!(m.redo(), Ok(false));
    }

    #[test]
    fn typing_at_consecutive_offsets_is_one_undo_step() {
        let mut m = model(&[0; 4]);
        type_all(&mut m, "1234");
        assert_eq!([value(&m, 0), value(&m, 1)], [0x12, 0x34]);
        assert_eq!(m.undo(), Ok(true));
        assert!(!m.store.dirty());
        assert_eq!(m.undo(), Ok(false));
    }

    #[test]
    fn a_caret_jump_closes_the_typing_step() {
        let mut m = model(&[0; 4]);
        type_all(&mut m, "12");
        m.caret.right(4, false);
        type_all(&mut m, "34");
        assert_eq!(m.undo(), Ok(true));
        assert_eq!([value(&m, 0), value(&m, 2)], [0x12, 0]);
        assert_eq!(m.undo(), Ok(true));
        assert!(!m.store.dirty());
    }

    #[test]
    fn a_side_toggle_closes_the_typing_step_even_back_on_the_same_offset() {
        let mut m = model(&[0; 4]);
        type_all(&mut m, "12");
        m.toggle_side();
        m.toggle_side();
        type_all(&mut m, "34");
        assert_eq!(m.undo(), Ok(true));
        assert_eq!([value(&m, 0), value(&m, 1)], [0x12, 0]);
    }

    #[test]
    fn a_paste_is_its_own_undo_step_and_closes_the_typing_step() {
        let mut m = model(&[0; 6]);
        type_all(&mut m, "12");
        m.paste(&[7, 8, 9]).unwrap();
        m.type_hex(b'5').unwrap();
        assert_eq!(m.undo(), Ok(true));
        assert_eq!([value(&m, 1), value(&m, 2), value(&m, 3)], [7, 8, 9]);
        assert_eq!(m.undo(), Ok(true));
        assert_eq!(
            [value(&m, 0), value(&m, 1), value(&m, 2), value(&m, 3)],
            [0x12, 0, 0, 0]
        );
        assert_eq!(m.undo(), Ok(true));
        assert!(!m.store.dirty());
    }

    #[test]
    fn undoing_a_paste_puts_the_caret_back_on_it() {
        let mut m = model(&[0; 6]);
        m.caret.offset = 2;
        m.paste(&[1, 2]).unwrap();
        assert_eq!(m.caret.offset, 4);
        m.undo().unwrap();
        assert_eq!(m.caret.offset, 2);
        m.redo().unwrap();
        assert_eq!(m.caret.offset, 4);
    }

    #[test]
    fn retyping_a_byte_inside_a_step_keeps_the_first_before() {
        let mut m = model(&[5]);
        type_all(&mut m, "ab");
        type_all(&mut m, "cd");
        assert_eq!(value(&m, 0), 0xCD);
        assert_eq!(m.undo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 5,
                edited: false
            }
        );
    }

    #[test]
    fn undo_that_needs_an_evicted_page_waits_for_it_and_changes_nothing() {
        let mut m = HexModel::new(Source::Paged { len: 100 });
        assert!(m.insert_page(0, 0, vec![0u8; 100]));
        type_all(&mut m, "99");
        m.undo().unwrap();
        // The reload-style drop evicts the page; redo restores a value, which needs the disk byte.
        m.store.bump_generation();
        assert_eq!(m.redo_missing_pages(), vec![0]);
        assert_eq!(m.redo(), Err(EditRefusal::NotLoaded));
        assert!(!m.store.dirty());
        let g = m.store.generation();
        assert!(m.insert_page(g, 0, vec![0u8; 100]));
        assert_eq!(m.redo(), Ok(true));
        assert_eq!(value(&m, 0), 0x99);
    }

    // --- patches and saving --------------------------------------------------------------------

    #[test]
    fn patches_join_neighbouring_edits_into_ascending_runs() {
        let mut m = model(&[0; 16]);
        for off in [10, 1, 2, 3, 7, 9] {
            m.caret.offset = off;
            m.type_ascii(char::from(b'a' + u8::try_from(off).unwrap()))
                .unwrap();
        }
        assert_eq!(
            m.patches(),
            vec![
                (1, vec![b'b', b'c', b'd']),
                (7, vec![b'h']),
                (9, vec![b'j', b'k']),
            ]
        );
    }

    #[test]
    fn saving_folds_the_edits_into_the_bytes_and_clears_dirty() {
        let mut m = model(&[1, 2, 3]);
        type_all(&mut m, "99");
        assert_eq!(m.patches(), vec![(0, vec![0x99])]);
        let snap = m.save_snapshot();
        assert_eq!(snap.patches(), vec![(0, vec![0x99])]);
        assert_eq!(snap.whole.as_deref(), Some(&[0x99, 2, 3][..]));
        m.apply_saved(&snap);
        assert!(!m.store.dirty());
        assert!(m.patches().is_empty());
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x99,
                edited: false
            }
        );
    }

    #[test]
    fn saving_folds_into_cached_pages_of_a_paged_file() {
        let mut m = HexModel::new(Source::Paged { len: 100 });
        assert!(m.insert_page(0, 0, vec![0u8; 100]));
        type_all(&mut m, "7f");
        let snap = m.save_snapshot();
        assert!(snap.whole.is_none());
        m.apply_saved(&snap);
        assert!(!m.store.dirty());
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x7F,
                edited: false
            }
        );
    }

    #[test]
    fn undo_after_a_save_restores_the_old_byte_as_an_unsaved_edit() {
        let mut m = model(&[1, 2, 3]);
        type_all(&mut m, "99");
        let snap = m.save_snapshot();
        m.apply_saved(&snap);
        assert_eq!(m.undo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 1,
                edited: true
            }
        );
        assert_eq!(m.patches(), vec![(0, vec![1])]);
        assert_eq!(m.redo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x99,
                edited: false
            }
        );
        assert!(!m.store.dirty());
    }

    // --- carets --------------------------------------------------------------------------------

    fn at(offset: u64) -> Caret {
        Caret {
            offset,
            ..Caret::default()
        }
    }

    #[test]
    fn caret_moves_clamp_at_the_start_and_the_end() {
        let mut c = at(0);
        c.left(40, false);
        c.up(40, false);
        c.row_start(40, false);
        assert_eq!(c.offset, 0);
        let mut c = at(39);
        c.right(40, false);
        c.down(40, false);
        c.row_end(40, false);
        assert_eq!(c.offset, 39);
        c.doc_start(40, false);
        assert_eq!(c.offset, 0);
        c.doc_end(40, false);
        assert_eq!(c.offset, 39);
    }

    #[test]
    fn caret_moves_stay_put_on_an_empty_file() {
        let mut c = Caret {
            offset: 5,
            anchor: Some(2),
            ..Caret::default()
        };
        c.right(0, true);
        assert_eq!(c, Caret::default());
        c.down(0, false);
        c.doc_end(0, false);
        c.page_down(5, 0, false);
        assert_eq!(c, Caret::default());
    }

    #[test]
    fn caret_moves_on_a_one_byte_file_never_leave_it() {
        let mut c = Caret::default();
        for step in [
            Caret::right,
            Caret::left,
            Caret::down,
            Caret::up,
            Caret::row_end,
            Caret::doc_end,
        ] {
            step(&mut c, 1, false);
            assert_eq!(c.offset, 0);
        }
    }

    #[test]
    fn caret_moves_on_a_partial_row_clamp_to_the_last_byte() {
        // 15 bytes: a single short row.
        let mut c = at(3);
        c.down(15, false);
        assert_eq!(c.offset, 3);
        c.row_end(15, false);
        assert_eq!(c.offset, 14);
        // 16 bytes: exactly one full row.
        let mut c = at(3);
        c.down(16, false);
        assert_eq!(c.offset, 3);
        c.row_end(16, false);
        assert_eq!(c.offset, 15);
        // 17 bytes: the second row holds one byte.
        let mut c = at(3);
        c.down(17, false);
        assert_eq!(c.offset, 16);
        c.up(17, false);
        assert_eq!(c.offset, 0);
        let mut c = at(16);
        c.row_end(17, false);
        assert_eq!(c.offset, 16);
        c.row_start(17, false);
        assert_eq!(c.offset, 16);
        c.left(17, false);
        assert_eq!(c.offset, 15);
        c.right(17, false);
        c.right(17, false);
        assert_eq!(c.offset, 16);
    }

    #[test]
    fn page_moves_keep_the_column_and_clamp_to_the_file() {
        let mut c = at(5);
        c.page_down(2, 100, false);
        assert_eq!(c.offset, 37);
        c.page_down(50, 100, false);
        assert_eq!(c.offset, 99);
        c.page_up(3, 100, false);
        assert_eq!(c.offset, 51);
        c.page_up(50, 100, false);
        assert_eq!(c.offset, 3);
    }

    #[test]
    fn extending_sets_the_anchor_once_and_a_plain_move_drops_the_selection() {
        let mut c = at(4);
        c.right(40, true);
        c.right(40, true);
        assert_eq!((c.anchor, c.offset), (Some(4), 6));
        assert_eq!(c.selection(), Some(4..7));
        c.left(40, true);
        c.left(40, true);
        c.left(40, true);
        assert_eq!(c.selection(), Some(3..5));
        c.left(40, false);
        assert_eq!((c.anchor, c.selection()), (None, None));
        assert_eq!(c.offset, 2);
    }

    #[test]
    fn a_caret_on_its_anchor_selects_that_one_byte() {
        let mut c = at(4);
        c.right(40, true);
        c.left(40, true);
        assert_eq!(c.selection(), Some(4..5));
        // Shift+arrow against the edge of the file still selects the byte under the caret.
        let mut c = at(0);
        c.left(40, true);
        assert_eq!(c.selection(), Some(0..1));
        c.right(40, false);
        assert_eq!(c.selection(), None);
    }

    #[test]
    fn edits_made_while_a_save_is_in_flight_stay_dirty_after_it_lands() {
        let mut m = model(&[1, 2, 3, 4]);
        type_all(&mut m, "99");
        type_all(&mut m, "88");
        let snap = m.save_snapshot();
        assert_eq!(snap.patches(), vec![(0, vec![0x99, 0x88])]);
        // While the save runs: edit an unwritten byte and re-edit a written one.
        m.caret.offset = 3;
        type_all(&mut m, "77");
        m.caret.offset = 1;
        type_all(&mut m, "55");
        m.apply_saved(&snap);
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0x99,
                edited: false
            }
        );
        assert_eq!(
            m.store.byte(1),
            Byte::Loaded {
                value: 0x55,
                edited: true
            }
        );
        assert_eq!(
            m.store.byte(2),
            Byte::Loaded {
                value: 3,
                edited: false
            }
        );
        assert_eq!(
            m.store.byte(3),
            Byte::Loaded {
                value: 0x77,
                edited: true
            }
        );
        assert_eq!(m.patches(), vec![(1, vec![0x55]), (3, vec![0x77])]);
    }

    #[test]
    fn a_byte_reverted_while_a_save_runs_is_an_edit_again_once_it_lands() {
        let mut m = model(&[1, 2]);
        type_all(&mut m, "99");
        let snap = m.save_snapshot();
        // Back to the old disk byte: the overlay entry goes, but the disk is about to change.
        m.caret.offset = 0;
        type_all(&mut m, "01");
        assert!(!m.store.dirty());
        m.apply_saved(&snap);
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 1,
                edited: true
            }
        );
        assert_eq!(m.patches(), vec![(0, vec![1])]);
    }

    #[test]
    fn an_aborted_save_leaves_the_edits_dirty_and_unpins_its_pages() {
        let mut m = HexModel::new(Source::Paged { len: 100 });
        assert!(m.insert_page(0, 0, vec![0u8; 100]));
        type_all(&mut m, "99");
        let snap = m.save_snapshot();
        m.abort_save();
        assert!(m.store.dirty());
        assert_eq!(m.patches(), snap.patches());
        assert_eq!(m.patches(), vec![(0, vec![0x99])]);

        // Once the edit is gone the page is evictable again; an unaborted save pins it.
        let mut s = paged(PAGE_U64 * 3, 1);
        assert!(page(&mut s, 0));
        s.set(5, 0xFF).unwrap();
        let _snap = s.begin_save();
        s.set(5, 0).unwrap();
        assert!(page(&mut s, 1));
        assert!(s.missing_pages(0..1).is_empty());
        s.end_save();
        assert!(page(&mut s, 2));
        assert_eq!(s.missing_pages(0..1), vec![0]);
    }

    #[test]
    fn undo_after_a_save_waits_for_an_evicted_page_and_then_works() {
        let mut m = HexModel {
            store: ByteStore::with_cache_cap(Source::Paged { len: PAGE_U64 * 3 }, 2),
            caret: Caret::default(),
            history: History::default(),
        };
        assert!(m.insert_page(0, 0, vec![0u8; PAGE]));
        assert!(m.insert_page(0, 1, vec![0u8; PAGE]));
        type_all(&mut m, "99");
        let snap = m.save_snapshot();
        m.apply_saved(&snap);
        assert!(m.undo_missing_pages().is_empty());
        // Move on to page 2: the caret pins it and page 0, now clean, is the one that goes.
        m.caret.offset = PAGE_U64 * 2;
        assert!(m.insert_page(0, 2, vec![0u8; PAGE]));
        assert_eq!(m.undo_missing_pages(), vec![0]);
        assert!(m.redo_missing_pages().is_empty());

        assert_eq!(m.undo(), Err(EditRefusal::NotLoaded));
        assert!(!m.store.dirty());
        assert_eq!(m.caret.offset, PAGE_U64 * 2);
        assert_eq!(m.undo_missing_pages(), vec![0]);

        // The page comes back as the file now is: with the saved byte.
        let mut reread = vec![0u8; PAGE];
        reread[0] = 0x99;
        assert!(m.insert_page(0, 0, reread));
        assert!(m.undo_missing_pages().is_empty());
        assert_eq!(m.undo(), Ok(true));
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 0,
                edited: true
            }
        );
    }

    #[test]
    fn a_snapshot_from_before_a_reload_is_ignored() {
        let mut m = model(&[1, 2]);
        type_all(&mut m, "99");
        let snap = m.save_snapshot();
        m.reload(Source::Whole(Arc::from(&[5u8, 6][..])));
        m.apply_saved(&snap);
        assert_eq!(
            m.store.byte(0),
            Byte::Loaded {
                value: 5,
                edited: false
            }
        );
    }

    #[test]
    fn pages_of_a_save_in_flight_survive_eviction_even_after_the_edit_is_reverted() {
        let mut s = paged(PAGE_U64 * 3, 1);
        assert!(page(&mut s, 0));
        s.set(5, 0xFF).unwrap();
        let snap = s.begin_save();
        s.set(5, 0).unwrap();
        assert!(!s.dirty());
        assert!(page(&mut s, 1));
        assert!(page(&mut s, 2));
        assert!(s.missing_pages(0..1).is_empty());
        s.fold_written(&snap.written);
        assert_eq!(
            s.byte(5),
            Byte::Loaded {
                value: 0,
                edited: true
            }
        );
    }

    #[test]
    fn toggling_the_side_resets_the_nibble_and_keeps_the_selection() {
        let mut c = Caret {
            offset: 3,
            low_nibble: true,
            anchor: Some(1),
            ..Caret::default()
        };
        c.toggle_side();
        assert_eq!(
            (c.side, c.low_nibble, c.anchor),
            (Side::Ascii, false, Some(1))
        );
        c.toggle_side();
        assert_eq!(c.side, Side::Hex);
    }

    // --- page cache ----------------------------------------------------------------------------

    #[test]
    fn rows_report_the_uncached_pages_they_touch() {
        let mut s = paged(PAGE_U64 * 3 + 5, 8);
        assert_eq!(s.missing_pages(0..page_rows()), vec![0]);
        assert_eq!(
            s.missing_pages(page_rows() - 1..page_rows() + 1),
            vec![0, 1]
        );
        assert!(page(&mut s, 0));
        assert_eq!(s.missing_pages(page_rows() - 1..page_rows() + 1), vec![1]);
        // Rows past the end ask for nothing; the short last page is the last one asked for.
        assert_eq!(s.missing_pages(page_rows() * 3..page_rows() * 10), vec![3]);
        assert!(s.missing_pages(page_rows() * 4..page_rows() * 5).is_empty());
        assert!(s.missing_pages(0..0).is_empty());
    }

    #[test]
    fn a_whole_file_is_never_missing_pages() {
        let s = ByteStore::new(Source::Whole(Arc::from(vec![0u8; 3 * PAGE])));
        assert!(s.missing_pages(0..10_000).is_empty());
        assert_eq!(
            s.byte(3 * PAGE_U64 - 1),
            Byte::Loaded {
                value: 0,
                edited: false
            }
        );
        assert_eq!(s.byte(3 * PAGE_U64), Byte::Pending);
    }

    #[test]
    fn the_short_last_page_is_accepted_and_other_lengths_are_not() {
        let mut s = paged(PAGE_U64 + 10, 8);
        let g = s.generation();
        assert!(!s.insert_page(g, 1, vec![0u8; PAGE]));
        assert!(!s.insert_page(g, 0, vec![0u8; 10]));
        assert!(!s.insert_page(g, 2, vec![0u8; 10]));
        assert!(s.insert_page(g, 1, vec![9u8; 10]));
        assert_eq!(
            s.byte(PAGE_U64 + 9),
            Byte::Loaded {
                value: 9,
                edited: false
            }
        );
        assert_eq!(s.byte(PAGE_U64 + 10), Byte::Pending);
    }

    #[test]
    fn a_page_read_before_a_reload_is_ignored() {
        let mut s = paged(PAGE_U64 * 2, 8);
        let stale = s.generation();
        s.bump_generation();
        assert!(!s.insert_page(stale, 0, vec![1u8; PAGE]));
        assert_eq!(s.byte(0), Byte::Pending);
        let now = s.generation();
        assert!(s.insert_page(now, 0, vec![1u8; PAGE]));
        assert_eq!(
            s.byte(0),
            Byte::Loaded {
                value: 1,
                edited: false
            }
        );
        s.bump_generation();
        assert_eq!(s.byte(0), Byte::Pending);
    }

    #[test]
    fn eviction_takes_the_least_recently_used_page_first() {
        let mut s = paged(PAGE_U64 * 4, 2);
        assert!(page(&mut s, 0));
        assert!(page(&mut s, 1));
        s.touch_rows(0..1);
        assert!(page(&mut s, 2));
        assert_eq!(s.missing_pages(0..page_rows() * 3), vec![1]);
    }

    #[test]
    fn eviction_never_drops_the_caret_page() {
        let mut s = paged(PAGE_U64 * 4, 2);
        s.pin_caret(0);
        assert!(page(&mut s, 0));
        assert!(page(&mut s, 1));
        assert!(page(&mut s, 2));
        assert!(page(&mut s, 3));
        assert!(s.missing_pages(0..page_rows()).is_empty());
        assert_eq!(s.missing_pages(0..page_rows() * 4), vec![1, 2]);
    }

    #[test]
    fn eviction_never_drops_a_page_that_holds_an_edit() {
        let mut s = paged(PAGE_U64 * 4, 2);
        s.pin_caret(PAGE_U64 * 3);
        assert!(page(&mut s, 0));
        s.set(5, 0xFF).unwrap();
        assert!(page(&mut s, 1));
        assert!(page(&mut s, 2));
        assert!(page(&mut s, 3));
        // Page 0 holds the edit, page 3 the caret; only 1 and 2 are evictable.
        assert!(s.missing_pages(0..page_rows()).is_empty());
        assert!(s.missing_pages(page_rows() * 3..page_rows() * 4).is_empty());
        assert_eq!(
            s.byte(5),
            Byte::Loaded {
                value: 0xFF,
                edited: true
            }
        );
        assert_eq!(s.missing_pages(0..page_rows() * 4), vec![1, 2]);
    }

    // --- parsers -------------------------------------------------------------------------------

    #[test]
    fn parse_offset_accepts_the_documented_forms() {
        let ok = |s: &str, caret: u64| parse_offset(s, caret, 10_000);
        assert_eq!(ok("0x1F", 0), Ok(31));
        assert_eq!(ok("0X1f", 0), Ok(31));
        assert_eq!(ok("1Fh", 0), Ok(31));
        assert_eq!(ok("1fH", 0), Ok(31));
        assert_eq!(ok("31", 0), Ok(31));
        assert_eq!(ok("  31\t", 0), Ok(31));
        assert_eq!(ok("1_000", 0), Ok(1000));
        assert_eq!(ok("0x1_F", 0), Ok(31));
        assert_eq!(ok("0", 0), Ok(0));
        assert_eq!(ok("+16", 10), Ok(26));
        assert_eq!(ok("-16", 20), Ok(4));
        assert_eq!(ok("- 16", 20), Ok(4));
        assert_eq!(ok("+0x10", 1), Ok(17));
        assert_eq!(ok("-0", 7), Ok(7));
    }

    #[test]
    fn parse_offset_rejects_out_of_range_and_junk() {
        let err = |s: &str, caret: u64, len: u64| parse_offset(s, caret, len).unwrap_err();
        assert_eq!(err("", 0, 100), OffsetError::Empty);
        assert_eq!(err("   ", 0, 100), OffsetError::Empty);
        assert_eq!(err("0x100", 0, 100), OffsetError::OutOfRange);
        assert_eq!(err("100", 0, 100), OffsetError::OutOfRange);
        assert_eq!(err("0", 0, 0), OffsetError::OutOfRange);
        assert_eq!(err("-16", 5, 100), OffsetError::OutOfRange);
        assert_eq!(err("+16", 90, 100), OffsetError::OutOfRange);
        assert_eq!(
            err("99999999999999999999999", 0, 100),
            OffsetError::OutOfRange
        );
        assert_eq!(
            err("+18446744073709551615", 5, u64::MAX),
            OffsetError::OutOfRange
        );
        for junk in [
            "zz", "0x", "1F", "h", "_1", "1_", "0x_1", "0x1Fh", "+", "-", "1 6", "--1", "+-1",
            "1.5",
        ] {
            assert_eq!(err(junk, 0, 100), OffsetError::Invalid, "{junk:?}");
        }
    }

    #[test]
    fn parse_hex_pattern_accepts_the_documented_forms() {
        let want = vec![0xDE, 0xAD, 0xBE, 0xEF];
        for ok in [
            "de ad BE ef",
            "deadbeef",
            "DE:AD:be:ef",
            "0xde 0xad 0xBE 0xef",
            "  de  ad\tbeef ",
            "de:ad be:ef",
            "0xdeadbeef",
        ] {
            assert_eq!(parse_hex_pattern(ok), Ok(want.clone()), "{ok:?}");
        }
        assert_eq!(parse_hex_pattern("00"), Ok(vec![0]));
    }

    #[test]
    fn parse_hex_pattern_rejects_odd_digits_junk_and_empty() {
        assert_eq!(parse_hex_pattern(""), Err(PatternError::Empty));
        assert_eq!(parse_hex_pattern("  : "), Err(PatternError::Empty));
        assert_eq!(parse_hex_pattern("dea"), Err(PatternError::OddDigits));
        assert_eq!(parse_hex_pattern("de a"), Err(PatternError::OddDigits));
        assert_eq!(parse_hex_pattern("0xd"), Err(PatternError::OddDigits));
        assert_eq!(parse_hex_pattern("zz"), Err(PatternError::InvalidDigit));
        assert_eq!(parse_hex_pattern("0x"), Err(PatternError::InvalidDigit));
        assert_eq!(parse_hex_pattern("de-ad"), Err(PatternError::InvalidDigit));
        assert_eq!(parse_hex_pattern("de ad,"), Err(PatternError::InvalidDigit));
        assert_eq!(parse_hex_pattern("dé"), Err(PatternError::InvalidDigit));
    }

    // --- find ----------------------------------------------------------------------------------

    /// Serves at most a few KiB per read, like a network or pipe-backed source.
    struct Trickle(Vec<u8>);

    impl ByteSource for Trickle {
        fn len(&self) -> u64 {
            wide(self.0.len())
        }

        fn read(&self, off: u64, buf: &mut [u8]) -> io::Result<usize> {
            let cut = buf.len().min(4093);
            self.0.read(off, &mut buf[..cut])
        }
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn plant(data: &mut [u8], at: usize, pat: &[u8]) {
        data[at..at + pat.len()].copy_from_slice(pat);
    }

    #[test]
    fn find_sees_a_match_across_a_chunk_boundary_in_both_directions() {
        let mut data = vec![0u8; FIND_CHUNK * 2 + 100];
        let pat = [1u8, 2, 3, 4];
        plant(&mut data, FIND_CHUNK - 2, &pat);
        let at = wide(FIND_CHUNK) - 2;
        let cancel = no_cancel();
        assert_eq!(
            find(&data, &pat, 0, true, false, &cancel).unwrap(),
            Some(at)
        );
        assert_eq!(
            find(&data, &pat, wide(data.len()), false, false, &cancel).unwrap(),
            Some(at)
        );
        // The next chunk starts `pat.len() - 1` before the cut, so a match one byte
        // earlier or later is just as visible.
        for shift in [1, 3] {
            let mut d = vec![0u8; FIND_CHUNK * 2];
            plant(&mut d, FIND_CHUNK - shift, &pat);
            assert_eq!(
                find(&d, &pat, 0, true, false, &cancel).unwrap(),
                Some(wide(FIND_CHUNK - shift))
            );
            assert_eq!(
                find(&d, &pat, wide(d.len()), false, false, &cancel).unwrap(),
                Some(wide(FIND_CHUNK - shift))
            );
        }
    }

    #[test]
    fn find_reads_through_short_reads() {
        let mut data = vec![0u8; FIND_CHUNK + 100];
        let pat = [9u8, 8, 7];
        plant(&mut data, FIND_CHUNK - 1, &pat);
        let src = Trickle(data);
        let cancel = no_cancel();
        assert_eq!(
            find(&src, &pat, 0, true, false, &cancel).unwrap(),
            Some(wide(FIND_CHUNK) - 1)
        );
        assert_eq!(
            find(&src, &pat, u64::MAX, false, false, &cancel).unwrap(),
            Some(wide(FIND_CHUNK) - 1)
        );
    }

    #[test]
    fn find_sees_unsaved_edits_and_not_the_bytes_they_replaced() {
        let mut data = vec![0u8; 64];
        plant(&mut data, 5, &[7, 7]);
        let cancel = no_cancel();
        let mut overlay = BTreeMap::new();
        overlay.insert(5, 0);
        let edited = Overlaid {
            src: data.as_slice(),
            overlay: &overlay,
        };
        assert_eq!(
            find(&edited, &[7, 7], 0, true, true, &cancel).unwrap(),
            None
        );
        overlay.insert(5, 7);
        overlay.insert(40, 7);
        overlay.insert(41, 7);
        let edited = Overlaid {
            src: data.as_slice(),
            overlay: &overlay,
        };
        assert_eq!(
            find(&edited, &[7, 7], 6, true, false, &cancel).unwrap(),
            Some(40)
        );
        assert_eq!(
            find(&edited, &[7, 7], 39, false, false, &cancel).unwrap(),
            Some(5)
        );
        // Without the overlay only the planted pair exists.
        assert_eq!(
            find(&data, &[7, 7], 6, true, true, &cancel).unwrap(),
            Some(5)
        );
    }

    #[test]
    fn find_starts_at_from_inclusive_and_goes_backward_to_the_nearest_match() {
        let mut data = vec![0u8; 100];
        for at in [10, 50, 90] {
            plant(&mut data, at, &[1, 2]);
        }
        let c = no_cancel();
        let go = |from, fwd| find(&data, &[1, 2], from, fwd, false, &c).unwrap();
        assert_eq!(go(50, true), Some(50));
        assert_eq!(go(51, true), Some(90));
        assert_eq!(go(91, true), None);
        assert_eq!(go(50, false), Some(50));
        assert_eq!(go(49, false), Some(10));
        assert_eq!(go(9, false), None);
        assert_eq!(go(10_000, false), Some(90));
    }

    #[test]
    fn find_wraps_around_only_when_asked() {
        let mut data = vec![0u8; 100];
        plant(&mut data, 20, &[1, 2]);
        let c = no_cancel();
        assert_eq!(find(&data, &[1, 2], 30, true, false, &c).unwrap(), None);
        assert_eq!(find(&data, &[1, 2], 30, true, true, &c).unwrap(), Some(20));
        assert_eq!(find(&data, &[1, 2], 10, false, false, &c).unwrap(), None);
        assert_eq!(find(&data, &[1, 2], 10, false, true, &c).unwrap(), Some(20));
        // Wrapping does not find a match that starts before `from` twice or loop forever.
        assert_eq!(find(&data, &[3, 3], 10, true, true, &c).unwrap(), None);
        assert_eq!(find(&data, &[3, 3], 10, false, true, &c).unwrap(), None);
    }

    #[test]
    fn find_matches_at_the_very_end_and_ignores_impossible_patterns() {
        let mut data = vec![0u8; 100];
        plant(&mut data, 98, &[5, 6]);
        let c = no_cancel();
        assert_eq!(find(&data, &[5, 6], 0, true, false, &c).unwrap(), Some(98));
        assert_eq!(
            find(&data, &[5, 6], 99, false, false, &c).unwrap(),
            Some(98)
        );
        assert_eq!(find(&data, &[], 0, true, true, &c).unwrap(), None);
        assert_eq!(find(&data, &[0u8; 101], 0, true, true, &c).unwrap(), None);
        assert_eq!(
            find(&Vec::<u8>::new(), &[1], 0, true, true, &c).unwrap(),
            None
        );
    }

    #[test]
    fn a_cancelled_find_stops_without_a_match() {
        let mut data = vec![0u8; FIND_CHUNK * 3];
        plant(&mut data, 5, &[1, 2]);
        let cancel = AtomicBool::new(true);
        assert_eq!(find(&data, &[1, 2], 0, true, true, &cancel).unwrap(), None);
        assert_eq!(
            find(&data, &[1, 2], 100, false, true, &cancel).unwrap(),
            None
        );
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            find(&data, &[1, 2], 0, true, true, &cancel).unwrap(),
            Some(5)
        );
    }

    #[test]
    fn find_passes_a_read_error_through() {
        struct Broken;
        impl ByteSource for Broken {
            fn len(&self) -> u64 {
                10
            }

            fn read(&self, _: u64, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        let err = find(&Broken, &[1], 0, true, false, &no_cancel()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    // --- inspector and rows --------------------------------------------------------------------

    #[test]
    fn inspect_reads_every_width_both_ways() {
        let v = inspect(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01]);
        assert_eq!(v.u8, Some(0xDE));
        assert_eq!(v.i8, Some(-34));
        assert_eq!(v.u16le, Some(0xADDE));
        assert_eq!(v.u16be, Some(0xDEAD));
        assert_eq!(v.u32le, Some(0xEFBE_ADDE));
        assert_eq!(v.u32be, Some(0xDEAD_BEEF));
    }

    #[test]
    fn inspect_leaves_the_fields_the_bytes_do_not_reach_empty() {
        assert_eq!(inspect(&[]), Values::default());
        let one = inspect(&[0x80]);
        assert_eq!(
            (one.u8, one.i8, one.u16le, one.u16be, one.u32le),
            (Some(0x80), Some(-128), None, None, None)
        );
        let three = inspect(&[1, 2, 3]);
        assert_eq!((three.u16le, three.u16be), (Some(0x0201), Some(0x0102)));
        assert_eq!((three.u32le, three.u32be), (None, None));
    }

    #[test]
    fn offsets_are_written_as_eight_or_sixteen_hex_digits() {
        let mut s = String::from("@");
        write_offset(&mut s, 0x1F, false);
        assert_eq!(s, "@0000001F");
        let mut s = String::new();
        write_offset(&mut s, 0x1_2345_6789_ABCD, true);
        assert_eq!(s, "000123456789ABCD");
        let mut s = String::new();
        write_offset(&mut s, u64::MAX, false);
        assert_eq!(s, "FFFFFFFF");
    }

    #[test]
    fn bytes_format_as_two_hex_digits_and_ascii_hides_the_unprintable() {
        assert_eq!(hex_byte(0x00), *b"00");
        assert_eq!(hex_byte(0xAB), *b"AB");
        assert_eq!(hex_byte(0x0F), *b"0F");
        assert_eq!(ascii_char(b'A'), 'A');
        assert_eq!(ascii_char(b' '), ' ');
        assert_eq!(ascii_char(b'~'), '~');
        for b in [0x00, 0x1F, 0x7F, 0x80, 0xFF] {
            assert_eq!(ascii_char(b), '.', "{b:#x}");
        }
    }
}
