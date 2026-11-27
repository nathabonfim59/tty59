//! Getting around the editor: back and forward through where the caret has
//! been, Go to Symbol, and the breadcrumbs over the text.
//!
//! **History.** Every jump — Go to File, Go to Line, Go to Symbol, a file link
//! clicked in the grid, a definition a language server found, a click far down
//! the file — is noticed the same way: [`Tty59App::editor_nav_tick`] samples the
//! caret each time the editor draws, and a caret that changed file, or moved
//! [`JUMP_LINES`] or more without the text changing under it, has jumped. The
//! place it left goes on the back stack. Nothing has to remember to report a
//! jump, so a new way of jumping cannot forget to.
//!
//! One history per tab, like a tab's strip of files: each tab is its own
//! editor group. A place in a file that has since been closed opens the file
//! again; one in an untitled buffer that is gone is skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{AnyElement, App, Context, Window, div, px, relative};
use gpui_component::input::{LineEdit, Position};
use gpui_component::{ActiveTheme as _, Icon, IconName, h_flex};
use tty59_core::core::machine::TabId;

use super::outline::{self, Outline, SymbolKind};
use super::{BufferId, CURSOR_SCROLL_ATTEMPTS, TabCode, place_cursor};
use crate::ui::app::Tty59App;
use crate::ui::host_ops::HostId;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::search::{CommandKind, Item, SearchTab};
use crate::ui::text_elision::{ElidedLabel, Mode};

/// A caret that moves this many lines or more at once has jumped. Arrowing
/// and typing never do; a click, a search hit or Page Down do.
pub(crate) const JUMP_LINES: u32 = 10;

/// How many places the back stack keeps.
pub(crate) const MAX_HISTORY: usize = 50;

/// How long a Back or Forward waits for the file it asked for to open before
/// the caret is watched for jumps again.
const PENDING_TIMEOUT: Duration = Duration::from_secs(3);

/// How often an outline is rebuilt while the text keeps changing. Between
/// rebuilds the breadcrumbs show the last one, a keystroke or two stale.
const OUTLINE_THROTTLE: Duration = Duration::from_millis(250);

/// The key context the editor's chords are bound in: the text field's own.
///
/// Not a context of the editor panel's: gpui ranks a context-free binding as
/// deep as whatever has focus, so a panel-level binding would lose ⌘⇧O to the
/// workspace switcher. At the text field's depth, and listed after the
/// window's table, it wins. Only the editor panel handles these actions, so in
/// any other text field — or the terminal — the chord falls through to the
/// binding next in line.
pub(crate) const KEY_CONTEXT: &str = "Input";

/// The key context of an editor document that has no text field: the image
/// view and the rendered markdown preview. `KEY_CONTEXT` never reaches them,
/// so the chords that must work with any file in front are bound in this one
/// too.
pub(crate) const DOCUMENT_KEY_CONTEXT: &str = "EditorDocument";

/// One place the caret has been.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NavLocation {
    pub host: HostId,
    /// Empty for an untitled buffer.
    pub path: PathBuf,
    /// Set only for an untitled buffer, which has no path to be found by and
    /// can be returned to only while it is still open.
    pub buffer: Option<BufferId>,
    pub pos: Position,
}

impl NavLocation {
    fn same_file(&self, other: &NavLocation) -> bool {
        self.host == other.host && self.path == other.path && self.buffer == other.buffer
    }

    /// In the same file and within a jump of each other: one place, as far
    /// as history is concerned.
    fn near(&self, other: &NavLocation) -> bool {
        self.same_file(other) && self.pos.line.abs_diff(other.pos.line) < JUMP_LINES
    }
}

/// Back and forward stacks, as a browser keeps them.
#[derive(Clone, Debug, Default)]
pub(crate) struct NavHistory {
    back: Vec<NavLocation>,
    forward: Vec<NavLocation>,
}

impl NavHistory {
    /// The caret jumped away from `from`. A new jump forgets the way forward,
    /// and a place near the last one recorded replaces it rather than
    /// stacking up beside it.
    pub(crate) fn push(&mut self, from: NavLocation) {
        self.forward.clear();
        match self.back.last_mut() {
            Some(last) if last.near(&from) => *last = from,
            _ => self.back.push(from),
        }
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
    }

    /// Where Back goes from `current`, which becomes the way forward.
    /// Places near `current` are passed over: going back to where you already
    /// are would look like the key did nothing.
    pub(crate) fn go_back(&mut self, current: Option<NavLocation>) -> Option<NavLocation> {
        let target = pop_past(&mut self.back, current.as_ref())?;
        if let Some(current) = current {
            self.forward.push(current);
        }
        Some(target)
    }

    /// Where Forward goes from `current`, which goes back on the back stack.
    pub(crate) fn go_forward(&mut self, current: Option<NavLocation>) -> Option<NavLocation> {
        let target = pop_past(&mut self.forward, current.as_ref())?;
        if let Some(current) = current {
            match self.back.last_mut() {
                Some(last) if last.near(&current) => *last = current,
                _ => self.back.push(current),
            }
        }
        Some(target)
    }

    /// Moves the places in one file along with edits made to it, so a place
    /// keeps pointing at the same code after lines above it come and go.
    pub(crate) fn shift(&mut self, file: &NavLocation, edits: &[LineEdit]) {
        for loc in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if loc.same_file(file) {
                loc.pos.line = shift_line(loc.pos.line, edits);
            }
        }
    }

    /// Drops the places `keep` turns down, from both stacks.
    pub(crate) fn retain(&mut self, keep: impl Fn(&NavLocation) -> bool) {
        self.back.retain(&keep);
        self.forward.retain(&keep);
    }

    #[cfg(test)]
    fn lens(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }
}

fn shift_line(line: u32, edits: &[LineEdit]) -> u32 {
    edits
        .iter()
        .fold(line as usize, |line, edit| edit.shift(line))
        .min(u32::MAX as usize) as u32
}

fn pop_past(stack: &mut Vec<NavLocation>, current: Option<&NavLocation>) -> Option<NavLocation> {
    loop {
        let top = stack.pop()?;
        if !current.is_some_and(|c| c.near(&top)) {
            return Some(top);
        }
    }
}

/// Where the caret is, and how long the text is — a text that changed length
/// since the last sample was being edited, and an edit is not a jump.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sample {
    pub at: NavLocation,
    pub text_len: usize,
}

/// One tab's history, and the caret sample the next one is compared against.
#[derive(Default)]
pub(crate) struct TabNav {
    pub(crate) history: NavHistory,
    last: Option<Sample>,
    /// A Back or Forward on its way to a file that is still opening. Until it
    /// lands, the caret that has not got there yet is not a jump.
    pending: Option<(NavLocation, Instant)>,
}

impl TabNav {
    /// Compares a fresh sample against the last one and records a jump.
    /// `quiet` says the caret was moved by a key that pages or pastes, which
    /// can go a long way without anyone having jumped anywhere.
    pub(crate) fn observe(&mut self, now: Option<Sample>, at: Instant, quiet: bool) {
        if let Some((target, since)) = &self.pending {
            let arrived = now.as_ref().is_some_and(|s| s.at.same_file(target));
            if arrived || at.duration_since(*since) > PENDING_TIMEOUT {
                self.pending = None;
                if arrived {
                    self.last = now;
                    return;
                }
            } else {
                return;
            }
        }
        let Some(now) = now else {
            // No buffer in front: nothing to compare the next one against but
            // the last place the caret was, which is kept.
            return;
        };
        if let Some(last) = &self.last {
            let jumped = !last.at.same_file(&now.at)
                || (!quiet
                    && last.text_len == now.text_len
                    && last.at.pos.line.abs_diff(now.at.pos.line) >= JUMP_LINES);
            if jumped {
                self.history.push(last.at.clone());
            }
        }
        self.last = Some(now);
    }

    /// Moves the history, and the last sample, along with edits to a file.
    fn shift(&mut self, file: &NavLocation, edits: &[LineEdit]) {
        self.history.shift(file, edits);
        if let Some(last) = self.last.as_mut().filter(|l| l.at.same_file(file)) {
            last.at.pos.line = shift_line(last.at.pos.line, edits);
        }
    }

    fn expect(&mut self, target: NavLocation, at: Instant) {
        self.pending = Some((target, at));
    }
}

struct OutlineCache {
    text: gpui_component::Rope,
    outline: Arc<Outline>,
    at: Instant,
}

/// Go to Symbol's preview: where the caret was before the picker started
/// moving it, put back if the picker is dismissed.
struct SymbolPreview {
    buffer: BufferId,
    origin: Position,
    scroll: gpui::Point<gpui::Pixels>,
}

