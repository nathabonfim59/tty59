use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use gpui::{
    AnyElement, Background, FocusHandle, FontWeight, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, SharedString, Window, div, prelude::*, px, rems,
};
use gpui_component::button::Button;
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};

use crate::core::config::{Config, DiffViewMode};
use crate::core::git::status::DecoStatus;
use crate::terminal::git_diff::{
    self, CommitLabel, DiffSnapshot, DiffSource, DiffStats, FileDiff, FileStatus, LineKind,
    Truncation,
};

/// How much of an untracked file the preview will read. Past this the card
/// says the read failed rather than showing a silently cut-off file — and the
/// line budget below cuts rendering long before this does anyway.
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
use crate::ui::app::{TitleStripRow as _, Tty59App};
use crate::ui::diff_list::{DiffRow, FileHead, RowAt};
use crate::ui::diff_rows::{DiffSelection, Side, SplitCell, SplitRow, UnifiedRow};
use crate::ui::document_column::{
    DocumentChrome, glyph_split, glyph_unified, switch_cell, switch_ink,
};
use crate::ui::i18n::{L10nKey, t, t_fmt, t_plural};
use crate::ui::right_panel::{HEADING, META, META_MONO, TEXT, TEXT_MONO, git_badge, info_chip};
use crate::ui::rounding;
use crate::ui::scm::path::relative_time;
use crate::ui::scm::status::{status_color, status_glyph};

pub(crate) enum DiffLoad {
    Loading,
    Ready(Loaded),
    NotARepo,
}

/// A snapshot as the overlay holds it, with the facts the header is drawn and
/// budgeted from, taken once where the snapshot is installed.
///
/// One value, built by the only way to make one — `From<Arc<DiffSnapshot>>` —
/// so the facts cannot describe a different snapshot than the one beside
/// them, and a new snapshot brings its own. The header and its budget read the
/// same facts every frame instead of walking the files again.
#[derive(Clone)]
pub(crate) struct Loaded {
    snapshot: Arc<DiffSnapshot>,
    facts: Arc<LoadFacts>,
}

struct LoadFacts {
    stats: DiffStats,
    /// Every path the snapshot names: the files it holds a patch for and the
    /// untracked ones it only lists. What a focus can point at.
    paths: HashSet<String>,
}

impl From<Arc<DiffSnapshot>> for Loaded {
    fn from(snapshot: Arc<DiffSnapshot>) -> Self {
        let facts = LoadFacts {
            stats: snapshot.stats(),
            paths: snapshot
                .files
                .iter()
                .map(|f| f.path.clone())
                .chain(snapshot.untracked.iter().cloned())
                .collect(),
        };
        Self {
            snapshot,
            facts: Arc::new(facts),
        }
    }
}

impl std::ops::Deref for Loaded {
    type Target = DiffSnapshot;

    fn deref(&self) -> &DiffSnapshot {
        &self.snapshot
    }
}

impl Loaded {
    /// The snapshot itself, for whoever keeps or compares it by identity.
    fn snapshot(&self) -> &Arc<DiffSnapshot> {
        &self.snapshot
    }

    /// [`DiffSnapshot::stats`], taken once when the snapshot was installed.
    fn stats(&self) -> DiffStats {
        self.facts.stats
    }

    /// [`DiffSnapshot::totals`], from the same facts.
    fn totals(&self) -> (u32, u32) {
        self.facts.stats.totals
    }

    /// Whether `path` is a file the snapshot holds or lists untracked.
    fn has_path(&self, path: &str) -> bool {
        self.facts.paths.contains(path)
    }
}

pub(crate) struct DiffOverlayState {
    pub(crate) host_id: crate::ui::host_ops::HostId,
    pub(crate) cwd: PathBuf,
    /// Which patch this overlay is showing. Part of its identity, not a
    /// setting: two sources over one directory are two different overlays.
    pub(crate) source: DiffSource,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) load: DiffLoad,
    pub(crate) loading: bool,
    pub(crate) expanded: HashMap<String, bool>,
    pub(crate) focus: Option<String>,
    /// A synthesized all-added card for a focused *untracked* file, keyed by
    /// path; `None` in the value means the read failed. git has no patch for
    /// an untracked file, so focusing one reads its bytes instead — lazily,
    /// only for the file on screen, never for the whole list. Cleared when a
    /// fresh snapshot lands, so an edit to the file shows up on the same
    /// cadence a tracked file's does.
    pub(crate) preview: Option<(String, Option<Arc<FileDiff>>)>,
    pub(crate) preview_loading: Option<String>,
    /// The rows the pointer has dragged over, and whether it is still down.
    ///
    /// A diff is read in order to be copied out of, and until this existed the
    /// text on screen was unreachable — no selection, no clipboard, nothing
    /// but retyping it (#721). Line-granular on purpose: the rows are a grid
    /// of independent elements, not one text run, so a range of them is the
    /// selection this layout can honestly offer.
    pub(crate) selection: Option<DiffSelection>,
    pub(crate) selecting: bool,
    /// The virtualised list the rows scroll in. Held across frames: it owns
    /// the scroll position, and the row heights gpui has measured.
    pub(crate) list: gpui::ListState,
    /// The patch, flattened into one row per line — see
    /// [`crate::ui::diff_list`]. Rebuilt only when [`RowsKey`] changes.
    pub(crate) rows: Rc<Vec<DiffRow>>,
    rows_key: Option<RowsKey>,
    /// The [`ScmData`](crate::terminal::git_data::ScmData) epoch this patch was
    /// read at, for the two sources that can go stale.
    ///
    /// Recorded when a probe *starts*, so a `git add` that lands while one is
    /// running is not mistaken for a change the result already reflects.
    /// `None` until the first snapshot arrives: the epoch is keyed by the
    /// repository root, and only a snapshot knows where that is.
    pub(crate) epoch: Option<u64>,
}

/// Paints the full-window diff surface without inheriting workspace opacity.
/// The preset's solid or gradient design remains intact, but neither it nor
/// the plain theme fallback may reveal the OS backdrop through diff text.
fn diff_overlay_background(
    active: Option<&crate::ui::presets::ActiveBackground>,
    fallback: Hsla,
) -> Background {
    match active {
        Some(bg) => crate::ui::theme::window_background_opaque(bg),
        None => fallback.alpha(1.0).into(),
    }
}

impl Tty59App {
    pub(crate) fn toggle_diff_overlay(
        &mut self,
        host: crate::ui::host_ops::HostId,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_diff_overlay_at(host, cwd, None, window, cx)
    }

    pub(crate) fn toggle_diff_overlay_at(
        &mut self,
        host: crate::ui::host_ops::HostId,
        cwd: PathBuf,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The sidebar's `+N −M` is `diff --numstat HEAD`, so opening it has
        // to show the same span. The panel names its own source per group.
        self.open_diff_overlay(host, cwd, DiffSource::Head, focus, window, cx)
    }

