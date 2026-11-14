//! The measured elision policy, with no gpui in it.
//!
//! Every function here decides *what* to keep and asks a caller-supplied
//! `measure` callback how wide a candidate really is, so the same code serves
//! the sidebar (real glyph widths), a character-budget column (one unit per
//! cluster) and the unit tests (any width model at all). Nothing here guesses
//! at a glyph width.
//!
//! The contract every mode shares:
//! - text that already fits comes back borrowed and untouched;
//! - cuts land on grapheme-cluster boundaries only;
//! - at most one `…` is ever inserted;
//! - whatever is returned measures within the budget, and a budget too small
//!   for even a lone `…` yields an empty string rather than an over-budget one;
//! - an optional scalar cap ([`Limits::max_chars`]) holds as well, checked as a
//!   predicate of its own before a candidate is ever measured, so no budget,
//!   infinite or NaN included, can accept a candidate over the cap;
//! - a cut is snapped to a word boundary (after one of `-_./\` or a space)
//!   when one lies within [`SNAP_REACH`] clusters of the best fitting cut.
//!   Snapping only ever drops more text, never adds it, so it cannot break the
//!   fit; the original cut is kept when no boundary is near.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation as _;

pub(crate) const ELLIPSIS: &str = "…";

/// How far a cut may travel to reach a word boundary, in grapheme clusters.
pub(crate) const SNAP_REACH: usize = 8;

/// Characters a cut is allowed to settle next to. Space is in the set because
/// labels a human typed (`Backend server logs`) are elided too, and a word
/// boundary is the cut a reader forgives. The backslash keeps a Windows path
/// cutting where a Unix one does.
const BREAKS: [char; 6] = ['-', '_', '/', '\\', '.', ' '];

/// The head [`Mode::Edges`] would rather keep, extended to just past the next
/// break (up to [`EDGE_HEAD_CAP`]) so it ends on a boundary: `window-…` rather
/// than `window…`.
const EDGE_HEAD: usize = 6;
const EDGE_HEAD_CAP: usize = 12;
/// The smallest head worth keeping when a longer one leaves no room for a tail.
const EDGE_HEAD_FLOOR: usize = 3;
/// Enough trailing clusters to tell two sibling branches apart.
const EDGE_MIN_TAIL: usize = 3;

/// What an oversized label gives up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    /// Keep the head: `fix/rpc-proxy-…`. For text read left to right.
    End,
    /// Keep the tail: `…tab_sidebar.rs`. The last resort of [`Mode::Path`].
    Start,
    /// Keep both ends in equal share, the extra cluster going to the head.
    Middle,
    /// Keep a head of about six clusters and as much tail as then fits, so
    /// `window-transparency-backdrop` reads `window-…backdrop`.
    Edges,
    /// A path: keep the root marker (`~`, a drive letter or the leading
    /// slash) and every trailing segment that fits, dropping whole segments
    /// from the front; only when the last segment alone is too wide is it torn.
    Path,
    /// A row label that may or may not be a path: text containing `/` or `\`
    /// elides as [`Mode::Path`], anything else as [`Mode::Edges`]. Decided
    /// only once an elision is actually needed, from the text itself, so the
    /// text in the memo key is all that determines it.
    Label,
}

/// What a result has to satisfy besides fitting the budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Limits {
    /// The widest the result may measure, in whatever unit `measure` speaks.
    pub budget: f32,
    /// The most Unicode scalar values the elided text may have, ellipsis
    /// included and any tail excluded. Cuts stay grapheme-aligned, so a
    /// cluster that would cross the cap is dropped whole.
    pub max_chars: Option<usize>,
    /// The fewest original clusters that must survive for the label to be worth
    /// showing: below it the whole label, tail included, is hidden. Text that
    /// fits whole is never hidden.
    pub min_visible: Option<usize>,
}

/// Grapheme clusters of one string, addressed by index without copying.
struct Clusters<'a> {
    text: &'a str,
    /// Byte offset of every cluster, plus `text.len()` as a final sentinel.
    starts: Vec<usize>,
}

impl<'a> Clusters<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts: Vec<usize> = text.grapheme_indices(true).map(|(at, _)| at).collect();
        starts.push(text.len());
        Self { text, starts }
    }

    fn len(&self) -> usize {
        self.starts.len() - 1
    }

    fn span(&self, from: usize, to: usize) -> &'a str {
        &self.text[self.starts[from]..self.starts[to]]
    }

    /// The first `n` clusters. Trailing space is dropped: `Backend …` looks
    /// like a mistake, `Backend…` does not.
    fn head(&self, n: usize) -> &'a str {
        self.span(0, n).trim_end()
    }

    /// The last `n` clusters, without leading space.
    fn tail(&self, n: usize) -> &'a str {
        self.span(self.len() - n, self.len()).trim_start()
    }

    /// Whether a head may end at `at`: the cluster before it is a break.
    fn head_boundary(&self, at: usize) -> bool {
        at > 0 && is_break(self.span(at - 1, at))
    }

    /// Whether a tail may begin at `at`: after a break, or on the `.` that
    /// opens an extension, which is worth more than the word before it.
    fn tail_boundary(&self, at: usize) -> bool {
        at > 0
            && at < self.len()
            && (is_break(self.span(at - 1, at)) || self.span(at, at + 1) == ".")
    }
}

fn is_break(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if BREAKS.contains(&c))
}

fn compose(head: &str, tail: &str) -> String {
    let mut out = String::with_capacity(head.len() + ELLIPSIS.len() + tail.len());
    out.push_str(head);
    out.push_str(ELLIPSIS);
    out.push_str(tail);
    out
}

/// The caller's measure and what a candidate has to satisfy.
struct Fit<'m, M> {
    measure: &'m mut M,
    budget: f32,
    max_chars: Option<usize>,
}

impl<M: FnMut(&str) -> f32> Fit<'_, M> {
    /// The cap is its own predicate, asked before the measure: a candidate over
    /// it is never shaped, and no budget value can accept it.
    fn fits(&mut self, text: &str) -> bool {
        if self.max_chars.is_some_and(|cap| text.chars().count() > cap) {
            return false;
        }
        (self.measure)(text) <= self.budget
    }

    /// Whether `head…tail` fits.
    fn fits_cut(&mut self, head: &str, tail: &str) -> bool {
        self.fits(&compose(head, tail))
    }
}

/// An elision and what of the original it kept, counted structurally from the
/// cut so that an `…` the text came with is retained text, never the marker
/// this module inserted.
struct Outcome<'a> {
    text: Cow<'a, str>,
    /// Where the cut fell: clusters taken from the front, and from the back.
    /// A head loses its trailing space and a tail its leading space on the
    /// way to the output, so these can be more than what is actually shown.
    /// Only the tests of the snapping contract read them; production decides
    /// on `kept`.
    #[cfg(test)]
    head: usize,
    #[cfg(test)]
    tail: usize,
    /// Original clusters actually in the output, after that trim.
    kept: usize,
    /// The text fits as it is.
    whole: bool,
}

impl<'a> Outcome<'a> {
    fn whole(text: &'a str) -> Self {
        Self {
            text: Cow::Borrowed(text),
            #[cfg(test)]
            head: 0,
            #[cfg(test)]
            tail: 0,
            kept: 0,
            whole: true,
        }
    }

    fn nothing() -> Self {
        Self::cut(
            String::new(),
            #[cfg(test)]
            0,
            #[cfg(test)]
            0,
            0,
        )
    }

    fn cut(text: String, #[cfg(test)] head: usize, #[cfg(test)] tail: usize, kept: usize) -> Self {
        Self {
            text: Cow::Owned(text),
            #[cfg(test)]
            head,
            #[cfg(test)]
            tail,
            kept,
            whole: false,
        }
    }

    fn kept(&self) -> usize {
        self.kept
    }
}

/// How many clusters a piece of the original holds.
fn clusters_in(text: &str) -> usize {
    text.graphemes(true).count()
}

