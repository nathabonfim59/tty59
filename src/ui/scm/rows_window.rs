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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{AnyElement, App, Bounds, Pixels, ScrollHandle, Window, canvas, div, prelude::*, px};

use crate::ui::scm::state::ScmGroup;

/// The space between two rows of a group — the group block's `gap`.
pub(crate) const ROW_GAP: f32 = 1.;

/// Rows built past each edge of the viewport, so a frame whose run moved a
/// little since it was measured still has the viewport covered.
pub(crate) const OVERSCAN_ROWS: usize = 12;

/// Where each group's run started in the scroller's content, as the last
/// frame laid it out. Shared with the measuring canvas, which runs in
/// prepaint, after render has handed the elements over.
#[derive(Clone, Default)]
pub(crate) struct RunTops(Rc<RefCell<HashMap<ScmGroup, f32>>>);

impl RunTops {
    fn get(&self, group: ScmGroup) -> Option<f32> {
        self.0.borrow().get(&group).copied()
    }

    fn set(&self, group: ScmGroup, top: f32) {
        self.0.borrow_mut().insert(group, top);
    }
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
    /// A row built wherever the viewport is: the one an open context menu
    /// hangs off. The menu lives in that row's element state, and a row that
    /// stops being built takes its menu down with it.
    pin: Option<usize>,
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
            pin: None,
            tops: tops.clone(),
            scroll: scroll.clone(),
            view,
        }
    }

    /// Also build row `pin`, wherever the viewport is.
    pub(crate) fn pin(mut self, pin: Option<usize>) -> Self {
        self.pin = pin.filter(|&i| i < self.len);
        self
    }

    /// The runs of consecutive rows that are built, in order.
    fn segments(&self) -> Vec<Range<usize>> {
        let range = self.range.clone();
        match self.pin {
            Some(p) if !range.contains(&p) => {
                let pin = p..p + 1;
                let mut out: Vec<Range<usize>> =
                    [range, pin].into_iter().filter(|r| !r.is_empty()).collect();
                out.sort_by_key(|r| r.start);
                // A pin right against the range joins it.
                if out.len() == 2 && out[0].end == out[1].start {
                    out[0].end = out[1].end;
                    out.truncate(1);
                }
                out
            }
            _ if range.is_empty() => Vec::new(),
            _ => vec![range],
        }
    }

    /// Whether row `i` of the run has to be built.
    pub(crate) fn builds(&self, i: usize) -> bool {
        self.range.contains(&i) || self.pin == Some(i)
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
                tops.set(group, top);
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
            pin: None,
            tops: RunTops::default(),
            scroll: ScrollHandle::new(),
            view: gpui::EntityId::from(1u64),
        }
        .pin(pin)
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