/// The window's navigation state, held by `EditorPanelState`.
#[derive(Default)]
pub(crate) struct EditorNav {
    tabs: HashMap<TabId, TabNav>,
    outlines: HashMap<BufferId, OutlineCache>,
    /// Symbols handed in from elsewhere — a language server's
    /// `documentSymbol` — which win over the tree's.
    provided: HashMap<BufferId, Arc<Outline>>,
    preview: Option<SymbolPreview>,
    refresh_scheduled: bool,
    /// How far into each buffer's edit log the history has been moved.
    edits_seen: HashMap<BufferId, u64>,
    /// A key that pages or pastes was just pressed in the editor: the caret
    /// move it makes is not a jump.
    quiet_move: bool,
    _keystrokes: Option<gpui::Subscription>,
}

/// Whether an action moves the caret a long way without jumping anywhere:
/// paging through the file, or pasting, undoing and redoing a large edit.
fn is_quiet_move(action: &dyn gpui::Action) -> bool {
    use gpui_component::input::{MovePageDown, MovePageUp, Paste, Redo, Undo};
    let any = action.as_any();
    any.is::<MovePageUp>()
        || any.is::<MovePageDown>()
        || any.is::<Paste>()
        || any.is::<Undo>()
        || any.is::<Redo>()
}

impl EditorNav {
    pub(crate) fn new(cx: &mut Context<Tty59App>) -> Self {
        let keystrokes = cx.observe_keystrokes(|this, event, window, cx| {
            if event.action.as_deref().is_some_and(is_quiet_move)
                && this.editor_panel_has_focus(window, cx)
            {
                this.editor.nav.quiet_move = true;
            }
        });
        Self {
            _keystrokes: Some(keystrokes),
            ..Self::default()
        }
    }

    pub(crate) fn forget_tab(&mut self, tab: TabId) {
        self.tabs.remove(&tab);
    }

    pub(crate) fn forget_buffer(&mut self, id: BufferId) {
        self.outlines.remove(&id);
        self.edits_seen.remove(&id);
        self.provided.remove(&id);
        for nav in self.tabs.values_mut() {
            nav.history.retain(|l| l.buffer != Some(id));
        }
    }
}

impl Tty59App {
    fn nav_tab(&self) -> Option<TabId> {
        self.tabs.get(self.active).map(|t| t.tree_id.get())
    }

    /// Where the caret is in the buffer in front, if there is one.
    fn nav_here(&self, cx: &gpui::App) -> Option<Sample> {
        let f = self.active_buffer()?;
        let state = f.input.read(cx);
        Some(Sample {
            at: NavLocation {
                host: f.host.id(),
                path: f.path.clone(),
                buffer: f.untitled.map(|_| f.id()),
                pos: state.cursor_position(),
            },
            text_len: state.text().len(),
        })
    }

    /// Samples the caret for jumps. Called whenever the editor draws, which
    /// every caret move and every edit makes it do.
    pub(crate) fn editor_nav_tick(&mut self, cx: &mut Context<Self>) {
        // The picker moving the caret around is looking, not jumping; the
        // jump it ends in is seen on the first draw after it closes.
        if self.editor.nav.preview.is_some() {
            return;
        }
        let Some(tab) = self.nav_tab() else { return };
        let now = self.nav_here(cx);
        let quiet = std::mem::take(&mut self.editor.nav.quiet_move);
        self.editor
            .nav
            .tabs
            .entry(tab)
            .or_default()
            .observe(now, Instant::now(), quiet);
    }

    /// A buffer's text changed: every tab's history of it moves with the
    /// lines. A buffer edited so much the log has forgotten some of it keeps
    /// its places where they were.
    pub(crate) fn editor_nav_note_edit(&mut self, id: BufferId, cx: &mut Context<Self>) {
        let Some(f) = self.buffer(id) else { return };
        let file = NavLocation {
            host: f.host.id(),
            path: f.path.clone(),
            buffer: f.untitled.map(|_| id),
            pos: Position::default(),
        };
        let state = f.input.read(cx);
        let version = state.edit_version();
        let seen = self.editor.nav.edits_seen.insert(id, version).unwrap_or(0);
        let Some(edits) = state.line_edits_since(seen) else {
            return;
        };
        if edits.is_empty() {
            return;
        }
        for nav in self.editor.nav.tabs.values_mut() {
            nav.shift(&file, &edits);
        }
    }

    /// Back (or, with `forward`, Forward) through the active tab's history.
    pub(crate) fn editor_navigate(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.nav_tab() else { return };
        // Settle the caret's latest move first, or a jump made since the
        // last draw would be lost.
        self.editor_nav_tick(cx);
        loop {
            let current = self.nav_here(cx).map(|s| s.at);
            let nav = self.editor.nav.tabs.entry(tab).or_default();
            let target = match forward {
                true => nav.history.go_forward(current),
                false => nav.history.go_back(current),
            };
            let Some(target) = target else { return };
            if self.nav_open(tab, target, window, cx) {
                return;
            }
        }
    }

    /// Shows `target`. `false` when it is gone for good — an untitled buffer
    /// that was closed, a machine no longer connected.
    fn nav_open(
        &mut self,
        tab: TabId,
        target: NavLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let at = Instant::now();
        if let Some(id) = target.buffer {
            if self.buffer(id).is_none() {
                return false;
            }
            self.editor
                .nav
                .tabs
                .entry(tab)
                .or_default()
                .expect(target.clone(), at);
            self.editor_show_in_tab(tab, id, true, window, cx);
            if let Some(f) = self.buffer(id) {
                place_cursor(
                    f.input.clone(),
                    target.pos,
                    CURSOR_SCROLL_ATTEMPTS,
                    window,
                    cx,
                );
            }
            return true;
        }
        let Some(host) = crate::ui::host_registry::HostRegistry::lookup(cx, target.host) else {
            return false;
        };
        self.editor
            .nav
            .tabs
            .entry(tab)
            .or_default()
            .expect(target.clone(), at);
        // The same road a file link takes, so a file closed since opens again.
        self.editor.pending_cursor = Some((
            target.path.clone(),
            target.pos.line + 1,
            target.pos.character + 1,
        ));
        self.editor_open_on_host(host, &target.path, window, cx);
        true
    }

    // ---- Outline ----

    /// Replaces the tree's symbols for `buffer` with ones found elsewhere — a
    /// language server's `textDocument/documentSymbol`, flattened with
    /// [`Outline::from_nodes`]. `None` goes back to the tree's. The caller
    /// keeps them current: they are shown as given until replaced.
    pub(crate) fn editor_set_document_symbols(
        &mut self,
        buffer: BufferId,
        symbols: Option<Outline>,
        cx: &mut Context<Self>,
    ) {
        match symbols {
            Some(outline) => self.editor.nav.provided.insert(buffer, Arc::new(outline)),
            None => self.editor.nav.provided.remove(&buffer),
        };
        cx.notify();
    }

