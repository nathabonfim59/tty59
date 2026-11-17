//! Building only the rows of a long run that the Changes scroller can show.
//!
//! An expanded group can hold `MAX_RENDERED_FILES` rows, and the panel used
//! to build, lay out and prepaint every one of them on every frame — ~620 in
//! a large merge, a couple of hundred milliseconds a frame, which is long
//! enough for keystrokes into the terminal beside it to queue for seconds.
//!
//! Every row of a run stands on one pitch: a file row and a tree directory
//! row are both `ROW_H` rems tall, one 1px gap apart. So a run can be drawn as
//! the rows the viewport reaches with a spacer above and below them that is
//! exactly as tall as the rows it stands in for. The content keeps its
//! height (and so the scroll offset, its clamp and the scrollbar keep
//! theirs) and every built row lands where it always did.
//!
//! The scroller is the panel's own `overflow_y_scroll` div, not a
//! `uniform_list`: a wheel or scrollbar move notifies the view, so render sees
//! the new offset before the frame lays out, and the pinned block, the
//! commit detail and the history footer around the list stay as they were.
//! What render cannot know is where each run starts inside the content; the
//! last frame's layout says, recorded by a zero-size canvas in the run. If a
//! frame finds the rows it built do not cover the viewport — the run moved
//! because something above it changed, or the offset was clamped — it asks
//! for one more frame. Otherwise it asks for nothing, so a parked panel stays
//! idle.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Pixels, ScrollHandle, Window, canvas, div, point, prelude::*, px,
};

use crate::ui::scm::state::ScmGroup;

/// The space between two rows of a group — the group block's `gap`.
pub(crate) const ROW_GAP: f32 = 1.;

/// Rows built past each edge of the viewport, so a frame whose run moved a
/// little since it was measured still has the viewport covered.
pub(crate) const OVERSCAN_ROWS: usize = 12;

/// Where each group's run started in the scroller's content, as the last
/// frame laid it out. Shared with the measuring canvas, which runs in
/// prepaint, after render has handed the elements over.
///
/// Every measurement also carries the token of the frame that wrote it
/// (`begin` hands one out per render of the list). A measurement is trusted for
/// a reveal only if it was written by the latest frame that measured anything:
/// a group that was collapsed, filtered out, or left behind by another
/// repository has no canvas in that frame, keeps an older token, and reads as
/// unmeasured instead of as wherever it last was.
#[derive(Clone, Default)]
pub(crate) struct RunTops(Rc<Tops>);

#[derive(Default)]
struct Tops {
    runs: RefCell<HashMap<ScmGroup, f32>>,
    /// The same measurements with the token of the frame that took them.
    stamped_runs: RefCell<HashMap<ScmGroup, (u64, f32)>>,
    stamped_heads: RefCell<HashMap<ScmGroup, (u64, f32)>>,
    /// The latest token any canvas wrote under.
    painted: Cell<u64>,
    next: Cell<u64>,
}

/// What a reveal found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reveal {
    /// Already fully in view.
    Visible,
    /// The scroll offset was set; the caller asks for the frame.
    Scrolled,
    /// No trustworthy measurement of where the target is.
    Unmeasured,
}

impl RunTops {
    fn get(&self, group: ScmGroup) -> Option<f32> {
        self.0.runs.borrow().get(&group).copied()
    }

    fn set(&self, group: ScmGroup, top: f32) {
        self.0.runs.borrow_mut().insert(group, top);
    }

    /// The token the next frame's measurements are written under. Called once
    /// at the top of the list's render.
    pub(crate) fn begin(&self) -> u64 {
        let token = self.0.next.get() + 1;
        self.0.next.set(token);
        token
    }

    /// The token of the frame being rendered, for the elements that will
    /// measure in prepaint.
    fn token(&self) -> u64 {
        self.0.next.get()
    }

    fn stamp(&self, token: u64) {
        if token > self.0.painted.get() {
            self.0.painted.set(token);
        }
    }

    fn set_run(&self, token: u64, group: ScmGroup, top: f32) {
        self.set(group, top);
        self.0.stamped_runs.borrow_mut().insert(group, (token, top));
        self.stamp(token);
    }

    fn set_head(&self, token: u64, group: ScmGroup, top: f32) {
        self.0
            .stamped_heads
            .borrow_mut()
            .insert(group, (token, top));
        self.stamp(token);
    }

