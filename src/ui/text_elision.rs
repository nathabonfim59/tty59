//! One measured elision for the whole UI.
//!
//! [`policy`] decides what to keep; this file is the GPUI side of it. A
//! label that has to give up part of its text is drawn by [`ElidedLabel`],
//! which elides against the width layout actually hands it, so no caller
//! reproduces padding or chrome arithmetic to guess a budget.
//!
//! ## Where the work happens
//!
//! Layout is the only place a width is known, and GPUI already keeps a
//! per-element state across frames for work that depends on it. The label
//! memoizes in that state, in two fixed slots, and nowhere else:
//!
//! - the *natural* width of the whole text, keyed by [`Face`] (text, tail kind,
//!   font, size). The measure callback answers from it, so taffy probing a flex
//!   item at several widths never reshapes anything;
//! - the *shown* output — the elided string and its prepared line — keyed by
//!   the same plus the mode and the final width. It is computed in `prepaint`,
//!   once layout has settled, because a measure callback is asked about
//!   several widths per frame and a one-entry memo keyed on those would be
//!   rebuilt by every frame.
//!
//! Each slot is replaced when its key changes, so nothing grows, and GPUI drops
//! the state with the element when it stops being drawn. Layout and paint read
//! only these keys, the ambient text style and the window's text system; no app
//! or model state is read or written. An unchanged frame compares keys and
//! paints the retained line: it shapes nothing and allocates no strings.
//!
//! Every shaping call, whether to measure a candidate or to prepare the line,
//! goes through [`shape`], which is where the tests count.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, Element, ElementId, Font, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, ShapedLine, SharedString, Size, Style,
    TextAlign, TextRun, Window, WindowTextSystem, px,
};

mod policy;

use policy::{Limits, elide_to};
pub(crate) use policy::{Mode, join_segments, path_separator};

#[cfg(test)]
#[derive(Default)]
struct Probe {
    /// Calls to [`shape`] on this thread.
    shapes: usize,
    /// Times the shown output had to be recomputed.
    elisions: usize,
}

/// What the labels drawn since the last [`observe::clear`] prepared and
/// painted, keyed by their element id, so a test that renders a whole screen
/// can ask about one label of it. Written from the prepared line itself at every
/// prepaint, cache hits included, and from the one place a line is painted.
#[cfg(test)]
pub(crate) mod observe {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gpui::{ElementId, SharedString};

    /// A label that was drawn. `label(id)` is `None` for one that was not
    /// drawn at all since the last `clear`, which is a different thing from a
    /// label that was drawn and is hidden: that one is `Some` with an empty
    /// `text` and `painted == false`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct Label {
        /// The text of the prepared line, empty when no line was prepared: the
        /// label is hidden, or has no room for even an ellipsis.
        pub(crate) text: SharedString,
        /// Whether `line.paint` ran for it this frame.
        pub(crate) painted: bool,
    }

    thread_local! {
        static LABELS: RefCell<HashMap<ElementId, Label>> = RefCell::new(HashMap::new());
    }

    /// Forgets every label, so what `label` answers afterwards is only what
    /// the next frame draws. Call it before the frame being inspected.
    pub(crate) fn clear() {
        LABELS.with(|labels| labels.borrow_mut().clear());
    }

    pub(crate) fn label(id: &ElementId) -> Option<Label> {
        LABELS.with(|labels| labels.borrow().get(id).cloned())
    }

    pub(super) fn prepared(id: &ElementId, text: Option<SharedString>) {
        let text = text.unwrap_or_else(|| SharedString::from(""));
        LABELS.with(|labels| {
            labels.borrow_mut().insert(
                id.clone(),
                Label {
                    text,
                    painted: false,
                },
            );
        });
    }

    pub(super) fn painted(id: &ElementId) {
        LABELS.with(|labels| {
            if let Some(label) = labels.borrow_mut().get_mut(id) {
                label.painted = true;
            }
        });
    }
}

#[cfg(test)]
thread_local! {
    static PROBE: RefCell<Probe> = RefCell::new(Probe::default());
}

#[cfg(test)]
fn probe(f: impl FnOnce(&mut Probe)) {
    PROBE.with(|p| f(&mut p.borrow_mut()));
}

