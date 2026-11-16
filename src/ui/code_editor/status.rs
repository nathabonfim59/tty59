//! Which readouts the editor's status bar has room for.
//!
//! The bar is one row in the document column. Every readout is `flex_none`, so
//! a column too narrow for all of them used to clip whatever was last, the
//! language included. Layout cannot drop a flex item, so the decision is made
//! here from measured widths: the lowest-ranked readouts are not built at all
//! until the rest fit.
//!
//! ## Where the numbers come from
//!
//! - The room is the document column's own width ([`Tty59App::editor_header_w`]),
//!   less the row's inset on both sides.
//! - Every width is a text width plus the constants the painters use for the
//!   non-text parts (dot, icon, gaps), all defined in this file, so a width and
//!   the element drawn for it cannot disagree.
//! - A text width is memoized in the element state of the bar, in one fixed slot
//!   per piece of text, keyed by the text, the full [`gpui::Font`] it is drawn in
//!   (family, weight, style, features, fallbacks) and the size. A frame whose
//!   keys are unchanged measures nothing; a changed key measures that slot
//!   only. Nothing grows, and GPUI drops the state with the bar. No app or
//!   model state is read or written.
//! - [`Face`] is the one description of the type the bar is set in. The row is
//!   pinned to it ([`Face::pin`]) so nothing is inherited that the measure did
//!   not see, and the keys are built from it.

use gpui::{
    App, ElementId, Font, FontFeatures, FontStyle, FontWeight, Pixels, SharedString, Styled,
    Window, px, rems,
};
use gpui_component::ActiveTheme as _;

/// The gap between the row's children. The spacer between the leading note
/// and the readouts is a child too, so it spends one of these as well.
pub(crate) const ROW_GAP: f32 = 14.;
/// The missing-server note's dot, and its gap to the label.
pub(crate) const DOT: f32 = 5.;
pub(crate) const LEAD_GAP: f32 = 6.;
/// An icon and its count, and one count and the next.
pub(crate) const COUNT_GAP: f32 = 3.;
pub(crate) const PAIR_GAP: f32 = 8.;

/// The status text size, the 11.5px token, relative to the root.
const TEXT_REM: f32 = 11.5 / 16.;
/// gpui-component's `xsmall` icon (`size_3`), so the painter can draw the same
/// icon at an explicit size the width sum knows.
const ICON_REM: f32 = 0.75;

/// The type the bar is set in.
#[derive(Clone)]
pub(crate) struct Face {
    family: SharedString,
    pub(crate) size: Pixels,
    pub(crate) icon: Pixels,
    tabular: FontFeatures,
}

impl Face {
    pub(crate) fn new(window: &Window, cx: &App) -> Self {
        let rem = window.rem_size();
        Self {
            family: cx.theme().font_family.clone(),
            size: rems(TEXT_REM).to_pixels(rem),
            icon: rems(ICON_REM).to_pixels(rem),
            tabular: crate::ui::theme::tabular_figures(),
        }
    }

    /// Features for a piece of text: tabular figures where the digits must not
    /// jitter sideways, the font's defaults elsewhere.
    pub(crate) fn features(&self, tabular: bool) -> FontFeatures {
        if tabular {
            self.tabular.clone()
        } else {
            FontFeatures::default()
        }
    }

    fn font(&self, tabular: bool) -> Font {
        Font {
            family: self.family.clone(),
            features: self.features(tabular),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    /// Sets the row in this face. What a child does not override it inherits
    /// from here, and the measure assumes exactly that.
    pub(crate) fn pin<E: Styled>(&self, row: E) -> E {
        row.font_family(self.family.clone())
            .text_size(self.size)
            .font_weight(FontWeight::NORMAL)
            .not_italic()
            .font_features(FontFeatures::default())
    }
}

/// A piece of text the bar draws, which has its own width slot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Lead,
    Lsp,
    Errors,
    Warnings,
    Cursor,
    Language,
    Eol,
    Wrap,
    Preview,
    Zoom,
    Indent,
    Encoding,
}

const PARTS: usize = 12;

impl Part {
    /// Digits that change under the caret or a count: tabular, as drawn.
    fn tabular(self) -> bool {
        matches!(self, Part::Errors | Part::Warnings | Part::Cursor)
    }
}

/// One unit the bar keeps or drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Readout {
    /// The missing-server note.
    Lead,
    /// The server's state, or its error and warning counts.
    Problems,
    Cursor,
    Language,
    Eol,
    Wrap,
    Preview,
    Zoom,
    Indent,
    Encoding,
}

const READOUTS: usize = 10;

/// Kept first to last. Diagnostics outrank every formatting readout, and
/// encoding and then indentation are the first to go.
const RANK: [Readout; READOUTS] = [
    Readout::Lead,
    Readout::Problems,
    Readout::Cursor,
    Readout::Language,
    Readout::Eol,
    Readout::Wrap,
    Readout::Preview,
    Readout::Zoom,
    Readout::Indent,
    Readout::Encoding,
];