    /// A buffer's outline. `fresh` rebuilds a stale one now; otherwise a text
    /// that keeps changing is re-outlined at most every [`OUTLINE_THROTTLE`],
    /// with a redraw scheduled for when it may be.
    fn editor_outline(
        &mut self,
        id: BufferId,
        fresh: bool,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Outline>> {
        if let Some(provided) = self.editor.nav.provided.get(&id) {
            return Some(provided.clone());
        }
        let f = self.buffer(id)?;
        let (language, tree, text) = match f.input.read(cx).syntax_tree() {
            Some((language, tree, text)) => (language.to_string(), Some(tree), text),
            // Not parsed yet — a buffer that has never been drawn. Only worth
            // a parse of our own when someone is waiting on the answer.
            None if fresh => (
                f.language().to_string(),
                None,
                f.input.read(cx).text().clone(),
            ),
            None => return None,
        };
        if !outline::supports(&language) {
            return None;
        }
        if let Some(cache) = self.editor.nav.outlines.get(&id) {
            let current = cache.text.len() == text.len() && cache.text == text;
            if current {
                return Some(cache.outline.clone());
            }
            if !fresh && cache.at.elapsed() < OUTLINE_THROTTLE {
                let stale = cache.outline.clone();
                self.nav_schedule_refresh(cx);
                return Some(stale);
            }
        }
        let outline = Arc::new(match tree {
            Some(tree) => outline::outline_from_tree(&language, &tree, &text),
            None => outline::outline_of_text(&language, &text),
        });
        self.editor.nav.outlines.insert(
            id,
            OutlineCache {
                text,
                outline: outline.clone(),
                at: Instant::now(),
            },
        );
        Some(outline)
    }

    fn nav_schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.editor.nav.refresh_scheduled {
            return;
        }
        self.editor.nav.refresh_scheduled = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUTLINE_THROTTLE).await;
            let _ = this.update(cx, |this, cx| {
                this.editor.nav.refresh_scheduled = false;
                cx.notify();
            });
        })
        .detach();
    }

    // ---- Go to Symbol ----

    /// Go to Symbol in the file in front: the search, on its Symbols tab.
    /// The same chord puts it away again.
    pub(crate) fn editor_go_to_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.clone() {
            if view.read(cx).tab() == SearchTab::Symbols {
                self.close_search(window, cx);
                return;
            }
            self.close_search(window, cx);
        }
        let Some(f) = self.active_buffer() else {
            return;
        };
        let id = f.id();
        let input = f.input.clone();
        // A symbol is a place in the source; a rendered Markdown file shows
        // the source to be taken to it.
        if let Some(f) = self.buffer_mut(id) {
            f.preview = false;
        }
        let (origin, scroll) = {
            let state = input.read(cx);
            (state.cursor_position(), state.scroll_offset())
        };
        let outline = self.editor_outline(id, true, cx).unwrap_or_default();
        let rows = symbol_rows(&outline);
        let here = outline
            .chain_at(origin)
            .last()
            .map(|&ix| symbol_command(&outline.symbols[ix]));
        self.open_search(SearchTab::Symbols, "", window, cx);
        self.editor.nav.preview = Some(SymbolPreview {
            buffer: id,
            origin,
            scroll,
        });
        let live = self.lsp_project_symbol_query(window, cx);
        if let Some(view) = self.search.clone() {
            view.update(cx, |view, cx| {
                view.set_symbols(rows, here, window, cx);
                if let Some(live) = live {
                    view.set_live_query(live, window, cx);
                }
            });
        }
    }

    /// The editor's search row as this window can fill it: Files always,
    /// Symbols with a file in front.
    pub(crate) fn editor_search_tabs(&self) -> Vec<SearchTab> {
        let mut tabs = vec![SearchTab::Files];
        if self.active_buffer().is_some() {
            tabs.push(SearchTab::Symbols);
        }
        tabs
    }

    /// Shows a symbol the picker is on, leaving the keyboard in the picker.
    pub(crate) fn editor_symbol_preview(&mut self, line: u32, column: u32, cx: &mut Context<Self>) {
        let Some(buffer) = self.editor.nav.preview.as_ref().map(|p| p.buffer) else {
            return;
        };
        let Some(f) = self.buffer(buffer) else { return };
        f.input.update(cx, |state, cx| {
            state.preview_cursor_position(Position::new(line, column), cx)
        });
    }

    /// Begins a preview in the file in front for a picker someone else
    /// filled — Find References (`ui::lsp`) — so arrowing through it moves
    /// the caret and closing it puts the caret back, as Go to Symbol does.
    pub(crate) fn editor_begin_preview(&mut self, cx: &App) {
        let Some(f) = self.active_buffer() else {
            return;
        };
        let (origin, scroll) = {
            let state = f.input.read(cx);
            (state.cursor_position(), state.scroll_offset())
        };
        self.editor.nav.preview = Some(SymbolPreview {
            buffer: f.id(),
            origin,
            scroll,
        });
    }

    /// The buffer a preview is running in, if one is.
    pub(crate) fn editor_preview_buffer(&self) -> Option<BufferId> {
        self.editor.nav.preview.as_ref().map(|p| p.buffer)
    }

    /// The picker's choice is final: the caret stays where it took it.
    pub(crate) fn editor_symbol_preview_commit(&mut self) {
        self.editor.nav.preview = None;
    }

    /// The picker went away without a choice: the caret and the scroll go
    /// back to where they were, and so does the keyboard.
    pub(crate) fn editor_symbol_preview_cancel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.editor.nav.preview.take() else {
            return;
        };
        let Some(f) = self.buffer(preview.buffer) else {
            return;
        };
        f.input.update(cx, |state, cx| {
            state.set_cursor_position(preview.origin, window, cx);
            state.set_scroll_offset(preview.scroll, cx);
        });
    }

    /// Puts the caret on a 0-based position in the buffer in front.
    pub(crate) fn editor_go_to_position(
        &mut self,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.nav.preview = None;
        let Some(f) = self.active_buffer() else {
            return;
        };
        place_cursor(
            f.input.clone(),
            Position::new(line, column),
            CURSOR_SCROLL_ATTEMPTS,
            window,
            cx,
        );
    }

    // ---- Chrome ----

    /// The palette's editor rows: the open-files search while the panel holds
    /// any file (text or image), a picture's own rows while one is in front,
    /// and the navigation rows while a text buffer is.
    pub(crate) fn editor_palette_items(&self, cx: &App) -> Vec<Item> {
        use crate::ui::search::CommandGroup;
        let mut items = Vec::new();
        if !self.code_panel_visible() {
            return items;
        }
        if self.tab_code().is_some_and(|c| !c.files.is_empty()) {
            items.push(
                Item::localized(
                    L10nKey::CmdEditorSearchOpenFiles,
                    CommandKind::EditorSearchOpenFiles,
                )
                .with_alias(t(L10nKey::CmdEditorSearchOpenFilesAlias))
                .with_aliases_of(L10nKey::CmdEditorSearchOpenFilesAlias)
                .in_group(CommandGroup::View),
            );
        }
        // A picture is not a text buffer: its rows come before the text gate
        // below, which would otherwise hide them.
        if let Some(actions) = self
            .editor_active_image()
            .and_then(|id| self.image_actions(id, cx))
        {
            let rows = [
                (
                    actions.copy,
                    L10nKey::CmdImageCopy,
                    CommandKind::EditorCopyImage,
                ),
                (
                    actions.download,
                    L10nKey::CmdImageDownload,
                    CommandKind::EditorDownloadImage,
                ),
            ];
            items.extend(
                rows.into_iter()
                    .filter(|(offered, ..)| *offered)
                    .map(|(_, key, kind)| Item::localized(key, kind).in_group(CommandGroup::View)),
            );
        }
        // Any saved document can be shown as bytes, and a hex tab of a text file as text:
        // gated here too, for the same reason.
        if let Some(id) = self.tab_code().and_then(TabCode::active_id) {
            let row = match self.hex_offer(id, cx) {
                Some(super::hex::HexOffer::AsHex) => {
                    Some((L10nKey::CmdOpenAsHex, CommandKind::EditorOpenAsHex))
                }
                Some(super::hex::HexOffer::AsText) => {
                    Some((L10nKey::CmdOpenAsText, CommandKind::EditorOpenAsText))
                }
                None => None,
            };
            items.extend(
                row.map(|(key, kind)| Item::localized(key, kind).in_group(CommandGroup::View)),
            );
        }
        if self.active_buffer().is_none() {
            return items;
        }
        items.extend(
            [
                (L10nKey::CmdEditorGoToSymbol, CommandKind::EditorGoToSymbol),
                (L10nKey::CmdEditorGoBack, CommandKind::EditorNavigateBack),
                (
                    L10nKey::CmdEditorGoForward,
                    CommandKind::EditorNavigateForward,
                ),
                (L10nKey::CmdEditorSplitRight, CommandKind::EditorSplitRight),
            ]
            .into_iter()
            .map(|(key, kind)| Item::localized(key, kind).in_group(CommandGroup::View)),
        );
        items
    }

    /// The editor's own chords, handled by the element that holds it.
    pub(crate) fn editor_nav_actions<E: InteractiveElement>(
        &self,
        element: E,
        cx: &mut Context<Self>,
    ) -> E {
        use crate::core::actions::{
            EditorFocusLeftGroup, EditorFocusRightGroup, EditorGoToSymbol, EditorNavigateBack,
            EditorNavigateForward, EditorSplitRight,
        };
        element
            .on_action(cx.listener(|this, _: &EditorSplitRight, window, cx| {
                if !this.editor_split(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorFocusLeftGroup, window, cx| {
                if !this.editor_focus_group(true, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorFocusRightGroup, window, cx| {
                if !this.editor_focus_group(false, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &EditorGoToSymbol, window, cx| {
                this.editor_go_to_symbol(window, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorNavigateBack, window, cx| {
                this.editor_navigate(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &EditorNavigateForward, window, cx| {
                this.editor_navigate(true, window, cx)
            }))
    }

    /// The row over the text: where the file is, then which symbols the caret
    /// is inside. The symbols open Go to Symbol.
    pub(crate) fn render_editor_breadcrumbs(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let id = self.tab_code().and_then(TabCode::active_id)?;
        let slot = self.tab_code().map_or(0, TabCode::focused_slot);
        self.render_editor_breadcrumbs_for(id, slot, true, window, cx)
    }

    /// The breadcrumbs of one group's file. `focused` is whether that group
    /// has the focus; the other group's symbols first bring the focus over.
    pub(crate) fn render_editor_breadcrumbs_for(
        &mut self,
        id: BufferId,
        slot: usize,
        focused: bool,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let f = self.buffer(id)?;
        let caret = f.input.read(cx).cursor_position();
        // The directories in front of the file, and the file's own name.
        let (dirs, file_name) = if f.untitled.is_some() {
            (String::new(), Some(f.label().to_string()))
        } else {
            let roots = self.tab_code().map(|c| c.roots.as_slice()).unwrap_or(&[]);
            let local = f.host.id() == self.spawn_host(cx);
            match local {
                true => path_crumbs(&self.project_spelling(&f.path, cx), roots),
                false => path_crumbs(&f.path, &[]),
            }
        };
        let file = (f.untitled.is_none()).then(|| f.path.clone());
        // A rendered Markdown file keeps the path; its symbols are the source's.
        let supported = !f.preview && f.untitled.is_none() && outline::supports(f.language());
        let chain: Vec<(SymbolKind, String)> = match supported {
            true => self
                .editor_outline(id, false, cx)
                .map(|o| {
                    o.chain_at(caret)
                        .into_iter()
                        .map(|ix| (o.symbols[ix].kind, o.symbols[ix].name.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            false => Vec::new(),
        };

        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        // The chevron is as quiet as the directories it follows: it separates
        // names, and a fainter one fell under the 3:1 a mark needs.
        let sep = || {
            div().flex_none().px(px(4.)).child(
                Icon::new(IconName::ChevronRight)
                    .size(px(9.))
                    .text_color(muted),
            )
        };
        // The file name is the part the reader came for and never gives way;
        // only the directories in front of it do, from their start, so the
        // folder next to the file is the last thing to go.
        let dirs = gpui::SharedString::from(dirs);
        let has_dirs = !dirs.is_empty();
        let mut path = h_flex()
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .items_center();
        if has_dirs {
            // The directories and the separator behind them are ONE shrinkable slot that clips
            // (overflow_hidden): they yield, and are clipped, together, so a chevron never
            // stands alone and the separator can never take room from the file name.
            path = path.child(
                h_flex()
                    .flex_shrink(1.)
                    .min_w_0()
                    .overflow_hidden()
                    .items_center()
                    .child(div().flex_shrink(1.).min_w_0().text_color(muted).child(
                        ElidedLabel::new(("editor-breadcrumb-dirs", slot), dirs, Mode::Path),
                    ))
                    .child(sep()),
            );
        }
        if let Some(name) = file_name {
            // The file name ALONE is capped to the row and never shrunk by flex: it stays whole
            // whenever it fits the row by itself (the directories slot gives way first), and a
            // name wider than the whole row is cut by its own label INSIDE the cap, in the
            // middle so the extension survives. The label shrinks from its natural width (auto
            // basis), so a name that fits is never touched; nothing overflows to be clipped.
            let name_slot = div()
                .flex_none()
                .max_w(relative(1.))
                .min_w_0()
                .text_color(fg)
                .child(ElidedLabel::new(
                    ("editor-breadcrumb-file", slot),
                    name,
                    Mode::Middle,
                ));
            #[cfg(test)]
            let name_slot = name_slot.debug_selector(|| format!("editor-breadcrumb-file-{slot}"));
            path = path.child(name_slot);
        }
        // The directories may be cut, so the whole path is one hover away. The
        // file's tab carries no path of its own while this row shows it.
        let full_path = file.clone();
        let path = div()
            .id(("editor-breadcrumb-path", slot))
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .cursor_pointer()
            .when_some(full_path, |d, full| {
                d.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(full.display().to_string())
                        .build(window, cx)
                })
            })
            .child(path)
            .on_click(cx.listener(move |this, _, _window, cx| {
                if let Some(file) = &file {
                    this.file_tree_reveal_path(file, cx);
                }
            }));
        #[cfg(test)]
        let path = path.debug_selector(|| format!("editor-breadcrumb-path-{slot}"));

        // A caret outside every symbol has nothing to name, so it draws no
        // crumb at all; Go to Symbol stays on its own chord.
        let symbols = (supported && !chain.is_empty()).then(|| {
            let mut row = h_flex().min_w_0().overflow_hidden().items_center();
            for (kind, name) in chain {
                row = row.child(sep()).child(
                    h_flex()
                        .flex_none()
                        .gap(px(4.))
                        .whitespace_nowrap()
                        // An impl is named by its header, `impl` and all.
                        .when(kind != SymbolKind::Impl, |d| {
                            d.child(div().text_color(muted).child(kind.tag()))
                        })
                        // The symbol is code, so it is set as code, a rung
                        // darker than the path it hangs off.
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(fg.opacity(0.6))
                                .child(name),
                        ),
                );
            }
            let crumb = div()
                .id(("editor-breadcrumb-symbols", slot))
                .flex_shrink(1.)
                .min_w_0()
                .overflow_hidden()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(cx.theme().muted))
                .child(row)
                .tooltip(|window, cx| {
                    gpui_component::tooltip::Tooltip::new(t(L10nKey::EditorGoToSymbolAction))
                        .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !focused {
                        let left = slot == 0;
                        this.editor_focus_group(left, window, cx);
                    }
                    this.editor_go_to_symbol(window, cx)
                }));
            #[cfg(test)]
            let crumb = crumb.debug_selector(|| format!("editor-breadcrumb-symbols-{slot}"));
            crumb
        });

        Some(
            h_flex()
                .id(("editor-breadcrumbs", slot))
                .flex_none()
                .w_full()
                .h(px(22.))
                .items_center()
                .overflow_hidden()
                .px(px(crate::ui::app::CONTENT_INSET))
                .text_size(gpui::rems(crate::ui::right_panel::META))
                .child(path)
                .children(symbols)
                .into_any_element(),
        )
    }
}

/// A path as the breadcrumbs spell it, split for drawing: the directories in
/// front of the file joined with `/`, and the file's own name. From inside the
/// project root it is in, the root's own name leads; outside every root the
/// path is spelled whole. Only the path's own (native) components count, so a
/// root marker or Windows prefix is not a name.
///
/// One pass into one buffer: no per-segment strings and no second join, so the
/// cost does not grow with the depth of the path.
pub(crate) fn path_crumbs(path: &Path, roots: &[PathBuf]) -> (String, Option<String>) {
    let (lead, rel) = match roots
        .iter()
        .find_map(|r| Some((r, path.strip_prefix(r).ok()?)))
    {
        Some((root, rel)) => (root.file_name(), rel),
        None => (None, path),
    };
    let names = lead
        .into_iter()
        .chain(rel.components().filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        }));
    let mut crumbs = String::with_capacity(path.as_os_str().len());
    let mut last = 0;
    for name in names {
        if !crumbs.is_empty() {
            crumbs.push('/');
        }
        last = crumbs.len();
        crumbs.push_str(&name.to_string_lossy());
    }
    // `last` is where the final name starts, so what is split off is exactly
    // that name and what stays ends with the `/` that led into it.
    let file_name = (!crumbs.is_empty()).then(|| crumbs.split_off(last));
    if crumbs.ends_with('/') {
        crumbs.pop();
    }
    (crumbs, file_name)
}

fn symbol_command(symbol: &outline::Symbol) -> CommandKind {
    CommandKind::GoToSymbol {
        line: symbol.selection.line,
        column: symbol.selection.character,
    }
}

/// The picker's rows, in document order, each with how deep it sits.
pub(crate) fn symbol_rows(outline: &Outline) -> Vec<(usize, Item)> {
    outline
        .symbols
        .iter()
        .enumerate()
        .map(|(ix, s)| {
            let mut item = Item::new(s.name.clone(), symbol_command(s)).with_note(s.kind.tag());
            let path = outline.container_path(ix);
            if !path.is_empty() {
                item = item.with_subtitle(path.join(" › "));
            }
            (s.depth, item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: u32) -> NavLocation {
        NavLocation {
            host: HostId::LOCAL,
            path: PathBuf::from(path),
            buffer: None,
            pos: Position::new(line, 0),
        }
    }

    fn sample(path: &str, line: u32, len: usize) -> Option<Sample> {
        Some(Sample {
            at: at(path, line),
            text_len: len,
        })
    }

    #[test]
    fn back_and_forward_walk_the_places_jumped_from() {
        let mut h = NavHistory::default();
        h.push(at("a.rs", 10));
        h.push(at("b.rs", 40));
        // Now at c.rs:5.
        assert_eq!(h.go_back(Some(at("c.rs", 5))), Some(at("b.rs", 40)));
        assert_eq!(h.go_back(Some(at("b.rs", 40))), Some(at("a.rs", 10)));
        assert_eq!(h.go_back(Some(at("a.rs", 10))), None);
        assert_eq!(h.go_forward(Some(at("a.rs", 10))), Some(at("b.rs", 40)));
        assert_eq!(h.go_forward(Some(at("b.rs", 40))), Some(at("c.rs", 5)));
        assert_eq!(h.go_forward(Some(at("c.rs", 5))), None);
    }

    #[test]
    fn a_new_jump_forgets_the_way_forward() {
        let mut h = NavHistory::default();
        h.push(at("a.rs", 10));
        h.push(at("b.rs", 40));
        h.go_back(Some(at("c.rs", 5)));
        assert_eq!(h.lens(), (1, 1));
        h.push(at("b.rs", 40));
        assert_eq!(h.lens(), (2, 0));
        assert_eq!(h.go_forward(Some(at("d.rs", 0))), None);
    }

    #[test]
    fn places_near_each_other_are_one_entry() {
        let mut h = NavHistory::default();
        h.push(at("a.rs", 10));
        h.push(at("a.rs", 14));
        assert_eq!(h.lens(), (1, 0));
        h.push(at("a.rs", 30));
        h.push(at("b.rs", 14));
        assert_eq!(h.lens(), (3, 0));
        // Back from right beside the last entry passes over it.
        assert_eq!(h.go_back(Some(at("b.rs", 16))), Some(at("a.rs", 30)));
    }

    #[test]
    fn the_back_stack_is_capped() {
        let mut h = NavHistory::default();
        for i in 0..(MAX_HISTORY as u32 + 5) {
            h.push(at("a.rs", i * JUMP_LINES));
        }
        assert_eq!(h.lens().0, MAX_HISTORY);
        let mut oldest = None;
        let mut cur = at("z.rs", 0);
        while let Some(l) = h.go_back(Some(cur.clone())) {
            cur = l.clone();
            oldest = Some(l);
        }
        assert_eq!(oldest, Some(at("a.rs", 5 * JUMP_LINES)));
    }

    #[test]
    fn closed_untitled_buffers_can_be_dropped() {
        let mut h = NavHistory::default();
        let mut untitled = at("", 3);
        untitled.buffer = Some(gpui::EntityId::from(7u64));
        h.push(untitled);
        h.push(at("a.rs", 0));
        h.retain(|l| l.buffer.is_none());
        assert_eq!(h.lens(), (1, 0));
    }

    #[test]
    fn the_tracker_records_jumps_but_not_walks_or_edits() {
        let t0 = Instant::now();
        let mut nav = TabNav::default();
        nav.observe(sample("a.rs", 0, 100), t0, false);
        // Arrowing down a line at a time is not a jump.
        for line in 1..=30 {
            nav.observe(sample("a.rs", line, 100), t0, false);
        }
        assert_eq!(nav.history.lens(), (0, 0));
        // Pasting forty lines moves the caret forty lines, and is an edit.
        nav.observe(sample("a.rs", 70, 900), t0, false);
        assert_eq!(nav.history.lens(), (0, 0));
        // A click far away is.
        nav.observe(sample("a.rs", 300, 900), t0, false);
        assert_eq!(nav.history.back, [at("a.rs", 70)]);
        // So is another file coming forward.
        nav.observe(sample("b.rs", 0, 10), t0, false);
        assert_eq!(nav.history.back, [at("a.rs", 70), at("a.rs", 300)]);
        // No buffer in front keeps the last place for the next one.
        nav.observe(None, t0, false);
        nav.observe(sample("c.rs", 0, 10), t0, false);
        assert_eq!(nav.history.back.last(), Some(&at("b.rs", 0)));
    }

    #[test]
    fn a_pending_back_is_not_mistaken_for_a_jump() {
        let t0 = Instant::now();
        let mut nav = TabNav::default();
        nav.observe(sample("a.rs", 0, 10), t0, false);
        nav.observe(sample("b.rs", 50, 10), t0, false);
        let target = nav.history.go_back(Some(at("b.rs", 50))).unwrap();
        nav.expect(target, t0);
        // The file is still opening: b.rs is on screen, and that is fine.
        nav.observe(sample("b.rs", 50, 10), t0, false);
        nav.observe(sample("a.rs", 0, 10), t0, false);
        assert_eq!(nav.history.lens(), (0, 1));
        // A target that never arrives stops being waited on.
        nav.expect(at("gone.rs", 0), t0);
        nav.observe(
            sample("a.rs", 0, 10),
            t0 + PENDING_TIMEOUT + Duration::from_secs(1),
            false,
        );
        nav.observe(
            sample("b.rs", 0, 10),
            t0 + PENDING_TIMEOUT + Duration::from_secs(1),
            false,
        );
        assert_eq!(nav.history.back, [at("a.rs", 0)]);
    }

    #[test]
    fn paging_pasting_and_undoing_are_the_quiet_moves() {
        use gpui_component::input::{MoveDown, MovePageDown, MovePageUp, Paste, Redo, Undo};
        assert!(is_quiet_move(&MovePageDown));
        assert!(is_quiet_move(&MovePageUp));
        assert!(is_quiet_move(&Paste));
        assert!(is_quiet_move(&Undo));
        assert!(is_quiet_move(&Redo));
        assert!(!is_quiet_move(&MoveDown));
    }

    #[test]
    fn a_page_down_or_a_paste_is_not_a_jump() {
        let t0 = Instant::now();
        let mut nav = TabNav::default();
        nav.observe(sample("a.rs", 0, 100), t0, false);
        nav.observe(sample("a.rs", 60, 100), t0, true);
        nav.observe(sample("a.rs", 120, 100), t0, true);
        assert_eq!(nav.history.lens(), (0, 0));
        // Only the move the key made: the next one is judged on its own.
        nav.observe(sample("a.rs", 400, 100), t0, false);
        assert_eq!(nav.history.back, [at("a.rs", 120)]);
    }

    #[test]
    fn places_move_with_lines_inserted_or_deleted_above_them() {
        let mut nav = TabNav::default();
        let t0 = Instant::now();
        nav.observe(sample("a.rs", 40, 10), t0, false);
        nav.observe(sample("b.rs", 0, 10), t0, false);
        nav.history.push(at("a.rs", 90));
        // Three lines pasted at line 10 of a.rs, then lines 20..=29 deleted.
        let edits = [
            LineEdit {
                start_line: 10,
                end_line: 10,
                new_lines: 3,
                at_line_start: false,
            },
            LineEdit {
                start_line: 20,
                end_line: 30,
                new_lines: 0,
                at_line_start: false,
            },
        ];
        nav.shift(&at("a.rs", 0), &edits);
        // b.rs is untouched; 40 → 43 → 33, 90 → 93 → 83.
        assert_eq!(nav.history.back, [at("a.rs", 33), at("a.rs", 83)]);
        nav.shift(&at("b.rs", 0), &edits[..1]);
        assert_eq!(nav.last.as_ref().unwrap().at, at("b.rs", 0));
    }

    #[test]
    fn breadcrumb_paths_start_at_their_project() {
        let crumbs = |path: &str, roots: &[PathBuf]| path_crumbs(Path::new(path), roots);
        let spelled = |dirs: &str, file: &str| (dirs.to_string(), Some(file.to_string()));
        let roots = [PathBuf::from("/work/tty59")];
        assert_eq!(
            crumbs("/work/tty59/src/ui/code_editor.rs", &roots),
            spelled("tty59/src/ui", "code_editor.rs"),
            "inside a root the root's own name leads"
        );
        assert_eq!(
            crumbs("/work/tty59/README.md", &roots),
            spelled("tty59", "README.md")
        );
        assert_eq!(
            crumbs("/work/tty59", &roots),
            (String::new(), Some("tty59".into())),
            "the root itself is only its name"
        );
        assert_eq!(
            crumbs("/etc/hosts", &roots),
            spelled("etc", "hosts"),
            "outside every root the path is spelled whole"
        );
        assert_eq!(
            crumbs("/srv/app/main.rs", &[]),
            spelled("srv/app", "main.rs"),
            "a remote file has no local root to start from"
        );
        assert_eq!(crumbs("/", &[]), (String::new(), None), "no name at all");
        assert_eq!(
            crumbs("/проект/源码/ファイル.rs", &[]),
            spelled("проект/源码", "ファイル.rs"),
            "multibyte names are cut on their own boundaries"
        );
    }

    /// `Path::components` follows the platform's own separators, and the
    /// crumbs keep that: a backslash is a filename character on Unix, and a
    /// drive prefix is not a name on Windows.
    #[cfg(not(windows))]
    #[test]
    fn a_backslash_is_an_ordinary_character_in_a_unix_breadcrumb() {
        assert_eq!(
            path_crumbs(Path::new(r"/srv/a\b/c.rs"), &[]),
            (r"srv/a\b".to_string(), Some("c.rs".to_string()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_drive_prefix_is_not_a_breadcrumb_name() {
        assert_eq!(
            path_crumbs(Path::new(r"C:\Users\dev\app\main.rs"), &[]),
            ("Users/dev/app".to_string(), Some("main.rs".to_string()))
        );
    }

    #[test]
    fn symbol_rows_carry_their_containers_and_the_breadcrumb_chain_names_them() {
        let src = "impl Tty59App {\n    fn open_file(&self) {\n        let x = 1;\n    }\n}\n";
        let outline = outline::outline_of("rust", src);
        let rows = symbol_rows(&outline);
        let shown: Vec<_> = rows
            .iter()
            .map(|(d, i)| {
                (
                    *d,
                    i.title.as_str(),
                    i.subtitle.as_deref(),
                    i.note.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (0, "impl Tty59App", None, Some("impl")),
                (1, "open_file", Some("impl Tty59App"), Some("method")),
            ]
        );
        assert_eq!(
            rows[1].1.kind,
            CommandKind::GoToSymbol { line: 1, column: 7 }
        );
        let chain: Vec<_> = outline
            .chain_at(Position::new(2, 8))
            .into_iter()
            .map(|i| {
                format!(
                    "{} {}",
                    outline.symbols[i].kind.tag(),
                    outline.symbols[i].name
                )
            })
            .collect();
        assert_eq!(chain, ["impl impl Tty59App", "method open_file"]);
    }
}

#[cfg(test)]
pub(super) mod gpui_tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::editor_text::{EditorConfig, TextFormat};
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};

    /// `alpha` on line 0, `beta` on 30, `gamma` on 50.
    pub(crate) fn source() -> String {
        let mut lines = vec!["fn alpha() {".to_string()];
        lines.extend((1..=20).map(|_| "    let x = 1;".to_string()));
        lines.push("}".into());
        lines.extend((22..30).map(|_| String::new()));
        lines.push("fn beta() {".into());
        lines.push("}".into());
        lines.extend((32..50).map(|_| String::new()));
        lines.push("fn gamma() {".into());
        lines.push("}".into());
        lines.join("\n") + "\n"
    }

    pub(crate) fn open_rs(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        path: &str,
    ) -> BufferId {
        let path = PathBuf::from(path);
        app.update_in(vcx, |app, window, cx| {
            let host = app.active_host(cx).expect("a host");
            let tab = app.nav_tab().expect("a tab");
            let indent = crate::ui::editor_text::detect_indent("", "rust");
            let id = app.editor_new_buffer(
                host,
                path,
                None,
                source(),
                TextFormat::default(),
                EditorConfig::default(),
                indent,
                None,
                window,
                cx,
            );
            app.editor_show_in_tab(tab, id, true, window, cx);
            id
        })
    }

    pub(crate) fn caret(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Position {
        app.read_with(vcx, |app, cx| {
            app.active_buffer()
                .unwrap()
                .input
                .read(cx)
                .cursor_position()
        })
    }

    /// What drawing the editor does: sample the caret.
    pub(crate) fn draw(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        app.update(vcx, |app, cx| app.editor_nav_tick(cx));
    }

    pub(crate) fn put_caret(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, line: u32) {
        app.update_in(vcx, |app, window, cx| {
            let input = app.active_buffer().unwrap().input.clone();
            input.update(cx, |s, cx| {
                s.set_cursor_position(Position::new(line, 4), window, cx)
            });
        });
        draw(app, vcx);
    }

    /// A real frame: the breadcrumbs are drawn by the editor panel's own
    /// render, not by calling its helper.
    fn paint(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// With the caret between symbols the row draws the path and nothing
    /// after it: a lone `…` crumb named nothing. Inside one, the symbol crumb
    /// is there, so the absence below is not a row that never draws them.
    #[gpui::test]
    fn the_breadcrumbs_draw_a_symbol_crumb_only_while_the_caret_is_inside_one(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            paint(&mut vcx);
            if vcx.debug_bounds("editor-breadcrumb-symbols-0").is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the caret is inside `alpha` and the crumb never appeared"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(vcx.debug_bounds("editor-breadcrumb-path-0").is_some());

        // Line 25 is one of the blank lines between `alpha` and `beta`.
        put_caret(&app, &mut vcx, 25);
        paint(&mut vcx);
        assert!(
            vcx.debug_bounds("editor-breadcrumb-path-0").is_some(),
            "the path is still drawn"
        );
        assert!(
            vcx.debug_bounds("editor-breadcrumb-symbols-0").is_none(),
            "a caret outside every symbol draws no symbol crumb"
        );
    }

    /// A file name wider than the whole breadcrumb row is cut by its own label,
    /// in the middle so the extension survives, inside a group (separator +
    /// name) capped to the row: nothing is left to overflow and be clipped at the
    /// right edge. Real layout at a narrow and a wide window.
    #[gpui::test]
    fn an_overlong_filename_is_cut_in_the_middle_inside_the_breadcrumb_row(
        cx: &mut TestAppContext,
    ) {
        use crate::ui::text_elision::observe;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let name = format!("{}.rs", "a-very-long-file-name-".repeat(10));
        open_rs(&app, &mut vcx, &format!("/nav-test/dir/{name}"));
        let file = gpui::ElementId::from(("editor-breadcrumb-file", 0usize));
        for width in [960., 1440.] {
            vcx.simulate_resize(size(px(width), px(600.)));
            paint(&mut vcx);
            observe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let label = observe::label(&file)
                .unwrap_or_else(|| panic!("at {width}: the file label was not drawn"));
            assert!(label.painted, "at {width}: {label:?}");
            assert!(
                label.text.contains('…') && label.text.len() < name.len(),
                "the overlong filename was not cut in the middle at {width}: {:?}",
                label.text
            );
            assert!(
                label.text.ends_with(".rs") && label.text.starts_with("a-very"),
                "the cut filename keeps its extension and its start at {width}: {:?}",
                label.text
            );
            let group = vcx
                .debug_bounds("editor-breadcrumb-file-0")
                .expect("the filename group was painted");
            let path = vcx
                .debug_bounds("editor-breadcrumb-path-0")
                .expect("the path row was painted");
            assert!(
                group.left() >= path.left() - px(0.5) && group.right() <= path.right() + px(0.5),
                "at {width}: the filename group {group:?} overflows the breadcrumb row {path:?}"
            );
        }
    }

    /// File priority is unchanged: a name that fits stays whole while the
    /// directories in front of it are the ones that yield. A short name at the
    /// narrow window, and the native fixture's actual long name (a component
    /// file with a long descriptive name, 57 characters) at the wide control.
    #[gpui::test]
    fn a_filename_that_fits_stays_whole_while_the_directories_yield(cx: &mut TestAppContext) {
        use crate::ui::text_elision::observe;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dirs: Vec<String> = (0..4)
            .map(|i| format!("{}{i}", "directory-with-a-long-name-".repeat(3)))
            .collect();
        for (width, name) in [
            (960., "main.rs"),
            (
                1440.,
                "a-very-long-component-file-name-for-the-breadcrumb-row.ts",
            ),
        ] {
            open_rs(
                &app,
                &mut vcx,
                &format!("/nav-test/{}/{name}", dirs.join("/")),
            );
            vcx.simulate_resize(size(px(width), px(600.)));
            paint(&mut vcx);
            observe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            // The directories first: they must be drawn and LEADING-elided (`…/last-dirs`, the
            // Proposed) before anything is expected of the filename, so a layout that pushes the
            // name off screen fails on the directories' guard, not on a later missing label.
            let dirs_label =
                observe::label(&gpui::ElementId::from(("editor-breadcrumb-dirs", 0usize)))
                    .unwrap_or_else(|| {
                        panic!("at {width}: the directories yield first: no directories label")
                    });
            assert!(
                dirs_label.painted && dirs_label.text.starts_with('…'),
                "at {width}: the directories yield first, from their start: {dirs_label:?}"
            );
            let file = observe::label(&gpui::ElementId::from(("editor-breadcrumb-file", 0usize)))
                .unwrap_or_else(|| panic!("at {width}: the file label was not drawn"));
            assert!(
                file.painted && file.text.as_str() == name,
                "at {width}: the filename stays whole: {file:?}"
            );
        }
    }

    /// BOUNDARY WITNESS. A file name that fits the breadcrumb row BY ITSELF is never cut,
    /// even when the row cannot also hold the separator in front of it: the separator
    /// belongs to the directories' slot, which yields (and clips) first. The name's natural
    /// width comes from the shared text measurer (theme font, the row's text-size token), NOT
    /// from any painted node, and the row's width from the path row's own bounds; a sweep of
    /// window widths must cross the boundary, so every width between "name alone fits" and
    /// "name plus separator fits" is exercised without any copied pixel constant.
    #[gpui::test]
    fn a_filename_that_fits_the_row_alone_is_never_cut_by_the_separator(cx: &mut TestAppContext) {
        use crate::ui::text_elision::observe;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        // `.txt`: no outline, so no symbol crumb shares the row with the path.
        let name = "a-descriptive-component-file-name-that-is-fairly-long-to-fill-the-row.txt";
        let dirs = [
            "directory-with-a-long-name-one",
            "directory-with-a-long-name-two",
            "directory-with-a-long-name-three",
        ]
        .join("/");
        open_rs(&app, &mut vcx, &format!("/nav-test/{dirs}/{name}"));
        // The name's own natural width, measured INDEPENDENTLY of any production node: the
        // shared text measurer, the theme font and the breadcrumb row's text-size token at the
        // window's rem. It must not come from the file-name slot's bounds, which are whatever
        // the structure under test puts in that slot (a separator moved back into it would
        // widen the oracle with it and the boundary would hide).
        vcx.simulate_resize(size(px(3000.), px(600.)));
        paint(&mut vcx);
        let natural = vcx.update(|window, cx| {
            let font = gpui::Font {
                family: cx.theme().font_family.clone(),
                features: Default::default(),
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: Default::default(),
            };
            let size = window.rem_size().as_f32() * crate::ui::right_panel::META;
            px(crate::ui::tab_strip::measure_text(
                window.text_system(),
                &font,
                size,
                name,
            ))
        });
        // Sanity only (true for any structure): the slot holds at least the name itself.
        let slot_at_3000 = vcx
            .debug_bounds("editor-breadcrumb-file-0")
            .expect("the file name was painted in a very wide window")
            .size
            .width;
        assert!(
            natural > px(100.) && slot_at_3000 + px(0.5) >= natural,
            "the independent measure ({natural:?}) disagrees with the painted slot ({slot_at_3000:?})"
        );
        let file = gpui::ElementId::from(("editor-breadcrumb-file", 0usize));
        let (mut whole_seen, mut cut_seen) = (false, false);
        for width in (400..=1600).rev().step_by(3).map(|w| w as f32) {
            vcx.simulate_resize(size(px(width), px(600.)));
            paint(&mut vcx);
            observe::clear();
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let row = vcx
                .debug_bounds("editor-breadcrumb-path-0")
                .unwrap_or_else(|| panic!("at {width}: the path row was not painted"))
                .size
                .width;
            let label = observe::label(&file)
                .unwrap_or_else(|| panic!("at {width}: the file label was not drawn"));
            let whole = label.text.as_str() == name;
            // Only the two CLEAR regimes are judged: a row within half a pixel of the name
            // (layout rounding against a fractional measure) is skipped, never widened into
            // either side.
            if row > natural + px(0.5) {
                assert!(
                    whole,
                    "at {width}: the filename fits the row alone ({row:?} > {natural:?}) and is cut: {:?}",
                    label.text
                );
                whole_seen = true;
            } else if row + px(0.5) < natural {
                assert!(
                    !whole,
                    "at {width}: the row ({row:?}) is narrower than the filename ({natural:?}) and shows it whole"
                );
                cut_seen = true;
            }
        }
        assert!(
            whole_seen && cut_seen,
            "the sweep did not cross the boundary: whole {whole_seen}, cut {cut_seen}"
        );
    }

    #[gpui::test]
    fn go_to_symbol_previews_and_escape_puts_the_caret_back(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        let symbols = app.read_with(&vcx, |app, cx| {
            let view = app.search.clone().expect("the search is open");
            assert_eq!(view.read(cx).tab(), SearchTab::Symbols);
            view.read(cx).symbol_count()
        });
        assert_eq!(symbols, 3);
        // Opening on the symbol around the caret does not move it.
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4));

        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(30, 3),
            "beta is previewed"
        );

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4), "and put back");
        draw(&app, &mut vcx);
        app.read_with(&vcx, |app, _| {
            let tab = app.nav_tab().unwrap();
            assert!(
                app.editor.nav.tabs[&tab].history.back.is_empty(),
                "looking is not jumping"
            );
        });
    }

    #[gpui::test]
    fn a_symbol_jump_can_be_walked_back_and_forward(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("g a m m a");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
        assert_eq!(caret(&app, &mut vcx), Position::new(50, 3));
        draw(&app, &mut vcx);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(5, 4),
            "back to where it was"
        );
        draw(&app, &mut vcx);

        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(true, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx),
            Position::new(50, 3),
            "and forward again"
        );
        draw(&app, &mut vcx);

        // Another file coming forward is a jump too, and Back returns to the
        // first one.
        open_rs(&app, &mut vcx, "/nav-test/b.rs");
        draw(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, _| {
            assert_eq!(
                app.active_buffer().unwrap().path,
                PathBuf::from("/nav-test/a.rs")
            );
        });
        assert_eq!(caret(&app, &mut vcx), Position::new(50, 3));
    }

    #[gpui::test]
    fn history_follows_edits_to_the_file(cx: &mut TestAppContext) {
        use gpui::EntityInputHandler as _;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        put_caret(&app, &mut vcx, 50);
        // Three lines typed at the top of the file.
        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_buffer().unwrap().input.clone();
            input.update(cx, |s, cx| {
                s.replace_text_in_range(Some(0..0), "//\n//\n//\n", window, cx);
                // Typing took the caret to the top; back to where it was
                // reading, which has moved down with its line.
                s.set_cursor_position(Position::new(53, 4), window, cx);
            });
        });
        vcx.run_until_parked();
        draw(&app, &mut vcx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_navigate(false, window, cx)
        });
        vcx.run_until_parked();
        assert_eq!(
            caret(&app, &mut vcx).line,
            8,
            "the place moved down with its code"
        );
    }

    #[gpui::test]
    fn the_symbol_chord_puts_go_to_symbol_away(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        put_caret(&app, &mut vcx, 5);
        app.update_in(&mut vcx, |app, window, cx| {
            app.editor_go_to_symbol(window, cx)
        });
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
        vcx.simulate_keystrokes("secondary-shift-o");
        vcx.run_until_parked();
        assert!(
            app.read_with(&vcx, |app, _| app.search.is_none()),
            "the chord that opened it closes it"
        );
        assert_eq!(caret(&app, &mut vcx), Position::new(5, 4));
    }

    /// Go to File and Go to Symbol share the editor's row: Tab walks from one
    /// to the other, each arriving set up the way its chord sets it up.
    #[gpui::test]
    fn tab_walks_the_editors_row(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        // No file in front: Files is all the row would hold.
        assert_eq!(
            app.read_with(&vcx, |app, _| app.editor_search_tabs()),
            vec![SearchTab::Files]
        );
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        assert_eq!(
            app.read_with(&vcx, |app, _| app.editor_search_tabs()),
            vec![SearchTab::Files, SearchTab::Symbols]
        );
        app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
        vcx.run_until_parked();
        let tab = |app: &Entity<Tty59App>, vcx: &mut gpui::VisualTestContext| {
            app.read_with(vcx, |app, cx| app.search.as_ref().map(|v| v.read(cx).tab()))
        };
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Files));
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Symbols));
        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();
        assert_eq!(tab(&app, &mut vcx), Some(SearchTab::Files));
    }

    #[gpui::test]
    fn the_palette_offers_the_editor_commands_only_with_a_file_open(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        assert!(app.read_with(&vcx, |app, cx| app.editor_palette_items(cx).is_empty()));
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        let kinds: Vec<CommandKind> = app.read_with(&vcx, |app, cx| {
            app.editor_palette_items(cx)
                .into_iter()
                .map(|i| i.kind)
                .collect()
        });
        assert_eq!(
            kinds,
            [
                CommandKind::EditorSearchOpenFiles,
                CommandKind::EditorOpenAsHex,
                CommandKind::EditorGoToSymbol,
                CommandKind::EditorNavigateBack,
                CommandKind::EditorNavigateForward,
                CommandKind::EditorSplitRight,
            ]
        );
    }

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><rect width="32" height="16" fill="#336699"/></svg>"##;

    /// Opens a real SVG through the shared entry point, so an image document
    /// is what is in front, and waits (bounded) for the off-thread read.
    fn open_svg(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, dir: &tempfile::TempDir) {
        let path = tty59_core::core::path_spelling::local_spelling_buf(
            std::fs::canonicalize(dir.path()).unwrap(),
        )
        .join("logo.svg");
        std::fs::write(&path, SVG).unwrap();
        app.update_in(vcx, |app, window, cx| {
            app.open_file_in_editor(&path, window, cx);
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.read_with(vcx, |app, _| {
            app.tab_code().is_none_or(|code| code.files.is_empty())
        }) && Instant::now() < deadline
        {
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        paint(vcx);
    }

    /// (picker open, keyboard in its filter).
    fn picker(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> (bool, bool) {
        app.update_in(vcx, |app, window, cx| {
            (
                app.editor_strip_picker_open(),
                app.editor_strip_picker_filter_focused(window, cx),
            )
        })
    }

    fn text_focused(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> bool {
        app.update_in(vcx, |app, window, cx| app.text_has_focus(window, cx))
    }

    fn focus_editor_now(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        app.update_in(vcx, |app, window, cx| app.focus_editor(window, cx));
        paint(vcx);
    }

    /// Exercises the root handler's open and close arms and the binding that
    /// reaches them: the chord opens the picker with the filter focused, and
    /// again closes it with the keyboard back in the text.
    #[gpui::test]
    fn the_chord_opens_and_closes_the_picker_from_a_text_file(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        focus_editor_now(&app, &mut vcx);
        assert!(text_focused(&app, &mut vcx));
        assert_eq!(picker(&app, &mut vcx), (false, false));

        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (true, true), "the chord opens it");

        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (false, false), "and closes it");
        assert!(text_focused(&app, &mut vcx), "the keyboard is back");
    }

    /// The image document declares `EditorDocument`, which `Input` never
    /// reaches: with a picture in front the chord still opens the picker.
    #[gpui::test]
    fn the_chord_opens_the_picker_from_an_image_document(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let dir = tempfile::tempdir().unwrap();
        open_svg(&app, &mut vcx, &dir);
        focus_editor_now(&app, &mut vcx);
        assert!(
            app.update_in(&mut vcx, |app, window, cx| app.image_has_focus(window, cx)),
            "the picture has the focus"
        );

        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (true, true));
    }

    /// The terminal never matches the binding, so the picker stays closed with
    /// a shell focused.
    #[gpui::test]
    fn the_chord_leaves_the_picker_closed_with_the_terminal_focused(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        paint(&mut vcx);
        assert!(!text_focused(&app, &mut vcx));

        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (false, false));
    }

    /// ⌘⇧↵ / Ctrl+Shift+Enter is Zoom Pane in a terminal and the document fill
    /// toggle in a document, through the real keymap and the real dispatch: in
    /// the focused editor it fills the workspace with the file and docks it
    /// again — the column back at its width, the keyboard still in the text,
    /// and no line inserted above — and on a pane of a split it still zooms
    /// the pane and leaves the document where it was.
    #[gpui::test]
    fn the_zoom_chord_fills_the_focused_document_and_zooms_the_focused_pane(
        cx: &mut TestAppContext,
    ) {
        use crate::core::config::DocumentLayout;
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::pane::{Pane, PaneSlot};

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        vcx.simulate_resize(size(px(1440.), px(900.)));
        // A second pane, so there is something for the zoom to hide.
        let _sibling = app.update_in(&mut vcx, |app, window, cx| {
            let (view, stream) = quiet_test_pane(90, window, cx);
            let tab = &mut app.tabs[0];
            let first = tab.pane.first_leaf().expect("tab has a pane");
            tab.pane = Pane::split_node(
                gpui::Axis::Horizontal,
                0.5,
                Pane::leaf(first),
                Pane::leaf(PaneSlot::Ready(view)),
            );
            stream
        });
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        focus_editor_now(&app, &mut vcx);
        assert!(text_focused(&app, &mut vcx), "the file has the keyboard");

        let layout = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |app, _, cx| app.document_layout(cx))
        };
        let dock_px = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.update_in(vcx, |app, window, cx| app.document_dock_px(window, cx))
        };
        let text = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.read_with(vcx, |app, cx| {
                app.active_buffer()
                    .unwrap()
                    .input
                    .read(cx)
                    .value()
                    .to_string()
            })
        };
        let zoomed = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| {
            app.read_with(vcx, |app, _| app.maximized.is_some())
        };
        assert_eq!(layout(&app, &mut vcx), DocumentLayout::Dock);
        let docked = dock_px(&app, &mut vcx).expect("a 1440 window seats both");

        vcx.simulate_keystrokes("secondary-shift-enter");
        paint(&mut vcx);
        assert_eq!(
            layout(&app, &mut vcx),
            DocumentLayout::Fill,
            "the chord in the editor fills the workspace with the file"
        );
        assert_eq!(dock_px(&app, &mut vcx), None, "no column while filling");
        assert!(!zoomed(&app, &mut vcx), "and zooms no pane");
        assert!(
            text_focused(&app, &mut vcx),
            "the keyboard stays in the file"
        );

        vcx.simulate_keystrokes("secondary-shift-enter");
        paint(&mut vcx);
        assert_eq!(
            layout(&app, &mut vcx),
            DocumentLayout::Dock,
            "again, and the file docks beside the terminal"
        );
        let back = dock_px(&app, &mut vcx).expect("docked again");
        assert!(
            (back - docked).abs() < 0.5,
            "the column comes back at its width: {back} after {docked}"
        );
        assert!(
            text_focused(&app, &mut vcx),
            "the keyboard is still in the file"
        );
        assert_eq!(
            text(&app, &mut vcx),
            source(),
            "Insert Line Above did not run alongside"
        );

        // The terminal side: the same chord is Zoom Pane's, as it always was.
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        paint(&mut vcx);
        assert!(!text_focused(&app, &mut vcx));
        vcx.simulate_keystrokes("secondary-shift-enter");
        paint(&mut vcx);
        assert!(zoomed(&app, &mut vcx), "the chord on a pane zooms it");
        assert_eq!(
            layout(&app, &mut vcx),
            DocumentLayout::Dock,
            "and leaves the document's layout alone"
        );
        vcx.simulate_keystrokes("secondary-shift-enter");
        paint(&mut vcx);
        assert!(!zoomed(&app, &mut vcx), "and again restores the split");
    }

    /// `Input` covers every text field, so the root handler has to let the
    /// chord go on from the ones that are not the editor: a settings field
    /// and the search palette's query. The picker stays closed in both, and
    /// the field keeps its text.
    #[gpui::test]
    fn the_chord_leaves_other_text_fields_alone(cx: &mut TestAppContext) {
        use crate::core::actions::TogglePalette;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");

        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(crate::ui::settings::SettingsSection::General, window, cx)
        });
        paint(&mut vcx);
        let query = app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_settings().unwrap().search.clone();
            input.update(cx, |s, cx| {
                s.set_value("theme", window, cx);
                s.focus(window, cx);
            });
            input
        });
        paint(&mut vcx);
        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (false, false), "settings field");
        assert_eq!(query.read_with(&vcx, |s, _| s.value().to_string()), "theme");

        vcx.dispatch_action(TogglePalette);
        paint(&mut vcx);
        vcx.simulate_input("abc");
        vcx.simulate_keystrokes("secondary-shift-a");
        paint(&mut vcx);
        assert_eq!(picker(&app, &mut vcx), (false, false), "palette query");
    }

    /// The row is offered while the panel holds any file, text or image, and
    /// not with the panel hidden or no file; "tabs" finds it by its alias.
    #[gpui::test]
    fn the_palette_offers_search_open_files_while_any_file_is_open(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        let row = |app: &Entity<Tty59App>, vcx: &mut VisualTestContext| -> Option<Item> {
            app.read_with(vcx, |app, cx| {
                app.editor_palette_items(cx)
                    .into_iter()
                    .find(|i| i.kind == CommandKind::EditorSearchOpenFiles)
            })
        };
        assert!(row(&app, &mut vcx).is_none(), "no file open");

        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        let item = row(&app, &mut vcx).expect("a text file is open");
        assert_eq!(item.title, "Search Open Files");
        assert!(
            item.aliases
                .iter()
                .any(|a| a.to_lowercase().contains("tabs")),
            "\"tabs\" finds it: {:?}",
            item.aliases
        );
        assert_eq!(
            item.kind.id(),
            Some("editor-search-open-files"),
            "the command id"
        );

        app.update(&mut vcx, |app, _| {
            app.tab_code_mut_or_init().unwrap().visible = false;
        });
        assert!(row(&app, &mut vcx).is_none(), "the panel is hidden");

        // An editor holding only a picture still has files to search.
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        let dir = tempfile::tempdir().unwrap();
        open_svg(&app, &mut vcx, &dir);
        assert!(
            app.read_with(&vcx, |app, _| app.active_buffer().is_none()),
            "no text buffer is in front"
        );
        assert!(row(&app, &mut vcx).is_some(), "an image-only editor");
    }

    /// Run from the palette, the command opens the picker and the filter is
    /// still what holds the keyboard after the palette has closed.
    #[gpui::test]
    fn running_the_palette_row_opens_the_picker_with_its_filter_focused(cx: &mut TestAppContext) {
        use crate::core::actions::TogglePalette;
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, "/nav-test/a.rs");
        focus_editor_now(&app, &mut vcx);

        vcx.dispatch_action(TogglePalette);
        paint(&mut vcx);
        vcx.simulate_input("search open files");
        paint(&mut vcx);
        vcx.simulate_keystrokes("enter");
        paint(&mut vcx);
        paint(&mut vcx);
        assert!(
            app.read_with(&vcx, |app, _| app.search.is_none()),
            "the palette closed"
        );
        assert_eq!(picker(&app, &mut vcx), (true, true));
    }
}