/// The one place text is shaped for measuring or drawing a label.
fn shape(
    text_system: &WindowTextSystem,
    text: SharedString,
    size: Pixels,
    run: TextRun,
) -> ShapedLine {
    #[cfg(test)]
    probe(|p| p.shapes += 1);
    text_system.shape_line(text, size, &[run], None)
}

/// Width of `text` shaped in `font` at `size`, in pixels.
///
/// The window's text system caches shaped runs, so measuring the same labels
/// across frames is cheap. The sidebar elides against real glyph widths
/// instead of guessing at character counts — that is the only way a mixed
/// CJK/Latin label can be squeezed without tearing mid-token.
pub(crate) fn measure_text(
    text_system: &WindowTextSystem,
    font: &Font,
    size: f32,
    text: &str,
) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    shape(text_system, SharedString::from(text), px(size), run)
        .width
        .as_f32()
}

/// Words that ride behind a label's text and are never cut.
#[derive(Clone, PartialEq)]
enum Tail {
    None,
    /// The label's text is the head alone and these words follow it.
    Append(SharedString),
    /// The label's text is the whole string and already ends with these words.
    Included(&'static str),
}

/// Everything that decides how wide a label's text is, short of its mode and
/// the width it is given. `Font` carries the family, weight, style, features
/// and fallbacks, so a change to any of them is a change of key.
#[derive(Clone, PartialEq)]
struct Face {
    text: SharedString,
    tail: Tail,
    font: Font,
    size: Pixels,
}

impl Face {
    /// The whole string the label spells, measured once for its natural width.
    fn whole(&self) -> SharedString {
        match &self.tail {
            Tail::Append(tail) => SharedString::from(format!("{}{tail}", self.text)),
            Tail::None | Tail::Included(_) => self.text.clone(),
        }
    }

    /// The head to elide and the tail that stays whole, borrowed from the text
    /// and only where an elision actually runs (a memo miss). A text that does
    /// not end with its declared tail has no tail to keep: it elides as the
    /// plain string it is, and the tail is not spelled twice.
    fn parts(&self) -> (&str, Option<&str>) {
        match &self.tail {
            Tail::None => (&*self.text, None),
            Tail::Append(tail) => (&*self.text, Some(&**tail)),
            Tail::Included(tail) => match self.text.strip_suffix(*tail) {
                Some(head) => (head, Some(*tail)),
                None => (&*self.text, None),
            },
        }
    }
}

/// The natural slot: how wide the label is with nothing but its scalar cap
/// applied. The layout box is this wide, so a label capped at ten characters
/// does not reserve the width of the whole name.
struct Natural {
    face: Face,
    mode: Mode,
    max_chars: Option<usize>,
    width: f32,
}

/// The shown slot: the output for one [`Face`], mode, width and pair of limits.
struct Shown {
    face: Face,
    mode: Mode,
    width: Pixels,
    max_chars: Option<usize>,
    min_visible: Option<usize>,
    /// Empty when the label is hidden or has no room for anything.
    text: SharedString,
    /// What the line was shaped with. Colour, underline and the like are
    /// painted from the line, so a change to any of them means a new line but
    /// not a new elision.
    run: TextRun,
    /// `None` when there is nothing to paint: no line is shaped for it.
    line: Option<ShapedLine>,
}

#[derive(Default)]
struct Memo {
    natural: Option<Natural>,
    shown: Option<Shown>,
}

fn natural_width(
    memo: &RefCell<Memo>,
    face: &Face,
    mode: Mode,
    max_chars: Option<usize>,
    window: &Window,
) -> f32 {
    if let Some(natural) = &memo.borrow().natural
        && natural.face == *face
        && natural.mode == mode
        && natural.max_chars == max_chars
    {
        return natural.width;
    }
    let text_system = window.text_system();
    let size = face.size.as_f32();
    let mut measure = |s: &str| measure_text(text_system, &face.font, size, s);
    let width = match max_chars {
        None => measure(&*face.whole()),
        // The one policy pass the cap needs, with no pixel limit: the cap
        // alone decides what is left, and only that is shaped as a whole.
        Some(_) => {
            let limits = Limits {
                budget: f32::INFINITY,
                max_chars,
                min_visible: None,
            };
            let (head, tail) = face.parts();
            match elide_to(head, tail, limits, mode, &mut measure) {
                Some(capped) => measure(capped.joined().as_str()),
                None => 0.,
            }
        }
    };
    memo.borrow_mut().natural = Some(Natural {
        face: face.clone(),
        mode,
        max_chars,
        width,
    });
    width
}

/// A single line of text that gives up part of itself, by `mode`, when the
/// width it is laid out in is too small.
///
/// It reads the ambient text style like any text element: set the font, size,
/// weight and colour on a parent. Give it a width through ordinary layout
/// (`flex_1` with `min_w_0`, or a fixed width). The caller keeps the original
/// text for a tooltip or copy; this element only paints the cut one.
///
/// `id` must be unique and stable per label: it is the key of the memo.
pub(crate) struct ElidedLabel {
    id: ElementId,
    text: SharedString,
    tail: Tail,
    mode: Mode,
    max_chars: Option<usize>,
    min_visible: Option<usize>,
}

impl ElidedLabel {
    pub(crate) fn new(id: impl Into<ElementId>, text: impl Into<SharedString>, mode: Mode) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            tail: Tail::None,
            mode,
            max_chars: None,
            min_visible: None,
        }
    }

    /// The most Unicode scalar values the shown text may have, its `…`
    /// included and the tail excluded. It holds together with the width the
    /// label is given, in the one pass that cuts from the full text.
    pub(crate) fn max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = Some(max_chars);
        self
    }

    /// Hides the label, tail included, when fewer than this many clusters of
    /// the text survive. This only stops painting; the box layout gave the
    /// label stays where it is. Text that fits whole is never hidden.
    pub(crate) fn min_visible(mut self, min_visible: usize) -> Self {
        self.min_visible = Some(min_visible);
        self
    }

    /// Words that follow the text and always stay whole, already localized by
    /// the caller from the state they describe: ` — disconnected`.
    pub(crate) fn tail(mut self, tail: impl Into<SharedString>) -> Self {
        self.tail = Tail::Append(tail.into());
        self
    }

    /// The text given to [`ElidedLabel::new`] is the FULL string and already ends
    /// with this tail: the label elides the head and keeps the tail whole, as
    /// [`ElidedLabel::tail`] does for one appended. A text that does not end
    /// with it is elided as it stands. The last of the two calls wins.
    pub(crate) fn tail_included(mut self, tail: &'static str) -> Self {
        self.tail = Tail::Included(tail);
        self
    }
}