/// What the server readout says.
pub(crate) enum Lsp {
    State(SharedString),
    Counts {
        errors: SharedString,
        warnings: SharedString,
    },
}

/// The texts the bar would draw if it had room for all of them.
#[derive(Default)]
pub(crate) struct Content {
    pub(crate) lead: Option<SharedString>,
    pub(crate) lsp: Option<Lsp>,
    pub(crate) cursor: Option<SharedString>,
    pub(crate) language: Option<SharedString>,
    pub(crate) eol: Option<SharedString>,
    pub(crate) wrap: Option<SharedString>,
    pub(crate) preview: Option<SharedString>,
    pub(crate) zoom: Option<SharedString>,
    pub(crate) indent: Option<SharedString>,
    pub(crate) encoding: Option<SharedString>,
}

/// Names an element for the layout tests. Outside a test build this is the
/// element itself: no selector, no closure.
#[cfg(test)]
pub(crate) fn probe<E>(element: E, name: &'static str) -> E
where
    E: gpui::InteractiveElement,
{
    element.debug_selector(|| name.into())
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn probe<E>(element: E, _name: &'static str) -> E {
    element
}

/// What the tests read back of text the panels passed to their elements.
#[cfg(test)]
pub(crate) mod observe {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static SAID: RefCell<HashMap<&'static str, Vec<String>>> = RefCell::new(HashMap::new());
        static FRAME: RefCell<Option<(super::Pixels, Vec<super::Readout>)>> = const { RefCell::new(None) };
    }

    /// The room the bar was given in the last frame, and what it kept in it.
    pub(crate) fn record(room: super::Pixels, kept: super::Kept) {
        let names = super::RANK
            .iter()
            .copied()
            .filter(|r| kept.has(*r))
            .collect();
        FRAME.with(|frame| *frame.borrow_mut() = Some((room, names)));
    }

    pub(crate) fn frame() -> Option<(super::Pixels, Vec<super::Readout>)> {
        FRAME.with(|frame| frame.borrow().clone())
    }

    /// Records `text` as what the element `name` was given this frame.
    pub(crate) fn say(name: &'static str, text: &str) {
        SAID.with(|said| {
            said.borrow_mut()
                .entry(name)
                .or_default()
                .push(text.to_string())
        });
    }

    pub(crate) fn clear() {
        SAID.with(|said| said.borrow_mut().clear());
    }

    /// The last text `name` was given since the last `clear`.
    pub(crate) fn last(name: &'static str) -> Option<String> {
        SAID.with(|said| {
            said.borrow()
                .get(name)
                .and_then(|texts| texts.last().cloned())
        })
    }
}

/// Which readouts are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kept([bool; READOUTS]);

impl Kept {
    pub(crate) fn has(&self, readout: Readout) -> bool {
        self.0[readout as usize]
    }
}

#[cfg(test)]
thread_local! {
    static MEASURES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many widths have been measured on this thread, for tests that hold a
/// frame to measuring nothing.
#[cfg(test)]
pub(crate) fn measures() -> usize {
    MEASURES.with(|m| m.get())
}

struct Slot {
    text: SharedString,
    font: Font,
    size: Pixels,
    width: Pixels,
}

#[derive(Default)]
struct Memo {
    slots: [Option<Slot>; PARTS],
}

impl Memo {
    /// The width of `text` drawn as `part` in `face`, measured only when the
    /// slot holds a different key.
    fn text(&mut self, part: Part, text: &SharedString, face: &Face, window: &Window) -> Pixels {
        let font = face.font(part.tabular());
        if let Some(slot) = &self.slots[part as usize]
            && slot.text == *text
            && slot.font == font
            && slot.size == face.size
        {
            return slot.width;
        }
        // GPUI sizes each text node up to a whole pixel, so that is the width
        // the layout will give it and the sum is taken over.
        let width = px(crate::ui::text_elision::measure_text(
            window.text_system(),
            &font,
            f32::from(face.size),
            text,
        )
        .ceil());
        #[cfg(test)]
        MEASURES.with(|m| m.set(m.get() + 1));
        self.slots[part as usize] = Some(Slot {
            text: text.clone(),
            font,
            size: face.size,
            width,
        });
        width
    }

    /// The width a readout is drawn at, from its parts: the text widths and
    /// the constants the painter lays them out with.
    fn readout(
        &mut self,
        readout: Readout,
        content: &Content,
        face: &Face,
        window: &Window,
    ) -> Option<Pixels> {
        let plain = |memo: &mut Self, part: Part, text: &Option<SharedString>| {
            text.as_ref().map(|t| memo.text(part, t, face, window))
        };
        match readout {
            Readout::Lead => {
                plain(self, Part::Lead, &content.lead).map(|w| px(DOT) + px(LEAD_GAP) + w)
            }
            Readout::Problems => match content.lsp.as_ref()? {
                Lsp::State(text) => Some(self.text(Part::Lsp, text, face, window)),
                Lsp::Counts { errors, warnings } => {
                    let errors = self.text(Part::Errors, errors, face, window);
                    let warnings = self.text(Part::Warnings, warnings, face, window);
                    Some(
                        face.icon
                            + px(COUNT_GAP)
                            + errors
                            + px(PAIR_GAP)
                            + face.icon
                            + px(COUNT_GAP)
                            + warnings,
                    )
                }
            },
            Readout::Cursor => plain(self, Part::Cursor, &content.cursor),
            Readout::Language => plain(self, Part::Language, &content.language),
            Readout::Eol => plain(self, Part::Eol, &content.eol),
            Readout::Wrap => plain(self, Part::Wrap, &content.wrap),
            Readout::Preview => plain(self, Part::Preview, &content.preview),
            Readout::Zoom => plain(self, Part::Zoom, &content.zoom),
            Readout::Indent => plain(self, Part::Indent, &content.indent),
            Readout::Encoding => plain(self, Part::Encoding, &content.encoding),
        }
    }
}

/// Drops readouts from the lowest rank up until the rest fit `room`. A
/// readout that is absent has no width and is never kept. The leading note is
/// never dropped: it is the one thing the bar is there to say.
fn drop_to_fit(widths: [Option<Pixels>; READOUTS], room: Pixels) -> Kept {
    let mut kept = [false; READOUTS];
    for (slot, width) in kept.iter_mut().zip(widths) {
        *slot = width.is_some();
    }
    let needs = |kept: &[bool; READOUTS]| {
        let mut sum = px(0.);
        let mut children = 0;
        for (width, &is_kept) in widths.iter().zip(kept) {
            if let (Some(width), true) = (*width, is_kept) {
                sum += width;
                children += 1;
            }
        }
        // The spacer is a child too: n readouts have n gaps among them.
        sum + px(ROW_GAP) * children as f32
    };
    for readout in RANK.iter().rev() {
        if needs(&kept) <= room {
            break;
        }
        if *readout != Readout::Lead {
            kept[*readout as usize] = false;
        }
    }
    Kept(kept)
}

/// Which readouts of `content` the bar draws in `room`, from the bar's own
/// element state.
pub(crate) fn kept(window: &mut Window, face: &Face, content: &Content, room: Pixels) -> Kept {
    window.with_global_id(ElementId::from("editor-status-fit"), |id, window| {
        window.with_element_state(id, |memo: Option<Memo>, window| {
            let mut memo = memo.unwrap_or_default();
            let mut widths = [None; READOUTS];
            for readout in RANK {
                widths[readout as usize] = memo.readout(readout, content, face, window);
            }
            let kept = drop_to_fit(widths, room);
            #[cfg(test)]
            observe::record(room, kept);
            (kept, memo)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(w: f32) -> [Option<Pixels>; READOUTS] {
        [Some(px(w)); READOUTS]
    }

    fn dropped(kept: Kept) -> Vec<Readout> {
        RANK.iter().copied().filter(|r| !kept.has(*r)).collect()
    }

    #[test]
    fn nothing_is_dropped_when_everything_fits() {
        // Ten readouts of 10 and ten gaps of 14.
        assert!(dropped(drop_to_fit(all(10.), px(240.))).is_empty());
        assert!(!dropped(drop_to_fit(all(10.), px(239.))).is_empty());
    }

    #[test]
    fn encoding_then_indentation_go_first_and_diagnostics_stay() {
        let mut widths = all(10.);
        widths[Readout::Zoom as usize] = None;
        // Nine present: 90 + 9 * 14 = 216.
        assert_eq!(
            dropped(drop_to_fit(widths, px(215.))),
            vec![Readout::Zoom, Readout::Encoding]
        );
        assert_eq!(
            dropped(drop_to_fit(widths, px(191.))),
            vec![Readout::Zoom, Readout::Indent, Readout::Encoding]
        );
        let narrow = drop_to_fit(widths, px(100.));
        assert!(narrow.has(Readout::Lead) && narrow.has(Readout::Problems));
        assert!(!narrow.has(Readout::Wrap) && !narrow.has(Readout::Encoding));
    }

    #[test]
    fn the_leading_note_is_never_dropped() {
        let kept = drop_to_fit(all(10.), px(1.));
        assert!(kept.has(Readout::Lead));
        assert_eq!(dropped(kept).len(), READOUTS - 1);
    }
}

/// The bar and the empty panel in a real window: layout decides what is
/// kept, the memo decides what is measured, and the panels follow the keymap.
#[cfg(test)]
mod gpui_tests {
    use std::sync::Arc;

    use gpui::{
        Bounds, Entity, KeyBinding, Keystroke, Modifiers, TestAppContext, VisualTestContext, size,
    };
    use gpui_component::input::Position;

    use super::*;
    use crate::core::actions::QuickOpenFile;
    use crate::core::config::{Config, KeybindingOverride, RightPanelTab};
    use crate::ui::app::Tty59App;
    use crate::ui::app::test_window::harness_with_tabs;
    use crate::ui::code_editor::nav::gpui_tests::open_rs;
    use crate::ui::lsp::{LspStore, TestState};

    const ITEMS: [&str; 6] = [
        "editor-status-wrap",
        "editor-status-cursor",
        "editor-status-indent",
        "editor-status-encoding",
        "editor-status-eol",
        "editor-status-language",
    ];
    const PATH: &str = "/status-test/a.rs";

    fn paint(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    /// Another frame of the whole app, as any notify makes.
    fn redraw(app: &Entity<Tty59App>, vcx: &mut VisualTestContext) {
        app.update(vcx, |_, cx| cx.notify());
        paint(vcx);
    }

    fn at(vcx: &mut VisualTestContext, width: f32) {
        vcx.simulate_resize(size(px(width), px(700.)));
        paint(vcx);
    }

    fn inside(inner: Bounds<Pixels>, row: Bounds<Pixels>) -> bool {
        inner.left() >= row.left() && inner.right() <= row.right()
    }

    fn spelled(keys: &str) -> String {
        gpui_component::kbd::Kbd::format(&Keystroke::parse(keys).expect("a keystroke"))
    }

    fn enlarge_ui(vcx: &mut VisualTestContext, by: f32) {
        vcx.update(|_, cx| {
            let mut config = cx.global::<Config>().clone();
            config.ui_font_size += by;
            cx.set_global(config);
        });
    }

    /// Window widths from wide to narrow: the bar keeps its readouts inside
    /// the row at every one of them, and encoding goes before indentation,
    /// both before the cursor and the language.
    #[gpui::test]
    fn a_narrow_column_drops_encoding_then_indentation_and_nothing_overflows(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, PATH);
        let mut encoding_alone_gone = false;
        let mut both_gone = false;
        let mut dropped_anything = false;
        for width in (260..=1600).rev().step_by(20) {
            at(&mut vcx, width as f32);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            let shown: Vec<(&str, Option<Bounds<Pixels>>)> = ITEMS
                .iter()
                .map(|name| (*name, vcx.debug_bounds(name)))
                .collect();
            for (name, bounds) in &shown {
                if let Some(b) = bounds {
                    assert!(inside(*b, row), "{width}: {name} {b:?} is outside {row:?}");
                }
            }
            let has = |name: &str| shown.iter().any(|(n, b)| *n == name && b.is_some());
            if width == 1600 {
                for name in ITEMS {
                    assert!(has(name), "{name} has room at {width}");
                }
            }
            if has("editor-status-encoding") {
                assert!(
                    has("editor-status-indent"),
                    "{width}: encoding kept but indentation dropped"
                );
            }
            if has("editor-status-language") {
                assert!(
                    has("editor-status-cursor"),
                    "{width}: the language outlived the cursor"
                );
            }
            let (encoding, indent) = (has("editor-status-encoding"), has("editor-status-indent"));
            if !encoding && indent {
                encoding_alone_gone = true;
            }
            if !encoding && !indent {
                assert!(
                    encoding_alone_gone,
                    "{width}: both went at once; encoding must go alone first"
                );
                both_gone = true;
            }
            dropped_anything |= ITEMS.iter().any(|name| !has(name));
        }
        assert!(dropped_anything, "no tested width was narrow enough");
        assert!(encoding_alone_gone && both_gone);
    }

    /// With a real running server the counts are the Problems readout, and it
    /// outlives encoding and indentation at every width; the starting and
    /// stopped states keep theirs the same way.
    #[gpui::test]
    fn the_problem_counts_outlive_encoding_and_indentation(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        vcx.update(|_, cx| LspStore::mark_missing_for_test("rust-analyzer", cx));
        open_rs(&app, &mut vcx, PATH);
        let input = app.read_with(&vcx, |app, _| app.active_buffer().unwrap().input.clone());
        let set = |vcx: &mut VisualTestContext, state: TestState| {
            vcx.update(|_, cx| {
                LspStore::set_state_for_test(
                    &std::path::PathBuf::from(PATH),
                    "rust",
                    "rust-analyzer",
                    state,
                    &input,
                    cx,
                )
            });
        };
        set(
            &mut vcx,
            TestState::Running {
                errors: 2,
                warnings: 1,
            },
        );
        // The counts' own width, read from the layout where nothing is short
        // of room; below, they are there exactly when that width and one row
        // gap fit the row's inner room, as measured at that width.
        at(&mut vcx, 1600.);
        let natural = vcx
            .debug_bounds("editor-status-problems")
            .expect("the counts at 1600")
            .size
            .width;
        let mut encoding_dropped = false;
        for width in (260..=1600).rev().step_by(20) {
            at(&mut vcx, width as f32);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            let problems = vcx.debug_bounds("editor-status-problems");
            let inner = row.size.width - px(2. * crate::ui::app::CONTENT_INSET);
            let fits = natural + px(ROW_GAP) <= inner;
            assert_eq!(
                problems.is_some(),
                fits,
                "{width}: counts {problems:?}, natural {natural:?} + gap in inner {inner:?} (row {row:?}, frame {:?})",
                observe::frame()
            );
            // The product's own window is never narrower than 720.
            if width >= 720 {
                assert!(problems.is_some(), "{width}: the counts were dropped");
            }
            // Priority, wherever there is room for anything: no lower
            // readout is shown while the counts are not.
            if problems.is_none() {
                for name in ITEMS {
                    assert!(
                        vcx.debug_bounds(name).is_none(),
                        "{width}: {name} is shown without the counts"
                    );
                }
            }
            if let Some(problems) = problems {
                assert!(
                    inside(problems, row),
                    "{width}: {problems:?} outside {row:?}"
                );
            }
            encoding_dropped |= vcx.debug_bounds("editor-status-encoding").is_none();
        }
        assert!(encoding_dropped, "no tested width dropped encoding");

        // The server's other states, at the narrowest window the product
        // makes (`MIN_SIZE` is 720 wide): their text is wider than the
        // document room of the synthetic widths above, where nothing is
        // promised.
        for state in [TestState::Starting, TestState::Down] {
            set(&mut vcx, state);
            at(&mut vcx, 720.);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            let shown = vcx
                .debug_bounds("editor-status-problems")
                .unwrap_or_else(|| {
                    panic!(
                        "the server's state is dropped at 720 (row {row:?}, frame {:?})",
                        observe::frame()
                    )
                });
            assert!(
                shown.size.width > px(0.) && shown.size.height > px(0.),
                "720: no target: {shown:?}"
            );
            assert!(inside(shown, row), "720: {shown:?} outside {row:?}");
        }
    }

    /// The note is held to the row: narrow, enlarged and in Russian its label
    /// elides inside the row rather than overflowing, and the note still opens
    /// its help.
    #[gpui::test]
    fn the_missing_note_stays_in_a_narrow_enlarged_russian_row_and_still_opens(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("ru-RU");
        vcx.update(|_, cx| LspStore::mark_missing_for_test("rust-analyzer", cx));
        open_rs(&app, &mut vcx, PATH);
        enlarge_ui(&mut vcx, 6.);
        let full = crate::ui::i18n::t(crate::ui::i18n::L10nKey::LspServerMissing);
        let label = gpui::ElementId::from("status-lsp-missing-label");
        let mut elided = false;
        for width in [900., 500., 320., 240., 200.] {
            crate::ui::text_elision::observe::clear();
            at(&mut vcx, width);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            let note = vcx
                .debug_bounds("editor-status-lsp-missing")
                .unwrap_or_else(|| panic!("{width}: the note is not drawn"));
            assert!(inside(note, row), "{width}: {note:?} is outside {row:?}");
            let shown = crate::ui::text_elision::observe::label(&label)
                .unwrap_or_else(|| panic!("{width}: the label was not drawn"));
            elided |= &*shown.text != full;
        }
        assert!(elided, "no tested width made the label give way");

        // Below the product's own minimum window there is no promise of a
        // target (at 200 the row has no room at all); at a real width the
        // note is a genuine target and opens its help.
        at(&mut vcx, 960.);
        let row = vcx.debug_bounds("editor-status").expect("the status row");
        let note = vcx
            .debug_bounds("editor-status-lsp-missing")
            .expect("the note at 960");
        assert!(inside(note, row), "960: {note:?} is outside {row:?}");
        assert!(
            note.size.width > px(0.) && note.size.height > px(0.),
            "960: no target: {note:?}"
        );
        vcx.simulate_click(note.center(), Modifiers::none());
        paint(&mut vcx);
        paint(&mut vcx);
        assert!(
            vcx.debug_bounds("editor-status-lsp-help").is_some(),
            "the help did not open (note {note:?}, row {row:?}, frame {:?})",
            observe::frame()
        );
        vcx.simulate_keystrokes("escape");
        paint(&mut vcx);
        assert!(
            vcx.debug_bounds("editor-status-lsp-help").is_none(),
            "Escape closes it"
        );
    }

    /// The missing-server note is a real control: a click opens what tty59
    /// tried, and Escape puts it away.
    #[gpui::test]
    fn the_missing_server_note_opens_its_help_and_escape_closes_it(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        vcx.update(|_, cx| LspStore::mark_missing_for_test("rust-analyzer", cx));
        open_rs(&app, &mut vcx, PATH);
        at(&mut vcx, 1200.);
        let note = vcx
            .debug_bounds("editor-status-lsp-missing")
            .expect("the missing-server note is drawn");
        assert!(vcx.debug_bounds("editor-status-lsp-help").is_none());

        vcx.simulate_click(note.center(), Modifiers::none());
        paint(&mut vcx);
        paint(&mut vcx);
        assert!(
            vcx.debug_bounds("editor-status-lsp-help").is_some(),
            "a click opens the help"
        );

        vcx.simulate_keystrokes("escape");
        paint(&mut vcx);
        assert!(vcx.debug_bounds("editor-status-lsp-help").is_none());

        let help = crate::ui::lsp::editor::missing_help("pyright");
        assert!(
            help.contains("pyright-langserver, basedpyright-langserver, pylsp"),
            "{help}"
        );
    }

    /// The keep/drop boundary of the six formatting readouts, in the real
    /// layout and read from nothing but its bounds: where all six fit the row
    /// the last one ends at the content edge, never inside the row's inset,
    /// and where the layout's own widths do fit none of them is dropped.
    /// GPUI sizes every text node up to a whole pixel, so a fractional sum is
    /// the thing this holds the fit to.
    #[gpui::test]
    fn the_bar_keeps_its_inset_at_the_keep_drop_boundary(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        vcx.update(|_, cx| {
            let mut config = cx.global::<Config>().clone();
            config.editor_lsp = false;
            cx.set_global(config);
        });
        open_rs(&app, &mut vcx, PATH);
        // Larger type, within the supported range, and the right panel open on
        // Info at 240: a narrow document column inside a window the product
        // makes, so the boundary falls between 720 and 1600.
        enlarge_ui(&mut vcx, 6.);
        vcx.update(|_, cx| {
            let mut config = cx.global::<Config>().clone();
            config.right_panel_width = 240.;
            cx.set_global(config);
        });
        app.update_in(&mut vcx, |app, _, cx| {
            app.right_panel_width.set(240.);
            app.set_right_panel_tab(RightPanelTab::Info, cx);
        });
        at(&mut vcx, 1600.);
        for name in ITEMS {
            assert!(vcx.debug_bounds(name).is_some(), "{name} is drawn wide");
        }
        let wrap = vcx.debug_bounds("editor-status-wrap").unwrap();
        let cursor = vcx.debug_bounds("editor-status-cursor").unwrap();
        let language = vcx.debug_bounds("editor-status-language").unwrap();
        // The readouts that act reach `ext` past their text under negative
        // margins; the layout's own gap between two of them says how far.
        let ext = (px(ROW_GAP) - (cursor.left() - wrap.right())) / 2.;
        // What the six and their gaps need, and the spacer's gap beside them.
        let need = language.right() - (wrap.left() + ext) + px(ROW_GAP);

        let inner_at = |vcx: &mut VisualTestContext, width: f32| -> Pixels {
            at(vcx, width);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            row.size.width - px(2. * crate::ui::app::CONTENT_INSET)
        };
        let (at_min, at_max) = (inner_at(&mut vcx, 720.), inner_at(&mut vcx, 1600.));
        assert!(
            at_min < need,
            "the boundary is not above the product's narrowest window: need {need:?}, \
             inner {at_min:?} at 720 and {at_max:?} at 1600"
        );
        assert!(
            at_max >= need,
            "the six do not fit even at 1600: need {need:?}, inner {at_min:?} at 720 and {at_max:?} at 1600"
        );
        // The narrowest whole window width whose row holds them.
        let (mut lo, mut hi) = (720, 1600);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if inner_at(&mut vcx, mid as f32) >= need {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        assert!(hi - 6 >= 720, "the sweep stays inside native windows");

        let (mut kept_all, mut dropped_some) = (false, false);
        for width in (hi - 6)..=(hi + 6) {
            let inner = inner_at(&mut vcx, width as f32);
            let row = vcx.debug_bounds("editor-status").expect("the status row");
            let drawn = ITEMS
                .iter()
                .filter(|n| vcx.debug_bounds(n).is_some())
                .count();
            if let Some(language) = vcx.debug_bounds("editor-status-language") {
                assert!(
                    language.right() <= row.right() - px(crate::ui::app::CONTENT_INSET) + px(0.01),
                    "{width}: the last readout ends at {:?}, inside the inset of {row:?} \
                     (inner {inner:?}, need {need:?}, drawn {drawn})",
                    language.right()
                );
            }
            if inner >= need - px(0.01) {
                assert_eq!(
                    drawn,
                    ITEMS.len(),
                    "{width}: room for all six (inner {inner:?}, need {need:?}) but {drawn} drawn"
                );
            }
            kept_all |= drawn == ITEMS.len();
            dropped_some |= drawn < ITEMS.len();
        }
        assert!(kept_all && dropped_some, "the sweep crossed the boundary");
    }

    /// The missing-server help in a real window: opened by a real click, its
    /// title and body measured from the layout, in English, Russian and
    /// Russian at the largest type. The row it hangs from keeps its words on
    /// one line; the help is prose and has to wrap inside its card.
    fn missing_help_wraps(cx: &mut TestAppContext, locale: &str, enlarge: f32) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale(locale);
        vcx.update(|_, cx| LspStore::mark_missing_for_test("rust-analyzer", cx));
        open_rs(&app, &mut vcx, PATH);
        if enlarge > 0. {
            enlarge_ui(&mut vcx, enlarge);
        }
        at(&mut vcx, 960.);
        let note = vcx
            .debug_bounds("editor-status-lsp-missing")
            .expect("the note is drawn");
        vcx.simulate_click(note.center(), Modifiers::none());
        paint(&mut vcx);
        paint(&mut vcx);
        let help = vcx
            .debug_bounds("editor-status-lsp-help")
            .expect("a click opens the help");
        let title = vcx
            .debug_bounds("editor-status-lsp-help-title")
            .expect("the title is drawn");
        let body = vcx
            .debug_bounds("editor-status-lsp-help-body")
            .expect("the body is drawn");
        let viewport = vcx.update(|window, _| window.viewport_size());
        let said = format!(
            "{locale} +{enlarge}: help {help:?}, title {title:?}, body {body:?}, window {viewport:?}"
        );
        assert!(
            help.left() >= px(0.) && help.right() <= viewport.width,
            "the help is outside the window: {said}"
        );
        assert!(
            body.size.width <= help.size.width + px(0.5)
                && body.size.width <= px(crate::ui::lsp::editor::HELP_MAX_W) + px(0.5),
            "the body is wider than its card: {said}"
        );
        // The title is one line, so it is the line height; prose this long
        // takes more than one of them.
        assert!(
            body.size.height >= title.size.height * 1.5,
            "the body is on one line: {said}"
        );
    }

    #[gpui::test]
    fn the_missing_help_wraps_inside_its_card_in_english(cx: &mut TestAppContext) {
        missing_help_wraps(cx, "en", 0.);
    }

    #[gpui::test]
    fn the_missing_help_wraps_inside_its_card_in_russian(cx: &mut TestAppContext) {
        missing_help_wraps(cx, "ru-RU", 0.);
    }

    #[gpui::test]
    fn the_missing_help_wraps_inside_its_card_in_russian_at_the_largest_type(
        cx: &mut TestAppContext,
    ) {
        missing_help_wraps(cx, "ru-RU", 6.);
    }

    /// A slot's key holds the features as well as the text, family and size:
    /// each alone remeasures it, and the same key does not.
    #[gpui::test]
    fn a_slot_remeasures_when_only_its_features_family_or_size_change(cx: &mut TestAppContext) {
        let (_app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        vcx.update(|window, cx| {
            let face = Face::new(window, cx);
            let mut memo = Memo::default();
            let text = SharedString::from("Ln 12, Col 40");
            let before = measures();
            memo.text(Part::Cursor, &text, &face, window);
            assert_eq!(measures(), before + 1, "the first look measures");
            memo.text(Part::Cursor, &text, &face, window);
            assert_eq!(measures(), before + 1, "the same key is a hit");

            let mut features = face.clone();
            features.tabular = FontFeatures(Arc::new(vec![("zero".to_string(), 1)]));
            memo.text(Part::Cursor, &text, &features, window);
            assert_eq!(measures(), before + 2, "features alone are a new key");

            // Each key differs from the one just before it in one field only.
            let mut family = features.clone();
            family.family = "Another Family".into();
            memo.text(Part::Cursor, &text, &family, window);
            assert_eq!(measures(), before + 3, "family alone is a new key");

            let mut bigger = family.clone();
            bigger.size += px(1.);
            memo.text(Part::Cursor, &text, &bigger, window);
            assert_eq!(measures(), before + 4, "size alone is a new key");
        });
    }

    /// A frame whose keys are unchanged shapes nothing; a changed text
    /// measures its own slot; a changed face or font measures them again.
    #[gpui::test]
    fn a_steady_frame_measures_nothing_and_a_changed_key_measures_its_slot(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        open_rs(&app, &mut vcx, PATH);
        at(&mut vcx, 1200.);
        redraw(&app, &mut vcx);
        let base = measures();
        assert!(base > 0, "the first frames measured the readouts");

        redraw(&app, &mut vcx);
        redraw(&app, &mut vcx);
        assert_eq!(measures(), base, "an unchanged frame measured something");

        app.update_in(&mut vcx, |app, window, cx| {
            let input = app.active_buffer().unwrap().input.clone();
            input.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(2, 0), window, cx);
            });
        });
        paint(&mut vcx);
        redraw(&app, &mut vcx);
        assert_eq!(
            measures(),
            base + 1,
            "only the cursor's text changed, so only its slot measures"
        );

        let before = measures();
        enlarge_ui(&mut vcx, 4.);
        redraw(&app, &mut vcx);
        assert!(
            measures() > before + 1,
            "a larger face measured every slot again"
        );

        let before = measures();
        vcx.update(|_, cx| {
            gpui_component::Theme::global_mut(cx).font_family = "Another Family".into();
        });
        redraw(&app, &mut vcx);
        assert!(
            measures() > before + 1,
            "another family measured every slot again"
        );
    }

    /// Go to File's hint reads the installed keymap's first binding for it:
    /// the shipped one, else the first the config adds; an exact list
    /// replaces; empty unbinds; every stroke of a sequence is spelled.
    #[gpui::test]
    fn the_hint_reads_the_first_installed_quick_open_binding(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
        });
        let chord =
            |cx: &mut TestAppContext| cx.update(|cx| crate::ui::code_editor::quick_open_chord(cx));
        let configure = |cx: &mut TestAppContext, value: Option<KeybindingOverride>| {
            cx.update(|cx| {
                let config = cx.global_mut::<Config>();
                config.keybindings.remove("QuickOpenFile");
                if let Some(value) = value {
                    config.keybindings.insert("QuickOpenFile".into(), value);
                }
                crate::ui::keymap::rebind(cx);
            });
        };
        // Cmd+O on macOS; unbound everywhere else, and nothing is invented.
        let shipped = cfg!(target_os = "macos").then(|| spelled("secondary-o"));
        assert_eq!(chord(cx), shipped);

        // Beside the shipped chord, never ahead of it.
        configure(cx, Some(KeybindingOverride::Add("ctrl-alt-j".into())));
        assert_eq!(
            chord(cx),
            shipped.clone().or_else(|| Some(spelled("ctrl-alt-j")))
        );

        // Exactly these, the first of them shown.
        configure(
            cx,
            Some(KeybindingOverride::Exact(vec![
                "ctrl-alt-k".into(),
                "ctrl-alt-m".into(),
            ])),
        );
        assert_eq!(chord(cx), Some(spelled("ctrl-alt-k")));

        configure(cx, Some(KeybindingOverride::Exact(Vec::new())));
        assert_eq!(chord(cx), None, "an empty list unbinds");
        configure(cx, Some(KeybindingOverride::Add(String::new())));
        assert_eq!(chord(cx), None, "an empty string unbinds");

        configure(
            cx,
            Some(KeybindingOverride::Exact(vec!["ctrl-k ctrl-o".into()])),
        );
        assert_eq!(
            chord(cx),
            Some(format!("{} {}", spelled("ctrl-k"), spelled("ctrl-o"))),
            "every stroke of the sequence is spelled"
        );
    }

    /// The empty panel says what the keymap says, in the words it passed to
    /// the elements: unbound, the file tree alone; bound, the first chord and
    /// the tree as the alternative.
    #[gpui::test]
    fn the_empty_panel_says_the_keymaps_chord(cx: &mut TestAppContext) {
        use super::observe;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        crate::ui::i18n::set_locale("en");
        app.update_in(&mut vcx, |app, window, cx| {
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.focus_active(window, cx);
        });

        vcx.update(|_, cx| cx.clear_key_bindings());
        observe::clear();
        redraw(&app, &mut vcx);
        assert_eq!(
            observe::last("editor-empty-open").as_deref(),
            Some("Pick a file in the file tree")
        );
        assert_eq!(
            observe::last("editor-empty-tree"),
            None,
            "the file tree is the one line, not said twice"
        );
        assert!(vcx.debug_bounds("editor-empty-open").is_some());
        assert!(vcx.debug_bounds("editor-empty-tree").is_none());

        vcx.update(|_, cx| {
            cx.bind_keys([
                KeyBinding::new("ctrl-alt-j", QuickOpenFile, None),
                KeyBinding::new("ctrl-alt-k", QuickOpenFile, None),
            ])
        });
        observe::clear();
        redraw(&app, &mut vcx);
        assert_eq!(
            observe::last("editor-empty-open"),
            Some(format!("Open a file with {}", spelled("ctrl-alt-j"))),
            "the first installed binding, not the newest and not a literal"
        );
        assert_eq!(
            observe::last("editor-empty-tree").as_deref(),
            Some("or pick one in the file tree")
        );
        assert!(vcx.debug_bounds("editor-empty-tree").is_some());
    }
}