    fn fresh(&self, entry: Option<(u64, f32)>) -> Option<f32> {
        entry
            .filter(|(token, _)| *token == self.0.painted.get())
            .map(|(_, top)| top)
    }

    /// Scroll row `index` of `group`'s run fully into view, from where the last
    /// frame measured the run, the row pitch and the scroller's own bounds.
    /// Never `ScrollHandle::scroll_to_item`: that indexes the scroller's direct
    /// children, and a logical row is not one.
    pub(crate) fn reveal_row(
        &self,
        group: ScmGroup,
        index: usize,
        row_h: f32,
        scroll: &ScrollHandle,
    ) -> Reveal {
        let run = self
            .fresh(self.0.stamped_runs.borrow().get(&group).copied())
            .map(|top| top + index as f32 * (row_h + ROW_GAP));
        reveal_at(run, row_h, scroll)
    }

    /// Scroll `group`'s header, `head_h` tall, fully into view.
    pub(crate) fn reveal_head(
        &self,
        group: ScmGroup,
        head_h: f32,
        scroll: &ScrollHandle,
    ) -> Reveal {
        let top = self.fresh(self.0.stamped_heads.borrow().get(&group).copied());
        reveal_at(top, head_h, scroll)
    }
}

fn reveal_at(top: Option<f32>, h: f32, scroll: &ScrollHandle) -> Reveal {
    let Some(top) = top else {
        return Reveal::Unmeasured;
    };
    let offset = scroll.offset();
    let view_h = scroll.bounds().size.height.as_f32();
    let max = scroll.max_offset().y.as_f32();
    match reveal_offset(top, h, -offset.y.as_f32(), view_h, max) {
        Some(to) => {
            scroll.set_offset(point(offset.x, px(-to)));
            Reveal::Scrolled
        }
        None => Reveal::Visible,
    }
}

/// The scroll offset (distance scrolled down, `0..=max_scroll`) that brings the
/// span `top..top + h` fully into the view `scroll_top..scroll_top + view_h`
/// with the least movement, or `None` if it is already in view. A span taller
/// than the view lines up with its top edge.
pub(crate) fn reveal_offset(
    top: f32,
    h: f32,
    scroll_top: f32,
    view_h: f32,
    max_scroll: f32,
) -> Option<f32> {
    if view_h <= 0. {
        return None;
    }
    let want = if h >= view_h || top < scroll_top {
        top
    } else if top + h > scroll_top + view_h {
        top + h - view_h
    } else {
        return None;
    };
    let want = want.clamp(0., max_scroll.max(0.));
    ((want - scroll_top).abs() > 0.01).then_some(want)
}

/// Scroll a row that is mounted, at `row` in window space, fully into the
/// scroller's view, from its own painted bounds. Whether it moved the scroll
/// offset.
pub(crate) fn reveal_painted(scroll: &ScrollHandle, row: Bounds<Pixels>) -> bool {
    let view = scroll.bounds();
    let offset = scroll.offset();
    let top = row.top().as_f32() - view.top().as_f32() - offset.y.as_f32();
    match reveal_offset(
        top,
        row.size.height.as_f32(),
        -offset.y.as_f32(),
        view.size.height.as_f32(),
        scroll.max_offset().y.as_f32(),
    ) {
        Some(to) => {
            scroll.set_offset(point(offset.x, px(-to)));
            true
        }
        None => false,
    }
}

