use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Axis, Bounds, Context, Edges, FontWeight,
    MouseButton, Pixels, SharedString, Window, canvas, deferred, div, ease_out_quint,
    linear_color_stop, linear_gradient, prelude::*, px, rems,
};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::kbd::Kbd;
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Side, Sizable as _, h_flex,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::core::actions::{
    CloseActiveTab, CloseOtherTabs, CloseTabsToTheRight, CopyAgentSessionId, CopyWorkingDirectory,
    ForkAgentSession, HibernateTab, MarkTabUnread, NewWorktreeTab, RenameTab, SelectWorkspace1,
    SelectWorkspace2, SelectWorkspace3, SelectWorkspace4, SelectWorkspace5, SelectWorkspace6,
    SelectWorkspace7, SelectWorkspace8, SelectWorkspace9, SplitDown, SplitRight,
};
use crate::core::cli_agent::CLIAgent;
use crate::core::config::{Config, RightPanelTab};
use crate::core::shells::DetectedShell;
use crate::daemon::protocol::ShellSpec;
use crate::ui::app::{SpawnWhere, TILE_GLYPH, TILE_SIZE, Tab, Tty59App, tile_trailing_inset};
use crate::ui::hints::tab_badge_label;
use crate::ui::i18n::{L10nKey, t, t_fmt, t_plural};
use crate::ui::reorder::{self, Reorder, Surface};
use crate::ui::search::SearchTab;

/// One duration and one curve for every transition the app runs, so a fade and
/// a slide read as the same hand. Long enough to be seen as movement, short
/// enough that nobody waits on it.
pub(crate) const TRANSITION_MS: u64 = 140;
pub(crate) const REORDER_SLIDE_MS: u64 = TRANSITION_MS;
const CHIP_GAP: f32 = 6.;

/// The gap between the strip row's own children, as a share of the rem: what
/// `gap_1p5` has always been, 6px at the default size, and it grows with the
/// interface font. The row is drawn with it and budgeted with it.
const STRIP_GAP_REM: f32 = 0.375;

pub(crate) const GRAB_HANDLE_W: f32 = 80.;

const KEEP_SEGMENTS: usize = 3;

/// Builds a launch specification without recomputing argument ownership locally.
/// The inventory may originate from a remote host, so only its transported
/// metadata can distinguish tty59 launch defaults from user-authored arguments.
fn shell_spec(shell: &DetectedShell) -> ShellSpec {
    ShellSpec {
        program: shell.program.clone(),
        args: shell.args.clone(),
        args_are_tty59_defaults: shell.args_are_tty59_defaults,
    }
}

/// Shared with the switcher's tab column and the CLI, which name tabs of
/// workspaces this process does not own and have to cut the same head off the
/// same titles.
pub(crate) use tty59_core::core::tab_view::strip_host_prefix;

/// `home` is the home directory of the machine `path` is on, from the
/// workspace's host; `None` leaves the path spelled out (#580).
pub(crate) fn abbreviate_home<'a>(
    path: &'a str,
    home: Option<&std::path::Path>,
) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if path.starts_with('~') {
        return Cow::Borrowed(path);
    }
    // The shared comparison: separators normalized, case folded — a Windows
    // pane whose cwd spells itself `C:/Users/…` shortens under a
    // `C:\Users\…` home too (#544).
    crate::ui::path_display::abbreviate_home(path, home)
}

/// The separator a path spells itself with. A path carrying a single `\` is
/// a Windows path and has to be put back together with `\`: rejoining it with
/// `/` would make one tab spell its location two ways, `C:\Users\dev\app`
/// while it fits and `C:/…/app` once it has to be elided.
fn path_separator(path: &str) -> char {
    if path.contains('\\') { '\\' } else { '/' }
}

fn join_segments(segments: &[&str], sep: char) -> String {
    segments.join(sep.encode_utf8(&mut [0u8; 4]) as &str)
}

pub(crate) fn short_title(raw: &str, home: Option<&std::path::Path>) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let after_host = strip_host_prefix(raw);
    let after_host = after_host.trim();
    if after_host.is_empty() {
        return String::new();
    }
    let abbreviated = abbreviate_home(after_host, home);
    let path: &str = abbreviated.as_ref();

    enum Kind {
        Home,
        Absolute,
        Relative,
    }
    let (kind, body) = if let Some(rest) = path.strip_prefix("~/") {
        (Kind::Home, rest)
    } else if path == "~" {
        return "~".to_string();
    } else if let Some(rest) = path.strip_prefix('/') {
        (Kind::Absolute, rest)
    } else {
        (Kind::Relative, path)
    };

    // Both separators: Windows shells report `C:\Users\…` while git and the
    // terminal integration use `/`, and a path must be cut on either one.
    let segments: Vec<&str> = body.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return match kind {
            Kind::Home => "~",
            Kind::Absolute => "/",
            Kind::Relative => "",
        }
        .to_string();
    }

    let sep = path_separator(path);
    let depth = segments.len() + usize::from(matches!(kind, Kind::Home));
    let mut label = if depth > KEEP_SEGMENTS {
        let tail = &segments[segments.len() - KEEP_SEGMENTS..];
        format!("…{sep}{}", join_segments(tail, sep))
    } else {
        match kind {
            Kind::Home => format!("~{sep}{}", join_segments(&segments, sep)),
            Kind::Absolute => format!("/{}", join_segments(&segments, sep)),
            Kind::Relative => join_segments(&segments, sep),
        }
    };
    // Clamped on cluster boundaries, or a label ending in an emoji comes back
    // holding half of one.
    let cells = clusters(&label);
    if cells.len() > 40 {
        label = format!("{}…", cells[..40].concat());
    }
    label
}

/// The one place a tab gets its displayed name, whichever surface is asking.
///
/// `label()` ranks the evidence — a given name, then the title the pane is
/// showing, then an agent, then the working directory, then the process it is
/// running — and this renders whatever came back. Both callers arrive with a
/// [`TabView`](crate::ui::machine_mirror::TabView): the switcher reads one out
/// of the machine tree for a window it does not own, and the strip builds one
/// from its own live panes in
/// [`Tab::label_view`](crate::ui::app::Tab::label_view).
///
/// They used to rank their own evidence, and disagreed where it mattered most:
/// a pane with a working directory and no title — every non-PowerShell shell
/// tty59 ships integration for reports OSC 7 and no OSC 0 — was listed by the
/// switcher as `~/repo/tty59` and by the strip that owned it as "tty59", the
/// app's own name (#740).
pub(crate) fn label_of(
    view: &crate::ui::machine_mirror::TabView,
    index: usize,
    home: Option<&std::path::Path>,
) -> String {
    use crate::ui::machine_mirror::TabLabel;

    let unnamed = || {
        t_fmt(
            L10nKey::TabUnnamedShell,
            &[("n", &((index + 1).to_string()))],
        )
    };
    // A path can shorten away to nothing (a bare "user@host:"), and the process
    // name the tree carries ("zsh") is still worth more than a number.
    //
    // Through `stated_title` because a tab of *this* window has no process name
    // to offer: `Tab::label_view` fills that slot with the placeholder a pane
    // answers to before anything has spoken, and printing the app's own name
    // here is the one thing #740 exists to stop. Nothing to say falls to the
    // number, which is what the strip showed before it shared this renderer.
    let shortened = |raw: &str| match short_title(raw, home) {
        shortened if !shortened.trim().is_empty() => shortened,
        _ => match crate::terminal::view::stated_title(&view.title) {
            Some(title) => title.to_string(),
            None => unnamed(),
        },
    };
    match view.label() {
        TabLabel::Named(name) => name.to_string(),
        // Through `short_title` because a title is so often a path: the shell
        // integration writes `user@host:~/dir`, and a tab spelling that out in
        // full where the one beside it says "…/dir" would be the same
        // disagreement in a new place.
        TabLabel::Osc(title) => shortened(title),
        TabLabel::Agent(agent) => agent.display_name().to_string(),
        TabLabel::Cwd(cwd) => shortened(cwd),
        TabLabel::Process(title) => title.to_string(),
        TabLabel::Unknown => unnamed(),
    }
}

/// What a row can add on hover: the name behind the one [`label_of`] cut down,
/// or `None` when it cut nothing and the tooltip would only repeat the row.
///
/// The comparison has to happen on the *same* spelling, which is the whole
/// trick here. `label_of` abbreviates a path under the home before it elides
/// it, and this returns the abbreviated form too, so a raw `/Users/x/repo`
/// measured against a label of `~/repo` looks like a difference that isn't
/// one — and every tab named after a directory inside the home would hang a
/// tooltip saying exactly what it already says. Abbreviate first, compare
/// after.
fn tooltip_of(
    view: &crate::ui::machine_mirror::TabView,
    index: usize,
    home: Option<&std::path::Path>,
) -> Option<SharedString> {
    use crate::ui::machine_mirror::TabLabel;

    // The other rungs are never shortened: a given name and a process name are
    // printed whole, and an agent's is a word.
    let raw = match view.label() {
        TabLabel::Osc(title) => title,
        TabLabel::Cwd(cwd) => cwd,
        _ => return None,
    };
    let full = abbreviate_home(raw.trim(), home);
    if full.trim().is_empty() || full.as_ref() == label_of(view, index, home).as_str() {
        return None;
    }
    Some(SharedString::from(full.into_owned()))
}

/// Width of `text` shaped in `font` at `size`, in pixels.
///
/// The window's text system caches shaped runs, so measuring the same labels
/// across frames is cheap. The sidebar elides against real glyph widths
/// instead of guessing at character counts — that is the only way a mixed
/// CJK/Latin label can be squeezed without tearing mid-token.
pub(crate) fn measure_text(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
) -> f32 {
    text_system
        .shape_line(
            SharedString::from(text),
            px(size),
            &[gpui::TextRun {
                len: text.len(),
                font: font.clone(),
                color: gpui::Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width
        .as_f32()
}

/// Elides a path from the front when it cannot fit `max_width`, keeping the
/// root marker (drive letter, `~`, or the leading slash) and every trailing
/// segment that fits.
///
/// The tail is what a user identifies a tab by — the file or directory they
/// are actually working on — so it is never torn: whole segments drop off the
/// front first (a half-eaten directory name reads as noise), and when even
/// the last segment is too wide, only that segment is elided character by
/// character, still tail-first.
pub(crate) fn elide_path_keep_tail(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    path: &str,
    max_width: f32,
) -> SharedString {
    let path = path.trim();
    if path.is_empty() || measure_text(text_system, font, size, path) <= max_width {
        return SharedString::from(path);
    }
    let sep = path_separator(path);
    let segments: Vec<&str> = path.split(['/', '\\']).collect();
    // A leading slash splits into an empty first segment; `~` and drive
    // letters (`E:`) carry the same "where this tree lives" weight, and a
    // leading `…` means `short_title` already elided once — that marker is
    // replaced by the new elision instead of stacking two ellipses. Keep
    // whichever marker there is so the result never reads as a bare
    // relative path.
    let root: &str = match segments.first() {
        Some(&"") => "/",
        Some(&"~") => "~",
        Some(&"…") => "",
        Some(head) if head.ends_with(':') => head,
        _ => "",
    };
    let root_kept = segments
        .first()
        .is_some_and(|s| s.is_empty() || *s == "~" || *s == "…" || s.ends_with(':'));
    let prefix = if root.is_empty() {
        format!("…{sep}")
    } else if root == "/" {
        // The absolute-path root is already the slash itself.
        format!("/…{sep}")
    } else {
        format!("{root}{sep}…{sep}")
    };
    // Drop whole segments from the front until the remaining tail fits. The
    // width only shrinks as segments leave, so the first fit is the widest
    // one — greedy is optimal here.
    //
    // With a root marker, `head = 1` would spell the root, the ellipsis, and
    // then every remaining segment — strictly wider than the original that
    // already failed to fit — so that candidate is skipped rather than
    // measured.
    let mut head = if root_kept { 2 } else { 0 };
    while head < segments.len() {
        let candidate = if head == 0 {
            join_segments(&segments, sep)
        } else {
            format!("{prefix}{}", join_segments(&segments[head..], sep))
        };
        if measure_text(text_system, font, size, &candidate) <= max_width {
            return SharedString::from(candidate);
        }
        if head + 1 >= segments.len() {
            break;
        }
        head += 1;
    }
    // Even the last segment alone is too wide: keep its tail after the
    // ellipsis, with no slash so the reader sees the segment was torn.
    elide_tail_clusters(
        text_system,
        font,
        size,
        segments[segments.len() - 1],
        max_width,
    )
}

/// Characters a token is allowed to break on. Space is in the set because
/// this also elides labels a human typed — `Backend server logs` — not just
/// branch names, and a word boundary is the cut a reader forgives.
const TOKEN_BREAKS: [char; 5] = ['-', '_', '/', '.', ' '];

/// Splits `text` into grapheme clusters — what a reader counts as one
/// character, and the only place a label may be cut.
///
/// Slicing by `char` passes every width check and still tears the result:
/// `👨‍👩‍👧` loses the joiner holding it together, `❤️` loses the variation
/// selector that makes it an emoji (and the orphan then attaches itself to the
/// ellipsis), and `🇨🇳` leaves behind a lone regional indicator that renders
/// as a bare letter.
fn clusters(text: &str) -> Vec<&str> {
    text.graphemes(true).collect()
}

/// The head this token would rather keep: six clusters, extended to just past
/// the next break so the cut lands on a boundary (`window-…` rather than
/// `window…`). When no break is within reach the plain six is kept — running
/// on to the cap would spend the whole budget on a prefix and leave the tail,
/// which is what identifies the token, with nothing.
fn preferred_head(clusters: &[&str]) -> usize {
    let base = clusters.len().min(6);
    let cap = clusters.len().min(12);
    if base >= cap {
        return base;
    }
    match clusters[base..cap]
        .iter()
        .position(|c| c.chars().next().is_some_and(|c| TOKEN_BREAKS.contains(&c)))
    {
        Some(offset) => base + offset + 1,
        None => base,
    }
}

/// Elides the middle of a single token (a branch name, a shell name, a label
/// the user typed) so both its head and its identifying tail survive:
/// `window-transparency-backdrop` reads `window-…backdrop` in a narrow
/// sidebar instead of losing its tail to a trailing ellipsis.
///
/// A head that fits but leaves no room for a tail is worse than no head at
/// all, so the preferred head is given up for a shorter one when that is what
/// it takes to keep a few trailing clusters; only when even a three-cluster
/// head cannot buy a tail does this fall back to a tail-only elision.
pub(crate) fn elide_keep_edges(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    let text = text.trim();
    if text.is_empty() || measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text);
    }
    let cells = clusters(text);
    let shaped = |head_n: usize, tail_n: usize| -> f32 {
        let mut s = cells[..head_n].concat();
        s.push('…');
        s.push_str(&cells[cells.len() - tail_n..].concat());
        measure_text(text_system, font, size, &s)
    };
    // Width is monotone in the tail length, so a binary search finds the
    // longest tail that still fits behind a given head.
    let longest_tail = |head_n: usize| -> usize {
        let (mut lo, mut hi) = (0usize, cells.len() - head_n);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if shaped(head_n, mid) <= max_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    };
    /// Enough trailing clusters to tell two sibling branches apart.
    const MIN_TAIL: usize = 3;
    let preferred = preferred_head(&cells);
    let mut candidates = vec![preferred, 6, 3];
    candidates.retain(|&h| h > 0 && h <= cells.len());
    candidates.dedup();
    let mut best: Option<(usize, usize)> = None;
    for head_n in candidates {
        if shaped(head_n, 0) > max_width {
            continue;
        }
        let tail = longest_tail(head_n);
        if tail >= MIN_TAIL.min(cells.len() - head_n) {
            best = Some((head_n, tail));
            break;
        }
        if best.is_none_or(|(_, best_tail)| tail > best_tail) {
            best = Some((head_n, tail));
        }
    }
    let Some((head, tail)) = best.filter(|&(_, tail)| tail > 0) else {
        // No head buys a tail worth showing; a bare tail says more.
        return elide_tail_clusters(text_system, font, size, text, max_width);
    };
    let mut out = cells[..head].concat();
    out.push('…');
    out.push_str(&cells[cells.len() - tail..].concat());
    SharedString::from(out)
}

/// Elides a row label. A path keeps its tail — the file or directory being
/// worked on — while anything else keeps both edges.
///
/// A shell title is not always a path: `npm run dev`, `man git-log`, or a name
/// the user typed into the rename box. Running those through the path rule
/// drops their head, which is the part that names them, and `… server logs`
/// says less than the CSS truncation this replaced.
pub(crate) fn elide_label(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    if text.contains('/') || text.contains('\\') {
        elide_path_keep_tail(text_system, font, size, text, max_width)
    } else {
        elide_keep_edges(text_system, font, size, text, max_width)
    }
}

/// Keeps the longest head of `text` that fits before a trailing ellipsis —
/// the plain cut a reader expects, for a string read left to right like a
/// branch name.
pub(crate) fn elide_end_clusters(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    if measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text.to_string());
    }
    let budget = max_width - measure_text(text_system, font, size, "…");
    if budget <= 0. {
        return SharedString::from("…");
    }
    let cells = clusters(text);
    let (mut lo, mut hi) = (0usize, cells.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if measure_text(text_system, font, size, &cells[..mid].concat()) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out = cells[..lo].concat();
    out.push('…');
    SharedString::from(out)
}

/// Keeps the longest tail of `text` that fits after a bare ellipsis. Shared
/// by the path and token elisions as their last resort.
pub(crate) fn elide_tail_clusters(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    // Nothing to cut. The search below is bounded by the budget *after* the
    // ellipsis, so without this a branch that fit came back as "…" plus the
    // whole of itself — the rail's group header printed `…feat/v4-redesign`
    // with room to spare.
    if measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text.to_string());
    }
    let budget = max_width - measure_text(text_system, font, size, "…");
    if budget <= 0. {
        return SharedString::from("…");
    }
    let cells = clusters(text);
    let (mut lo, mut hi) = (0usize, cells.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s = cells[cells.len() - mid..].concat();
        if measure_text(text_system, font, size, &s) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return SharedString::from("…");
    }
    let mut out = String::with_capacity(1 + text.len());
    out.push('…');
    out.push_str(&cells[cells.len() - lo..].concat());
    SharedString::from(out)
}

#[derive(Clone)]
pub(crate) struct DragTab;

impl Render for DragTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What a chrome tile says on hover: what it does, then the chord that does it.
/// The tile's own name is no use as a tooltip — the workspace head already
/// wears it as its label.
///
/// This used to be `chord_hint`, which pasted the two together into one string
/// — `"Hide sidebar  \u{2318}B"` — and handed that to `Button::tooltip`. Inside the
/// card the chord then wore the label's own size and colour, so the tooltip
/// read as one odd sentence rather than as a name with a shortcut beside it.
/// `Tooltip` has a `key_binding` slot that already renders a chord the way a
/// chord should look — set apart on the right, a size down, in
/// `muted_foreground` — and all that was missing was a way to hand `Button` a
/// built tooltip instead of a string, which is what `tooltip_element` is for.
pub(crate) fn chord_tooltip(
    what: impl Into<SharedString>,
    action: &str,
    cx: &gpui::App,
) -> impl Fn(&mut Window, &mut App) -> gpui::Entity<Tooltip> + 'static {
    let what: SharedString = what.into();
    // Resolved now, while there is a `cx`: the builder below runs on hover, and
    // is handed only the window it is drawing into.
    let kbd = crate::ui::home::key_stroke(action, cx).map(Kbd::new);
    move |_window, cx| cx.new(|_| Tooltip::new(what.clone()).key_binding(kbd.clone()))
}

pub(crate) fn chrome_tile_variant(cx: &gpui::App) -> ButtonCustomVariant {
    chrome_tile_variant_for(false, cx)
}

