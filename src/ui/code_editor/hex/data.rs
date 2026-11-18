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
    Ready(HexModel),
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
    pub(crate) fn from_loaded(host: SharedHost, loaded: HexLoaded) -> Self {
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
                    HexContent::Ready(model),
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