    pub(crate) fn open_diff_overlay(
        &mut self,
        host: crate::ui::host_ops::HostId,
        cwd: PathBuf,
        source: DiffSource,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        let was_front = self.tabs.get(active).is_some_and(|t| {
            t.overlay_top == crate::ui::app::OverlayTop::Diff || !self.code_panel_visible()
        });
        if let Some(tab) = self.tabs.get_mut(active) {
            tab.overlay_top = crate::ui::app::OverlayTop::Diff;
        }
        // The source belongs in this filter: an open worktree overlay reused
        // for a staged file would only move the focus, and go on showing the
        // unstaged patch under the staged file's name.
        match self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
            .filter(|o| o.cwd == cwd && o.host_id == host && o.source == source)
        {
            Some(o) if o.focus == focus && was_front => {
                self.close_diff_overlay(window, cx);
                return;
            }
            Some(o) => {
                o.focus = focus;
                // Another file is on screen now; the range belonged to the
                // one that left.
                o.selection = None;
                o.selecting = false;
                let handle = o.focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
                return;
            }
            None => {}
        }
        self.remember_active_pane(window, cx);
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        let focus_handle = cx.focus_handle();
        tab.diff_overlay = Some(DiffOverlayState {
            host_id: host,
            cwd,
            source,
            focus_handle: focus_handle.clone(),
            // Every open starts at Loading until its own probe lands. The old
            // panel-snapshot seeding died with the panel that held a snapshot
            // per source; re-seeding would need the caller to carry one.
            load: DiffLoad::Loading,
            loading: false,
            expanded: HashMap::new(),
            focus,
            preview: None,
            preview_loading: None,
            selection: None,
            selecting: false,
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(256.))
                .with_size_hint(DIFF_LINE_H),
            rows: Rc::new(Vec::new()),
            rows_key: None,
            epoch: None,
        });
        window.focus(&focus_handle, cx);
        self.spawn_diff_probe(cx);
        cx.notify();
    }

    /// Open (or refocus, or close — the same toggle as any other source) the
    /// overlay on a patch that did not come from git, with its snapshot.
    ///
    /// The snapshot names its own directory and source, which must be a
    /// [`DiffSource::Patch`]; the probe skips those, so the snapshot installed
    /// here is the only one the overlay will ever have.
    pub(crate) fn open_supplied_diff(
        &mut self,
        host: crate::ui::host_ops::HostId,
        snapshot: Arc<DiffSnapshot>,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (cwd, source) = (snapshot.root.clone(), snapshot.source.clone());
        debug_assert!(source.is_supplied());
        self.open_diff_overlay(host, cwd.clone(), source.clone(), focus, window, cx);
        if let Some(overlay) = self
            .tabs
            .get_mut(self.active)
            .and_then(|t| t.diff_overlay.as_mut())
            .filter(|o| o.cwd == cwd && o.host_id == host && o.source == source)
            && !matches!(overlay.load, DiffLoad::Ready(_))
        {
            overlay.load = DiffLoad::Ready(snapshot.into());
            cx.notify();
        }
    }

    /// Opens `snapshot` as an overlay that has already loaded, whatever its
    /// source, for tests outside this file: [`Self::open_supplied_diff`] takes
    /// only patches, and the overlay's state is private to this module.
    ///
    /// Builds the overlay itself instead of going through
    /// [`Self::open_diff_overlay`], which would start a probe of the
    /// directory for any source git can answer — and a probe of a directory
    /// that is not there lands later as "not a repository" over the snapshot
    /// this installs.
    #[cfg(test)]
    pub(crate) fn open_loaded_diff_for_test(
        &mut self,
        snapshot: Arc<DiffSnapshot>,
        loading: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (cwd, source) = (snapshot.root.clone(), snapshot.source.clone());
        let active = self.active;
        let Some(tab) = self.tabs.get_mut(active) else {
            return;
        };
        tab.overlay_top = crate::ui::app::OverlayTop::Diff;
        let focus_handle = cx.focus_handle();
        tab.diff_overlay = Some(DiffOverlayState {
            host_id: crate::ui::host_ops::HostId::LOCAL,
            cwd,
            source,
            focus_handle: focus_handle.clone(),
            load: DiffLoad::Ready(snapshot.into()),
            loading,
            expanded: HashMap::new(),
            focus: None,
            preview: None,
            preview_loading: None,
            selection: None,
            selecting: false,
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(256.))
                .with_size_hint(DIFF_LINE_H),
            rows: Rc::new(Vec::new()),
            rows_key: None,
            epoch: None,
        });
        window.focus(&focus_handle, cx);
        cx.notify();
    }

    /// Whether the active overlay is ready and holds exactly `expected`, by
    /// identity, so a test measures the fixture it installed and not whatever
    /// replaced it.
    #[cfg(test)]
    pub(crate) fn diff_overlay_holds_for_test(&self, expected: &Arc<DiffSnapshot>) -> bool {
        match self
            .tabs
            .get(self.active)
            .and_then(|t| t.diff_overlay.as_ref())
            .map(|o| &o.load)
        {
            Some(DiffLoad::Ready(loaded)) => std::ptr::eq(&**loaded, expected.as_ref()),
            _ => false,
        }
    }

    /// Which file the open overlay is focused on — for the row that asked,
    /// which means the *source* has to match too: a file staged and edited
    /// again sits in two panel groups, and only the row whose patch is
    /// actually on screen may draw itself selected.
    pub(crate) fn diff_overlay_focus(
        &self,
        host: crate::ui::host_ops::HostId,
        cwd: &std::path::Path,
        source: &crate::terminal::git_diff::DiffSource,
    ) -> Option<&str> {
        let overlay = self.tabs.get(self.active)?.diff_overlay.as_ref()?;
        (overlay.cwd == cwd && overlay.host_id == host && overlay.source == *source)
            .then_some(overlay.focus.as_deref())?
    }

    pub(crate) fn close_diff_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active;
        let taken = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.take());
        if taken.is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Begin a drag at `at`, in the column it was pressed in.
    fn start_diff_selection(
        &mut self,
        at: &RowAt,
        mode: DiffViewMode,
        side: Option<Side>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        let Some(overlay) = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
        else {
            return;
        };
        overlay.selection = Some(DiffSelection {
            path: at.path.to_string(),
            mode,
            side,
            anchor: at.id,
            head: at.id,
        });
        overlay.selecting = true;
        let handle = overlay.focus_handle.clone();
        // Copying needs the overlay to hold the keyboard. Docked beside a
        // shell it often does not, and Ctrl+C would otherwise reach the pane
        // and interrupt whatever is running in it.
        window.focus(&handle, cx);
        cx.notify();
    }

    /// Extend the drag in flight to `at`.
    ///
    /// `held` is what the pointer is still pressing. A move with nothing held
    /// means the button came up somewhere the overlay never saw — over a pane,
    /// or outside the window — so the drag ends here rather than resuming the
    /// next time the pointer wanders back over a row.
    fn extend_diff_selection(
        &mut self,
        at: &RowAt,
        side: Option<Side>,
        held: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        let Some(overlay) = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
        else {
            return;
        };
        if !overlay.selecting {
            return;
        }
        if held != Some(MouseButton::Left) {
            overlay.selecting = false;
            cx.notify();
            return;
        }
        let Some(sel) = overlay.selection.as_mut() else {
            return;
        };
        if sel.path != at.path.as_ref() || sel.side != side || sel.head == at.id {
            return;
        }
        sel.head = at.id;
        cx.notify();
    }

    fn end_diff_selection(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        if let Some(overlay) = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
            && overlay.selecting
        {
            overlay.selecting = false;
            cx.notify();
        }
    }

    /// Put the selected rows on the clipboard, as the file spells them.
    fn copy_diff_selection(&self, cx: &mut Context<Self>) {
        let Some(overlay) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.diff_overlay.as_ref())
        else {
            return;
        };
        let Some(sel) = overlay.selection.as_ref() else {
            return;
        };
        let hunks = match &overlay.load {
            DiffLoad::Ready(snap) => snap
                .files
                .iter()
                .find(|f| f.path == sel.path)
                .map(|f| f.hunks.as_slice()),
            _ => None,
        }
        // An untracked file has no patch in the snapshot — its rows are
        // synthesized from the file's own bytes, and so is its text.
        .or_else(|| match &overlay.preview {
            Some((held, Some(file))) if *held == sel.path => Some(file.hunks.as_slice()),
            _ => None,
        });
        let Some(hunks) = hunks else {
            return;
        };
        let text = sel.text(hunks);
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn spawn_diff_probe(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let Some(overlay) = self.tabs.get(active).and_then(|t| t.diff_overlay.as_ref()) else {
            return;
        };
        // A supplied patch has nothing to probe: git never had it.
        if overlay.loading || overlay.source.is_supplied() {
            return;
        }
        let cwd = overlay.cwd.clone();
        let source = overlay.source.clone();
        let id = overlay.host_id;
        // Read before the probe is dispatched, not after it lands: anything
        // bumped in between belongs to the next read, not this one.
        let epoch = match &overlay.load {
            DiffLoad::Ready(snap) => Some(scm_epoch(cx, id, &snap.root)),
            _ => None,
        };
        let Some(host) = crate::ui::host_registry::HostRegistry::lookup(cx, id) else {
            return;
        };
        let Some(overlay) = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
        else {
            return;
        };
        overlay.loading = true;
        overlay.epoch = epoch;
        self.spawn_diff_probe_for(host, cwd, source, cx);
    }

    pub(crate) fn spawn_diff_probe_for(
        &mut self,
        host: crate::ui::host_ops::SharedHost,
        cwd: PathBuf,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        let key = probe_key(host.id(), &cwd, &source);
        if !self.diff_probes_inflight.insert(key.clone()) {
            self.diff_probes_restale.insert(key);
            return;
        }
        let host_for_retry = host.clone();
        let probe_cwd = cwd.clone();
        let probe_source = source.clone();
        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                let req = git_diff::DiffRequest {
                    source: probe_source,
                    ..Default::default()
                };
                git_diff::probe_diff(h, &probe_cwd, &req)
            },
            move |app, result, cx| {
                let id = key.0;
                app.diff_probes_inflight.remove(&key);
                app.install_diff_snapshot(id, &cwd, &source, result.map(Arc::new), cx);
                if app.diff_probes_restale.remove(&key) {
                    app.spawn_diff_probe_for(host_for_retry, cwd, source, cx);
                }
            },
        );
    }

    fn install_diff_snapshot(
        &mut self,
        host: crate::ui::host_ops::HostId,
        cwd: &Path,
        source: &DiffSource,
        snap: Option<Arc<DiffSnapshot>>,
        cx: &mut Context<Self>,
    ) {
        // A diff read is a fresher answer to the question the sidebar's branch
        // and +N −N ask, and it is the one the reader is looking at. Hand the
        // branch and the numbers back before anything renders, or the row can
        // disagree with the overlay it just opened — and `maybe_refresh` reads
        // that disagreement as a reason to probe again.
        //
        // A read that failed is not an answer at all: its totals are whatever
        // got parsed before git gave up, usually zero. Publishing those wipes
        // the counts the sidebar already had right — the overlay says so in
        // words a few lines below, and the row would silently disagree.
        let mut landed = if let Some(snap) = snap.as_ref().filter(|s| !s.read_failed) {
            // Only a HEAD snapshot counts what the sidebar counts. A worktree
            // or staged patch is a smaller answer to a different question, and
            // a commit or a range is not about the working tree at all.
            let counts = matches!(snap.source, DiffSource::Head).then(|| snap.totals());
            let root = snap.root.clone();
            let branch = snap.branch.clone();
            cx.default_global::<crate::terminal::git_status::GitStatusCache>();
            cx.update_global::<crate::terminal::git_status::GitStatusCache, _>(|cache, _| {
                cache.note_diff_read(host, &root, &branch, counts)
            })
        } else {
            false
        };
        // Only wanted by an overlay whose first probe could not know the root,
        // and so could not read its own epoch before dispatching.
        let landing_epoch = snap.as_ref().map(|s| scm_epoch(cx, host, &s.root));
        // The facts the header is drawn from, taken once for the snapshot and
        // shared by every overlay it lands in.
        let loaded = snap.as_ref().map(|s| Loaded::from(Arc::clone(s)));
        for tab in self.tabs.iter_mut() {
            let Some(overlay) = tab
                .diff_overlay
                .as_mut()
                .filter(|o| o.cwd == cwd && o.host_id == host && o.source == *source)
            else {
                continue;
            };
            overlay.loading = false;
            overlay.epoch = overlay.epoch.or(landing_epoch);
            overlay.load = match &loaded {
                Some(loaded) => DiffLoad::Ready(loaded.clone()),
                None => DiffLoad::NotARepo,
            };
            // A new snapshot restarts any untracked preview: the file may
            // have changed with the tree, and the re-read costs one file.
            overlay.preview = None;
            // The rows it was drawn against are gone. A range that survived
            // would keep its coordinates and quietly cover other code.
            overlay.selection = None;
            overlay.selecting = false;
            landed = true;
        }
        if landed {
            cx.notify();
        }
    }

    pub(crate) fn maybe_refresh_diff_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.diff_overlay.as_ref())
        else {
            return;
        };
        if overlay.loading {
            return;
        }
        let DiffLoad::Ready(snap) = &overlay.load else {
            return;
        };
        let stale = match overlay.source {
            // A commit and a range are fixed patches. Nothing can make either
            // of them out of date, so nothing should reprobe them.
            DiffSource::Commit { .. } | DiffSource::Range { .. } | DiffSource::Patch { .. } => {
                return;
            }
            // The cached counts come from `git diff --numstat HEAD`, so only a
            // HEAD snapshot is comparable to them.
            DiffSource::Head => {
                let Some(status) = cx
                    .try_global::<crate::terminal::git_status::GitStatusCache>()
                    .and_then(|cache| cache.status_for(overlay.host_id, &overlay.cwd))
                else {
                    return;
                };
                status.branch != snap.branch || (status.added, status.removed) != snap.totals()
            }
            // Those same counts would differ from a staged or unstaged patch
            // the moment anything is staged, and the overlay would reprobe
            // forever. The epoch answers the question that was actually being
            // asked — "did anything happen to this repository" — without
            // knowing what either side is counting.
            DiffSource::Worktree | DiffSource::Staged => {
                let Some(seen) = overlay.epoch else {
                    return;
                };
                scm_epoch(cx, overlay.host_id, &snap.root) != seen
            }
        };
        if stale {
            self.spawn_diff_probe(cx);
        }
    }

    pub(crate) fn render_diff_overlay(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.spawn_untracked_preview_if_needed(cx);
        let body = self.sync_diff_rows(cx)?;
        let overlay = self.tabs.get(self.active)?.diff_overlay.as_ref()?;

        let content = match body {
            DiffBody::Message(text) => self.diff_message(text, cx),
            DiffBody::Rows(snap) => self.diff_rows_list(overlay, snap, cx),
        };

        let header = chrome
            .renders_own_header()
            .then(|| self.diff_header(overlay, chrome, window, cx));
        let focus_handle = overlay.focus_handle.clone();

        let shell = v_flex();
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                // Opaque on purpose: this overlay covers the entire workspace,
                // so window translucency and backdrop material must stop here.
                .bg(diff_overlay_background(
                    cx.try_global::<crate::ui::presets::ActiveBackground>(),
                    cx.theme().background,
                ))
                // The opaque fill above covers the theme background image the
                // workspace root paints, so the overlay carries its own copy,
                // dimmed back to the strength it had when this overlay was
                // itself translucent.
                .children(crate::ui::app::overlay_surface_layers(cx)),
            // Docked, the column wrapper has already painted the surface this
            // sits on — the same one the right panel uses — and nothing behind
            // it needs stopping.
            DocumentChrome::Dock | DocumentChrome::DockHoisted => shell.size_full().min_w_0(),
        };
        Some(
            shell
                .when(cfg!(test), |d| {
                    d.debug_selector(|| "diff-overlay-body".into())
                })
                .text_color(cx.theme().foreground)
                .track_focus(&focus_handle)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key.as_str() == "escape" {
                        this.close_diff_overlay(window, cx);
                    }
                    // The overlay takes focus when a row is dragged, so this is
                    // the copy key for the selection that drag made — and only
                    // then: with nothing selected it falls through to whatever
                    // else the window binds it to.
                    let mods = ev.keystroke.modifiers;
                    if ev.keystroke.key.as_str() == "c" && mods.secondary() && !mods.alt {
                        this.copy_diff_selection(cx);
                    }
                }))
                // A drag that ends anywhere in the overlay ends here; one that
                // ends outside it is caught by the next move over a row, which
                // sees no button held.
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.end_diff_selection(cx)),
                )
                .children(header)
                .child(content)
                .into_any_element(),
        )
    }

    /// The diff header alone, for the strip above a docked column.
    pub(crate) fn render_diff_header_only(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.tabs.get(self.active)?.diff_overlay.as_ref()?;
        Some(
            self.diff_header(overlay, chrome, window, cx)
                .into_any_element(),
        )
    }

    fn diff_header(
        &self,
        overlay: &DiffOverlayState,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (branch, files, untracked, added, removed) = match &overlay.load {
            DiffLoad::Ready(s) => {
                let stats = s.stats();
                let (a, r) = stats.totals;
                (s.branch.clone(), s.files.len(), stats.untracked_count, a, r)
            }
            _ => (String::new(), 0, 0, 0, 0),
        };
        let lead = self.diff_header_lead(chrome, cx);
        let mono = SharedString::from(self.font_family.clone());
        let subject = source_subject(&overlay.source, branch);
        let shape = source_shape(&overlay.source);
        // A filled header decides what is on its second line from what is
        // measured and the room it is drawn in, never from what it has painted:
        // the first level whose two lines both fit the body, held the editor's
        // margin by the last answer the app's events recorded. The names then
        // take what their line leaves.
        let need =
            (chrome == DocumentChrome::Fill).then(|| self.diff_fill_need(overlay, window, cx));
        let body = self.document_body_px(window, cx);
        let plan = need.map(|n| n.plan(body, self.editor.diff_wrap));
        let overflow = plan.is_some_and(|p| p.overflow);
        let level = plan.map_or(0, |p| p.ladder);
        let two_line = overflow || need.is_some_and(|n| n.has_second_line(level));
        let moved = |tier: usize| !overflow && need.is_some_and(|n| n.tier_moved(tier, level));
        let switch_lead = plan.map_or(crate::ui::document_column::SWITCH_LEAD, |p| p.lead);
        let parts = HeaderParts::of(overlay, shape, chrome, (added, removed));
        let caps = need.map(|n| n.caps(body, level));
        let rem = window.rem_size();
        let hover = gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().window.hover);

        // The focused file's way back and the two totals are on the first
        // line whatever the level; the summary and the Refreshing word are on
        // the first or the second with the chip and the terminal's name.
        let mut back: Option<AnyElement> = None;
        let mut summary_el: Option<AnyElement> = None;
        let mut added_el: Option<AnyElement> = None;
        let mut removed_el: Option<AnyElement> = None;
        let mut refresh_el: Option<AnyElement> = None;
        // The focused file's path is the only other thing on the strip that
        // grows without bound, and it used to refuse to yield any of it: a
        // header with a commit label already spends its slack on the subject,
        // so the path pushed the view switch and the close tile off the end of
        // a docked column and they were clipped away mid-word. It shrinks now,
        // ahead of the subject (`999.` against the subject's `1.`) because a
        // path has a second home one line down in the file list and the
        // subject has none — and it shrinks head-first, so the filename is the
        // last thing to go.
        if let Some(name) = focused_name(overlay) {
            let (head, leaf) = crate::ui::path_display::split_path_leaf(&name);
            // In the overflow rows the path yields, so the whole of it is the
            // tooltip there; nowhere else is anything about it changed.
            let path_tip = overflow.then(|| SharedString::from(name.clone()));
            // What the held row leaves the way back: its least, or, where the
            // row cannot hold that, the chevron part and what remains for the
            // path. The same width the plan packed it at.
            let back_min = need.zip(plan).map_or(0., |(n, p)| n.eff(&p, Item::Back));
            // The same overflow-only whole path says itself to assistive
            // technology: nothing is allocated where the path is not cut.
            let path_label = path_tip.clone();
            let path_el = if overflow {
                // The path in the overflow rows is cut by the one measured
                // elision the UI has, keeping both ends, in the width layout
                // gives it, which is never less than the planned tail.
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(META_MONO))
                    .font_family(self.font_family.clone())
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-path".into())
                    })
                    .child(crate::ui::text_elision::ElidedLabel::new(
                        "diff-overlay-path-text",
                        SharedString::from(name.clone()),
                        crate::ui::text_elision::Mode::Middle,
                    ))
                    .into_any_element()
            } else {
                h_flex()
                    .min_w_0()
                    .text_size(rems(META_MONO))
                    .font_family(self.font_family.clone())
                    .child(div().min_w_0().flex_shrink(999.).truncate().child(head))
                    .child(div().min_w_0().flex_shrink(1.).truncate().child(leaf))
                    .painted("header-path")
                    .into_any_element()
            };
            back = Some(
                div()
                    .occlude()
                    .when(!overflow, |d| d.min_w_0())
                    .when(overflow, |d| d.min_w(px(back_min)))
                    .flex_shrink(999.)
                    .child(
                        h_flex()
                            .id("diff-overlay-unfocus")
                            .when(cfg!(test), |d| {
                                d.debug_selector(|| "diff-overlay-unfocus".into())
                            })
                            .items_center()
                            .min_w_0()
                            .gap(rems(FOCUS_GAP_REM))
                            .h(CONTROL_H)
                            .pl(px(FOCUS_PAD_L))
                            .pr(ROW_INSET)
                            .rounded(ROW_RADIUS)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                let active = this.active;
                                if let Some(overlay) = this
                                    .tabs
                                    .get_mut(active)
                                    .and_then(|t| t.diff_overlay.as_mut())
                                {
                                    overlay.focus = None;
                                    cx.notify();
                                }
                            }))
                            .when_some(path_tip, |d, tip| {
                                d.tooltip(move |window, cx| text_tooltip(tip.clone(), window, cx))
                            })
                            // In the overflow rows the path is cut to what the
                            // row leaves, so the control says the whole path to
                            // assistive technology; nowhere else is it changed.
                            .when_some(path_label, |d, label| {
                                d.role(gpui::Role::Button).aria_label(label)
                            })
                            .child(
                                Icon::new(IconName::ChevronLeft)
                                    .size(rem * FOCUS_CHEVRON_REM)
                                    .flex_shrink_0()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(path_el),
                    )
                    .into_any_element(),
            );
        }
        // The file count is the first thing a column drops: the same number is
        // one line down, at the top of the list. The totals stay — they have no
        // second home. Filled, it is also one of the two names that give, so
        // it is held between its minimum and what its line leaves, and its
        // tooltip has the whole of it.
        let summary_string = (parts.summary && !overflow).then(|| summary_text(files, untracked));
        if let Some(summary) = summary_string {
            let whole = SharedString::from(summary);
            let tip = whole.clone();
            summary_el = Some(
                div()
                    .id("diff-overlay-summary")
                    .text_size(rems(META))
                    .font_features(tabular())
                    .text_color(cx.theme().muted_foreground)
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-summary".into())
                    })
                    .when_some(caps, |d, (_, cap)| {
                        d.min_w(px(need.map_or(0., |n| n.summary_min())))
                            .max_w(px(cap))
                            .truncate()
                    })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .child(whole)
                    .into_any_element(),
            );
        }
        if parts.added {
            added_el = Some(
                div()
                    .text_size(rems(META))
                    .font_features(tabular())
                    .text_color(cx.theme().success)
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-added".into())
                    })
                    .child(format!("+{added}"))
                    .into_any_element(),
            );
        }
        if parts.removed {
            removed_el = Some(
                div()
                    .text_size(rems(META))
                    .font_features(tabular())
                    .text_color(cx.theme().danger)
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-removed".into())
                    })
                    .child(format!("−{removed}"))
                    .into_any_element(),
            );
        }
        if parts.loading {
            // Where no row of the overflow holds the word whole it takes the
            // row it has and is cut there, the way a summary word wider than
            // its row is, and says so: its whole text on hover and to assistive
            // technology. The cut is the UI's one measured elision, whose node
            // carries the whole word as its value (a `Label` node is named from
            // its value, never from an `aria_label`, which a real AT-SPI read
            // showed to come out empty). Where a row holds it, it is what it
            // always was.
            let cut = need.zip(plan).and_then(|(n, p)| n.refreshing_cut(&p));
            let word = SharedString::from(t(L10nKey::Refreshing));
            let tip = word.clone();
            let shown = match cut {
                Some(_) => crate::ui::text_elision::ElidedLabel::new(
                    "diff-overlay-refreshing-text",
                    word.clone(),
                    crate::ui::text_elision::Mode::End,
                )
                .into_any_element(),
                None => word.clone().into_any_element(),
            };
            refresh_el = Some(
                div()
                    .id("diff-overlay-refreshing")
                    .text_size(rems(META))
                    .text_color(cx.theme().muted_foreground)
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-refreshing".into())
                    })
                    .when_some(cut, |d, width| {
                        d.flex_none()
                            .w(px(width))
                            .truncate()
                            .tooltip(move |window, cx| text_tooltip(tip.clone(), window, cx))
                    })
                    .child(shown)
                    .into_any_element(),
            );
        }
        // The chip stands beside the subject on the first line, and leads the
        // second when it has left.
        // Its padding is the one the plan packed it with: the usual 5px either
        // side, and, only where the overflow's row lacks room for it, exactly
        // what the row lacks of them, down to 3.5px, its word never trimmed.
        let chip_pad = need
            .zip(plan)
            .filter(|(_, p)| p.overflow)
            .map(|(n, p)| n.chip_pad(&p));
        let chip_el = subject.chip.map(|text| match chip_pad {
            Some(pad) if pad < crate::ui::right_panel::CHIP_PAD_X => {
                crate::ui::right_panel::info_chip_padded(
                    text,
                    cx.theme().foreground.opacity(CHIP_FILL),
                    cx.theme().muted_foreground,
                    &mono,
                    pad,
                )
            }
            _ => info_chip(
                text,
                cx.theme().foreground.opacity(CHIP_FILL),
                cx.theme().muted_foreground,
                &mono,
            ),
        });
        let (chip_first, chip_second) = if moved(0) {
            (None, chip_el)
        } else {
            (chip_el, None)
        };
        let (refresh_first, refresh_second) = if moved(0) {
            (None, refresh_el)
        } else {
            (refresh_el, None)
        };
        let (summary_first, summary_second) = if moved(2) {
            (None, summary_el)
        } else {
            (summary_el, None)
        };
        // The terminal's name leaves its button for the second line when the
        // level says so; the button keeps its badge, its divider and its tip.
        let name_mode = if moved(1) {
            crate::ui::document_column::TerminalName::Moved
        } else {
            crate::ui::document_column::TerminalName::Inline(caps.map(|(name, _)| name))
        };
        let name_second = moved(1)
            .then(|| self.render_document_terminal_name(caps.map(|(name, _)| name), window, cx))
            .flatten();

        let icon_el = gpui::svg()
            .path(subject.icon)
            .flex_shrink_0()
            .size(px(SUBJECT_ICON))
            .when(cfg!(test), |d| {
                d.debug_selector(|| "diff-overlay-icon".into())
            })
            .text_color(cx.theme().muted_foreground)
            .into_any_element();
        let subject_el = if subject.is_rev {
            // A revision is an identifier, not a name: it belongs in the
            // same monospace the patch below it is set in. It is also
            // whatever the source says — an arbitrary ref, a range, a
            // patch id — so it is the one part of the header that gives
            // when the controls beside it need the room, the way the branch
            // name below does, and the tooltip keeps the whole ref.
            let full = SharedString::from(subject.full);
            div()
                .id("diff-overlay-rev")
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT_MONO))
                .font_weight(FontWeight::MEDIUM)
                .font_family(self.font_family.clone())
                .tooltip(move |window, cx| {
                    // The ref is shared with the ladder, which keeps the plain
                    // tooltip; the overflow rows' is the observable text one.
                    if overflow {
                        text_tooltip(full.clone(), window, cx)
                    } else {
                        gpui_component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                    }
                })
                .child(subject.text)
                .when(cfg!(test), |d| {
                    d.debug_selector(|| "diff-overlay-rev".into())
                })
                .painted("header-rev")
                .into_any_element()
        } else {
            // This is the name that gives, in a column and filled alike:
            // the header has the room it has rather than the room a name
            // would like, and a branch name that refused to yield any of it
            // pushed the view toggle and the close tile off the end. It
            // takes the slack the spacer below would otherwise have — the
            // same trade the label branch makes, and for the same reason
            // two `flex_1` siblings would split the line and truncate the
            // name with empty space beside it.
            let styled = |d: gpui::Div| {
                d.when(parts.slack, |d| d.flex_1().min_w_0().truncate())
                    .when(!parts.slack, |d| d.flex_shrink_0())
                    .text_size(rems(TEXT))
                    .font_weight(FontWeight::MEDIUM)
            };
            if overflow {
                // Yielding in the overflow rows, it carries its whole name.
                let tip = SharedString::from(subject.text.clone());
                styled(div())
                    .id("diff-overlay-subject")
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-subject".into())
                    })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .child(subject.text)
                    .into_any_element()
            } else {
                styled(div()).child(subject.text).into_any_element()
            }
        };
        // The subject takes the slack the spacer below would otherwise
        // have, which is why that one is skipped when a label is present:
        // two growing siblings split the line in half and the subject
        // would truncate with empty space beside it.
        //
        // `flex_auto` rather than `flex_1` for the shrinking half of that:
        // both grow the same, but `flex_1` bases the item at zero, and an
        // item based at zero has a scaled shrink factor of zero — it
        // absorbs none of a deficit and simply gets nothing, so the
        // subject would vanish first however high the others' shrink
        // factors were. Based at its content width it yields last, which
        // is the order the strip wants.
        //
        // The two of them are one group in the row, based at zero (`flex_1`)
        // and held at no less than the gap between them. A group based at
        // zero has no shrink factor, so it takes only the room left after
        // the ref and the fixed parts have what they need and gives the ref
        // none of its own: the ref gives way to nothing but a deficit that
        // is its own to take, and the label and byline are squeezed first.
        // Inside the group their order is what it was.
        let (label_el, label_piece, byline_piece) = match subject.label.as_ref() {
            None => (None, None, None),
            Some(label) if !overflow => {
                let whole = SharedString::from(label.subject.clone());
                let group = h_flex()
                    .flex_1()
                    .min_w(rems(HEADER_GAP_REM))
                    .items_center()
                    .gap(rems(HEADER_GAP_REM))
                    .child(
                        div()
                            .flex_auto()
                            .min_w_0()
                            .truncate()
                            .text_size(rems(TEXT))
                            .child(whole),
                    )
                    // Yields before the subject does, for the same reason
                    // the path below it does: an author name is unbounded
                    // too, and of the three things on this strip it is the
                    // one nobody reads twice.
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(999.)
                            .truncate()
                            .text_size(rems(META))
                            .font_features(tabular())
                            .text_color(cx.theme().muted_foreground)
                            .child(label_byline(label, now_unix())),
                    )
                    .into_any_element();
                (Some(group), None, None)
            }
            Some(label) => {
                // In the overflow rows the label and the byline are pieces of
                // their own, each drawn at no less than its planned minimum, so
                // neither can be laid out at nothing; the commit's subject
                // carries its whole text.
                let whole = SharedString::from(label.subject.clone());
                let tip = whole.clone();
                let (label_min, byline_min) = need.map_or((0., 0.), |n| {
                    (n.piece_min(Item::Label), n.piece_min(Item::Byline))
                });
                let words = div()
                    .id("diff-overlay-label-subject")
                    .flex_1()
                    .min_w(px(label_min))
                    .truncate()
                    .text_size(rems(TEXT))
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-label-subject".into())
                    })
                    .tooltip(move |window, cx| text_tooltip(tip.clone(), window, cx))
                    .child(whole)
                    .into_any_element();
                let byline = div()
                    .min_w(px(byline_min))
                    .flex_shrink(999.)
                    .truncate()
                    .text_size(rems(META))
                    .font_features(tabular())
                    .text_color(cx.theme().muted_foreground)
                    .when(cfg!(test), |d| {
                        d.debug_selector(|| "diff-overlay-label-byline".into())
                    })
                    .child(label_byline(label, now_unix()))
                    .into_any_element();
                (None, Some(words), Some(byline))
            }
        };
        let view_el = div()
            .occlude()
            .flex_shrink_0()
            .child(self.diff_view_switch(switch_lead, cx))
            .into_any_element();
        // Where the panel sits, answered by the same two cells the code panel
        // offers. This used to be a right-click menu only, so the one surface
        // whose whole point is showing half a screen of context had no way to
        // ask for the screen.
        let layout_el = self.document_layout_switch_with_lead(chrome, switch_lead, cx);
        let close_el = div()
            .occlude()
            .flex_shrink_0()
            .when(cfg!(test), |d| {
                d.debug_selector(|| "diff-overlay-close".into())
            })
            .child(
                crate::ui::tab_strip::chrome_tile_sized(
                    Button::new("diff-overlay-close").icon(Icon::new(IconName::Close)),
                    CLOSE_TILE,
                    crate::ui::tab_strip::RAIL_TILE_GLYPH,
                    false,
                    cx,
                )
                .rounded(px(crate::ui::tab_strip::RAIL_TILE_RADIUS))
                .tooltip(t(L10nKey::DiffCloseTooltip))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_diff_overlay(window, cx);
                })),
            )
            .into_any_element();

        let row = h_flex()
            .id("diff-overlay-header")
            .when(cfg!(test) && !two_line, |d| {
                d.debug_selector(|| "diff-overlay-header".into())
            });
        let row = if chrome.header_is_title_strip() {
            crate::ui::app::title_bar_drag(row, "diff-overlay-header", window, cx)
        } else {
            row
        };
        let (Some(plan), Some(need)) = (plan.filter(|p| p.overflow), need) else {
            let second_line: Vec<AnyElement> =
                [chip_second, refresh_second, name_second, summary_second]
                    .into_iter()
                    .flatten()
                    .collect();
            let first_line = row
                .flex_shrink_0()
                // The row's own hairline stays exactly as thin as it is drawn; it
                // only gives back the half point the title bar spends, so this
                // header's tiles land on the traffic lights' line rather than below.
                .title_strip_row(
                    HAIRLINE_W,
                    // Wrapped, the one rule is the second line's; this row keeps
                    // the box it has and draws nothing at its bottom edge.
                    if two_line {
                        gpui::transparent_black()
                    } else {
                        hairline(cx)
                    },
                )
                // The header's sha and paths are identifiers, not prose: set the
                // way the patch rows below are, by the Font ligatures setting.
                .font_features(code_features(cx))
                .pl(px(lead))
                // The close tile is a notch smaller than the title bar's own, and
                // is pulled in by half the difference so its glyph stays on the
                // column every other trailing tile in the window is centred on.
                .pr(px(header_trail()))
                .gap(rems(HEADER_GAP_REM))
                .items_center()
                .children(self.render_document_terminal_button_with(chrome, name_mode, window, cx))
                .child(icon_el)
                .child(subject_el)
                // A faint neutral pill, the one the graph gives HEAD: the chip
                // qualifies the branch beside it rather than competing with it,
                // and the title bar spends no accent on anything.
                .children(chip_first)
                .children(label_el)
                .children(back)
                .children(summary_first)
                .children(added_el)
                .children(removed_el)
                .children(refresh_first)
                .when(parts.spacer, |bar| bar.child(div().flex_1()))
                .child(view_el)
                .child(layout_el)
                .child(close_el);
            if !two_line {
                return first_line.into_any_element();
            }
            // The metadata that did not fit beside the subject, on a line of its
            // own below the first: every figure and word whole, the summary
            // holding what its line leaves.
            return v_flex()
                .flex_shrink_0()
                .when(cfg!(test), |d| {
                    d.debug_selector(|| "diff-overlay-header".into())
                })
                .child(first_line)
                .child(
                    h_flex()
                        .h(px(SECOND_LINE_H))
                        .items_center()
                        .pl(px(crate::ui::app::CONTENT_INSET))
                        .pr(px(header_trail()))
                        .gap(rems(HEADER_GAP_REM))
                        .font_features(code_features(cx))
                        .border_b(HAIRLINE_W)
                        .border_color(hairline(cx))
                        .children(second_line),
                )
                .into_any_element();
        };

        // No ladder level fits both lines: the first row is the terminal's
        // badge group and the close tile at the trailing end, and every other
        // piece is whole in the rows below, in the order it reads, the summary
        // last on a row of its own as prose. The widths are the ones planned.
        let first_row = row
            .flex_shrink_0()
            .title_strip_row(HAIRLINE_W, gpui::transparent_black())
            .font_features(code_features(cx))
            .pl(px(lead))
            .pr(px(header_trail()))
            .gap(rems(HEADER_GAP_REM))
            .items_center()
            .children(self.render_document_terminal_button_with(
                chrome,
                crate::ui::document_column::TerminalName::Moved,
                window,
                cx,
            ))
            .child(div().flex_1())
            .child(close_el);
        // The icon with the ref or the branch name: drawn at no less than the
        // planned minimum, so the ref never gives way below its floor.
        // Where the row cannot hold the icon, the gap and the ref, the icon is
        // alone on the row before and the ref, whole, is first on the next: it
        // is drawn at no less than the ref's own least, the same number the
        // plan packed it at.
        let (ref_el, icon_row_el) = if plan.ref_split {
            let ref_alone = h_flex()
                .items_center()
                .min_w(px(need.ref_text_min()))
                .flex_shrink(1.)
                .child(subject_el)
                .into_any_element();
            (ref_alone, Some(icon_el))
        } else {
            let atom = h_flex()
                .items_center()
                .gap(rems(HEADER_GAP_REM))
                .min_w(px(need.piece_min(Item::Ref)))
                .flex_shrink(1.)
                .child(icon_el)
                .child(subject_el)
                .into_any_element();
            (atom, None)
        };
        let name_el =
            self.render_document_terminal_name(Some(need.overflow_name_cap(&plan)), window, cx);
        // The pieces, in the order the plan packed them.
        let mut pieces: Vec<AnyElement> = vec![ref_el];
        pieces.extend(label_piece);
        pieces.extend(byline_piece);
        pieces.extend(back);
        pieces.extend(added_el);
        pieces.extend(removed_el);
        pieces.push(view_el);
        pieces.push(layout_el);
        pieces.extend(chip_first.or(chip_second));
        pieces.extend(refresh_first.or(refresh_second));
        pieces.extend(name_el);
        debug_assert_eq!(pieces.len(), need.pieces().count());
        let rule = hairline(cx);
        let row_count = usize::from(plan.rows);
        let mut grouped: Vec<Vec<AnyElement>> = (0..row_count).map(|_| Vec::new()).collect();
        // The split ref's icon leads the row of its own, before the ref's row.
        if let Some(icon) = icon_row_el {
            grouped[need.icon_row(&plan)].push(icon);
        }
        for (i, piece) in pieces.into_iter().enumerate() {
            grouped[usize::from(need.row_of(&plan)[i])].push(piece);
        }
        let mut rows: Vec<AnyElement> = Vec::with_capacity(row_count + 1);
        for (r, els) in grouped.into_iter().enumerate() {
            let closing = !parts.summary && r + 1 == row_count;
            rows.push(
                h_flex()
                    .h(px(SECOND_LINE_H))
                    .items_center()
                    .pl(px(crate::ui::app::CONTENT_INSET))
                    .pr(px(header_trail()))
                    .gap(rems(HEADER_GAP_REM))
                    .font_features(code_features(cx))
                    .when(closing, |d| d.border_b(HAIRLINE_W).border_color(rule))
                    .children(els)
                    .into_any_element(),
            );
        }
        // The summary is prose: its own row, every word an element of its own
        // (so it can break only between words), the numbers at their budgeted
        // width, a word wider than the row alone cut with its own tooltip.
        if parts.summary {
            let metrics = &self.editor.diff_header;
            let lexicon = &metrics.lexicon;
            let cap_w = plan.summary_cap();
            let muted = cx.theme().muted_foreground;
            let words: Vec<AnyElement> = lexicon
                .render_words(files, untracked)
                .into_iter()
                .enumerate()
                .map(|(i, (shared, budget))| {
                    // Every word's box is the width planned for it, or the
                    // row's if it is wider than that, where it is cut alone.
                    let over = budget > cap_w + 0.01;
                    let tip = shared.clone();
                    div()
                        .id(("diff-overlay-summary-word", i))
                        .flex_none()
                        .w(px(budget.min(cap_w)))
                        .text_size(rems(META))
                        .font_family(lexicon.family.clone())
                        .font_weight(FontWeight::NORMAL)
                        .not_italic()
                        .font_features(lexicon.features.clone())
                        .h(px(lexicon.line_h))
                        .line_height(px(lexicon.line_h))
                        .text_color(muted)
                        .when(over, |d| {
                            d.truncate().tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                            })
                        })
                        .when(cfg!(test), |d| {
                            d.debug_selector(move || format!("diff-overlay-summary-word-{i}"))
                        })
                        .child(shared)
                        .into_any_element()
                })
                .collect();
            rows.push(
                h_flex()
                    .h(px(plan.summary_h))
                    .items_center()
                    .pl(px(crate::ui::app::CONTENT_INSET))
                    .pr(px(header_trail()))
                    .border_b(HAIRLINE_W)
                    .border_color(rule)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            // Held to the plan: as tall as its lines, the lines
                            // from the top, no stretching of them or the words.
                            .items_start()
                            .content_start()
                            .gap_x(px(metrics.lexicon.space))
                            .w(px(cap_w))
                            .h(px(f32::from(plan.state.lines) * lexicon.line_h))
                            .when(cfg!(test), |d| {
                                d.debug_selector(|| "diff-overlay-summary".into())
                            })
                            .children(words),
                    )
                    .into_any_element(),
            );
        }
        v_flex()
            .flex_shrink_0()
            .when(cfg!(test), |d| {
                d.debug_selector(|| "diff-overlay-header".into())
            })
            .child(first_row)
            .children(rows)
            .into_any_element()
    }

    /// The two views, as a drawn switch on the track the layout switch sits on.
    ///
    /// Text labels said "Split" and "Unified" in whatever language the interface
    /// is set to, in a row that carries no other words of its own, and the choice
    /// is a picture of the layout rather than a name for it. Same two answers, now
    /// the same two cells as Dock / Fill beside them — which has always been drawn
    /// rather than written, so the pair would have looked like two unrelated
    /// widgets sharing one strip.
    ///
    /// The tooltip still carries the words: a divider down a frame is not
    /// self-explanatory, and neither is a set of rules across one.
    fn diff_view_switch(&self, lead: f32, cx: &mut Context<Self>) -> AnyElement {
        let current = view_mode(cx);
        let cell =
            |i: usize, mode: DiffViewMode, glyph: fn(gpui::Hsla) -> AnyElement, tip: L10nKey| {
                let live = mode == current;
                let ink = switch_ink(live, cx);
                switch_cell(
                    ("diff-overlay-view-cell", i),
                    glyph(ink),
                    live,
                    tip,
                    cx.listener(move |this, _, _window, cx| {
                        this.update_config(cx, |cfg| cfg.diff_view = mode);
                    }),
                    cx,
                )
            };
        crate::ui::document_column::switch_track_with_lead(
            "diff-overlay-view",
            vec![
                cell(0, DiffViewMode::Split, glyph_split, L10nKey::DiffViewSplit),
                cell(
                    1,
                    DiffViewMode::Unified,
                    glyph_unified,
                    L10nKey::DiffViewUnified,
                ),
            ],
            lead,
            cx,
        )
    }

    /// Dispatch the byte read behind an untracked file's preview, at most
    /// once per (path, snapshot). Runs from `render`, so the guards are the
    /// point: `preview` says the answer is in hand, `preview_loading` says it
    /// is on the way.
    fn spawn_untracked_preview_if_needed(&mut self, cx: &mut Context<Self>) {
        let want = {
            let overlay = self
                .tabs
                .get(self.active)
                .and_then(|t| t.diff_overlay.as_ref());
            match overlay {
                Some(o) => match &o.load {
                    DiffLoad::Ready(snap) => {
                        untracked_focus(snap, o.focus.as_deref()).and_then(|path| {
                            let seen = o.preview.as_ref().is_some_and(|(held, _)| held == path)
                                || o.preview_loading.as_deref() == Some(path);
                            (!seen).then(|| (o.host_id, snap.root.clone(), path.to_string()))
                        })
                    }
                    _ => None,
                },
                None => None,
            }
        };
        let Some((host_id, root, path)) = want else {
            return;
        };
        let Some(host) = crate::ui::host_registry::HostRegistry::lookup(cx, host_id) else {
            return;
        };
        let active = self.active;
        if let Some(o) = self
            .tabs
            .get_mut(active)
            .and_then(|t| t.diff_overlay.as_mut())
        {
            o.preview_loading = Some(path.clone());
        }
        let read_path = root.join(&path);
        let key_path = path.clone();
        crate::ui::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                h.read_file(&read_path, MAX_PREVIEW_BYTES)
                    .ok()
                    .map(|bytes| {
                        Arc::new(git_diff::synthesize_added(
                            &path,
                            &bytes,
                            &git_diff::DiffBudget::SINGLE_FILE,
                        ))
                    })
            },
            move |this, file, cx| {
                let active = this.active;
                let Some(o) = this
                    .tabs
                    .get_mut(active)
                    .and_then(|t| t.diff_overlay.as_mut())
                    .filter(|o| o.host_id == host_id)
                else {
                    return;
                };
                if o.preview_loading.as_deref() == Some(key_path.as_str()) {
                    o.preview_loading = None;
                }
                o.preview = Some((key_path.clone(), file));
                cx.notify();
            },
        );
    }

    fn diff_message(&self, text: &'static str, cx: &Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(rems(TEXT))
            .text_color(cx.theme().muted_foreground)
            .child(text)
            .into_any_element()
    }

    /// Brings the active overlay's flattened rows up to date with what it is
    /// meant to be showing, and says what to draw.
    ///
    /// Called from `render`, so the [`RowsKey`] comparison is what keeps it
    /// cheap: flattening a twenty-thousand-line patch allocates a row per
    /// line, and nothing about that changes between two frames of scrolling.
    fn sync_diff_rows(&mut self, cx: &mut Context<Self>) -> Option<DiffBody> {
        let mode = view_mode(cx);
        let active = self.active;
        let overlay = self.tabs.get_mut(active)?.diff_overlay.as_mut()?;

        let snap = match &overlay.load {
            DiffLoad::Loading => return Some(DiffBody::Message(t(L10nKey::DiffReading))),
            DiffLoad::NotARepo => return Some(DiffBody::Message(t(L10nKey::DiffNotARepo))),
            DiffLoad::Ready(snap) if empty_snapshot(snap) && snap.read_failed => {
                return Some(DiffBody::Message(t(L10nKey::DiffReadFailed)));
            }
            DiffLoad::Ready(snap) if empty_snapshot(snap) => {
                return Some(DiffBody::Message(t(L10nKey::DiffWorkingTreeClean)));
            }
            DiffLoad::Ready(snap) => Arc::clone(snap.snapshot()),
        };

        // A focused *untracked* file has no patch in the snapshot; its card is
        // synthesized from the file's own bytes — see `preview`.
        let preview = match untracked_focus(&snap, overlay.focus.as_deref()) {
            Some(path) => match &overlay.preview {
                Some((held, file)) if held == path => match file {
                    Some(file) => Some(Arc::clone(file)),
                    None => return Some(DiffBody::Message(t(L10nKey::DiffReadFailed))),
                },
                _ => return Some(DiffBody::Message(t(L10nKey::DiffReading))),
            },
            None => None,
        };

        // Forget a range the rows under it no longer answer to. The two views
        // pair the same lines differently, so a row range drawn in one of them
        // points at other code in the other. Here rather than beside the
        // switch that flips the mode: this is the one place that knows which
        // rows are about to be drawn.
        let drawn = |path: &str| {
            preview
                .as_deref()
                .filter(|f| f.path == path)
                .or_else(|| snap.files.iter().find(|f| f.path == path))
                .map_or(mode, |f| crate::ui::diff_list::file_mode(f, mode))
        };
        if overlay
            .selection
            .as_ref()
            .is_some_and(|s| s.mode != drawn(&s.path))
        {
            overlay.selection = None;
            overlay.selecting = false;
        }

        let focused = focused_file(&snap, overlay);
        let from = RowsFrom {
            snap: &snap,
            preview: preview.as_ref(),
            mode,
            focused,
            oversized: focused.is_none() && snap.stats().oversized,
            expanded: &overlay.expanded,
        };
        let stale = overlay
            .rows_key
            .as_ref()
            .is_none_or(|held| !held.describes(&from));
        if stale {
            let rows = match from.preview {
                Some(file) => crate::ui::diff_list::preview_rows(file, from.mode),
                None => crate::ui::diff_list::build_rows(
                    from.snap,
                    from.expanded,
                    from.focused,
                    from.mode,
                    from.oversized,
                ),
            };
            let key = from.to_key();
            resync_list(&overlay.list, &overlay.rows, &rows);
            overlay.rows = Rc::new(rows);
            overlay.rows_key = Some(key);
        } else if let Some(held) = overlay.rows_key.as_mut() {
            held.retarget(&from);
        }
        Some(DiffBody::Rows(snap))
    }

    /// The rows, in the virtualised list that draws only the visible ones.
    fn diff_rows_list(
        &self,
        overlay: &DiffOverlayState,
        snap: Arc<DiffSnapshot>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = Rc::clone(&overlay.rows);
        let font = SharedString::from(self.font_family.clone());
        let app = cx.entity().downgrade();
        let list = overlay.list.clone();
        // The selection is read here, once a frame, rather than keyed into
        // `RowsKey`: it changes what a row *looks like*, not which rows there
        // are, and re-flattening the patch for every step of a drag is the
        // cost this list exists to avoid. `extend_diff_selection` notifies,
        // the view renders, and the list rebuilds the rows on screen from the
        // `Drag` this frame carries.
        let drag = Drag {
            sel: overlay.selection.clone().map(Rc::new),
            selecting: overlay.selecting,
        };
        let body = gpui::list(list.clone(), move |ix, _window, cx| {
            #[cfg(test)]
            row_probe::record();
            match rows.get(ix) {
                Some(row) => diff_row_element(row, ix, &drag, &font, &snap, &app, cx),
                // The list is spliced in step with `rows`, so this is
                // unreachable — and an empty row is a better answer to a bug
                // than an index panic in a paint.
                None => div().into_any_element(),
            }
        })
        .size_full()
        // Only the vertical padding: `List` lays every item out at its own
        // full width and puts it at its own left edge, so a horizontal
        // padding here would be silently ignored. The rows carry their own —
        // see `diff_row_element`.
        .py(px(LIST_INSET));
        // The bar reads the list's own height, and a list only counts the
        // rows it has measured. Left at that, a patch of any length would
        // report itself as one screen long and the thumb would fill the
        // track: a drag from top to bottom would travel one screen and stop,
        // on the one document in the app long enough to need the bar. The
        // rows below the fold are counted at `DIFF_LINE_H` until they are
        // laid out — `ListState::measure_all` would settle it exactly, by
        // laying out every row on the first frame, which is the cost this
        // whole list exists to avoid.
        crate::ui::scrollbar::with_vertical_scrollbar("diff-overlay-scrollbar", body, &list)
    }
}