pub(crate) fn chrome_tile_variant_for(selected: bool, cx: &gpui::App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(cx.theme().transparent)
        // Resting chrome is *not* body ink. `sidebar_foreground` is the rung a
        // tab title is written at, so a toolbar drawn in it made the two
        // controls at the top of the rail the darkest marks in the whole
        // sidebar — louder than the twenty rows they exist to act on, which is
        // the opposite of how a native sidebar ranks itself. One rung down puts
        // them level with the workspace chip beside them, and the hover fill
        // this variant already carries is what answers the pointer.
        .foreground(if selected {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        // `sidebar_accent` is the surface's *selected* step, and it was handed
        // to hover as well — so a hovered tile wore the fill of a selected one
        // and, with the right panel open, two tiles read as current at once.
        // Hover takes the step the palette derives for it. (A selected button
        // never renders the hover style, so this only reaches the rest.)
        .hover(gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().sidebar.hover).into())
        .active(cx.theme().sidebar_accent)
}

pub(crate) const BUTTON_ICON_SCALE: f32 = 0.75;

/// Compact tiles for document headers and overlays: 26px boxes with a 6px
/// corner and a glyph a notch under the toolbar's. Sidebar navigation uses
/// the full title-bar size so opening a panel does not resize its controls.
pub(crate) const RAIL_TILE: f32 = 26.;

/// A sidebar row's trailing status dot, and the box it is centred in (wide
/// enough for the unread pill that replaces it).
pub(crate) const ROW_STATUS_DOT: f32 = 5.;
pub(crate) const ROW_STATUS_SLOT: f32 = 18.;
pub(crate) const RAIL_TILE_GLYPH: f32 = 14.;
pub(crate) const RAIL_TILE_RADIUS: f32 = 6.;

/// WCAG 2.2 SC 2.5.8 puts the desktop floor for a pointer target at 24×24, and
/// gpui-component renders an icon-only `.xsmall()` button as a 20×20 box (18×18
/// where the chrome overrode it). Grow only the box: the glyph keeps its size,
/// so the chrome looks unchanged and simply stops being fiddly to hit.
pub(crate) const MIN_TARGET: f32 = 24.;

pub(crate) fn hit_target(button: Button) -> Button {
    button.w(px(MIN_TARGET)).h(px(MIN_TARGET))
}

/// The narrowest a chip gets: its `min_w`, which flex-shrink cannot go under.
const CHIP_MIN_W: f32 = 100.;

/// How wide the tab strip is drawn: the content width less what the row
/// reserves beside it (the lead, plus the window buttons or a docked column),
/// never below `floor`.
///
/// Under Linux client-side decorations `viewport_w` measures the whole surface,
/// shadow included, while the title bar row sits inside the frame padding —
/// so both paddings come off before anything else does. Sizing the strip to
/// the raw viewport overran the frame by exactly that much and clipped the
/// close button (#1169). `window_paddings` answers `Edges::all(0)` under
/// server-side decorations and on a maximized or tiled CSD window, so
/// everywhere else this is the width the strip always had.
fn strip_width(viewport_w: Pixels, pad: Edges<Pixels>, reserved: f32, floor: f32) -> Pixels {
    (viewport_w - pad.left - pad.right - px(reserved)).max(px(floor))
}

/// How many chips `avail` points hold at their narrowest. At least one, so the
/// active tab always has a place.
fn chips_fit(avail: f32) -> usize {
    ((avail / (CHIP_MIN_W + CHIP_GAP)).floor() as usize).max(1)
}

/// The hidden-tab count, `+14`: a 28px tile, its type size (a share of the
/// rem), the slot every drawn glyph sits in, and the inset kept on each side
/// of the slots.
///
/// Each glyph, the `+` and every digit, is centred in a slot of exactly
/// [`count_slot`], and the tile is as wide as its slots plus the inset, never
/// narrower than [`COUNT_MIN_W`]. The budget that reserves the tile and the
/// boxes that are drawn therefore come from the same number, and nothing is
/// shaped or measured while the strip is laid out.
const COUNT_H: f32 = 28.;
const COUNT_MIN_W: f32 = 28.;
const COUNT_TEXT: f32 = 12. / 16.;
const COUNT_SLOT_EM: f32 = 0.75;
const COUNT_INSET: f32 = 2.;

fn count_digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

/// The room one glyph of the count takes at `rem` pixels.
fn count_slot(rem: f32) -> f32 {
    COUNT_SLOT_EM * COUNT_TEXT * rem
}

/// Width of the count tile for a count of `digits` digits at `rem` pixels:
/// one slot for the `+` and one for each digit, widening the tile only once
/// they outgrow [`COUNT_MIN_W`].
fn count_w(digits: usize, rem: f32) -> f32 {
    (2. * COUNT_INSET + (1 + digits) as f32 * count_slot(rem)).max(COUNT_MIN_W)
}

const COUNT_DIGIT_GLYPHS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The Agents pill at the end of the strip: its text size (a share of the rem,
/// what the pill has always been drawn at) and the padding either side of its
/// label. The label is drawn as our own child at exactly this size and with
/// [`StripMetrics`]' features, so what is measured is what is on screen.
pub(crate) const PILL_TEXT: f32 = 0.875;
pub(crate) const PILL_PAD_X: f32 = 10.;

/// What the Agents pill's label measures at the current font, size and
/// language: its fixed words, and the width of one digit. The strip budgets the
/// pill from these without shaping any text while it is laid out; drawing only
/// reads them, and they are measured again when the font, the size or the
/// language they are for has changed.
#[derive(Clone)]
pub(crate) struct StripMetrics {
    rem: f32,
    family: SharedString,
    agents: &'static str,
    workbench: &'static str,
    /// The tabular figures the label is drawn with, built once: the measured
    /// font, the pill and the count tile all take their features from here.
    features: gpui::FontFeatures,
    /// "Agents ()" — the label less its digits.
    agents_fixed: f32,
    /// One tabular digit, which is every digit's width.
    digit: f32,
    workbench_w: f32,
}

impl StripMetrics {
    /// Measures now and keeps itself current: the configured size and the
    /// theme's font are what a change can come through, and a language change
    /// reaches the app as a notify. Each of them only compares what was
    /// measured against what is now, and measures again when they differ.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Tty59App>) -> Self {
        cx.observe_in(&cx.entity(), window, |this, _, window, cx| {
            this.strip_metrics_refresh(window, cx)
        })
        .detach();
        cx.observe_global_in::<Config>(window, |this, window, cx| {
            this.strip_metrics_refresh(window, cx)
        })
        .detach();
        cx.observe_global_in::<gpui_component::Theme>(window, |this, window, cx| {
            this.strip_metrics_refresh(window, cx)
        })
        .detach();
        Self::measure(window, cx)
    }

    fn measure(window: &Window, cx: &App) -> Self {
        let rem = cx.global::<Config>().ui_font_size;
        let features = crate::ui::theme::tabular_figures();
        let font = gpui::Font {
            family: cx.theme().font_family.clone(),
            features: features.clone(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
        };
        let size = rem * PILL_TEXT;
        let width = |text: &str| measure_text(window.text_system(), &font, size, text).ceil();
        let (agents, workbench) = (t(L10nKey::VisionAgents), t(L10nKey::VisionWorkbench));
        Self {
            rem,
            family: font.family.clone(),
            agents,
            workbench,
            features,
            agents_fixed: width(&format!("{agents} ()")),
            digit: width("0"),
            workbench_w: width(workbench),
        }
    }

    fn is_for(&self, cx: &App) -> bool {
        self.rem == cx.global::<Config>().ui_font_size
            && self.family == cx.theme().font_family
            && self.agents == t(L10nKey::VisionAgents)
            && self.workbench == t(L10nKey::VisionWorkbench)
    }

    pub(crate) fn features(&self) -> gpui::FontFeatures {
        self.features.clone()
    }

    /// The pill's width with `count` agents needing attention (`Agents (3)`),
    /// or with the Workbench label while the agent view is up.
    pub(crate) fn pill_w(&self, workbench: bool, count: usize) -> f32 {
        2. * PILL_PAD_X
            + match workbench {
                true => self.workbench_w,
                false => self.agents_fixed + count_digits(count) as f32 * self.digit,
            }
    }
}

impl Tty59App {
    fn strip_metrics_refresh(&mut self, window: &Window, cx: &App) {
        if !self.strip_metrics.is_for(cx) {
            self.strip_metrics = StripMetrics::measure(window, cx);
        }
    }
}

/// What the count tile takes out of the chips' `avail` when `total` chips do
/// not all fit: the tile and the strip row's gap before it, or nothing when
/// they fit.
///
/// The count's width depends on its digits and the digits on how many chips
/// the rest of the room holds, so settle on the fewest digits it can have:
/// start from one and widen while the count it leaves needs more. Widening
/// only ever takes chips away, so the digits it stops at are exactly the ones
/// the count that is drawn has, and more room never hides more tabs.
fn count_reserve(avail: f32, total: usize, rem: f32) -> f32 {
    if total <= chips_fit(avail) {
        return 0.;
    }
    let reserve = |digits| STRIP_GAP_REM * rem + count_w(digits, rem);
    let mut digits = 1;
    loop {
        let kept = chips_fit(avail - reserve(digits)).min(total - 1);
        let needed = count_digits(total - kept);
        if needed <= digits {
            return reserve(digits);
        }
        digits = needed;
    }
}

/// The run of chips to draw when they cannot all fit.
///
/// The row clips what overflows, so past a certain tab count the chips at the
/// end simply were not drawn — including, right after ⌘T, the tab that was
/// just opened and made active. Slide the run instead: keep it anchored at the
/// first tab until the active one would fall off the right edge, then move it
/// by as little as it takes to hold the active chip.
fn visible_chips(order: &[usize], active: usize, avail: f32) -> Vec<usize> {
    let fits = chips_fit(avail);
    if order.len() <= fits {
        return order.to_vec();
    }
    let at = order.iter().position(|&i| i == active).unwrap_or(0);
    let start = at.saturating_sub(fits - 1).min(order.len() - fits);
    order[start..start + fits].to_vec()
}

pub(crate) fn chrome_tile(button: Button, selected: bool, cx: &gpui::App) -> Button {
    chrome_tile_sized(button, TILE_SIZE, TILE_GLYPH, selected, cx)
}

/// The right panel's tabs, left to right, by name. Info leads as the default
/// and the pane's overview; then the project's files, and its version
/// control, local to remote (Changes, GitHub). GitHub is last so that hiding
/// it for a repository without a GitHub remote moves nothing. Search is not
/// among them: the Files tab searches names and contents in one field.
const RIGHT_PANEL_TABS: [(RightPanelTab, L10nKey); 4] = [
    (RightPanelTab::Info, L10nKey::PanelInfoTitle),
    (RightPanelTab::Files, L10nKey::PanelFilesTitle),
    (RightPanelTab::Scm, L10nKey::PanelChangesTitle),
    (RightPanelTab::GitHub, L10nKey::PanelGitHubTitle),
];

fn right_panel_tab_size(window: &Window) -> f32 {
    window.rem_size().as_f32() * crate::ui::right_panel::TAB_TEXT
}

fn right_panel_tab_font(cx: &gpui::App) -> gpui::Font {
    gpui::Font {
        family: cx.theme().font_family.clone(),
        features: Default::default(),
        fallbacks: None,
        // The current tab's weight, which is the widest any label is drawn at.
        weight: FontWeight::MEDIUM,
        style: Default::default(),
    }
}

/// What the bare tab labels take, padding included. The panel's floor is
/// built on it, so the chrome tiles beside them always fit.
pub(crate) fn right_panel_tab_labels_w(window: &Window, cx: &gpui::App) -> f32 {
    let size = right_panel_tab_size(window);
    let font = right_panel_tab_font(cx);
    RIGHT_PANEL_TABS
        .iter()
        .map(|(_, key)| {
            measure_text(window.text_system(), &font, size, t(*key))
                + 2. * (TAB_OUTER_PAD + TAB_INNER_PAD)
        })
        .sum()
}

/// Right panel tab geometry: each tab's click target reaches this far past
/// its pill, so neighbouring pills sit twice this apart.
pub(crate) const TAB_OUTER_PAD: f32 = 2.;
/// A label's hover target reaches this far past its word.
const TAB_INNER_PAD: f32 = 4.;

/// The panel toggle and its trailing inset. The open panel uses this same
/// reservation so the title bar never budgets for a second, absent control.
pub(crate) fn trailing_chrome_tiles_w() -> f32 {
    let trailing_pad = if cfg!(target_os = "macos") {
        tile_trailing_inset()
    } else {
        4.
    };
    trailing_pad + crate::ui::app::TILE_SIZE
}

/// The whole trailing cluster: those tiles and the OS window buttons beyond
/// them.
///
/// Anything else drawn into that end of the title bar has to stop short of it —
/// which for the hoisted document header means the case where the detail panel
/// is closed and the document column runs to the window's right edge.
pub(crate) fn trailing_chrome_w(fullscreen: bool) -> f32 {
    trailing_chrome_tiles_w() + crate::ui::app::window_controls_w(fullscreen)
}

pub(crate) fn chrome_tile_sized(
    button: Button,
    tile: f32,
    glyph: f32,
    selected: bool,
    cx: &gpui::App,
) -> Button {
    button
        .custom(chrome_tile_variant_for(selected, cx))
        .selected(selected)
        .with_size(px(glyph / BUTTON_ICON_SCALE))
        .w(px(tile))
        .h(px(tile))
}

/// How many saved hosts the New Tab menu names.
///
/// Sorted by frecency, so the ones actually used are the ones that fit. A menu
/// is not a search field — past a handful the list stops being scannable, and
/// the search's Hosts tab already lists every host and can filter. The row that
/// closes the section is where the rest are.
const MENU_HOSTS: usize = 6;

/// How many shells the New Tab menu names, the default among them.
///
/// The same reasoning as [`MENU_HOSTS`], at a smaller number: a stock macOS box
/// reports nine shells and almost nobody opens more than one or two of them.
/// The default always leads — it is the answer to "what do I get if I just
/// click" — and the rest of the rows go to whatever has actually been opened,
/// by frecency. Everything else is one row away, in the search.
const MENU_SHELLS: usize = 3;

/// How many agents the New Tab menu names.
///
/// The same cap as [`MENU_SHELLS`], and for the same reason: the agents
/// someone actually runs are two or three, and a menu is not a search field.
/// Only agents that have been run are named — there is no "default agent" to
/// lead the way shells' default does. Everything else is one row away.
const MENU_AGENTS: usize = 3;

/// The Search Everywhere button in the middle of the title bar: the width of
/// a field that reads as one, and the 28px of the rail's own search.
const TITLEBAR_SEARCH_W: f32 = 360.;
pub(in crate::ui) const TITLEBAR_SEARCH_H: f32 = 28.;
pub(in crate::ui) const TITLEBAR_SEARCH_RADIUS: f32 = 7.;

pub(in crate::ui) fn titlebar_search_fill(cx: &App) -> gpui::Hsla {
    gpui::rgb(cx.global::<crate::ui::presets::Surfaces>().rail.base).into()
}

fn unread_count_label(unread: usize) -> Option<String> {
    (unread > 1).then(|| unread.min(9).to_string())
}

/// Air between the search box and the nearest chrome tile when the bar is too
/// narrow for the box to sit at its full width.
const TITLEBAR_SEARCH_CLEAR_GAP: f32 = 8.;

/// Where the title-bar search box is centred, as offsets from the tab strip it
/// is drawn inside.
///
/// The strip does not start at the terminal column's left edge. The title bar
/// leaves a lead before it — `TRAFFIC_LIGHTS_LEAD` on macOS, for the traffic lights, whether or
/// not the rail is standing in front of them, plus the bar's own inset in
/// fullscreen; 12 elsewhere — and a box centred on the strip was centred that
/// much too far right by half: 40pt on macOS then, which is plainly visible with
/// the rail collapsed and the box standing alone in an empty bar (#1033).
/// So the band reaches back over the lead, and its left edge is the column's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SearchBand {
    /// `left` for the band, against the strip's left edge: the lead, negated.
    pub(crate) left: f32,
    /// `right` for the band, against the strip's right edge.
    pub(crate) right: f32,
    /// How much of *each* end of the band the box keeps clear of, so that in
    /// a narrow bar it shrinks rather than slides under the chrome tiles. The
    /// same at both ends, because a box clear of only one is no longer
    /// centred.
    pub(crate) clear: f32,
}

/// [`SearchBand`] for a strip that starts `lead` in from the terminal column's
/// left edge.
///
/// `docked_right` is what is docked to the right of the terminal — the detail
/// panel and a document column. Off macOS the bar spans those too, so the band
/// stops at the terminal column: the strip already stops short of the window
/// controls, so they come off the columns' share. On macOS the bar is inside
/// the terminal column already and the strip runs to its end.
///
/// `leading_w` is the collapsed rail's group at the strip's left end, measured
/// from the strip's left edge; `trailing_w` the chrome tiles at the band's
/// right end, when they are there rather than off over a docked column.
pub(crate) fn search_band(
    macos: bool,
    lead: f32,
    docked_right: f32,
    controls_w: f32,
    leading_w: Option<f32>,
    trailing_w: Option<f32>,
) -> SearchBand {
    let right = match !macos && docked_right > 0. {
        true => (docked_right - controls_w).max(0.),
        false => 0.,
    };
    let leading = leading_w.map_or(0., |w| lead + w + TITLEBAR_SEARCH_CLEAR_GAP);
    let trailing = trailing_w.map_or(0., |w| w + TITLEBAR_SEARCH_CLEAR_GAP);
    SearchBand {
        left: -lead,
        right,
        clear: leading.max(trailing),
    }
}

/// How far the title bar's lead reaches before the tab strip starts: the room
/// we have `TitleBar` leave for the traffic lights on macOS, and the bar's own inset
/// that gpui-component adds to it in fullscreen, where the lights are gone but
/// the room is not; `TITLE_BAR_LEAD` elsewhere, in both of the rows the strip
/// can sit in.
fn title_bar_strip_lead(fullscreen: bool, rem: f32) -> f32 {
    match cfg!(target_os = "macos") && fullscreen {
        // `pl_3` on the bar inside `TitleBar`.
        true => crate::ui::app::TITLE_BAR_LEAD + 0.75 * rem,
        false => crate::ui::app::TITLE_BAR_LEAD,
    }
}

/// How wide the collapsed rail's group at the start of the strip is: the
/// window mark where there is one, then New Tab and Show Sidebar. Mirrors the
/// `left_group` built in `tab_strip`, `ml` included.
fn collapsed_rail_group_w() -> f32 {
    use crate::ui::app::{CONTENT_INSET, TILE_SIZE, WINDOW_MARK_SIZE, tile_trailing_inset};
    let mark = match cfg!(target_os = "macos") {
        true => 0.,
        false => (CONTENT_INSET - tile_trailing_inset()) + WINDOW_MARK_SIZE + 4. + 2.,
    };
    crate::ui::app::title_bar_hug_offset() + mark + TILE_SIZE + 2. + TILE_SIZE
}

/// How wide the New Tab menu is allowed to get.
const MENU_W: Pixels = px(360.);

/// How tall, before it starts scrolling.
///
/// The lists are capped — [`MENU_SHELLS`] shells, [`MENU_AGENTS`] agents and
/// [`MENU_HOSTS`] hosts — so the menu's full hand is a fixed number of rows:
/// those, the three headings, the row closing each section, the separators and
/// the modifier hint, at the 26px a row occupies. The Agents section is absent
/// when nothing is offered, and this still leaves room above the full hand for
/// a seam row or two more, so it always arrives whole and never scrolls on its
/// own. It is capped again against the window in [`NewTabMenu::build`], since
/// a menu taller than what it hangs off is worse than one that scrolls.
const MENU_H: Pixels = px(720.);

/// What the row closing the Local section types into the search's Terminals
/// tab for you.
///
/// This is where filtering lives, and the reason the menu does not do any.
/// A search field inside a [`PopupMenu`] is not possible — the menu holds the
/// keyboard for its own navigation — and the branch that tried it had to
/// become a popover carrying the palette's own list, which read as far too
/// heavy hanging off a button in the chrome. The menu names the few worth
/// naming; the search, which already filters better than a menu could, holds
/// the rest. The rows closing each section are the seam between the two, and
/// they only work if they land *already filtered*: a row that says "other
/// shells" and opens every terminal has made the reader ask twice.
///
/// The Terminals tab holds open tabs as well as ways to open one, so the
/// shells are narrowed to by word: every one is titled `Shell: {label}`
/// ([`L10nKey::AppCmdShellTitle`], the same word in every language we ship),
/// so this lands on exactly the shells, default first and then by frecency.
const SEARCH_SHELL_QUERY: &str = "shell";

/// What the row closing the Agents section types into the search's Terminals
/// tab.
///
/// The same seam as [`SEARCH_SHELL_QUERY`]. Every quick-launch row is titled
/// `Agent: {name}` ([`L10nKey::AppCmdAgentLaunchTitle`], the same word in every
/// language), so this lands on exactly the agents.
const SEARCH_AGENT_QUERY: &str = "agent";

/// How this platform spells the key that turns a New Tab row into a split.
fn split_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    }
}

/// What the New Tab menu offers, read off the app when the menu is opened.
///
/// The builder runs on the popup's own entity, so what a row needs to name
/// itself is taken here and carried in. Taken on open rather than on render:
/// the strip redraws on every frame a terminal paints, and the host list is
/// worth sorting once per menu, not once per frame — and a host saved while
/// the window sat still is in the list the next time the `+` is pressed.
struct NewTabMenu {
    app: gpui::WeakEntity<Tty59App>,
    /// The shells the menu names, by label — at most [`MENU_SHELLS`].
    shells: Vec<SharedString>,
    default_shell: SharedString,
    /// The inventory holds shells the menu does not name, so the section
    /// closes with a row into the palette.
    more_shells: bool,
    /// Saved host, its display name, and the `user@host:port` beside it —
    /// empty when the name already says it.
    hosts: Vec<(uuid::Uuid, SharedString, SharedString)>,
    /// Agents the menu names — at most [`MENU_AGENTS`]: the ones actually
    /// run, or the first offered while nothing has run yet.
    agents: Vec<CLIAgent>,
    /// The window can offer agents the menu does not name, so the section
    /// closes with a row into the palette.
    more_agents: bool,
}

impl NewTabMenu {
    fn build(&self, menu: PopupMenu, window: &Window) -> PopupMenu {
        // Whatever [`MENU_H`] asks for, a menu still has to fit the window it
        // hangs off — on a short one the list gives way, not the window.
        //
        // Measured off the viewport, not `window_bounds()`: that one answers
        // "how should this window be reopened after it is closed", so on macOS
        // a fullscreen window reports the small bounds it would restore to,
        // not the screen it currently fills. A terminal spends much of its
        // life fullscreen, and reading that would cap the menu at 80% of a
        // window nobody is looking at — putting back the scrollbar and the
        // cut-off `Local` this whole change is here to remove.
        let ceiling = MENU_H.min(window.viewport_size().height * 0.8);
        let mut menu = menu
            .min_w(px(240.))
            // A menu is a list of names, not a place to read a full address.
            // Left to itself the panel widens to its longest row — one saved
            // host with a descriptive name and a long `user@host` drags every
            // other row out with it and the menu stops looking like chrome.
            .max_w(MENU_W)
            // Both lists are capped, but a short window is not. Past the height
            // of the window an un-scrollable menu simply loses its last rows —
            // and the last rows here are the SSH section.
            .scrollable(true)
            // Our own ceiling rather than `PopupMenu`'s built-in 450px, so the
            // one that applies is the one [`MENU_H`] reasons about.
            .max_h(ceiling)
            .item(PopupMenuItem::label(t(L10nKey::TabMenuLocalShells)));
        for label in &self.shells {
            let app = self.app.clone();
            let open = label.clone();
            let row = if *label == self.default_shell {
                let label = label.clone();
                PopupMenuItem::element(move |_window, cx| {
                    menu_row(label.clone(), t(L10nKey::ShellDefault).into(), cx)
                })
            } else {
                PopupMenuItem::new(label.clone())
            };
            menu = menu.item(row.on_click(move |_, window, cx| {
                let at = SpawnWhere::from_modifiers(window.modifiers());
                if let Some(app) = app.upgrade() {
                    app.update(cx, |this, cx| this.open_listed_shell(&open, at, window, cx));
                }
            }));
        }
        // No inventory yet — the machine has not answered, or this is a host
        // that reports none. The default shell is still openable.
        if self.shells.is_empty() {
            let app = self.app.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::AppMenuNewTab)).on_click(
                move |_, window, cx| {
                    let at = SpawnWhere::from_modifiers(window.modifiers());
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| this.open_shell(None, at, window, cx));
                    }
                },
            ));
        }
        // The rest of the inventory is in the search, already filtered to
        // it — the same seam the SSH section closes with. Absent when the
        // rows above are the whole inventory: a row into a list of nothing
        // new would be one more thing to read for no gain.
        if self.more_shells {
            let app = self.app.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::TabMenuOtherShells)).on_click(
                move |_, window, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| {
                            this.open_search(SearchTab::Terminals, SEARCH_SHELL_QUERY, window, cx)
                        });
                    }
                },
            ));
        }

        menu = menu
            .item(PopupMenuItem::separator())
            .item(PopupMenuItem::label(t(L10nKey::CmdGroupSsh)));
        for (id, name, endpoint) in &self.hosts {
            let (id, name, endpoint) = (*id, name.clone(), endpoint.clone());
            let app = self.app.clone();
            // Every host row is a custom element, note or not: a plain item
            // renders its label as bare text with nothing to elide against,
            // and a host saved on its address alone is *named* `user@host:port`
            // — the longest string in the menu, on the row least able to cut
            // it. [`menu_row`] drops the right half when there is no note.
            let row = PopupMenuItem::element(move |_window, cx| {
                menu_row(name.clone(), endpoint.clone(), cx)
            });
            menu = menu.item(row.on_click(move |_, window, cx| {
                let at = SpawnWhere::from_modifiers(window.modifiers());
                if let Some(app) = app.upgrade() {
                    app.update(cx, |this, cx| {
                        this.connect_ssh_profile_at(id, at, window, cx)
                    });
                }
            }));
        }
        // With no hosts saved, the row that closes the section is the one that
        // gets you your first — the list of hosts is not somewhere to send
        // someone who has none.
        let app = self.app.clone();
        let empty = self.hosts.is_empty();
        let last = if empty {
            L10nKey::TabMenuAddHost
        } else {
            L10nKey::TabMenuAllHosts
        };
        menu = menu.item(PopupMenuItem::new(t(last)).on_click(move |_, window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |this, cx| {
                    if empty {
                        this.open_new_ssh_host(window, cx);
                    } else {
                        this.open_search(SearchTab::Hosts, "", window, cx);
                    }
                });
            }
        }));

        // A peer of Local and SSH, and absent when this machine has no agent
        // to offer — an empty heading would be a section about nothing. The
        // rows are the ones actually run — or the first few offered, while
        // nothing has run yet; the seam holds the rest, already filtered,
        // the way Other Shells does.
        if !self.agents.is_empty() || self.more_agents {
            menu = menu
                .item(PopupMenuItem::separator())
                .item(PopupMenuItem::label(t(L10nKey::CmdGroupAgents)));
            for agent in self.agents.iter().copied() {
                let app = self.app.clone();
                menu = menu.item(PopupMenuItem::new(agent.display_name()).on_click(
                    move |_, window, cx| {
                        let at = SpawnWhere::from_modifiers(window.modifiers());
                        if let Some(app) = app.upgrade() {
                            app.update(cx, |this, cx| this.launch_agent(agent, at, window, cx));
                        }
                    },
                ));
            }
            if self.more_agents {
                let app = self.app.clone();
                menu = menu.item(PopupMenuItem::new(t(L10nKey::TabMenuOtherAgents)).on_click(
                    move |_, window, cx| {
                        if let Some(app) = app.upgrade() {
                            app.update(cx, |this, cx| {
                                this.open_search(
                                    SearchTab::Terminals,
                                    SEARCH_AGENT_QUERY,
                                    window,
                                    cx,
                                )
                            });
                        }
                    },
                ));
            }
        }

        // The one place ⌥ is spelled out. Nothing else in the app teaches it,
        // and a modifier nobody is told about is a feature nobody has. No rule
        // above it: a separator divides two lists of things to pick, and this
        // is a footnote about the list it follows, not a section of its own.
        menu.item(PopupMenuItem::label(t_fmt(
            L10nKey::TabMenuSplitHint,
            &[("key", split_modifier())],
        )))
    }
}