/// The largest `n` in `0..=max` for which `ok` holds, assuming `ok(0)` and
/// that width only grows with `n`.
fn longest(max: usize, mut ok: impl FnMut(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, max);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Moves a head cut at `n` back to the nearest boundary within reach, when the
/// result still fits beside `tail`.
fn snap_head<M: FnMut(&str) -> f32>(
    cl: &Clusters,
    fit: &mut Fit<M>,
    n: usize,
    tail: &str,
) -> usize {
    let floor = n.saturating_sub(SNAP_REACH).max(1);
    for at in (floor..=n).rev() {
        if !cl.head_boundary(at) {
            continue;
        }
        if at == n {
            return n;
        }
        let head = cl.head(at);
        if head.is_empty() {
            continue;
        }
        return if fit.fits_cut(head, tail) { at } else { n };
    }
    n
}

/// Moves a tail of `m` clusters shorter, to the nearest boundary within
/// reach, when the result still fits behind `head`.
fn snap_tail<M: FnMut(&str) -> f32>(
    cl: &Clusters,
    fit: &mut Fit<M>,
    m: usize,
    head: &str,
) -> usize {
    if m == 0 {
        return 0;
    }
    let start = cl.len() - m;
    let last = (start + SNAP_REACH).min(cl.len() - 1);
    for at in start..=last {
        if !cl.tail_boundary(at) {
            continue;
        }
        if at == start {
            return m;
        }
        let tail = cl.tail(cl.len() - at);
        if tail.is_empty() {
            continue;
        }
        return if fit.fits_cut(head, tail) {
            cl.len() - at
        } else {
            m
        };
    }
    m
}

fn cut_end<'a, M: FnMut(&str) -> f32>(text: &str, fit: &mut Fit<M>) -> Outcome<'a> {
    let cl = Clusters::new(text);
    let n = longest(cl.len() - 1, |n| fit.fits_cut(cl.head(n), ""));
    let n = snap_head(&cl, fit, n, "");
    let head = cl.head(n);
    Outcome::cut(
        compose(head, ""),
        #[cfg(test)]
        n,
        #[cfg(test)]
        0,
        clusters_in(head),
    )
}

fn cut_start<'a, M: FnMut(&str) -> f32>(text: &str, fit: &mut Fit<M>) -> Outcome<'a> {
    let cl = Clusters::new(text);
    let m = longest(cl.len() - 1, |m| fit.fits_cut("", cl.tail(m)));
    let m = snap_tail(&cl, fit, m, "");
    let tail = cl.tail(m);
    Outcome::cut(
        compose("", tail),
        #[cfg(test)]
        0,
        #[cfg(test)]
        m,
        clusters_in(tail),
    )
}

fn cut_middle<'a, M: FnMut(&str) -> f32>(text: &str, fit: &mut Fit<M>) -> Outcome<'a> {
    let cl = Clusters::new(text);
    // The extra cluster of an odd total goes to the head: the tail is usually a
    // file name whose last few clusters repeat across rows anyway.
    let split = |n: usize| (n.div_ceil(2), n / 2);
    let n = longest(cl.len() - 1, |n| {
        let (head, tail) = split(n);
        fit.fits_cut(cl.head(head), cl.tail(tail))
    });
    let (head, tail) = split(n);
    let head = snap_head(&cl, fit, head, cl.tail(tail));
    let tail = snap_tail(&cl, fit, tail, cl.head(head));
    let (kept_head, kept_tail) = (cl.head(head), cl.tail(tail));
    Outcome::cut(
        compose(kept_head, kept_tail),
        #[cfg(test)]
        head,
        #[cfg(test)]
        tail,
        clusters_in(kept_head) + clusters_in(kept_tail),
    )
}

/// The head [`Mode::Edges`] would rather keep: [`EDGE_HEAD`] clusters, extended
/// to just past the next break. When no break is within reach the plain six is
/// kept: running on to the cap would spend the budget on a prefix and leave the
/// tail, which is what identifies the token, with nothing.
fn preferred_head(cl: &Clusters) -> usize {
    let base = cl.len().min(EDGE_HEAD);
    let cap = cl.len().min(EDGE_HEAD_CAP);
    if base >= cap {
        return base;
    }
    (base..cap)
        .position(|at| is_break(cl.span(at, at + 1)))
        .map_or(base, |offset| base + offset + 1)
}

/// `None` when no head buys a tail worth showing.
fn cut_edges<'a, M: FnMut(&str) -> f32>(text: &str, fit: &mut Fit<M>) -> Option<Outcome<'a>> {
    let cl = Clusters::new(text);
    // Each candidate head is snapped back to a boundary before the tail is
    // sized, so the width a snap frees goes to the tail.
    let mut candidates: Vec<usize> = Vec::with_capacity(3);
    for head in [preferred_head(&cl), EDGE_HEAD, EDGE_HEAD_FLOOR] {
        if head == 0 || head > cl.len() {
            continue;
        }
        let head = snap_head(&cl, fit, head, "");
        if !candidates.contains(&head) {
            candidates.push(head);
        }
    }
    let mut best: Option<(usize, usize)> = None;
    for head in candidates {
        if !fit.fits_cut(cl.head(head), "") {
            continue;
        }
        // Width is monotone in the tail length, so a binary search finds the
        // longest tail that still fits behind this head.
        let tail = longest(cl.len() - head, |m| fit.fits_cut(cl.head(head), cl.tail(m)));
        if tail >= EDGE_MIN_TAIL.min(cl.len() - head) {
            best = Some((head, tail));
            break;
        }
        if best.is_none_or(|(_, kept)| tail > kept) {
            best = Some((head, tail));
        }
    }
    let (head, tail) = best.filter(|&(_, tail)| tail > 0)?;
    let tail = snap_tail(&cl, fit, tail, cl.head(head));
    let (kept_head, kept_tail) = (cl.head(head), cl.tail(tail));
    Some(Outcome::cut(
        compose(kept_head, kept_tail),
        #[cfg(test)]
        head,
        #[cfg(test)]
        tail,
        clusters_in(kept_head) + clusters_in(kept_tail),
    ))
}

/// The separator a path spells itself with. A path carrying a single `\` is
/// a Windows path and has to be put back together with `\`: rejoining it with
/// `/` would make one tab spell its location two ways, `C:\Users\dev\app`
/// while it fits and `C:/…/app` once it has to be elided.
pub(crate) fn path_separator(path: &str) -> char {
    if path.contains('\\') { '\\' } else { '/' }
}

pub(crate) fn join_segments(segments: &[&str], sep: char) -> String {
    segments.join(sep.encode_utf8(&mut [0u8; 4]) as &str)
}

/// The `\\server\share` (or `//server/share`) a path starts with, sliced from
/// the text as written: two leading separators, then a server and a share.
/// It is one unit like a drive letter, kept whole or not at all, since a
/// server without its share names another resource.
fn unc_anchor<'a>(text: &'a str, segments: &[&str]) -> Option<&'a str> {
    match segments {
        ["", "", server, share, ..] if !server.is_empty() && !share.is_empty() => {
            Some(&text[..2 + server.len() + 1 + share.len()])
        }
        _ => None,
    }
}