/// Counts the rows the list actually built, so a test can tell that a patch of
/// any size costs the handful of rows on screen rather than all of them.
#[cfg(test)]
pub(crate) mod row_probe {
    use std::cell::Cell;

    thread_local! {
        static BUILT: Cell<u64> = const { Cell::new(0) };
    }

    pub(crate) fn record() {
        BUILT.set(BUILT.get() + 1);
    }

    /// The count since the last call, and zero from here.
    #[cfg(not(target_os = "windows"))]
    pub(crate) fn take() -> u64 {
        BUILT.replace(0)
    }

    type Seen = (&'static str, gpui::FontFeatures);

    thread_local! {
        static PAINTED: std::cell::RefCell<Vec<Seen>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub(crate) fn record_painted(label: &'static str, features: gpui::FontFeatures) {
        PAINTED.with(|seen| seen.borrow_mut().push((label, features)));
    }

    /// What the probes saw since the last call, in paint order.
    pub(crate) fn take_painted() -> Vec<Seen> {
        PAINTED.take()
    }
}

/// Test-only paint observer. Under `cfg(test)` it appends a zero-size,
/// out-of-flow child that records the font features in force where it paints,
/// so a test can read what the shaper would be handed rather than what a
/// builder claims. Outside tests it is the identity.
trait Painted: Sized {
    fn painted(self, label: &'static str) -> Self;
}

#[cfg(test)]
impl<E: gpui::ParentElement> Painted for E {
    fn painted(self, label: &'static str) -> Self {
        self.child(
            gpui::canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    row_probe::record_painted(label, window.text_style().font_features)
                },
            )
            .absolute()
            .size_0(),
        )
    }
}

#[cfg(not(test))]
impl<E> Painted for E {
    fn painted(self, _label: &'static str) -> Self {
        self
    }
}

/// What a row needs to take part in a drag, for the frame it is drawn in.
///
/// Read off the overlay once and moved into the list's item builder, so a step
/// of a drag costs a refcount bump rather than a walk of the patch.
struct Drag {
    sel: Option<Rc<DiffSelection>>,
    /// Whether a drag is in flight. Rows only listen for pointer movement
    /// while one is: a diff runs to thousands of rows, and a listener each is
    /// worth paying for during a drag and not otherwise.
    selecting: bool,
}

impl Drag {
    /// Whether the selection covers this cell. `side` names the column a split
    /// cell sits in, and is `None` for a unified row — a selection made in one
    /// column never lights up the other.
    fn covers(&self, at: &RowAt, side: Option<Side>) -> bool {
        self.sel
            .as_ref()
            .is_some_and(|sel| sel.covers(at.path.as_ref(), at.id, side))
    }

    /// Whether this row is inside the selection at all, whichever column the
    /// drag ran down. What decides whether the row offers to copy it.
    fn holds(&self, at: &RowAt) -> bool {
        self.sel
            .as_ref()
            .is_some_and(|sel| sel.covers(at.path.as_ref(), at.id, sel.side))
    }
}

/// What the overlay's scrolling area holds this frame.
enum DiffBody {
    Message(&'static str),
    /// The rows are in [`DiffOverlayState::rows`]; the snapshot rides along
    /// for the few rows whose text is derived from it.
    Rows(Arc<DiffSnapshot>),
}

/// What this frame would flatten its rows from, borrowed from the overlay.
struct RowsFrom<'a> {
    snap: &'a Arc<DiffSnapshot>,
    preview: Option<&'a Arc<FileDiff>>,
    mode: DiffViewMode,
    focused: Option<usize>,
    oversized: bool,
    expanded: &'a HashMap<String, bool>,
}

impl RowsFrom<'_> {
    fn to_key(&self) -> RowsKey {
        RowsKey {
            snap: Arc::clone(self.snap),
            preview: self.preview.cloned(),
            mode: self.mode,
            focused: self.focused,
            oversized: self.oversized,
            expanded: self.expanded.clone(),
        }
    }
}

/// What [`DiffOverlayState::rows`] was flattened from, kept so the next frame
/// can tell whether it would flatten the same rows again.
struct RowsKey {
    snap: Arc<DiffSnapshot>,
    preview: Option<Arc<FileDiff>>,
    mode: DiffViewMode,
    focused: Option<usize>,
    oversized: bool,
    expanded: HashMap<String, bool>,
}

impl RowsKey {
    /// Whether the rows built from `from` would be the rows already held.
    ///
    /// The snapshot is compared by pointer first and by contents second: a
    /// probe that found nothing new still lands a fresh `Arc` over an equal
    /// snapshot, and rebuilding every row of the patch for that would undo the
    /// point of keeping them.
    ///
    /// The scalars go first so that the walk of the patch behind that second
    /// comparison is only ever paid to answer a question the cheap fields
    /// have not already answered.
    fn describes(&self, from: &RowsFrom<'_>) -> bool {
        self.mode == from.mode
            && self.focused == from.focused
            && self.oversized == from.oversized
            && self.expanded == *from.expanded
            && self.same_preview(from)
            && (Arc::ptr_eq(&self.snap, from.snap) || self.snap == *from.snap)
    }

    /// The preview, by pointer and then by contents — a re-read of an
    /// untracked file lands a fresh `Arc` over bytes that did not change.
    fn same_preview(&self, from: &RowsFrom<'_>) -> bool {
        match (&self.preview, from.preview) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            _ => false,
        }
    }

    /// Points the key at the `Arc`s this frame was asked about, having just
    /// found them equal to the ones held.
    ///
    /// Without this the key goes on holding the snapshot from the last
    /// *rebuild*, so every frame after a probe that found nothing new proves
    /// the two equal the long way — a walk of every line of the patch, once
    /// per wheel event, which is the cost this key exists to avoid.
    fn retarget(&mut self, from: &RowsFrom<'_>) {
        self.snap = Arc::clone(from.snap);
        self.preview = from.preview.cloned();
    }
}

/// Tells the list which rows changed, rather than that all of them did.
///
/// `ListState::reset` would drop the scroll position, so collapsing one file
/// would throw the reader back to the top of the tree. The rows either side of
/// an edit are untouched, so the shared prefix and suffix are kept and only
/// what is between them is spliced.
fn resync_list(list: &gpui::ListState, old: &[DiffRow], new: &[DiffRow]) {
    let (replaced, with) = spliced_range(old, new);
    list.splice(replaced, with);
}

/// Which of the old rows were replaced, and by how many new ones.
fn spliced_range(old: &[DiffRow], new: &[DiffRow]) -> (std::ops::Range<usize>, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    // Whatever is left of the shorter list once the shared head is off it —
    // the most the shared tail can be, and what keeps the two slices below in
    // step with each other.
    let rest = old.len().min(new.len()) - prefix;
    let suffix = old[old.len() - rest..]
        .iter()
        .rev()
        .zip(new[new.len() - rest..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (prefix..old.len() - suffix, new.len() - prefix - suffix)
}

/// How far a row's pill sits in from the edge of the list: the window's
/// `CONTENT_INSET`, the same 12px the rail and the right panel keep, and the
/// same inset the header's subject starts at.
const LIST_INSET: f32 = crate::ui::app::CONTENT_INSET;

/// The padding a row keeps inside its pill — the rail's 8px — so a hovered
/// row's fill reaches past its text on both sides. The overlay is a second
/// view of the Changes tab's list, and the two stopped looking like one app
/// when this one drew cards.
const ROW_INSET: Pixels = px(8.);

/// The height of a row that is a *file* rather than a line of one: the same
/// 26px the panel gives its file rows.
const FILE_ROW_H: Pixels = px(26.);

/// The radius on a row that lights up under the pointer: the 6px the Changes
/// tab's 26px rows use.
const ROW_RADIUS: Pixels = px(6.);

/// The header's pills — the view switch and the way back to the list — are
/// one row tall, so the title bar's controls are the list's rows in another
/// place rather than a smaller widget of their own.
const CONTROL_H: Pixels = FILE_ROW_H;

/// The header's close tile: the rail header's 26px tile, not the title bar's
/// 32. The diff header already carries a subject, a switch and a path; the
/// way out should be the quietest shape on it.
const CLOSE_TILE: f32 = crate::ui::tab_strip::RAIL_TILE;

/// The fill under a neutral chip, the same faint ink the graph's HEAD chip
/// wears.
const CHIP_FILL: f32 = 0.06;

/// What the header is drawn from and budgeted from, one definition for both:
/// the subject's icon, the gap between its children (a share of the rem, so
/// it follows the interface font), the focused file's back button (its inset,
/// the gap inside it and its chevron, which is a share of the rem too), and the
/// room the close tile leaves at the end of the row.
const SUBJECT_ICON: f32 = 13.;
/// The height of the filled header's second line, which carries the metadata
/// that did not fit beside the subject.
const SECOND_LINE_H: f32 = 36.;
const HEADER_GAP_REM: f32 = 0.5;
const FOCUS_PAD_L: f32 = 4.;
const FOCUS_GAP_REM: f32 = 0.25;
const FOCUS_CHEVRON_REM: f32 = 0.875;

/// The close tile is a notch smaller than the title bar's own, and is pulled in
/// by half the difference so its glyph stays on the column every other
/// trailing tile in the window is centred on.
fn header_trail() -> f32 {
    crate::ui::app::tile_trailing_inset() + (crate::ui::app::TILE_SIZE - CLOSE_TILE) / 2.
}

/// Which of the header's optional children are there. The render asks it what
/// to draw and the budget asks it how many gaps and which fixed labels to
/// count, so what is budgeted is what is drawn.
#[derive(Clone, Copy)]
struct HeaderParts {
    /// The branch name takes the slack the row has.
    slack: bool,
    /// The filled header leads with the terminal button.
    terminal: bool,
    staged: bool,
    labelled: bool,
    focused: bool,
    summary: bool,
    added: bool,
    removed: bool,
    loading: bool,
    spacer: bool,
}

impl HeaderParts {
    fn of(
        overlay: &DiffOverlayState,
        shape: SourceShape,
        chrome: DocumentChrome,
        (added, removed): (u32, u32),
    ) -> Self {
        let ready = matches!(overlay.load, DiffLoad::Ready(_));
        let stats = ready && overlay.focus.is_none();
        // The branch name is the part that gives, in a column and filled alike:
        // a name that refuses to yield pushes the controls off the end.
        let slack = !shape.is_rev && !shape.labelled;
        Self {
            slack,
            terminal: chrome == DocumentChrome::Fill,
            staged: shape.staged,
            labelled: shape.labelled,
            focused: focused_name_ref(overlay).is_some(),
            summary: stats && !chrome.is_dock(),
            added: stats && added > 0,
            removed: stats && removed > 0,
            loading: overlay.loading && ready,
            spacer: !shape.labelled && !slack,
        }
    }

    /// How many children the row has that never leave it: the terminal button
    /// of a filled header, the icon and the subject, the focused file's way
    /// back, the two totals, the spacer, the two switches and the close tile.
    ///
    /// A label still counts as the two pieces it is made of, though the row
    /// holds them as one group: the gap inside that group, which the group is
    /// held to no less than, is the gap the second piece would have had in the
    /// row. The count, and so the gaps the budget reserves, is what it was.
    fn fixed_children(&self) -> usize {
        let one = |there: bool| usize::from(there);
        one(self.terminal)
            + 2
            + 2 * one(self.labelled)
            + one(self.focused)
            + one(self.added)
            + one(self.removed)
            + one(self.spacer)
            + 3
    }
}

/// What the header's fixed words and figures measure at the current fonts,
/// size and language: the Staged chip's word, the Refreshing word, and the
/// glyphs the added and removed counts are made of.
///
/// Measured as they are drawn — the chip in the editor font, the others in
/// the interface font; the counts with tabular figures at the meta size; the
/// words with the header's own feature set — and only when what they depend on
/// has changed: the interface size, the two font families, the feature set the
/// header inherits (so a live ligature or custom-tag edit measures again) and
/// the two words' language. Drawing and the budget only read them. The ref is
/// not in here: it truncates, so it needs no room of its own, and the figures
/// are counted from their digits when they are read.
#[derive(Clone)]
pub(crate) struct DiffHeaderMetrics {
    rem: f32,
    ui_family: SharedString,
    mono_family: String,
    features: gpui::FontFeatures,
    staged: &'static str,
    refreshing: &'static str,
    /// Whole words, rounded up the way a drawn run is.
    staged_w: f32,
    refreshing_w: f32,
    /// Glyphs of the counts, as the shaper reports them; a count rounds up
    /// once, as a whole run does.
    plus: f32,
    minus: f32,
    digit: f32,
    /// One object id at its short length, [`OID_SHORT`] characters, as one run
    /// as the ref is drawn: the editor font, medium weight, the mono size and
    /// the header's features. What the header keeps whole of any ref.
    rev_floor: f32,
    /// The least a cut summary and a cut terminal name keep: three digits and
    /// the ellipsis, as one run each in the font and size they are drawn at.
    /// A readable design minimum for the filled header, not a promise of three
    /// of any glyphs.
    summary_min: f32,
    name_min: f32,
    /// The least a commit's label and its byline keep in the overflow rows, in
    /// the fonts and sizes they are drawn at (three digits and an ellipsis,
    /// like the minima above), and the least the focused file's path keeps
    /// beside its chevron: a head, an ellipsis and a tail in the path's own
    /// monospace face. Measured here, with the others, and nowhere else.
    label_min: f32,
    byline_min: f32,
    path_min: f32,
    /// The ten templates the summary can be made of in the current language,
    /// and every word of them, measured: the language and the summary's font
    /// are what the words depend on, never the counts.
    templates: [&'static str; 10],
    lexicon: SummaryLexicon,
}

impl DiffHeaderMetrics {
    pub(crate) fn measure(window: &Window, cx: &gpui::App, mono_family: &str) -> Self {
        let rem = cx.global::<Config>().ui_font_size;
        let ui_family = cx.theme().font_family.clone();
        let features = code_features(cx);
        let font = |family: SharedString, features: gpui::FontFeatures| gpui::Font {
            family,
            features,
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
        };
        let text = window.text_system();
        let (meta, meta_mono) = (rem * META, rem * META_MONO);
        let figures = font(ui_family.clone(), tabular());
        let figure = |glyph: &str| crate::ui::tab_strip::measure_text(text, &figures, meta, glyph);
        let (staged, refreshing) = (t(L10nKey::ScmChipStaged), t(L10nKey::Refreshing));
        let chip_font = font(
            SharedString::from(mono_family.to_string()),
            features.clone(),
        );
        let rev_font = gpui::Font {
            weight: FontWeight::MEDIUM,
            ..chip_font.clone()
        };
        let word_font = font(ui_family.clone(), features.clone());
        let templates = summary_templates();
        let lexicon = SummaryLexicon::measure(window, &figures, meta, &templates);
        Self {
            rem,
            ui_family,
            mono_family: mono_family.to_string(),
            features,
            staged,
            refreshing,
            staged_w: crate::ui::tab_strip::measure_text(text, &chip_font, meta_mono, staged)
                .ceil(),
            refreshing_w: crate::ui::tab_strip::measure_text(text, &word_font, meta, refreshing)
                .ceil(),
            plus: figure("+"),
            minus: figure("−"),
            // The widest of the ten digits, measured once with the summary's
            // words: a font that ignores tabular figures cannot under-price a
            // count.
            digit: lexicon.digit,
            rev_floor: crate::ui::tab_strip::measure_text(
                text,
                &rev_font,
                rem * TEXT_MONO,
                &"0".repeat(OID_SHORT),
            )
            .ceil(),
            summary_min: crate::ui::tab_strip::measure_text(text, &figures, meta, "000\u{2026}")
                .ceil(),
            name_min: crate::ui::tab_strip::measure_text(
                text,
                &word_font,
                rem * crate::ui::document_column::HEADER_CELL_TEXT,
                "000\u{2026}",
            )
            .ceil(),
            label_min: crate::ui::tab_strip::measure_text(
                text,
                &word_font,
                rem * TEXT,
                "000\u{2026}",
            )
            .ceil(),
            byline_min: crate::ui::tab_strip::measure_text(text, &figures, meta, "000\u{2026}")
                .ceil(),
            path_min: crate::ui::tab_strip::measure_text(
                text,
                &chip_font,
                meta_mono,
                "000\u{2026}000",
            )
            .ceil(),
            templates,
            lexicon,
        }
    }

    pub(crate) fn is_for(&self, mono_family: &str, cx: &gpui::App) -> bool {
        self.rem == cx.global::<Config>().ui_font_size
            && self.ui_family == cx.theme().font_family
            && self.mono_family == mono_family
            && self.features == code_features(cx)
            && self.staged == t(L10nKey::ScmChipStaged)
            && self.refreshing == t(L10nKey::Refreshing)
            && self.templates == summary_templates()
    }

    /// A count drawn as `sign` and the digits of `n`, as a run.
    fn count_w(&self, sign: f32, n: u32) -> f32 {
        (sign + crate::ui::tab_strip::count_digits(n as usize) as f32 * self.digit).ceil()
    }
}

impl Tty59App {
    /// Where the diff header's first control starts: a header that begins at
    /// the window's left edge has the traffic lights to clear, a column's does
    /// not.
    fn diff_header_lead(&self, chrome: DocumentChrome, cx: &gpui::App) -> f32 {
        if self.left_panel_open(cx) || chrome.is_dock() {
            crate::ui::app::CONTENT_INSET
        } else {
            crate::ui::app::TITLE_BAR_LEAD
        }
    }

    /// The least width the hoisted diff header needs to keep every control
    /// and every fixed part of what it says: the lead and trail, the icon,
    /// the two switches and the close tile, the Staged chip, the back button
    /// and the figures and words that are there now, and the gaps between the
    /// children that are there. The ref and the other names give: they
    /// truncate, so they are not in it. Read-only, from the overlay as it is.
    pub(crate) fn diff_header_need(&self, cx: &gpui::App) -> f32 {
        let Some(overlay) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.diff_overlay.as_ref())
        else {
            return 0.;
        };
        self.diff_need(overlay, DocumentChrome::DockHoisted, 0., cx)
            .one_line
    }

    /// The filled header's budget, from the overlay as it is.
    pub(crate) fn diff_fill_need(
        &self,
        overlay: &DiffOverlayState,
        window: &Window,
        cx: &gpui::App,
    ) -> DiffNeed {
        let divider = crate::ui::theme::hairline(window).as_f32();
        self.diff_need(overlay, DocumentChrome::Fill, divider, cx)
    }

    /// What the header's parts add up to at `chrome`: the same definition of
    /// each part as the one that draws it. Arithmetic over what is already
    /// measured; nothing is shaped here.
    ///
    /// Every figure is read off what the diff has, never off where the parts
    /// are drawn now: the tiers that may leave the first line are there or not
    /// by the overlay, so a level's budget is the same whichever line each
    /// part happens to be on.
    fn diff_need(
        &self,
        overlay: &DiffOverlayState,
        chrome: DocumentChrome,
        divider: f32,
        cx: &gpui::App,
    ) -> DiffNeed {
        let (totals, files, untracked) = match &overlay.load {
            DiffLoad::Ready(loaded) => {
                let stats = loaded.stats();
                (stats.totals, loaded.files.len(), stats.untracked_count)
            }
            _ => ((0, 0), 0, 0),
        };
        let shape = source_shape(&overlay.source);
        let parts = HeaderParts::of(overlay, shape, chrome, totals);
        let m = &self.editor.diff_header;
        let gap = HEADER_GAP_REM * m.rem;
        // What never leaves the first line: the icon, the two switches' tracks
        // (their margins are given separately, they may yield), the close
        // tile, a ref at the least it keeps, the focused file's way back and
        // the two totals, whole.
        let mut core = SUBJECT_ICON
            + 2. * (crate::ui::document_column::layout_switch_w()
                - crate::ui::document_column::SWITCH_LEAD)
            + CLOSE_TILE;
        if shape.is_rev {
            core += m.rev_floor;
        }
        let back_w =
            FOCUS_PAD_L + FOCUS_CHEVRON_REM * m.rem + FOCUS_GAP_REM * m.rem + f32::from(ROW_INSET);
        if parts.focused {
            core += back_w;
        }
        let (added_w, removed_w) = (m.count_w(m.plus, totals.0), m.count_w(m.minus, totals.1));
        if parts.added {
            core += added_w;
        }
        if parts.removed {
            core += removed_w;
        }
        let (pill, pill_badge) = if parts.terminal {
            (
                crate::ui::document_column::terminal_button_fixed_w(divider),
                crate::ui::document_column::terminal_badge_only_w(divider),
            )
        } else {
            (0., 0.)
        };
        // The tiers, in the order they leave the first line: the Staged chip
        // with the Refreshing word, the terminal's name, the summary.
        let chip_w = if parts.staged {
            m.staged_w + 2. * crate::ui::right_panel::CHIP_PAD_X
        } else {
            0.
        };
        let refreshing_w = if parts.loading { m.refreshing_w } else { 0. };
        let tiers = [
            Tier {
                present: parts.staged || parts.loading,
                min: chip_w + refreshing_w,
                kids_first: usize::from(parts.staged) + usize::from(parts.loading),
                kids_second: usize::from(parts.staged) + usize::from(parts.loading),
                flex: false,
            },
            Tier {
                present: parts.terminal,
                min: m.name_min,
                kids_first: 0,
                kids_second: 1,
                flex: true,
            },
            Tier {
                present: parts.summary,
                min: m.summary_min,
                kids_first: 1,
                kids_second: 1,
                flex: true,
            },
        ];
        // What the overflow rows pack, in the order they read: the icon with
        // the ref or the branch name, the commit's label and its byline, the
        // way back with a readable tail of the path, the totals, the two
        // switches (with their margins), the chip, the Refreshing word, the
        // terminal's name. Each is packed at the least it keeps, and drawn at no
        // less: the same number.
        let ref_min = SUBJECT_ICON
            + gap
            + if shape.is_rev {
                m.rev_floor
            } else {
                m.name_min
            };
        let switch_w = crate::ui::document_column::layout_switch_w();
        let mut items = [ItemSpec {
            item: Item::Ref,
            w: 0.,
        }; ITEM_MAX];
        let mut n_items = 0;
        for (present, item, w) in [
            (true, Item::Ref, ref_min),
            (parts.labelled, Item::Label, m.label_min),
            (parts.labelled, Item::Byline, m.byline_min),
            (parts.focused, Item::Back, back_w + m.path_min),
            (parts.added, Item::Added, added_w),
            (parts.removed, Item::Removed, removed_w),
            (true, Item::View, switch_w),
            (true, Item::Layout, switch_w),
            (parts.staged, Item::Chip, chip_w),
            (parts.loading, Item::Refreshing, refreshing_w),
            (parts.terminal, Item::Name, m.name_min),
        ] {
            if present {
                items[n_items] = ItemSpec { item, w };
                n_items += 1;
            }
        }
        // The summary, as the words the lexicon prices: the templates its two
        // counts select, never shaped or formatted here.
        let words = if parts.summary {
            m.lexicon.words(files, untracked)
        } else {
            SummaryWords::default()
        };
        let mut need = DiffNeed {
            one_line: 0.,
            lead: self.diff_header_lead(chrome, cx),
            trail: header_trail(),
            gap,
            core,
            pill,
            pill_badge,
            fixed_children: parts.fixed_children(),
            tiers,
            items,
            n_items,
            words,
            space: m.lexicon.space,
            line_h: m.lexicon.line_h,
            summary: parts.summary,
            ref_text: if shape.is_rev {
                m.rev_floor
            } else {
                m.name_min
            },
            back_chrome: back_w,
        };
        need.one_line = need.first_line(0).total();
        need
    }
}

/// The summary as the header says it for a diff of `files` files and
/// `untracked` untracked ones.
fn summary_text(files: usize, untracked: usize) -> String {
    let mut summary = t_plural(L10nKey::DiffChangedFiles, files, &[]);
    if untracked > 0 {
        summary.push_str(&t_plural(L10nKey::DiffUntrackedCount, untracked, &[]));
    }
    summary
}

/// The ten templates the summary can be made of in the current language: the
/// changed files' five and the untracked count's five.
fn summary_templates() -> [&'static str; 10] {
    let (changed, untracked) = (
        crate::ui::i18n::plural_templates(L10nKey::DiffChangedFiles),
        crate::ui::i18n::plural_templates(L10nKey::DiffUntrackedCount),
    );
    [
        changed[0],
        changed[1],
        changed[2],
        changed[3],
        changed[4],
        untracked[0],
        untracked[1],
        untracked[2],
        untracked[3],
        untracked[4],
    ]
}

/// What a header's pieces are, in the order the overflow rows read them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Item {
    /// The icon and the ref or the branch name.
    Ref,
    /// A commit's or a pull request's label, and its byline.
    Label,
    Byline,
    Back,
    Added,
    Removed,
    View,
    Layout,
    Chip,
    Refreshing,
    Name,
}

/// How many pieces the overflow rows pack.
const ITEM_MAX: usize = 11;

#[derive(Clone, Copy)]
struct ItemSpec {
    item: Item,
    /// The least it takes in a row.
    w: f32,
}

/// The most words the summary's plan holds: two templates' worth, each of at
/// most half of it (see [`SummaryLexicon::measure`]).
const SUMMARY_WORDS_MAX: usize = 32;

/// The widths the summary's words take, in the order they read, each a whole
/// element: bounded, on the stack.
#[derive(Clone, Copy)]
struct SummaryWords {
    n: usize,
    w: [f32; SUMMARY_WORDS_MAX],
}

impl Default for SummaryWords {
    fn default() -> Self {
        Self {
            n: 0,
            w: [0.; SUMMARY_WORDS_MAX],
        }
    }
}

impl SummaryWords {
    /// How many lines the words take in `cap` pixels, broken only between
    /// words with `space` between: a word wider than `cap` is a capped word on
    /// a line of its own, the words before it end their line and the words
    /// after it begin a new one. The comparison is exact: `cap` is whole
    /// pixels, floored, and is the width the row is drawn at.
    fn lines(&self, space: f32, cap: f32) -> u8 {
        let (mut lines, mut used) = (0u8, 0.);
        for &w in &self.w[..self.n] {
            if w > cap {
                lines += 1;
                used = 0.;
            } else if used == 0. {
                lines += 1;
                used = w;
            } else if used + space + w <= cap {
                used += space + w;
            } else {
                lines += 1;
                used = w;
            }
        }
        lines.max(1)
    }
}

/// One word of a summary template.
#[derive(Clone)]
enum Tok {
    /// A word as it stands: kept for the render to clone, measured once and
    /// rounded up as the element it is drawn as.
    Word { text: SharedString, w: f32 },
    /// A word with the count in it: what comes before the digits and after,
    /// each measured once; the digits are priced at the widest digit.
    Count {
        prefix: SharedString,
        suffix: SharedString,
        prefix_w: f32,
        suffix_w: f32,
    },
}

/// Where the plural category `category` is among a plural-aware key's five
/// templates, which [`crate::ui::i18n::plural_templates`] lists in this order.
fn category_slot(category: crate::ui::i18n::PluralCategory) -> usize {
    use crate::ui::i18n::PluralCategory as C;
    match category {
        C::Zero => 0,
        C::One => 1,
        C::Few => 2,
        C::Many => 3,
        C::Other => 4,
    }
}

/// Every template the summary can be made of in the current language, each
/// split into typed words and measured when the fonts, the size, the features
/// or the language change, and priced by arithmetic after that: the summary is
/// the template the existing plural selection picks for the changed files'
/// count and, with untracked files, the one it picks for theirs, so every word
/// of it is one of these or a count between a template's affixes.
#[derive(Clone)]
struct SummaryLexicon {
    /// The changed files' five templates, then the untracked count's five.
    templates: Vec<Vec<Tok>>,
    /// The gap between two words: a space.
    space: f32,
    /// The widest of the ten digits in the summary's font and features, so a
    /// font whose digits are not all alike is never under budget.
    digit: f32,
    /// One line of the summary, rounded up once: the line height every word is
    /// drawn with, the same one the plan multiplies.
    line_h: f32,
    /// The font the words are measured in and drawn in: shared, not rebuilt.
    family: SharedString,
    features: gpui::FontFeatures,
}

impl SummaryLexicon {
    fn measure(
        window: &Window,
        font: &gpui::Font,
        size: f32,
        templates: &[&'static str; 10],
    ) -> Self {
        let text = window.text_system();
        let shaped = |s: &str| crate::ui::tab_strip::measure_text(text, font, size, s);
        let digit = (0..10).map(|d| shaped(&d.to_string())).fold(0., f32::max);
        let split = |template: &'static str| -> Vec<Tok> {
            template
                .split_whitespace()
                .map(|token| match token.find("{count}") {
                    Some(at) => {
                        let (prefix, suffix) = (&token[..at], &token[at + "{count}".len()..]);
                        Tok::Count {
                            prefix: SharedString::from(prefix.to_string()),
                            suffix: SharedString::from(suffix.to_string()),
                            prefix_w: if prefix.is_empty() {
                                0.
                            } else {
                                shaped(prefix)
                            },
                            suffix_w: if suffix.is_empty() {
                                0.
                            } else {
                                shaped(suffix)
                            },
                        }
                    }
                    None => Tok::Word {
                        text: SharedString::from(token.to_string()),
                        w: shaped(token).ceil(),
                    },
                })
                .collect()
        };
        let templates: Vec<Vec<Tok>> = templates.iter().map(|t| split(*t)).collect();
        // The plan holds the words of two templates on the stack: a template
        // with more words than half of it is a translation the header cannot
        // plan, and is refused here, where the language changes, rather than
        // having a word left out of what is drawn.
        for template in &templates {
            assert!(
                template.len() <= SUMMARY_WORDS_MAX / 2,
                "a summary template has more than {} words",
                SUMMARY_WORDS_MAX / 2
            );
        }
        // One line of the summary, as the element draws it: the default line
        // height at the summary's size.
        let mut style = window.text_style();
        style.font_size = px(size).into();
        Self {
            templates,
            space: shaped(" "),
            digit,
            line_h: style
                .line_height_in_pixels(window.rem_size())
                .as_f32()
                .ceil(),
            family: font.family.clone(),
            features: font.features.clone(),
        }
    }

    /// The template slot of a count's plural form: `base` for the changed
    /// files' templates, 5 more for the untracked count's.
    fn slot(base: usize, count: usize) -> usize {
        base + category_slot(crate::ui::i18n::PluralCategory::from_count(count))
    }

    /// The widths of the summary's words for `files` changed files and
    /// `untracked` untracked ones, from the typed words of the templates the
    /// plural selection picks and the digits of each count: arithmetic, no
    /// text.
    fn words(&self, files: usize, untracked: usize) -> SummaryWords {
        let mut words = SummaryWords::default();
        for (base, count, there) in [(0, files, true), (5, untracked, untracked > 0)] {
            if !there {
                continue;
            }
            let digits = crate::ui::tab_strip::count_digits(count) as f32;
            for tok in &self.templates[Self::slot(base, count)] {
                words.w[words.n] = match tok {
                    Tok::Word { w, .. } => *w,
                    Tok::Count {
                        prefix_w, suffix_w, ..
                    } => (prefix_w + digits * self.digit + suffix_w).ceil(),
                };
                words.n += 1;
            }
        }
        words
    }

    /// What the render draws, word by word: the text and the width of its box,
    /// the same width [`Self::words`] planned.
    fn render_words(&self, files: usize, untracked: usize) -> Vec<(SharedString, f32)> {
        let mut out = Vec::new();
        for (base, count, there) in [(0, files, true), (5, untracked, untracked > 0)] {
            if !there {
                continue;
            }
            let digits = crate::ui::tab_strip::count_digits(count) as f32;
            for tok in &self.templates[Self::slot(base, count)] {
                out.push(match tok {
                    Tok::Word { text, w } => (text.clone(), *w),
                    Tok::Count {
                        prefix,
                        suffix,
                        prefix_w,
                        suffix_w,
                    } => (
                        SharedString::from(format!("{prefix}{count}{suffix}")),
                        (prefix_w + digits * self.digit + suffix_w).ceil(),
                    ),
                });
            }
        }
        out
    }
}

/// One tier of what may leave the filled header's first line.
#[derive(Clone, Copy)]
struct Tier {
    /// The diff has it.
    present: bool,
    /// The least it keeps: whole for the chip and the Refreshing word, the
    /// readable minimum for a name and a summary that give way.
    min: f32,
    /// How many children of the row it is on the first line, and on the second.
    kids_first: usize,
    kids_second: usize,
    /// Its width gives way to the room its line leaves.
    flex: bool,
}

/// What one line's parts add up to: the fixed widths with the gaps between
/// the children, and the minima of the parts that give way.
#[derive(Clone, Copy)]
struct LineNeed {
    fixed: f32,
    flex: f32,
}

impl LineNeed {
    fn total(self) -> f32 {
        self.fixed + self.flex
    }
}

/// How many tiers of the filled header's secondary parts are on its second
/// line: none, the chip with the Refreshing word, those and the terminal's
/// name, all three with the summary.
pub(crate) const WRAP_LEVELS: u8 = 3;

/// What the header was last found to be, so each part of it is held the
/// editor's margin past the point it would shrink again: `level` is the
/// ladder's (0 to [`WRAP_LEVELS`]) or, past it, one more than the rows the
/// overflow gives the header; `lines` is the lines of the summary on its own
/// row, 1 outside the overflow. Written by `editor_header_latch_sync` alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WrapState {
    pub(crate) level: u8,
    pub(crate) lines: u8,
}