/// Every shell in the inventory, most likely first: the default, then whatever
/// has been opened often and recently, then the rest in the inventory's own
/// order. Shared by the palette and the New Tab menu so the same shell leads
/// both lists.
pub(crate) fn shells_by_frecency<'a>(
    shells: &'a [DetectedShell],
    default: &str,
    usage: &std::collections::HashMap<String, crate::core::config::ProfileUsage>,
    now: u64,
) -> Vec<&'a DetectedShell> {
    let score = |s: &DetectedShell| usage.get(&s.label).map_or(0.0, |u| u.score(now));
    let mut sorted: Vec<&DetectedShell> = shells.iter().collect();
    // Stable, so shells nobody has opened keep the order the inventory chose.
    sorted.sort_by(|a, b| {
        (b.label == default)
            .cmp(&(a.label == default))
            .then_with(|| {
                score(b)
                    .partial_cmp(&score(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    sorted
}

/// The shells the menu names, off the front of [`shells_by_frecency`]'s list:
/// its head — the default, whenever the inventory has it — and then only
/// shells that have actually been opened, [`MENU_SHELLS`] rows in all.
///
/// A shell nobody has opened is not named just because there is room: a menu
/// that filled its spare rows with `csh` and `tcsh` would be the nine-row list
/// again, only shorter.
fn menu_shells<'a>(
    sorted: &[&'a DetectedShell],
    default: &str,
    usage: &std::collections::HashMap<String, crate::core::config::ProfileUsage>,
    now: u64,
) -> Vec<&'a DetectedShell> {
    sorted
        .iter()
        .take(MENU_SHELLS)
        .enumerate()
        .filter(|(i, s)| {
            *i == 0 || s.label == default || usage.get(&s.label).is_some_and(|u| u.score(now) > 0.0)
        })
        .map(|(_, s)| *s)
        .collect()
}

/// The agents the menu names, off the front of the frecency-ordered list
/// quick launch already built: those that have actually been run, at most
/// [`MENU_AGENTS`].
///
/// A zero count is not a use — launching stamps recency before the pane
/// reports the agent, and the count is what says it really ran. But when
/// nothing offered has run yet the first [`MENU_AGENTS`] are named anyway:
/// a fresh profile's menu that is only a heading and the seam makes the
/// first launch take a menu, a search scope and a pick. `offered` is
/// already most-used-first; this does not reorder.
fn menu_agents(
    offered: &[CLIAgent],
    usage: &std::collections::HashMap<String, crate::core::config::ProfileUsage>,
    now: u64,
) -> Vec<CLIAgent> {
    let named: Vec<CLIAgent> = offered
        .iter()
        .copied()
        .filter(|agent| usage.get(agent.slug()).is_some_and(|u| u.score(now) > 0.0))
        .take(MENU_AGENTS)
        .collect();
    match named.is_empty() {
        true => offered.iter().copied().take(MENU_AGENTS).collect(),
        false => named,
    }
}

/// The hosts the menu names, in the order they were handed over — frecency,
/// so the ones that fit are the ones actually used.
///
/// A host saved without a name has nothing to show but where it goes, so it
/// leads with the endpoint rather than leaving the row blank — and then drops
/// the endpoint beside it, because a row that says `root@build.lan` twice
/// tells the reader nothing the first half did not.
fn menu_hosts(
    profiles: Vec<crate::core::ssh_profile::SshProfile>,
) -> Vec<(uuid::Uuid, SharedString, SharedString)> {
    profiles
        .into_iter()
        .take(MENU_HOSTS)
        .map(|p| {
            let endpoint = crate::core::ssh_profile::to_connect_string(&p);
            let name = if p.name.trim().is_empty() {
                endpoint.clone()
            } else {
                p.name.clone()
            };
            let note = if name == endpoint {
                String::new()
            } else {
                endpoint
            };
            (p.id, SharedString::from(name), SharedString::from(note))
        })
        .collect()
}

/// A menu row that names a thing on the left and says what it is on the right.
///
/// Both halves are cut rather than allowed to push: the panel stops at
/// [`MENU_W`], and a descriptive host name next to a long `user@host:port`
/// asks for more than that. Which half gives way is the whole point — the name
/// is what the reader is picking by, so it keeps its width up to
/// [`MENU_NAME_W`], and the endpoint takes whatever is left and elides first.
/// Sized the other way round (name growing from nothing, endpoint at its
/// natural width) a long address squeezes the name down to `..` and the row
/// names nothing at all.
///
/// Both caps are fixed lengths, never a share of the row. The panel is sized
/// to fit its content, so while the text is laid out no row has a width yet:
/// anything capped by `relative(..)` or by flex shrinking measures at full
/// length, is squeezed afterwards, and is clipped mid-glyph at the panel edge
/// (`git@ssh.github.con`) instead of ending in an ellipsis. A pixel cap is
/// known up front, so the text elides against it. The two caps and the row's
/// insets add up to [`MENU_W`].
///
/// An empty note drops the right half entirely rather than leaving a zero-width
/// child to hold the `gap_3` open — the name is then free to use the full row,
/// still eliding at the panel edge.
fn menu_row(label: SharedString, note: SharedString, cx: &gpui::App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_shrink_0()
                .max_w(match note.is_empty() {
                    true => MENU_W - MENU_ROW_INSET,
                    false => MENU_NAME_W,
                })
                .truncate()
                .child(label),
        )
        .when(!note.is_empty(), |this| {
            // Pushed right by the cell, not by `text_right`: a line set
            // with `truncate` ignores its alignment and hugs the name.
            this.child(
                h_flex().flex_1().justify_end().child(
                    div()
                        .max_w(MENU_NOTE_W)
                        .truncate()
                        .text_color(muted)
                        .child(note),
                ),
            )
        })
}

/// The most of a [`menu_row`] its name keeps before it elides — the larger
/// share, since the name is what the row is picked by.
const MENU_NAME_W: Pixels = px(172.);

/// The most the endpoint beside a name gets: room for `user@host:port` on a
/// typical host, and the gap and the insets take the rest of [`MENU_W`].
const MENU_NOTE_W: Pixels = px(150.);

/// What the panel spends on either side of a row's content: its own 5px
/// inset and the row's 8px padding, twice.
const MENU_ROW_INSET: Pixels = px(26.);

/// The words behind the status dot's colour.
pub(crate) fn agent_status_label(
    status: Option<crate::core::cli_agent::AgentStatus>,
) -> Option<&'static str> {
    use crate::core::cli_agent::AgentStatus;
    match status? {
        AgentStatus::Idle => None,
        AgentStatus::Working => Some(t(L10nKey::AgentStatusWorking)),
        AgentStatus::Waiting => Some(t(L10nKey::AgentStatusWaiting)),
        AgentStatus::Done => Some(t(L10nKey::AgentStatusDone)),
    }
}

pub(crate) const LIVE_DOT: u32 = 0x22C55E;
pub(crate) const UNKNOWN_DOT: u32 = 0x9AA0A6;

pub(crate) fn select_workspace_action(index: usize) -> Option<Box<dyn gpui::Action>> {
    Some(match index {
        0 => Box::new(SelectWorkspace1) as Box<dyn gpui::Action>,
        1 => Box::new(SelectWorkspace2),
        2 => Box::new(SelectWorkspace3),
        3 => Box::new(SelectWorkspace4),
        4 => Box::new(SelectWorkspace5),
        5 => Box::new(SelectWorkspace6),
        6 => Box::new(SelectWorkspace7),
        7 => Box::new(SelectWorkspace8),
        8 => Box::new(SelectWorkspace9),
        _ => return None,
    })
}

impl Tty59App {
    pub(crate) const AVATAR_PX: f32 = 18.0;

    pub(crate) fn workspace_head(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        if let Some(rename) = self.workspace_rename.as_ref() {
            return h_flex()
                .id("workspace-rename")
                .flex_shrink_0()
                .items_center()
                .h(px(28.))
                .w_full()
                .px(px(9.))
                .rounded_md()
                .bg(cx.theme().sidebar_accent)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Input::new(&rename.input).appearance(false).xsmall())
                .into_any_element();
        }

        crate::terminal::pane_liveness::sweep(cx);
        let current = crate::ui::machine_mirror::display_name_for(cx, self.workspace)
            .unwrap_or_else(|| "tty59".to_string());
        let monogram: String = current
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "~".to_string());

        div()
            .occlude()
            .w_full()
            .capture_any_mouse_down(|ev: &gpui::MouseDownEvent, _window, cx| {
                if ev.button == MouseButton::Right {
                    cx.stop_propagation();
                }
            })
            .child(
                Button::new("rail-workspace-head")
                    .custom(chrome_tile_variant(cx))
                    .child(
                        h_flex()
                            .id("rail-workspace-head-ink")
                            .w_full()
                            .h_full()
                            .items_center()
                            .gap(px(6.))
                            // The tile's own hover is a fill the palette keeps
                            // a hair off the surface, which on the rail is
                            // barely a change at all — and the name and the
                            // chevron pinned their own ink, so the pointer
                            // landing on the one control at the top of the
                            // column said nothing. Answer the way a group
                            // header does: the ink steps up to full strength.
                            .text_color(cx.theme().muted_foreground)
                            .hover(|s| s.text_color(cx.theme().foreground))
                            .child(
                                div()
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .justify_center()
                                    .size(px(Self::AVATAR_PX))
                                    .rounded_full()
                                    .bg(cx.theme().secondary)
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(monogram),
                            )
                            .child(
                                // Chrome, not a row: the tile inherits the
                                // rail's title ink, which now belongs to the
                                // tabs. The workspace name reads at the group
                                // headers' weight so the one dark line in
                                // the column stays the tab in front.
                                div()
                                    .flex_shrink(1.)
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(SharedString::from(current.clone())),
                            )
                            .child(
                                // Not a chevron-down: this opens a centred
                                // panel, not a menu hanging off the button.
                                Icon::empty()
                                    .path("icons/chevrons-up-down.svg")
                                    .size(px(11.))
                                    .flex_shrink_0(),
                            ),
                    )
                    .xsmall()
                    .w_full()
                    .h(px(28.))
                    .rounded(px(7.))
                    // Button pins the arrow for every non-link variant; the
                    // switcher's own rows point, so the tile that opens them
                    // does too.
                    .cursor_pointer()
                    .accessible_label(t(L10nKey::HomeSwitchWorkspace))
                    .tooltip_element(chord_tooltip(
                        t(L10nKey::HomeSwitchWorkspace),
                        "ToggleSwitcher",
                        cx,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_switcher(window, cx);
                    })),
            )
            .into_any_element()
    }

    /// Persistent window controls, including when either sidebar is collapsed.
    pub(crate) fn window_chrome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let trailing = match cfg!(target_os = "macos") {
            true => tile_trailing_inset(),
            false => 4.,
        };
        // Opening the panel moves this control to the panel's header on
        // macOS. Reuse the same tile and inset there: a different hit target
        // moves the centred glyph even when the icon itself keeps its size.
        let panel_open = self.right_panel_open(cx);
        h_flex()
            .flex_shrink_0()
            .items_center()
            .pr(px(trailing))
            .child(
                div().occlude().flex_shrink_0().child(
                    chrome_tile(
                        Button::new("titlebar-right-panel")
                            // Keep the outline fixed; only the panel region's
                            // fill changes to show whether the panel is visible.
                            .icon(Icon::empty().path(match panel_open {
                                true => "icons/panel-right.svg",
                                false => "icons/panel-right-collapsed.svg",
                            })),
                        false,
                        cx,
                    )
                    .rounded_lg()
                    .accessible_label(match panel_open {
                        true => t(L10nKey::TabTooltipHideDetailPanel),
                        false => t(L10nKey::TabTooltipShowDetailPanel),
                    })
                    .tooltip_element(chord_tooltip(
                        match panel_open {
                            true => t(L10nKey::TabTooltipHideDetailPanel),
                            false => t(L10nKey::TabTooltipShowDetailPanel),
                        },
                        "ToggleRightPanel",
                        cx,
                    ))
                    // On macOS this tile is drawn inside the panel's own
                    // titlebar while the panel is open, so closing from it
                    // destroys the element holding the focus — and a keymap
                    // scoped to a focused thing goes quiet with it, leaving the
                    // ⌘J that would undo this doing nothing. Hand the terminal
                    // back what it lost.
                    .on_click(cx.listener(|this, _, window, cx| {
                        let closing = this.right_panel_open(cx);
                        this.toggle_right_panel(cx);
                        if closing {
                            this.focus_active(window, cx);
                        }
                    })),
                ),
            )
    }

    /// The right panel's tabs: words, the current one in body ink and medium
    /// weight on a pill (ink and weight alone read as no selection at all).
    /// Four names fit the panel's resting width once Search folded into Files.
    pub(crate) fn right_panel_tabs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active_tab = self.right_panel_tab;
        let body_ink = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        RIGHT_PANEL_TABS
            .into_iter()
            .map(|(tab, label_key)| {
                let current = active_tab == tab;
                let ink = match current {
                    true => body_ink,
                    false => muted,
                };
                div()
                    .id(("right-panel-tab", tab as usize))
                    .role(gpui::Role::Tab)
                    .aria_label(t(label_key))
                    .aria_selected(current)
                    // The press must not start a window drag from the title bar
                    // the tabs sit in.
                    .occlude()
                    .flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .px(px(TAB_OUTER_PAD))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(TAB_INNER_PAD))
                            .py_0p5()
                            .rounded_md()
                            .when(current, |s| s.bg(cx.theme().secondary))
                            .text_size(gpui::rems(crate::ui::right_panel::TAB_TEXT))
                            .font_weight(match current {
                                true => FontWeight::MEDIUM,
                                false => FontWeight::NORMAL,
                            })
                            .text_color(ink)
                            .hover(move |s| s.text_color(body_ink))
                            .child(t(label_key)),
                    )
                    // The current one does nothing: only ⌘J and the title-bar
                    // panel tile put the panel away.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.right_panel_tab != tab {
                            this.set_right_panel_tab(tab, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Working and Done differ only in hue (blue vs green), and Waiting vs Done
    /// — the pair that actually decides whether you go and look — is amber vs
    /// green, the pair red-green colour vision separates worst. Give Waiting a
    /// hole so it is a different *shape*, not just a different colour.
    fn status_dot(
        rgb: u32,
        unread: usize,
        size: f32,
        ring: gpui::Hsla,
        hollow: bool,
        faded: bool,
    ) -> gpui::AnyElement {
        let d = (size * 0.42).max(7.);
        // The halo was the surface itself, which is only a ring while the
        // surface is light — on a dark theme it went near-black and read as a
        // notch bitten out of the avatar rather than a badge sitting on it.
        // Light themes already ring the dot in white; give the dark ones the
        // same white edge, and the hollow Waiting dot the same white hole.
        let bg = match crate::ui::presets::surface_is_dark(ring) {
            true => gpui::white(),
            false => ring,
        };
        // The dim beat of a blink is a paler fill, still opaque: fading the
        // whole badge let the avatar show through the dot and its ring.
        let fill: gpui::Hsla = match faded {
            true => bg.blend(gpui::Hsla::from(gpui::rgb(rgb)).opacity(0.4)),
            false => gpui::rgb(rgb).into(),
        };
        if let Some(label) = unread_count_label(unread) {
            let nd = (size * 0.72).max(13.0);
            let pill = div()
                .absolute()
                .right(px(-(nd - d) / 2.0 - d * 0.22))
                .bottom(px(-(nd - d) / 2.0 - d * 0.22))
                .h(px(nd))
                .min_w(px(18.))
                .px(px(4.))
                .rounded_full()
                .border_1()
                .border_color(bg)
                .bg(fill)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px((nd * 0.62).round()))
                .line_height(px(nd))
                .font_features(crate::ui::theme::tabular_figures())
                .font_weight(FontWeight::BOLD)
                .text_color(gpui::white())
                .child(label);
            #[cfg(test)]
            let pill = pill.debug_selector(move || format!("avatar-status-count-{unread}"));
            pill.into_any_element()
        } else {
            let dot = div()
                .absolute()
                .right(px(-(d * 0.22)))
                .bottom(px(-(d * 0.22)))
                .size(px(d))
                .rounded_full()
                .border_2()
                .border_color(bg)
                .bg(fill)
                .when(hollow, |dot| {
                    dot.flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px((d * 0.36).max(2.5))).rounded_full().bg(bg))
                });
            #[cfg(test)]
            let dot = dot.debug_selector(move || format!("avatar-status-circle-{unread}"));
            dot.into_any_element()
        }
    }

    pub(crate) fn tab_avatar(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        ssh: Option<u32>,
        size: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        self.tab_avatar_badged(id, agent, status, unread, ssh, size, true, cx)
    }

    /// The avatar without the agent's status badge, for a row that says the
    /// status at its trailing end instead (see [`row_status_dot`]). The
    /// tooltip still names the state; the SSH dot on a shell stays, since it
    /// describes the connection rather than an agent's run.
    ///
    /// [`row_status_dot`]: Self::row_status_dot
    pub(crate) fn tab_avatar_plain(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        ssh: Option<u32>,
        size: f32,
        cx: &App,
    ) -> gpui::AnyElement {
        self.tab_avatar_badged(id, agent, status, 0, ssh, size, false, cx)
    }

    /// A row's trailing status mark: a 5px dot in the state's colour, hollow
    /// while the agent waits on the user, blinking while it works — or the
    /// count in a wider pill only when more than one result is unread. `None`
    /// for a row with nothing to report.
    pub(crate) fn row_status_dot(
        &self,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        surface: gpui::Hsla,
    ) -> Option<gpui::AnyElement> {
        let rgb = status.and_then(|s| s.dot_rgb())?;
        let hollow = status == Some(crate::core::cli_agent::AgentStatus::Waiting);
        let faded =
            status == Some(crate::core::cli_agent::AgentStatus::Working) && !self.working_dot_on;
        let ink: gpui::Hsla = gpui::rgb(rgb).into();
        let fill = match faded {
            true => surface.blend(ink.opacity(0.4)),
            false => ink,
        };
        let slot = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(ROW_STATUS_SLOT));
        let mark = match unread_count_label(unread) {
            Some(label) => {
                let pill = div()
                    .h(px(14.))
                    .min_w(px(18.))
                    .px(px(4.))
                    .rounded_full()
                    .bg(fill)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .line_height(px(14.))
                    .font_features(crate::ui::theme::tabular_figures())
                    .font_weight(FontWeight::BOLD)
                    .text_color(gpui::white())
                    .child(label);
                #[cfg(test)]
                let pill = pill.debug_selector(move || format!("row-status-count-{unread}"));
                pill.into_any_element()
            }
            None => {
                let dot = div()
                    .size(px(ROW_STATUS_DOT))
                    .rounded_full()
                    .when(!hollow, |d| d.bg(fill))
                    .when(hollow, |d| d.border_1().border_color(fill));
                #[cfg(test)]
                let dot = dot.debug_selector(move || format!("row-status-circle-{unread}"));
                dot.into_any_element()
            }
        };
        Some(slot.child(mark).into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_avatar_badged(
        &self,
        id: impl Into<gpui::ElementId>,
        agent: Option<crate::core::cli_agent::CLIAgent>,
        status: Option<crate::core::cli_agent::AgentStatus>,
        unread: usize,
        ssh: Option<u32>,
        size: f32,
        badge: bool,
        cx: &App,
    ) -> gpui::AnyElement {
        avatar_disc(
            id,
            crate::ui::search::Avatar {
                agent,
                status,
                unread,
                ssh,
            },
            size,
            self.working_dot_on,
            badge,
            cx,
        )
    }

    /// The mark a tab wears while one of its panes is zoomed over the others
    /// (#752). Without it a zoomed tab is pixel-for-pixel a tab that only ever
    /// had one pane, and the only way to tell was to toggle the zoom off.
    ///
    /// Drawn in the tab entry rather than on the pane so it reads from either
    /// tab surface, and so it says something about the tabs you are *not*
    /// looking at — the zoom outlives a switch away from them.
    /// The mark a sleeping tab carries (#762), in the slot the zoom mark uses:
    /// leading, beside the other state marks, where the pointer that comes to
    /// read it does not cover it.
    pub(crate) fn sleep_mark(&self, id: impl Into<gpui::ElementId>, cx: &App) -> gpui::AnyElement {
        let tip = SharedString::from(t(L10nKey::TabTooltipAsleep));
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.))
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Moon).size(px(11.)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
    }

    pub(crate) fn zoom_mark(&self, id: impl Into<gpui::ElementId>, cx: &App) -> gpui::AnyElement {
        let tip = chord_tooltip(t(L10nKey::TabTooltipZoomed), "ToggleMaximizePane", cx);
        div()
            .id(id)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .size(px(16.))
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Maximize).size(px(11.)))
            // Not a `Button`, so this one goes through gpui's own `tooltip`
            // rather than `tooltip_element` — but it is the same builder, and
            // the chord lands in the same slot the chrome tiles use.
            .tooltip(move |window, cx| tip(window, cx).into())
            .into_any_element()
    }

    /// The full title behind a shortened one, for the row to name on hover.
    ///
    /// `tab_label` hands back a path elided to its last three segments and then
    /// capped, and the chip truncates whatever is left over — so a tab could
    /// read `…/a/b/c` with no way to find out which `a` that was. `None` when
    /// nothing was dropped, so tabs that already show their whole name stay
    /// quiet under the pointer.
    ///
    /// It has to unshorten whatever the label was *made of*, which is why it
    /// reads the same [`TabView`](crate::ui::machine_mirror::TabView) the label
    /// did: a tab named after its directory wants that directory spelled out,
    /// not the title it never had. See [`tooltip_of`].
    pub(crate) fn tab_title_tooltip(
        &self,
        tab: &Tab,
        index: usize,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<SharedString> {
        let (view, home) = tab.label_view(window, cx);
        tooltip_of(&view, index, home.as_deref())
    }

    /// What this window puts on a tab of its own — the same ladder, through the
    /// same renderer, as the switcher uses for a tab of somebody else's window.
    /// See [`label_of`].
    pub(crate) fn tab_label(
        &self,
        tab: &Tab,
        index: usize,
        window: Option<&Window>,
        cx: &App,
    ) -> String {
        let (view, home) = tab.label_view(window, cx);
        label_of(&view, index, home.as_deref())
    }

    /// The same ladder as [`tab_label`](Self::tab_label), left whole: a path
    /// is only abbreviated under the home and stripped of its `user@host:`,
    /// never cut down to its last segments. For the surfaces with room to
    /// spare — the rail's rows and the centred title — where only the width
    /// they have may decide what gets elided, not a segment count picked for
    /// the chips. `None` when there is nothing to say and the caller should
    /// fall back to its own placeholder.
    pub(crate) fn full_tab_label(
        &self,
        tab: &Tab,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<String> {
        use crate::ui::machine_mirror::TabLabel;

        if let Some(name) = tab.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return Some(name.trim().to_string());
        }
        let (view, home) = tab.label_view(window, cx);
        let raw = match view.label() {
            TabLabel::Osc(title) | TabLabel::Cwd(title) => {
                abbreviate_home(strip_host_prefix(title.trim()), home.as_deref()).into_owned()
            }
            TabLabel::Agent(agent) => agent.display_name().to_string(),
            TabLabel::Named(name) => name.to_string(),
            TabLabel::Process(title) => title.to_string(),
            TabLabel::Unknown => String::new(),
        };
        (!raw.trim().is_empty()).then_some(raw)
    }

    /// The New Tab control: one `+` that drops the list of everything it could
    /// open — the installed shells, and the saved SSH hosts.
    ///
    /// It was briefly split in two, a `+` that opened a tab outright next to a
    /// caret for the list, the way Windows Terminal and VS Code split theirs.
    /// That buys back the click the menu costs on the most common action, and
    /// it costs a second mark in a row of single ones. This row is four icons
    /// wide and reads as four icons; a fifth that was half of the fourth had
    /// to earn its place by looking like a pair, and a pair is exactly what a
    /// chrome row of plain icons has no vocabulary for.
    ///
    /// So: one tile, drawn and hovered like every other tile beside it, and
    /// the click it costs is answered by ⌘T rather than by a second button.
    pub(crate) fn new_tab_button(
        &self,
        id: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        self.new_tab_button_sized(id, crate::ui::app::TILE_SIZE, cx)
    }

    /// [`new_tab_button`](Self::new_tab_button) at another tile size — the
    /// document header can use a smaller tile without changing the title bar.
    pub(crate) fn new_tab_button_sized(
        &self,
        id: &'static str,
        tile: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let app = cx.entity().downgrade();
        let glyph = match tile < crate::ui::app::TILE_SIZE {
            true => RAIL_TILE_GLYPH,
            false => TILE_GLYPH,
        };
        chrome_tile_sized(
            Button::new(id).icon(Icon::new(IconName::Plus)),
            tile,
            glyph,
            false,
            cx,
        )
        .rounded(px(RAIL_TILE_RADIUS))
        // Every other tile in this row names itself on hover — Switch
        // Workspace, More, Hide Sidebar. The three New Tab buttons that
        // come through here were the ones left silent. The chord is worth
        // more here than anywhere else in the row: it is the way back to
        // opening a tab without reading a menu first.
        .accessible_label(t(L10nKey::AppMenuNewTab))
        .tooltip_element(chord_tooltip(t(L10nKey::AppMenuNewTab), "NewTab", cx))
        // Built when the menu opens, not when the strip draws: this
        // closure runs once per press, and again after each dismissal.
        .dropdown_menu(move |menu, window, cx| {
            let Some(this) = app.upgrade() else {
                return menu;
            };
            this.read(cx)
                .new_tab_menu_rows(app.clone(), cx)
                .build(menu, window)
        })
    }

    /// What the menu offers, read off the app as the menu opens — the builder
    /// runs on the popup's own entity, so the rows carry a weak handle back.
    fn new_tab_menu_rows(&self, app: gpui::WeakEntity<Self>, cx: &App) -> NewTabMenu {
        let default_shell = self.default_shell_label(cx);
        let usage = &cx.global::<Config>().shell_frecency;
        let now = crate::core::config::unix_now();
        let sorted = shells_by_frecency(&self.shells.shells, &default_shell, usage, now);
        let shells = menu_shells(&sorted, &default_shell, usage, now);
        let offered = self.offered_agents(cx);
        let agents = menu_agents(&offered, &cx.global::<Config>().agent_frecency, now);
        NewTabMenu {
            app,
            more_shells: shells.len() < sorted.len(),
            shells: shells
                .into_iter()
                .map(|s| SharedString::from(s.label.clone()))
                .collect(),
            default_shell: SharedString::from(default_shell),
            hosts: menu_hosts(crate::ui::ssh_connect::ssh_profiles_by_frecency(cx)),
            more_agents: agents.len() < offered.len(),
            agents,
        }
    }

    /// Open the inventory's shell by the label it is listed under, wherever
    /// `at` says — the New Tab menu's rows and the palette's `Shell:` commands
    /// both land here, so both count toward the frecency that orders them.
    pub(crate) fn open_listed_shell(
        &mut self,
        label: &str,
        at: SpawnWhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(spec) = self
            .shells
            .shells
            .iter()
            .find(|s| s.label == label)
            .map(shell_spec)
        else {
            return;
        };
        self.update_config(cx, |cfg| {
            let entry = cfg.shell_frecency.entry(label.to_string()).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = crate::core::config::unix_now();
        });
        self.open_shell(Some(spec), at, window, cx);
    }

    pub(crate) fn tab_context_menu(
        menu: PopupMenu,
        index: usize,
        below_wording: bool,
        app: &gpui::WeakEntity<Self>,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let Some(entity) = app.upgrade() else {
            return menu;
        };
        let this = entity.read(cx);
        // Taken before `this` is let go for the Move to Group submenu below.
        let sidebar =
            cx.global::<Config>().tab_bar_position == crate::core::config::TabBarPosition::Left;
        let move_targets = sidebar.then(|| {
            let keys = this.sidebar_group_keys(cx);
            crate::ui::tab_sidebar::move_targets(&keys, &this.sidebar_groups, index)
        });
        let in_kept_group = this
            .tabs
            .get(index)
            .and_then(|t| t.group.get())
            .is_some_and(|g| this.sidebar_groups.contains(g));
        let tab_count = this.tabs.len();
        let cwd = this.tab_cwd_text(index, window, cx);
        let has_cwd = cwd.is_some();
        let mut menu = menu.min_w(px(200.));

        // Every item here acts on *this* tab, so the work is done by the click
        // handler and the action is carried only so `PopupMenu` can look its
        // chord up and print it. The handler wins when both are set. Without
        // this the tab menu was the one context menu in the app that taught no
        // shortcuts — right-clicking a pane offered "Split Right ⌘D" while
        // right-clicking its tab offered a bare "Split Right".
        menu = menu.item(
            PopupMenuItem::new(t(L10nKey::AppMenuRenameTab))
                .action(Box::new(RenameTab))
                .on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.start_rename(index, window, cx));
                    }
                }),
        );

        let tab = this.tabs.get(index);
        if tab.is_some_and(|t| t.agent(cx).is_some()) {
            let done = tab.and_then(|t| t.agent_status(cx))
                == Some(crate::core::cli_agent::AgentStatus::Done);
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::TabContextMarkUnread))
                    .action(Box::new(MarkTabUnread))
                    .disabled(!done)
                    .on_click({
                        let app = app.clone();
                        move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| this.mark_tab_unread(index, cx));
                        }
                    }),
            );
        }

        // Sleep and wake (#762). Waking is also what selecting the tab does;
        // the item is here for waking one without leaving the tab on screen.
        // Hibernate is offered only where the machine can do it, and greyed
        // out for a tab it cannot be done to right now — the last one awake,
        // or one still connecting — rather than vanishing from under the
        // pointer that came looking for it.
        if tab.is_some_and(|t| t.is_asleep()) {
            menu = menu.item(PopupMenuItem::new(t(L10nKey::TabContextWake)).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |this, cx| this.wake_tab(index, window, cx));
                }
            }));
        } else if crate::ui::tree_sync::can_hibernate_on(cx, this.workspace) {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::TabContextHibernate))
                    .action(Box::new(HibernateTab))
                    .disabled(!this.can_hibernate_tab(index, cx))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ =
                                app.update(cx, |this, cx| this.hibernate_tab(index, window, cx));
                        }
                    }),
            );
        }

        // Where this tab sits, and where it could be put instead: every group
        // the sidebar draws, auto ones included, behind a submenu — a sidebar
        // grouping by repo can hold dozens, far too many to lay out flat.
        //
        // An auto group's membership is its tabs' cwds, so a tab can only be
        // put in one by keeping the group: picking it pins the group (as its
        // header's pin does) and the tab with it. Remove from Group, offered
        // only for a tab kept in a group, hands it back to auto grouping.
        //
        // Offered only with the tabs in the sidebar — a group is something the
        // sidebar draws, and "move to group" from the top tab bar would name
        // something the user cannot see.
        if let Some(targets) = move_targets {
            let app = app.clone();
            menu = menu.separator().submenu(
                t(L10nKey::SidebarMoveToGroup),
                window,
                cx,
                move |sub, _window, _cx| {
                    // A left check makes every row of the menu reserve a
                    // check column, so the whole submenu sat one icon's
                    // width right of the tab menu beside it. On the right,
                    // its labels line up with the parent's.
                    let mut sub = sub.check_side(Side::Right);
                    for (i, target) in targets.iter().enumerate() {
                        // Pinned groups, then the auto ones, as the divider
                        // splits them in the sidebar.
                        if i > 0 && targets[i - 1].key.is_pinned() && !target.key.is_pinned() {
                            sub = sub.separator();
                        }
                        let mut item =
                            PopupMenuItem::new(target.name.clone()).checked(target.checked);
                        if !target.checked {
                            let app = app.clone();
                            let key = target.key.clone();
                            item = item.on_click(move |_, _window, cx| {
                                let _ = app.update(cx, |this, cx| {
                                    this.move_tab_to(index, key.clone(), cx)
                                });
                            });
                        }
                        sub = sub.item(item);
                    }
                    if !targets.is_empty() {
                        sub = sub.separator();
                    }
                    let remove = app.clone();
                    let new = app.clone();
                    sub.item(
                        PopupMenuItem::new(t(L10nKey::SidebarRemoveFromGroup))
                            .disabled(!in_kept_group)
                            .on_click(move |_, _window, cx| {
                                let _ = remove
                                    .update(cx, |this, cx| this.set_tab_group(index, None, cx));
                            }),
                    )
                    .item(
                        PopupMenuItem::new(t(L10nKey::SidebarNewGroup)).on_click(
                            move |_, window, cx| {
                                let _ = new
                                    .update(cx, |this, cx| this.new_tab_group(index, window, cx));
                            },
                        ),
                    )
                },
            );
        }
        // Read again: the submenu above needed `cx` mutably.
        let this = entity.read(cx);

        let in_repo = this.tab_is_in_repo(index, window, cx);
        if in_repo {
            menu = menu.separator().item(
                PopupMenuItem::new(t(L10nKey::AppMenuNewWorktreeTab))
                    .action(Box::new(NewWorktreeTab))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ =
                                app.update(cx, |this, cx| this.new_worktree_tab(index, window, cx));
                        }
                    }),
            );
        }

        let agent_session = this.tab_agent_session(index, window, cx);
        if let Some((source, session)) = &agent_session
            && let Some(label) = session.fork_label
        {
            if !in_repo {
                menu = menu.separator();
            }
            let forkable = session.forkable();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .action(Box::new(ForkAgentSession))
                    .disabled(!forkable)
                    .on_click({
                        let app = app.clone();
                        let source = source.clone();
                        move |_, window, cx| {
                            let source = source.clone();
                            let _ = app.update(cx, |this, cx| {
                                this.fork_agent_session(
                                    index,
                                    source,
                                    crate::ui::app::ForkPlacement::NewTab,
                                    window,
                                    cx,
                                )
                            });
                        }
                    }),
            );
        }

        // The connection this tab is on, editable from the tab itself. A
        // hostname or password typed wrong used to be fixable only by finding
        // the same host again in Settings, and right-clicking the connection —
        // the gesture that asks "change this" — offered nothing (#438). The row
        // is the switcher machine menu's, word for word: the saved host when
        // there is one, an offer to keep the address when it was dialled by
        // hand, and nothing at all for a tab with no host form behind it.
        if let Some((form, label)) = this.tab_ssh_host_form(index, window, cx) {
            menu = menu.separator().item(PopupMenuItem::new(label).on_click({
                let app = app.clone();
                move |_, window, cx| {
                    let form = form.clone();
                    let _ = app.update(cx, |this, cx| {
                        this.open_tab_ssh_host_form(&form, window, cx)
                    });
                }
            }));
        }

        menu = menu
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitRight))
                    .action(Box::new(SplitRight))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split(Axis::Horizontal, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuSplitDown))
                    .action(Box::new(SplitDown))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| {
                                this.activate(index, window, cx);
                                this.split(Axis::Vertical, window, cx);
                            });
                        }
                    }),
            );

        menu = menu.separator().item(
            PopupMenuItem::new(t(L10nKey::AppMenuCopyWorkingDirectory))
                .action(Box::new(CopyWorkingDirectory))
                .disabled(!has_cwd)
                .on_click(move |_, _window, cx| {
                    if let Some(text) = cwd.as_ref() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    }
                }),
        );

        if let Some(session_id) = agent_session.map(|(_, s)| s.session_id) {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::AppMenuCopySessionId))
                    .action(Box::new(CopyAgentSessionId))
                    .disabled(session_id.is_none())
                    .on_click(move |_, _window, cx| {
                        if let Some(id) = session_id.as_ref() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.clone()));
                        }
                    }),
            );
        }

        menu.separator()
            .item(
                PopupMenuItem::new(t(L10nKey::TabContextCloseTab))
                    .action(Box::new(CloseActiveTab))
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ = app.update(cx, |this, cx| this.close_tab(index, window, cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::AppMenuCloseOtherTabs))
                    .action(Box::new(CloseOtherTabs))
                    .disabled(tab_count <= 1)
                    .on_click({
                        let app = app.clone();
                        move |_, window, cx| {
                            let _ =
                                app.update(cx, |this, cx| this.close_other_tabs(index, window, cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(if below_wording {
                    t(L10nKey::TabContextCloseTabsBelow)
                } else {
                    t(L10nKey::AppMenuCloseTabsRight)
                })
                .action(Box::new(CloseTabsToTheRight))
                .disabled(index + 1 >= tab_count)
                .on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ =
                            app.update(cx, |this, cx| this.close_tabs_right_of(index, window, cx));
                    }
                }),
            )
    }

    pub(crate) fn tab_strip(
        &self,
        show_chips: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active;
        let agent_mode = self.agent_mode_active();
        let panel_open = !agent_mode && self.right_panel_open(cx);
        let badge_positions = show_chips.then(|| self.tab_badge_positions(cx)).flatten();
        // On macOS an open detail panel draws its own chrome in the title bar,
        // so the strip stops at the panel's edge rather than running the width
        // of the window. Sizing it to the whole viewport made it overrun that
        // edge, and what got pushed out past it was the New Tab button.
        let panel_w = match cfg!(target_os = "macos") && panel_open {
            true => self.right_panel_px(window, cx),
            false => 0.,
        };
        // A docked document column has the same claim on this strip the detail
        // panel does, and it is answered in the same two ways. On macOS the
        // strip lives *inside* the terminal column, so the column's width comes
        // off `strip_w` the way the panel's does — sizing the strip to more
        // than it gets is what pushed the New Tab button out before.
        // Everywhere else the strip spans the workspace and the column's header
        // is drawn over its trailing end, so the width is reserved as a corner
        // instead: that header carries no fill of its own, and a chip left
        // under it showed through the file name while staying clickable.
        let document_w = if agent_mode {
            0.
        } else {
            self.document_dock_px(window, cx).unwrap_or(0.)
        };
        let controls_w = crate::ui::app::window_controls_w(window.is_fullscreen());
        let pad = gpui_component::window_paddings(window);
        let strip_w = if cfg!(target_os = "macos") {
            strip_width(
                window.viewport_size().width,
                pad,
                crate::ui::app::TITLE_BAR_LEAD + panel_w + document_w,
                160.,
            )
        } else {
            strip_width(
                window.viewport_size().width,
                pad,
                crate::ui::app::TITLE_BAR_LEAD + controls_w,
                140.,
            )
        };
        let chrome_band_w = (!cfg!(target_os = "macos") && panel_open)
            .then(|| (self.right_panel_px(window, cx) - controls_w - 1.).max(0.));
        // `corner_w` reserves the trailing window chrome. With the panel open on
        // macOS that chrome belongs to the panel's own header, which the strip
        // now stops short of, so reserving for it here would charge the chips
        // for it twice.
        // On macOS the trailing tiles sit at the end of the terminal column's
        // strip only while nothing stands to the right of it. A docked document
        // takes the window's right edge the way the panel does, so the tiles
        // give way to it instead of hanging in the middle of the window beside
        // the document's own header. ⌘J and the palette still reach both.
        let strip_chrome = !cfg!(target_os = "macos") || (panel_w <= 0. && document_w <= 0.);
        let corner_w = if !strip_chrome || panel_w > 0. {
            0.
        } else {
            chrome_band_w.unwrap_or_else(trailing_chrome_tiles_w)
        } + if cfg!(target_os = "macos") {
            // Already taken out of `strip_w` above; charging it here too would
            // narrow the chips by a column's width twice over.
            0.
        } else {
            document_w
        };
        // One scan of the panes a frame: the pill's width, which the strip
        // budgets, and the number the pill says come from the same count.
        let attention = self
            .agent_vision
            .state
            .attention_count(&self.agent_vision.panes, &self.agent_vision.reads)
            .total();
        let pill_w = self.strip_metrics.pill_w(agent_mode, attention);
        let rem = window.rem_size().as_f32();
        // The row's gaps: chips to New Tab, New Tab to the spacer, the spacer
        // to the pill, and the pill to the trailing chrome.
        let fixed_w = 4. * STRIP_GAP_REM * rem + crate::ui::app::TILE_SIZE + pill_w + corner_w;
        let chips_avail = (strip_w - px(fixed_w + GRAB_HANDLE_W)).max(px(80.));
        // The hidden-tab count sits beside the chips, so what it takes comes
        // off their room before anything is drawn. Never below one chip.
        let count_room = match show_chips {
            true => count_reserve(f32::from(chips_avail), self.tabs.len(), rem),
            false => 0.,
        };
        let chips_avail = match count_room > 0. {
            true => (chips_avail - px(count_room)).max(px(CHIP_MIN_W)),
            false => chips_avail,
        };
        let mut chips = h_flex()
            .items_center()
            .gap(px(CHIP_GAP))
            .min_w_0()
            .max_w(chips_avail)
            .overflow_hidden();

        // Held by the app rather than by the frame: a pane dropped up here has
        // to read the gaps between the chips, and it is asking a frame later
        // than the one that drew them. Blanked here and written again from
        // paint, so a chip that is not drawn this time — the strip is hidden,
        // or the chip scrolled out of it — leaves nothing behind to aim at.
        let slots = self.strip_slots.clone();
        *slots.borrow_mut() = vec![Bounds::default(); self.tabs.len()];
        let preview = reorder::preview(
            &self.reorder,
            &Surface::Strip,
            self.tabs.len(),
            window.mouse_position(),
        );
        let display: Vec<usize> = match &preview {
            Some(p) => {
                reorder::set_pending(&self.reorder, &Surface::Strip, p.order.clone());
                p.order.clone()
            }
            None => (0..self.tabs.len()).collect(),
        };
        let shown = visible_chips(&display, active, f32::from(chips_avail));
        let hidden = display.len() - shown.len();
        let display = shown;

        for i in display {
            if !show_chips {
                break;
            }
            let dragged = preview.as_ref().is_some_and(|p| p.from == i);
            let badge_pos = badge_positions.as_ref().and_then(|positions| positions[i]);
            let tab = &self.tabs[i];
            let is_active = i == active;
            let label = self.tab_label(tab, i, Some(window), cx);
            let full_title = self.tab_title_tooltip(tab, i, Some(window), cx);
            let ssh_dot = self.tab_ssh_dot(tab, cx);
            let asleep = tab.is_asleep();
            let agent = tab.agent(cx).or_else(|| tab.asleep_agent());
            let agent_status = tab.agent_status(cx);
            let agent_unread = tab.agent_unread_count(cx);
            let zoomed = self.tab_is_zoomed(i);

            let rename_input = self
                .renaming
                .as_ref()
                .filter(|r| r.tab == tab.tree_id.get())
                .map(|r| r.input.clone());
            let label_region = match rename_input {
                Some(input) => div()
                    .id(("tab-rename", i))
                    .flex_1()
                    .min_w_0()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // The chip switches tabs on the *release* now, so holding
                    // the press back is no longer enough: a click landing in
                    // the field would reach the chip behind it and switch away
                    // from the name being typed, taking the focus with it.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    // No inset of its own: the label it replaces starts
                    // flush, and the field's 12px padding jumped the name
                    // sideways the moment rename began.
                    .child(Input::new(&input).appearance(false).px_0())
                    .into_any_element(),
                None => div()
                    .id(("tab-label", i))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(is_active, |d| d.font_weight(FontWeight::MEDIUM))
                    .when_some(full_title, |d, title| {
                        d.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(title.clone()).build(window, cx)
                        })
                    })
                    .child(label)
                    .into_any_element(),
            };

            let chip = h_flex()
                .id(("tab-chip", i))
                .on_drag(DragTab, {
                    let state = self.reorder.clone();
                    let slots = slots.clone();
                    let id = tab.tree_id.get();
                    move |_drag, grab, _window, cx| {
                        cx.stop_propagation();
                        *state.borrow_mut() = Some(
                            Reorder::new(
                                Surface::Strip,
                                i,
                                slots.borrow().clone(),
                                Axis::Horizontal,
                                px(CHIP_GAP),
                                grab,
                            )
                            .of_tab(id),
                        );
                        cx.new(|_| DragTab)
                    }
                })
                .occlude()
                .group(SharedString::from(format!("tab-chip-{i}")))
                .cursor_pointer()
                .items_center()
                .justify_between()
                .gap_1p5()
                .h(px(30.))
                .min_w(px(CHIP_MIN_W))
                .flex_shrink(1.)
                .pl_3()
                .pr_1p5()
                .rounded_lg()
                .when(is_active, |s| {
                    s.bg(cx.theme().secondary).text_color(cx.theme().foreground)
                })
                .when(!is_active, |s| {
                    s.text_color(cx.theme().muted_foreground)
                        .hover(|s| s.bg(cx.theme().muted))
                })
                // Faded as well as marked: a row of chips is read at a glance,
                // and the tabs holding nothing should be the quiet ones.
                .when(asleep, |s| s.opacity(0.6))
                .when(dragged, |s| s.opacity(0.75))
                .child(
                    canvas(
                        {
                            let slots = slots.clone();
                            move |bounds, _window, _cx| {
                                if let Some(slot) = slots.borrow_mut().get_mut(i) {
                                    *slot = bounds;
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                // The press is kept from the title bar under it, but it is the
                // release that switches tabs: a press that turns into a drag is
                // the tab being picked up, and picking a tab up to drop it into
                // another one must not first put it on screen.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    let double =
                        matches!(ev, gpui::ClickEvent::Mouse(e) if e.down.click_count >= 2);
                    if double {
                        window.titlebar_double_click();
                    } else {
                        this.activate(i, window, cx);
                    }
                }))
                .when_some(ssh_dot, |c, rgb| {
                    c.child(
                        div()
                            .flex_shrink_0()
                            .size(px(6.))
                            .rounded_full()
                            .bg(gpui::rgb(rgb)),
                    )
                })
                .when_some(agent, |chip, agent| {
                    chip.child(self.tab_avatar(
                        ("tab-avatar", i),
                        Some(agent),
                        agent_status,
                        agent_unread,
                        None,
                        18.,
                        cx,
                    ))
                })
                // Leading, beside the other state marks: the trailing end of a
                // chip belongs to the badge and to the close button that fades
                // in over it, and a mark parked there would vanish under the
                // pointer that came to read it.
                .when(zoomed, |chip| {
                    chip.child(self.zoom_mark(("tab-zoom", i), cx))
                })
                .when(asleep, |chip| {
                    chip.child(self.sleep_mark(("tab-asleep", i), cx))
                })
                .child(label_region)
                .when_some(badge_pos, |chip, badge_pos| {
                    chip.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_active {
                                cx.theme().foreground
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(tab_badge_label(badge_pos)),
                    )
                })
                .when(badge_pos.is_none(), |chip| {
                    let backing = if is_active {
                        cx.theme().secondary
                    } else {
                        cx.theme().muted
                    };
                    let mut fade_from = backing;
                    fade_from.a = 0.;
                    chip.child(
                        h_flex()
                            .absolute()
                            // 3 + MIN_TARGET + 3 centres the button in the 30px chip.
                            .top(px(3.))
                            .right(px(6.))
                            .opacity(0.)
                            .group_hover(SharedString::from(format!("tab-chip-{i}")), |s| {
                                s.opacity(1.)
                            })
                            .child(div().w(px(10.)).h(px(MIN_TARGET)).bg(linear_gradient(
                                90.,
                                linear_color_stop(fade_from, 0.),
                                linear_color_stop(backing, 1.),
                            )))
                            .child(
                                div().bg(backing).child(
                                    hit_target(
                                        Button::new(("tab-close", i))
                                            .icon(IconName::Close)
                                            .ghost()
                                            .xsmall(),
                                    )
                                    .tooltip(t(L10nKey::TabContextCloseTab))
                                    // Held here, because the chip behind it
                                    // switches tabs on the release too: without
                                    // this the same click closes tab `i` and
                                    // then activates whichever tab slid into
                                    // its place.
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.close_tab(i, window, cx);
                                        },
                                    )),
                                ),
                            ),
                    )
                });

            let menu_app = cx.entity().downgrade();
            let chip = chip.context_menu(move |menu, window, cx| {
                Self::tab_context_menu(menu, i, false, &menu_app, window, cx)
            });
            chips = chips.child(match &preview {
                Some(p) if p.from == i => deferred(chip.relative().left(p.held)).into_any_element(),
                Some(p) => {
                    let offset = p.offsets[i].as_f32();
                    chip.with_animation(
                        (
                            SharedString::from(format!("chip-slide-{}", p.generation)),
                            i,
                        ),
                        Animation::new(std::time::Duration::from_millis(REORDER_SLIDE_MS))
                            .with_easing(ease_out_quint()),
                        move |el, delta| el.left(px(offset * (1. - delta))),
                    )
                    .into_any_element()
                }
                None => chip.into_any_element(),
            });
        }

        let add_button = div()
            .occlude()
            .flex_shrink_0()
            .child(self.new_tab_button("tab-add", cx));
        #[cfg(test)]
        let add_button = add_button.debug_selector(|| "tab-strip-new".into());

        // What the chips leave out, said on screen. It opens the same recent
        // tabs list Ctrl+Tab raises, so the hidden tabs are one click away.
        let count_tile = (show_chips && hidden > 0).then(|| {
            let theme = cx.theme();
            let label = SharedString::from(t_plural(L10nKey::TabStripHiddenTabs, hidden, &[]));
            let slot = px(count_slot(rem));
            let glyph = move |index: usize, text: &'static str| {
                let cell = div()
                    .flex_none()
                    .w(slot)
                    .flex()
                    .justify_center()
                    .child(text);
                #[cfg(test)]
                let cell = cell.debug_selector(move || format!("tab-strip-overflow-glyph-{index}"));
                #[cfg(not(test))]
                let _ = index;
                cell
            };
            let tile = div()
                .id("tab-strip-overflow")
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .occlude()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(count_w(count_digits(hidden), rem)))
                .h(px(COUNT_H))
                .rounded_lg()
                .text_size(rems(COUNT_TEXT))
                .font_features(self.strip_metrics.features())
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .hover(|s| s.bg(theme.muted).text_color(theme.foreground))
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                // The press is kept from the title bar under it, like the
                // chips', so a click here is never the start of a window drag.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.tab_switch(true, window, cx);
                    // A click may land while a modifier is down; that
                    // modifier coming up must not commit the list.
                    this.switcher_release_hold(cx);
                }))
                .child(glyph(0, "+"))
                .children(hidden.to_string().bytes().enumerate().map(|(at, digit)| {
                    glyph(at + 1, COUNT_DIGIT_GLYPHS[usize::from(digit - b'0')])
                }));
            #[cfg(test)]
            let tile = tile.debug_selector(move || format!("tab-strip-overflow-{hidden}"));
            tile
        });

        // Same bargain the rail's own tiles keep when the Appearance switch is
        // on: present in the layout, painted only while the pointer is on the
        // bar. The New Tab button that trails the chips is left alone — it is
        // part of the row of tabs, not of the window's chrome.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let strip_chrome_shown =
            crate::ui::app::titlebar_chrome_shown(auto_hide_chrome, self.strip_chrome_hover.get());
        let rail_collapsed = !self.agent_mode_active() && !show_chips && !self.left_panel_open(cx);
        let left_group = rail_collapsed.then(|| {
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(2.))
                .ml(px(crate::ui::app::title_bar_hug_offset()))
                .when_some(crate::ui::app::window_mark(), |group, mark| {
                    group.child(
                        div()
                            .flex_shrink_0()
                            .pl(px(crate::ui::app::CONTENT_INSET
                                - crate::ui::app::tile_trailing_inset()))
                            .pr(px(4.))
                            .child(mark),
                    )
                })
                // The logo above stays put: it is the window's mark, not a
                // control, and a window that loses its identity when nobody is
                // pointing at it reads as a different window.
                .child(
                    crate::ui::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(self.new_tab_button("titlebar-add-collapsed", cx)),
                )
                .child(
                    crate::ui::app::resting_chrome(
                        div().occlude().flex_shrink_0(),
                        strip_chrome_shown,
                    )
                    .child(
                        chrome_tile(
                            Button::new("titlebar-expand-sidebar")
                                .icon(Icon::empty().path("icons/panel-left-collapsed.svg")),
                            false,
                            cx,
                        )
                        .rounded_lg()
                        .accessible_label(t(L10nKey::TabTooltipShowSidebar))
                        .tooltip_element(chord_tooltip(
                            t(L10nKey::TabTooltipShowSidebar),
                            "ToggleLeftPanel",
                            cx,
                        ))
                        .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                    ),
                )
        });

        // macOS places these controls in the open panel's own title bar, and
        // drops them while a docked document holds the right edge.
        //
        // Elsewhere, with the panel open, they stand in the band above it,
        // beside the panel's own tab row — which is painted whenever the panel
        // is, so a band that grew a button on hover read as a glitch next to
        // it. Once the panel is open they belong to its chrome, and stay.
        let right_chrome_shown = strip_chrome_shown || panel_open;
        let right_chrome = (strip_chrome && !self.agent_mode_active()).then(|| {
            crate::ui::app::resting_chrome(div().flex_shrink_0(), right_chrome_shown)
                .child(self.window_chrome(cx))
        });

        // With the tabs in the rail, the middle of the bar over the terminal
        // is the way into Search Everywhere: a field-shaped button, centred,
        // that opens the search where it stands. The rail beside it already
        // names every tab, so the bar has no title of its own to repeat. The
        // box alone takes the pointer; the rest of the bar still drags.
        //
        // Centred over the terminal column, not the bar: see `search_band`.
        // Off macOS the bar spans the workspace while a document or the detail
        // panel is docked, and centred on all of it the box landed under the
        // document's hoisted header, which has no fill to hide it.
        let docked_right = document_w
            + match panel_open {
                true => self.right_panel_px(window, cx),
                false => 0.,
            };
        let band = search_band(
            cfg!(target_os = "macos"),
            title_bar_strip_lead(window.is_fullscreen(), window.rem_size().as_f32()),
            docked_right,
            controls_w,
            rail_collapsed.then(collapsed_rail_group_w),
            (strip_chrome && docked_right <= 0.).then(trailing_chrome_tiles_w),
        );
        let centre_search = (!show_chips).then(|| {
            // The chord as text, not caps: a cap's fill is this box's own
            // grey, so on it a cap is only a gap between two letters.
            let chord = crate::ui::keymap::effective_key("TogglePalette", cx)
                .map(|spec| crate::ui::keymap::key_tokens(&spec).join(""));
            // Two equal springs either side keep the box centred; each stops
            // at the band's `clear`, and past that it is the box that gives.
            // Reserve the agent-mode controls on both sides while keeping the
            // command search centred. On narrow windows the field shrinks.
            let clear = band
                .clear
                .max(if self.agent_mode_active() { 140. } else { 110. });
            let spring = || div().flex_1().min_w(px(clear));
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(band.left))
                .right(px(band.right))
                .flex()
                .items_center()
                .child(spring())
                .child(
                    h_flex()
                        .id("titlebar-search")
                        .occlude()
                        .w(px(TITLEBAR_SEARCH_W))
                        .min_w_0()
                        // Enough of the bar that the label and its chord still
                        // fit with a document docked beside the terminal; at a
                        // half it read `Search Everywhe…` in exactly the
                        // layout people search from most.
                        .max_w(gpui::relative(0.6))
                        .h(px(TITLEBAR_SEARCH_H))
                        .items_center()
                        .gap(px(7.))
                        .pl(px(10.))
                        .pr(px(10.))
                        .rounded(px(TITLEBAR_SEARCH_RADIUS))
                        // The left rail's fill (`Surfaces::rail`, #f5f5f3 /
                        // #1e1e20 on the default pair) — not `sidebar`, which
                        // is the window's own fill and vanished into the bar.
                        .bg(titlebar_search_fill(cx))
                        .cursor_pointer()
                        .text_size(window.rem_size() * 0.8125)
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| s.text_color(cx.theme().foreground))
                        .child(Icon::new(IconName::Search).size(px(12.)).flex_shrink_0())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(t(L10nKey::AppMenuSearchEverywhere)),
                        )
                        .when_some(chord, |row, chord| {
                            row.child(div().flex_shrink_0().child(chord))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(
                            cx.listener(|this, _, window, cx| match this.search.is_some() {
                                true => this.close_search(window, cx),
                                false => this.open_search(SearchTab::All, "", window, cx),
                            }),
                        ),
                )
                .child(spring())
        });

        h_flex()
            .id("tab-strip")
            .relative()
            .items_center()
            .gap(rems(STRIP_GAP_REM))
            .when(show_chips, |this| this.w(strip_w))
            .when(!show_chips, |this| this.w_full())
            .pl_0()
            .min_w_0()
            .when_some(centre_search, |this, search| this.child(search))
            .when_some(left_group, |this, g| this.child(g))
            .when(self.agent_mode_active(), |this| {
                this.child(self.agent_view_switch(cx))
            })
            .child(chips)
            .when_some(count_tile, |this, tile| this.child(tile))
            .when(show_chips, move |this| this.child(add_button))
            .child(div().flex_1().min_w(px(GRAB_HANDLE_W)))
            .child(self.agent_mode_button(attention, cx))
            .when_some(right_chrome, |this, chrome| match chrome_band_w {
                Some(w) => this.child(
                    h_flex()
                        .flex_none()
                        .w(px(w))
                        .items_center()
                        .pl(px(tile_trailing_inset()))
                        .child(chrome),
                ),
                None => this.child(chrome),
            })
            .when(auto_hide_chrome, |this| {
                this.child(crate::ui::app::hover_sheet(
                    "strip-chrome-hover",
                    &self.strip_chrome_hover,
                ))
            })
    }
}