fn cut_path<'a, M: FnMut(&str) -> f32>(text: &'a str, fit: &mut Fit<M>) -> Outcome<'a> {
    let sep = path_separator(text);
    let segments: Vec<&str> = text.split(['/', '\\']).collect();
    // A leading separator splits into an empty first segment, and the root is
    // that separator as the path was written (`/` or `\`); `~` and drive
    // letters (`E:`) carry the same "where this tree lives" weight, and a
    // leading `…` means an earlier elision already happened — that marker is
    // replaced by the new one instead of stacking two ellipses. Keep whichever
    // marker there is so the result never reads as a bare relative path.
    let root: &str = match segments.first() {
        Some(&"") => &text[..1],
        Some(&"~") => "~",
        Some(&"…") => "",
        Some(head) if head.ends_with(':') => head,
        _ => "",
    };
    let root_kept = segments
        .first()
        .is_some_and(|s| s.is_empty() || *s == "~" || *s == "…" || s.ends_with(':'));
    let anchor = unc_anchor(text, &segments);
    let rooted = segments.first().is_some_and(|s| s.is_empty());
    let prefix = if let Some(anchor) = anchor {
        format!("{anchor}{sep}…{sep}")
    } else if root.is_empty() {
        format!("…{sep}")
    } else if rooted {
        // The root is already the separator itself.
        format!("{root}…{sep}")
    } else {
        format!("{root}{sep}…{sep}")
    };
    // Drop whole segments from the front until the remaining tail fits. The
    // width only shrinks as segments leave, so the first fit is the widest
    // one — greedy is optimal here.
    //
    // With a root marker, `head = 1` would spell the root, the ellipsis, and
    // then every remaining segment — strictly wider than the original that
    // already failed to fit — so that candidate is skipped rather than measured.
    // A UNC anchor spans four segments, so its first candidate is `head = 5`.
    let mut head = match (anchor, root_kept) {
        (Some(_), _) => 5,
        (None, true) => 2,
        (None, false) => 0,
    };
    while head < segments.len() {
        let rest = join_segments(&segments[head..], sep);
        let candidate = if head == 0 {
            rest.clone()
        } else {
            format!("{prefix}{rest}")
        };
        if fit.fits(&candidate) {
            let kept = rest.graphemes(true).count();
            return Outcome {
                text: Cow::Owned(candidate),
                #[cfg(test)]
                head: 0,
                #[cfg(test)]
                tail: kept,
                kept,
                whole: head == 0,
            };
        }
        if head + 1 >= segments.len() {
            break;
        }
        head += 1;
    }
    // Even the last segment behind a root is too wide: keep it alone when it
    // fits, else its tail after the ellipsis, with no slash so the reader sees
    // the segment was torn.
    let last = segments[segments.len() - 1];
    if fit.fits(last) {
        let kept = last.graphemes(true).count();
        Outcome {
            text: Cow::Borrowed(last),
            #[cfg(test)]
            head: 0,
            #[cfg(test)]
            tail: kept,
            kept,
            whole: false,
        }
    } else {
        cut_start(last, fit)
    }
}

fn elide_text<'a, M: FnMut(&str) -> f32>(
    text: &'a str,
    mode: Mode,
    fit: &mut Fit<M>,
) -> Outcome<'a> {
    let text = match mode {
        Mode::Path | Mode::Edges | Mode::Label => text.trim(),
        Mode::End | Mode::Start | Mode::Middle => text,
    };
    if text.is_empty() || fit.fits(text) {
        return Outcome::whole(text);
    }
    if !fit.fits(ELLIPSIS) {
        return Outcome::nothing();
    }
    match mode {
        Mode::End => cut_end(text, fit),
        Mode::Start => cut_start(text, fit),
        Mode::Middle => cut_middle(text, fit),
        // No head buys a tail worth showing: a bare tail says more.
        Mode::Edges => cut_edges(text, fit).unwrap_or_else(|| elide_text(text, Mode::Start, fit)),
        Mode::Path => cut_path(text, fit),
        // Told apart only now that an elision is needed, from the text alone.
        Mode::Label => {
            let resolved = if text.contains(['/', '\\']) {
                Mode::Path
            } else {
                Mode::Edges
            };
            elide_text(text, resolved, fit)
        }
    }
}

/// A title and the localized words that ride behind it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Suffixed<'a> {
    pub head: Cow<'a, str>,
    pub tail: Cow<'a, str>,
}

impl Suffixed<'_> {
    pub(crate) fn joined(&self) -> String {
        format!("{}{}", self.head, self.tail)
    }
}

/// What may lead a suffix without being part of what it says: the ` — ` that
/// joins it to the title. Dropped when the title is not shown, so a suffix
/// standing alone never opens with a dangling dash.
fn is_joiner(c: char) -> bool {
    c.is_whitespace() || matches!(c, '—' | '–' | '-' | '|' | ':' | '·')
}