/// The header's plan in a body: which ladder level, or which rows, and the
/// widths each of them was planned at, so that drawing, the names' caps and
/// the summary's height all use what was planned.
#[derive(Clone, Copy)]
pub(crate) struct RowPlan {
    pub(crate) state: WrapState,
    /// No ladder level fits: the header is in the overflow rows.
    pub(crate) overflow: bool,
    /// The ladder level, when not in the overflow.
    pub(crate) ladder: u8,
    /// The margin before each switch, which yields only to what the last
    /// ladder level lacks.
    pub(crate) lead: f32,
    /// The width the pieces were packed at, and the one the summary was
    /// wrapped at: the body, or the body less the margin while a count is held.
    level_w: f32,
    sum_w: f32,
    /// The row each piece is on, and how many rows hold them.
    row_of: [u8; ITEM_MAX],
    pub(crate) rows: u8,
    /// The width each piece was packed at, which is the width it is drawn at.
    eff: [f32; ITEM_MAX],
    /// The ref's icon is alone on `icon_row`, and the ref is first on the row
    /// after it: the row could not hold the icon, the gap and the ref.
    pub(crate) ref_split: bool,
    icon_row: u8,
    /// How tall the summary's row is.
    pub(crate) summary_h: f32,
}

impl RowPlan {
    /// The width the summary's words are wrapped in, inside its row.
    pub(crate) fn summary_cap(&self) -> f32 {
        (self.sum_w - crate::ui::app::CONTENT_INSET - header_trail()).floor()
    }
}

/// What a count is, at `body` pixels and in `body` less the margin, once held:
/// a count that would fall is kept at no more than it was, and no less than
/// what the narrower width gives, until the margin clears for each step of it;
/// a count that does not fall is the one the body gives. Returns the count and
/// the width it was found at, which is never more than `body`.
fn held(count_at: impl Fn(f32) -> u8, body: f32, was: u8) -> (u8, f32) {
    let now = count_at(body);
    if now >= was {
        return (now, body);
    }
    let margin = body - crate::ui::code_editor::HEADER_HYSTERESIS;
    let kept = count_at(margin);
    if kept <= was {
        return (kept, margin);
    }
    // More than it was at the narrower width and fewer at the body: the
    // narrowest width in between that has no more than it was.
    let (mut lo, mut hi) = (margin, body);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if count_at(mid) <= was {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (count_at(hi), hi)
}

/// What the header's fixed parts add up to, and what each line of it needs at
/// each level. The levels are ordered: level `k` has the first `k` tiers on a
/// second line, a tier the diff does not have is not a level, and the budget
/// of a level is read off what the diff has, never off where its parts are.
/// When no level fits both lines the header has more rows: every piece is
/// packed whole into rows in the order it reads.
#[derive(Clone, Copy)]
pub(crate) struct DiffNeed {
    /// Everything on one line, with the names at their minima.
    pub(crate) one_line: f32,
    lead: f32,
    trail: f32,
    gap: f32,
    /// What never leaves the first line, apart from the terminal button and
    /// the switches' margins.
    core: f32,
    /// The terminal button beside its name, and without it, as the name's gap
    /// goes with the name.
    pill: f32,
    pill_badge: f32,
    fixed_children: usize,
    tiers: [Tier; 3],
    items: [ItemSpec; ITEM_MAX],
    n_items: usize,
    words: SummaryWords,
    space: f32,
    line_h: f32,
    /// The diff has a summary.
    summary: bool,
    /// The ref or branch name at the least it keeps, without its icon: the
    /// half of the ref piece the overflow rows keep whole when the piece cannot
    /// fit one row and the icon goes to a row of its own.
    ref_text: f32,
    /// The way back without its path: the chevron part.
    back_chrome: f32,
}

/// What [`DiffNeed::pack`] decided: the row of each piece, how many rows, the
/// width each piece was packed at and whether the ref was split from its icon.
#[derive(Clone, Copy)]
struct Packed {
    row_of: [u8; ITEM_MAX],
    rows: u8,
    eff: [f32; ITEM_MAX],
    /// The ref's icon is alone on `icon_row` and the ref, whole, is first on
    /// the row after it.
    split: bool,
    icon_row: u8,
}

impl DiffNeed {
    /// The width `item` is packed at, and drawn at, in a row of `cap` pixels:
    /// the least it keeps, except for the three parts that may give up room
    /// where the row has none to give. The Refreshing word takes the row when
    /// it is wider than any row; the way back's path gives up only what the
    /// row lacks, to what the row leaves after its chevron part; the Staged chip
    /// gives up exactly what it lacks of its padding, at most
    /// `2 * (CHIP_PAD_X - CHIP_PAD_X_MIN)`. Wherever the row holds the piece
    /// whole, each is its least, to the pixel. The one function the packing,
    /// the plan and the drawing read.
    fn eff_w(&self, item: Item, cap: f32) -> f32 {
        let w = self.piece_min(item);
        match item {
            Item::Refreshing => w.min(cap.max(0.)),
            Item::Back => {
                self.back_chrome + (w - self.back_chrome).min((cap - self.back_chrome).max(0.))
            }
            Item::Chip => {
                w - 2.
                    * (crate::ui::right_panel::CHIP_PAD_X
                        - crate::ui::right_panel::chip_pad_x(w, cap))
            }
            _ => w,
        }
    }

    /// Whether tier `tier` is on the second line at level `level`.
    fn moved(&self, tier: usize, level: u8) -> bool {
        self.tiers[tier].present && (tier as u8) < level
    }

    /// The first line at `level` with `lead` before each switch: the lead and
    /// trail, what never leaves, the terminal button (without its name once
    /// the name has left), the tiers that have not left, and a gap between
    /// each child.
    fn first_line_with(&self, level: u8, lead: f32) -> LineNeed {
        let mut fixed = self.lead
            + self.trail
            + self.core
            + 2. * lead
            + if self.moved(1, level) {
                self.pill_badge
            } else {
                self.pill
            };
        let mut flex = 0.;
        let mut kids = self.fixed_children;
        for (tier, t) in self.tiers.iter().enumerate() {
            if !t.present || self.moved(tier, level) {
                continue;
            }
            kids += t.kids_first;
            if t.flex {
                flex += t.min;
            } else {
                fixed += t.min;
            }
        }
        LineNeed {
            fixed: fixed + self.gap * (kids as f32 - 1.),
            flex,
        }
    }

    fn first_line(&self, level: u8) -> LineNeed {
        self.first_line_with(level, crate::ui::document_column::SWITCH_LEAD)
    }

    /// The second line at `level`: what is on it, or none when nothing is.
    fn second_line(&self, level: u8) -> Option<LineNeed> {
        let mut fixed = crate::ui::app::CONTENT_INSET + self.trail;
        let mut flex = 0.;
        let mut kids = 0;
        for (tier, t) in self.tiers.iter().enumerate() {
            if !self.moved(tier, level) {
                continue;
            }
            kids += t.kids_second;
            if t.flex {
                flex += t.min;
            } else {
                fixed += t.min;
            }
        }
        (kids > 0).then(|| LineNeed {
            fixed: fixed + self.gap * (kids as f32 - 1.),
            flex,
        })
    }

    /// Whether the header has a second line at `level`.
    pub(crate) fn has_second_line(&self, level: u8) -> bool {
        self.second_line(level).is_some()
    }

    /// Whether the tier that holds the chip and the Refreshing word, the
    /// terminal's name, or the summary (`tier` 0, 1, 2) is on the second line.
    pub(crate) fn tier_moved(&self, tier: usize, level: u8) -> bool {
        self.moved(tier, level)
    }

    /// The last level the ladder has for this diff: every tier it has moved.
    fn last_level(&self) -> u8 {
        (1..=WRAP_LEVELS)
            .rev()
            .find(|level| self.tiers[*level as usize - 1].present)
            .unwrap_or(0)
    }

    /// The ladder's level in `body` pixels: the fewest tiers on the second
    /// line for which both lines fit, a level held the editor's margin past
    /// the point it would fit once the header has been at a level with more
    /// moved, or in the overflow rows (any `was` past the ladder). The last
    /// level fits when the first line does with the switches' margins at
    /// nothing, which is what they yield to. None when no level fits.
    fn ladder_level(&self, body: f32, was: u8) -> Option<u8> {
        let last = self.last_level();
        for level in 0..=WRAP_LEVELS {
            if level > 0 && !self.tiers[level as usize - 1].present {
                continue;
            }
            let lead = if level == last {
                0.
            } else {
                crate::ui::document_column::SWITCH_LEAD
            };
            let need = self
                .second_line(level)
                .map_or(0., LineNeed::total)
                .max(self.first_line_with(level, lead).total());
            if !crate::ui::code_editor::header_inline(body, need, was > level) {
                return Some(level);
            }
        }
        None
    }

    /// The margin before each switch at `level` in `body`: whole, except at
    /// the last level when the first line is short of the body, when each
    /// gives up half of the shortage, never below nothing.
    pub(crate) fn switch_lead(&self, body: f32, level: u8) -> f32 {
        let lead = crate::ui::document_column::SWITCH_LEAD;
        if level != self.last_level() {
            return lead;
        }
        let short = self.first_line_with(level, lead).total() - body;
        if short <= 0. {
            lead
        } else {
            (lead - short / 2.).clamp(0., lead)
        }
    }

    /// How the names share `free` pixels on a line they are both on, or one
    /// takes what there is: `(name, summary)`, whole pixels, floored, never
    /// less than a minimum. After each has its minimum the name takes half of
    /// what is left (up to the button's own cap) and the summary the rest.
    fn share(&self, free: f32, name: bool, summary: bool) -> (f32, f32) {
        let cap = crate::ui::document_column::HEADER_CELL_MAX_W;
        let (name_min, summary_min) = (self.tiers[1].min, self.tiers[2].min);
        match (name, summary) {
            (true, true) => {
                let extra = free - name_min - summary_min;
                let name = (name_min + extra / 2.).min(cap).floor().max(name_min);
                (name, (free - name).floor().max(summary_min))
            }
            (true, false) => (free.min(cap).floor().max(name_min), summary_min),
            (false, true) => (name_min, free.floor().max(summary_min)),
            (false, false) => (name_min, summary_min),
        }
    }

    /// The room the terminal's name and the summary keep at `level`, as
    /// `(name, summary)` caps: each has the room its own line leaves, and
    /// neither is ever less than its minimum.
    pub(crate) fn caps(&self, body: f32, level: u8) -> (f32, f32) {
        let (name_here, summary_here) = (
            self.tiers[1].present && !self.moved(1, level),
            self.tiers[2].present && !self.moved(2, level),
        );
        let first = self.share(
            body - self
                .first_line_with(level, self.switch_lead(body, level))
                .fixed,
            name_here,
            summary_here,
        );
        let second = self.second_line(level).map_or((0., 0.), |line| {
            self.share(
                body - line.fixed,
                self.moved(1, level),
                self.moved(2, level),
            )
        });
        (
            if name_here { first.0 } else { second.0 },
            if summary_here { first.1 } else { second.1 },
        )
    }

    pub(crate) fn summary_min(&self) -> f32 {
        self.tiers[2].min
    }

    /// The pieces packed greedily, in order, into rows of `width` less the
    /// rows' own inset and trail with a gap between each two: the row each is
    /// on, and how many rows there are.
    ///
    /// Each piece is packed at its [`Self::eff_w`] in that row, and the ref
    /// piece, the icon, a gap and the ref itself, which is the one piece the
    /// row may not hold whole, comes apart there: the icon is alone on a row
    /// before (the current one when it is empty, the next otherwise) and the
    /// whole ref is first on the row after it. Wherever the row holds the ref
    /// piece it is packed whole, as it always was.
    fn pack(&self, width: f32) -> Packed {
        let cap = (width - crate::ui::app::CONTENT_INSET - self.trail).floor();
        let mut packed = Packed {
            row_of: [0u8; ITEM_MAX],
            rows: 0,
            eff: [0.; ITEM_MAX],
            split: false,
            icon_row: 0,
        };
        let (mut row, mut used) = (0u8, 0.);
        for (i, spec) in self.items[..self.n_items].iter().enumerate() {
            let mut w = self.eff_w(spec.item, cap);
            if spec.item == Item::Ref && w > cap {
                if used != 0. {
                    row += 1;
                }
                packed.split = true;
                packed.icon_row = row;
                row += 1;
                w = self.ref_text;
                used = w;
            } else if used == 0. {
                used = w;
            } else if used + self.gap + w <= cap {
                used += self.gap + w;
            } else {
                row += 1;
                used = w;
            }
            packed.row_of[i] = row;
            packed.eff[i] = w;
        }
        packed.rows = if self.n_items == 0 { 0 } else { row + 1 };
        packed
    }

    /// The overflow's level in a body of `width`: one more than its rows, the
    /// first row, the packed rows and the summary's own.
    fn overflow_level(&self, width: f32) -> u8 {
        (self.pack(width).rows + u8::from(self.summary) + 2).max(WRAP_LEVELS + 1)
    }

    /// How many lines the summary takes in a body of `width`.
    fn lines_at(&self, width: f32) -> u8 {
        if !self.summary {
            return 1;
        }
        self.words.lines(
            self.space,
            (width - crate::ui::app::CONTENT_INSET - self.trail).floor(),
        )
    }

    /// How tall a row of `lines` lines of the summary is: as tall as the
    /// others when it is one line, and as the lines need, in the same padding
    /// above and below, when it is more.
    fn summary_height(&self, lines: u8) -> f32 {
        let pad = ((SECOND_LINE_H - self.line_h) / 2.).max(0.);
        (f32::from(lines) * self.line_h + 2. * pad).max(SECOND_LINE_H)
    }

    /// The header's plan in `body` pixels, given what it was last found to
    /// be. The ladder first, unchanged; only when no level fits both of its
    /// lines are the pieces packed into rows and the summary given its own,
    /// each count held the editor's margin on its own.
    pub(crate) fn plan(&self, body: f32, was: WrapState) -> RowPlan {
        if let Some(level) = self.ladder_level(body, was.level) {
            return RowPlan {
                state: WrapState { level, lines: 1 },
                overflow: false,
                ladder: level,
                lead: self.switch_lead(body, level),
                level_w: body,
                sum_w: body,
                row_of: [0; ITEM_MAX],
                rows: 0,
                eff: [0.; ITEM_MAX],
                ref_split: false,
                icon_row: 0,
                summary_h: 0.,
            };
        }
        let past = was.level > WRAP_LEVELS;
        let (level, level_w) = held(
            |w| self.overflow_level(w),
            body,
            if past { was.level } else { 0 },
        );
        let (lines, sum_w) = if self.summary {
            held(|w| self.lines_at(w), body, if past { was.lines } else { 0 })
        } else {
            (1, body)
        };
        let packed = self.pack(level_w);
        RowPlan {
            state: WrapState { level, lines },
            overflow: true,
            ladder: self.last_level(),
            lead: crate::ui::document_column::SWITCH_LEAD,
            level_w,
            sum_w,
            row_of: packed.row_of,
            rows: packed.rows,
            eff: packed.eff,
            ref_split: packed.split,
            icon_row: packed.icon_row,
            summary_h: self.summary_height(lines),
        }
    }

    /// The width `item` is drawn at in `plan`: the one it was packed at in the
    /// held row. Outside the overflow rows it is the least the piece keeps.
    pub(crate) fn eff(&self, plan: &RowPlan, item: Item) -> f32 {
        if !plan.overflow {
            return self.piece_min(item);
        }
        self.pieces()
            .position(|piece| piece == item)
            .map_or(0., |at| plan.eff[at])
    }

    /// The whole ref, without its icon, at the least it keeps: what the split
    /// ref is drawn at in its own row, never less.
    pub(crate) fn ref_text_min(&self) -> f32 {
        self.ref_text
    }

    /// The row the split ref's icon is alone on.
    pub(crate) fn icon_row(&self, plan: &RowPlan) -> usize {
        usize::from(plan.icon_row)
    }

    /// The width the Refreshing word is cut to in `plan`, when no row holds it
    /// whole.
    pub(crate) fn refreshing_cut(&self, plan: &RowPlan) -> Option<f32> {
        let whole = self.piece_min(Item::Refreshing);
        let drawn = self.eff(plan, Item::Refreshing);
        (plan.overflow && whole > 0. && drawn < whole).then_some(drawn)
    }

    /// The padding either side of the Staged chip in `plan`: the one the plan
    /// packed it with.
    pub(crate) fn chip_pad(&self, plan: &RowPlan) -> f32 {
        let whole = self.piece_min(Item::Chip);
        crate::ui::right_panel::CHIP_PAD_X - (whole - self.eff(plan, Item::Chip)).max(0.) / 2.
    }

    /// The least `item` is packed at, and drawn at: what its row was planned
    /// with, so that what is drawn is never narrower than what was planned.
    pub(crate) fn piece_min(&self, item: Item) -> f32 {
        self.items[..self.n_items]
            .iter()
            .find(|spec| spec.item == item)
            .map_or(0., |spec| spec.w)
    }

    /// The pieces the overflow rows pack, in the order they read.
    pub(crate) fn pieces(&self) -> impl Iterator<Item = Item> + '_ {
        self.items[..self.n_items].iter().map(|spec| spec.item)
    }

    /// How wide the terminal's name may be in the overflow rows: what its row
    /// leaves beside the other pieces on it (each at its least), up to the
    /// button's cap, never under its minimum.
    pub(crate) fn overflow_name_cap(&self, plan: &RowPlan) -> f32 {
        let cap = (plan.level_w - crate::ui::app::CONTENT_INSET - self.trail).floor();
        let Some(at) = self.pieces().position(|item| item == Item::Name) else {
            return self.tiers[1].min;
        };
        let row = plan.row_of[at];
        let (mut others, mut count) = (0., 0);
        for i in 0..self.n_items {
            if i != at && plan.row_of[i] == row {
                others += plan.eff[i];
                count += 1;
            }
        }
        (cap - others - self.gap * count as f32)
            .min(crate::ui::document_column::HEADER_CELL_MAX_W)
            .floor()
            .max(self.tiers[1].min)
    }

    /// The row index of each packed piece, in the order of [`Self::pieces`].
    pub(crate) fn row_of<'a>(&self, plan: &'a RowPlan) -> &'a [u8] {
        &plan.row_of[..self.n_items]
    }
}