/// The header of `group`, measured: a zero-size canvas whose prepaint records
/// where the header sits in the scroller's content under this frame's token.
/// Only headers that are built have one, so a hidden group can never overwrite
/// a measurement.
pub(crate) fn head_probe(group: ScmGroup, tops: &RunTops, scroll: &ScrollHandle) -> AnyElement {
    let (tops, scroll, token) = (tops.clone(), scroll.clone(), tops.token());
    canvas(
        move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
            let top = bounds.origin.y.as_f32()
                - scroll.bounds().origin.y.as_f32()
                - scroll.offset().y.as_f32();
            tops.set_head(token, group, top);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_0()
    .into_any_element()
}

/// The rows of a run of `len` that reach `[scroll_top, scroll_top + view_h)`,
/// widened by `overscan` rows at each end. `top` is the run's first row in
/// content space; `None` (never measured) builds from the run's start.
pub(crate) fn visible_range(
    len: usize,
    pitch: f32,
    top: Option<f32>,
    scroll_top: f32,
    view_h: f32,
    overscan: usize,
) -> Range<usize> {
    if len == 0 || pitch <= 0. {
        return 0..0;
    }
    let Some(top) = top else {
        let fits = (view_h / pitch).ceil().max(0.) as usize;
        return 0..len.min(fits + 1 + overscan);
    };
    let from = ((scroll_top - top) / pitch).floor();
    let to = ((scroll_top + view_h - top) / pitch).ceil();
    let clamp = |v: f32| v.clamp(0., len as f32) as usize;
    let start = clamp(from).saturating_sub(overscan);
    let end = (clamp(to) + overscan).min(len);
    if start >= end {
        start..start
    } else {
        start..end
    }
}

/// The height of `n` consecutive rows and the gaps between them.
pub(crate) fn span(n: usize, row: f32) -> f32 {
    if n == 0 {
        0.
    } else {
        n as f32 * row + (n - 1) as f32 * ROW_GAP
    }
}

#[cfg(test)]
thread_local! {
    /// Frames a run asked for because the rows it built missed the viewport.
    pub(crate) static REDRAWS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One group's run of rows, windowed to the viewport.
pub(crate) struct RowsWindow {
    group: ScmGroup,
    len: usize,
    row: f32,
    range: Range<usize>,
    /// Rows built wherever the viewport is: at most the one an open context
    /// menu hangs off (the menu lives in that row's element state, and a row
    /// that stops being built takes its menu down with it) and the one the
    /// keyboard is on (a focused row that stops being built loses the focus).
    pins: [Option<usize>; 2],
    /// The token the run's measurement is written under; see `RunTops::begin`.
    token: u64,
    tops: RunTops,
    scroll: ScrollHandle,
    view: gpui::EntityId,
}

impl RowsWindow {
    /// `row_h` is a row's height in px at the rem in force this frame.
    pub(crate) fn new(
        group: ScmGroup,
        len: usize,
        row_h: f32,
        tops: &RunTops,
        scroll: &ScrollHandle,
        view: gpui::EntityId,
        window: &Window,
    ) -> Self {
        let bounds = scroll.bounds();
        let view_h = match bounds.size.height.as_f32() {
            h if h > 0. => h,
            // Never laid out yet: the window is as tall as it can be.
            _ => window.viewport_size().height.as_f32(),
        };
        let range = visible_range(
            len,
            row_h + ROW_GAP,
            tops.get(group),
            -scroll.offset().y.as_f32(),
            view_h,
            OVERSCAN_ROWS,
        );
        Self {
            group,
            len,
            row: row_h,
            range,
            pins: [None; 2],
            token: tops.token(),
            tops: tops.clone(),
            scroll: scroll.clone(),
            view,
        }
    }

    /// Also build row `pin`, wherever the viewport is. Two rows can be pinned
    /// (the menu's and the keyboard's, in either order); a third is ignored.
    pub(crate) fn pin(mut self, pin: Option<usize>) -> Self {
        let Some(pin) = pin.filter(|&i| i < self.len) else {
            return self;
        };
        if self.pins.contains(&Some(pin)) {
            return self;
        }
        if let Some(slot) = self.pins.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(pin);
        }
        self
    }

    /// The runs of consecutive rows that are built, in order: the range and
    /// the pins outside it, with any that touch joined.
    fn segments(&self) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::with_capacity(3);
        if !self.range.is_empty() {
            out.push(self.range.clone());
        }
        for &p in self.pins.iter().flatten() {
            if !self.range.contains(&p) {
                out.push(p..p + 1);
            }
        }
        out.sort_by_key(|r| r.start);
        let mut joined: Vec<Range<usize>> = Vec::with_capacity(out.len());
        for r in out {
            match joined.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => joined.push(r),
            }
        }
        joined
    }

    /// Whether row `i` of the run has to be built.
    pub(crate) fn builds(&self, i: usize) -> bool {
        self.range.contains(&i) || self.pins.contains(&Some(i))
    }

    /// The rows the caller has to build, by index into the run, in order.
    pub(crate) fn indices(&self) -> std::iter::Flatten<std::vec::IntoIter<Range<usize>>> {
        self.segments().into_iter().flatten()
    }

    /// The run's element: the built rows (`indices`, in order) between
    /// spacers, laid out as a column with the group's gap — the same column
    /// the rows stood in when every one of them was built.
    pub(crate) fn finish(self, rows: Vec<AnyElement>) -> AnyElement {
        let segments = self.segments();
        debug_assert_eq!(rows.len(), segments.iter().map(|r| r.len()).sum::<usize>());
        let Self {
            group,
            len,
            row,
            range,
            token,
            tops,
            scroll,
            view,
            ..
        } = self;
        let built = range;
        let measure = canvas(
            move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                let port = scroll.bounds();
                let offset = scroll.offset().y.as_f32();
                let top = bounds.origin.y.as_f32() - port.origin.y.as_f32() - offset;
                tops.set_run(token, group, top);
                let need = visible_range(
                    len,
                    row + ROW_GAP,
                    Some(top),
                    -offset,
                    port.size.height.as_f32(),
                    0,
                );
                let covered =
                    need.is_empty() || (built.start <= need.start && need.end <= built.end);
                if !covered {
                    #[cfg(test)]
                    REDRAWS.with(|n| n.set(n.get() + 1));
                    // Not `window.refresh()`: that is a no-op mid-draw.
                    cx.defer(move |cx| cx.notify(view));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_0();
        // Any mix of spacers and rows in one gapped column stands exactly as
        // tall as the rows it stands for: see `span`.
        let spacer = |n: usize| div().flex_none().h(px(span(n, row))).into_any_element();
        let mut children = Vec::with_capacity(rows.len() + segments.len() + 1);
        let mut rows = rows.into_iter();
        let mut at = 0;
        for seg in segments {
            if seg.start > at {
                children.push(spacer(seg.start - at));
            }
            children.extend(rows.by_ref().take(seg.len()));
            at = seg.end;
        }
        if at < len {
            children.push(spacer(len - at));
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .child(measure)
            .children(children)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_never_measured_builds_from_its_start() {
        assert_eq!(visible_range(600, 27., None, 5000., 540., 4), 0..25);
        assert_eq!(visible_range(10, 27., None, 0., 540., 4), 0..10);
    }

    #[test]
    fn the_range_covers_the_viewport_plus_overscan() {
        // Run starts 100px into the content; viewport is rows 10..30 of it.
        let r = visible_range(600, 27., Some(100.), 100. + 270., 540., 0);
        assert_eq!(r, 10..30);
        let r = visible_range(600, 27., Some(100.), 100. + 270., 540., 5);
        assert_eq!(r, 5..35);
        // A partial row at either edge is built.
        let r = visible_range(600, 27., Some(0.), 13., 540., 0);
        assert_eq!(r, 0..21);
    }

    #[test]
    fn a_run_off_either_end_builds_nothing_or_its_edge() {
        // Entirely below the viewport.
        assert!(visible_range(600, 27., Some(5000.), 0., 540., 0).is_empty());
        // Entirely above it.
        assert!(visible_range(600, 27., Some(0.), 50_000., 540., 0).is_empty());
        // Above it, with overscan: the last rows, never past the end.
        let r = visible_range(600, 27., Some(0.), 50_000., 540., 3);
        assert_eq!(r, 597..600);
        // Below it, with overscan: the first rows.
        let r = visible_range(600, 27., Some(1000.), 0., 540., 3);
        assert_eq!(r, 0..3);
    }

    fn run(len: usize, range: Range<usize>, pin: Option<usize>) -> RowsWindow {
        RowsWindow {
            group: ScmGroup::Changes,
            len,
            row: 26.,
            range,
            pins: [None; 2],
            token: 0,
            tops: RunTops::default(),
            scroll: ScrollHandle::new(),
            view: gpui::EntityId::from(1u64),
        }
        .pin(pin)
    }

    // The tests below are post-implementation controls for the two-pin and
    // reveal code: they were written after it, not before.

    #[test]
    fn two_pins_are_both_built_and_a_third_is_ignored() {
        let two = |a, b| run(300, 10..40, Some(a)).pin(Some(b));
        assert_eq!(two(250, 3).segments(), vec![3..4, 10..40, 250..251]);
        assert_eq!(two(40, 9).segments(), vec![9..41]);
        assert_eq!(two(41, 9).segments(), vec![9..40, 41..42]);
        assert_eq!(two(250, 251).segments(), vec![10..40, 250..252]);
        // The same row pinned twice is one pin, and so leaves a slot free.
        assert_eq!(
            two(250, 250).pin(Some(5)).segments(),
            vec![5..6, 10..40, 250..251]
        );
        // A third distinct pin has no slot.
        assert_eq!(
            two(250, 3).pin(Some(100)).segments(),
            vec![3..4, 10..40, 250..251]
        );
        let r = two(250, 3);
        assert!(r.builds(250) && r.builds(3) && r.builds(10) && !r.builds(100));
        assert_eq!(
            r.indices().collect::<Vec<_>>().len(),
            1 + 30 + 1,
            "viewport range plus the two pins and nothing else"
        );
    }

    #[test]
    fn a_reveal_scrolls_the_least_that_puts_the_span_in_view() {
        // View is 200px tall and scrolled 100px down; content is 1000px.
        let at = |top, h| reveal_offset(top, h, 100., 200., 800.);
        assert_eq!(at(150., 26.), None, "already in view");
        assert_eq!(at(100., 26.), None, "flush with the top edge");
        assert_eq!(at(274., 26.), None, "flush with the bottom edge");
        assert_eq!(at(40., 26.), Some(40.), "above: its top lines up");
        assert_eq!(at(400., 26.), Some(226.), "below: its bottom lines up");
        assert_eq!(at(275., 26.), Some(101.), "one pixel past the bottom");
        assert_eq!(
            at(500., 300.),
            Some(500.),
            "taller than the view: top lines up"
        );
        assert_eq!(
            reveal_offset(990., 26., 100., 200., 800.),
            Some(800.),
            "clamped to the end of the content"
        );
        assert_eq!(reveal_offset(0., 26., 0., 200., 800.), None);
        assert_eq!(
            reveal_offset(5., 26., 0., 0., 800.),
            None,
            "no view, no move"
        );
    }

    #[test]
    fn a_measurement_is_trusted_only_from_the_latest_painted_frame() {
        let tops = RunTops::default();
        let first = tops.begin();
        tops.set_run(first, ScmGroup::Staged, 100.);
        tops.set_run(first, ScmGroup::Changes, 900.);
        assert_eq!(
            tops.fresh(tops.0.stamped_runs.borrow().get(&ScmGroup::Staged).copied()),
            Some(100.)
        );
        // The next frame measures Changes only: Staged was collapsed away.
        let second = tops.begin();
        tops.set_run(second, ScmGroup::Changes, 300.);
        assert_eq!(
            tops.fresh(tops.0.stamped_runs.borrow().get(&ScmGroup::Staged).copied()),
            None,
            "a group the latest frame did not measure reads as unmeasured"
        );
        assert_eq!(
            tops.fresh(
                tops.0
                    .stamped_runs
                    .borrow()
                    .get(&ScmGroup::Changes)
                    .copied()
            ),
            Some(300.)
        );
        // Unmeasured is an answer a reveal can act on: it does nothing.
        let scroll = ScrollHandle::new();
        assert_eq!(
            tops.reveal_row(ScmGroup::Staged, 3, 26., &scroll),
            Reveal::Unmeasured
        );
        assert_eq!(
            tops.reveal_head(ScmGroup::Staged, 22., &scroll),
            Reveal::Unmeasured
        );
    }

    #[test]
    fn a_pinned_row_is_built_wherever_the_viewport_is() {
        let segs = |r: RowsWindow| r.segments();
        assert_eq!(segs(run(300, 10..40, None)), vec![10..40]);
        assert_eq!(segs(run(300, 10..40, Some(20))), vec![10..40]);
        assert_eq!(segs(run(300, 10..40, Some(250))), vec![10..40, 250..251]);
        assert_eq!(segs(run(300, 10..40, Some(3))), vec![3..4, 10..40]);
        assert_eq!(segs(run(300, 10..40, Some(40))), vec![10..41]);
        assert_eq!(segs(run(300, 10..40, Some(9))), vec![9..40]);
        assert_eq!(segs(run(300, 40..40, Some(7))), vec![7..8]);
        assert!(segs(run(300, 40..40, None)).is_empty());
        // A pin past the end of the run (the row went away) is no pin.
        assert_eq!(segs(run(300, 10..40, Some(300))), vec![10..40]);
        let r = run(300, 10..12, Some(250));
        assert_eq!(r.indices().collect::<Vec<_>>(), vec![10, 11, 250]);
        assert!(r.builds(250) && r.builds(10) && !r.builds(12));
    }

    #[test]
    fn spacers_stand_in_for_exactly_the_rows_they_replace() {
        let row = 26.;
        // a rows, a gap, b rows == a + b rows, for every split of a run.
        for len in 1..40 {
            for a in 0..=len {
                let joined = span(a, row) + if a > 0 && a < len { ROW_GAP } else { 0. };
                assert_eq!(joined + span(len - a, row), span(len, row), "{a}/{len}");
            }
        }
    }
}