/// The whole elision a label asks for: `text` under `mode`, followed by `tail`
/// (` — disconnected`, ` · `) that is never cut, within `limits`. `None` means
/// the label is hidden because fewer than [`Limits::min_visible`] clusters of
/// the original survive; the tail goes with it.
///
/// The caller hands over the tail already localized, from the state it
/// describes; nothing here parses it. The tail is measured in the string it
/// ends, so the budget is exact rather than a sum of two widths, and the scalar
/// cap applies to the elided text alone.
///
/// Without `min_visible`, a title that cannot keep even one cluster beside the
/// tail is dropped and the tail stands alone without its joiner, cut from the
/// end when it alone exceeds the budget.
pub(crate) fn elide_to<'a, M: FnMut(&str) -> f32>(
    text: &'a str,
    tail: Option<&'a str>,
    limits: Limits,
    mode: Mode,
    measure: &mut M,
) -> Option<Suffixed<'a>> {
    let Limits {
        budget,
        max_chars,
        min_visible,
    } = limits;
    let min_visible = min_visible.filter(|&n| n > 0);
    let hides = |out: &Outcome| min_visible.is_some_and(|n| !out.whole && out.kept() < n);

    let Some(tail) = tail else {
        let mut fit = Fit {
            measure,
            budget,
            max_chars,
        };
        let out = elide_text(text, mode, &mut fit);
        return (!hides(&out)).then(|| Suffixed {
            head: out.text,
            tail: Cow::Borrowed(""),
        });
    };

    let bare_tail = |measure: &mut M| -> Option<Suffixed<'a>> {
        if min_visible.is_some() {
            return None;
        }
        let mut fit = Fit {
            measure,
            budget,
            max_chars: None,
        };
        let tail = elide_text(tail.trim_start_matches(is_joiner), Mode::End, &mut fit).text;
        Some(Suffixed {
            head: Cow::Borrowed(""),
            tail,
        })
    };
    if text.trim().is_empty() || measure(tail) > budget {
        return bare_tail(measure);
    }
    let mut with_tail = |s: &str| measure(&[s, tail].concat());
    let mut fit = Fit {
        measure: &mut with_tail,
        budget,
        max_chars,
    };
    let out = elide_text(text, mode, &mut fit);
    if hides(&out) {
        return None;
    }
    // Nothing of the title survived beside the tail.
    if !out.whole && out.kept() == 0 {
        return bare_tail(measure);
    }
    Some(Suffixed {
        head: out.text,
        tail: Cow::Borrowed(tail),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit per grapheme cluster: the width model of a character budget.
    fn mono(text: &str) -> f32 {
        text.graphemes(true).count() as f32
    }

    /// CJK and emoji clusters take two units, everything else one — enough of
    /// a mixed-script model to catch a cut sized by counting instead of
    /// measuring.
    fn wide(text: &str) -> f32 {
        text.graphemes(true)
            .map(|g| {
                let c = g.chars().next().unwrap_or(' ') as u32;
                let double = matches!(c, 0x2E80..=0x9FFF | 0xAC00..=0xD7A3 | 0xFF00..=0xFF60)
                    || c >= 0x1F300;
                if double { 2. } else { 1. }
            })
            .sum()
    }

    /// No cap, no tail: the plain elision most of these tests are about.
    fn elide<'a, M: FnMut(&str) -> f32>(
        text: &'a str,
        budget: f32,
        mode: Mode,
        measure: &mut M,
    ) -> Cow<'a, str> {
        let mut fit = Fit {
            measure,
            budget,
            max_chars: None,
        };
        elide_text(text, mode, &mut fit).text
    }

    /// A suffixed elision that has no `min_visible`, so it can never be hidden.
    fn suffixed<'a, M: FnMut(&str) -> f32>(
        title: &'a str,
        tail: &'a str,
        budget: f32,
        mode: Mode,
        measure: &mut M,
    ) -> Suffixed<'a> {
        elide_to(
            title,
            Some(tail),
            Limits {
                budget,
                max_chars: None,
                min_visible: None,
            },
            mode,
            measure,
        )
        .expect("without min_visible a label is never hidden")
    }

    fn run(text: &str, budget: f32, mode: Mode) -> String {
        elide(text, budget, mode, &mut mono).into_owned()
    }

    const MODES: [Mode; 5] = [
        Mode::End,
        Mode::Start,
        Mode::Middle,
        Mode::Edges,
        Mode::Path,
    ];

    /// Every way of slicing `text` on a cluster boundary.
    fn prefixes(text: &str) -> Vec<String> {
        let cells: Vec<&str> = text.graphemes(true).collect();
        (0..=cells.len()).map(|n| cells[..n].concat()).collect()
    }

    fn suffixes(text: &str) -> Vec<String> {
        let cells: Vec<&str> = text.graphemes(true).collect();
        (0..=cells.len())
            .map(|n| cells[cells.len() - n..].concat())
            .collect()
    }

    /// Mixed fixtures: Latin, Russian, CJK, spaces, combining marks and emoji
    /// clusters that a `char`-indexed cut would tear.
    const FIXTURES: [&str; 12] = [
        "window-transparency-backdrop",
        "fix/rpc-proxy-and-error-classification",
        "Backend server logs and more words",
        "~/projects/toolbox/src/ui/tab_sidebar.rs",
        r"C:\Users\dev\AppData\Local\Temp\verify-build",
        "ветка/исправление-длинного-названия",
        "文档/设计/源代码管理方案.md",
        "ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}cdefghijklmnop",
        "abcdef\u{2764}\u{fe0f}ghijklmnopqr",
        "abcdef\u{1F1E8}\u{1F1F3}ghijklmnopqr",
        "abcde\u{301}fghijklmnopqrst",
        "verylongbranchnamewithoutseps",
    ];

    #[test]
    fn text_that_fits_comes_back_borrowed_and_untouched() {
        for mode in MODES {
            for text in ["main", "exact", "", "a/b"] {
                let out = elide(text, mono(text), mode, &mut mono);
                assert!(
                    matches!(out, Cow::Borrowed(s) if s == text),
                    "{mode:?} {text}"
                );
                let out = elide(text, 100., mode, &mut mono);
                assert!(
                    matches!(out, Cow::Borrowed(s) if s == text),
                    "{mode:?} {text}"
                );
            }
        }
    }

    #[test]
    fn a_budget_that_cannot_hold_an_ellipsis_yields_nothing() {
        for mode in MODES {
            for budget in [-5., 0., 0.5, f32::NAN] {
                assert_eq!(run("abcdefghij", budget, mode), "", "{mode:?} @{budget}");
            }
        }
    }

    #[test]
    fn every_mode_fits_every_budget_with_at_most_one_ellipsis() {
        for mode in MODES {
            for text in FIXTURES {
                for measure in [mono as fn(&str) -> f32, wide] {
                    for budget in 0..=(wide(text) as usize + 2) {
                        let budget = budget as f32;
                        let out = elide(text, budget, mode, &mut { measure });
                        assert!(
                            measure(&out) <= budget,
                            "{mode:?} {text:?} @{budget} -> {out:?}"
                        );
                        assert!(
                            out.matches('…').count() <= 1,
                            "{mode:?} {text:?} @{budget} -> {out:?}"
                        );
                    }
                }
            }
        }
    }

    /// Whatever survives either side of the ellipsis is a cluster-aligned
    /// prefix and suffix of the input: no joiner dangles, no variation
    /// selector is stripped, no half flag is left to render as a bare letter.
    #[test]
    fn cuts_land_on_cluster_boundaries() {
        for mode in [Mode::End, Mode::Start, Mode::Middle, Mode::Edges] {
            for text in FIXTURES {
                for budget in 1..=(wide(text) as usize) {
                    let out = elide(text, budget as f32, mode, &mut wide);
                    let Some((head, tail)) = out.split_once('…') else {
                        assert_eq!(out, text.trim());
                        continue;
                    };
                    assert!(
                        prefixes(text).iter().any(|p| p == head),
                        "{mode:?} head {head:?} of {text:?} @{budget}"
                    );
                    assert!(
                        suffixes(text).iter().any(|s| s == tail),
                        "{mode:?} tail {tail:?} of {text:?} @{budget}"
                    );
                }
            }
        }
    }

    /// More room never keeps fewer clusters, snapping included.
    #[test]
    fn kept_text_grows_monotonically_with_the_budget() {
        for mode in [Mode::End, Mode::Start, Mode::Middle] {
            for text in FIXTURES {
                for measure in [mono as fn(&str) -> f32, wide] {
                    let mut kept = 0;
                    for budget in 1..=(wide(text) as usize + 1) {
                        let out = elide(text, budget as f32, mode, &mut { measure });
                        let now = out.replace('…', "").graphemes(true).count();
                        assert!(
                            now >= kept,
                            "{mode:?} {text:?} @{budget}: kept {now} after {kept}"
                        );
                        kept = now;
                    }
                }
            }
        }
    }

    // ---- word boundaries: what the old mid-word cuts got wrong ----

    #[test]
    fn end_cut_snaps_back_to_a_word_boundary() {
        // Without snapping this is `fix/rpc-proxy-a…`, a word torn in half.
        assert_eq!(
            run("fix/rpc-proxy-and-error-classification", 16., Mode::End),
            "fix/rpc-proxy-…"
        );
        // A space is dropped rather than left dangling before the ellipsis.
        assert_eq!(
            run("Backend server logs and more words", 17., Mode::End),
            "Backend server…"
        );
    }

    #[test]
    fn start_cut_snaps_forward_to_a_word_boundary() {
        // Without snapping this keeps `ate-final.tar.gz`, starting mid-word.
        assert_eq!(
            run(
                "build/artifacts/release-candidate-final.tar.gz",
                17.,
                Mode::Start
            ),
            "…final.tar.gz"
        );
    }

    #[test]
    fn start_cut_prefers_the_extension_to_half_a_word() {
        // Unsnapped this is `…est.rs`.
        assert_eq!(
            run("some_long_module_name.test.rs", 7., Mode::Start),
            "….rs"
        );
    }

    #[test]
    fn middle_cut_keeps_both_ends_on_boundaries() {
        // Head would end at `crates/tty`; the tail is already a whole name.
        let out = run(
            "crates/tty59-core/src/core/git/status.rs",
            20.,
            Mode::Middle,
        );
        assert_eq!(out, "crates/…status.rs");
        assert!(mono(&out) <= 20.);
    }

    #[test]
    fn a_cut_with_no_boundary_in_reach_keeps_the_best_fitting_cut() {
        assert_eq!(run("abcdefghij", 5., Mode::Middle), "ab…ij");
        assert_eq!(run("abcdefghij", 6., Mode::Middle), "abc…ij");
        assert_eq!(run("abcdefghijklmnop", 6., Mode::End), "abcde…");
        assert_eq!(run("abcdefghijklmnop", 6., Mode::Start), "…lmnop");
    }

    #[test]
    fn a_boundary_beyond_eight_clusters_is_not_chased() {
        // The only break is 11 clusters from the best cut.
        let text = "a-bcdefghijklmnopqrstuvwxyz";
        assert_eq!(run(text, 14., Mode::End), "a-bcdefghijkl…");
    }

    #[test]
    fn middle_degenerate_budgets() {
        assert_eq!(run("abcdef", 3., Mode::Middle), "a…f");
        assert_eq!(run("abcdef", 1., Mode::Middle), "…");
    }

    // ---- Edges (the old keep-edges contract) ----

    #[test]
    fn edges_keep_a_head_and_the_identifying_tail() {
        let out = run("window-transparency-backdrop", 16., Mode::Edges);
        assert!(out.starts_with("window"), "{out}");
        assert!(out.ends_with("backdrop"), "{out}");
        assert!(mono(&out) <= 16.);
    }

    #[test]
    fn edges_keep_a_head_on_a_separatorless_token() {
        for budget in [10., 12., 14., 20.] {
            let out = run("verylongbranchnamewithoutseps", budget, Mode::Edges);
            assert!(out.starts_with('v'), "{out}");
            assert!(out.ends_with('s'), "{out}");
        }
    }

    #[test]
    fn edges_fall_back_to_a_bare_tail_when_no_head_fits() {
        let out = run("window-transparency-backdrop", 3., Mode::Edges);
        assert!(out.starts_with('…'), "{out}");
        assert!(mono(&out) <= 3.);
    }

    // ---- Path ----

    #[test]
    fn path_keeps_root_and_tail_segments() {
        assert_eq!(
            run("~/projects/toolbox/src/ui/tab_sidebar.rs", 24., Mode::Path),
            "~/…/ui/tab_sidebar.rs"
        );
        assert_eq!(
            run("/usr/local/share/man/man1/git.1", 14., Mode::Path),
            "/…/man1/git.1"
        );
    }

    #[test]
    fn path_keeps_windows_separators_and_drive() {
        let out = run(
            r"C:\Users\dev\AppData\Local\Temp\verify-build",
            28.,
            Mode::Path,
        );
        assert_eq!(out, r"C:\…\Local\Temp\verify-build");
        assert!(!out.contains('/'));
        let unix = run("/home/dev/projects/toolbox/src/ui/app.rs", 20., Mode::Path);
        assert!(!unix.contains('\\'), "{unix}");
    }

    #[test]
    fn path_tears_only_the_last_segment_and_without_a_slash() {
        let out = run("E:/supercalifragilisticexpialidocious", 8., Mode::Path);
        assert_eq!(out, "…docious");
        assert!(mono(&out) <= 8.);
    }

    #[test]
    fn path_shows_a_whole_last_segment_that_fits_alone() {
        // Root and ellipsis would not fit beside it, but the name does.
        assert_eq!(
            run("~/projects/very-long-name", 14., Mode::Path),
            "very-long-name"
        );
    }

    #[test]
    fn path_replaces_an_earlier_leading_ellipsis() {
        let out = run("…/deep/er/path/here/file.rs", 15., Mode::Path);
        assert_eq!(out.matches('…').count(), 1, "{out}");
        assert!(out.ends_with("file.rs"), "{out}");
    }

    // ---- Russian and CJK ----

    #[test]
    fn russian_and_cjk_are_cut_by_measured_width() {
        let ru = "ветка/исправление-длинного-названия";
        let out = elide(ru, 12., Mode::End, &mut wide);
        assert!(wide(&out) <= 12.);
        assert_eq!(out, "ветка/…");
        let zh = "文档/设计/源代码管理方案.md";
        let out = elide(zh, 14., Mode::Middle, &mut wide);
        assert!(wide(&out) <= 14.);
        assert!(out.ends_with(".md"), "{out}");
        assert!(out.starts_with("文档/"), "{out}");
    }

    // ---- suffix contract (ended tab titles) ----

    /// The three states in the three shipped languages, as the caller would
    /// hand them over.
    const SUFFIXES: [&str; 9] = [
        " — disconnected",
        " — not connected",
        " — process exited",
        " — отключено",
        " — нет соединения",
        " — процесс завершён",
        " — 已断开连接",
        " — 未连接",
        " — 进程已退出",
    ];

    #[test]
    fn suffix_stays_whole_while_the_title_gives_way() {
        for tail in SUFFIXES {
            let title = "user@prod-eu-west-1.example.internal:~/projects/service";
            let budget = wide(tail) + 10.;
            let out = suffixed(title, tail, budget, Mode::Edges, &mut wide);
            assert_eq!(out.tail, tail, "the suffix is never cut");
            assert!(out.head.contains('…'), "{out:?}");
            assert!(wide(&out.joined()) <= budget, "{out:?} @{budget}");
        }
    }

    #[test]
    fn a_title_that_fits_with_its_suffix_is_untouched() {
        for tail in SUFFIXES {
            let budget = wide("zsh") + wide(tail);
            let out = suffixed("zsh", tail, budget, Mode::Edges, &mut wide);
            assert!(matches!(out.head, Cow::Borrowed("zsh")));
            assert!(matches!(out.tail, Cow::Borrowed(t) if t == tail));
        }
    }

    #[test]
    fn a_suffix_wider_than_the_budget_is_cut_itself_and_the_title_dropped() {
        for tail in SUFFIXES {
            // Narrower than the suffix even once its joiner is gone.
            let budget = wide(tail.trim_start_matches(is_joiner)) - 3.;
            let out = suffixed("server", tail, budget, Mode::Edges, &mut wide);
            assert_eq!(out.head, "", "{tail}");
            assert!(out.tail.contains('…'), "{out:?}");
            assert!(
                !out.tail.starts_with(is_joiner),
                "no dangling joiner: {out:?}"
            );
            assert!(wide(&out.joined()) <= budget, "{out:?} @{budget}");
        }
    }

    #[test]
    fn a_title_that_cannot_keep_a_cluster_beside_the_suffix_is_dropped() {
        for tail in SUFFIXES {
            // Room for the suffix, and for nothing else.
            let budget = wide(tail);
            let out = suffixed("server-name", tail, budget, Mode::Edges, &mut wide);
            assert_eq!(out.head, "", "{tail}");
            assert_eq!(out.joined(), tail.trim_start_matches(is_joiner), "{tail}");
        }
    }

    #[test]
    fn an_empty_title_leaves_the_suffix_without_its_joiner() {
        let out = suffixed("  ", " — disconnected", 40., Mode::Edges, &mut mono);
        assert_eq!(out.joined(), "disconnected");
    }

    #[test]
    fn suffix_fit_holds_at_every_budget() {
        for tail in SUFFIXES {
            for budget in 0..=60 {
                let budget = budget as f32;
                let title = "very-long-hostname.example.com";
                let out = suffixed(title, tail, budget, Mode::Edges, &mut wide);
                assert!(wide(&out.joined()) <= budget, "{out:?} @{budget}");
                assert!(out.joined().matches('…').count() <= 1, "{out:?} @{budget}");
            }
        }
    }

    // ---- the structure of a cut, not the look of its output ----

    fn outcome<'a>(text: &'a str, budget: f32, mode: Mode, cap: Option<usize>) -> Outcome<'a> {
        let mut measure = mono;
        let mut fit = Fit {
            measure: &mut measure,
            budget,
            max_chars: cap,
        };
        elide_text(text, mode, &mut fit)
    }

    /// Fixtures that carry an `…` of their own, which is retained text and not
    /// the marker this module inserts.
    const AUTHORED: [&str; 3] = [
        "ab…cdefghijklmnopqrst",
        "wait…what-is-this-long-name",
        "release-notes-final-…-draft-copy",
    ];

    #[test]
    fn edges_snap_the_head_too() {
        // The head used to stop at `ab-lon`, tearing the word although the
        // boundary after `ab-` was three clusters back.
        assert_eq!(run("ab-longtoken-suffix", 12., Mode::Edges), "ab-…suffix");
    }

    /// If a boundary lies within reach behind a cut, the cut sits on a boundary:
    /// head cuts after a break, tail cuts after a break or on an extension dot.
    /// Read off the structural head/tail counts, so an authored `…` cannot be
    /// mistaken for the inserted one.
    #[test]
    fn cuts_sit_on_a_boundary_whenever_one_is_in_reach() {
        let texts = FIXTURES.iter().chain(AUTHORED.iter());
        for text in texts {
            for mode in [Mode::End, Mode::Start, Mode::Middle, Mode::Edges] {
                let trimmed = match mode {
                    Mode::Edges => text.trim(),
                    _ => text,
                };
                let cl = Clusters::new(trimmed);
                for budget in 1..=(wide(text) as usize) {
                    let out = outcome(text, budget as f32, mode, None);
                    if out.whole || out.kept() == 0 {
                        continue;
                    }
                    let head = out.head;
                    if head > 0 && !cl.head_boundary(head) {
                        for at in head.saturating_sub(SNAP_REACH).max(1)..head {
                            assert!(
                                !cl.head_boundary(at),
                                "{mode:?} {text:?} @{budget}: head {head} passes boundary {at}"
                            );
                        }
                    }
                    if out.tail > 0 {
                        let start = cl.len() - out.tail;
                        if !cl.tail_boundary(start) {
                            for at in start + 1..=(start + SNAP_REACH).min(cl.len() - 1) {
                                assert!(
                                    !cl.tail_boundary(at),
                                    "{mode:?} {text:?} @{budget}: tail {start} passes boundary {at}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// One inserted marker, counted apart from retained text: the output has
    /// exactly the kept clusters plus one, whether or not the text came with
    /// an `…` of its own.
    #[test]
    fn kept_counts_are_structural_not_a_scan_for_the_marker() {
        for text in AUTHORED {
            for mode in [Mode::End, Mode::Start, Mode::Middle, Mode::Edges] {
                for budget in 2..=(text.graphemes(true).count()) {
                    let out = outcome(text, budget as f32, mode, None);
                    if out.whole {
                        continue;
                    }
                    assert_eq!(
                        out.text.graphemes(true).count(),
                        out.kept() + 1,
                        "{mode:?} {text:?} @{budget} -> {:?}",
                        out.text
                    );
                }
            }
        }
    }

    // ---- Mode::Label ----

    #[test]
    fn a_label_is_a_path_when_it_has_a_separator_and_edges_otherwise() {
        for path in [
            "~/projects/toolbox/src/ui/tab_sidebar.rs",
            r"C:\Users\dev\AppData\Local\Temp\verify-build",
        ] {
            for budget in 1..=48 {
                assert_eq!(
                    run(path, budget as f32, Mode::Label),
                    run(path, budget as f32, Mode::Path),
                    "{path} @{budget}"
                );
            }
        }
        for word in [
            "Backend server logs",
            "npm run dev -- --watch",
            "window-transparency-backdrop",
        ] {
            for budget in 1..=30 {
                assert_eq!(
                    run(word, budget as f32, Mode::Label),
                    run(word, budget as f32, Mode::Edges),
                    "{word} @{budget}"
                );
            }
        }
    }

    /// A path that fits is left exactly as it came: its mixed separators would
    /// be normalised by the path rejoin, so this fails if the label is
    /// classified and cut before the fit check.
    #[test]
    fn a_label_that_fits_is_not_classified_or_rejoined() {
        let out = elide(r"a/b\c", 10., Mode::Label, &mut mono);
        assert!(matches!(out, Cow::Borrowed(r"a/b\c")), "{out}");
    }

    #[test]
    fn a_label_trims_like_path_and_edges() {
        assert_eq!(run("  main  ", 10., Mode::Label), "main");
        assert_eq!(run("  ~/a/b  ", 10., Mode::Label), "~/a/b");
    }

    // ---- joint limits: pixel budget and scalar cap together ----

    fn limited<'a>(
        text: &'a str,
        tail: Option<&'a str>,
        budget: f32,
        cap: Option<usize>,
        mode: Mode,
        measure: fn(&str) -> f32,
    ) -> Option<Suffixed<'a>> {
        let limits = Limits {
            budget,
            max_chars: cap,
            min_visible: None,
        };
        let mut measure = measure;
        elide_to(text, tail, limits, mode, &mut measure)
    }

    /// A candidate over the cap is rejected before it is measured: the measure
    /// is never handed one, whatever the budget.
    #[test]
    fn the_cap_rejects_before_anything_is_measured() {
        let mut longest_seen = 0;
        let mut measure = |s: &str| {
            longest_seen = longest_seen.max(s.chars().count());
            mono(s)
        };
        for mode in MODES {
            for text in FIXTURES {
                let mut fit = Fit {
                    measure: &mut measure,
                    budget: f32::INFINITY,
                    max_chars: Some(10),
                };
                let out = elide_text(text, mode, &mut fit);
                assert!(out.text.chars().count() <= 10, "{mode:?} {text:?}");
            }
        }
        assert!(
            longest_seen <= 10,
            "measured a {longest_seen}-scalar candidate"
        );
    }

    #[test]
    fn both_limits_hold_at_every_combination() {
        for mode in MODES {
            for text in FIXTURES {
                for measure in [mono as fn(&str) -> f32, wide] {
                    for cap in 0..=20 {
                        for budget in 0..=30 {
                            let budget = budget as f32;
                            let out = limited(text, None, budget, Some(cap), mode, measure)
                                .expect("no min_visible");
                            let head = out.head;
                            assert!(
                                head.chars().count() <= cap,
                                "{mode:?} {text:?} cap {cap} @{budget} -> {head:?}"
                            );
                            assert!(
                                measure(&head) <= budget,
                                "{mode:?} {text:?} cap {cap} @{budget} -> {head:?}"
                            );
                            assert!(head.matches('…').count() <= 1, "{head:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_cap_binds_when_pixels_are_plentiful() {
        let out = limited(
            "feature/long-branch-name-here",
            None,
            f32::INFINITY,
            Some(10),
            Mode::Middle,
            mono,
        )
        .unwrap();
        assert_eq!(out.head, "featu…here");
    }

    #[test]
    fn pixels_bind_when_the_cap_is_plentiful() {
        let text = "文档/设计/源代码管理方案.md";
        let capped = limited(text, None, 10., Some(18), Mode::Middle, wide).unwrap();
        assert_eq!(capped.head, elide(text, 10., Mode::Middle, &mut wide));
        assert!(wide(&capped.head) <= 10.);
    }

    /// The cap counts scalar values and the cuts stay on clusters: `é` spelt
    /// with a combining mark costs two, and a cluster that would cross the cap
    /// is dropped whole.
    #[test]
    fn the_cap_counts_scalars_and_cuts_on_clusters() {
        let combining = "abcde\u{301}fghijklmnop";
        assert_eq!(
            limited(combining, None, f32::INFINITY, Some(6), Mode::End, mono)
                .unwrap()
                .head,
            "abcd…"
        );
        let family = "ab\u{1F468}\u{200d}\u{1F469}\u{200d}\u{1F467}cdefghijklmnop";
        for cap in 0..=8 {
            let out = limited(family, None, f32::INFINITY, Some(cap), Mode::Middle, mono)
                .unwrap()
                .head
                .into_owned();
            assert!(out.chars().count() <= cap, "cap {cap} -> {out:?}");
            if let Some((head, tail)) = out.split_once('…') {
                assert!(prefixes(family).iter().any(|p| p == head), "{out:?}");
                assert!(suffixes(family).iter().any(|s| s == tail), "{out:?}");
            }
        }
    }

    #[test]
    fn the_cap_counts_chars_not_bytes_in_russian_and_cjk() {
        assert_eq!(
            limited(
                "ветка/исправление-длинного",
                None,
                f32::INFINITY,
                Some(8),
                Mode::End,
                mono
            )
            .unwrap()
            .head,
            "ветка/…"
        );
        assert_eq!(
            limited(
                "文档/设计/源代码管理方案.md",
                None,
                f32::INFINITY,
                Some(5),
                Mode::End,
                wide
            )
            .unwrap()
            .head,
            "文档/…"
        );
    }

    #[test]
    fn the_cap_leaves_the_tail_whole_and_uncounted() {
        let out = limited(
            "feature/long-branch-name",
            Some(" +3"),
            40.,
            Some(10),
            Mode::Middle,
            mono,
        )
        .unwrap();
        assert_eq!(out.head, "featu…name");
        assert_eq!(out.tail, " +3");
        assert!(out.joined().chars().count() > 10);
        assert!(mono(&out.joined()) <= 40.);
    }

    /// The column contract the old `elide_middle(max_chars)` had, now a scalar
    /// cap on a middle cut: borrowed when it fits, both ends kept, one
    /// ellipsis, never over the cap, never a cluster torn.
    #[test]
    fn a_middle_cut_under_a_scalar_cap_keeps_the_old_column_contract() {
        let cut = |text, cap| {
            limited(text, None, f32::INFINITY, Some(cap), Mode::Middle, mono)
                .unwrap()
                .head
                .into_owned()
        };
        for (text, cap) in [("short", 10), ("exact", 5)] {
            let out = limited(text, None, f32::INFINITY, Some(cap), Mode::Middle, mono).unwrap();
            assert!(matches!(out.head, Cow::Borrowed(t) if t == text), "{text}");
        }
        let out = cut("crates/tty59-core/src/core/git/status.rs", 20);
        assert!(out.chars().count() <= 20, "{out}");
        assert_eq!(out.matches('…').count(), 1, "{out}");
        assert!(
            out.starts_with("crates/") && out.ends_with("status.rs"),
            "{out}"
        );
        assert_eq!(cut("abcdefghij", 5), "ab…ij");
        assert_eq!(cut("abcdefghij", 6), "abc…ij");
        assert_eq!(cut("abcdef", 3), "a…f");
        let out = cut("文档/设计/源代码管理方案.md", 8);
        assert!(out.chars().count() <= 8, "{out}");
        assert!(out.starts_with("文档/") && out.ends_with(".md"), "{out}");
        let combining = "abcde\u{301}fghijklmnop";
        for cap in 0..=combining.chars().count() + 1 {
            let out = cut(combining, cap);
            assert!(out.chars().count() <= cap, "{cap}: {out:?}");
            if let Some((head, tail)) = out.split_once('…') {
                assert!(prefixes(combining).iter().any(|p| p == head), "{out:?}");
                assert!(suffixes(combining).iter().any(|s| s == tail), "{out:?}");
            }
        }
    }

    /// Infinity, NaN, zero, one and two, at the edges of both limits.
    #[test]
    fn extreme_limits() {
        let long = "abcdefghij";
        // An infinite budget and no cap changes nothing.
        let out = elide(long, f32::INFINITY, Mode::End, &mut mono);
        assert!(matches!(out, Cow::Borrowed("abcdefghij")));
        // A NaN budget accepts nothing, whatever the cap.
        for text in ["abc", long] {
            let out = limited(text, None, f32::NAN, Some(100), Mode::End, mono).unwrap();
            assert_eq!(out.head, "", "{text}");
        }
        // An infinite budget does not let an over-cap candidate through.
        for cap in 0..=2 {
            for mode in [Mode::End, Mode::Start, Mode::Middle] {
                let out = limited(long, None, f32::INFINITY, Some(cap), mode, mono).unwrap();
                assert!(out.head.chars().count() <= cap, "{mode:?} cap {cap}");
            }
        }
        let head = |cap, mode| {
            limited(long, None, f32::INFINITY, Some(cap), mode, mono)
                .unwrap()
                .head
                .into_owned()
        };
        assert_eq!(head(0, Mode::End), "");
        assert_eq!(head(1, Mode::End), "…");
        assert_eq!(head(2, Mode::End), "a…");
        assert_eq!(head(2, Mode::Middle), "a…");
        assert_eq!(head(2, Mode::Start), "…j");
        // No room at all, whatever the cap.
        assert_eq!(
            limited(long, None, 0., Some(5), Mode::End, mono)
                .unwrap()
                .head,
            ""
        );
    }

    // ---- examples carried over from the sidebar helper tests ----

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

    /// A path or branch that fits is left exactly as it came, shallow or deep,
    /// whichever separator it carries.
    #[test]
    fn a_path_that_fits_is_left_exactly_as_it_came() {
        for text in [
            "~/tty59",
            "E:/work/toolbox/crates/tty59-core/src/client",
            r"C:\Users\dev\projects\toolbox\src\ui\app.rs",
        ] {
            let out = elide(text, mono(text) + 1., Mode::Path, &mut mono);
            assert!(matches!(out, Cow::Borrowed(t) if t == text), "{out}");
        }
        let out = elide("main", mono("main") + 1., Mode::Edges, &mut mono);
        assert!(matches!(out, Cow::Borrowed("main")), "{out}");
    }

    #[test]
    fn a_path_keeps_its_drive_and_its_tail_within_the_budget() {
        let out = run("E:/work/toolbox/src/ui/tab_sidebar.rs", 24., Mode::Path);
        assert_eq!(out, "E:/…/ui/tab_sidebar.rs");
        assert!(mono(&out) <= 24.);
    }

    /// A head that fits but leaves nothing behind the ellipsis says less than a
    /// shorter head that keeps the identifying tail.
    #[test]
    fn edges_give_up_head_room_to_keep_a_tail() {
        let out = run("feature/some-really-long-thing", 6., Mode::Edges);
        assert!(!out.ends_with('…'), "the tail is never traded away: {out}");
        assert!(out.ends_with('g'), "{out}");
        assert!(mono(&out) <= 6.);
    }

    /// Whatever survives either side of an ellipsis is cluster-aligned, for
    /// every mode and for the last-resort tear of a path's leaf: slicing by
    /// `char` instead tears `👨‍👩‍👧`, strips the selector off `❤️` or leaves half a flag.
    #[test]
    fn every_mode_cuts_cluster_fixtures_and_path_leaves_on_cluster_boundaries() {
        for text in CLUSTER_FIXTURES {
            for mode in [Mode::End, Mode::Start, Mode::Middle, Mode::Edges] {
                for budget in 3..=40 {
                    let out = run(text, budget as f32, mode);
                    assert!(mono(&out) <= budget as f32, "{mode:?} {out:?} @{budget}");
                    if let Some((head, tail)) = out.split_once('…') {
                        assert!(prefixes(text).iter().any(|p| p == head), "{out:?}");
                        assert!(suffixes(text).iter().any(|s| s == tail), "{out:?}");
                    }
                }
            }
            let path = format!("~/projects/toolbox/{text}");
            for budget in 3..=40 {
                let out = run(&path, budget as f32, Mode::Path);
                if let Some((_, tail)) = out.split_once('…') {
                    let tail = tail.trim_start_matches('/');
                    assert!(
                        suffixes(&path).iter().any(|s| s == tail),
                        "tail {tail:?} of {path:?} @{budget}"
                    );
                }
            }
        }
    }

    // ---- rooted and UNC paths keep their own root ----

    /// A rooted Windows path keeps the root it was written with, and the
    /// separator it was written with: `\…\etc\hosts`, never `/…\etc\hosts`.
    #[test]
    fn rooted_backslash_paths_keep_their_own_root() {
        let path = r"\Windows\System32\drivers\etc\hosts";
        let out = run(path, 12., Mode::Path);
        assert_eq!(out, r"\…\etc\hosts");
        assert!(!out.contains('/'), "{out}");
    }

    /// `\\server\share` is one anchor, like a drive letter: kept whole in
    /// front of the ellipsis while it and the tail fit.
    #[test]
    fn a_unc_anchor_stays_whole_beside_the_tail() {
        let path = r"\\srv\share\dir\sub\file.txt";
        assert_eq!(run(path, 26., Mode::Path), r"\\srv\share\…\sub\file.txt");
        assert_eq!(run(path, 25., Mode::Path), r"\\srv\share\…\file.txt");
        assert_eq!(run(path, 22., Mode::Path), r"\\srv\share\…\file.txt");
    }

    /// When the anchor, the ellipsis and the leaf do not fit, the whole anchor
    /// goes — never half of it, since `\\srv` alone names another resource —
    /// and the leaf rule takes over: the leaf alone, or torn tail-first.
    #[test]
    fn a_unc_anchor_is_dropped_whole_when_it_cannot_fit() {
        let path = r"\\srv\share\dir\sub\file.txt";
        assert_eq!(run(path, 21., Mode::Path), "file.txt");
        assert_eq!(run(path, 6., Mode::Path), "….txt");
        for budget in 1..=21 {
            let out = run(path, budget as f32, Mode::Path);
            assert!(
                !out.starts_with(r"\\") && !out.contains("srv") && !out.contains('/'),
                "@{budget}: {out:?}"
            );
            assert!(mono(&out) <= budget as f32, "@{budget}: {out:?}");
        }
    }

    /// A POSIX path that starts with two slashes has the same anchor.
    #[test]
    fn a_double_slash_posix_anchor_is_treated_the_same() {
        let path = "//host/share/dir/sub/file.txt";
        assert_eq!(run(path, 27., Mode::Path), "//host/share/…/sub/file.txt");
        assert_eq!(run(path, 23., Mode::Path), "//host/share/…/file.txt");
        assert_eq!(run(path, 22., Mode::Path), "file.txt");
    }

    /// Whatever the root, the output never spells a separator the input did
    /// not use: a path written with backslashes does not come back with a `/`.
    #[test]
    fn separators_never_appear_that_the_input_lacks() {
        for text in [
            "/usr/local/share/man/man1/git.1",
            r"C:\Users\dev\AppData\Local\Temp\verify-build",
            r"\Windows\System32\drivers\etc\hosts",
            r"\\srv\share\dir\sub\file.txt",
            "//host/share/dir/sub/file.txt",
            "~/projects/toolbox/src/ui/tab_sidebar.rs",
            r"~\projects\toolbox\src",
        ] {
            for budget in 1..=(mono(text) as usize + 2) {
                let out = run(text, budget as f32, Mode::Path);
                for sep in ['/', '\\'] {
                    assert!(
                        !out.contains(sep) || text.contains(sep),
                        "{text:?} @{budget} -> {out:?}"
                    );
                }
                assert!(
                    out.matches('…').count() <= 1,
                    "{text:?} @{budget} -> {out:?}"
                );
                assert!(mono(&out) <= budget as f32, "{text:?} @{budget} -> {out:?}");
            }
        }
    }

    /// A rooted or UNC path that fits comes back exactly as it was written.
    #[test]
    fn unc_and_rooted_text_that_fits_is_left_verbatim() {
        for text in [
            r"\\srv\share\dir\file",
            "//host/share/x",
            r"\Windows\System32",
        ] {
            let out = elide(text, mono(text), Mode::Path, &mut mono);
            assert!(matches!(out, Cow::Borrowed(t) if t == text), "{out}");
        }
    }

    // ---- min_visible ----

    fn visible(text: &str, tail: Option<&str>, budget: f32, min: usize, mode: Mode) -> bool {
        let limits = Limits {
            budget,
            max_chars: None,
            min_visible: Some(min),
        };
        elide_to(text, tail, limits, mode, &mut mono).is_some()
    }

    /// Hidden exactly when the original keeps fewer than `min` clusters, read
    /// off the structural count at every budget, for End and for Middle.
    #[test]
    fn a_label_is_hidden_exactly_when_fewer_than_min_clusters_survive() {
        for text in [
            "feature-branch-name",
            "ветка-исправление-длинная",
            "文档设计源代码管理方案",
        ] {
            for mode in [Mode::End, Mode::Middle] {
                for budget in 1..=30 {
                    let out = outcome(text, budget as f32, mode, None);
                    let expect = out.whole || out.kept() >= 6;
                    assert_eq!(
                        visible(text, None, budget as f32, 6, mode),
                        expect,
                        "{mode:?} {text:?} @{budget}: kept {}",
                        out.kept()
                    );
                }
            }
        }
    }

    /// The boundary itself, with nothing but the budget deciding: five kept
    /// clusters hide a label that must keep six, six show it. This reads the
    /// visible result, not the count the policy keeps, so a count that included
    /// the inserted `…` would move it.
    #[test]
    fn six_kept_clusters_show_and_five_hide() {
        assert!(!visible("abcdefghijklmn", None, 6., 6, Mode::End));
        assert!(visible("abcdefghijklmn", None, 7., 6, Mode::End));
        assert!(!visible("abcdefghijklmn", None, 6., 6, Mode::Middle));
        assert!(visible("abcdefghijklmn", None, 7., 6, Mode::Middle));
    }

    /// A head loses its trailing space and a tail its leading space on the way
    /// to the output, and what counts is what is actually left. Here the cut
    /// takes eleven clusters but only the five of `abcde` are shown.
    #[test]
    fn dropped_whitespace_is_not_counted_as_visible() {
        let text = "abcde      fghijk";
        let out = outcome(text, 7., Mode::End, None);
        assert_eq!(out.text, "abcde…");
        assert_eq!(out.head, 11, "where the cut fell");
        assert_eq!(out.kept(), 5, "what survived");
        assert!(!visible(text, None, 7., 6, Mode::End));
        assert!(visible(text, None, 7., 5, Mode::End));

        let text = "abcdef      ghijk";
        let out = outcome(text, 7., Mode::Start, None);
        assert_eq!(out.kept(), 5, "{:?}", out.text);
        assert!(!visible(text, None, 7., 6, Mode::Start));
    }

    #[test]
    fn text_that_fits_whole_is_never_hidden() {
        assert!(visible("main", None, 10., 6, Mode::End));
        assert!(visible("main", Some(" · "), 10., 6, Mode::End));
        // Too narrow for even that: nothing of it fits, so it is hidden.
        assert!(!visible("main", None, 2., 6, Mode::End));
    }

    /// A cut snapped back to a boundary can leave fewer than `min` than the
    /// budget allowed; the snapped result is what counts.
    #[test]
    fn the_snapped_cut_is_what_is_counted() {
        assert!(!visible("ab-cdefghijklmn", None, 8., 6, Mode::End));
        assert!(visible("abcdefghijklmn", None, 8., 6, Mode::End));
    }

    /// The `…` the text came with is retained text.
    #[test]
    fn an_authored_ellipsis_counts_as_retained_text() {
        let limits = Limits {
            budget: 7.,
            max_chars: None,
            min_visible: Some(6),
        };
        let out = elide_to("ab…cdefghijkl", None, limits, Mode::End, &mut mono).unwrap();
        assert_eq!(out.head, "ab…cde…");
    }

    /// With a tail the whole label goes, separator included: no orphan dot.
    #[test]
    fn the_tail_is_hidden_with_the_label() {
        let tail = Some(" · ");
        assert!(!visible("feature-branch-name", tail, 9., 6, Mode::End));
        let limits = Limits {
            budget: 14.,
            max_chars: None,
            min_visible: Some(6),
        };
        let shown = elide_to("feature-branch-name", tail, limits, Mode::End, &mut mono).unwrap();
        assert_eq!(shown.tail, " · ");
        assert!(mono(&shown.joined()) <= 14.);
        let wide_enough = Limits {
            budget: 40.,
            ..limits
        };
        let whole = elide_to(
            "feature-branch-name",
            tail,
            wide_enough,
            Mode::End,
            &mut mono,
        );
        assert_eq!(whole.unwrap().joined(), "feature-branch-name · ");
    }

    #[test]
    fn min_visible_zero_never_hides() {
        assert!(visible("abcdefghij", None, 3., 0, Mode::End));
        assert!(visible("abcdefghij", Some(" · "), 5., 0, Mode::End));

        // No room for even an ellipsis beside the tail: the title is dropped
        // and the tail stands alone. Zero asks for no hiding, so it is shown,
        // where any real minimum hides the whole label.
        let limits = |min| Limits {
            budget: 7.,
            max_chars: None,
            min_visible: Some(min),
        };
        let tail = Some(" — gone");
        let shown = elide_to("abcdefghij", tail, limits(0), Mode::End, &mut mono);
        assert_eq!(shown.map(|s| s.joined()).as_deref(), Some("gone"));
        assert!(elide_to("abcdefghij", tail, limits(1), Mode::End, &mut mono).is_none());
    }
}