/// A section label's line in the list — the untracked heading — at the
/// Changes tab's group-header height.
const GROUP_HEADER_H: Pixels = px(22.);

/// The width of every rule the diff views draw: the header's bottom edge,
/// the hunk breaks, the split view's centre and the unified gutter.
pub(crate) const HAIRLINE_W: Pixels = px(0.5);

/// The ink of those rules: a hairline at 8% of the foreground, which reads as
/// an edge on the window fill without ever drawing a box. Derived from the
/// foreground rather than `border`, the control-edge role, because a
/// half-pixel of a colour tuned for 1px outlines reads heavier than the rest
/// of the v4 chrome.
pub(crate) fn hairline(cx: &gpui::App) -> Hsla {
    cx.theme().foreground.opacity(0.08)
}

/// Tabular figures for counts set in the interface face, so `+12 −3` and a
/// file count keep their width as they change. Mono text needs none of this.
pub(crate) fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The font features the diff's own text is shaped with, resolved when the
/// setting changes rather than on each row of each frame: a row is drawn every
/// frame, so building the table there would re-allocate it per line of patch.
struct DiffFeatures {
    /// Code, paths and identifiers. The configured list as the user wrote it,
    /// so an arbitrary tag survives; with none configured, the terminal's
    /// grid set — `calt`, `liga` and `clig` all named off, since `calt: 0`
    /// alone leaves the other two at the shaper's default-on — so `=>` and
    /// `->` draw as the two glyphs the file holds.
    code: gpui::FontFeatures,
    /// The line-number gutters: the same list with `tnum` forced on, so the
    /// digit column keeps its grid whichever way ligatures are set.
    gutter: gpui::FontFeatures,
}

impl gpui::Global for DiffFeatures {}

impl DiffFeatures {
    fn resolve(configured: Option<&gpui::FontFeatures>) -> Self {
        let code = match configured.filter(|f| !f.0.is_empty()) {
            Some(configured) => configured.clone(),
            None => gpui::FontFeatures(Arc::new(vec![
                ("calt".to_string(), 0),
                ("liga".to_string(), 0),
                ("clig".to_string(), 0),
            ])),
        };
        let mut gutter: Vec<_> = code
            .0
            .iter()
            .filter(|(tag, _)| tag != "tnum")
            .cloned()
            .collect();
        gutter.push(("tnum".to_string(), 1));
        Self {
            code,
            gutter: gpui::FontFeatures(Arc::new(gutter)),
        }
    }
}

/// Resolves the diff's feature sets from the configured list. Called where
/// the app already assigns its own copy of that list, so the two never
/// disagree and nothing is read from the config while drawing.
pub(crate) fn refresh_diff_features(configured: Option<&gpui::FontFeatures>, cx: &mut gpui::App) {
    cx.set_global(DiffFeatures::resolve(configured));
}

/// Before the app has resolved anything (a bare window in a test) the diff
/// reads as ligatures off.
fn features_of<R>(cx: &gpui::App, pick: impl FnOnce(&DiffFeatures) -> R) -> R {
    match cx.try_global::<DiffFeatures>() {
        Some(resolved) => pick(resolved),
        None => {
            static OFF: LazyLock<DiffFeatures> = LazyLock::new(|| DiffFeatures::resolve(None));
            pick(&*OFF)
        }
    }
}

fn code_features(cx: &gpui::App) -> gpui::FontFeatures {
    features_of(cx, |f| f.code.clone())
}

fn gutter_features(cx: &gpui::App) -> gpui::FontFeatures {
    features_of(cx, |f| f.gutter.clone())
}

/// The height of one line of a patch, in either view.
///
/// Also what the list counts a row it has not laid out yet at. A list knows
/// only the rows it has measured, so without an estimate for the rest a patch
/// of any length reports itself as one screen long — and the scrollbar, which
/// reads that height, drags the reader one screen and stops. The file and hunk
/// rows are a few pixels taller, so the estimate runs a little short until
/// they have been measured; a diff is overwhelmingly its lines.
const DIFF_LINE_H: Pixels = px(19.);

/// The rule between one hunk and the last line of the one before it.
///
/// Barely there on purpose: with the cards gone it is the only line left in
/// the list, and it is separating two parts of one file rather than two
/// files. It is the header's hairline, so the view has one kind of line.
fn hunk_rule(cx: &gpui::App) -> Hsla {
    hairline(cx)
}

/// One row, inset the way every row in the list is.
fn diff_row_element(
    row: &DiffRow,
    ix: usize,
    drag: &Drag,
    font: &SharedString,
    snap: &Arc<DiffSnapshot>,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &mut gpui::App,
) -> AnyElement {
    match row {
        // Stands in for the gap between the groups this list used to be a
        // flex column of.
        DiffRow::Gap => div().w_full().h(gpui::rems(0.75)).into_any_element(),
        DiffRow::Oversized => padded(diff_oversized_notice(snap, cx)),
        DiffRow::FileHeader(head) => padded(diff_file_header(head, font, app, cx)),
        DiffRow::HunkHeader { text, leads } => padded(
            div()
                .w_full()
                .px(ROW_INSET)
                .py_1()
                .when(!leads, |h| {
                    h.mt_1().border_t(HAIRLINE_W).border_color(hunk_rule(cx))
                })
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .font_features(code_features(cx))
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(text.clone())
                .painted("hunk")
                .into_any_element(),
        ),
        // The lines run the full width of the list. A diff is read as a
        // column of code, and code that is inset from both sides reads as a
        // quotation of itself.
        DiffRow::Split { row, at } => copy_menu(
            diff_split_row(row, at, drag, font, app, cx),
            ix,
            at,
            drag,
            app,
        ),
        DiffRow::Unified { row, at } => copy_menu(
            diff_unified_row(row, at, drag, font, app, cx),
            ix,
            at,
            drag,
            app,
        ),
        DiffRow::Truncated(reason) => {
            let note = match reason {
                Truncation::PerFile => t_fmt(
                    L10nKey::DiffTruncatedPerFile,
                    &[("limit", &git_diff::MAX_LINES_PER_FILE.to_string())],
                ),
                Truncation::Budget => t(L10nKey::DiffTruncatedBudget).to_string(),
            };
            padded(note_row(note, cx))
        }
        DiffRow::MoreFiles { rest } => {
            padded(note_row(t_plural(L10nKey::DiffMoreFiles, *rest, &[]), cx))
        }
        // A section label, in the shape the Changes tab gives its group
        // headings: 11.5px, medium, muted, sentence case — small, quiet, and
        // carried by the space around it rather than a bar of its own.
        DiffRow::UntrackedHeader { total } => padded(
            div()
                .w_full()
                .flex()
                .items_center()
                .h(GROUP_HEADER_H)
                .px(ROW_INSET)
                .text_size(rems(HEADING))
                .font_weight(FontWeight::MEDIUM)
                .font_features(tabular())
                .text_color(cx.theme().muted_foreground)
                .child(t_plural(L10nKey::DiffUntrackedHeader, *total, &[]))
                .into_any_element(),
        ),
        DiffRow::Untracked { index, path } => {
            padded(diff_untracked_row(*index, path, font, app, cx))
        }
        DiffRow::MoreUntracked { rest } => padded(note_row(
            t_plural(L10nKey::DiffMoreUntracked, *rest, &[]),
            cx,
        )),
    }
}

/// The one place a copy is offered by name, on the rows that would be copied.
///
/// A drag says what will be copied; the menu says that copying is a thing you
/// can do. It hangs on the selected rows themselves because with the cards
/// gone there is no longer an element that owns a file's lines, and the
/// overlay root already carries the header's own menu — two of them over one
/// right-click would open two popups.
fn copy_menu(
    row: gpui::Div,
    ix: usize,
    at: &RowAt,
    drag: &Drag,
    app: &gpui::WeakEntity<Tty59App>,
) -> AnyElement {
    if !drag.holds(at) {
        return row.into_any_element();
    }
    let app = app.clone();
    row.id(("diff-row-menu", ix))
        .context_menu(move |menu, _window, _cx| {
            menu.item(PopupMenuItem::new(t(L10nKey::DiffCopySelection)).on_click({
                let app = app.clone();
                move |_, _window, cx| {
                    app.update(cx, |this, cx| this.copy_diff_selection(cx)).ok();
                }
            }))
        })
        .into_any_element()
}

/// Wire one drawn row into the drag: a press starts a selection there, and
/// while one is in flight a move across the row extends it.
fn diff_row_drag<E: InteractiveElement + Styled>(
    el: E,
    at: &RowAt,
    side: Option<Side>,
    drag: &Drag,
    app: &gpui::WeakEntity<Tty59App>,
) -> E {
    // From the row, not the overlay's view: a new or deleted file draws
    // unified whatever the view (`diff_list::file_mode`).
    let mode = match side {
        Some(_) => DiffViewMode::Split,
        None => DiffViewMode::Unified,
    };
    // The I-beam is the only standing sign that this text can be taken;
    // nothing else about a row says so until one is dragged.
    let el = el.cursor_text().on_mouse_down(MouseButton::Left, {
        let (app, at) = (app.clone(), at.clone());
        move |_: &MouseDownEvent, window, cx| {
            app.update(cx, |this, cx| {
                this.start_diff_selection(&at, mode, side, window, cx);
            })
            .ok();
        }
    });
    if !drag.selecting {
        return el;
    }
    el.on_mouse_move({
        let (app, at) = (app.clone(), at.clone());
        move |ev: &MouseMoveEvent, _window, cx| {
            let held = ev.pressed_button;
            app.update(cx, |this, cx| {
                this.extend_diff_selection(&at, side, held, cx);
            })
            .ok();
        }
    })
}

/// The margin the file rows keep from the edge of the list.
fn padded(row: AnyElement) -> AnyElement {
    div()
        .w_full()
        .px(px(LIST_INSET))
        .child(row)
        .into_any_element()
}

/// An aside in the list's own voice — a cap that was hit, a tail that was not
/// drawn. Never a row you can act on, so never one that lights up.
fn note_row(text: String, cx: &gpui::App) -> AnyElement {
    div()
        .w_full()
        .px(ROW_INSET)
        .py_1()
        .text_size(rems(META))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn diff_oversized_notice(snap: &DiffSnapshot, cx: &gpui::App) -> AnyElement {
    let stats = snap.stats();
    let text = t_fmt(
        L10nKey::DiffOversizedNotice,
        &[("summary", &oversized_summary(snap, &stats))],
    );
    // The row pill at rest, filled with the hover step: a notice sits in the
    // list's own shape, one quiet rung off the window, rather than in a card.
    let fill = gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().window.hover);
    div()
        .w_full()
        .px(ROW_INSET)
        .py_2()
        .rounded(rounding::ROW_RADIUS)
        .bg(fill)
        .text_size(rems(META))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn diff_file_header(
    head: &FileHead,
    font: &SharedString,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &gpui::App,
) -> AnyElement {
    let sf = cx.global::<crate::ui::presets::Surfaces>().window;
    let hover = gpui::rgb(sf.hover);
    let deco = deco_status(head.status);
    let (glyph, glyph_color) = (status_glyph(deco), status_color(deco, cx));
    let mut header = h_flex()
        .id(("diff-file-header", head.index))
        .w_full()
        .items_center()
        .gap_2()
        .h(FILE_ROW_H)
        .px(ROW_INSET)
        .rounded(ROW_RADIUS)
        .font_features(code_features(cx))
        .when(head.expandable, |h| {
            let path = head.path.clone();
            let want = !head.expanded;
            let app = app.clone();
            h.cursor_pointer()
                .hover(|s| s.bg(hover))
                .on_click(move |_, _window, cx| {
                    let path = path.clone();
                    app.update(cx, |this, cx| {
                        let active = this.active;
                        if let Some(overlay) = this
                            .tabs
                            .get_mut(active)
                            .and_then(|t| t.diff_overlay.as_mut())
                        {
                            overlay.expanded.insert(path, want);
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .child(
                    Icon::new(if head.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(crate::ui::app::TILE_GLYPH_XS))
                    .text_color(cx.theme().muted_foreground),
                )
        })
        // The status letter is the Changes tab's badge — same cell, weight
        // and ink — so M reads amber and A/U green here exactly as it does
        // one click away.
        .child(git_badge(glyph, glyph_color, font))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT_MONO))
                .font_family(font.clone())
                .text_color(gpui::rgb(sf.text_resting))
                .child(head.shown_path.clone())
                .painted("file-path"),
        );
    if head.binary {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META))
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::Binary)),
        );
    }
    if head.added > 0 {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .text_color(cx.theme().success)
                .child(format!("+{}", head.added)),
        );
    }
    if head.removed > 0 {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .text_color(cx.theme().danger)
                .child(format!("−{}", head.removed)),
        );
    }
    header.into_any_element()
}

// An untracked file has no patch in the snapshot, so it cannot be expanded in
// place the way the files above it are — its contents are read one file at a
// time and shown on their own. The row asks for that read, which until now
// only the Source Control panel could: in the overlay these rows were the only
// files in a list of files that did nothing when clicked.
fn diff_untracked_row(
    index: usize,
    path: &str,
    font: &SharedString,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &gpui::App,
) -> AnyElement {
    let sf = cx.global::<crate::ui::presets::Surfaces>().window;
    let hover = gpui::rgb(sf.hover);
    let for_focus = path.to_string();
    let app = app.clone();
    h_flex()
        .id(("diff-untracked", index))
        .w_full()
        .items_center()
        .gap_2()
        .h(FILE_ROW_H)
        .px(ROW_INSET)
        .rounded(ROW_RADIUS)
        .font_features(code_features(cx))
        .cursor_pointer()
        .hover(|s| s.bg(hover))
        .on_click(move |_, window, cx| {
            let for_focus = for_focus.clone();
            app.update(cx, |this, cx| {
                let Some((host, cwd, source)) = this
                    .tabs
                    .get(this.active)
                    .and_then(|t| t.diff_overlay.as_ref())
                    .map(|o| (o.host_id, o.cwd.clone(), o.source.clone()))
                else {
                    return;
                };
                this.open_diff_overlay(host, cwd, source, Some(for_focus), window, cx);
            })
            .ok();
        })
        .child(git_badge(
            status_glyph(DecoStatus::Untracked),
            status_color(DecoStatus::Untracked, cx),
            font,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT_MONO))
                .font_family(font.clone())
                .text_color(gpui::rgb(sf.text_resting))
                .child(path.to_string())
                .painted("untracked-path"),
        )
        .into_any_element()
}

fn diff_split_row(
    row: &SplitRow,
    at: &RowAt,
    drag: &Drag,
    font: &SharedString,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &gpui::App,
) -> gpui::Div {
    h_flex()
        .w_full()
        .h(DIFF_LINE_H)
        .items_stretch()
        .text_xs()
        .font_family(font.clone())
        .font_features(code_features(cx))
        .child(diff_split_cell(
            row.left.as_ref(),
            Side::Old,
            at,
            drag,
            app,
            cx,
        ))
        .child(div().flex_shrink_0().w(HAIRLINE_W).bg(hunk_rule(cx)))
        .child(diff_split_cell(
            row.right.as_ref(),
            Side::New,
            at,
            drag,
            app,
            cx,
        ))
}