/// The tab menu's SSH row, against real tabs in a real window.
///
/// `PopupMenu` keeps its items to itself — nothing outside `gpui_component` can
/// read back what a built menu says — so these drive the predicate the menu
/// branches on instead, which is where every decision about the row is made.
///
/// Ungated: `test_window::harness` and `quiet_test_pane` both run on Windows,
/// and a `unix` gate here would skip the one platform this was written on.
/// A tab's avatar — agent disc and status dot, or the terminal glyph — for
/// surfaces that draw a tab without owning the strip. It does not blink: a
/// working agent's dot holds steady outside the strip's own clock.
pub(crate) fn avatar(
    id: impl Into<gpui::ElementId>,
    avatar: crate::ui::search::Avatar,
    size: f32,
    cx: &App,
) -> gpui::AnyElement {
    avatar_disc(id, avatar, size, true, true, cx)
}

fn avatar_disc(
    id: impl Into<gpui::ElementId>,
    crate::ui::search::Avatar {
        agent,
        status,
        unread,
        ssh,
    }: crate::ui::search::Avatar,
    size: f32,
    blink_on: bool,
    badge: bool,
    cx: &App,
) -> gpui::AnyElement {
    // The wrapper positions; the disc below carries the radius.
    // `status_dot` hangs itself off the edge with negative offsets — that
    // overhang is what makes it a badge on the avatar rather than a notch
    // in it — and as a child of the rounded element the overhang was
    // clipped along the arc, leaving a crescent.
    let base = div().id(id).flex_shrink_0().relative().size(px(size));
    // Fill, hairline and mark all live here, so the radius only ever clips
    // the disc's own paint.
    let disc = || {
        div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
    };
    match agent {
        Some(agent) => {
            let hollow = status == Some(crate::core::cli_agent::AgentStatus::Waiting);
            let dot = status
                .filter(|_| badge)
                .and_then(|s| s.dot_rgb())
                .map(|rgb| {
                    // A working agent's dot blinks, so a column of tabs
                    // says at a glance which ones are still going.
                    let faded =
                        status == Some(crate::core::cli_agent::AgentStatus::Working) && !blink_on;
                    Tty59App::status_dot(rgb, unread, size, cx.theme().background, hollow, faded)
                });
            // Which agent this is, and what it wants, were carried entirely
            // by a brand hue and a nine-pixel dot. Say it in words too.
            let tip = match agent_status_label(status) {
                Some(state) => format!("{} — {state}", agent.display_name()),
                None => agent.display_name().to_string(),
            };
            // The disc is a solid fill of the agent's brand on every
            // row, lit or not: a tint reads as a disabled tab, and the
            // colour is how the eye tells one agent from another down a
            // column of twenty.
            let accent = agent.accent_rgb();
            let surface = cx.theme().background;
            base.child(
                disc()
                    .bg(gpui::rgb(accent))
                    // Codex and Grok are both pure black, which is the
                    // window fill on a dark theme — the disc dissolves and
                    // leaves the glyph floating. A hairline keeps it a disc
                    // in any theme.
                    .when(crate::ui::presets::needs_edge(accent, surface), |d| {
                        d.border_1().border_color(cx.theme().border)
                    })
                    .child(
                        gpui::svg()
                            .path(agent.icon_path())
                            .size(px(size * 0.54))
                            // SVG assets render as a single-colour mask, so
                            // the mark's colour comes from the agent rather
                            // than from the file. The tray icon reads the
                            // same answer.
                            .text_color(gpui::rgb(agent.icon_rgb())),
                    ),
            )
            .when_some(dot, |b, dot| b.child(dot))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .into_any_element()
        }
        None => base
            .child(
                disc().bg(cx.theme().muted).child(
                    gpui::svg()
                        .path("icons/terminal.svg")
                        .size(px(size * 0.56))
                        .text_color(cx.theme().foreground.opacity(0.65)),
                ),
            )
            .when_some(ssh, |b, rgb| {
                b.child(Tty59App::status_dot(
                    rgb,
                    0,
                    size,
                    cx.theme().background,
                    false,
                    false,
                ))
            })
            .into_any_element(),
    }
}