impl IntoElement for ElidedLabel {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct LabelLayout {
    memo: Rc<RefCell<Memo>>,
    face: Face,
    line_height: Pixels,
    align: TextAlign,
}

impl Element for ElidedLabel {
    type RequestLayoutState = LabelLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<gpui::accesskit::Role> {
        Some(gpui::accesskit::Role::Label)
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        // Assistive tech reads the whole text, not the cut the sighted see; the
        // words appended behind it are their own property rather than a joined
        // copy, and an included tail is already inside the text.
        node.set_value(self.text.to_string());
        match &self.tail {
            Tail::Append(tail) => node.set_description(tail.to_string()),
            Tail::None | Tail::Included(_) => {}
        }
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, LabelLayout) {
        // The helper's state is `Option<Option<S>>`: no id given, then nothing
        // stored yet. Either way a memo starts empty and fills on the first
        // miss; `id()` is always `Some`, so it is found again by its id.
        let memo = window.with_optional_element_state(id, |state, _| {
            let memo: Rc<RefCell<Memo>> = state.flatten().unwrap_or_default();
            (memo.clone(), Some(memo))
        });
        let rem = window.rem_size();
        let text_style = window.text_style();
        let face = Face {
            text: self.text.clone(),
            tail: self.tail.clone(),
            font: text_style.font(),
            size: text_style.font_size.to_pixels(rem),
        };
        let line_height = text_style.line_height_in_pixels(rem);
        // A label has to be able to shrink below its text; without this a flex
        // parent would refuse to squeeze it and the text would overflow instead.
        let mut style = Style::default();
        style.min_size.width = px(0.).into();
        let layout_id = window.request_measured_layout(style, {
            let memo = memo.clone();
            let face = face.clone();
            let (mode, max_chars) = (self.mode, self.max_chars);
            move |known, available, window, _cx| {
                let natural = px(natural_width(&memo, &face, mode, max_chars, window));
                let width = match (known.width, available.width) {
                    (Some(known), _) => known,
                    (None, AvailableSpace::Definite(room)) => natural.min(room),
                    (None, _) => natural,
                };
                Size {
                    width,
                    height: line_height,
                }
            }
        });
        (
            layout_id,
            LabelLayout {
                memo,
                face,
                line_height,
                align: text_style.text_align,
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut LabelLayout,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let width = bounds.size.width;
        let text_style = window.text_style();
        let text_system = window.text_system();
        let mut memo = layout.memo.borrow_mut();
        let current = memo.shown.as_ref().is_some_and(|shown| {
            shown.face == layout.face
                && shown.mode == self.mode
                && shown.width == width
                && shown.max_chars == self.max_chars
                && shown.min_visible == self.min_visible
        });
        if !current {
            let size = layout.face.size.as_f32();
            let mut measure = |s: &str| measure_text(text_system, &layout.face.font, size, s);
            let limits = Limits {
                budget: width.as_f32(),
                max_chars: self.max_chars,
                min_visible: self.min_visible,
            };
            let (head, tail) = layout.face.parts();
            let elided = elide_to(head, tail, limits, self.mode, &mut measure);
            let text = elided.map_or_else(
                || SharedString::from(""),
                |elided| SharedString::from(elided.joined()),
            );
            let run = text_style.to_run(text.len());
            let line = (!text.is_empty())
                .then(|| shape(text_system, text.clone(), layout.face.size, run.clone()));
            #[cfg(test)]
            probe(|p| p.elisions += 1);
            memo.shown = Some(Shown {
                face: layout.face.clone(),
                mode: self.mode,
                width,
                max_chars: self.max_chars,
                min_visible: self.min_visible,
                text,
                run,
                line,
            });
        } else if let Some(shown) = memo.shown.as_mut()
            && shown.line.is_some()
        {
            let run = text_style.to_run(shown.text.len());
            if shown.run != run {
                shown.line = Some(shape(
                    text_system,
                    shown.text.clone(),
                    layout.face.size,
                    run.clone(),
                ));
                shown.run = run;
            }
        }
        #[cfg(test)]
        observe::prepared(
            &self.id,
            memo.shown
                .as_ref()
                .and_then(|shown| shown.line.as_ref())
                .map(|line| line.text.clone()),
        );
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut LabelLayout,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let memo = layout.memo.borrow();
        let Some(line) = memo.shown.as_ref().and_then(|shown| shown.line.as_ref()) else {
            return;
        };
        #[cfg(test)]
        observe::painted(&self.id);
        if let Err(err) = line.paint(
            bounds.origin,
            layout.line_height,
            layout.align,
            Some(bounds.size.width),
            window,
            cx,
        ) {
            log::warn!("could not paint an elided label: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    //! These drive the real render, layout, prepaint and paint path of a
    //! window. `TestAppContext` shapes through gpui's `NoopTextSystem`, where
    //! every `char` advances the same 0.6 of the font size, so widths here are
    //! multiples of a cell the tests measure, never constants. They speak to
    //! *when* work happens and to the shape of the result, not to real font
    //! widths: those are checked in native captures.

    use super::*;
    use gpui::{
        Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
        Render, Styled as _, TestAppContext, VisualTestContext, div, hsla,
    };

    struct Row {
        id: SharedString,
        text: SharedString,
        tail: Option<SharedString>,
        tail_included: Option<&'static str>,
        mode: Mode,
        width: f32,
        size: f32,
        weight: FontWeight,
        color: Hsla,
        max_chars: Option<usize>,
        min_visible: Option<usize>,
    }

    impl Row {
        fn new() -> Self {
            Self {
                id: "probe".into(),
                text: "window-transparency-backdrop-and-more-words".into(),
                tail: None,
                tail_included: None,
                mode: Mode::End,
                width: 280.,
                size: 14.,
                weight: FontWeight::NORMAL,
                color: hsla(0., 0., 0.9, 1.),
                max_chars: None,
                min_visible: None,
            }
        }
    }

    impl Render for Row {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut label = ElidedLabel::new(self.id.clone(), self.text.clone(), self.mode);
            if let Some(tail) = &self.tail {
                label = label.tail(tail.clone());
            }
            if let Some(tail) = self.tail_included {
                label = label.tail_included(tail);
            }
            if let Some(cap) = self.max_chars {
                label = label.max_chars(cap);
            }
            if let Some(min) = self.min_visible {
                label = label.min_visible(min);
            }
            div()
                .w(px(self.width))
                .text_size(px(self.size))
                .font_weight(self.weight)
                .text_color(self.color)
                .child(label)
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<Row>, &mut VisualTestContext) {
        open_with(cx, Row::new())
    }

    fn open_with(cx: &mut TestAppContext, row: Row) -> (Entity<Row>, &mut VisualTestContext) {
        let (shapes, elisions) = counts();
        let (row, vcx) = cx.add_window_view(move |_, _| row);
        let (shapes_created, elisions_created) = counts();
        // Creating the window draws its first frame, which is where the label
        // measures, elides and shapes. Every test below compares counts across
        // later frames; they would all pass on a label that never did any work
        // if that first frame did not.
        assert!(
            shapes_created > shapes && elisions_created > elisions,
            "the first frame must measure and elide"
        );
        draw(&row, vcx, |_| {});
        (row, vcx)
    }

    /// Changes the row, then lays out and paints a frame.
    fn draw(row: &Entity<Row>, vcx: &mut VisualTestContext, change: impl FnOnce(&mut Row)) {
        observe::clear();
        row.update(vcx, |row, cx| {
            change(row);
            cx.notify();
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn counts() -> (usize, usize) {
        PROBE.with(|p| (p.borrow().shapes, p.borrow().elisions))
    }

    /// What the row's label prepared in the latest frame; `Some` even when it
    /// is hidden, so a test cannot mistake a label that was never drawn for one
    /// that is.
    fn observed() -> observe::Label {
        observe::label(&ElementId::from(SharedString::from("probe")))
            .expect("the label was drawn in the latest frame")
    }

    fn shown() -> SharedString {
        observed().text
    }

    fn hidden() -> bool {
        observed().text.is_empty()
    }

    fn painted() -> bool {
        observed().painted
    }

    /// One glyph cell in this window's text system: widths in the tests below
    /// are multiples of it, so they hold whatever the metrics are.
    fn cell(vcx: &mut VisualTestContext) -> f32 {
        vcx.update(|window, _| measure_text(window.text_system(), &gpui::Font::default(), 14., "m"))
    }

    #[gpui::test]
    fn the_label_is_cut_to_the_width_it_is_given(cx: &mut TestAppContext) {
        let (_row, _vcx) = open(cx);
        let out = shown();
        assert!(out.ends_with('…'), "{out}");
        assert!(out.chars().count() < "window-transparency-backdrop-and-more-words".len());
    }

    #[gpui::test]
    fn an_unchanged_key_shapes_nothing(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let before = counts();
        for _ in 0..3 {
            draw(&row, vcx, |_| {});
        }
        assert_eq!(counts(), before, "three identical frames did work");
    }

    /// The memo is the element's own state, found again by its id. An id that
    /// changes every frame finds nothing, and the test shows it as repeated
    /// work instead of letting it pass as a cache.
    #[gpui::test]
    fn an_unstable_id_loses_the_memo_and_shows_as_work(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let before = counts();
        draw(&row, vcx, |row| row.id = "other".into());
        let after = counts();
        assert!(
            after.0 > before.0 && after.1 == before.1 + 1,
            "{before:?} {after:?}"
        );
    }

    #[gpui::test]
    fn a_width_change_reshapes_and_an_unchanged_width_does_not(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let wide = shown();
        let (shapes, elisions) = counts();
        draw(&row, vcx, |row| row.width = 140.);
        let (shapes_after, elisions_after) = counts();
        assert!(shapes_after > shapes, "a new width must be measured");
        assert_eq!(elisions_after, elisions + 1);
        let narrow = shown();
        assert!(
            narrow.chars().count() < wide.chars().count(),
            "{narrow} should be shorter than {wide}"
        );
        draw(&row, vcx, |_| {});
        assert_eq!(counts(), (shapes_after, elisions_after));
    }

    #[gpui::test]
    fn each_field_of_the_key_invalidates_the_memo(cx: &mut TestAppContext) {
        type Change = fn(&mut Row);
        let changes: [(&str, Change); 7] = [
            ("text", |row| {
                row.text = "release-candidate-final-build-and-more-words".into()
            }),
            ("tail", |row| row.tail = Some(" — disconnected".into())),
            ("mode", |row| row.mode = Mode::Start),
            ("size", |row| row.size = 15.),
            ("weight", |row| row.weight = FontWeight::BOLD),
            ("min_visible", |row| row.min_visible = Some(3)),
            ("max_chars", |row| row.max_chars = Some(12)),
        ];
        for (field, change) in changes {
            let (row, vcx) = open(cx);
            let (shapes, elisions) = counts();
            draw(&row, vcx, change);
            let (shapes_after, elisions_after) = counts();
            assert!(shapes_after > shapes, "{field}: nothing was measured");
            assert_eq!(elisions_after, elisions + 1, "{field}: not recomputed once");
        }
    }

    /// Colour is painted from the line, not part of what is elided: a recolour
    /// prepares one new line and does not cut the text again.
    #[gpui::test]
    fn a_recolour_reshapes_the_line_only(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let before = shown();
        let (shapes, elisions) = counts();
        draw(&row, vcx, |row| row.color = hsla(0.5, 0.5, 0.5, 1.));
        assert_eq!(counts(), (shapes + 1, elisions));
        assert_eq!(shown(), before);
    }

    /// Hidden exactly when fewer than six clusters of the text survive, swept
    /// across widths measured in glyph cells.
    #[gpui::test]
    fn a_label_below_min_visible_is_hidden_and_paints_nothing(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let cell = cell(vcx);
        for cells in 1..=12 {
            draw(&row, vcx, |row| {
                row.min_visible = Some(6);
                row.width = cell * cells as f32 + cell / 2.;
            });
            // `cells` glyphs fit, one of them the `…`: `cells - 1` kept.
            let expect_hidden = cells - 1 < 6;
            assert_eq!(hidden(), expect_hidden, "{cells} cells");
            assert_eq!(painted(), !expect_hidden, "{cells} cells");
            if !expect_hidden {
                assert!(shown().ends_with('…'), "{}", shown());
            }
        }
    }

    #[gpui::test]
    fn a_hidden_frame_shapes_nothing_and_paints_nothing(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let cell = cell(vcx);
        draw(&row, vcx, |row| {
            row.min_visible = Some(6);
            row.width = cell * 3.5;
        });
        assert!(hidden());
        let before = counts();
        for _ in 0..3 {
            draw(&row, vcx, |_| {});
            // Observed on every frame, cache hits included.
            assert!(hidden() && !painted());
        }
        assert_eq!(counts(), before);
    }

    /// With a tail the whole label goes: no orphan separator is left behind.
    #[gpui::test]
    fn the_tail_goes_with_a_hidden_label(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let cell = cell(vcx);
        draw(&row, vcx, |row| {
            row.min_visible = Some(6);
            row.tail = Some(" · ".into());
            row.width = cell * 8.5;
        });
        assert!(hidden(), "{}", shown());
        draw(&row, vcx, |row| row.width = cell * 20.5);
        assert!(!hidden());
        let out = shown();
        assert!(out.ends_with(" · "), "{out}");
        assert!(out.contains('…'), "{out}");
    }

    /// The scalar cap holds although the width would allow far more, and the
    /// layout box is the capped text's, not the whole name's.
    #[gpui::test]
    fn the_cap_binds_before_the_width_does(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        draw(&row, vcx, |row| row.max_chars = Some(10));
        let out = shown();
        assert!(out.chars().count() <= 10, "{out}");
        assert!(out.ends_with('…'), "{out}");
        // Narrower than the cap: the width binds instead.
        let cell = cell(vcx);
        draw(&row, vcx, |row| row.width = cell * 4.5);
        assert!(shown().chars().count() <= 4, "{}", shown());
    }

    /// `Label` classifies from the text, and only on a miss: the same mode cuts
    /// a path like a path and a name like a name, and changing the text
    /// changes the choice through the key.
    #[gpui::test]
    fn a_label_is_cut_as_a_path_or_as_a_name_by_its_text(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        let cell = cell(vcx);
        draw(&row, vcx, |row| {
            row.mode = Mode::Label;
            row.width = cell * 10.5;
            row.text = "projects/toolbox/src/ui/tab_sidebar.rs".into();
        });
        assert!(
            shown().starts_with('…'),
            "a path keeps its tail: {}",
            shown()
        );
        draw(&row, vcx, |row| {
            row.text = "window-transparency-backdrop-and-more-words".into();
        });
        assert!(
            shown().starts_with("window"),
            "a name keeps its head: {}",
            shown()
        );
    }

    /// A label inside a content-sized box, to read the box layout made for it.
    struct Boxed {
        cap: Option<usize>,
    }

    impl Render for Boxed {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut label = ElidedLabel::new(
                "boxed",
                "window-transparency-backdrop-and-more-words",
                Mode::End,
            );
            if let Some(cap) = self.cap {
                label = label.max_chars(cap);
            }
            div()
                .flex()
                .w(px(280.))
                .text_size(px(14.))
                .child(div().debug_selector(|| "box".into()).child(label))
        }
    }

    /// The box is the capped text's width, not the whole name's: a label capped
    /// at ten characters does not reserve the room of forty.
    #[gpui::test]
    fn the_layout_box_is_the_capped_text_not_the_whole_name(cx: &mut TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, _| Boxed { cap: None });
        let cell = cell(vcx);
        let box_width = |vcx: &mut VisualTestContext| {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            vcx.debug_bounds("box")
                .expect("the box was laid out")
                .size
                .width
        };
        let uncapped = box_width(vcx);
        view.update(vcx, |view, cx| {
            view.cap = Some(10);
            cx.notify();
        });
        let capped = box_width(vcx);
        assert!(
            capped < uncapped,
            "{capped:?} should be narrower than {uncapped:?}"
        );
        assert!(capped <= px(cell * 10.5), "{capped:?} for ten characters");
    }

    /// The titles and tails of an ended tab, in the three shipped languages.
    const ENDED: [(&str, &str); 3] = [
        (
            "window-transparency-backdrop-and-more-words",
            " — disconnected",
        ),
        ("очень-длинное-название-вкладки-терминала", " — отключено"),
        ("很长很长的终端标签标题很长很长的终端标签", " — 已断开连接"),
    ];

    fn ended(head: &str, tail: &'static str, included: bool) -> Row {
        if included {
            Row {
                text: format!("{head}{tail}").into(),
                tail_included: Some(tail),
                ..Row::new()
            }
        } else {
            Row {
                text: head.into(),
                tail: Some(tail.into()),
                ..Row::new()
            }
        }
    }

    /// The full string with its tail declared as included shows exactly what the
    /// same words appended show, at every width, with and without a minimum.
    #[gpui::test]
    fn included_tail_shows_exactly_what_the_appended_tail_shows(cx: &mut TestAppContext) {
        let (_row, vcx) = open(cx);
        let cell = cell(vcx);
        for (head, tail) in ENDED {
            for mode in [Mode::Label, Mode::End] {
                for cells in [3.5, 12.5, 20.5, 40.5] {
                    for min_visible in [None, Some(6)] {
                        let width = cell * cells;
                        let configure = |mut row: Row| {
                            row.mode = mode;
                            row.width = width;
                            row.min_visible = min_visible;
                            row
                        };
                        open_with(cx, configure(ended(head, tail, false)));
                        let appended = observed();
                        open_with(cx, configure(ended(head, tail, true)));
                        let included = observed();
                        assert_eq!(
                            appended, included,
                            "{head}{tail} {mode:?} @{cells} cells, min {min_visible:?}"
                        );
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn an_unchanged_included_frame_shapes_nothing(cx: &mut TestAppContext) {
        let (head, tail) = ENDED[0];
        let (row, vcx) = open_with(cx, ended(head, tail, true));
        let before = counts();
        for _ in 0..3 {
            draw(&row, vcx, |_| {});
        }
        assert_eq!(counts(), before, "three identical frames did work");
    }

    /// The declared tail is part of the key, with the full text and the mode.
    #[gpui::test]
    fn the_included_tail_is_part_of_the_key(cx: &mut TestAppContext) {
        type Change = fn(&mut Row);
        let (head, tail) = ENDED[0];
        let changes: [(&str, Change); 3] = [
            ("tail value", |row| {
                row.tail_included = Some(" disconnected")
            }),
            ("full text", |row| {
                row.text = "release-candidate-final-build-and-more-words — disconnected".into()
            }),
            ("mode", |row| row.mode = Mode::Label),
        ];
        for (field, change) in changes {
            let (row, vcx) = open_with(cx, ended(head, tail, true));
            let (shapes, elisions) = counts();
            draw(&row, vcx, change);
            let (shapes_after, elisions_after) = counts();
            assert!(shapes_after > shapes, "{field}: nothing was measured");
            assert_eq!(elisions_after, elisions + 1, "{field}: not recomputed once");
        }
    }

    /// A text that does not end with the declared tail is elided as the plain
    /// string it is, and the tail is not spelled a second time.
    #[gpui::test]
    fn a_tail_the_text_does_not_end_with_falls_back_to_plain_elision(cx: &mut TestAppContext) {
        let text = "window-transparency-backdrop-and-more-words";
        let cell = {
            let (_row, vcx) = open(cx);
            cell(vcx)
        };
        for cells in [6.5, 14.5, 25.5] {
            let width = cell * cells;
            open_with(
                cx,
                Row {
                    text: text.into(),
                    tail_included: Some(" — gone"),
                    width,
                    ..Row::new()
                },
            );
            let declared = observed();
            open_with(
                cx,
                Row {
                    text: text.into(),
                    width,
                    ..Row::new()
                },
            );
            let plain = observed();
            assert_eq!(declared, plain, "@{cells} cells");
            assert!(!declared.text.contains("gone"), "{declared:?}");
        }
    }

    /// A label inside a content-sized box, appended or included.
    struct BoxedTail {
        included: bool,
    }

    impl Render for BoxedTail {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let label = if self.included {
                ElidedLabel::new("boxed", "terminal — disconnected", Mode::End)
                    .tail_included(" — disconnected")
            } else {
                ElidedLabel::new("boxed", "terminal", Mode::End).tail(" — disconnected")
            };
            div()
                .flex()
                .w(px(280.))
                .text_size(px(14.))
                .child(div().debug_selector(|| "box".into()).child(label))
        }
    }

    /// Both spell the same string, so layout gives both the same box.
    #[gpui::test]
    fn the_layout_box_of_an_included_tail_matches_the_appended_one(cx: &mut TestAppContext) {
        let mut widths = Vec::new();
        for included in [false, true] {
            let (_view, vcx) = cx.add_window_view(move |_, _| BoxedTail { included });
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            widths.push(
                vcx.debug_bounds("box")
                    .expect("the box was laid out")
                    .size
                    .width,
            );
        }
        assert!(widths[0] > px(0.));
        assert_eq!(widths[0], widths[1]);
    }

    /// Assistive tech gets the whole text once: an included tail is inside the
    /// value and is not also described; an appended one is described.
    #[test]
    fn a11y_carries_the_whole_text_once() {
        use gpui::accesskit::{Node, Role};
        let included = ElidedLabel::new("a", "title — gone", Mode::Label).tail_included(" — gone");
        let mut node = Node::new(Role::Label);
        included.write_a11y_info(&mut node);
        assert_eq!(node.value(), Some("title — gone"));
        assert_eq!(node.description(), None);

        let appended = ElidedLabel::new("a", "title", Mode::Label).tail(" — gone");
        let mut node = Node::new(Role::Label);
        appended.write_a11y_info(&mut node);
        assert_eq!(node.value(), Some("title"));
        assert_eq!(node.description(), Some(" — gone"));
    }

    /// Russian and Chinese tails and titles, at every width: a prefix of the
    /// title and the whole tail, or the tail alone, never a torn character.
    #[gpui::test]
    fn included_utf8_tails_never_split_a_char(cx: &mut TestAppContext) {
        let (_row, vcx) = open(cx);
        let cell = cell(vcx);
        for (head, tail) in ENDED {
            let bare = tail.trim_start_matches(|c: char| c.is_whitespace() || c == '—');
            for cells in 1..=45 {
                let mut row = ended(head, tail, true);
                row.width = cell * cells as f32 + cell / 2.;
                open_with(cx, row);
                let out = shown();
                let text: &str = &out;
                if let Some(cut) = text.strip_suffix(tail) {
                    let kept = cut.strip_suffix('…').unwrap_or(cut);
                    assert!(head.starts_with(kept), "{text:?} @{cells}");
                } else {
                    assert!(
                        text.is_empty() || bare.starts_with(text.trim_end_matches('…')),
                        "{text:?} @{cells}"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn the_tail_survives_and_only_the_text_is_cut(cx: &mut TestAppContext) {
        let (row, vcx) = open(cx);
        draw(&row, vcx, |row| {
            row.mode = Mode::Edges;
            row.tail = Some(" — not connected".into());
        });
        let out = shown();
        assert!(out.ends_with(" — not connected"), "{out}");
        assert!(out.contains('…'), "{out}");
    }
}