fn diff_split_cell(
    cell: Option<&SplitCell>,
    side: Side,
    at: &RowAt,
    drag: &Drag,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &gpui::App,
) -> AnyElement {
    let base = h_flex().flex_1().min_w_0().h_full().items_center();
    let Some(cell) = cell else {
        // Blank, but still this row's half of this column. Left inert it is a
        // dead band under the pointer — no I-beam, a press that starts
        // nothing, and a range that visibly stops at the padding and resumes
        // below it. A one-sided change is the ordinary shape of a diff, so
        // that band runs down most of one column.
        let fill = match drag.covers(at, Some(side)) {
            true => cx.theme().selection,
            false => cx.theme().muted.opacity(0.3),
        };
        return diff_row_drag(base, at, Some(side), drag, app)
            .bg(fill)
            .into_any_element();
    };
    let (marker, tint) = match (cell.changed, side) {
        (true, Side::Old) => ("−", Some(cx.theme().danger.opacity(0.12))),
        (true, Side::New) => ("+", Some(cx.theme().success.opacity(0.12))),
        (false, _) => (" ", None),
    };
    // A selected cell wears the theme's selection colour in place of its own
    // wash, the way selected text does anywhere else. The `+`/`−` in front of
    // the code still says which side of the change it is.
    let fill = match drag.covers(at, Some(side)) {
        true => Some(cx.theme().selection),
        false => tint,
    };
    diff_row_drag(base, at, Some(side), drag, app)
        .when_some(fill, |row, bg| row.bg(bg))
        .child(
            h_flex()
                .flex_shrink_0()
                .w(px(42.))
                .justify_end()
                .pr_1p5()
                .font_features(gutter_features(cx))
                .text_color(cx.theme().muted_foreground)
                .child(cell.no.map(|n| n.to_string()).unwrap_or_default())
                .painted("gutter-split"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(format!("{marker} {}", cell.text))
                .painted("code-split"),
        )
        .into_any_element()
}

/// One line of the unified view.
///
/// Every measurement it shares with [`diff_split_cell`] is shared on purpose —
/// the same 19px row, the same `text_xs` in the same family, and above all the
/// same `0.12` wash behind an addition and a removal. The two views are one
/// diff seen twice; a different green would read as a different thing.
///
/// What differs is forced by the shape. The line numbers get 34px a side
/// rather than 42 (there are two gutters here in front of one column of text,
/// not one in front of each), and the `+`/`−` gets a column of its own rather
/// than riding in the text: with three kinds of line stacked in one column, an
/// inlined marker would leave the context lines' code starting two characters
/// left of everything else.
fn diff_unified_row(
    row: &UnifiedRow,
    at: &RowAt,
    drag: &Drag,
    font: &SharedString,
    app: &gpui::WeakEntity<Tty59App>,
    cx: &gpui::App,
) -> gpui::Div {
    let (marker_color, tint) = match row.kind {
        LineKind::Added => (cx.theme().success, Some(cx.theme().success.opacity(0.12))),
        LineKind::Removed => (cx.theme().danger, Some(cx.theme().danger.opacity(0.12))),
        LineKind::Context => (cx.theme().muted_foreground, None),
    };
    let gutter = |no: Option<u32>| {
        h_flex()
            .flex_shrink_0()
            .w(px(34.))
            .justify_end()
            .pr_1p5()
            .font_features(gutter_features(cx))
            .text_color(cx.theme().muted_foreground)
            .child(no.map(|n| n.to_string()).unwrap_or_default())
            .painted("gutter-unified")
    };
    let fill = match drag.covers(at, None) {
        true => Some(cx.theme().selection),
        false => tint,
    };
    diff_row_drag(h_flex(), at, None, drag, app)
        .w_full()
        .h(DIFF_LINE_H)
        .items_center()
        .text_xs()
        .font_family(font.clone())
        .font_features(code_features(cx))
        .when_some(fill, |line, bg| line.bg(bg))
        .child(gutter(row.old))
        .child(gutter(row.new))
        // The split view's centre rule, in the one place it still means the
        // same thing: everything left of it is a number, everything right of
        // it is the file.
        .child(
            div()
                .flex_shrink_0()
                .w(HAIRLINE_W)
                .h_full()
                .bg(hunk_rule(cx)),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(12.))
                .text_center()
                .text_color(marker_color)
                .child(unified_marker(row.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(row.text.clone())
                .painted("code-unified"),
        )
}

/// Which layout the overlay draws. One setting for the window, not one per
/// overlay: VS Code's `diffEditor.renderSideBySide` is global for the same
/// reason — re-picking on every open is a chore, not a choice.
fn view_mode(cx: &gpui::App) -> DiffViewMode {
    cx.try_global::<Config>()
        .map(|cfg| cfg.diff_view)
        .unwrap_or_default()
}

/// The change column. `−` is U+2212, matching the split view: the ASCII hyphen
/// is narrower than `+` and the two columns would not line up.
fn unified_marker(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Added => "+",
        LineKind::Removed => "−",
        LineKind::Context => "",
    }
}

/// The git status letter and colour every part of the app agrees on.
///
/// `Copied` and `TypeChanged` have no decoration of their own — porcelain v2's
/// index folds them the same way — so they take the nearest one rather than
/// inventing a `C` and a `T` that appear in the overlay and nowhere else.
pub(crate) fn deco_status(status: FileStatus) -> DecoStatus {
    match status {
        FileStatus::Added => DecoStatus::Added,
        FileStatus::Modified => DecoStatus::Modified,
        FileStatus::Deleted => DecoStatus::Deleted,
        FileStatus::Renamed | FileStatus::Copied => DecoStatus::Renamed,
        FileStatus::TypeChanged => DecoStatus::Modified,
        FileStatus::Unmerged => DecoStatus::Conflict,
    }
}

/// The current epoch for a repository, or 0 where nothing has ever bumped one.
/// Zero is the same value a never-touched repository reports, so an overlay
/// that reads it before the global exists simply never looks stale.
fn scm_epoch(cx: &gpui::App, host: crate::ui::host_ops::HostId, root: &Path) -> u64 {
    cx.try_global::<crate::terminal::git_data::ScmData>()
        .map(|data| data.epoch(host, root))
        .unwrap_or(0)
}

/// What the header calls the patch it is showing.
struct SourceSubject {
    icon: &'static str,
    text: String,
    /// The whole ref, for a source that names one: the original revision, both
    /// ends of a range, the patch's full id. `text` is what the header has
    /// room to draw; this is what the tooltip says. Empty for a branch.
    full: String,
    /// Set only where the branch name alone would be ambiguous.
    chip: Option<&'static str>,
    is_rev: bool,
    /// What the commit is *about*, where whoever opened it knew. An object id
    /// is an address, not a name, and a header with nothing but eight hex
    /// digits leaves the reader to remember which commit that was.
    label: Option<CommitLabel>,
}

fn source_subject(source: &DiffSource, branch: String) -> SourceSubject {
    let shape = source_shape(source);
    let branch_of = |chip| SourceSubject {
        icon: "icons/git-branch.svg",
        text: branch.clone(),
        full: String::new(),
        chip,
        is_rev: shape.is_rev,
        label: None,
    };
    // An empty subject is no more use than no label at all, and a
    // `Default::default()` that leaked through would render as one.
    let labelled = |label: &Option<CommitLabel>| label.clone().filter(|_| shape.labelled);
    match source {
        // Worktree and Head are both "the branch, right now"; the header for
        // them is what it has always been.
        DiffSource::Worktree | DiffSource::Head => branch_of(None),
        // Staged is the branch too, but a patch that does not match the files
        // on disk — without the chip it is indistinguishable from the above.
        DiffSource::Staged => branch_of(shape.staged.then(|| t(L10nKey::ScmChipStaged))),
        DiffSource::Commit { rev, label } => SourceSubject {
            icon: "icons/git-commit.svg",
            text: short_rev(rev),
            full: rev.clone(),
            chip: None,
            is_rev: shape.is_rev,
            label: labelled(label),
        },
        DiffSource::Range { base, head } => SourceSubject {
            icon: "icons/git-commit.svg",
            text: format!("{}…{}", short_rev(base), short_rev(head)),
            full: format!("{base}…{head}"),
            chip: None,
            is_rev: shape.is_rev,
            label: None,
        },
        // A pull request: `#12` in mono, its title beside it.
        DiffSource::Patch { id, label } => SourceSubject {
            icon: "icons/github.svg",
            text: id
                .rsplit_once('#')
                .map_or(id.clone(), |(_, n)| format!("#{n}")),
            full: id.clone(),
            chip: None,
            is_rev: shape.is_rev,
            label: labelled(label),
        },
    }
}

/// What a source says about the shape of the header, without building any of
/// its text: whether its subject is a revision, whether it carries the Staged
/// chip, whether a commit label stands beside it. The header is drawn from
/// this and budgeted from it, so the two cannot disagree about what is there.
#[derive(Clone, Copy)]
struct SourceShape {
    is_rev: bool,
    staged: bool,
    labelled: bool,
}

fn source_shape(source: &DiffSource) -> SourceShape {
    let labelled =
        |label: &Option<CommitLabel>| label.as_ref().is_some_and(|l| !l.subject.is_empty());
    match source {
        DiffSource::Worktree | DiffSource::Head => SourceShape {
            is_rev: false,
            staged: false,
            labelled: false,
        },
        DiffSource::Staged => SourceShape {
            is_rev: false,
            staged: true,
            labelled: false,
        },
        DiffSource::Commit { label, .. } | DiffSource::Patch { label, .. } => SourceShape {
            is_rev: true,
            staged: false,
            labelled: labelled(label),
        },
        DiffSource::Range { .. } => SourceShape {
            is_rev: true,
            staged: false,
            labelled: false,
        },
    }
}

/// A tooltip that is the plain text, in the card every tooltip is, with a
/// selector on its content so a test can find where the real tooltip put it.
fn text_tooltip(text: SharedString, window: &mut Window, cx: &mut gpui::App) -> gpui::AnyView {
    // The card is at most the UI's hover maximum, and with its outer margin
    // (`m_3`, 0.75 rem a side) at most the window less a gutter: read when the
    // tooltip is shown, so a resized window is honored. It wraps by words and
    // loses none of them.
    let margin = 0.75 * window.rem_size().as_f32();
    let limit = OVERFLOW_TOOLTIP_MAX_W
        .min(window.viewport_size().width.as_f32() - OVERFLOW_TOOLTIP_GUTTER - 2. * margin)
        .max(0.);
    // Text only wraps at a width it is given, and reports its whole unwrapped
    // width as its smallest, so the card's cap alone leaves the content wider
    // than the card: the content gets the card's inner cap, the card's border
    // box less its padding (`px_2p5`, 0.625 rem a side) and border.
    let content_limit = (limit - 2. * (0.625 * window.rem_size().as_f32() + 1.)).max(0.);
    gpui_component::tooltip::Tooltip::element(move |_, _| {
        div()
            .max_w(px(content_limit))
            .whitespace_normal()
            .when(cfg!(test), |d| {
                d.debug_selector(|| "diff-overlay-tooltip-text".into())
            })
            .child(text.clone())
    })
    .max_w(px(limit))
    .whitespace_normal()
    .build(window, cx)
}

/// The widest an overflow tooltip's visible card is: the hover maximum.
const OVERFLOW_TOOLTIP_MAX_W: f32 = 420.;

/// What an overflow tooltip leaves of the window's width, its outer margins
/// included.
const OVERFLOW_TOOLTIP_GUTTER: f32 = 16.;

/// `Ada · 2h`, the byline under a commit's subject.
///
/// One string rather than two elements: the separator has to disappear along
/// with whichever half is missing, and a `when_some` chain around a middle dot
/// says less than this does.
fn label_byline(label: &CommitLabel, now: i64) -> String {
    let when = (label.at > 0).then(|| relative_time(now, label.at));
    match (label.author.trim(), when) {
        ("", Some(when)) => when,
        (author, Some(when)) => format!("{author} · {when}"),
        (author, None) => author.to_string(),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Object ids get cut to eight characters; anything else is already a name a
/// person chose, and cutting `origin/main` in half would only hide which it is.
fn short_rev(rev: &str) -> String {
    let is_oid = rev.len() >= 40 && rev.chars().all(|c| c.is_ascii_hexdigit());
    match is_oid {
        true => rev[..OID_SHORT].to_string(),
        false => rev.to_string(),
    }
}

/// The characters of an object id the header keeps. The readable floor the
/// header budgets for a ref is one id at this length, so what is drawn for an
/// id and what is reserved for any ref come from the one number.
const OID_SHORT: usize = 8;

fn focused_file(snap: &DiffSnapshot, overlay: &DiffOverlayState) -> Option<usize> {
    let path = overlay.focus.as_deref()?;
    snap.files.iter().position(|f| f.path == path)
}

/// The focused path, when it is an *untracked* file — one the snapshot lists
/// by name but holds no patch for. A path that is both (staged half tracked,
/// say) prefers the real patch.
fn untracked_focus<'a>(snap: &DiffSnapshot, focus: Option<&'a str>) -> Option<&'a str> {
    let path = focus?;
    if snap.files.iter().any(|f| f.path == path) {
        return None;
    }
    snap.untracked.iter().any(|u| u == path).then_some(path)
}

/// The file the overlay is focused on, for the header's way back to the list.
///
/// An untracked file is focused like any other but has no entry in `files` —
/// its card is synthesized from the file's own bytes — so reading only `files`
/// left the one view with no way out of it: the breadcrumb never drew, and the
/// list was reachable again only by closing the overlay and reopening it.
fn focused_name(overlay: &DiffOverlayState) -> Option<String> {
    focused_name_ref(overlay).map(str::to_string)
}

/// [`focused_name`] without the copy, for whoever only needs to know whether
/// there is one.
fn focused_name_ref(overlay: &DiffOverlayState) -> Option<&str> {
    let DiffLoad::Ready(loaded) = &overlay.load else {
        return None;
    };
    // A file with a patch and an untracked one both stand for the focus
    // string itself, so a path the snapshot names is the answer.
    overlay
        .focus
        .as_deref()
        .filter(|path| loaded.has_path(path))
}

fn empty_snapshot(snap: &DiffSnapshot) -> bool {
    snap.files.is_empty() && snap.untracked.is_empty()
}

fn oversized_summary(snap: &DiffSnapshot, stats: &DiffStats) -> String {
    let mut parts = vec![t_plural(L10nKey::DiffChangedFiles, snap.files.len(), &[])];
    let (added, removed) = stats.totals;
    let total_lines = (added + removed) as usize;
    let loaded = stats.retained_lines;
    let budget = stats.budget_exhausted;
    let per_file = stats.per_file_truncated;
    parts.push(match (budget, per_file) {
        (false, false) => t_plural(L10nKey::DiffLines, total_lines, &[]),
        _ => {
            let cap_key = match (budget, per_file) {
                (true, true) => L10nKey::DiffBudgetAndCap,
                (true, false) => L10nKey::DiffBudget,
                _ => L10nKey::DiffPerFileCap,
            };
            t_fmt(
                L10nKey::DiffChangedLines,
                &[
                    ("total", &total_lines.to_string()),
                    ("loaded", &loaded.to_string()),
                    ("cap", t(cap_key)),
                ],
            )
        }
    });
    if stats.untracked_count > 0 {
        parts.push(t_plural(
            L10nKey::DiffUntrackedSummary,
            stats.untracked_count,
            &[],
        ));
    }
    parts.join(", ")
}

/// The de-duplication sets on `Tty59App` are keyed by `(HostId, PathBuf)`, so
/// the source rides along inside the path: two sources over one directory are
/// two independent probes and must not cancel one another.
///
/// `DiffSource::tag` rather than `Debug`, which is what this used to be built
/// from. `Debug` prints a commit's label too, so the same commit opened with a
/// subject in hand and without one would have been two keys and two probes for
/// one patch — the same split `DiffSource`'s own `PartialEq` is written to
/// avoid. The separator is a byte no path contains.
fn probe_key(
    host: crate::ui::host_ops::HostId,
    cwd: &Path,
    source: &DiffSource,
) -> (crate::ui::host_ops::HostId, PathBuf) {
    let mut tagged = std::ffi::OsString::from(format!("{}\u{1}", source.tag()));
    tagged.push(cwd.as_os_str());
    (host, PathBuf::from(tagged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::git_diff::{AUTO_COLLAPSE_LINES, DiffLine, LineKind, MAX_RENDERED_FILES};
    use crate::ui::diff_list::{build_rows, file_expanded};
    use crate::ui::diff_rows::split_hunk;
    use crate::ui::i18n::set_locale;

    #[test]
    fn full_window_diff_background_is_opaque_with_or_without_a_preset() {
        let active = crate::ui::presets::ActiveBackground {
            fill: crate::ui::presets::Fill::Solid(0x12_34_56),
            opacity: Some(0.2),
            image: None,
        };
        let mut fallback: Hsla = gpui::rgb(0x65_43_21).into();
        fallback.a = 0.3;
        let mut opaque_fallback = fallback;
        opaque_fallback.a = 1.0;

        assert_eq!(
            diff_overlay_background(Some(&active), fallback),
            crate::ui::theme::window_background_opaque(&active),
            "the active preset must keep its fill while discarding workspace translucency"
        );
        assert_eq!(
            diff_overlay_background(None, fallback),
            opaque_fallback.into(),
            "the theme fallback must also block the window material"
        );
    }

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
        }
    }

    #[test]
    fn the_probe_key_separates_the_sources_over_one_directory() {
        let host = crate::ui::host_ops::HostId::LOCAL;
        let cwd = Path::new("/repo");
        let worktree = probe_key(host, cwd, &DiffSource::Worktree);
        assert_ne!(worktree, probe_key(host, cwd, &DiffSource::Staged));
        assert_ne!(worktree, probe_key(host, cwd, &DiffSource::Head));
        assert_ne!(
            probe_key(host, cwd, &DiffSource::commit("a")),
            probe_key(host, cwd, &DiffSource::commit("b")),
            "two commits are two probes"
        );
        assert_eq!(worktree, probe_key(host, cwd, &DiffSource::Worktree));
        assert_ne!(
            worktree,
            probe_key(host, Path::new("/other"), &DiffSource::Worktree)
        );
        // …and one commit is one probe however much is known about it. Built
        // from `Debug`, as this key once was, the labelled one would have been
        // a second in-flight probe for a patch already being read.
        assert_eq!(
            probe_key(host, cwd, &DiffSource::commit("a")),
            probe_key(
                host,
                cwd,
                &DiffSource::Commit {
                    rev: "a".into(),
                    label: Some(CommitLabel {
                        subject: "s".into(),
                        author: "Ada".into(),
                        at: 1,
                    }),
                }
            )
        );
    }

    #[test]
    fn every_file_status_lands_on_a_shared_decoration() {
        use DecoStatus as D;
        for (status, want) in [
            (FileStatus::Added, D::Added),
            (FileStatus::Modified, D::Modified),
            (FileStatus::Deleted, D::Deleted),
            (FileStatus::Renamed, D::Renamed),
            // A copy is a rename that left the original behind: same letter.
            (FileStatus::Copied, D::Renamed),
            // A symlink that became a file is a modification, not a category
            // of its own — the overlay is the only place that ever saw a `T`.
            (FileStatus::TypeChanged, D::Modified),
            (FileStatus::Unmerged, D::Conflict),
        ] {
            assert_eq!(deco_status(status), want, "{status:?}");
        }
        assert_eq!(status_glyph(deco_status(FileStatus::Unmerged)), "U");
        assert_eq!(status_glyph(deco_status(FileStatus::Copied)), "R");
    }

    #[test]
    fn the_change_column_uses_the_typographic_minus() {
        assert_eq!(unified_marker(LineKind::Added), "+");
        assert_eq!(unified_marker(LineKind::Removed), "\u{2212}");
        assert_ne!(
            unified_marker(LineKind::Removed),
            "-",
            "the ASCII hyphen is narrower than `+`, and the column would wobble"
        );
        assert_eq!(
            unified_marker(LineKind::Context),
            "",
            "a context line is neither, and a placeholder glyph would be noise"
        );
    }

    #[test]
    fn the_header_shortens_an_object_id_and_nothing_else() {
        let oid = "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a";
        assert_eq!(short_rev(oid), "3f2a1b9c");
        assert_eq!(short_rev("v26.7.5"), "v26.7.5");
        assert_eq!(
            short_rev("origin/main"),
            "origin/main",
            "half a ref name says less than the whole of it"
        );
        assert_eq!(short_rev("3f2a1b9"), "3f2a1b9", "already short");
    }

    #[test]
    fn each_source_names_itself_in_the_header() {
        let branch = || "main".to_string();
        let plain = source_subject(&DiffSource::Worktree, branch());
        assert_eq!((plain.icon, plain.text.as_str()), (BRANCH_ICON, "main"));
        assert_eq!(plain.chip, None);
        assert!(!plain.is_rev);

        assert_eq!(source_subject(&DiffSource::Head, branch()).chip, None);

        let staged = source_subject(&DiffSource::Staged, branch());
        assert_eq!(staged.icon, BRANCH_ICON, "still a branch, still its name");
        assert_eq!(
            staged.chip,
            Some("STAGED"),
            "without it the staged patch is indistinguishable from the unstaged one"
        );

        let commit = source_subject(
            &DiffSource::commit("3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
            branch(),
        );
        assert_eq!(commit.icon, COMMIT_ICON);
        assert_eq!(commit.text, "3f2a1b9c", "the branch is not what is shown");
        assert!(commit.is_rev);

        let range = source_subject(
            &DiffSource::Range {
                base: "main".into(),
                head: "feature".into(),
            },
            branch(),
        );
        assert_eq!(range.icon, COMMIT_ICON);
        assert_eq!(range.text, "main…feature");
    }

    #[test]
    fn a_labelled_commit_says_what_it_was_about() {
        let label = CommitLabel {
            subject: "fix(scm): stop the panel asking twice".into(),
            author: "Ada".into(),
            at: 1_786_255_391,
        };
        let with = source_subject(
            &DiffSource::Commit {
                rev: "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a".into(),
                label: Some(label.clone()),
            },
            "main".to_string(),
        );
        assert_eq!(with.text, "3f2a1b9c", "the sha is still the identifier");
        assert_eq!(
            with.label.as_ref().map(|l| l.subject.as_str()),
            Some(label.subject.as_str())
        );

        // Nothing else grows a subject line, least of all a working-tree
        // patch, whose "subject" would be a branch name repeated.
        assert!(
            source_subject(&DiffSource::Worktree, "main".into())
                .label
                .is_none()
        );
        assert!(
            source_subject(&DiffSource::Head, "main".into())
                .label
                .is_none()
        );
        assert!(
            source_subject(&DiffSource::commit("deadbeef"), "main".into())
                .label
                .is_none(),
            "a commit nobody has read yet has nothing to say"
        );
        // A default-constructed label is indistinguishable from none, and must
        // not paint an empty row where the subject would go.
        let empty = source_subject(
            &DiffSource::Commit {
                rev: "deadbeef".into(),
                label: Some(CommitLabel::default()),
            },
            "main".into(),
        );
        assert!(empty.label.is_none());
    }

    #[test]
    fn the_byline_drops_the_separator_along_with_the_half_it_joined() {
        let now = 1_786_255_391 + 7200;
        let full = CommitLabel {
            subject: "s".into(),
            author: "Ada".into(),
            at: 1_786_255_391,
        };
        assert_eq!(label_byline(&full, now), "Ada · 2h");
        assert_eq!(
            label_byline(
                &CommitLabel {
                    author: String::new(),
                    ..full.clone()
                },
                now
            ),
            "2h",
            "a commit with no author is not `· 2h`"
        );
        assert_eq!(
            label_byline(&CommitLabel { at: 0, ..full }, now),
            "Ada",
            "and a timestamp that would not parse is not `Ada · 56y`"
        );
    }

    const BRANCH_ICON: &str = "icons/git-branch.svg";
    const COMMIT_ICON: &str = "icons/git-commit.svg";

    fn small_file(path: &str, added: u32) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            added,
            removed: 0,
            binary: false,
            truncated: None,
            hunks: vec![git_diff::Hunk {
                header: "@@ -1,1 +1,1 @@".to_string(),
                lines: (0..added)
                    .map(|i| line(LineKind::Added, None, Some(i + 1), "x"))
                    .collect(),
            }],
        }
    }

    fn context_heavy_file(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            added: 1,
            removed: 0,
            binary: false,
            truncated: None,
            hunks: vec![git_diff::Hunk {
                header: "@@ -1,7 +1,7 @@".to_string(),
                lines: (0..6)
                    .map(|i| line(LineKind::Context, Some(i + 1), Some(i + 1), "ctx"))
                    .chain(std::iter::once(line(LineKind::Added, None, Some(7), "x")))
                    .collect(),
            }],
        }
    }

    fn choices<const N: usize>(pairs: [(&str, bool); N]) -> HashMap<String, bool> {
        pairs.into_iter().map(|(p, v)| (p.to_string(), v)).collect()
    }

    fn banner(snap: &DiffSnapshot) -> String {
        set_locale("en");
        oversized_summary(snap, &snap.stats())
    }

    #[test]
    fn per_file_collapse_is_unchanged_below_the_repo_threshold() {
        let small = small_file("small.rs", 10);
        let big = small_file("big.rs", AUTO_COLLAPSE_LINES + 1);
        let none = HashMap::new();
        assert!(file_expanded(&small, &none, false));
        assert!(!file_expanded(&big, &none, false));

        let picked = choices([("small.rs", false), ("big.rs", true)]);
        assert!(!file_expanded(&small, &picked, false));
        assert!(file_expanded(&big, &picked, false));
    }

    #[test]
    fn repo_wide_collapse_overrides_the_per_file_default() {
        let small = small_file("small.rs", 10);
        let none = HashMap::new();
        assert!(file_expanded(&small, &none, false));
        assert!(!file_expanded(&small, &none, true), "collapsed en masse");

        assert!(
            file_expanded(&small, &choices([("small.rs", true)]), true),
            "the user's own click still opens it"
        );
    }

    #[test]
    fn explicit_choices_survive_an_oversized_transition() {
        let opened = small_file("opened.rs", 10);
        let closed = small_file("closed.rs", 10);
        let untouched = small_file("untouched.rs", 10);
        let picked = choices([("opened.rs", true), ("closed.rs", false)]);

        for collapse_all in [true, false] {
            assert!(
                file_expanded(&opened, &picked, collapse_all),
                "an explicitly opened file stays open (collapse_all={collapse_all})"
            );
            assert!(
                !file_expanded(&closed, &picked, collapse_all),
                "an explicitly closed file stays closed (collapse_all={collapse_all})"
            );
        }
        assert!(!file_expanded(&untouched, &picked, true));
        assert!(file_expanded(&untouched, &picked, false));
    }

    #[test]
    fn many_medium_files_are_oversized_and_build_no_rows() {
        let snap = DiffSnapshot {
            files: (0..60)
                .map(|i| small_file(&format!("f{i}.rs"), 150))
                .collect(),
            ..Default::default()
        };
        assert!(
            snap.files.iter().all(|f| f.added <= AUTO_COLLAPSE_LINES),
            "no single file is over the per-file threshold"
        );
        assert!(snap.stats().oversized);

        let none = HashMap::new();
        let rows_expanded: usize = snap
            .files
            .iter()
            .filter(|f| file_expanded(f, &none, false))
            .flat_map(|f| &f.hunks)
            .map(|h| split_hunk(&h.lines).len())
            .sum();
        let rows_collapsed: usize = snap
            .files
            .iter()
            .filter(|f| file_expanded(f, &none, true))
            .flat_map(|f| &f.hunks)
            .map(|h| split_hunk(&h.lines).len())
            .sum();
        assert_eq!(rows_expanded, 9000, "what the old rule would have built");
        assert_eq!(rows_collapsed, 0);
    }

    #[test]
    fn an_ordinary_busy_tree_is_not_oversized() {
        let snap = DiffSnapshot {
            files: (0..40)
                .map(|i| {
                    let mut f = context_heavy_file(&format!("f{i}.rs"));
                    f.hunks = std::iter::repeat_n(f.hunks[0].clone(), 10).collect();
                    f.added = 10;
                    f
                })
                .collect(),
            ..Default::default()
        };
        let (added, removed) = snap.totals();
        assert!(
            snap.stats().retained_lines > (added + removed) as usize * 4,
            "the context lines dominate, as they do in a real diff"
        );
        assert!(
            !snap.stats().oversized,
            "an ordinary afternoon must not read as a tree too large to render \
             ({} retained lines)",
            snap.stats().retained_lines
        );
        let none = HashMap::new();
        assert!(snap.files.iter().all(|f| file_expanded(f, &none, false)));
    }

    #[test]
    fn an_empty_snapshot_reads_as_clean_only_when_the_probe_worked() {
        let clean = DiffSnapshot {
            branch: "main".into(),
            ..Default::default()
        };
        assert!(empty_snapshot(&clean));
        assert!(!clean.read_failed, "nothing went wrong: this tree is clean");

        let broken = DiffSnapshot {
            branch: "main".into(),
            read_failed: true,
            ..Default::default()
        };
        assert!(
            empty_snapshot(&broken),
            "indistinguishable by shape — which is the point"
        );
        assert!(broken.read_failed, "and distinguishable by this");

        let partial = DiffSnapshot {
            files: vec![small_file("one.rs", 3)],
            read_failed: true,
            ..Default::default()
        };
        assert!(!empty_snapshot(&partial));
    }

    #[test]
    fn a_huge_untracked_list_does_not_collapse_the_diff() {
        let snap = DiffSnapshot {
            files: vec![small_file("one.rs", 3)],
            untracked: (0..git_diff::MAX_UNTRACKED)
                .map(|i| format!("node_modules/p{i}/index.js"))
                .collect(),
            untracked_total: 40_000,
            ..Default::default()
        };
        assert!(snap.stats().retained_lines < git_diff::AUTO_COLLAPSE_TOTAL_LINES);
        assert!(snap.files.len() < git_diff::AUTO_COLLAPSE_TOTAL_FILES);
        assert!(
            !snap.stats().oversized,
            "collapsing the diff would not have removed a single untracked row"
        );

        assert_eq!(
            snap.untracked.len(),
            git_diff::MAX_UNTRACKED,
            "retention is capped at the parser"
        );
        assert_eq!(
            snap.untracked.len().min(MAX_RENDERED_FILES),
            MAX_RENDERED_FILES,
            "and rows at the renderer"
        );
        assert_eq!(
            snap.untracked_count(),
            40_000,
            "while the reported count stays the true total"
        );
    }

    /// `ListState::reset` would drop the scroll position, so opening or
    /// closing one file in a long tree would throw the reader back to the top.
    /// Only the rows that actually changed are spliced.
    #[test]
    fn only_the_rows_that_changed_are_spliced() {
        let snap = DiffSnapshot {
            files: vec![
                small_file("a.rs", 2),
                small_file("b.rs", 2),
                small_file("c.rs", 2),
            ],
            ..Default::default()
        };
        let shut = |path: &str| -> HashMap<String, bool> {
            [(path.to_string(), false)].into_iter().collect()
        };
        let open = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false);
        let middle_shut = build_rows(&snap, &shut("b.rs"), None, DiffViewMode::Unified, false);

        let (replaced, with) = spliced_range(&open, &middle_shut);
        assert_eq!(
            (replaced.start, with),
            (5, 1),
            "the first card and the gap after it are untouched"
        );
        assert_eq!(
            open.len() - replaced.end,
            middle_shut.len() - (replaced.start + with),
            "and so is everything below the file that closed"
        );

        let (replaced, with) = spliced_range(&open, &open);
        assert_eq!(
            (replaced.start, replaced.end, with),
            (open.len(), open.len(), 0)
        );
    }

    #[test]
    fn a_list_that_was_empty_or_becomes_empty_splices_in_one_piece() {
        let snap = DiffSnapshot {
            files: vec![small_file("a.rs", 2)],
            ..Default::default()
        };
        let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false);

        let (replaced, with) = spliced_range(&[], &rows);
        assert_eq!((replaced.start, replaced.end, with), (0, 0, rows.len()));

        let (replaced, with) = spliced_range(&rows, &[]);
        assert_eq!((replaced.start, replaced.end, with), (0, rows.len(), 0));
    }

    /// A probe that found nothing new still lands a fresh `Arc` over an equal
    /// snapshot. The rows are rightly kept — and the key has to come away
    /// holding the `Arc` that landed, or every frame from then on proves the
    /// two equal the long way: a walk of every line of the patch, per wheel
    /// event.
    #[test]
    fn an_equal_snapshot_leaves_the_key_pointing_at_the_one_that_landed() {
        let held = Arc::new(DiffSnapshot {
            files: vec![small_file("a.rs", 2)],
            ..Default::default()
        });
        let landed = Arc::new(DiffSnapshot {
            files: vec![small_file("a.rs", 2)],
            ..Default::default()
        });
        assert!(
            !Arc::ptr_eq(&held, &landed),
            "two separate Arcs over equal contents"
        );

        let expanded = HashMap::new();
        let mut key = RowsFrom {
            snap: &held,
            preview: None,
            mode: DiffViewMode::Unified,
            focused: None,
            oversized: false,
            expanded: &expanded,
        }
        .to_key();
        let landed_from = RowsFrom {
            snap: &landed,
            preview: None,
            mode: DiffViewMode::Unified,
            focused: None,
            oversized: false,
            expanded: &expanded,
        };

        assert!(
            key.describes(&landed_from),
            "nothing about the rows changed"
        );
        key.retarget(&landed_from);
        assert!(
            Arc::ptr_eq(&key.snap, &landed),
            "so the next frame settles it by pointer rather than by contents"
        );
    }

    #[test]
    fn a_focused_untracked_file_asks_for_a_preview_not_the_list() {
        let snap = DiffSnapshot {
            files: vec![small_file("tracked.rs", 3)],
            untracked: vec!["new.md".to_string()],
            ..Default::default()
        };
        assert_eq!(untracked_focus(&snap, Some("new.md")), Some("new.md"));
        assert_eq!(
            untracked_focus(&snap, Some("tracked.rs")),
            None,
            "a real patch wins over the name list"
        );
        assert_eq!(untracked_focus(&snap, Some("absent.rs")), None);
        assert_eq!(untracked_focus(&snap, None), None);
    }

    #[test]
    fn untracked_rows_are_capped_but_the_count_stays_true() {
        let snap = DiffSnapshot {
            untracked: (0..git_diff::MAX_UNTRACKED)
                .map(|i| format!("p{i}"))
                .collect(),
            untracked_total: 12_345,
            ..Default::default()
        };
        let rendered = snap.untracked.len().min(MAX_RENDERED_FILES);
        assert_eq!(rendered, MAX_RENDERED_FILES);
        assert_eq!(snap.untracked_count(), 12_345, "header count is honest");
        assert_eq!(snap.untracked_count() - rendered, 12_045);
    }

    #[test]
    fn untracked_count_falls_back_to_the_retained_length() {
        let snap = DiffSnapshot {
            untracked: vec!["a".into(), "b".into(), "c".into()],
            ..Default::default()
        };
        assert_eq!(snap.untracked_total, 0);
        assert_eq!(snap.untracked_count(), 3);
    }

    #[test]
    fn file_cards_are_capped() {
        let snap = DiffSnapshot {
            files: (0..MAX_RENDERED_FILES + 25)
                .map(|i| small_file(&format!("f{i}.rs"), 1))
                .collect(),
            ..Default::default()
        };
        let shown = snap.files.len().min(MAX_RENDERED_FILES);
        assert_eq!(shown, MAX_RENDERED_FILES);
        assert_eq!(
            snap.files.len() - shown,
            25,
            "the tail gets one summary line"
        );
    }

    #[test]
    fn the_banner_names_the_budget_when_context_outweighs_the_changes() {
        let files: Vec<FileDiff> = (0..200)
            .map(|i| context_heavy_file(&format!("f{i}.rs")))
            .collect();

        let mut truncated = files.clone();
        truncated.push(FileDiff {
            hunks: vec![],
            truncated: Some(Truncation::Budget),
            ..context_heavy_file("dropped.rs")
        });
        let snap = DiffSnapshot {
            files: truncated,
            ..Default::default()
        };
        let (added, removed) = snap.totals();
        let total = (added + removed) as usize;
        assert!(
            snap.stats().retained_lines > total,
            "the context lines outweigh the changed ones — the shape that slipped through"
        );
        assert!(snap.stats().budget_exhausted);

        let summary = banner(&snap);
        assert!(summary.contains("budget"), "{summary}");
        assert!(
            summary.contains(&format!("{total} changed lines")),
            "the exact `+N −N` from the header, not the retained count: {summary}"
        );

        let whole = DiffSnapshot {
            files,
            ..Default::default()
        };
        assert!(!whole.stats().budget_exhausted);
        assert!(!banner(&whole).contains("budget"));
    }

    #[test]
    fn the_banner_names_the_per_file_cap_when_context_outweighs_the_changes() {
        let files: Vec<FileDiff> = (0..200)
            .map(|i| context_heavy_file(&format!("f{i}.rs")))
            .collect();

        let mut cut = files.clone();
        cut.push(FileDiff {
            truncated: Some(Truncation::PerFile),
            ..context_heavy_file("huge.rs")
        });
        let snap = DiffSnapshot {
            files: cut,
            ..Default::default()
        };
        let (added, removed) = snap.totals();
        let total = (added + removed) as usize;
        assert!(
            snap.stats().retained_lines > total,
            "the shape the comparison reads backwards"
        );
        assert!(
            !snap.stats().budget_exhausted,
            "the budget axis is not what fired"
        );

        let summary = banner(&snap);
        assert!(summary.contains("per-file cap"), "{summary}");
        assert!(
            summary.contains(&format!("{total} changed lines")),
            "the exact `+N −N` from the header, not the retained count: {summary}"
        );

        let whole = DiffSnapshot {
            files: files.clone(),
            ..Default::default()
        };
        assert!(!banner(&whole).contains("per-file"));

        let mut both = files;
        both.push(FileDiff {
            truncated: Some(Truncation::PerFile),
            ..context_heavy_file("huge.rs")
        });
        both.push(FileDiff {
            hunks: vec![],
            truncated: Some(Truncation::Budget),
            ..context_heavy_file("dropped.rs")
        });
        let summary = banner(&DiffSnapshot {
            files: both,
            ..Default::default()
        });
        assert!(summary.contains("budget"), "{summary}");
        assert!(summary.contains("per-file cap"), "{summary}");
    }

    #[test]
    #[ignore = "measurement, not an assertion"]
    fn bench_snapshot_share() {
        use std::time::Instant;

        for (label, files, per_file) in [
            ("unbudgeted (v26.7.5 shape)", 300, 300),
            ("budgeted (this build retains)", 300, 67),
        ] {
            let snap = Arc::new(DiffSnapshot {
                files: (0..files)
                    .map(|i| small_file(&format!("f{i}.rs"), per_file))
                    .collect(),
                ..Default::default()
            });
            let lines: usize = snap.stats().retained_lines;

            let t = Instant::now();
            for _ in 0..10 {
                let _deep = (*snap).clone();
            }
            let deep = t.elapsed() / 10;

            let t = Instant::now();
            for _ in 0..100 {
                let _shared = Arc::clone(&snap);
            }
            let shared = t.elapsed() / 100;
            println!(
                "{label}: {files} files / {lines} lines — deep clone {deep:?} vs \
                 Arc::clone {shared:?}, per holder on the UI thread"
            );
        }
    }

    #[test]
    fn a_supplied_patch_is_named_by_its_number() {
        let source = DiffSource::Patch {
            id: "nathabonfim59/tty59#974".into(),
            label: Some(CommitLabel {
                subject: "feat(tabs): reorder".into(),
                author: "migege".into(),
                at: 0,
            }),
        };
        let subject = source_subject(&source, "move-tab-keybind".into());
        assert_eq!(subject.text, "#974");
        assert!(subject.is_rev);
        assert_eq!(subject.icon, "icons/github.svg");
        assert_eq!(subject.label.unwrap().subject, "feat(tabs): reorder");
        assert!(source.is_supplied());
        assert!(!DiffSource::Head.is_supplied());
        // One pull request is one overlay, however it is labelled.
        assert_eq!(
            source,
            DiffSource::Patch {
                id: "nathabonfim59/tty59#974".into(),
                label: None
            }
        );
    }
}