#[cfg(test)]
mod ssh_host_row_tests {
    use crate::core::config::Config;
    use crate::core::session::RemoteTarget;
    use crate::core::ssh_profile::SshProfile;
    use crate::daemon::protocol::SshProxy;
    use crate::terminal::view::{
        quiet_test_pane, quiet_test_ssh_pane, quiet_test_ssh_pane_of, quiet_test_ssh_pane_with,
    };
    use crate::ui::app::{Tab, test_window::harness};
    use crate::ui::i18n::{L10nKey, set_locale, t};
    use crate::ui::pane::{Pane, PaneSlot};
    use crate::ui::ssh_connect::TabHostForm;
    use gpui::TestAppContext;

    #[gpui::test]
    fn only_a_tab_on_an_ssh_host_is_offered_the_host_form(cx: &mut TestAppContext) {
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let saved = uuid::Uuid::new_v4();

        // Held for the life of the test: dropping the daemon end of a pane's
        // transport tears the pane down under the assertions.
        let _ends = app.update_in(&mut vcx, |app, window, cx| {
            let mut cfg = cx.global::<Config>().clone();
            let mut profile = SshProfile::new("build-box");
            profile.id = saved;
            profile.user = "me".to_string();
            profile.host = "build-box".to_string();
            cfg.ssh_profiles = vec![profile];
            cx.set_global(cfg);

            let (local, a) = quiet_test_pane(1, window, cx);
            let (dialled, b) = quiet_test_ssh_pane(2, window, cx);
            let (from_host, c) = quiet_test_ssh_pane_of(3, Some(saved), window, cx);
            for view in [local, dialled, from_host] {
                app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            }
            app.active = 0;
            cx.notify();
            (a, b, c)
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            // A local shell has no connection to edit, so the menu it opens is
            // the one it always was.
            assert_eq!(
                app.tab_ssh_host_form(0, window, cx),
                None,
                "a local tab was offered an SSH host form"
            );

            // An address typed by hand is worth keeping, not editing: there is
            // no saved host behind it yet, so the live session itself is what
            // the form opens on.
            let (form, label) = app
                .tab_ssh_host_form(1, window, cx)
                .expect("a tab dialled by hand offers to save the host");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("a hand-dialled tab must offer its own session, not a bare address");
            };
            assert_eq!(
                (spec.user.as_str(), spec.host.as_str(), spec.port),
                ("me", "build-box", 22)
            );
            assert_eq!(label, t(L10nKey::SwitcherSaveAsHost));

            // One opened from a saved host edits that host — by its id, so the
            // form lands on the record the connection actually came from.
            let (form, label) = app
                .tab_ssh_host_form(2, window, cx)
                .expect("a tab on a saved host offers to edit it");
            assert_eq!(
                form,
                TabHostForm::Saved(RemoteTarget::Profile { id: saved })
            );
            assert_eq!(label, t(L10nKey::SwitcherEditHost));
        });
    }

    #[gpui::test]
    fn a_host_deleted_under_a_live_tab_is_offered_back_as_a_new_one(cx: &mut TestAppContext) {
        // The id a pane carries is the one it was spawned with, and a quick
        // connection is handed a fresh uuid on its way to the daemon. Trusting
        // the id alone would open the form on a host that is not there.
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let _end = app.update_in(&mut vcx, |app, window, cx| {
            let (view, end) = quiet_test_ssh_pane_of(1, Some(uuid::Uuid::new_v4()), window, cx);
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = 0;
            cx.notify();
            end
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let (form, label) = app
                .tab_ssh_host_form(0, window, cx)
                .expect("an unresolvable profile id still names a host worth keeping");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("a dangling profile id must not open a form on a host that is gone");
            };
            assert_eq!(
                (spec.user.as_str(), spec.host.as_str(), spec.port),
                ("me", "build-box", 22)
            );
            assert_eq!(label, t(L10nKey::SwitcherSaveAsHost));
        });
    }

    /// The row says "Save as SSH Host", and a host saved without the proxy it
    /// was reached through is a host that will not connect. What the session
    /// was dialled with has to reach the form whole — an address is only the
    /// part of it that fits in `user@host:port`.
    #[gpui::test]
    fn saving_a_hand_dialled_tab_keeps_what_it_was_dialled_with(cx: &mut TestAppContext) {
        set_locale("en");
        let (app, mut vcx) = harness(cx);
        let _end = app.update_in(&mut vcx, |app, window, cx| {
            let mut spec: crate::daemon::protocol::NativeSshSpec = serde_json::from_str(
                r#"{"host":"build-box","port":2222,"user":"me","auth_mode":"auto"}"#,
            )
            .expect("a minimal NativeSshSpec decodes");
            spec.proxy = SshProxy::Socks {
                host: "127.0.0.1".to_string(),
                port: 1080,
            };
            spec.identity_files = vec!["/keys/id_ed25519".to_string()];
            spec.login_script = vec!["tmux attach".to_string()];
            let (view, end) = quiet_test_ssh_pane_with(1, spec, window, cx);
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = 0;
            cx.notify();
            end
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let (form, _) = app
                .tab_ssh_host_form(0, window, cx)
                .expect("a hand-dialled tab offers to save the host");
            let TabHostForm::Unsaved(spec) = form else {
                panic!("nothing here is saved, so nothing here is an edit");
            };
            assert_eq!(
                spec.proxy,
                SshProxy::Socks {
                    host: "127.0.0.1".to_string(),
                    port: 1080,
                },
                "the proxy the session was reached through was dropped on the way to the form"
            );
            assert_eq!(spec.identity_files, vec!["/keys/id_ed25519".to_string()]);
            assert_eq!(spec.login_script, vec!["tmux attach".to_string()]);
            assert_eq!(spec.port, 2222, "a non-default port is part of the address");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::TRAFFIC_LIGHTS_LEAD;
    use gpui::TestAppContext;
    use std::path::Path;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn unread_counts_only_label_multiple_results() {
        assert_eq!(unread_count_label(0), None);
        assert_eq!(unread_count_label(1), None);
        assert_eq!(unread_count_label(2).as_deref(), Some("2"));
        assert_eq!(unread_count_label(12).as_deref(), Some("9"));
    }

    #[gpui::test]
    fn one_unread_result_draws_circles_and_two_draw_wide_pills(cx: &mut TestAppContext) {
        struct StatusMarks(gpui::Entity<Tty59App>);
        impl gpui::Render for StatusMarks {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let app = self.0.read(cx);
                let status = crate::core::cli_agent::AgentStatus::Done;
                let surface = cx.theme().background;
                div().flex().children((0..=2).flat_map(|unread| {
                    [
                        div()
                            .relative()
                            .size(px(32.))
                            .children(app.row_status_dot(Some(status), unread, surface))
                            .into_any_element(),
                        div()
                            .relative()
                            .size(px(32.))
                            .child(Tty59App::status_dot(
                                status.dot_rgb().unwrap(),
                                unread,
                                18.,
                                surface,
                                false,
                                false,
                            ))
                            .into_any_element(),
                    ]
                }))
            }
        }
        let (app, _app_window) = crate::ui::app::test_window::harness(cx);
        let window = cx.add_window(|_, _| StatusMarks(app));
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for (circles, counts, two_circle, two_count) in [
            (
                ["row-status-circle-0", "row-status-circle-1"],
                ["row-status-count-0", "row-status-count-1"],
                "row-status-circle-2",
                "row-status-count-2",
            ),
            (
                ["avatar-status-circle-0", "avatar-status-circle-1"],
                ["avatar-status-count-0", "avatar-status-count-1"],
                "avatar-status-circle-2",
                "avatar-status-count-2",
            ),
        ] {
            for circle in circles {
                assert!(vcx.debug_bounds(circle).is_some(), "{circle} was painted");
            }
            for count in counts {
                assert!(vcx.debug_bounds(count).is_none(), "{count} was not painted");
            }
            assert!(vcx.debug_bounds(two_circle).is_none());
            let pill = vcx
                .debug_bounds(two_count)
                .expect("two unread results paint a count pill");
            assert!(
                pill.size.width >= px(18.),
                "the count has room on both sides"
            );
            assert!(pill.size.height >= px(13.));
        }
    }

    /// #1169: under Linux client-side decorations the viewport measures the
    /// whole surface, shadow included, while the title bar row sits inside the
    /// frame padding. A strip sized off the raw viewport overran the frame by
    /// both paddings and pushed the close button out past the right edge.
    #[test]
    fn the_strip_and_window_buttons_fit_inside_the_csd_frame() {
        let (lead, controls) = (12., 102.);
        let pad = Edges::all(px(12.));
        let strip = strip_width(px(1200.), pad, lead + controls, 140.);
        assert!(
            px(lead) + strip + px(controls) <= px(1200. - 12. - 12.),
            "strip {strip:?} leaves the window buttons outside the frame"
        );
        // No frame padding (server-side decorations, macOS, Windows, or a
        // maximized/tiled CSD window): the strip keeps the width it always had.
        let bare = strip_width(px(1200.), Edges::all(px(0.)), lead + controls, 140.);
        assert_eq!(bare, px(1200. - lead - controls));
    }

    /// Where a band lands in the terminal column, for a strip that starts
    /// `lead` in and ends `strip_end` from the column's left edge.
    fn band_in_column(band: SearchBand, lead: f32, strip_end: f32) -> (f32, f32) {
        (lead + band.left, strip_end - band.right)
    }

    /// #1033, with the reporter's window: 1946pt wide, the rail collapsed.
    /// The strip started 80pt in, after the traffic lights, and centred on it
    /// the box sat at 1013 — 40pt right of the window's middle at 973.
    #[test]
    fn with_the_rail_collapsed_on_macos_the_search_box_centres_on_the_window() {
        let (w, lead) = (1946., TRAFFIC_LIGHTS_LEAD);
        let band = search_band(true, lead, 0., 0., Some(58.), Some(49.));
        let (l, r) = band_in_column(band, lead, w);
        assert_eq!((l, r), (0., w));
        assert_eq!((l + r) / 2., 973.);
        // Clear of the traffic lights and the two tiles after them, at both
        // ends so that clearing them does not pull the box off centre.
        assert_eq!(band.clear, lead + 58. + TITLEBAR_SEARCH_CLEAR_GAP);
    }

    /// With the rail open, the title bar sits in the terminal column but
    /// still leaves the traffic lights' lead before the strip, over nothing
    /// — the lights are over the rail. The band reaches back over that too,
    /// and centres on the column.
    #[test]
    fn with_the_rail_open_on_macos_the_search_box_centres_on_the_terminal_column() {
        let (column, lead) = (1200., TRAFFIC_LIGHTS_LEAD);
        let band = search_band(true, lead, 0., 0., None, Some(49.));
        assert_eq!(band_in_column(band, lead, column), (0., column));
        assert_eq!(band.clear, 49. + TITLEBAR_SEARCH_CLEAR_GAP);

        // A docked panel on macOS is beside the column, not under its bar:
        // nothing to subtract from the strip's end.
        let docked = search_band(true, lead, 360., 0., None, None);
        assert_eq!(band_in_column(docked, lead, column), (0., column));
        assert_eq!(docked.clear, 0.);
    }

    /// Fullscreen keeps the traffic lights' lead and adds the bar's own inset.
    #[test]
    fn fullscreen_on_macos_reaches_back_over_the_bars_extra_inset_as_well() {
        let lead = TRAFFIC_LIGHTS_LEAD + 12.;
        let band = search_band(true, lead, 0., 0., Some(58.), Some(49.));
        assert_eq!(band_in_column(band, lead, 1440.), (0., 1440.));
    }

    /// Off macOS the bar spans the docked columns and stops short of the
    /// window buttons; the band ends where the terminal column does, as it
    /// always has, and now starts where it does too.
    #[test]
    fn off_macos_the_band_is_the_terminal_column_beside_a_docked_panel() {
        let (w, lead, controls, panel) = (1600., 12., 102., 360.);
        let strip_end = w - controls;
        let band = search_band(false, lead, panel, controls, None, None);
        assert_eq!(band.right, panel - controls);
        assert_eq!(band_in_column(band, lead, strip_end), (0., w - panel));

        // Nothing docked: the band runs to the end of the strip, short of
        // the window buttons, and keeps clear of the tiles standing there.
        let bare = search_band(false, lead, 0., controls, None, Some(38.));
        assert_eq!(band_in_column(bare, lead, strip_end), (0., strip_end));
        assert_eq!(bare.clear, 38. + TITLEBAR_SEARCH_CLEAR_GAP);

        // A panel narrower than the buttons leaves no room to take off.
        let thin = search_band(false, lead, 80., controls, None, None);
        assert_eq!(thin.right, 0.);
    }

    /// Most of these tests are about where a title is *cut*, not about what
    /// `~` means: the paths they pass either already start with `~` or are
    /// nowhere near anybody's home. Naming no home keeps the assertions off
    /// the process environment — and is what a title of unknown provenance
    /// gets in the app too (#580).
    fn short_title(raw: &str) -> String {
        super::short_title(raw, None)
    }

    fn host(name: &str, user: &str, addr: &str) -> crate::core::ssh_profile::SshProfile {
        let mut p = crate::core::ssh_profile::SshProfile::new(name);
        p.user = user.to_string();
        p.host = addr.to_string();
        p
    }

    #[test]
    fn the_new_tab_menu_stops_naming_hosts_before_it_becomes_a_list() {
        // Frecency has already put the useful ones first by the time this
        // runs, so the cut can only ever drop the tail.
        let many: Vec<_> = (0..MENU_HOSTS + 4)
            .map(|i| host(&format!("box-{i}"), "dev", &format!("10.0.0.{i}")))
            .collect();
        let rows = menu_hosts(many);
        assert_eq!(rows.len(), MENU_HOSTS);
        assert_eq!(rows[0].1, "box-0", "the order handed in is the order shown");
        assert_eq!(rows[0].2, "dev@10.0.0.0");
    }

    #[test]
    fn a_host_saved_without_a_name_still_says_where_it_goes() {
        // Settings lets a host be saved on its address alone. A row that led
        // with an empty name would be a blank line you could click.
        let rows = menu_hosts(vec![host("", "root", "build.lan")]);
        assert_eq!(rows[0].1, "root@build.lan");
        // And it says it once: the endpoint has already led the row, so
        // repeating it in the note column would be a row talking to itself.
        assert_eq!(rows[0].2, "");
    }

    #[test]
    fn a_host_named_after_its_own_address_does_not_say_it_twice() {
        // Quick Connect saves a host under the target that was typed, so this
        // is the ordinary shape of a host nobody has renamed.
        let rows = menu_hosts(vec![host("deploy@10.0.0.5", "deploy", "10.0.0.5")]);
        assert_eq!(rows[0].1, "deploy@10.0.0.5");
        assert_eq!(rows[0].2, "");
    }

    #[test]
    fn only_alt_turns_a_new_tab_row_into_a_split() {
        use crate::ui::app::SpawnWhere;
        use gpui::Modifiers;
        assert_eq!(
            SpawnWhere::from_modifiers(Modifiers::none()),
            SpawnWhere::NewTab
        );
        assert_eq!(
            SpawnWhere::from_modifiers(Modifiers::alt()),
            SpawnWhere::Split
        );
        // One modifier means one thing. Every other one a hand might be
        // resting on leaves the row doing what it says it does.
        assert_eq!(
            SpawnWhere::from_modifiers(Modifiers::secondary_key()),
            SpawnWhere::NewTab
        );
        assert_eq!(
            SpawnWhere::from_modifiers(Modifiers::shift()),
            SpawnWhere::NewTab
        );
    }

    #[test]
    fn every_visible_agent_state_has_words_for_it() {
        use crate::core::cli_agent::AgentStatus;
        crate::ui::i18n::set_locale("en");
        // Idle draws no dot, so it has nothing to name.
        assert_eq!(agent_status_label(None), None);
        assert_eq!(agent_status_label(Some(AgentStatus::Idle)), None);
        // Every state that does draw a dot can be read out loud.
        for status in [
            AgentStatus::Working,
            AgentStatus::Waiting,
            AgentStatus::Done,
        ] {
            assert!(status.dot_rgb().is_some());
            assert!(
                agent_status_label(Some(status)).is_some_and(|s| !s.is_empty()),
                "{status:?} paints a dot with no words behind it"
            );
        }
        // Waiting is the state worth acting on; it must not read as Done.
        assert_ne!(
            agent_status_label(Some(AgentStatus::Waiting)),
            agent_status_label(Some(AgentStatus::Done))
        );
    }

    #[test]
    fn a_brand_disc_that_matches_the_window_gets_an_edge() {
        use crate::ui::presets::needs_edge;
        let dark: gpui::Hsla = gpui::rgb(0x111111).into();
        let light: gpui::Hsla = gpui::rgb(0xffffff).into();
        let codex = crate::core::cli_agent::CLIAgent::Codex.accent_rgb();
        let claude = crate::core::cli_agent::CLIAgent::Claude.accent_rgb();

        assert_eq!(codex, 0x000000, "Codex's disc is pure black");
        assert!(
            needs_edge(codex, dark),
            "a black disc on a dark window is not a disc"
        );
        assert!(!needs_edge(codex, light));
        assert!(!needs_edge(claude, dark) && !needs_edge(claude, light));
    }

    #[test]
    fn short_title_strips_user_host_and_shows_shallow_path_in_full() {
        assert_eq!(short_title("user@host:~/projects/app"), "~/projects/app");
        // Debian's stock bash title, which spaces the path off the colon.
        assert_eq!(short_title("user@host: ~/projects/app"), "~/projects/app");
        assert_eq!(short_title("/usr/local/bin"), "/usr/local/bin");
        assert_eq!(short_title("plain"), "plain");
    }

    /// A title shortens under the home of the machine it came from, and
    /// under no other (#580).
    #[test]
    fn short_title_shortens_under_the_home_it_was_given() {
        let server = Path::new("/home/deploy");
        assert_eq!(
            super::short_title("/home/deploy/app", Some(server)),
            "~/app"
        );
        // This machine's home is not a stand-in for the server's: the same
        // path stays whole when the home naming it is somewhere else.
        assert_eq!(
            super::short_title("/home/deploy/app", Some(Path::new("/Users/thomas"))),
            "/home/deploy/app"
        );
        // And a pane nothing here can place — no link to its host, or a
        // shell that has ssh'd on — shortens against nothing.
        assert_eq!(
            super::short_title("/home/deploy/app", None),
            "/home/deploy/app"
        );
    }

    /// The name a freshly dialled SSH pane wears until the remote shell says
    /// otherwise. Cutting at the colon left the tab reading "2222" (#438).
    #[test]
    fn short_title_keeps_an_ssh_address_whole() {
        assert_eq!(short_title("deploy@10.0.0.5:2222"), "deploy@10.0.0.5:2222");
        assert_eq!(short_title("root@prod"), "root@prod");
        assert_eq!(short_title("prod-web"), "prod-web");
        // Only a port stops the cut: a drive letter is still a path, and this
        // is the title tty59's own pwsh integration writes on Windows.
        assert_eq!(short_title(r"ann@BOX:C:/Users/app"), r"C:/Users/app");
    }

    #[test]
    fn short_title_truncates_deep_paths_to_trailing_segments() {
        assert_eq!(
            short_title("user@host:~/repo/025/tty59"),
            "…/repo/025/tty59"
        );
        assert_eq!(short_title("/usr/local/share/man"), "…/local/share/man");
        assert_eq!(short_title("a/b/c/d"), "…/b/c/d");
    }

    #[test]
    fn short_title_keeps_home_tilde_and_normalizes_trailing_slash() {
        assert_eq!(short_title("user@host:~"), "~");
        assert_eq!(short_title("~"), "~");
        assert_eq!(short_title("a/b/c/"), "a/b/c");
    }

    #[test]
    fn short_title_blank_input_is_empty_and_long_names_are_clamped() {
        assert_eq!(short_title("   "), "");
        let long = "a".repeat(50);
        let out = short_title(&long);
        assert_eq!(out.chars().count(), 41);
        assert!(out.ends_with('…'));
    }

    /// `TestAppContext` shapes through gpui's `NoopTextSystem`, where every
    /// glyph is exactly one em — weight-agnostic and, more to the point,
    /// CJK-agnostic. That keeps these tests identical on all three CI targets,
    /// but it also means they cannot speak to the proportional and mixed-script
    /// widths the elision exists for: what they pin is the contract — which
    /// parts of a label must survive, and that the result fits its budget.
    fn elide_setup(cx: &mut TestAppContext) -> (gpui::WindowTextSystem, gpui::Font, f32) {
        let size = 14.;
        (
            gpui::WindowTextSystem::new(cx.text_system().clone()),
            gpui::Font::default(),
            size,
        )
    }

    #[gpui::test]
    fn elide_path_fits_shallow_paths_untouched(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let path = "~/tty59";
        let max = measure_text(&ts, &font, size, path) + 1.;
        assert_eq!(elide_path_keep_tail(&ts, &font, size, path, max), "~/tty59");
    }

    #[gpui::test]
    fn elide_path_shows_the_whole_deep_path_when_the_budget_allows(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        // A wide sidebar must not elide a deep path: only the width may
        // decide, never a fixed segment cap.
        let path = "E:/work/toolbox/crates/tty59-core/src/client";
        let max = measure_text(&ts, &font, size, path) + 1.;
        assert_eq!(elide_path_keep_tail(&ts, &font, size, path, max), path);
    }

    #[gpui::test]
    fn elide_path_keeps_drive_tail_and_budget(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let path = "E:/work/toolbox/src/ui/tab_sidebar.rs";
        let max = 200.;
        assert!(
            measure_text(&ts, &font, size, path) > max,
            "the fixture has to be wider than the budget to exercise elision"
        );
        let out = elide_path_keep_tail(&ts, &font, size, path, max);
        assert!(out.starts_with("E:/…/"), "drive letter survives: {out}");
        assert!(
            out.ends_with("tab_sidebar.rs"),
            "the file name always survives: {out}"
        );
        assert!(
            measure_text(&ts, &font, size, &out) <= max,
            "the elided label fits the budget"
        );
    }

    #[gpui::test]
    fn elide_path_keeps_tilde_and_leading_slash(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let home = "~/projects/toolbox/src/ui/tab_sidebar.rs";
        assert!(
            measure_text(&ts, &font, size, home) > 200.,
            "the fixture has to be wider than the budget to exercise elision"
        );
        let out = elide_path_keep_tail(&ts, &font, size, home, 200.);
        assert!(out.starts_with("~/…/"), "tilde root survives: {out}");
        assert!(out.ends_with("tab_sidebar.rs"));

        let abs = "/usr/local/share/man/man1/git.1";
        assert!(
            measure_text(&ts, &font, size, abs) > 120.,
            "the fixture has to be wider than the budget to exercise elision"
        );
        let out = elide_path_keep_tail(&ts, &font, size, abs, 120.);
        assert!(out.starts_with("/…/"), "absolute root survives: {out}");
        assert!(out.ends_with("git.1"));
    }

    #[gpui::test]
    fn elide_path_tears_only_the_last_segment_as_a_last_resort(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let path = "E:/supercalifragilisticexpialidocious";
        let max = 60.;
        assert!(measure_text(&ts, &font, size, path) > max);
        let out = elide_path_keep_tail(&ts, &font, size, path, max);
        assert!(out.starts_with('…'), "a torn segment reads as torn: {out}");
        assert!(
            out.chars().nth(1) != Some('/'),
            "no slash after a torn segment: {out}"
        );
        assert!(out.ends_with('s'), "the word's tail survives: {out}");
        assert!(measure_text(&ts, &font, size, &out) <= max);
    }

    #[gpui::test]
    fn elide_edges_keeps_both_ends_of_a_branch(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "window-transparency-backdrop";
        let max = 140.;
        assert!(measure_text(&ts, &font, size, branch) > max);
        let out = elide_keep_edges(&ts, &font, size, branch, max);
        assert!(out.starts_with("window-"), "head survives: {out}");
        assert!(out.ends_with("backdrop"), "tail survives: {out}");
        assert!(out.contains('…'));
        assert!(measure_text(&ts, &font, size, &out) <= max);
        assert!(out.chars().count() < branch.chars().count());
    }

    #[gpui::test]
    fn elide_end_keeps_the_head_of_a_branch(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "fix/rpc-proxy-and-error-classification";
        let max = 140.;
        let out = elide_end_clusters(&ts, &font, size, branch, max);
        assert!(out.starts_with("fix/rpc-"), "head survives: {out}");
        assert!(out.ends_with('…') && out.matches('…').count() == 1, "{out}");
        assert!(measure_text(&ts, &font, size, &out) <= max);
        let fits = measure_text(&ts, &font, size, branch);
        assert_eq!(elide_end_clusters(&ts, &font, size, branch, fits), branch);
    }

    #[gpui::test]
    fn elide_edges_leaves_short_branches_alone(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "main";
        let max = measure_text(&ts, &font, size, branch) + 1.;
        assert_eq!(elide_keep_edges(&ts, &font, size, branch, max), "main");
    }

    #[gpui::test]
    fn elide_edges_falls_back_to_a_tail_sliver_when_the_head_cannot_fit(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "window-transparency-backdrop";
        let out = elide_keep_edges(&ts, &font, size, branch, 30.);
        assert!(out.starts_with('…'));
        assert!(measure_text(&ts, &font, size, &out) <= 30.);
    }

    #[gpui::test]
    fn elide_path_cuts_windows_backslash_paths_on_segments(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let path = r"C:\Users\dev\AppData\Local\Temp\verify-build";
        let max = 200.;
        assert!(measure_text(&ts, &font, size, path) > max);
        let out = elide_path_keep_tail(&ts, &font, size, path, max);
        assert!(out.starts_with(r"C:\…\"), "drive letter survives: {out}");
        assert!(
            out.ends_with("verify-build"),
            "the leaf segment survives: {out}"
        );
        assert!(measure_text(&ts, &font, size, &out) <= max);
    }

    /// One tab must not spell its location two ways depending on how wide the
    /// sidebar happens to be.
    #[gpui::test]
    fn elide_path_keeps_the_separator_the_path_arrived_with(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let windows = r"C:\Users\dev\projects\toolbox\src\ui\app.rs";
        let wide = measure_text(&ts, &font, size, windows) + 1.;
        assert_eq!(
            elide_path_keep_tail(&ts, &font, size, windows, wide),
            windows,
            "a path that fits is left exactly as it arrived"
        );
        let out = elide_path_keep_tail(&ts, &font, size, windows, 120.);
        assert!(!out.contains('/'), "no forward slash creeps in: {out}");

        let unix = "/home/dev/projects/toolbox/src/ui/app.rs";
        let out = elide_path_keep_tail(&ts, &font, size, unix, 120.);
        assert!(!out.contains('\\'), "no backslash creeps in: {out}");
    }

    /// A branch with no `-`, `_`, `/` or `.` in reach used to lose its head
    /// entirely, which is the one thing this function promises not to do.
    #[gpui::test]
    fn elide_edges_keeps_a_head_on_a_separatorless_token(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "verylongbranchnamewithoutseps";
        for max in [60., 80., 100., 120.] {
            let out = elide_keep_edges(&ts, &font, size, branch, max);
            assert!(
                out.starts_with('v'),
                "head survives at {max}px: {out}",
                max = max
            );
            assert!(out.ends_with('s'), "tail survives at {max}px: {out}");
            assert!(measure_text(&ts, &font, size, &out) <= max);
        }
    }

    /// A head that fits but leaves nothing behind the ellipsis says less than
    /// a shorter head that keeps the identifying tail.
    #[gpui::test]
    fn elide_edges_gives_up_head_room_to_keep_a_tail(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let branch = "feature/some-really-long-thing";
        let max = 80.;
        assert!(measure_text(&ts, &font, size, branch) > max);
        let out = elide_keep_edges(&ts, &font, size, branch, max);
        assert!(
            !out.ends_with('…'),
            "the tail is never traded away for a longer head: {out}"
        );
        assert!(out.ends_with('g'), "the identifying tail survives: {out}");
        assert!(measure_text(&ts, &font, size, &out) <= max);
    }

    /// The sidebar title is not always a path. `elide_label` has to notice,
    /// because the path rule drops the head — and for a command line or a name
    /// the user typed, the head is the part that names it.
    #[gpui::test]
    fn elide_label_keeps_the_head_of_a_non_path(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        for text in ["Backend server logs", "npm run dev -- --watch"] {
            let max = 90.;
            assert!(measure_text(&ts, &font, size, text) > max);
            let out = elide_label(&ts, &font, size, text, max);
            let first = text.chars().next().unwrap();
            assert!(
                out.starts_with(first),
                "a non-path keeps its head: {out} (from {text})"
            );
            assert!(measure_text(&ts, &font, size, &out) <= max);
        }
    }

    /// …while a path still gets the tail-first treatment through the same
    /// entry point.
    #[gpui::test]
    fn elide_label_still_keeps_the_tail_of_a_path(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        let path = "~/projects/toolbox/src/ui/tab_sidebar.rs";
        let out = elide_label(&ts, &font, size, path, 200.);
        assert!(out.starts_with("~/…/"), "root survives: {out}");
        assert!(out.ends_with("tab_sidebar.rs"), "leaf survives: {out}");
    }

    #[test]
    fn short_title_cuts_windows_paths_on_backslashes() {
        assert_eq!(
            short_title(r"C:\Users\dev\projects\app"),
            r"…\dev\projects\app"
        );
        assert_eq!(
            short_title(r"C:\Users\dev\repo\deep\path\src\ui"),
            r"…\path\src\ui"
        );
        // A shallow Windows path keeps its drive and its backslashes.
        assert_eq!(short_title(r"C:\Users\app"), r"C:\Users\app");
    }

    /// Every way of slicing `text` that lands on a grapheme-cluster boundary.
    fn cluster_prefixes(text: &str) -> Vec<String> {
        let clusters: Vec<&str> = text.graphemes(true).collect();
        (0..=clusters.len())
            .map(|n| clusters[..n].concat())
            .collect()
    }

    fn cluster_suffixes(text: &str) -> Vec<String> {
        let clusters: Vec<&str> = text.graphemes(true).collect();
        (0..=clusters.len())
            .map(|n| clusters[clusters.len() - n..].concat())
            .collect()
    }

    /// An elision may only drop whole grapheme clusters, so whatever survives
    /// on either side of the ellipsis has to be a cluster-aligned prefix and
    /// suffix of what went in. Slicing by `char` instead passes every width
    /// check and still tears `👨‍👩‍👧` into a dangling joiner, strips the
    /// variation selector off `❤️`, or leaves half of `🇨🇳` to render as a
    /// bare letter.
    #[track_caller]
    fn assert_cut_on_cluster_boundaries(input: &str, out: &str, max: f32) {
        let Some((head, tail)) = out.split_once('…') else {
            assert_eq!(out, input, "an unelided label comes back verbatim");
            return;
        };
        assert!(
            cluster_prefixes(input).iter().any(|p| p == head),
            "head {head:?} is not a cluster-aligned prefix of {input:?} (@{max}px)"
        );
        assert!(
            cluster_suffixes(input).iter().any(|s| s == tail),
            "tail {tail:?} is not a cluster-aligned suffix of {input:?} (@{max}px)"
        );
    }

    /// Fixtures whose clusters are wider than one `char`, placed so that a
    /// `char`-indexed cut lands inside one at some width.
    const CLUSTER_FIXTURES: [&str; 7] = [
        "ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}cdefghijklmnop",
        "release-notes-final-ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}",
        "abcdef\u{2764}\u{fe0f}ghijklmnopqr",
        "long-branch-name-x\u{2764}\u{fe0f}",
        "abcdef\u{1F1E8}\u{1F1F3}ghijklmnopqr",
        "abcde\u{301}fghijklmnopqrst",
        "review-\u{1F44D}\u{1F3FD}-approved-changes",
    ];

    #[gpui::test]
    fn elide_edges_cuts_only_on_cluster_boundaries(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        for text in CLUSTER_FIXTURES {
            let mut max = 30.;
            while max <= 200. {
                let out = elide_keep_edges(&ts, &font, size, text, max);
                assert_cut_on_cluster_boundaries(text, &out, max);
                assert!(
                    measure_text(&ts, &font, size, &out) <= max,
                    "{out:?} still has to fit its budget (@{max}px)"
                );
                max += 2.;
            }
        }
    }

    /// The path elision tears its last segment character by character as a
    /// last resort; that resort has to respect clusters too.
    #[gpui::test]
    fn elide_path_tears_its_last_segment_on_cluster_boundaries(cx: &mut TestAppContext) {
        let (ts, font, size) = elide_setup(cx);
        for leaf in CLUSTER_FIXTURES {
            let path = format!("~/projects/toolbox/{leaf}");
            let mut max = 30.;
            while max <= 120. {
                let out = elide_path_keep_tail(&ts, &font, size, &path, max);
                // Whatever it settled on, the tail after the ellipsis has to
                // be cluster-aligned against the path it came from.
                if let Some((_, tail)) = out.split_once('…') {
                    let tail = tail.trim_start_matches('/');
                    assert!(
                        cluster_suffixes(&path).iter().any(|s| s == tail),
                        "tail {tail:?} is not a cluster-aligned suffix of {path:?} (@{max}px)"
                    );
                }
                max += 2.;
            }
        }
    }

    /// `short_title`'s 40-glyph clamp is the other `char`-indexed cut.
    #[test]
    fn short_title_clamps_on_cluster_boundaries() {
        for tail in ["\u{1F1E8}\u{1F1F3}-suffix", "\u{2764}\u{fe0f}-suffix"] {
            for pad in 37..=41 {
                let name = format!("{}{tail}", "a".repeat(pad));
                let out = short_title(&name);
                let Some(body) = out.strip_suffix('…') else {
                    continue;
                };
                assert!(
                    cluster_prefixes(&name).iter().any(|p| p == body),
                    "clamped to {body:?}, not a cluster-aligned prefix of {name:?}"
                );
            }
        }
    }

    /// One chip is 100 wide plus a 6 gap, so this is "room for exactly four".
    const FOUR_CHIPS: f32 = 4. * (CHIP_MIN_W + CHIP_GAP);

    #[test]
    fn every_chip_is_drawn_while_they_all_fit() {
        let order: Vec<usize> = (0..4).collect();
        assert_eq!(visible_chips(&order, 0, FOUR_CHIPS), order);
        assert_eq!(visible_chips(&order, 3, FOUR_CHIPS), order);
        assert_eq!(visible_chips(&[0, 1], 1, FOUR_CHIPS), vec![0, 1]);
    }

    #[test]
    fn the_run_stays_put_until_the_active_chip_would_fall_off() {
        let order: Vec<usize> = (0..9).collect();
        // Anchored at the first tab for as long as the active one is inside it.
        assert_eq!(visible_chips(&order, 0, FOUR_CHIPS), vec![0, 1, 2, 3]);
        assert_eq!(visible_chips(&order, 3, FOUR_CHIPS), vec![0, 1, 2, 3]);
        // Then it slides by exactly as much as it has to.
        assert_eq!(visible_chips(&order, 4, FOUR_CHIPS), vec![1, 2, 3, 4]);
        assert_eq!(visible_chips(&order, 8, FOUR_CHIPS), vec![5, 6, 7, 8]);
    }

    #[test]
    fn the_active_chip_is_always_among_the_drawn_ones() {
        let order: Vec<usize> = (0..40).collect();
        for active in 0..40 {
            for avail in [0., 1., 80., FOUR_CHIPS, 4000.] {
                let shown = visible_chips(&order, active, avail);
                assert!(
                    shown.contains(&active),
                    "active {active} missing at {avail}px: {shown:?}"
                );
            }
        }
    }

    #[test]
    fn a_reordered_run_is_sliced_in_its_own_order() {
        // Mid-drag the strip renders `preview.order`, not 0..n.
        let order = vec![3, 0, 1, 2, 4, 5];
        assert_eq!(visible_chips(&order, 5, FOUR_CHIPS), vec![1, 2, 4, 5]);
    }

    #[test]
    fn configured_shell_arguments_remain_user_authored_in_the_menu() {
        let shell = DetectedShell {
            label: "custom".into(),
            program: "custom-shell".into(),
            args: vec!["--login".into()],
            args_are_tty59_defaults: false,
            user_authored: false,
        };
        let spec = shell_spec(&shell);

        assert_eq!(spec.program, "custom-shell");
        assert_eq!(spec.args, ["--login"]);
        assert!(!spec.args_are_tty59_defaults);
    }

    /// A tab of this window as the strip reads it: `tab_label` is nothing but
    /// [`label_of`] over the [`TabView`](crate::ui::machine_mirror::TabView)
    /// that [`Tab::label_view`](crate::ui::app::Tab::label_view) builds from
    /// the live leaf, so naming one here climbs the same ladder a real tab
    /// climbs. `title` is the placeholder `label_view` fills that slot with —
    /// the machine tree puts a process name there, a live pane has only the
    /// name it answers to before anything has spoken.
    fn strip_tab() -> crate::ui::machine_mirror::TabView {
        crate::ui::machine_mirror::TabView {
            id: tty59_core::core::machine::TabId::new(),
            name: None,
            title: crate::terminal::view::DEFAULT_TITLE.to_string(),
            osc_title: None,
            cwd: None,
            agent: None,
            status: None,
            live: true,
            panes: 1,
        }
    }

    /// The home the paths below are measured against — named rather than read
    /// off this machine, so the assertions do not depend on who is running
    /// them (#580).
    fn home() -> &'static Path {
        Path::new("/Users/x")
    }

    #[test]
    fn a_renamed_tab_keeps_its_name_over_every_other_answer() {
        let mut tab = strip_tab();
        tab.name = Some("  build  ".into());
        tab.osc_title = Some("vim — main.rs".into());
        tab.cwd = Some("/Users/x/repo/tty59".into());

        assert_eq!(label_of(&tab, 0, Some(home())), "build");
    }

    #[test]
    fn a_pane_showing_a_title_is_named_by_it_and_not_by_its_directory() {
        let mut tab = strip_tab();
        tab.osc_title = Some("vim — main.rs".into());
        tab.cwd = Some("/Users/x/repo/tty59".into());

        assert_eq!(label_of(&tab, 0, Some(home())), "vim — main.rs");

        // Including the title an SSH pane answers to before the far shell has
        // said anything (#438): `label_view` hands that up here, so a window
        // full of them still reads as hosts rather than as directories.
        tab.osc_title = Some("prod-web".into());
        assert_eq!(label_of(&tab, 0, Some(home())), "prod-web");
    }

    /// #740: every shell tty59 ships integration for except PowerShell reports
    /// its directory over OSC 7 and never sets a title, which left the tab
    /// reading "tty59" — the app's own name — while the switcher listing the
    /// very same tab showed the directory.
    #[test]
    fn a_pane_that_has_only_said_where_it_is_is_named_after_that() {
        let mut tab = strip_tab();
        tab.cwd = Some("/Users/x/repo/tty59".into());

        assert_eq!(label_of(&tab, 0, Some(home())), "~/repo/tty59");
        // Through the same shortener as a title, so a deep directory is cut
        // where a deep path in a title would be.
        tab.cwd = Some("/Users/x/repo/tty59/crates/tty59-core/src".into());
        assert_eq!(
            label_of(&tab, 0, Some(home())),
            super::short_title("/Users/x/repo/tty59/crates/tty59-core/src", Some(home())),
        );
    }

    /// A tooltip exists to say what the row had to leave out. One that repeats
    /// the row is worse than none, and the label and the raw string it came
    /// from are not comparable until both have been abbreviated: `~/repo` and
    /// `/Users/x/repo` are the same name spelled two ways, and reading them as
    /// a difference hung a tooltip on every tab named after a directory under
    /// the home — which, after this change, is most of them.
    #[test]
    fn a_tab_named_after_a_directory_says_nothing_more_on_hover_unless_it_was_cut() {
        let mut tab = strip_tab();
        tab.cwd = Some("/Users/x/repo".into());

        assert_eq!(label_of(&tab, 0, Some(home())), "~/repo");
        assert_eq!(
            tooltip_of(&tab, 0, Some(home())),
            None,
            "the row is already showing the whole directory"
        );

        // Cut down to its last three segments, so the head is worth having.
        tab.cwd = Some("/Users/x/repo/crates/tty59-core/src".into());
        assert_eq!(label_of(&tab, 0, Some(home())), "…/crates/tty59-core/src");
        assert_eq!(
            tooltip_of(&tab, 0, Some(home())).as_deref(),
            Some("~/repo/crates/tty59-core/src")
        );

        // The same holds for a title that happens to be a path — the rung this
        // guard was already getting wrong before a directory could reach it.
        let mut titled = strip_tab();
        titled.osc_title = Some("/Users/x/repo".into());
        assert_eq!(tooltip_of(&titled, 0, Some(home())), None);

        // A shell integration's `user@host:` head is not in the label, so it
        // is still worth spelling out.
        titled.osc_title = Some("me@box:/Users/x/repo".into());
        assert_eq!(
            tooltip_of(&titled, 0, Some(home())).as_deref(),
            Some("me@box:/Users/x/repo")
        );
    }

    /// The one test that fails if any of the wiring is put back: a real tab,
    /// built the way the window builds one, named through `tab_label` — and
    /// checked against what the switcher renders from the machine tree's view
    /// of that very same pane. Before this change the strip said "tty59" and
    /// the switcher said the directory (#740).
    #[gpui::test]
    fn the_strip_names_a_titleless_pane_exactly_as_the_switcher_does(cx: &mut TestAppContext) {
        use crate::ui::pane::{Pane, PaneSlot};

        let (app, mut vcx) = crate::ui::app::test_window::harness(cx);
        let _stream = app.update_in(&mut vcx, |app, window, cx| {
            let (view, stream) = crate::terminal::view::quiet_test_pane(1, window, cx);
            // A pane that has reported where it is over OSC 7 and has never
            // titled itself — every shell tty59 ships integration for except
            // PowerShell.
            view.read(cx)
                .terminal
                .seed_cwd(Some(std::path::PathBuf::from("/work/repo")));
            app.tabs
                .push(crate::ui::app::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = app.tabs.len() - 1;
            stream
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            let index = app.active;
            let tab = &app.tabs[index];
            let (view, home) = tab.label_view(Some(window), cx);
            assert_eq!(view.osc_title, None, "the pane never titled itself");
            assert_eq!(view.cwd.as_deref(), Some("/work/repo"));

            let strip = app.tab_label(tab, index, Some(window), cx);
            assert_eq!(strip, "/work/repo");
            assert_ne!(
                strip,
                crate::terminal::view::DEFAULT_TITLE,
                "and is not named after the app any more"
            );

            // The machine tree's reading of the same pane, which is all the
            // switcher ever has: no title was seen, the cwd is the one above,
            // and `title` is the foreground process name.
            let from_tree = crate::ui::machine_mirror::TabView {
                id: tab.tree_id.get(),
                name: None,
                title: "zsh".into(),
                osc_title: None,
                cwd: Some("/work/repo".into()),
                agent: None,
                status: None,
                live: true,
                panes: 1,
            };
            assert_eq!(
                strip,
                label_of(&from_tree, index, home.as_deref()),
                "the two columns name the same tab the same way"
            );

            assert_eq!(
                app.tab_title_tooltip(tab, index, Some(window), cx),
                None,
                "and the row is showing the whole path, so it stays quiet"
            );
        });
    }

    #[test]
    fn a_pane_with_nothing_to_say_falls_back_the_way_it_always_did() {
        // No title and no directory: the placeholder, exactly as before.
        let tab = strip_tab();
        assert_eq!(label_of(&tab, 0, Some(home())), "tty59");

        // And a tab holding no live pane at all is still numbered.
        let mut empty = strip_tab();
        empty.title = String::new();
        assert!(label_of(&empty, 2, Some(home())).contains('3'));
    }

    /// The rung under the shortener, which the two surfaces reach holding
    /// different things. A shell that has said who and where it is but not
    /// *where* — `user@host:` with nothing after the colon — leaves nothing to
    /// show, and whatever stands in has to be something the tab does not
    /// already say: the switcher has the foreground process name, and a tab of
    /// this window has only the placeholder, which is the answer #740 removed.
    #[test]
    fn a_title_that_shortens_away_never_puts_the_app_name_back_on_the_tab() {
        let mut strip = strip_tab();
        strip.osc_title = Some("user@host:".into());
        assert_ne!(
            label_of(&strip, 0, Some(home())),
            crate::terminal::view::DEFAULT_TITLE
        );
        assert!(
            label_of(&strip, 0, Some(home())).contains('1'),
            "the numbered placeholder, which is what the strip showed here \
             before it shared this renderer"
        );

        // The switcher arrives with a real process name in that slot, and it
        // is still worth more than a number.
        let from_tree = crate::ui::machine_mirror::TabView {
            title: "zsh".into(),
            osc_title: Some("user@host:".into()),
            ..strip_tab()
        };
        assert_eq!(label_of(&from_tree, 0, Some(home())), "zsh");
    }

    /// A path is spelled the way the machine it is on spells it, and which
    /// machine that is has nothing to do with which one tty59 is running on: a
    /// remote pane reports POSIX to a Windows client, and a Windows pane
    /// reports backslashes to a client that has never seen one (#580).
    #[test]
    fn a_cwd_is_cut_in_its_own_spelling_whichever_client_is_reading_it() {
        let windows_home = Path::new(r"C:\Users\x");

        // A Windows pane: shortened under its own home, and a path too deep to
        // fit is rejoined with its own separator rather than with `/`.
        let mut win = strip_tab();
        win.cwd = Some(r"C:\Users\x\repo".into());
        assert_eq!(label_of(&win, 0, Some(windows_home)), "~/repo");
        win.cwd = Some(r"D:\work\a\b\proj".into());
        assert_eq!(label_of(&win, 0, Some(windows_home)), r"…\a\b\proj");

        // A remote pane's cwd is POSIX even when the client reading it is the
        // Windows one: no drive to hang it off, no `~` borrowed from this
        // machine's home, and no backslash anywhere in the answer.
        let mut remote = strip_tab();
        remote.cwd = Some("/srv/app".into());
        assert_eq!(label_of(&remote, 0, Some(windows_home)), "/srv/app");
        remote.cwd = Some("/home/deploy/app".into());
        assert_eq!(
            label_of(&remote, 0, Some(Path::new("/home/deploy"))),
            "~/app",
            "measured against the home of the host it is on, not of this one"
        );

        // The root of a filesystem is a directory like any other: a tab
        // sitting in it says so, and says nothing more on hover.
        let mut root = strip_tab();
        root.cwd = Some("/".into());
        assert_eq!(label_of(&root, 0, Some(home())), "/");
        assert_eq!(tooltip_of(&root, 0, Some(home())), None);
    }

    fn inventory(labels: &[&str]) -> Vec<DetectedShell> {
        labels
            .iter()
            .map(|l| DetectedShell {
                label: l.to_string(),
                program: format!("/bin/{l}"),
                args: Vec::new(),
                args_are_tty59_defaults: true,
                user_authored: false,
            })
            .collect()
    }

    fn used(
        entries: &[(&str, u32, u64)],
    ) -> std::collections::HashMap<String, crate::core::config::ProfileUsage> {
        entries
            .iter()
            .map(|(l, count, last_used)| {
                (
                    l.to_string(),
                    crate::core::config::ProfileUsage {
                        count: *count,
                        last_used: *last_used,
                    },
                )
            })
            .collect()
    }

    fn labels(shells: &[&DetectedShell]) -> Vec<String> {
        shells.iter().map(|s| s.label.clone()).collect()
    }

    const STOCK_MAC: &[&str] = &["zsh", "bash", "sh", "csh", "tcsh", "ksh", "dash"];

    #[test]
    fn the_new_tab_menu_names_the_default_and_what_has_been_opened_by_frecency() {
        let now = 100_000_000u64;
        let day = 86_400u64;
        let shells = inventory(STOCK_MAC);
        // `bash` used often and lately; `dash` used more but a year ago;
        // `ksh` once, two months back; and the default opened too.
        let usage = used(&[
            ("zsh", 50, now),
            ("ksh", 1, now - 60 * day),
            ("bash", 9, now - day),
            ("dash", 20, now - 365 * day),
        ]);
        let sorted = shells_by_frecency(&shells, "zsh", &usage, now);
        assert_eq!(
            labels(&sorted),
            ["zsh", "bash", "dash", "ksh", "sh", "csh", "tcsh"],
            "default first, then by frecency, then the inventory's own order"
        );
        let menu = menu_shells(&sorted, "zsh", &usage, now);
        assert_eq!(menu.len(), MENU_SHELLS);
        assert_eq!(labels(&menu), ["zsh", "bash", "dash"]);
    }

    #[test]
    fn the_default_shell_leads_the_menu_even_when_it_has_never_been_opened() {
        let now = 100_000_000u64;
        let shells = inventory(STOCK_MAC);
        let usage = used(&[("tcsh", 3, now), ("sh", 7, now), ("ksh", 5, now)]);
        let sorted = shells_by_frecency(&shells, "bash", &usage, now);
        let menu = menu_shells(&sorted, "bash", &usage, now);
        assert_eq!(labels(&menu), ["bash", "sh", "ksh"]);
    }

    #[test]
    fn a_shell_nobody_has_opened_is_not_named_just_because_there_is_room() {
        // A fresh install: nothing has been opened, so the menu names the
        // default alone and leaves the rest to the palette.
        let now = 100_000_000u64;
        let shells = inventory(STOCK_MAC);
        let none = used(&[]);
        let sorted = shells_by_frecency(&shells, "zsh", &none, now);
        assert_eq!(labels(&menu_shells(&sorted, "zsh", &none, now)), ["zsh"]);

        // A usage record at zero counts as never opened.
        let zeroed = used(&[("bash", 0, now)]);
        let sorted = shells_by_frecency(&shells, "zsh", &zeroed, now);
        assert_eq!(labels(&menu_shells(&sorted, "zsh", &zeroed, now)), ["zsh"]);

        // An inventory that does not list the default still offers its head,
        // which is the shell a new tab opens with.
        let sorted = shells_by_frecency(&shells, "fish", &none, now);
        assert_eq!(labels(&menu_shells(&sorted, "fish", &none, now)), ["zsh"]);
    }

    #[test]
    fn other_shells_is_offered_only_when_the_inventory_holds_more_than_the_menu_names() {
        let now = 100_000_000u64;
        let more = |inv: &[&str], usage: &[(&str, u32, u64)]| {
            let shells = inventory(inv);
            let usage = used(usage);
            let default = inv.first().copied().unwrap_or("");
            let sorted = shells_by_frecency(&shells, default, &usage, now);
            menu_shells(&sorted, default, &usage, now).len() < sorted.len()
        };
        assert!(more(STOCK_MAC, &[]), "a stock box names one of seven");
        assert!(
            !more(&["pwsh"], &[]),
            "the default alone is the whole inventory"
        );
        assert!(
            !more(&["zsh", "bash"], &[("bash", 1, now)]),
            "every shell is already a row"
        );
        assert!(
            !more(
                &["zsh", "bash", "fish"],
                &[("bash", 1, now), ("fish", 2, now)]
            ),
            "three shells, all named"
        );
        assert!(
            more(&["zsh", "bash", "fish"], &[("bash", 1, now)]),
            "`fish` has never been opened, so it is left to the palette"
        );
        assert!(
            !more(&[], &[]),
            "no inventory: the fallback row, nothing more"
        );
    }

    #[test]
    fn the_new_tab_menu_names_agents_that_have_actually_been_run() {
        use crate::core::cli_agent::CLIAgent;
        let now = 100_000_000u64;
        let day = 86_400u64;
        // Already most-used-first, the way `offered_agents` hands them over,
        // except Amp has never been run and sits between two that have — the
        // filter has to skip it rather than stop.
        let offered = [
            CLIAgent::Claude,
            CLIAgent::Amp,
            CLIAgent::Codex,
            CLIAgent::Gemini,
            CLIAgent::Aider,
        ];
        let usage = used(&[
            ("claude", 12, now - 2 * day),
            ("codex", 3, now - day),
            ("gemini", 1, now - 30 * day),
            ("aider", 0, now),
        ]);
        let named = menu_agents(&offered, &usage, now);
        assert_eq!(
            named,
            vec![CLIAgent::Claude, CLIAgent::Codex, CLIAgent::Gemini],
            "three that have run fill the cap; a zero count is not a use, and a gap is skipped"
        );
        assert!(named.len() < offered.len(), "the rest are the seam row's");

        // Nothing has run yet: the menu still leads with the first agents
        // the machine offers, and the seam covers the rest — a heading above
        // a single "Other Agents…" makes launching the first agent take a
        // menu, a search scope and a pick.
        let none = used(&[]);
        let named = menu_agents(&offered, &none, now);
        assert_eq!(
            named,
            vec![CLIAgent::Claude, CLIAgent::Amp, CLIAgent::Codex],
            "a fresh profile names the first offered agents, in offered order"
        );
        assert!(
            named.len() < offered.len(),
            "the rest remain the seam row's"
        );
        // A record for an agent this machine does not offer is no use either.
        let stale = used(&[("goose", 9, now)]);
        assert_eq!(menu_agents(&offered, &stale, now), named);

        // Offered fewer than the cap, every one is named — no seam row.
        let few = [CLIAgent::Claude, CLIAgent::Codex];
        let named = menu_agents(&few, &none, now);
        assert_eq!(named, few);
        assert_eq!(named.len(), few.len());

        assert!(menu_agents(&[], &none, now).is_empty());
        assert!(menu_agents(&[], &usage, now).is_empty());

        // Every offered agent has been run and they fit: no seam row.
        let two = used(&[("claude", 4, now), ("codex", 1, now)]);
        let named = menu_agents(&few, &two, now);
        assert_eq!(named, vec![CLIAgent::Claude, CLIAgent::Codex]);
        assert_eq!(named.len(), few.len());
    }
}

/// The top strip through the real window: a harness app with the tab bar on
/// top, resized, drawn, and read back from what was painted (`strip_slots` is
/// written from paint, so a chip that is not drawn leaves nothing in it).
#[cfg(test)]
mod overflow_gpui_tests {
    use crate::core::config::{Config, TabBarPosition, UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX};
    use crate::ui::app::{Tty59App, test_window::harness_with_tabs};
    use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, px, size};
    use gpui_component::ActiveTheme as _;

    /// `windows::MIN_SIZE.0`: the narrowest window the app lets anyone drag to.
    const NARROWEST_WINDOW: f32 = 720.;

    /// The control carries the count it draws in its selector, so a test can
    /// compare what is on screen against the chips that were actually left out.
    /// `debug_bounds` wants a `&'static str`, so the supported counts are listed
    /// once instead of leaking a string per lookup.
    const COUNT_SELECTORS: [&str; 24] = [
        "tab-strip-overflow-1",
        "tab-strip-overflow-2",
        "tab-strip-overflow-3",
        "tab-strip-overflow-4",
        "tab-strip-overflow-5",
        "tab-strip-overflow-6",
        "tab-strip-overflow-7",
        "tab-strip-overflow-8",
        "tab-strip-overflow-9",
        "tab-strip-overflow-10",
        "tab-strip-overflow-11",
        "tab-strip-overflow-12",
        "tab-strip-overflow-13",
        "tab-strip-overflow-14",
        "tab-strip-overflow-15",
        "tab-strip-overflow-16",
        "tab-strip-overflow-17",
        "tab-strip-overflow-18",
        "tab-strip-overflow-19",
        "tab-strip-overflow-20",
        "tab-strip-overflow-21",
        "tab-strip-overflow-22",
        "tab-strip-overflow-23",
        "tab-strip-overflow-24",
    ];

    fn count_selector(hidden: usize) -> &'static str {
        COUNT_SELECTORS[hidden - 1]
    }

    /// The painted slot of each glyph of the count (`+`, then each digit), by
    /// position. `debug_bounds` wants `&'static str`, so they are listed once.
    const GLYPH_SELECTORS: [&str; 3] = [
        "tab-strip-overflow-glyph-0",
        "tab-strip-overflow-glyph-1",
        "tab-strip-overflow-glyph-2",
    ];

    /// The count's type is 12px at the default 16px rem.
    const COUNT_TEXT_OF_REM: f32 = 12. / 16.;

    /// Every glyph of the count sits inside its own painted slot, in the tile,
    /// and the widest glyph the font can put there, shaped by the real text
    /// system, fits that slot: no digit clips or runs into its neighbour at
    /// this interface font size.
    fn assert_glyphs_fit_their_slots(
        vcx: &mut VisualTestContext,
        tile: Bounds<Pixels>,
        counted: usize,
        ui_font_size: f32,
        what: &str,
    ) {
        let glyphs = 1 + counted.to_string().len();
        let slots: Vec<Bounds<Pixels>> = (0..glyphs)
            .map(|at| {
                vcx.debug_bounds(GLYPH_SELECTORS[at])
                    .unwrap_or_else(|| panic!("{what}: glyph {at} of +{counted} was not painted"))
            })
            .collect();
        let same = |a: Pixels, b: Pixels| f32::from(a - b).abs() < 0.01;
        for (at, slot) in slots.iter().enumerate() {
            assert!(
                same(slot.size.width, slots[0].size.width),
                "{what}: +{counted} glyph {at} has its own width: {:?} vs {:?}",
                slot.size.width,
                slots[0].size.width
            );
            assert!(
                slot.left() >= tile.left() && slot.right() <= tile.right(),
                "{what}: +{counted} glyph {at} sticks out of its tile"
            );
            if at > 0 {
                assert!(
                    slots[at - 1].right() <= slot.left() + px(0.01),
                    "{what}: +{counted} glyphs {} and {at} overlap",
                    at - 1
                );
            }
        }
        let widest = vcx.update(|window, cx| {
            let font = gpui::Font {
                family: cx.theme().font_family.clone(),
                features: crate::ui::theme::tabular_figures(),
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            };
            let size = ui_font_size * COUNT_TEXT_OF_REM;
            ["+", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]
                .into_iter()
                .map(|glyph| {
                    crate::ui::tab_strip::measure_text(window.text_system(), &font, size, glyph)
                })
                .fold(0., f32::max)
        });
        assert!(
            widest <= f32::from(slots[0].size.width),
            "{what}: +{counted} has glyphs {widest}px wide in {:?} slots at {ui_font_size}px",
            slots[0].size.width
        );
    }

    /// The two languages the strip is drawn in here, by registry code, with
    /// what each calls the Agents pill: its label, and so its width, is the one
    /// thing in the row that changes with them.
    const LOCALES: [(&str, &str); 2] = [("en", "Agents"), ("ru-RU", "Агенты")];

    /// `set_locale` falls back to English for a code it does not know, so the
    /// language is checked by what it says, not by what was asked for.
    fn use_locale(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, locale: &str, agents: &str) {
        assert!(
            crate::ui::i18n::find_language(locale).is_some(),
            "{locale} is not a supported language code"
        );
        crate::ui::i18n::set_locale(locale);
        assert_eq!(
            crate::ui::i18n::t(crate::ui::i18n::L10nKey::VisionAgents),
            agents,
            "the strip is not drawn in {locale}"
        );
        app.update_in(vcx, |_, _, cx| cx.notify());
        redraw(vcx);
    }

    fn redraw(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn resize(vcx: &mut VisualTestContext, width: f32) {
        vcx.simulate_resize(size(px(width), px(700.)));
        redraw(vcx);
    }

    fn set_ui_font_size(app: &Entity<Tty59App>, vcx: &mut VisualTestContext, ui_font_size: f32) {
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.ui_font_size = ui_font_size;
            cx.set_global(cfg);
        });
        app.update_in(vcx, |_, _, cx| cx.notify());
        redraw(vcx);
    }

    fn strip_window(
        cx: &mut TestAppContext,
        tabs: usize,
        width: f32,
    ) -> (
        Entity<Tty59App>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, streams) = harness_with_tabs(cx, tabs);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.tab_bar_position = TabBarPosition::Top;
            cx.set_global(cfg);
        });
        app.update_in(&mut vcx, |_, _, cx| cx.notify());
        resize(&mut vcx, width);
        (app, vcx, streams)
    }

    /// Indices of the chips the strip actually painted.
    fn drawn(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) -> Vec<usize> {
        app.update(vcx, |app, _| {
            app.strip_slots
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, bounds)| bounds.size.width > px(0.))
                .map(|(i, _)| i)
                .collect()
        })
    }

    /// The overflow control, whatever count it carries.
    fn find_tile(vcx: &mut VisualTestContext, tabs: usize) -> Option<(usize, Bounds<Pixels>)> {
        (1..=tabs).find_map(|n| vcx.debug_bounds(count_selector(n)).map(|b| (n, b)))
    }

    /// Right edge of the last painted chip.
    fn last_chip_right(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        shown: &[usize],
    ) -> Pixels {
        app.update(vcx, |app, _| {
            let slots = app.strip_slots.borrow();
            let right = shown
                .iter()
                .map(|&i| slots[i].right())
                .fold(px(0.), |a, b| if b > a { b } else { a });
            right
        })
    }

    /// Where the strip's own controls sit beside the chips: the whole row is
    /// the chips, the count when there is one, New Tab, then the Agents pill,
    /// and none of them may stand on another or on the window's edge. The
    /// pill is not budgeted by the chips' own arithmetic, so a strip that
    /// forgets it shrinks the chips' row until the last chip overlaps what
    /// follows it.
    fn assert_row_does_not_overlap(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        shown: &[usize],
        width: f32,
        what: &str,
    ) {
        let new_tab = vcx
            .debug_bounds("tab-strip-new")
            .expect("the New Tab button is drawn");
        let pill = vcx
            .debug_bounds("agent-mode-pill")
            .expect("the Agents pill is drawn");
        let last_chip = last_chip_right(app, vcx, shown);
        assert!(
            last_chip <= new_tab.left(),
            "{what}: the last chip overlaps New Tab at {width}px: chip ends {last_chip:?}, \
             New Tab starts {:?}",
            new_tab.left()
        );
        assert!(
            new_tab.right() <= pill.left(),
            "{what}: New Tab overlaps the Agents pill at {width}px"
        );
        assert!(
            pill.right() <= px(width),
            "{what}: the Agents pill is pushed out of the window at {width}px"
        );
    }

    /// Whatever the strip drew, the count is exact and sits between the last
    /// chip and the New Tab button, which stays inside the window.
    fn assert_count_is_exact_and_paid_for(
        app: &Entity<Tty59App>,
        vcx: &mut VisualTestContext,
        tabs: usize,
        width: f32,
        what: &str,
    ) -> Option<(usize, Bounds<Pixels>)> {
        let shown = drawn(app, vcx);
        let hidden = tabs - shown.len();
        let tile = find_tile(vcx, tabs);
        assert_row_does_not_overlap(app, vcx, &shown, width, what);
        if hidden == 0 {
            assert_eq!(tile, None, "{what}: everything fits at {width}px, no count");
            return None;
        }
        let (counted, bounds) =
            tile.unwrap_or_else(|| panic!("{what}: {hidden} tabs hidden at {width}px, no count"));
        assert_eq!(counted, hidden, "{what}: the count is off at {width}px");
        let last_chip = last_chip_right(app, vcx, &shown);
        assert!(
            last_chip <= bounds.left(),
            "{what}: the count overlaps the last chip at {width}px: chip ends {last_chip:?}, \
             count starts {:?}",
            bounds.left()
        );
        let new_tab = vcx
            .debug_bounds("tab-strip-new")
            .expect("the New Tab button is drawn");
        assert!(
            bounds.right() <= new_tab.left(),
            "{what}: the count overlaps the New Tab button at {width}px"
        );
        assert!(
            new_tab.right() <= px(width),
            "{what}: the New Tab button was pushed out of the window at {width}px"
        );
        Some((counted, bounds))
    }

    /// The reported bug: 24 tabs in a 1000px window, a run of them drawn and
    /// nothing on screen saying how many were left out. The same strip with
    /// three tabs is the control: everything fits, so there is no count.
    #[gpui::test]
    fn tabs_that_do_not_fit_are_counted_on_screen(cx: &mut TestAppContext) {
        let (few, mut few_vcx, _few_streams) = strip_window(cx, 3, 1000.);
        assert_eq!(drawn(&few, &mut few_vcx), vec![0, 1, 2]);
        assert_eq!(find_tile(&mut few_vcx, 3), None, "three tabs need no count");
        assert!(few_vcx.debug_bounds("tab-strip-new").is_some());

        let (app, mut vcx, _streams) = strip_window(cx, 24, 1000.);
        let shown = drawn(&app, &mut vcx);
        assert!(
            !shown.is_empty() && shown.len() < 24,
            "the window has to be narrow enough to clip some tabs: drew {shown:?}"
        );
        let hidden = 24 - shown.len();
        assert!(
            vcx.debug_bounds(count_selector(hidden)).is_some(),
            "{hidden} tabs are not drawn and nothing carries that count \
             (found {:?})",
            find_tile(&mut vcx, 24).map(|(n, _)| n)
        );
    }

    /// Thresholds, from the narrowest supported window to one that holds every
    /// chip, at the default and at the largest interface font. The count is
    /// exact, never grows with more room, and the control is paid for out of
    /// the chips' width instead of pushing `+` out; the control itself grows
    /// with the interface font and with a second digit.
    #[gpui::test]
    fn the_count_is_exact_and_paid_for_at_every_width(cx: &mut TestAppContext) {
        const TABS: usize = 20;
        let (app, mut vcx, _streams) = strip_window(cx, TABS, NARROWEST_WINDOW);

        // tile width by [font size, digits - 1]
        let mut tile_w = [[None::<f32>; 2]; 2];
        // each locale x font x digit count is proved once, not at every width
        let mut glyphs_checked = [[[false; 2]; 2]; 2];
        for (locale_at, (locale, agents)) in LOCALES.into_iter().enumerate() {
            use_locale(&app, &mut vcx, locale, agents);
            for (font, ui_font_size) in [UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX]
                .into_iter()
                .enumerate()
            {
                set_ui_font_size(&app, &mut vcx, ui_font_size);
                let (mut saw_all_fit, mut saw_hidden) = (false, false);
                let mut previous_hidden = usize::MAX;
                let mut width = NARROWEST_WINDOW;
                while width <= 2700. {
                    resize(&mut vcx, width);
                    let hidden = TABS - drawn(&app, &mut vcx).len();
                    assert!(
                        hidden <= previous_hidden,
                        "{locale} {ui_font_size}px font: more room hid more tabs at {width}px: \
                         {hidden} after {previous_hidden}"
                    );
                    previous_hidden = hidden;
                    let what = format!("{locale} {ui_font_size}px font");
                    match assert_count_is_exact_and_paid_for(&app, &mut vcx, TABS, width, &what) {
                        None => saw_all_fit = true,
                        Some((counted, bounds)) => {
                            saw_hidden = true;
                            let digits = counted.to_string().len() - 1;
                            tile_w[font][digits] = Some(f32::from(bounds.size.width));
                            if !std::mem::replace(
                                &mut glyphs_checked[locale_at][font][digits],
                                true,
                            ) {
                                assert_glyphs_fit_their_slots(
                                    &mut vcx,
                                    bounds,
                                    counted,
                                    ui_font_size,
                                    &what,
                                );
                            }
                        }
                    }
                    width += 13.;
                }
                assert!(
                    saw_all_fit && saw_hidden,
                    "{locale} {ui_font_size}px font: the sweep has to cross the point where \
                     everything fits"
                );
            }
        }
        use_locale(&app, &mut vcx, "en", "Agents");

        let w = |font: usize, digits: usize| {
            tile_w[font][digits].unwrap_or_else(|| panic!("no tile with {} digits", digits + 1))
        };
        for digits in 0..2 {
            assert!(
                w(1, digits) > w(0, digits),
                "the tile ignores the interface font: {} vs {} px with {} digit(s)",
                w(1, digits),
                w(0, digits),
                digits + 1
            );
        }
        assert!(
            w(0, 1) >= w(0, 0) && w(1, 1) > w(1, 0),
            "a second digit has no room: {:?}",
            tile_w
        );
    }

    /// The active tab is one of the drawn chips beside the count, wherever it
    /// sits, including after the strip is reordered under it and at the
    /// narrowest supported window.
    #[gpui::test]
    fn the_active_tab_stays_drawn_beside_the_count(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = strip_window(cx, 24, 1000.);

        for (locale, agents) in LOCALES {
            use_locale(&app, &mut vcx, locale, agents);
            for (width, target) in [
                (1440., 23usize),
                (1000., 23),
                (1000., 11),
                (1000., 0),
                (NARROWEST_WINDOW, 17),
            ] {
                resize(&mut vcx, width);
                app.update_in(&mut vcx, |app, window, cx| app.activate(target, window, cx));
                redraw(&mut vcx);
                let shown = drawn(&app, &mut vcx);
                assert!(
                    shown.contains(&target),
                    "{locale}: tab {target} is not drawn at {width}px: {shown:?}"
                );
                assert_count_is_exact_and_paid_for(&app, &mut vcx, 24, width, locale)
                    .expect("24 tabs overflow");
            }

            app.update_in(&mut vcx, |app, _, cx| {
                let order: Vec<usize> = (0..app.tabs.len()).rev().collect();
                app.apply_tab_order(&order, cx);
            });
            redraw(&mut vcx);
            let shown = drawn(&app, &mut vcx);
            let active = app.update(&mut vcx, |app, _| app.active);
            assert!(
                shown.contains(&active),
                "{locale} reordered: {active} not in {shown:?}"
            );
            assert_count_is_exact_and_paid_for(&app, &mut vcx, 24, NARROWEST_WINDOW, locale)
                .expect("24 tabs overflow");
        }
        use_locale(&app, &mut vcx, "en", "Agents");
    }

    /// A real click on the count raises the existing recent-tabs list, the
    /// way the keyboard does but with no modifier held, so the list stays up
    /// for the pointer. Escape backs out with the active tab untouched, Enter
    /// lands on the tab used before this one, and Ctrl+Tab keeps walking the
    /// same list afterwards.
    #[gpui::test]
    fn clicking_the_count_opens_the_recent_tabs_list(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = strip_window(cx, 24, 1000.);
        app.update_in(&mut vcx, |app, window, cx| {
            app.activate(20, window, cx);
            app.activate(5, window, cx);
        });
        redraw(&mut vcx);

        let (_, tile) = find_tile(&mut vcx, 24).expect("24 tabs overflow a 1000px strip");
        vcx.simulate_click(tile.center(), Modifiers::none());
        redraw(&mut vcx);
        app.update(&mut vcx, |app, _| {
            assert!(app.switcher.is_some(), "the click opens the list");
        });

        vcx.simulate_modifiers_change(Modifiers::control());
        vcx.simulate_modifiers_change(Modifiers::none());
        app.update(&mut vcx, |app, _| {
            assert!(
                app.switcher.is_some(),
                "a modifier going up must not commit a list the mouse opened"
            );
        });

        vcx.simulate_keystrokes("escape");
        redraw(&mut vcx);
        app.update(&mut vcx, |app, _| {
            assert!(app.switcher.is_none(), "Escape closes the list");
            assert_eq!(app.active, 5, "backing out leaves the active tab alone");
        });

        let (_, tile) = find_tile(&mut vcx, 24).expect("the count is still there");
        vcx.simulate_click(tile.center(), Modifiers::none());
        redraw(&mut vcx);
        vcx.simulate_keystrokes("enter");
        redraw(&mut vcx);
        app.update(&mut vcx, |app, _| {
            assert!(app.switcher.is_none(), "Enter picks a tab and closes it");
            assert_eq!(
                app.active, 20,
                "the recent-tabs list leads with the last tab used"
            );
        });

        vcx.simulate_modifiers_change(Modifiers::control());
        app.update_in(&mut vcx, |app, window, cx| app.tab_switch(true, window, cx));
        app.update(&mut vcx, |app, _| {
            assert!(app.switcher.is_some(), "Ctrl+Tab raises the list");
        });
        vcx.simulate_modifiers_change(Modifiers::none());
        app.update(&mut vcx, |app, _| {
            assert!(app.switcher.is_none());
            assert_eq!(app.active, 5, "releasing commits the previously used tab");
        });
    }
}