#[cfg(test)]
mod overlay_gpui_tests {
    use super::*;
    use crate::ui::app::test_window;
    use crate::ui::host_ops::HostId;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn overlay_source_and_load(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
    ) -> (DiffSource, bool) {
        app.update_in(vcx, |app, _, _| {
            let overlay = app.tabs[app.active]
                .diff_overlay
                .as_ref()
                .expect("an overlay is open");
            (
                overlay.source.clone(),
                matches!(overlay.load, DiffLoad::Loading),
            )
        })
    }

    /// Opening a staged file while a worktree overlay is up must re-probe. The
    /// filter used to match on `(cwd, host)` alone, so it took the "just move
    /// the focus" branch and left the unstaged patch on screen under the
    /// staged file's name.
    #[gpui::test]
    fn a_second_source_over_one_directory_is_a_second_overlay(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let cwd = std::path::PathBuf::from("/no/such/tty59/repo");

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(
                HostId::LOCAL,
                cwd.clone(),
                DiffSource::Worktree,
                Some("a.rs".to_string()),
                window,
                cx,
            );
        });
        assert_eq!(
            overlay_source_and_load(&app, &mut vcx),
            (DiffSource::Worktree, true)
        );

        // Pretend the worktree probe landed, so a reused overlay would show it.
        app.update_in(&mut vcx, |app, _, _| {
            let active = app.active;
            let overlay = app.tabs[active].diff_overlay.as_mut().unwrap();
            overlay.loading = false;
            overlay.load = DiffLoad::Ready(
                Arc::new(DiffSnapshot {
                    source: DiffSource::Worktree,
                    branch: "main".into(),
                    ..Default::default()
                })
                .into(),
            );
        });

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(
                HostId::LOCAL,
                cwd.clone(),
                DiffSource::Staged,
                Some("a.rs".to_string()),
                window,
                cx,
            );
        });
        assert_eq!(
            overlay_source_and_load(&app, &mut vcx),
            (DiffSource::Staged, true),
            "the same file from a different source is a different question"
        );
    }

    /// What the header and its budget read off a loaded snapshot — its totals,
    /// its untracked count and whether a focus names a path in it — is what a
    /// fresh walk of the same snapshot finds, for an empty one, a single file,
    /// many hunks, a name that is both a patch and untracked, and untracked
    /// names alone. Asked only through the snapshot's own methods and the
    /// header's `focused_name`, so it holds however they are answered.
    #[gpui::test]
    fn a_loaded_snapshot_answers_what_a_fresh_traversal_finds(cx: &mut TestAppContext) {
        use crate::terminal::git_diff::{
            AUTO_COLLAPSE_TOTAL_FILES, AUTO_COLLAPSE_TOTAL_LINES, DiffLine, Hunk,
        };
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let file =
            |path: &str, added: u32, removed: u32, hunks: usize, truncated: Option<Truncation>| {
                FileDiff {
                    path: path.into(),
                    old_path: None,
                    status: FileStatus::Modified,
                    added,
                    removed,
                    binary: false,
                    truncated,
                    hunks: (0..hunks)
                        .map(|i| Hunk {
                            header: format!("@@ -{i} +{i} @@"),
                            lines: vec![DiffLine {
                                kind: LineKind::Added,
                                old_no: None,
                                new_no: Some(1),
                                text: "x".into(),
                            }],
                        })
                        .collect(),
                }
            };
        let snapshots = [
            DiffSnapshot::default(),
            DiffSnapshot {
                files: vec![file("a.rs", 3, 1, 1, None)],
                ..Default::default()
            },
            // Many hunks, both truncations, and a name that is also untracked.
            DiffSnapshot {
                files: vec![
                    file("a.rs", 10, 0, 40, None),
                    file("b/c.rs", 0, 7, 25, Some(Truncation::PerFile)),
                    file("dup.rs", 1, 1, 1, Some(Truncation::Budget)),
                ],
                untracked: vec!["new.txt".into(), "dup.rs".into()],
                untracked_total: 9,
                ..Default::default()
            },
            DiffSnapshot {
                untracked: vec!["only.txt".into()],
                ..Default::default()
            },
            // Oversized by files, and by lines.
            DiffSnapshot {
                files: (0..=AUTO_COLLAPSE_TOTAL_FILES)
                    .map(|i| file(&format!("f{i}.rs"), 1, 0, 0, None))
                    .collect(),
                ..Default::default()
            },
            DiffSnapshot {
                files: vec![file(
                    "long.rs",
                    AUTO_COLLAPSE_TOTAL_LINES as u32 + 1,
                    0,
                    AUTO_COLLAPSE_TOTAL_LINES + 1,
                    None,
                )],
                ..Default::default()
            },
        ];
        let (mut budget, mut per_file, mut oversized) = (false, false, false);
        for snapshot in snapshots {
            let snapshot = Arc::new(snapshot);
            // A fresh walk of the snapshot, written out here.
            let mut expected = DiffStats::default();
            for f in &snapshot.files {
                expected.totals.0 += f.added;
                expected.totals.1 += f.removed;
                expected.retained_lines += f.hunks.iter().map(|h| h.lines.len()).sum::<usize>();
                match f.truncated {
                    Some(Truncation::Budget) => expected.budget_exhausted = true,
                    Some(Truncation::PerFile) => expected.per_file_truncated = true,
                    None => {}
                }
            }
            expected.untracked_count = snapshot.untracked_total.max(snapshot.untracked.len());
            expected.oversized = snapshot.files.len() > AUTO_COLLAPSE_TOTAL_FILES
                || expected.retained_lines > AUTO_COLLAPSE_TOTAL_LINES;
            budget |= expected.budget_exhausted;
            per_file |= expected.per_file_truncated;
            oversized |= expected.oversized;
            let named: Vec<String> = snapshot
                .files
                .iter()
                .map(|f| f.path.clone())
                .chain(snapshot.untracked.iter().cloned())
                .chain(["missing.rs".to_string(), String::new()])
                .collect();
            let held = Arc::clone(&snapshot);
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_diff_overlay(
                    HostId::LOCAL,
                    std::path::PathBuf::from("/no/such/tty59/repo"),
                    DiffSource::Worktree,
                    None,
                    window,
                    cx,
                );
                let active = app.active;
                let overlay = app.tabs[active].diff_overlay.as_mut().unwrap();
                overlay.loading = false;
                overlay.load = DiffLoad::Ready(held.into());
                {
                    let DiffLoad::Ready(loaded) = &overlay.load else {
                        panic!("the fixture is ready");
                    };
                    assert_eq!(loaded.stats(), expected);
                    assert_eq!(loaded.totals(), expected.totals);
                    assert_eq!(loaded.files.len(), snapshot.files.len());
                }
                for path in &named {
                    overlay.focus = Some(path.clone());
                    let swept = snapshot.files.iter().any(|f| &f.path == path)
                        || snapshot.untracked.iter().any(|u| u == path);
                    assert_eq!(
                        focused_name(overlay),
                        swept.then(|| path.clone()),
                        "focus on {path:?}"
                    );
                }
            });
        }
        assert!(
            budget && per_file && oversized,
            "the fixtures must include both truncations ({budget}, {per_file}) and an oversized \
             patch ({oversized})"
        );
    }

    fn one_file_snapshot(source: DiffSource) -> DiffSnapshot {
        use crate::terminal::git_diff::{DiffLine, Hunk, LineKind};
        DiffSnapshot {
            root: std::path::PathBuf::from("/no/such/tty59/repo"),
            source,
            branch: "main".into(),
            files: vec![FileDiff {
                path: "a.rs".into(),
                old_path: None,
                status: FileStatus::Modified,
                added: 1,
                removed: 1,
                binary: false,
                truncated: None,
                hunks: vec![Hunk {
                    header: "@@ -1,2 +1,2 @@".into(),
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Context,
                            old_no: Some(1),
                            new_no: Some(1),
                            text: "keep".into(),
                        },
                        DiffLine {
                            kind: LineKind::Removed,
                            old_no: Some(2),
                            new_no: None,
                            text: "old".into(),
                        },
                        DiffLine {
                            kind: LineKind::Added,
                            old_no: None,
                            new_no: Some(2),
                            text: "new".into(),
                        },
                    ],
                }],
            }],
            untracked: vec!["scratch.txt".into()],
            untracked_total: 1,
            read_failed: false,
        }
    }

    fn show(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, source: DiffSource) {
        let cwd = std::path::PathBuf::from("/no/such/tty59/repo");
        app.update_in(vcx, |app, window, cx| {
            app.open_diff_overlay(HostId::LOCAL, cwd.clone(), source.clone(), None, window, cx);
            let active = app.active;
            let overlay = app.tabs[active].diff_overlay.as_mut().unwrap();
            overlay.loading = false;
            overlay.load = DiffLoad::Ready(Arc::new(one_file_snapshot(source)).into());
            // The card is what carries the rows, so open it.
            overlay.expanded.insert("a.rs".to_string(), true);
        });
    }

    /// By default (no features configured) the diff draws the file's own
    /// characters: its code carries the terminal grid's set, naming all three
    /// ligature features off because `calt: 0` alone leaves `liga`/`clig` at
    /// the shaper's default (on). With Font ligatures on, the configured
    /// features apply instead. The line-number gutters are set apart from the
    /// code and keep `tnum` either way;
    /// `diff_text_follows_the_ligature_setting_in_both_views` reads them
    /// where they paint.
    #[gpui::test]
    fn diff_rows_name_every_ligature_feature_off(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        app.update_in(&mut vcx, |_app, _window, cx| {
            let font = SharedString::from("Test Mono");
            let drag = Drag {
                sel: None,
                selecting: false,
            };
            let weak = cx.entity().downgrade();
            let at = RowAt {
                path: SharedString::from("a.rs"),
                id: crate::ui::diff_rows::RowId { hunk: 0, row: 0 },
            };
            let want = Some(gpui::FontFeatures(Arc::new(vec![
                ("calt".to_string(), 0),
                ("liga".to_string(), 0),
                ("clig".to_string(), 0),
            ])));

            let unified = UnifiedRow {
                old: Some(1),
                new: None,
                kind: LineKind::Removed,
                text: "let f = () => browser -> api;".to_string(),
                line: 0,
            };
            let mut row = diff_unified_row(&unified, &at, &drag, &font, &weak, cx);
            assert_eq!(
                row.style().text.font_features,
                want,
                "the unified row carries the literal set for its code"
            );

            let split = SplitRow {
                left: Some(SplitCell {
                    no: Some(1),
                    text: "let f = () => browser -> api;".to_string(),
                    changed: true,
                    line: 0,
                }),
                right: None,
            };
            let mut row = diff_split_row(&split, &at, &drag, &font, &weak, cx);
            assert_eq!(
                row.style().text.font_features,
                want,
                "the split row carries it for both cells' code"
            );
        });
    }

    type Seen = (&'static str, gpui::FontFeatures);

    fn features(pairs: &[(&str, u32)]) -> gpui::FontFeatures {
        gpui::FontFeatures(Arc::new(
            pairs.iter().map(|(t, v)| (t.to_string(), *v)).collect(),
        ))
    }

    /// A patch with a `#`-numbered subject, so the header carries an
    /// identifier, and no background probe that could replace the fixture.
    fn arrows_source() -> DiffSource {
        DiffSource::Patch {
            id: "ligatures#7".into(),
            label: None,
        }
    }

    /// Opens `arrows_source` on a one-file patch whose only change is
    /// `=>` becoming `->`, in a window big enough to lay the list out.
    fn show_arrows(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        use crate::terminal::git_diff::DiffLine;

        vcx.simulate_resize(gpui::size(px(1200.), px(800.)));
        show(app, vcx, arrows_source());
        app.update_in(vcx, |app, _, cx| {
            let overlay = app.tabs[app.active].diff_overlay.as_mut().unwrap();
            let mut snap = one_file_snapshot(arrows_source());
            snap.files[0].hunks[0].lines = vec![
                DiffLine {
                    kind: LineKind::Context,
                    old_no: Some(1),
                    new_no: Some(1),
                    text: "fn keep() {}".into(),
                },
                DiffLine {
                    kind: LineKind::Removed,
                    old_no: Some(2),
                    new_no: None,
                    text: "let f = |a| a => b;".into(),
                },
                DiffLine {
                    kind: LineKind::Added,
                    old_no: None,
                    new_no: Some(2),
                    text: "let f = |a| a -> b;".into(),
                },
            ];
            overlay.load = DiffLoad::Ready(Arc::new(snap).into());
            cx.notify();
        });
    }

    fn set_view(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, mode: DiffViewMode) {
        app.update_in(vcx, |app, _, cx| {
            app.update_config(cx, |cfg| cfg.diff_view = mode)
        });
    }

    /// What every probe saw in each of two real frames, with the overlay
    /// focused on `focus` (or on the file list). Each frame is forced dirty
    /// first, so the second is a genuine repaint of the settled list rather
    /// than the first one counted twice.
    fn paint(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        focus: Option<&str>,
    ) -> [Vec<Seen>; 2] {
        vcx.background_executor.run_until_parked();
        app.update_in(vcx, |app, _, cx| {
            let overlay = app.tabs[app.active].diff_overlay.as_mut().unwrap();
            overlay.focus = focus.map(str::to_string);
            cx.notify();
        });
        [(), ()].map(|()| {
            row_probe::take_painted();
            vcx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
            row_probe::take_painted()
        })
    }

    /// Every sighting of `label`, in every frame, must carry `want`, and each
    /// frame must have at least one: a probe that never painted proves
    /// nothing, and one frame's sighting must not stand in for the other's.
    fn assert_painted(
        frames: &[Vec<Seen>; 2],
        label: &str,
        want: &gpui::FontFeatures,
        state: &str,
    ) {
        for (n, seen) in frames.iter().enumerate() {
            let mine: Vec<_> = seen.iter().filter(|(l, _)| *l == label).collect();
            assert!(
                !mine.is_empty(),
                "{label} never painted in frame {n} ({state})"
            );
            for (_, got) in mine {
                assert_eq!(got, want, "{label} in frame {n} ({state})");
            }
        }
    }

    /// Settings → Font ligatures decides how the diff's own text is drawn,
    /// code rows and the header's identifiers alike; the line-number gutters
    /// keep tabular figures whichever way it is set. Observed where the text
    /// paints, after the setting changes under a live overlay, both
    /// directions and both views.
    ///
    /// Off must read exactly as the terminal's grid does (`calt`, `liga` and
    /// `clig` named off, `tnum` not part of it): the gutter alone adds `tnum`.
    #[gpui::test]
    fn diff_text_follows_the_ligature_setting_in_both_views(cx: &mut TestAppContext) {
        let off = features(&[("calt", 0), ("liga", 0), ("clig", 0)]);
        let off_gutter = features(&[("calt", 0), ("liga", 0), ("clig", 0), ("tnum", 1)]);
        let on = features(&[("calt", 1), ("liga", 1)]);
        let on_gutter = features(&[("calt", 1), ("liga", 1), ("tnum", 1)]);
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        show_arrows(&app, &mut vcx);

        for (mode, view) in [
            (DiffViewMode::Split, "split"),
            (DiffViewMode::Unified, "unified"),
        ] {
            set_view(&app, &mut vcx, mode);
            let code = format!("code-{view}");
            let gutter = format!("gutter-{view}");
            // Starts on, so the first assertion is the user's change (the
            // face's ligatures reaching the diff). The `false` step is the
            // off-set control and the last `true` proves it comes back.
            for (ligatures, want, want_gutter) in [
                (true, &on, &on_gutter),
                (false, &off, &off_gutter),
                (true, &on, &on_gutter),
            ] {
                app.update_in(&mut vcx, |app, _, cx| app.set_font_ligatures(ligatures, cx));
                let state = format!("{view}, ligatures {ligatures}");
                let list = paint(&app, &mut vcx, None);
                assert_painted(&list, &code, want, &state);
                assert_painted(&list, &gutter, want_gutter, &state);
                for label in ["header-rev", "hunk", "file-path", "untracked-path"] {
                    assert_painted(&list, label, want, &state);
                }
                let focused = paint(&app, &mut vcx, Some("a.rs"));
                assert_painted(&focused, "header-path", want, &state);
                assert_painted(&focused, &code, want, &state);
            }
        }
    }

    /// A configured list is the user's, tag for tag: the diff does not read
    /// it as a yes/no. Only the gutter touches it, and only to keep `tnum` on.
    /// Clearing the list afterwards returns to the off set.
    #[gpui::test]
    fn a_configured_feature_list_reaches_the_diff_tag_for_tag(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        show_arrows(&app, &mut vcx);
        set_view(&app, &mut vcx, DiffViewMode::Unified);

        let custom = crate::core::config::FontFeatures(Arc::new(vec![
            ("ss01".to_string(), 1),
            ("tnum".to_string(), 0),
        ]));
        app.update_in(&mut vcx, |app, _, cx| {
            app.update_config(cx, |cfg| cfg.font_features = Some(custom))
        });
        let seen = paint(&app, &mut vcx, None);
        assert_painted(
            &seen,
            "code-unified",
            &features(&[("ss01", 1), ("tnum", 0)]),
            "custom list",
        );
        assert_painted(
            &seen,
            "hunk",
            &features(&[("ss01", 1), ("tnum", 0)]),
            "custom list",
        );
        assert_painted(
            &seen,
            "gutter-unified",
            &features(&[("ss01", 1), ("tnum", 1)]),
            "custom list",
        );

        app.update_in(&mut vcx, |app, _, cx| {
            app.update_config(cx, |cfg| cfg.font_features = None)
        });
        let seen = paint(&app, &mut vcx, None);
        assert_painted(
            &seen,
            "code-unified",
            &features(&[("calt", 0), ("liga", 0), ("clig", 0)]),
            "list cleared",
        );
    }

    /// What the window is built with counts too: a list already in the config
    /// when the app starts reaches the diff the same way one edited later does.
    #[gpui::test]
    fn a_feature_list_present_at_startup_reaches_the_diff(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs_configured(cx, 1, |cfg| {
            cfg.font_features = Some(crate::core::config::FontFeatures(Arc::new(vec![(
                "ss01".to_string(),
                1,
            )])));
        });
        show_arrows(&app, &mut vcx);
        set_view(&app, &mut vcx, DiffViewMode::Unified);

        let seen = paint(&app, &mut vcx, None);
        assert_painted(
            &seen,
            "code-unified",
            &features(&[("ss01", 1)]),
            "at startup",
        );
        assert_painted(
            &seen,
            "gutter-unified",
            &features(&[("ss01", 1), ("tnum", 1)]),
            "at startup",
        );
    }

    /// The set a row is drawn with is resolved when the setting changes, not
    /// rebuilt for each row or each frame: every row in a frame, and the
    /// next frame's rows, hold the one shared table.
    #[gpui::test]
    fn the_resolved_feature_set_is_shared_across_rows_and_frames(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        show_arrows(&app, &mut vcx);
        set_view(&app, &mut vcx, DiffViewMode::Split);
        app.update_in(&mut vcx, |app, _, cx| app.set_font_ligatures(true, cx));

        let frames = paint(&app, &mut vcx, None);
        assert_painted(
            &frames,
            "code-split",
            &features(&[("calt", 1), ("liga", 1)]),
            "ligatures on",
        );
        let later = paint(&app, &mut vcx, None);

        for group in ["code-split", "gutter-split"] {
            let tables: Vec<_> = frames
                .iter()
                .chain(&later)
                .flatten()
                .filter(|(l, _)| *l == group)
                .map(|(_, f)| f.0.clone())
                .collect();
            assert!(
                tables.len() >= 8,
                "{group}: two cells a row over four frames, got {}",
                tables.len()
            );
            assert!(
                tables.iter().all(|t| Arc::ptr_eq(t, &tables[0])),
                "{group} was rebuilt rather than cloned from the cached set"
            );
        }
    }

    /// The comparable facts of every row the overlay holds: which line, which
    /// side, its numbers and text, and whether it is a change.
    fn row_facts(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Vec<String> {
        app.update_in(vcx, |app, _, _| {
            let overlay = app.tabs[app.active].diff_overlay.as_ref().unwrap();
            let mut facts = Vec::new();
            for row in overlay.rows.iter() {
                match row {
                    DiffRow::Split { row, at } => {
                        for (side, cell) in [("old", &row.left), ("new", &row.right)] {
                            if let Some(c) = cell {
                                facts.push(format!(
                                    "{} {side} {:?} {:?} changed={} {:?}",
                                    at.path, at.id.hunk, c.no, c.changed, c.text
                                ));
                            }
                        }
                    }
                    DiffRow::Unified { row, at } => facts.push(format!(
                        "{} {:?} {:?}/{:?} {:?}",
                        at.path, row.kind as u8, row.old, row.new, row.text
                    )),
                    _ => {}
                }
            }
            facts
        })
    }

    /// CONTROL (passes on the unchanged code): the patch is diffed and its
    /// changes classified by characters, so the rows — which line, which
    /// side, which text, which are changes — are identical whatever the
    /// ligature setting. `=>` against `->` is one removed and one added line
    /// holding exactly the characters the file has.
    #[gpui::test]
    fn control_rows_and_changes_ignore_the_ligature_setting(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        show_arrows(&app, &mut vcx);
        for (mode, view) in [
            (DiffViewMode::Split, "split"),
            (DiffViewMode::Unified, "unified"),
        ] {
            set_view(&app, &mut vcx, mode);
            paint(&app, &mut vcx, None);
            let before = row_facts(&app, &mut vcx);
            for on in [true, false] {
                app.update_in(&mut vcx, |app, _, cx| app.set_font_ligatures(on, cx));
                paint(&app, &mut vcx, None);
                assert_eq!(
                    row_facts(&app, &mut vcx),
                    before,
                    "{view} rows moved with ligatures {on}"
                );
            }
            let joined = before.join("\n");
            assert!(joined.contains("let f = |a| a => b;"), "{view}: {joined}");
            assert!(joined.contains("let f = |a| a -> b;"), "{view}: {joined}");
            assert!(
                before.len() >= 3,
                "{view}: the control saw no rows to compare: {joined}"
            );
        }
    }

    /// Returning to the terminal docks the current diff instead of closing or
    /// reloading it. The same button the editor uses must preserve a focused
    /// file and the view mode, and change only this tab's document layout.
    #[gpui::test]
    fn the_filled_diffs_terminal_button_docks_without_losing_the_patch(cx: &mut TestAppContext) {
        use crate::core::config::DocumentLayout;

        let (app, mut vcx, _panes) = test_window::harness_with_tabs(cx, 2);
        vcx.simulate_resize(gpui::size(px(1200.), px(800.)));
        // A supplied patch has no background git probe that can replace the
        // fixture while the simulated click pumps the executor.
        let source = DiffSource::Patch {
            id: "terminal-return-test".into(),
            label: None,
        };
        show(&app, &mut vcx, source.clone());
        let snapshot = app.update_in(&mut vcx, |app, _, cx| {
            app.set_document_layout(DocumentLayout::Fill, cx);
            app.update_config(cx, |cfg| cfg.diff_view = DiffViewMode::Unified);
            let overlay = app.tabs[app.active].diff_overlay.as_mut().unwrap();
            overlay.focus = Some("a.rs".into());
            let DiffLoad::Ready(snapshot) = &overlay.load else {
                panic!("the fixture has a ready patch");
            };
            snapshot.snapshot().clone()
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let button = vcx
            .debug_bounds("document-back-to-terminal")
            .expect("a filled diff exposes the terminal button");
        vcx.simulate_click(button.center(), gpui::Modifiers::none());

        app.update_in(&mut vcx, |app, _, cx| {
            assert_eq!(app.document_layout(cx), DocumentLayout::Dock);
            assert_eq!(
                app.tabs[1].document_layout, None,
                "other tabs keep their layout"
            );
            assert_eq!(cx.global::<Config>().diff_view, DiffViewMode::Unified);
            let overlay = app.tabs[app.active].diff_overlay.as_ref().unwrap();
            assert_eq!(overlay.source, source);
            assert_eq!(overlay.focus.as_deref(), Some("a.rs"));
            assert_eq!(overlay.expanded.get("a.rs"), Some(&true));
            let DiffLoad::Ready(current) = &overlay.load else {
                panic!("docking keeps the ready patch");
            };
            assert!(
                Arc::ptr_eq(current.snapshot(), &snapshot),
                "docking does not reload the patch"
            );
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            vcx.debug_bounds("document-back-to-terminal").is_none(),
            "a docked diff already has the terminal beside it"
        );
    }

    /// Every header branch, every row renderer, once each. A missing icon, an
    /// unset global or a panicking helper shows up here rather than the first
    /// time somebody opens a commit.
    #[gpui::test]
    fn every_source_renders_in_both_views(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);

        for mode in [DiffViewMode::Split, DiffViewMode::Unified] {
            app.update_in(&mut vcx, |app, _, cx| {
                app.update_config(cx, |cfg| cfg.diff_view = mode);
            });
            for source in [
                DiffSource::Worktree,
                DiffSource::Staged,
                DiffSource::Head,
                DiffSource::commit("3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
                DiffSource::Commit {
                    rev: "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a".into(),
                    label: Some(CommitLabel {
                        subject: "fix(scm): read a commit's own header".into(),
                        author: "Ada".into(),
                        at: 1_786_255_391,
                    }),
                },
                DiffSource::Range {
                    base: "main".into(),
                    head: "feature".into(),
                },
            ] {
                show(&app, &mut vcx, source.clone());
                // A real frame, so layout and paint run too: `title_bar_drag`
                // and the segmented track both want a window that is drawing.
                crate::ui::app::render_probe::arm(10_000);
                app.update_in(&mut vcx, |_, _, cx| cx.notify());
                vcx.background_executor.run_until_parked();
                assert!(
                    crate::ui::app::render_probe::draws() > 0,
                    "nothing was drawn, so nothing was proved: {source:?} in {mode:?}"
                );
                app.update_in(&mut vcx, |app, window, cx| {
                    app.close_diff_overlay(window, cx)
                });
            }
        }
    }

    #[gpui::test]
    fn toggling_the_diff_view_mode_writes_config(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let mode =
            |vcx: &mut VisualTestContext| vcx.update(|_, cx| cx.global::<Config>().diff_view);

        assert_eq!(
            mode(&mut vcx),
            DiffViewMode::Split,
            "side by side is what everyone already sees"
        );

        app.update_in(&mut vcx, |app, _, cx| app.toggle_diff_view_mode(cx));
        assert_eq!(mode(&mut vcx), DiffViewMode::Unified);

        app.update_in(&mut vcx, |app, _, cx| app.toggle_diff_view_mode(cx));
        assert_eq!(mode(&mut vcx), DiffViewMode::Split, "and back again");
    }

    /// Focusing an untracked file has to leave a way back to the list.
    ///
    /// Its card is synthesized from the file's own bytes, so it has no entry in
    /// `files` — and the header's breadcrumb read only `files`. The one view
    /// that could be entered was the one view that could not be left except by
    /// closing the overlay and opening it again.
    #[gpui::test]
    fn a_focused_untracked_file_still_has_a_way_back(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let cwd = std::path::PathBuf::from("/no/such/tty59/repo");

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(
                HostId::LOCAL,
                cwd.clone(),
                DiffSource::Worktree,
                Some("scratch.txt".to_string()),
                window,
                cx,
            );
            let active = app.active;
            let overlay = app.tabs[active].diff_overlay.as_mut().unwrap();
            overlay.loading = false;
            overlay.load = DiffLoad::Ready(
                Arc::new(DiffSnapshot {
                    source: DiffSource::Worktree,
                    branch: "main".into(),
                    untracked: vec!["scratch.txt".to_string()],
                    untracked_total: 1,
                    ..Default::default()
                })
                .into(),
            );
        });

        app.update_in(&mut vcx, |app, _, _| {
            let overlay = app.tabs[app.active].diff_overlay.as_ref().unwrap();
            assert_eq!(
                focused_name(overlay).as_deref(),
                Some("scratch.txt"),
                "the breadcrumb is the way back"
            );
        });
    }

    /// The same source and the same focus still toggles the overlay shut.
    #[gpui::test]
    fn the_same_source_twice_still_closes(cx: &mut TestAppContext) {
        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let cwd = std::path::PathBuf::from("/no/such/tty59/repo");

        for _ in 0..2 {
            app.update_in(&mut vcx, |app, window, cx| {
                app.open_diff_overlay(
                    HostId::LOCAL,
                    cwd.clone(),
                    DiffSource::Worktree,
                    None,
                    window,
                    cx,
                );
            });
        }
        app.update_in(&mut vcx, |app, _, _| {
            assert!(app.tabs[app.active].diff_overlay.is_none());
        });
    }
}

/// An overlay that has read its patch has to stop reading it.
///
/// `maybe_refresh_diff_overlay` calls a `Head` overlay stale when the cached
/// git status disagrees with the snapshot on screen, and every landed probe
/// wakes that check by touching the status cache. So a disagreement the probe
/// cannot settle is not a stale badge — it is a loop: read the diff, publish
/// it, wake the watchers, find the same disagreement, read it again. Two `git`
/// processes a lap, `refreshing…` pinned to the header, and a window that
/// costs 7% of a core sitting still.
///
/// The way in is ordinary: switch branches anywhere outside tty59 — another
/// terminal, an editor, a worktree command — and the cached branch is a branch
/// the repository has left. That is what the stale entry below stands for.
///
/// Unix-only, and not for the harness: on Windows the root this test seeds
/// the cache with is not the root the probe lands with, so `scm_epoch` never
/// agrees with the landing snapshot and the overlay re-probes on every frame
/// — `load` reaches `Ready` and `loading` goes straight back to `true`, which
/// is the exact spin this test exists to catch.
///
/// Not the slash direction — `Path` compares by component, so `C:/x` and
/// `C:\x` are already equal. It is the prefix, and since #796 it is this
/// test's own: the product keys a repository by one spelling now
/// (`Host::canonicalize` drops the `\\?\` extended-length prefix and
/// `core::git::git_path` re-spells what git prints), while the seed below
/// still comes straight from `std::fs::canonicalize` and so carries
/// `\\?\C:\Users\—` — a `VerbatimDisk` prefix where everything it is
/// compared against is now `Disk`. Seeding through
/// `tty59_core::core::path_spelling` should lift this, as a change that can
/// show it green rather than a drive-by.
#[cfg(all(test, unix))]
mod render_idle_gpui_tests {
    use super::*;
    use crate::terminal::git_status::{GitStatusCache, RepoSnapshot};
    use crate::ui::app::{render_probe, test_window};
    use crate::ui::host_ops::HostId;
    use gpui::TestAppContext;

    const BUDGET: u64 = 200;

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
    }

    /// The whole point of the flattened row list: a patch of any size costs
    /// the rows on screen, not the rows in the patch.
    ///
    /// Before this, the overlay built a card per file and an element per line
    /// on every frame, and gpui notifies the view on every scroll wheel event
    /// — so a few hundred lines of diff rebuilt tens of thousands of elements
    /// tens of times a second, and the window visibly stalled.
    #[gpui::test]
    fn a_long_patch_builds_only_the_rows_on_screen(cx: &mut TestAppContext) {
        const LINES: usize = 800;

        let root = std::env::temp_dir().join(format!("tty59-diff-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        git(&root, &["init", "--quiet"]);
        let before: String = (0..LINES).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("long.txt"), &before).unwrap();
        git(&root, &["add", "long.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.email=t@x",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "one",
            ],
        );
        // Every line rewritten, so the patch is a removal and an addition per
        // line rather than a handful of hunks in a sea of context.
        let after: String = (0..LINES).map(|i| format!("edited {i}\n")).collect();
        std::fs::write(root.join("long.txt"), &after).unwrap();

        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);
        let open = root.clone();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(HostId::LOCAL, open, DiffSource::Head, None, window, cx);
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            vcx.background_executor.run_until_parked();
            let ready = app.update_in(&mut vcx, |app, _, _| {
                app.tabs[app.active]
                    .diff_overlay
                    .as_ref()
                    .is_some_and(|o| matches!(o.load, DiffLoad::Ready(_)) && !o.loading)
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the overlay never landed a snapshot"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 1600 changed lines is well past the auto-collapse threshold, so the
        // card starts shut. Open it: a reader opening a large file is exactly
        // the frame this is about.
        app.update_in(&mut vcx, |app, _, cx| {
            let active = app.active;
            app.tabs[active]
                .diff_overlay
                .as_mut()
                .expect("the overlay is open")
                .expanded
                .insert("long.txt".to_string(), true);
            cx.notify();
        });
        vcx.background_executor.run_until_parked();

        let rows = app.update_in(&mut vcx, |app, _, _| {
            app.tabs[app.active]
                .diff_overlay
                .as_ref()
                .map(|o| o.rows.len())
                .unwrap_or(0)
        });
        assert!(
            rows > LINES,
            "the patch flattens to a row per changed line ({rows} rows)"
        );

        row_probe::take();
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        vcx.background_executor.run_until_parked();
        let built = row_probe::take();
        assert!(
            built > 0,
            "the list drew something — a probe that counts nothing proves nothing"
        );
        assert!(
            built < rows as u64 / 4,
            "a frame built {built} of {rows} rows: the list is not virtualised"
        );

        // The other half of virtualising: a list counts the rows it has not
        // laid out at zero unless it is given an estimate, and the scrollbar
        // reads that count as the length of the document. Without the size
        // hint the bar reaches 248px into this patch — its thumb fills the
        // track, and dragging it to the bottom lands a screen down.
        let reach = app.update_in(&mut vcx, |app, _, _| {
            app.tabs[app.active]
                .diff_overlay
                .as_ref()
                .expect("the overlay is open")
                .list
                .max_offset_for_scrollbar()
                .y
        });
        assert!(
            reach > DIFF_LINE_H * (rows as f32 * 0.75),
            "the scrollbar reaches {reach} into a patch of {rows} rows"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn an_overlay_over_a_stale_branch_reaches_render_idle(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!("tty59-diff-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        git(&root, &["init", "--quiet"]);
        std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
        git(&root, &["add", "a.rs"]);
        git(
            &root,
            &[
                "-c",
                "user.email=t@x",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "one",
            ],
        );
        // Something for the overlay to actually show, so it settles on a
        // snapshot rather than on the empty state.
        std::fs::write(root.join("a.rs"), "fn main() { /* edited */ }\n").unwrap();

        let (app, mut vcx, _pane) = test_window::harness_with_tabs(cx, 1);

        // The cache as a branch switch outside tty59 leaves it: a branch this
        // repository is no longer on, and counts from before the switch.
        let stale = root.clone();
        app.update_in(&mut vcx, |_, _, cx| {
            cx.default_global::<GitStatusCache>();
            cx.update_global::<GitStatusCache, _>(|cache, _| {
                cache.finish_probe(
                    HostId::LOCAL,
                    &stale,
                    Some(RepoSnapshot {
                        root: stale.clone(),
                        home: stale.clone(),
                        branch: "a-branch-this-repo-has-left".into(),
                        counts: Some((99, 99)),
                    }),
                );
            });
        });

        let open = root.clone();
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_diff_overlay(HostId::LOCAL, open, DiffSource::Head, None, window, cx);
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            vcx.background_executor.run_until_parked();
            let ready = app.update_in(&mut vcx, |app, _, _| {
                app.tabs[app.active]
                    .diff_overlay
                    .as_ref()
                    .is_some_and(|o| matches!(o.load, DiffLoad::Ready(_)) && !o.loading)
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the overlay never landed a snapshot"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        test_window::quiesce(&mut vcx, Some(&root));
        render_probe::arm(BUDGET);
        vcx.background_executor.run_until_parked();
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(3));
        vcx.background_executor.run_until_parked();
        render_probe::arm(BUDGET);
        vcx.executor()
            .advance_clock(std::time::Duration::from_secs(9));
        vcx.background_executor.run_until_parked();

        assert_eq!(
            render_probe::draws(),
            0,
            "a settled overlay must stop re-reading its own diff"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The drag itself, in a window: a press, a move, and what lands on the
/// clipboard. The row geometry is `diff_rows`' business and tested there —
/// what these check is the wiring the list hangs on it.
#[cfg(test)]
mod selection_gpui_tests {
    use super::*;
    use crate::terminal::git_diff::{DiffLine, LineKind};
    use crate::ui::app::test_window;
    use crate::ui::diff_rows::RowId;
    use crate::ui::pane::{Pane, PaneSlot};
    use crate::ui::pending_pane::{PendingPane, PendingSpawn};
    use gpui::{Entity, MouseButton, TestAppContext, VisualTestContext};

    const PATH: &str = "src/a.rs";

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
        }
    }

    /// `a` kept, `b`/`c` replaced by `B`, `d` kept — four split rows, five
    /// unified ones.
    fn patched_file() -> FileDiff {
        FileDiff {
            path: PATH.to_string(),
            old_path: None,
            status: FileStatus::Modified,
            added: 1,
            removed: 2,
            binary: false,
            truncated: None,
            hunks: vec![git_diff::Hunk {
                header: "@@ -1,4 +1,3 @@".to_string(),
                lines: vec![
                    line(LineKind::Context, Some(1), Some(1), "a"),
                    line(LineKind::Removed, Some(2), None, "b"),
                    line(LineKind::Removed, Some(3), None, "c"),
                    line(LineKind::Added, None, Some(2), "B"),
                    line(LineKind::Context, Some(4), Some(3), "d"),
                ],
            }],
        }
    }

    /// The coordinate the list carries on the row `row` of the only hunk.
    fn at(row: usize) -> RowAt {
        RowAt {
            path: PATH.into(),
            id: RowId { hunk: 0, row },
        }
    }

    /// A window with one tab, showing that patch. Built by hand rather than
    /// through `open_diff_overlay`: that dispatches a probe, and a probe
    /// landing mid-test would drop the selection under it.
    fn window(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
        let (app, mut vcx) = test_window::harness(cx);
        app.update_in(&mut vcx, |app, _, cx| {
            let pending = cx.new(|cx| {
                PendingPane::new(
                    "test-box",
                    PendingSpawn {
                        workspace: None,
                        working_directory: None,
                        restore_pane: None,
                        shell: None,
                        agent: None,
                        agent_session_id: None,
                        agent_launch_argv: None,
                        run_on_land: None,
                        owner: None,
                        font_size: 14.0,
                    },
                    cx,
                )
            });
            app.tabs
                .push(crate::ui::app::Tab::new(Pane::leaf(PaneSlot::Connecting(
                    pending,
                ))));
            app.active = 0;
            app.tabs[0].diff_overlay = Some(DiffOverlayState {
                host_id: crate::ui::host_ops::HostId::LOCAL,
                cwd: PathBuf::from("/repo"),
                source: DiffSource::Head,
                focus_handle: cx.focus_handle(),
                load: DiffLoad::Ready(
                    Arc::new(DiffSnapshot {
                        files: vec![patched_file()],
                        ..Default::default()
                    })
                    .into(),
                ),
                loading: false,
                expanded: HashMap::new(),
                focus: None,
                preview: None,
                preview_loading: None,
                selection: None,
                selecting: false,
                list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(256.))
                    .with_size_hint(DIFF_LINE_H),
                rows: Rc::new(Vec::new()),
                rows_key: None,
                epoch: None,
            });
        });
        (app, vcx)
    }

    fn drag(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        mode: DiffViewMode,
        side: Option<Side>,
        from: usize,
        to: usize,
    ) {
        // The view mode is a window-wide setting, and the overlay drops a
        // selection whose rows the current view never drew — so a drag in the
        // unified view has to happen with the unified view on.
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.diff_view = mode;
            cx.set_global(cfg);
        });
        app.update_in(vcx, |this, window, cx| {
            this.start_diff_selection(&at(from), mode, side, window, cx);
            this.extend_diff_selection(&at(to), side, Some(MouseButton::Left), cx);
        });
    }

    fn copied(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<String> {
        app.update_in(vcx, |this, _, cx| this.copy_diff_selection(cx));
        vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
    }

    fn selection(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Option<DiffSelection> {
        app.update_in(vcx, |this, _, _| {
            this.tabs[0]
                .diff_overlay
                .as_ref()
                .and_then(|o| o.selection.clone())
        })
    }

    #[gpui::test]
    fn a_drag_down_a_column_copies_that_column(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);

        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 0, 3);
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("a\nB\nd"));

        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::Old), 0, 3);
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("a\nb\nc\nd"));

        drag(&app, &mut vcx, DiffViewMode::Unified, None, 1, 3);
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("b\nc\nB"));
    }

    /// A drag that runs up the list leaves the head above the anchor. The
    /// range is read in drawn order either way, so it copies what the same two
    /// rows copy dragged the other way round.
    #[gpui::test]
    fn a_drag_up_a_column_copies_the_same_rows(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);

        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 3, 0);
        let sel = selection(&app, &mut vcx).expect("the drag this test just made");
        assert!(
            sel.head < sel.anchor,
            "the drag ended above where it started"
        );
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("a\nB\nd"));

        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::Old), 3, 0);
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("a\nb\nc\nd"));

        drag(&app, &mut vcx, DiffViewMode::Unified, None, 3, 1);
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("b\nc\nB"));
    }

    /// The overlay is often drawn beside a live shell that holds the keyboard.
    /// Ctrl+C is the copy key only once the overlay has taken focus — until
    /// then the same keystroke would reach the pane and interrupt whatever is
    /// running in it.
    #[gpui::test]
    fn starting_a_drag_takes_the_keyboard(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 0, 1);
        let focused = app.update_in(&mut vcx, |this, window, _| {
            this.tabs[0]
                .diff_overlay
                .as_ref()
                .expect("the overlay this window was built with")
                .focus_handle
                .is_focused(window)
        });
        assert!(focused);
    }

    /// A move with no button held is a release the overlay never saw — over a
    /// pane, or outside the window. The drag ends there rather than resuming
    /// the next time the pointer crosses a row.
    #[gpui::test]
    fn a_release_the_overlay_missed_ends_the_drag(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 0, 1);

        app.update_in(&mut vcx, |this, _, cx| {
            this.extend_diff_selection(&at(3), Some(Side::New), None, cx);
        });
        assert_eq!(
            copied(&app, &mut vcx).as_deref(),
            Some("a\nB"),
            "the range stops where the pointer was last seen holding the button"
        );
    }

    /// The two views pair the same lines into different rows, so a range drawn
    /// in one of them points at other code in the other. `sync_diff_rows` is
    /// where the rows for a frame are settled, so it is where the range that
    /// no longer names any of them is dropped.
    #[gpui::test]
    fn switching_the_view_drops_the_selection(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 0, 3);

        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.diff_view = DiffViewMode::Unified;
            cx.set_global(cfg);
        });
        app.update_in(&mut vcx, |this, _, cx| {
            let _ = this.sync_diff_rows(cx);
        });
        assert!(selection(&app, &mut vcx).is_none());
    }

    /// A fresh read re-cuts the hunks. A range that survived one would keep
    /// its coordinates and quietly cover other code.
    #[gpui::test]
    fn a_fresh_snapshot_drops_the_selection(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        drag(&app, &mut vcx, DiffViewMode::Split, Some(Side::New), 0, 3);

        app.update_in(&mut vcx, |this, _, cx| {
            this.install_diff_snapshot(
                crate::ui::host_ops::HostId::LOCAL,
                &PathBuf::from("/repo"),
                &DiffSource::Head,
                Some(Arc::new(DiffSnapshot {
                    files: vec![patched_file()],
                    ..Default::default()
                })),
                cx,
            );
            let overlay = this.tabs[0].diff_overlay.as_ref().unwrap();
            assert!(overlay.selection.is_none());
            assert!(!overlay.selecting);
        });
    }

    #[gpui::test]
    fn a_copy_with_nothing_selected_leaves_the_clipboard_alone(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("untouched".into()))
        });
        assert_eq!(copied(&app, &mut vcx).as_deref(), Some("untouched"));
    }

    /// Collapsing a file above the selection re-cuts the list, but not the
    /// rows the range names. A selection keyed on list positions would come
    /// away pointing at whatever slid into those slots.
    #[gpui::test]
    fn collapsing_a_file_above_the_range_leaves_it_on_the_same_lines(cx: &mut TestAppContext) {
        let (app, mut vcx) = window(cx);
        app.update_in(&mut vcx, |this, _, _| {
            let overlay = this.tabs[0].diff_overlay.as_mut().unwrap();
            overlay.load = DiffLoad::Ready(
                Arc::new(DiffSnapshot {
                    files: vec![
                        FileDiff {
                            path: "src/above.rs".to_string(),
                            ..patched_file()
                        },
                        patched_file(),
                    ],
                    ..Default::default()
                })
                .into(),
            );
        });
        drag(&app, &mut vcx, DiffViewMode::Unified, None, 1, 3);

        app.update_in(&mut vcx, |this, _, cx| {
            let active = this.active;
            {
                let overlay = this.tabs[active].diff_overlay.as_mut().unwrap();
                overlay.expanded.insert("src/above.rs".to_string(), false);
            }
            let _ = this.sync_diff_rows(cx);
        });
        assert_eq!(
            copied(&app, &mut vcx).as_deref(),
            Some("b\nc\nB"),
            "the range still names the same three lines of the same file"
        );
    }
}
